//! Installed map foot poses and durable seat assignments for authoritative respawn.
//! A fallback keeps the existing pawn archetype, so its centre is derived later
//! from this foot anchor and that pawn's current capsule, never the spawn class's.

use postretro_entities::{EntityTypeDescriptor, RespawnPlacement, RespawnPlacements, ScriptCtx};

use crate::netcode::SeatTable;
use crate::scripting::builtins::player_spawn::{first_player_spawn_order, joinable_spawns};
use crate::scripting::map_entity::MapEntity;

pub(crate) fn build(
    spawn_points: &[MapEntity],
    descriptors: &[EntityTypeDescriptor],
    seats: Option<&SeatTable>,
    first_player: Option<usize>,
) -> RespawnPlacements {
    let eligible = joinable_spawns(spawn_points, descriptors);
    let map_order = eligible
        .iter()
        .map(|&map_index| {
            let spawn = &spawn_points[map_index];
            RespawnPlacement {
                map_index,
                foot_anchor: spawn.origin,
                yaw: spawn.rotation_quat().to_euler(glam::EulerRot::YXZ).0,
            }
        })
        .collect();
    let first_player = first_player
        .or_else(|| seats.and_then(|seats| seats.placement_for_seat(postretro_foundation::Seat(0))))
        .filter(|index| eligible.contains(index))
        .or_else(|| {
            first_player_spawn_order(spawn_points)
                .into_iter()
                .map(|(index, _)| index)
                .find(|index| eligible.contains(index))
        });
    RespawnPlacements {
        seats: seats
            .into_iter()
            .flat_map(SeatTable::assigned_placements)
            .filter(|(_, index)| eligible.contains(index))
            .collect(),
        map_order,
        first_player,
    }
}

/// Publish only on install or a changed seat assignment. Retain the installed
/// first-player placement when a later join rebuilds the assignment projection.
/// No registry borrow is needed: the participation caller already holds one.
pub(crate) fn publish(
    ctx: &ScriptCtx,
    spawn_points: &[MapEntity],
    descriptors: &[EntityTypeDescriptor],
    seats: Option<&SeatTable>,
    first_player: Option<usize>,
) {
    if !ctx.owner_slot_writes_enabled.get() {
        return;
    }
    let first_player = first_player.or(ctx.respawn_placements.borrow().first_player);
    let placements = build(spawn_points, descriptors, seats, first_player);
    *ctx.respawn_placements.borrow_mut() = placements;
}

/// Actual level retirement only. Surface teardown also runs for suspend, which
/// must retain requests and placements for the same materialized level.
pub(crate) fn clear_for_level_unload(ctx: &ScriptCtx) {
    ctx.pending_respawns.borrow_mut().clear();
    *ctx.respawn_placements.borrow_mut() = RespawnPlacements::default();
}

#[cfg(test)]
mod tests;
