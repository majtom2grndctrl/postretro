// Remote equip follows client ticks; host machine timers cannot commit it.
// See: context/lib/networking.md §Combat authority
use crate::command_queue::{
    HostCommandQueues, MovementOwners, ResolvedPawnCommand, SwitchDelivery, WeaponOwners,
};
use crate::host_switch::{send_switch_accepted, send_switch_refusal};
use crate::prediction::client_tick_le;
use postretro_entities::components::{inventory::Inventory, weapon::WeaponComponent};
use postretro_entities::{EntityId, EntityRegistry};
use postretro_net::transport::NetServer;
use postretro_net::wire::{ClientSwitchDeclaration, SwitchDeclarationKind};

#[derive(Debug, Clone, Copy)]
pub(crate) struct PendingLower {
    pub declaration: ClientSwitchDeclaration,
    pub origin_slot: usize,
    pub outgoing: EntityId,
    pub target: EntityId,
    pub repoint_tick: u32,
}
#[derive(Debug, Clone, Copy)]
struct RaiseGate {
    weapon: EntityId,
    repoint_tick: u32,
    fire_tick: u32,
}
#[derive(Debug, Default)]
pub(crate) struct HostSwitchTiming {
    pub lower: Option<PendingLower>,
    raise: Option<RaiseGate>,
    deferred_begin: Option<EntityId>,
}
/// The independent lanes before starts and presses drain this resolution.
#[derive(Debug, Clone, Copy)]
pub(crate) struct SwitchFrontier {
    pub cursor: Option<u32>,
    pub newest: Option<u32>,
    pub oldest_start: Option<u32>,
    pub oldest_press: Option<u32>,
    pub live_horizon: Option<u32>,
}
impl SwitchFrontier {
    pub fn ready(self, tick: u32) -> bool {
        self.cursor
            .is_some_and(|cursor| client_tick_le(tick, cursor))
            && self
                .newest
                .is_some_and(|newest| client_tick_le(tick, newest))
            && !self
                .oldest_start
                .is_some_and(|older| !client_tick_le(tick, older))
            && !self
                .oldest_press
                .is_some_and(|older| !client_tick_le(tick, older))
            && self
                .live_horizon
                .is_none_or(|horizon| client_tick_le(tick, horizon))
    }
}
pub(crate) enum SwitchAction {
    Declaration(SwitchDelivery),
    Repoint,
}
pub(crate) type SwitchApply<'a> =
    dyn FnMut(&mut HostSwitchTiming, SwitchAction, SwitchFrontier) + 'a;
