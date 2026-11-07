// Board → Ride → Exit: how an agent uses a lift a path planned through. Riding
// suspends steering, stuck detection and replans; the mover's own motion does the
// work, and the agent replans wherever it steps off.
// See: context/lib/build_pipeline.md §Navigation bake · context/lib/movement.md §6

use glam::Vec3;

use crate::collision::moving::CombinedCollisionWorld;
use crate::movement::agent_motion::carried_by_mover;
use crate::nav::{NavGate, distance_xz};
use postretro_entities::components::agent::{AgentComponent, AgentRide, PlannedRide};
use postretro_entities::{ComponentKind, ComponentValue, EntityRegistry, KinematicMoverMode};
use postretro_foundation::GroundRef;

/// How close to its boarding point an agent stops steering by path and walks the
/// rest in a straight line: `arrived` would otherwise stop it on the rim.
const BOARD_RADIUS_FACTOR: f32 = 3.0;

/// How long a rider may be airborne off its mover and keep the ride, so a hop or
/// a jolt mid-shaft does not lose its exit: the blocking pass's rider grace, so
/// a rider keeps its ride exactly as long as it keeps from stopping its mover.
const RIDE_AIRBORNE_GRACE_MS: f32 = crate::kinematic_mover::RIDER_GRACE_MS;

/// The shortest wait at a stop, in ticks, that surely holds a self-driven mover
/// at rest for one whole tick. A mover arrives part way through a tick, so a
/// shorter wait can begin and end between two tick boundaries, which is no dock.
const RESTING_WAIT_TICKS: f32 = 2.0;

/// What the ride code reads besides the agent: this tick's docks and movers.
#[derive(Clone, Copy)]
pub(super) struct RideWorld<'a> {
    pub gate: NavGate<'a>,
    pub collision: &'a CombinedCollisionWorld<'a>,
    pub registry: &'a EntityRegistry,
    pub dt: f32,
}

/// What the ride asks of this tick's motion.
pub(super) enum RideStep {
    /// Not riding: ordinary steering.
    Free,
    /// Stand on the mover.
    Hold,
    /// Walk at this velocity.
    Walk(Vec3),
}

/// Whether [`advance`] may move this agent this tick in place of path steering:
/// a ride under way, a boarding walk, or a body a mover carries off. The replan
/// admission reads it so a budget slot goes only to an agent that replans. Reads
/// the same predicates `board` and `enter_ride_if_carried` act on, and changes
/// nothing.
pub(super) fn takes_tick(agent: &AgentComponent, position: Vec3, world: RideWorld<'_>) -> bool {
    if agent.ride.is_some() {
        return true;
    }
    if let Some(plan) = agent.planned_ride {
        if standing_on(agent) == Some(plan.mover_id) {
            return true;
        }
        if boarding_open(plan, world.gate) && boarding_walk_due(agent, position, plan) {
            return true;
        }
    }
    carried_off(agent, world).is_some()
}

