// The single post-movement pawn relocation chokepoint, shared by authority and prediction.
// See: context/lib/movement.md §6 (Teleport)

use super::swept::floor_below;
use super::{MovementEvents, MovementInput, MovementStateEdge};
use crate::collision::moving::CombinedCollisionWorld;
use crate::collision::{CollisionCapsule, MAP_UNIT_METRES, SKIN_DISTANCE};
use glam::{Quat, Vec3};
use postretro_foundation::{
    GroundRef, MovementState, MovementStateKind, PlayerMovementComponent, SeamlessMapping,
    wrap_teleport_yaw,
};
use postretro_level_format::teleporters::TeleportMode;

/// The relocation a tick performed, reported on that tick. Callers turn the
/// view and the aims they captured before the tick by `yaw_delta` on the same
/// tick. The host's authoritative tick fires the `teleport` event from it and
/// forward prediction plays the traveller's arrival cue; neither comes from
/// reconcile replay, which reproduces it.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TeleportEvent {
    /// Map-order index of the teleporter that fired.
    pub teleporter: u16,
    /// World-up yaw the view and every velocity channel turned by.
    pub yaw_delta: f32,
    /// Body point after relocation: the pose the tick ends at.
    pub arrival: Vec3,
}

/// Fire at most one teleporter for the post-step body point and end the tick at
/// its arrival pose, or return `position` unchanged. Runs after the substrate
/// in every tick, so host, prediction and replay agree by construction; `armed`
/// is the caller's snapshot of installed armed state for this tick.
pub(super) fn resolve(
    component: &mut PlayerMovementComponent,
    input: &MovementInput,
    collision: &CombinedCollisionWorld<'_>,
    position: Vec3,
    events: &mut MovementEvents,
    mut armed: impl FnMut(usize) -> bool,
) -> Vec3 {
    let teleporters = &collision.static_world.teleporters;
    component
        .teleport
        .arrival_latches
        .retain(|index| teleporters.regions.contains(usize::from(*index), position));
    if component.vault_barred {
        return position;
    }
    let Some(index) = teleporters.regions.winner(position, |index| {
        armed(index)
            && u16::try_from(index).is_ok_and(|index| {
                component
                    .teleport
                    .arrival_latches
                    .binary_search(&index)
                    .is_err()
            })
    }) else {
        return position;
    };
    let record = &teleporters.records()[index];
    let (yaw_delta, arrival) = match record.mode {
        TeleportMode::Classic => (
            record.destination_yaw - input.facing_yaw,
            Vec3::from(record.destination_origin),
        ),
        TeleportMode::Seamless => {
            let mapping = SeamlessMapping::new(
                Vec3::from(record.landmark_origin),
                record.landmark_yaw,
                Vec3::from(record.destination_origin),
                record.destination_yaw,
            );
            (mapping.yaw_delta(), mapping.point(position))
        }
    };
    let rotation = Quat::from_rotation_y(yaw_delta);
    // Installed poses are validated finite, so only a non-finite caller facing
    // or body point lands here. That is not an authored blocked arrival: refuse
    // without spending the teleporter's one blocked-arrival warning.
    if !arrival.is_finite() || !yaw_delta.is_finite() {
        return position;
    }
    if !crate::collision::capsule_fits_combined(collision, arrival, &component.capsule) {
        teleporters.warn_refused(index);
        return position;
    }
    // State payloads bank motion too; rotating total velocity alone would let the
    // next dash/slide intent reintroduce motion in the source room's frame.
    match &mut component.movement_state {
        MovementState::Dash { boost, .. } | MovementState::Sliding { boost, .. } => {
            *boost = rotation * *boost
        }
        MovementState::Vault { .. } => {
            // An abort, not the shared vault settle: the teleport keeps and
            // rotates every velocity channel instead of zeroing the path's.
            component.movement_state = MovementState::Normal;
            component.swim_reentry_latched = false;
            events.state_edges.push(MovementStateEdge {
                from: MovementStateKind::Vault,
                to: MovementStateKind::Normal,
            });
        }
        _ => {}
    }
    component.velocity = rotation * component.velocity;
    component.knockback_velocity = rotation * component.knockback_velocity;
    component.push_velocity = rotation * component.push_velocity;
    settle_arrival_ground(component, collision, arrival);
    component.teleport.generation = component.teleport.generation.wrapping_add(1);
    component.teleport.accumulated_yaw =
        wrap_teleport_yaw(component.teleport.accumulated_yaw + yaw_delta);
    component.teleport.last_teleporter = index as u16;
    component.teleport.arrival_latches.clear();
    teleporters.regions.visit_containing(arrival, |index| {
        if armed(index) {
            component.teleport.arrival_latches.push(index as u16);
        }
    });
    component.teleport.arrival_latches.sort_unstable();
    events.teleport = Some(TeleportEvent {
        teleporter: index as u16,
        yaw_delta,
        arrival,
    });
    arrival
}

/// How far an arrival may sit above its resting height on a floor and still
/// stand on it: a skin width of probe slop plus one map unit (0.0254 m), the
/// margin `docs/teleporters.md` asks authors to leave under a destination. The
/// arrival never drops the pawn onto the floor, so any wider band would ground a
/// floating pawn: its next tick would take a grounded jump, or a ground-entry
/// vault, from feet hanging in the air.
const ARRIVAL_GROUND_TOLERANCE: f32 = SKIN_DISTANCE + MAP_UNIT_METRES;

/// Ground the pawn on the static floor under its arrival pose, or leave it
/// airborne with coyote spent. The capsule never moves: the arrival is exact.
///
/// This is a classification, not the substrate's ground-stick. Ground-stick
/// only follows a body that was already grounded (or floor-pushed) and needs a
/// positive step height, and it drops that body up to a step onto the floor in
/// the same tick. An arrival does neither, so it counts as standing only when
/// a non-rising pawn already rests on walkable static floor within
/// [`ARRIVAL_GROUND_TOLERANCE`]. A walking or sliding pawn shifted onto a
/// matching floor therefore takes no air tick and a slide continues; a crouched
/// pawn on a pad authored for standing, or a destination set higher, falls.
/// Mover carry never follows a teleport, so a mover under the arrival counts as
/// no floor. Without a floor the arrival starts a fresh airborne stretch that
/// never stood on ground there, so no coyote jump is owed: a pawn grounded at
/// the source would otherwise jump once from mid-air. Air jumps keep their
/// budget. Pure position and component state, so host, prediction and replay
/// agree.
fn settle_arrival_ground(
    component: &mut PlayerMovementComponent,
    collision: &CombinedCollisionWorld<'_>,
    arrival: Vec3,
) {
    let floor = (component.velocity.y <= 1.0e-3)
        .then(|| {
            floor_below(
                &CombinedCollisionWorld::static_only(collision.static_world),
                CollisionCapsule::new(component.capsule.radius, component.capsule.half_height),
                arrival,
                ARRIVAL_GROUND_TOLERANCE,
                component.cos_walkable,
            )
        })
        .flatten();
    match floor {
        Some(floor) => {
            // The substrate's floor-contact writes, minus the relocation and
            // the velocity clamp: speed survives the teleport unchanged.
            component.ground = GroundRef::World;
            component.last_floor_normal = Some(floor.normal);
            component.air_ticks = 0;
            component.refresh_on_landing();
        }
        None => {
            component.ground = GroundRef::Airborne;
            component.last_floor_normal = None;
            component.jump_spent = true;
        }
    }
}

#[cfg(test)]
#[path = "teleport_tests.rs"]
mod tests;