impl HostSwitchTiming {
    fn reload_refused(&self, weapon: EntityId, tick: u32) -> bool {
        self.lower.is_some_and(|lower| lower.outgoing == weapon)
            || self.raise.is_some_and(|raise| {
                raise.weapon == weapon
                    && client_tick_le(raise.repoint_tick, tick)
                    && client_tick_le(tick, raise.fire_tick)
            })
    }
    fn start_refused(&self, weapon: EntityId, tick: u32) -> bool {
        self.lower.is_some_and(|lower| lower.outgoing == weapon)
            || self.raise.is_some_and(|raise| {
                raise.weapon == weapon
                    && client_tick_le(raise.repoint_tick, tick)
                    && !client_tick_le(raise.fire_tick, tick)
            })
    }
}
fn reply(
    server: &mut NetServer,
    client: u64,
    declaration: ClientSwitchDeclaration,
    accepted: bool,
) {
    if accepted {
        send_switch_accepted(server, client, declaration.declaration_id, declaration.slot);
    } else {
        send_switch_refusal(server, client, declaration.declaration_id, declaration.slot);
    }
}
fn abandon(registry: &mut EntityRegistry, pawn: EntityId, lower: PendingLower) {
    crate::sim::refuse_local_wieldable_switch(
        registry,
        pawn,
        usize::from(lower.declaration.slot),
        lower.origin_slot,
    );
    if let Ok(postretro_entities::ComponentValue::Weapon(weapon)) =
        registry.get_component_value_mut(lower.outgoing, postretro_entities::ComponentKind::Weapon)
    {
        crate::sim::finish_wieldable_lower(weapon);
    }
}
/// Release and liveness are checked before commands and after simulation. A
/// slot refilled in the same tick is not this declaration's target instance.
fn repair(
    timing: &mut HostSwitchTiming,
    registry: &mut EntityRegistry,
    server: &mut NetServer,
    pawn: EntityId,
    client: u64,
    weapon_owners: &mut WeaponOwners,
) {
    if timing.raise.is_some_and(|gate| {
        !registry
            .get_component::<Inventory>(pawn)
            .is_ok_and(|inventory| inventory.wieldables.contains(&Some(gate.weapon)))
            || registry
                .get_component::<WeaponComponent>(gate.weapon)
                .is_err()
    }) {
        timing.raise = None;
    }
    let Some(lower) = timing.lower else {
        return;
    };
    let normalized = crate::sim::normalize_wieldable_inventory(registry, pawn);
    if normalized.as_ref().is_some_and(|(_, changed)| *changed) {
        weapon_owners.mark_attachment_dirty(pawn);
    }
    let accepted = normalized.as_ref().is_some_and(|(inventory, _)| {
        inventory
            .wieldables
            .get(usize::from(lower.declaration.slot))
            .copied()
            .flatten()
            == Some(lower.target)
            && !inventory.wieldables.contains(&Some(lower.outgoing))
            && inventory.active_wieldable() == Some(lower.target)
    });
    let still_lowering = normalized.as_ref().is_some_and(|(inventory, _)| {
        inventory.active_slot == lower.origin_slot
            && inventory
                .wieldables
                .get(lower.origin_slot)
                .copied()
                .flatten()
                == Some(lower.outgoing)
            && inventory.switch_target == Some(usize::from(lower.declaration.slot))
            && inventory
                .wieldables
                .get(usize::from(lower.declaration.slot))
                .copied()
                .flatten()
                == Some(lower.target)
    });
    if still_lowering {
        return;
    }
    timing.lower = None;
    timing.deferred_begin = None;
    if !accepted {
        abandon(registry, pawn, lower);
    }
    weapon_owners.mark_attachment_dirty(pawn);
    reply(server, client, lower.declaration, accepted);
}
fn repoint(
    timing: &mut HostSwitchTiming,
    registry: &mut EntityRegistry,
    server: &mut NetServer,
    pawn: EntityId,
    client: u64,
    weapon_owners: &mut WeaponOwners,
    tick_dt: f32,
) {
    repair(timing, registry, server, pawn, client, weapon_owners);
    let Some(lower) = timing.lower.take() else {
        return;
    };
    timing.deferred_begin = None;
    let Ok(mut inventory) = registry.get_component::<Inventory>(pawn).cloned() else {
        reply(server, client, lower.declaration, false);
        return;
    };
    if let Ok(postretro_entities::ComponentValue::Weapon(weapon)) =
        registry.get_component_value_mut(lower.outgoing, postretro_entities::ComponentKind::Weapon)
    {
        crate::sim::finish_wieldable_lower(weapon);
    }
    inventory.active_slot = usize::from(lower.declaration.slot);
    inventory.switch_target = None;
    inventory.switch_origin = None;
    let _ = registry.set_component(pawn, inventory);
    let raise_ms = registry
        .get_component::<WeaponComponent>(lower.target)
        .map_or(0, |weapon| weapon.raise_ms);
    timing.raise = Some(RaiseGate {
        weapon: lower.target,
        repoint_tick: lower.repoint_tick,
        fire_tick: lower
            .repoint_tick
            .wrapping_add(crate::sim::wieldable_equip_ticks(raise_ms, tick_dt)),
    });
    weapon_owners.mark_attachment_dirty(pawn);
    reply(server, client, lower.declaration, true);
}
#[allow(clippy::too_many_arguments)]
fn apply(
    timing: &mut HostSwitchTiming,
    action: SwitchAction,
    frontier: SwitchFrontier,
    registry: &mut EntityRegistry,
    server: &mut NetServer,
    pawn: EntityId,
    client: u64,
    weapon_owners: &mut WeaponOwners,
    block_reload: bool,
    tick_dt: f32,
) {
    let SwitchAction::Declaration(delivery) = action else {
        repoint(
            timing,
            registry,
            server,
            pawn,
            client,
            weapon_owners,
            tick_dt,
        );
        return;
    };
    let (declaration, malformed) = match delivery {
        SwitchDelivery::Apply(declaration) => (declaration, false),
        SwitchDelivery::Refuse(declaration) => (declaration, true),
    };
    let mut original_repoint = None;
    if let Some(parked) = timing.lower {
        if client_tick_le(declaration.client_tick, parked.repoint_tick) {
            timing.lower = None;
            original_repoint = Some(parked);
            reply(server, client, parked.declaration, false);
        } else {
            repoint(
                timing,
                registry,
                server,
                pawn,
                client,
                weapon_owners,
                tick_dt,
            );
        }
    }
    if malformed {
        if let Some(parked) = original_repoint {
            abandon(registry, pawn, parked);
        }
        timing.deferred_begin = None;
        reply(server, client, declaration, false);
        return;
    }
    let Some((mut inventory, active_changed)) =
        crate::sim::normalize_wieldable_inventory(registry, pawn)
    else {
        reply(server, client, declaration, false);
        return;
    };
    if active_changed {
        weapon_owners.mark_attachment_dirty(pawn);
    }
    let slot = usize::from(declaration.slot);
    let target = inventory.wieldables.get(slot).copied().flatten();
    let outgoing = inventory.active_wieldable();
    let blocked = outgoing
        .and_then(|weapon| registry.get_component::<WeaponComponent>(weapon).ok())
        .is_some_and(|weapon| {
            weapon.block_during_reload.unwrap_or(block_reload) && weapon.state.is_reload_activity()
        });
    if target.is_none() || (slot != inventory.active_slot && blocked) {
        if let Some(parked) = original_repoint {
            abandon(registry, pawn, parked);
        }
        timing.deferred_begin = None;
        reply(server, client, declaration, false);
        return;
    }
    if slot == inventory.active_slot {
        if let Some(parked) = original_repoint {
            abandon(registry, pawn, parked);
            timing.deferred_begin = None;
        }
        // A lagged owner projection can resync an already-held active weapon.
        // Accept without interrupting its charge, burst or remaining raise gate.
        reply(server, client, declaration, true);
        return;
    }
    if declaration.kind == SwitchDeclarationKind::Resync {
        if let Some(parked) = original_repoint {
            abandon(registry, pawn, parked);
        }
        if let Some(outgoing) = outgoing
            && let Ok(postretro_entities::ComponentValue::Weapon(weapon)) = registry
                .get_component_value_mut(outgoing, postretro_entities::ComponentKind::Weapon)
        {
            weapon.cancel_activation();
        }
        inventory.active_slot = slot;
        inventory.switch_target = None;
        inventory.switch_origin = None;
        let _ = registry.set_component(pawn, inventory);
        timing.raise = None;
        timing.deferred_begin = None;
        weapon_owners.mark_attachment_dirty(pawn);
        reply(server, client, declaration, true);
        return;
    }
    let (Some(outgoing), Some(target)) = (outgoing, target) else {
        reply(server, client, declaration, false);
        return;
    };
    let Ok(component) = registry.get_component::<WeaponComponent>(outgoing) else {
        reply(server, client, declaration, false);
        return;
    };
    let steps = crate::sim::wieldable_equip_ticks(component.lower_ms, tick_dt);
    let repoint_tick = original_repoint.map_or_else(
        || declaration.client_tick.wrapping_add(steps - 1),
        |lower| lower.repoint_tick,
    );
    let defer =
        !frontier.ready(repoint_tick) && crate::sim::wieldable_reload_due(component, tick_dt);
    inventory.switch_origin = Some(inventory.active_slot);
    inventory.switch_target = Some(slot);
    let lower = PendingLower {
        declaration,
        origin_slot: inventory.active_slot,
        outgoing,
        target,
        repoint_tick,
    };
    let _ = registry.set_component(pawn, inventory);
    timing.lower = Some(lower);
    timing.raise = None;
    timing.deferred_begin = defer.then_some(outgoing);
    if !defer
        && let Ok(postretro_entities::ComponentValue::Weapon(weapon)) =
            registry.get_component_value_mut(outgoing, postretro_entities::ComponentKind::Weapon)
    {
        crate::sim::begin_wieldable_lower(weapon);
    }
}
/// Equip callbacks run inside command resolution before starts and presses drain.
#[allow(clippy::too_many_arguments)]
pub fn host_resolve_remote_commands_with_switches(
    registry: &mut EntityRegistry,
    server: &mut NetServer,
    owners: &MovementOwners,
    queues: &mut HostCommandQueues,
    weapon_owners: &mut WeaponOwners,
    block_reload: bool,
    tick_dt: f32,
) -> Vec<ResolvedPawnCommand> {
    let mut commands = Vec::new();
    for (pawn, client) in owners.iter() {
        if let Some(timing) = queues.switch_timing_mut(client) {
            repair(timing, registry, server, pawn, client, weapon_owners);
        }
        let mut callback = |timing: &mut HostSwitchTiming, action, frontier| {
            apply(
                timing,
                action,
                frontier,
                registry,
                server,
                pawn,
                client,
                weapon_owners,
                block_reload,
                tick_dt,
            );
        };
        if let Some(resolved) = queues.resolve_tick_with_switches(client, Some(&mut callback)) {
            let mut remote = ResolvedPawnCommand {
                pawn,
                client_id: client,
                command: resolved.command,
                teleport_yaw_stamp: resolved.teleport_yaw_stamp,
                aim_pitch: queues.current_aim_pitch(client).unwrap_or(0.0),
                client_tick: resolved.client_tick,
                rejected_activation: resolved.rejected_activation,
                reload_press_tick: resolved.reload_press_tick,
                source: resolved.source,
            };
            if let Ok(movement) =
                registry.get_component::<postretro_foundation::PlayerMovementComponent>(pawn)
            {
                remote.command.movement.facing_yaw = crate::wire_convert::facing_in_teleport_frame(
                    remote.command.movement.facing_yaw,
                    movement.teleport.accumulated_yaw,
                    remote.teleport_yaw_stamp,
                );
            }
            commands.push(remote);
        }
    }
    commands
}
/// Touch/scripts may end a lower after resolution; settle them before snapshots.
pub fn host_settle_switches(
    registry: &mut EntityRegistry,
    server: &mut NetServer,
    owners: &MovementOwners,
    queues: &mut HostCommandQueues,
    weapon_owners: &mut WeaponOwners,
) {
    for (pawn, client) in owners.iter() {
        if let Some(timing) = queues.switch_timing_mut(client) {
            timing.deferred_begin = None;
            repair(timing, registry, server, pawn, client, weapon_owners);
        }
    }
}
impl HostCommandQueues {
    pub fn switch_start_refused(&self, client: u64, weapon: EntityId, tick: u32) -> bool {
        self.switch_timing(client)
            .is_some_and(|timing| timing.start_refused(weapon, tick))
    }
    pub fn remote_equip_gate(
        &self,
        client: u64,
        weapon: EntityId,
        reload_tick: u32,
    ) -> crate::sim::RemoteEquipGate {
        let Some(timing) = self.switch_timing(client) else {
            return Default::default();
        };
        crate::sim::RemoteEquipGate {
            reload_refused: timing.reload_refused(weapon, reload_tick),
            begin_lower_after_machine: timing.deferred_begin == Some(weapon),
        }
    }
}
