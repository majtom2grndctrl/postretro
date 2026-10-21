// Host intake and application of client weapon-switch declarations.
// See: context/lib/networking.md §Combat authority · §Host input command queue

use postretro_entities::{EntityId, EntityRegistry};
use postretro_net::transport::NetServer;
use postretro_net::wire::{
    self, ClientSwitchDeclaration, ServerMessage, ServerSwitchAccepted, ServerSwitchRefused,
};

use crate::command_queue::{HostCommandQueues, MovementOwners, SwitchDelivery, WeaponOwners};

/// Retain a client's switch declaration in its command queue's switch lane.
/// The lane releases it on a later fixed tick, in client-tick order with the
/// client's retained starts and presses; see [`host_apply_switch_deliveries`].
/// A client that owns no pawn gets an immediate refusal for the declaration's
/// id and slot, through the same reply a refused slot uses: the client resolves
/// its pending chain strictly in declaration order, so a dropped declaration
/// would leave the front unresolved and discard every later outcome.
pub fn host_retain_switch_declaration(
    server: &mut NetServer,
    command_queues: &mut HostCommandQueues,
    owners: &MovementOwners,
    client_id: u64,
    declaration: ClientSwitchDeclaration,
) {
    if owners.iter().any(|(_, owner)| owner == client_id) {
        command_queues.retain_switch(client_id, declaration);
    } else {
        send_switch_refusal(
            server,
            client_id,
            declaration.declaration_id,
            declaration.slot,
        );
    }
}

/// Apply every switch the lanes released this fixed tick, after command
/// resolution and before the simulation, replying to each client in arrival
/// order. Validation is the host's: the client owns its equip presentation,
/// while the host owns the committed slot used by snapshots and server-side
/// systems. Each outcome is owner-private reliable Input, in the same ordered
/// stream as the client's commands, because a snapshot cannot recover a
/// stationary client with no later baseline change to compare against.
pub fn host_apply_switch_deliveries(
    registry: &mut EntityRegistry,
    server: &mut NetServer,
    owners: &MovementOwners,
    command_queues: &mut HostCommandQueues,
    weapon_owners: &mut WeaponOwners,
    mod_block_during_reload: bool,
) {
    let owned: Vec<(EntityId, u64)> = owners.iter().collect();
    for (pawn, client_id) in owned {
        for delivery in command_queues.take_switch_deliveries(client_id) {
            let (declaration, accepted) = match delivery {
                SwitchDelivery::Apply(declaration) => (
                    declaration,
                    apply_host_switch_declaration(
                        registry,
                        pawn,
                        weapon_owners,
                        usize::from(declaration.slot),
                        mod_block_during_reload,
                    ) == HostSwitchDecision::Accepted,
                ),
                SwitchDelivery::Refuse(declaration) => (declaration, false),
            };
            if accepted {
                send_switch_accepted(
                    server,
                    client_id,
                    declaration.declaration_id,
                    declaration.slot,
                );
            } else {
                send_switch_refusal(
                    server,
                    client_id,
                    declaration.declaration_id,
                    declaration.slot,
                );
            }
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum HostSwitchDecision {
    Accepted,
    Refused,
}

pub(crate) fn apply_host_switch_declaration(
    registry: &mut EntityRegistry,
    pawn: EntityId,
    weapon_owners: &mut WeaponOwners,
    target_slot: usize,
    mod_block_during_reload: bool,
) -> HostSwitchDecision {
    let Some((mut inventory, active_changed)) =
        crate::sim::normalize_wieldable_inventory(registry, pawn)
    else {
        return HostSwitchDecision::Refused;
    };
    if active_changed {
        weapon_owners.mark_attachment_dirty(pawn);
    }
    let Some(_target) = inventory.wieldables.get(target_slot).copied().flatten() else {
        return HostSwitchDecision::Refused;
    };
    if target_slot == inventory.active_slot {
        return HostSwitchDecision::Accepted;
    }

    let reload_blocks_switch = inventory
        .active_wieldable()
        .and_then(|active| {
            registry
                .get_component::<postretro_entities::components::weapon::WeaponComponent>(active)
                .ok()
        })
        .is_some_and(|weapon| {
            weapon
                .block_during_reload
                .unwrap_or(mod_block_during_reload)
                && weapon.state.is_reload_activity()
        });
    if reload_blocks_switch {
        return HostSwitchDecision::Refused;
    }

    if let Some(outgoing) = inventory.active_wieldable()
        && let Ok(postretro_entities::ComponentValue::Weapon(component)) =
            registry.get_component_value_mut(outgoing, postretro_entities::ComponentKind::Weapon)
    {
        component.cancel_activation();
    }
    inventory.active_slot = target_slot;
    inventory.switch_target = None;
    inventory.switch_origin = None;
    let _ = registry.set_component(pawn, inventory);
    weapon_owners.mark_attachment_dirty(pawn);
    HostSwitchDecision::Accepted
}

fn send_switch_accepted(server: &mut NetServer, client_id: u64, declaration_id: u32, slot: u8) {
    server.send_input(
        client_id,
        wire::encode(&ServerMessage::SwitchAccepted(ServerSwitchAccepted {
            declaration_id,
            slot,
        })),
    );
}

fn send_switch_refusal(server: &mut NetServer, client_id: u64, declaration_id: u32, slot: u8) {
    server.send_input(
        client_id,
        wire::encode(&ServerMessage::SwitchRefused(ServerSwitchRefused {
            declaration_id,
            slot,
        })),
    );
}

#[cfg(test)]
mod tests {
    use std::net::{Ipv4Addr, UdpSocket};
    use std::time::Duration;

    use postretro_net::transport::NetClient;

    use super::*;

    #[test]
    fn declaration_from_client_without_owner_entry_is_refused_once() {
        let server_socket = UdpSocket::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
        let address = server_socket.local_addr().unwrap();
        let mut server =
            NetServer::new(server_socket, address, 2, Duration::from_secs(1), None).unwrap();
        let client_socket = UdpSocket::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
        let mut client = NetClient::new(
            client_socket,
            address,
            7,
            Duration::from_secs(1),
            None,
            None,
        )
        .unwrap();
        server.add_relay_connection(7, None);
        client.set_connected();

        let mut command_queues = HostCommandQueues::new();
        host_retain_switch_declaration(
            &mut server,
            &mut command_queues,
            &MovementOwners::default(),
            7,
            ClientSwitchDeclaration {
                declaration_id: 41,
                slot: 3,
                client_tick: 100,
            },
        );

        for packet in server.packets_to_send(7) {
            client.process_packet(&packet);
        }
        let replies: Vec<ServerMessage> = client
            .drain_input()
            .iter()
            .map(|bytes| wire::decode(bytes).unwrap())
            .collect();
        assert_eq!(
            replies,
            vec![ServerMessage::SwitchRefused(ServerSwitchRefused {
                declaration_id: 41,
                slot: 3,
            })]
        );
    }
}
