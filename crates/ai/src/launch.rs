// Knockback launch presentation for enemy brains: the impact-yaw latch facing
// holds, and the airborne clip after the player's fall delay.
// See: context/lib/entity_model.md §7c (Engaged motion and facing)

use glam::{Quat, Vec3};
use postretro_entities::components::brain::LaunchState;
use postretro_entities::components::mesh::MeshComponent;
use postretro_entities::{EntityId, EntityRegistry};

use super::facing::yaw_from_rotation;
use crate::agent_steering::AgentPathState;
use crate::sim::JUMP_LOOP_STATE;

/// Airborne ticks of a launch before its body presents `jump_loop`: the
/// player's fall delay, about 150 ms, kept as the NPC's own so a player retune
/// does not move it. Counted in elapsed ticks.
pub(crate) const NPC_FALL_DELAY_TICKS: u32 = 9;

/// Advance a brain's launch memory from this tick's start-of-tick path state.
/// `heading::resolve` calls it once per tick, before facing reads the impact
/// yaw.
///
/// The impact yaw is held while knockback velocity remains. The first such
/// tick latches the current yaw, which is the yaw at impact: the hit lands
/// before the AI tick reads it. An airborne stretch a launch began keeps
/// counting until the body lands, even if drag spends the knockback in the
/// air; either keeps the enemy launched, which cancels a clip-timed attack's
/// pending moment ([`launched`]).
///
/// `span` is the ticks since the enemy's last run: an airborne stretch counts
/// every one of them.
pub(super) fn advance(
    launch: &mut LaunchState,
    path: Option<&AgentPathState>,
    rotation: Quat,
    span: u32,
) {
    let knockback = path.is_some_and(|path| path.knockback_velocity != Vec3::ZERO);
    launch.impact_yaw = if knockback {
        Some(
            launch
                .impact_yaw
                .unwrap_or_else(|| yaw_from_rotation(rotation)),
        )
    } else {
        None
    };
    let airborne = path.is_some_and(|path| path.airborne);
    launch.air_ticks = if airborne && (knockback || launch.air_ticks > 0) {
        launch.air_ticks.saturating_add(span.max(1))
    } else {
        0
    };
}

/// Whether a knockback launch is under way: knockback velocity remains, or the
/// body is still in the airborne stretch a launch began, though drag spent the
/// knockback.
pub(super) fn launched(launch: &LaunchState) -> bool {
    launch.impact_yaw.is_some() || launch.air_ticks > 0
}

/// The clip an airborne launch presents this tick: `jump_loop` once the stretch
/// passes the player's fall delay, when the mesh can play it. `None` leaves the
/// activity's clip in charge, so a mesh without `jump_loop` keeps its state.
pub(super) fn airborne_clip(
    registry: &EntityRegistry,
    id: EntityId,
    launch: &LaunchState,
) -> Option<&'static str> {
    if launch.air_ticks < NPC_FALL_DELAY_TICKS {
        return None;
    }
    registry
        .get_component::<MeshComponent>(id)
        .ok()
        .and_then(|mesh| mesh.animation.as_ref())
        .is_some_and(|animation| animation.is_state_usable(JUMP_LOOP_STATE))
        .then_some(JUMP_LOOP_STATE)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn launched_path() -> AgentPathState {
        AgentPathState {
            has_destination: false,
            has_path: false,
            arrived: false,
            blocked: false,
            distance_to_destination: 0.0,
            position: Vec3::ZERO,
            velocity: Vec3::X,
            knockback_velocity: Vec3::X,
            airborne: true,
        }
    }

    /// The run on which a launch's airborne stretch first passes the fall
    /// delay, running every `span` ticks.
    fn presenting_run(span: u32) -> u32 {
        let mut launch = LaunchState::default();
        let path = launched_path();
        (1..)
            .find(|_| {
                advance(&mut launch, Some(&path), Quat::IDENTITY, span);
                launch.air_ticks >= NPC_FALL_DELAY_TICKS
            })
            .unwrap()
    }

    // P18: the fall delay counts elapsed ticks: at 60 Hz the stretch passes it
    // on its ninth tick, at 2 Hz on its first run.
    #[test]
    fn the_fall_delay_passes_on_the_first_run_at_or_past_it_at_any_span() {
        assert_eq!(presenting_run(1), NPC_FALL_DELAY_TICKS);
        assert_eq!(presenting_run(30), 1);
        assert_eq!(presenting_run(4), NPC_FALL_DELAY_TICKS.div_ceil(4));
    }
}
