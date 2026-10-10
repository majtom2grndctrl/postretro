// Host intake and application of client weapon-switch declarations.
// See: context/lib/networking.md §Combat authority · §Host input command queue

use postretro_entities::{EntityId, EntityRegistry};
use postretro_net::transport::NetServer;
use postretro_net::wire::{
    self, ClientSwitchDeclaration, ServerControlMessage, ServerSwitchAccepted, ServerSwitchRefused,
};

use crate::command_queue::{HostCommandQueues, MovementOwners, SwitchDelivery, WeaponOwners};

/// Retain a client's switch declaration in its command queue's switch lane.
/// The lane releases it on a later fixed tick, in client-tick order with the
/// client's retained starts and presses; see [`host_apply_switch_deliveries`].
/// A client that owns no pawn gets no reply, as a declaration has no meaning
/// before its slot owns one.
pub fn host_retain_switch_declaration(
    command_queues: &mut HostCommandQueues,
    owners: &MovementOwners,
    client_id: u64,
    declaration: ClientSwitchDeclaration,
) {
    if owners.iter().any(|(_, owner)| owner == client_id) {
        command_queues.retain_switch(client_id, declaration);
    }
}

/// Apply every switch the lanes released this fixed tick, after command
/// resolution and before the simulation, replying to each client in arrival
/// order. Validation is the host's: the client owns its equip presentation,
/// while the host owns the committed slot used by snapshots and server-side
/// systems. A refusal is owner-private reliable Control because a snapshot
/// cannot recover a stationary client with no later baseline change to
/// compare against.
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
    server.send_control(
        client_id,
        wire::encode(&ServerControlMessage::SwitchAccepted(
            ServerSwitchAccepted {
                declaration_id,
                slot,
            },
        )),
    );
}

fn send_switch_refusal(server: &mut NetServer, client_id: u64, declaration_id: u32, slot: u8) {
    server.send_control(
        client_id,
        wire::encode(&ServerControlMessage::SwitchRefused(ServerSwitchRefused {
            declaration_id,
            slot,
        })),
    );
}
