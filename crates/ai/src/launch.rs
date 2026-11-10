// Knockback launch presentation for enemy brains: the impact-yaw latch facing
// holds, and the airborne clip after the player's fall delay.
// See: context/lib/entity_model.md §7c (Engaged motion and facing)

use glam::{Quat, Vec3};
use postretro_entities::components::brain::LaunchState;
use postretro_entities::components::mesh::MeshComponent;
use postretro_entities::{EntityId, EntityRegistry};

use super::facing::yaw_from_rotation;
use crate::agent_steering::AgentPathState;
use crate::sim::{FALL_DELAY_TICKS, JUMP_LOOP_STATE};

/// Advance a brain's launch memory from this tick's start-of-tick path state.
/// `heading::resolve` calls it once per tick, before facing reads the impact
/// yaw.
///
/// Launched means knockback velocity remains. The first launched tick latches
/// the current yaw, which is the yaw at impact: the hit lands before the AI
/// tick reads it. An airborne stretch a launch began keeps counting until the
/// body lands, even if drag spends the knockback in the air.
pub(super) fn advance(launch: &mut LaunchState, path: Option<&AgentPathState>, rotation: Quat) {
    let launched = path.is_some_and(|path| path.knockback_velocity != Vec3::ZERO);
    launch.impact_yaw = if launched {
        Some(
            launch
                .impact_yaw
                .unwrap_or_else(|| yaw_from_rotation(rotation)),
        )
    } else {
        None
    };
    let airborne = path.is_some_and(|path| path.airborne);
    launch.air_ticks = if airborne && (launched || launch.air_ticks > 0) {
        launch.air_ticks.saturating_add(1)
    } else {
        0
    };
}

/// The clip an airborne launch presents this tick: `jump_loop` once the stretch
/// passes the player's fall delay, when the mesh can play it. `None` leaves the
/// activity's clip in charge, so a mesh without `jump_loop` keeps its state.
pub(super) fn airborne_clip(
    registry: &EntityRegistry,
    id: EntityId,
    launch: &LaunchState,
) -> Option<&'static str> {
    if launch.air_ticks < FALL_DELAY_TICKS {
        return None;
    }
    registry
        .get_component::<MeshComponent>(id)
        .ok()
        .and_then(|mesh| mesh.animation.as_ref())
        .is_some_and(|animation| animation.is_state_usable(JUMP_LOOP_STATE))
        .then_some(JUMP_LOOP_STATE)
}