/// Advance the agent's board, ride and exit state by one tick. Runs before path
/// steering, so a ride that ends here hands the tick back to it.
pub(super) fn advance(
    agent: &mut AgentComponent,
    position: Vec3,
    world: RideWorld<'_>,
) -> RideStep {
    let standing_on = standing_on(agent);
    if agent.ride.is_none() {
        if let Some(step) = board(agent, position, world, standing_on) {
            return step;
        }
        // Boarding a planned ride above sets one; an unplanned ride never
        // replaces it.
        if agent.ride.is_none() {
            enter_ride_if_carried(agent, world);
        }
    }
    let Some(mut ride) = agent.ride else {
        return RideStep::Free;
    };
    if standing_on == Some(ride.mover_id) {
        agent.ride_airborne_ticks = 0;
    } else if agent.ground == GroundRef::Airborne
        && (agent.ride_airborne_ticks as f32) * world.dt * 1000.0 < RIDE_AIRBORNE_GRACE_MS
    {
        // A short hop off the deck keeps the ride and its exit.
        agent.ride_airborne_ticks += 1;
    } else {
        // Stepped onto the landing, knocked off, or airborne past the grace.
        // Either way the plan is over.
        leave_ride(agent);
        return RideStep::Free;
    }
    let Some(docked) = world.gate.docked_waypoint(ride.mover_id) else {
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
    if world.gate.moves_on_its_own(ride.mover_id) {
        return RideStep::Hold;
    }
    leave_ride(agent);
    RideStep::Free
}

/// The boarding walk. `Some` overrides path steering this tick.
fn board(
    agent: &mut AgentComponent,
    position: Vec3,
    world: RideWorld<'_>,
    standing_on: Option<u32>,
) -> Option<RideStep> {
    let plan = agent.planned_ride?;
    let open = boarding_open(plan, world.gate);
    if standing_on == Some(plan.mover_id) {
        let arrival = super::ARRIVAL_RADIUS_FACTOR * agent.radius;
        if open && distance_xz(position, plan.board_point) > arrival {
            // On the deck while it waits: walk well onto it before standing
            // still, so a lift leaving or an overhang does not catch the body
            // on the rim.
            return Some(walk_towards(agent, position, plan.board_point));
        }
        if !rests_at_its_stops(world.registry, plan.mover_id, world.dt) {
            give_up_boarding(agent);
            return None;
        }
        agent.planned_ride = None;
        clear_path(agent);
        start_ride(
            agent,
            AgentRide {
                mover_id: plan.mover_id,
                board_waypoint: Some(plan.board_waypoint),
                exit_waypoint: Some(plan.exit_waypoint),
                exit_point: plan.exit_point,
                departed: false,
            },
        );
        return None;
    }
    if !open {
        // The lift left, or was parked, before the agent stepped on. Never walk
        // into the shaft after it.
        give_up_boarding(agent);
        return None;
    }
    boarding_walk_due(agent, position, plan)
        .then(|| walk_towards(agent, position, plan.board_point))
}

/// A body a watched mover carries off rides it, planned or not: it was standing
/// on a lift as it left, or a riser came up under it.
fn enter_ride_if_carried(agent: &mut AgentComponent, world: RideWorld<'_>) {
    let Some(mover_id) = carried_off(agent, world) else {
        return;
    };
    agent.planned_ride = None;
    clear_path(agent);
    start_ride(
        agent,
        AgentRide {
            mover_id,
            board_waypoint: None,
            exit_waypoint: None,
            exit_point: Vec3::ZERO,
            departed: true,
        },
    );
}

/// The watched mover carrying this agent off, if one is and a ride on it ends.
/// A ride ends at the mover's next stop, so one that never rests at a stop
/// carries the body as plain ground and never starts a ride: no rider stays in
/// Ride past one leg's travel, or one run, of a moving mover.
fn carried_off(agent: &AgentComponent, world: RideWorld<'_>) -> Option<u32> {
    let mover_id = standing_on(agent)?;
    (world.gate.watches(mover_id)
        && carried_by_mover(world.collision, agent.ground)
        && rests_at_its_stops(world.registry, mover_id, world.dt))
    .then_some(mover_id)
}

/// Whether `mover_id`'s own motion ends each leg docked: a run that completes,
/// or a wait at each stop of at least [`RESTING_WAIT_TICKS`]. Scans the movers,
/// so it runs only as a ride is about to start.
fn rests_at_its_stops(registry: &EntityRegistry, mover_id: u32, dt: f32) -> bool {
    let resting_wait_ms = RESTING_WAIT_TICKS * dt * 1000.0;
    registry
        .iter_with_kind(ComponentKind::KinematicMover)
        .any(|(_, value)| match value {
            ComponentValue::KinematicMover(mover) => {
                mover.mover_id == mover_id
                    && (mover.mode == KinematicMoverMode::Once || mover.wait_ms >= resting_wait_ms)
            }
            _ => false,
        })
}

/// The lift a plan boards waits at the boarding dock and will leave by itself.
fn boarding_open(plan: PlannedRide, gate: NavGate<'_>) -> bool {
    gate.docked_at(plan.mover_id, plan.board_waypoint) && gate.moves_on_its_own(plan.mover_id)
}

/// Close enough to the boarding point, or done with the path, to walk on.
fn boarding_walk_due(agent: &AgentComponent, position: Vec3, plan: PlannedRide) -> bool {
    agent.arrived || distance_xz(position, plan.board_point) <= BOARD_RADIUS_FACTOR * agent.radius
}

fn standing_on(agent: &AgentComponent) -> Option<u32> {
    match agent.ground {
        GroundRef::Mover(mover_id) => Some(mover_id),
        _ => None,
    }
}

fn walk_towards(agent: &AgentComponent, position: Vec3, target: Vec3) -> RideStep {
    let offset = Vec3::new(target.x - position.x, 0.0, target.z - position.z);
    RideStep::Walk(offset.normalize_or_zero() * agent.move_speed)
}

fn start_ride(agent: &mut AgentComponent, ride: AgentRide) {
    agent.ride = Some(ride);
    agent.ride_airborne_ticks = 0;
}

/// Drop a ride plan that can no longer board, and owe a replan at once: a lift
/// a script parked may never flip a gate again to wake the agent.
fn give_up_boarding(agent: &mut AgentComponent) {
    agent.planned_ride = None;
    clear_path(agent);
    agent.blocked = true;
    agent.planned_destination = None;
    agent.steer_velocity = Vec3::ZERO;
}

fn leave_ride(agent: &mut AgentComponent) {
    agent.ride = None;
    agent.ride_airborne_ticks = 0;
    agent.planned_destination = None;
    clear_path(agent);
}

fn clear_path(agent: &mut AgentComponent) {
    agent.path.clear();
    agent.path_crossings.clear();
    agent.mandatory_waypoints.clear();
    agent.waypoint_cursor = 0;
    agent.arrived = false;
}
