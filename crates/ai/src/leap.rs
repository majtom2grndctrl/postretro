// Engaged `leap`: the landing spot fixed on entry, and the steering that
// arrives on it by the activity's clip moment.
// See: context/lib/entity_model.md §7c (Engaged motion and facing)

use glam::Vec3;
use postretro_entities::components::brain::BrainComponent;
use postretro_foundation::MotionVerb;

use super::engine_floor::SteeringIntent;
use super::graph_eval::ResolvedMotion;
use super::targeting::TargetPawn;

/// Steer a path whose motion is `leap`; any other path keeps `steering` and
/// drops a spot left from an earlier leap.
///
/// On entry (an activity entry at or above the depth that supplies the
/// motion), or with no spot yet (a brain restored from before spots were
/// kept, or a leap entered with no target), it fixes the landing spot: its
/// standoff short of where the target stands now, on the line from the
/// target to the enemy, or where the enemy stands if it is already that
/// close. It never re-aims after: a target that moves off the spot is missed
/// at the moment. It travels to the spot until its moment has passed, then
/// stands. [`super::clip_clock::seconds_to_moment`] paces the travel.
pub(super) fn steer(
    brain: &mut BrainComponent,
    motion: Option<ResolvedMotion>,
    target: Option<TargetPawn>,
    position: Vec3,
    steering: SteeringIntent,
) -> SteeringIntent {
    let Some(motion) = motion.filter(|motion| motion.verb == MotionVerb::Leap) else {
        brain.leap_spot = None;
        return steering;
    };
    let entered = brain
        .entry_pending_depth()
        .is_some_and(|depth| depth <= motion.depth);
    if entered || brain.leap_spot.is_none() {
        let standoff = brain.graph.standoff_distance_for_action(
            brain
                .activity_at_depth(motion.depth)
                .and_then(|(_, activity)| activity.action.as_ref()),
        );
        brain.leap_spot = target.map(|target| landing_spot(target.position, position, standoff));
    }
    // The clip clock opens the moment on entry, after this; before then the
    // latch still reads the activity left.
    let moment_ahead = entered || brain.clip_clock.moment_open;
    match brain.leap_spot {
        Some(spot) if moment_ahead => SteeringIntent::MoveTo(spot),
        _ => SteeringIntent::Clear,
    }
}

/// `standoff` short of `target` toward `enemy`, at the target's height; the
/// enemy's own position when it already stands within `standoff`.
pub(super) fn landing_spot(target: Vec3, enemy: Vec3, standoff: f32) -> Vec3 {
    let offset = Vec3::new(enemy.x - target.x, 0.0, enemy.z - target.z);
    let distance = offset.length();
    if !distance.is_finite() || distance <= standoff.max(0.0) {
        return Vec3::new(enemy.x, target.y, enemy.z);
    }
    target + offset * (standoff.max(0.0) / distance)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_spot_is_the_standoff_short_of_the_target_toward_the_enemy() {
        let target = Vec3::new(0.0, 1.0, 0.0);
        let spot = landing_spot(target, Vec3::new(30.0, 0.0, 40.0), 1.5);
        assert!(
            (spot - Vec3::new(0.9, 1.0, 1.2)).length() < 1.0e-5,
            "{spot}"
        );
        // Already that close: it stays put, at the target's height.
        let near = landing_spot(target, Vec3::new(1.0, 0.0, 0.0), 1.5);
        assert_eq!(near, Vec3::new(1.0, 1.0, 0.0));
    }
}
