// An enemy's heading for one compute tick: launch memory, the resolved facing
// and its post-slew yaw, the attack facing gate, and the travel clip variant.
// See: context/lib/entity_model.md §7c (Engaged motion and facing)

use glam::{Quat, Vec3};
use postretro_entities::components::brain::{BrainComponent, TravelVariant};
use postretro_entities::{EntityId, EntityRegistry};
use postretro_foundation::MotionVerb;

use super::LocomotionIntent;
use super::engine_floor::SteeringIntent;
use super::facing::{
    FACING_TURN_RATE, facing_direction, resolve_facing_policy, slewed_yaw_toward,
    verb_facing_policy, yaw_from_rotation, yaw_within_attack_tolerance,
};
use super::graph_eval::authored_facing;
use super::{launch, travel_variants};
use crate::agent_steering;

/// The post-transition inputs heading resolution reads.
pub(super) struct HeadingQuery {
    pub(super) id: EntityId,
    /// The enemy's rotation as snapshotted at the start of the tick.
    pub(super) rotation: Quat,
    pub(super) motion: Option<MotionVerb>,
    pub(super) steering: SteeringIntent,
    pub(super) engaged: bool,
    /// The selected target is retained across ticks (see compute).
    pub(super) retains_target: bool,
    /// Eye to target aim point, when a target is perceived.
    pub(super) aim_direction: Option<Vec3>,
    pub(super) tick_dt: f32,
}

/// What this tick's heading hands the rest of compute.
pub(super) struct Heading {
    /// The direction apply slews toward; `None` leaves the transform as is.
    pub(super) facing_direction: Option<Vec3>,
    /// The post-slew yaw aims within the attack tolerance of the target.
    pub(super) within_attack_tolerance: bool,
    pub(super) travel_variant: TravelVariant,
}

/// Resolve this tick's heading after the transition settles the active path.
/// Advances the brain's launch memory first, because facing reads its impact
/// yaw.
pub(super) fn resolve(
    registry: &EntityRegistry,
    brain: &mut BrainComponent,
    query: HeadingQuery,
) -> Heading {
    // A nested offense phase can intentionally hold at an authored standoff
    // before its leaf exposes an action. It is still committed to the
    // target: keep its aim moving even though its resolved current verb is
    // `hold`, which `engages_path` correctly leaves false.
    let committed_aim = query.retains_target && query.motion == Some(MotionVerb::Hold);
    // Facing and the attack gate read steering velocity alone, so knockback
    // never turns the enemy toward its flight. A launch holds its impact yaw
    // under travel facing; target facing keeps re-aiming.
    let path_state = agent_steering::path_state(registry, query.id);
    launch::advance(&mut brain.launch, path_state.as_ref(), query.rotation);
    let steering_velocity = path_state.map(|path| path.steering_velocity());
    let facing_policy = resolve_facing_policy(
        brain.aggro_armed.then(|| authored_facing(brain)).flatten(),
        query.motion,
        || {
            verb_facing_policy(
                query.steering,
                query.engaged,
                committed_aim,
                steering_velocity
                    .is_some_and(|velocity| LocomotionIntent::from_velocity(velocity).moving),
            )
        },
    );
    let facing_direction = facing_direction(
        facing_policy,
        steering_velocity,
        query.aim_direction,
        brain.launch.impact_yaw,
    );
    // If no horizontal yaw is derivable (for example, melee contact's
    // vertical eye-to-aim segment), apply leaves the transform untouched;
    // the firing check therefore reads that unchanged heading.
    let post_slew_yaw = facing_direction
        .and_then(|direction| {
            slewed_yaw_toward(query.rotation, direction, FACING_TURN_RATE * query.tick_dt)
        })
        .unwrap_or_else(|| yaw_from_rotation(query.rotation));
    let travel_variant = travel_variants::classify(
        facing_policy,
        steering_velocity,
        post_slew_yaw,
        brain.presented_travel_variant,
    );
    let within_attack_tolerance = query
        .aim_direction
        .is_some_and(|aim| yaw_within_attack_tolerance(post_slew_yaw, aim));
    Heading {
        facing_direction,
        within_attack_tolerance,
        travel_variant,
    }
}
