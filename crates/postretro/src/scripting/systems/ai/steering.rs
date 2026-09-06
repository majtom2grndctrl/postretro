use glam::Vec3;
use postretro_entities::components::brain::BrainComponent;
use postretro_foundation::{MotionVerb, PatrolMode};

use super::engine_floor::{POSITION_GOAL_ARRIVAL_EPSILON, SteeringIntent};
use super::graph_eval::steering_for;

/// Resolve a state motion whose destination depends on per-brain state. Unlike
/// [`steering_for`], this runs in the compute pass where the spawn anchor,
/// patrol descriptor, and persistent patrol cursor are all available.
pub(super) fn position_goal_steering(
    motion: MotionVerb,
    brain: &mut BrainComponent,
    position: Vec3,
) -> SteeringIntent {
    match motion {
        MotionVerb::MoveToAnchor => {
            if crate::nav::distance_xz(position, brain.home_anchor) <= POSITION_GOAL_ARRIVAL_EPSILON
            {
                SteeringIntent::Clear
            } else {
                SteeringIntent::MoveTo(brain.home_anchor)
            }
        }
        MotionVerb::MoveToLastKnown => {
            let Some(goal) = brain.last_known_target_pos else {
                return SteeringIntent::Clear;
            };
            if crate::nav::distance_xz(position, goal) <= POSITION_GOAL_ARRIVAL_EPSILON {
                SteeringIntent::Clear
            } else {
                SteeringIntent::MoveTo(goal)
            }
        }
        MotionVerb::Patrol => patrol_steering(brain, position),
        // This resolver owns only motion modes with per-brain position goals.
        // Every other mode remains the pure graph evaluator's responsibility.
        MotionVerb::ChaseTarget | MotionVerb::Hold | MotionVerb::Freeze => steering_for(motion),
    }
}

/// Resolve the next patrol point and preserve the route phase on the brain.
/// A malformed hand-built graph degrades to standing still; descriptor
/// validation rejects the same shape before authored data reaches this path.
fn patrol_steering(brain: &mut BrainComponent, position: Vec3) -> SteeringIntent {
    let Some(patrol) = brain.graph.patrol.as_ref() else {
        return SteeringIntent::Clear;
    };
    let point_count = patrol.points.len();
    if point_count == 0 {
        return SteeringIntent::Clear;
    }
    let mode = patrol.mode;

    // A saved brain may outlive a descriptor edit that shortens the route.
    // Preserve its phase rather than resetting it before indexing.
    brain.patrol_cursor %= point_count;
    let mut goal = patrol_goal(brain);
    if crate::nav::distance_xz(position, goal) <= POSITION_GOAL_ARRIVAL_EPSILON {
        if point_count == 1 {
            return SteeringIntent::Clear;
        }
        advance_patrol_cursor(brain, point_count, mode);
        goal = patrol_goal(brain);
    }
    SteeringIntent::MoveTo(goal)
}

fn patrol_goal(brain: &BrainComponent) -> Vec3 {
    let patrol = brain
        .graph
        .patrol
        .as_ref()
        .expect("patrol goal is only requested for a present non-empty route");
    let [x, z] = patrol.points[brain.patrol_cursor];
    brain.home_anchor + Vec3::new(x, 0.0, z)
}

fn advance_patrol_cursor(brain: &mut BrainComponent, point_count: usize, mode: PatrolMode) {
    if point_count == 1 {
        return;
    }

    match mode {
        PatrolMode::Loop => {
            brain.patrol_cursor = (brain.patrol_cursor + 1) % point_count;
        }
        PatrolMode::PingPong if brain.patrol_direction >= 0 => {
            brain.patrol_direction = 1;
            if brain.patrol_cursor + 1 == point_count {
                brain.patrol_direction = -1;
                brain.patrol_cursor -= 1;
            } else {
                brain.patrol_cursor += 1;
            }
        }
        PatrolMode::PingPong => {
            brain.patrol_direction = -1;
            if brain.patrol_cursor == 0 {
                brain.patrol_direction = 1;
                brain.patrol_cursor = 1;
            } else {
                brain.patrol_cursor -= 1;
            }
        }
    }
}
