// Board → Ride → Exit: how an agent uses a lift a path planned through. Riding
// suspends steering, stuck detection and replans; the mover's own motion does the
// work, and the agent replans wherever it steps off.
// See: context/lib/build_pipeline.md §Navigation bake · context/lib/movement.md §6

use glam::Vec3;

use crate::collision::moving::CombinedCollisionWorld;
use crate::movement::agent_motion::carried_by_mover;
use crate::nav::{NavGate, distance_xz};
use postretro_entities::components::agent::{AgentComponent, AgentRide};
use postretro_foundation::GroundRef;

/// How close to its boarding point an agent stops steering by path and walks the
/// rest in a straight line: `arrived` would otherwise stop it on the rim.
const BOARD_RADIUS_FACTOR: f32 = 3.0;

/// What the ride asks of this tick's motion.
pub(super) enum RideStep {
    /// Not riding: ordinary steering.
    Free,
    /// Stand on the mover.
    Hold,
    /// Walk at this velocity.
    Walk(Vec3),
}

/// Advance the agent's board, ride and exit state by one tick. Runs before path
/// steering, so a ride that ends here hands the tick back to it.
pub(super) fn advance(
    agent: &mut AgentComponent,
    position: Vec3,
    gate: NavGate<'_>,
    collision: &CombinedCollisionWorld<'_>,
) -> RideStep {
    let standing_on = match agent.ground {
        GroundRef::Mover(mover_id) => Some(mover_id),
        _ => None,
    };
    if agent.ride.is_none()
        && let Some(step) = board(agent, position, gate, standing_on)
    {
        return step;
    }
    // Boarding a planned ride above sets one; an unplanned ride never replaces it.
    if agent.ride.is_none() {
        enter_ride_if_carried(agent, gate, collision, standing_on);
    }
    let Some(mut ride) = agent.ride else {
        return RideStep::Free;
    };
    if standing_on != Some(ride.mover_id) {
        // Stepped onto the landing, or knocked off. Either way the plan is over.
        leave_ride(agent);
        return RideStep::Free;
    }
    let Some(docked) = gate.docked_waypoint(ride.mover_id) else {
        // Between docks, stopped or not: nowhere safe to step.
        ride.departed = true;
        agent.ride = Some(ride);
        return RideStep::Hold;
    };
    if ride.departed || Some(docked) != ride.board_waypoint {
        if Some(docked) == ride.exit_waypoint {
            return walk_towards(agent, position, ride.exit_point);
        }
        // A stop with no planned exit: a passed landing, a hold elsewhere, or a
        // full cycle back to where it got on.
        leave_ride(agent);
        return RideStep::Free;
    }
    // Waiting at the boarding dock. A script may park the lift under it.
    if gate.moves_on_its_own(ride.mover_id) {
        return RideStep::Hold;
    }
    leave_ride(agent);
    RideStep::Free
}

/// The boarding walk. `Some` overrides path steering this tick.
fn board(
    agent: &mut AgentComponent,
    position: Vec3,
    gate: NavGate<'_>,
    standing_on: Option<u32>,
) -> Option<RideStep> {
    let plan = agent.planned_ride?;
    if standing_on == Some(plan.mover_id) {
        agent.planned_ride = None;
        clear_path(agent);
        agent.ride = Some(AgentRide {
            mover_id: plan.mover_id,
            board_waypoint: Some(plan.board_waypoint),
            exit_waypoint: Some(plan.exit_waypoint),
            exit_point: plan.exit_point,
            departed: false,
        });
        return None;
    }
    if !gate.docked_at(plan.mover_id, plan.board_waypoint) || !gate.moves_on_its_own(plan.mover_id)
    {
        // The lift left, or was parked, before the agent stepped on. Never walk
        // into the shaft after it.
        agent.planned_ride = None;
        clear_path(agent);
        agent.blocked = true;
        agent.steer_velocity = Vec3::ZERO;
        return None;
    }
    let near = distance_xz(position, plan.board_point) <= BOARD_RADIUS_FACTOR * agent.radius;
    (near || agent.arrived).then(|| walk_towards(agent, position, plan.board_point))
}

/// A body a watched mover carries off rides it, planned or not: it was standing
/// on a lift as it left, or a riser came up under it.
fn enter_ride_if_carried(
    agent: &mut AgentComponent,
    gate: NavGate<'_>,
    collision: &CombinedCollisionWorld<'_>,
    standing_on: Option<u32>,
) {
    let Some(mover_id) = standing_on else {
        return;
    };
    if gate.watches(mover_id) && carried_by_mover(collision, agent.ground) {
        agent.planned_ride = None;
        clear_path(agent);
        agent.ride = Some(AgentRide {
            mover_id,
            board_waypoint: None,
            exit_waypoint: None,
            exit_point: Vec3::ZERO,
            departed: true,
        });
    }
}

fn walk_towards(agent: &AgentComponent, position: Vec3, target: Vec3) -> RideStep {
    let offset = Vec3::new(target.x - position.x, 0.0, target.z - position.z);
    RideStep::Walk(offset.normalize_or_zero() * agent.move_speed)
}

fn leave_ride(agent: &mut AgentComponent) {
    agent.ride = None;
    agent.planned_destination = None;
    clear_path(agent);
}

fn clear_path(agent: &mut AgentComponent) {
    agent.path.clear();
    agent.path_portals.clear();
    agent.mandatory_waypoints.clear();
    agent.waypoint_cursor = 0;
    agent.arrived = false;
}
