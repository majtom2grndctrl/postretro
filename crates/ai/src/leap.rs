// Engaged `leap`: the landing spot fixed on entry, and the steering that
// arrives on it by the activity's clip moment.
// See: context/lib/entity_model.md §7c (Engaged motion and facing)

use glam::Vec3;
use postretro_entities::components::brain::BrainComponent;
use postretro_foundation::MotionVerb;

use super::clip_clock::{self, ClipSource};
use super::engine_floor::SteeringIntent;
use super::graph_eval::ResolvedMotion;
use super::targeting::TargetPawn;
use crate::nav::NavGraph;

/// The most steps [`walkable_spot`] takes along each side of its line.
const MAX_SPOT_STEPS: usize = 256;

/// A leap travelling to its spot this tick, as apply writes it to steering.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct LeapTravel {
    /// Seconds to the clip moment it arrives by. `None` when the drawn clip
    /// cannot time the moment while its latch stays open (a defaulted `hit`
    /// the clip lacks): it flies at its cap, and apply warns.
    pub(crate) arrive_in_s: Option<f32>,
}

/// Steer a path whose motion is `leap`; any other path keeps `steering` and
/// drops a spot left from an earlier leap.
///
/// On entry (an activity entry at or above the depth that supplies the
/// motion), or with no spot yet (a brain restored from before spots were
/// kept, or a leap entered with no target), it fixes the landing spot: its
/// standoff short of where the target stands now, on the line from the
/// target to the enemy, or where the enemy stands if it is already that
/// close. A spot no route can reach moves to walkable ground on that line
/// ([`walkable_spot`]); the second value is `true` when none was found, and
/// the leap then has no route and stands. It never re-aims after: a target
/// that moves off the spot is missed at the moment. It travels to the spot
/// while its moment is ahead; [`pace`] then decides this tick's travel once
/// the clip clock has settled.
pub(super) fn steer(
    brain: &mut BrainComponent,
    motion: Option<ResolvedMotion>,
    target: Option<TargetPawn>,
    position: Vec3,
    steering: SteeringIntent,
    nav_graph: Option<&NavGraph>,
) -> (SteeringIntent, bool) {
    let Some(motion) = motion.filter(|motion| motion.verb == MotionVerb::Leap) else {
        brain.leap_spot = None;
        return (steering, false);
    };
    let entered = brain
        .entry_pending_depth()
        .is_some_and(|depth| depth <= motion.depth);
    let mut off_mesh = false;
    if entered || brain.leap_spot.is_none() {
        let standoff = brain.graph.standoff_distance_for_action(
            brain
                .activity_at_depth(motion.depth)
                .and_then(|(_, activity)| activity.action.as_ref()),
        );
        brain.leap_spot = target.map(|target| {
            let spot = landing_spot(target.position, position, standoff);
            match nav_graph {
                Some(graph) => walkable_spot(graph, spot, target.position, position)
                    .unwrap_or_else(|| {
                        off_mesh = true;
                        spot
                    }),
                None => spot,
            }
        });
    }
    // The clip clock opens the moment on entry, after this; before then the
    // latch still reads the activity left.
    let moment_ahead = entered || brain.clip_clock.moment_open;
    let steering = match brain.leap_spot {
        Some(spot) if moment_ahead => SteeringIntent::MoveTo(spot),
        _ => SteeringIntent::Clear,
    };
    (steering, off_mesh)
}

