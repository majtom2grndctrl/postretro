// Host intake and application of client weapon-switch declarations.
// See: context/lib/networking.md §Combat authority · §Host input command queue

#[cfg(test)]
use postretro_entities::{EntityId, EntityRegistry};
use postretro_net::transport::NetServer;
use postretro_net::wire::{
    self, ClientSwitchDeclaration, ServerMessage, ServerSwitchAccepted, ServerSwitchRefused,
};

#[cfg(test)]
use crate::command_queue::WeaponOwners;
use crate::command_queue::{HostCommandQueues, MovementOwners};

/// Retain a client's switch declaration in its command queue's switch lane.
/// The lane releases it on a later fixed tick, in client-tick order with the
/// client's retained starts and presses; see
/// [`crate::host_resolve_remote_commands_with_switches`].
/// A client that owns no pawn gets an immediate refusal for the declaration's
/// id and slot, through the same reply a refused slot uses: the client resolves
/// its pending chain strictly in declaration order, so a dropped declaration
/// would leave the front unresolved and discard every later outcome.
///
/// The immediate refusal assumes a client's owner entry goes only with its
/// switch lane, at participation exit or slot replacement. Were the owner
/// removed alone, this refusal could overtake a declaration the lane still
/// retains and answer the chain out of order.
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

#[cfg(test)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SettlementOnlySwitchDecision {
    Accepted,
    Refused,
}

/// Immediate fixture for client outcome-chain settlement tests only.
/// It deliberately omits lower/raise timing and must never drive host timing
/// proofs; those use `host_resolve_remote_commands_with_switches`.
#[cfg(test)]
pub(crate) fn apply_settlement_only_switch_fixture(
    registry: &mut EntityRegistry,
    pawn: EntityId,
    weapon_owners: &mut WeaponOwners,
    target_slot: usize,
    mod_block_during_reload: bool,
) -> SettlementOnlySwitchDecision {
    let Some((mut inventory, active_changed)) =
        crate::sim::normalize_wieldable_inventory(registry, pawn)
    else {
        return SettlementOnlySwitchDecision::Refused;
    };
    if active_changed {
        weapon_owners.mark_attachment_dirty(pawn);
    }
    let Some(_target) = inventory.wieldables.get(target_slot).copied().flatten() else {
        return SettlementOnlySwitchDecision::Refused;
    };
    if target_slot == inventory.active_slot {
        return SettlementOnlySwitchDecision::Accepted;
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
        return SettlementOnlySwitchDecision::Refused;
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
    SettlementOnlySwitchDecision::Accepted
}

pub(crate) fn send_switch_accepted(
    server: &mut NetServer,
    client_id: u64,
    declaration_id: u32,
    slot: u8,
) {
    server.send_input(
        client_id,
        wire::encode(&ServerMessage::SwitchAccepted(ServerSwitchAccepted {
            declaration_id,
            slot,
        })),
    );
}

pub(crate) fn send_switch_refusal(
    server: &mut NetServer,
    client_id: u64,
    declaration_id: u32,
    slot: u8,
) {
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
                kind: postretro_net::wire::SwitchDeclarationKind::Switch,
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
        assert_eq!(
            command_queues.retained_switch_count(7),
            0,
            "the refused declaration is not retained"
        );
    }
}