/// A leap's travel this tick, read after [`clip_clock::settle`]: the moment
/// latch [`steer`] read may have closed since (the moment reached, a
/// knockback launch), or the clock may hold for a dropped enemy's redraw.
///
/// While the moment is ahead the leap keeps its `MoveTo`, paced to arrive by
/// the moment ([`clip_clock::seconds_to_moment`]), or with no deadline when
/// the clip cannot time it. Otherwise it stands this tick: past its moment a
/// leap that fell short never lurches on. Any other path keeps `steering`.
pub(super) fn pace(
    brain: &BrainComponent,
    source: &ClipSource<'_>,
    motion: Option<MotionVerb>,
    steering: SteeringIntent,
) -> (SteeringIntent, Option<LeapTravel>) {
    if motion != Some(MotionVerb::Leap) || !matches!(steering, SteeringIntent::MoveTo(_)) {
        return (steering, None);
    }
    let clock = &brain.clip_clock;
    if !clock.moment_open || clock.redraw {
        return (SteeringIntent::Clear, None);
    }
    let arrive_in_s = clip_clock::seconds_to_moment(brain, source);
    (steering, Some(LeapTravel { arrive_in_s }))
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

/// `spot`, when a route query resolves it; else the nearest point a route
/// query resolves on the line from `target` through `spot` to `enemy`,
/// stepping a nav cell at a time out from `spot` both ways, toward the
/// target first at equal distance (a spot past a ledge's lip moves back onto
/// the ledge the target stands on, not across the drop). `None` when nothing
/// on the line resolves.
pub(super) fn walkable_spot(
    graph: &NavGraph,
    spot: Vec3,
    target: Vec3,
    enemy: Vec3,
) -> Option<Vec3> {
    if graph.resolves(spot) {
        return Some(spot);
    }
    let step = graph.cell_size().max(0.05);
    let flat = |a: Vec3, b: Vec3| Vec3::new(b.x - a.x, 0.0, b.z - a.z).length();
    let ends = [(target, flat(spot, target)), (enemy, flat(spot, enemy))];
    let reach = ends[0].1.max(ends[1].1);
    let steps = ((reach / step).ceil() as usize).min(MAX_SPOT_STEPS);
    for k in 1..=steps {
        let along = k as f32 * step;
        for (end, length) in ends {
            if length.is_nan() || length <= 0.0 || along - step >= length {
                continue;
            }
            let candidate = spot.lerp(end, (along / length).min(1.0));
            if graph.resolves(candidate) {
                return Some(candidate);
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use postretro_level_format::navmesh::{NAVMESH_VERSION, NavMeshSection, NavRegion};

    /// A 1 m-cell navmesh of floor regions spanning `x0..x1` by `0..4`.
    fn strips(spans: &[(u32, u32)]) -> NavGraph {
        NavGraph::from_section(&NavMeshSection {
            version: NAVMESH_VERSION,
            origin: [0.0, 0.0, 0.0],
            cell_size: 1.0,
            dim_x: 64,
            dim_z: 8,
            agent_radius: 0.35,
            agent_height: 1.8,
            step_height: 0.4,
            max_slope_deg: 45.0,
            regions: spans
                .iter()
                .map(|&(x0, x1)| NavRegion {
                    x0,
                    z0: 0,
                    x1,
                    z1: 4,
                    floor_y_min: 0.0,
                    floor_y_max: 0.25,
                })
                .collect(),
            portals: Vec::new(),
            gates: Vec::new(),
            links: Vec::new(),
        })
    }

    // A spot past a ledge's lip, over a drop no route crosses, moves to the
    // nearest walkable ground on its line: back onto the ledge the target
    // stands on, which is nearer than the enemy's floor across the drop.
    // Off every region, it is not found, and the leap warns.
    #[test]
    fn a_spot_over_a_drop_moves_to_the_nearest_walkable_ground_on_its_line() {
        let graph = strips(&[(0, 4), (12, 16)]);
        let target = Vec3::new(3.5, 0.0, 2.0);
        let enemy = Vec3::new(14.0, 0.0, 2.0);
        let spot = landing_spot(target, enemy, 4.0);
        assert!(!graph.resolves(spot), "fixture: the spot is off the mesh");
        let moved = walkable_spot(&graph, spot, target, enemy).expect("ground on the line");
        assert!(graph.resolves(moved));
        assert!(
            (moved - Vec3::new(5.5, 0.0, 2.0)).length() < 1.0e-4,
            "two cells back toward the target, onto the ledge: {moved}"
        );
        // On the mesh already: unchanged.
        let near = landing_spot(target, enemy, 1.0);
        assert_eq!(walkable_spot(&graph, near, target, enemy), Some(near));
        // Nothing walkable anywhere on the line.
        let far = strips(&[(40, 44)]);
        assert_eq!(walkable_spot(&far, spot, target, enemy), None);
    }

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
