// Movable navigation agent component: live kinematic state for an entity that
// follows a baked-navmesh path through the collision world. Engine-internal —
// never reachable through `worldQuery` (the `PlayerMovement` precedent).
//
// Carries the agent's collision capsule (radius / height / step_height), its
// current velocity and ground, the path it is following (an ordered list
// of world-space waypoints plus a cursor into them), and a destination. The
// capsule is seeded at attach time from the navmesh's baked `NavAgentParams`,
// so each agent matches what the bake eroded the navmesh for; storing it per
// agent (rather than reading one global size each tick) leaves a future
// per-archetype size override a descriptor field away.
//
// See: context/lib/entity_model.md §7 (collision), §7b (engine-internal
//      component, no script surface)
//      context/lib/movement.md §1 (custom-kinematic capsule, collision-only)

use glam::Vec3;
use serde::{Deserialize, Serialize};

use crate::registry::{EntityId, EntityRegistry, RegistryError};
use postretro_foundation::{GroundRef, KnockbackResponse, NavAgentParams};

/// A pinned agent's walk out of a mover's course. Host-only.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct PinEscape {
    /// The mover that pinned it.
    pub mover_id: u32,
    /// The clear point it walks to.
    pub target: Vec3,
    /// Ticks left before it gives up and yields as a corpse.
    pub ticks_left: u32,
}

/// A ride a planned path ends in: board `mover_id` where it docks now, get off
/// at its other dock. Host-only.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct PlannedRide {
    pub mover_id: u32,
    pub board_waypoint: u32,
    pub exit_waypoint: u32,
    /// A point on the mover's top, past the boarding edge.
    pub board_point: Vec3,
    /// A point on the landing, past the exit edge.
    pub exit_point: Vec3,
}

/// What a [`PathCrossing`] crosses.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum CrossingKind {
    /// A portal a mover's dock can close; `index` is the portal's.
    Portal,
    /// A step onto or off a mover; `index` is the link's. Closed unless the
    /// mover docks at the link's waypoint.
    Link,
}

/// One place a route depends on a mover's dock: a gated portal it walks
/// through, or a link it walks over. Host-only.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct PathCrossing {
    pub kind: CrossingKind,
    /// Index of the portal or link in the level's navigation graph.
    pub index: u32,
    /// Index into the agent's `path` of the waypoint it walks toward while
    /// crossing. Once the cursor is past it the crossing is behind the agent,
    /// and a mover closing it changes nothing about the route ahead.
    pub waypoint: u32,
}

/// A ride under way: the agent stands on `mover_id` and does not steer. Host-only.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct AgentRide {
    pub mover_id: u32,
    /// Where it got on, when it boarded by plan.
    pub board_waypoint: Option<u32>,
    /// Where it means to get off; `None` for a body a mover simply carried off.
    pub exit_waypoint: Option<u32>,
    pub exit_point: Vec3,
    /// The mover has left the dock it boarded at.
    pub departed: bool,
}

/// Live state for one movable navigation agent.
///
/// The capsule geometry (`radius`, `height`, `step_height`) is fixed at attach
/// time from the baked agent parameters and never mutated by the steering tick;
/// `velocity`, `steer_velocity`, stuck-recovery counters, `ground`, the
/// path/cursor, and `destination` are the live fields the steering system
/// writes each tick.
///
/// `radius`/`height` describe the engine-owned collision capsule, with `height`
/// measured end-cap to end-cap. Its center-to-cylinder-endpoint span is
/// `height / 2.0 - radius`, matching the player collision capsule semantics.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AgentComponent {
    /// Capsule radius, in world units. Seeded from `NavAgentParams::radius`.
    pub radius: f32,
    /// Total collision-capsule height (end-cap to end-cap), in world units.
    /// Seeded from `NavAgentParams::height`. The collision world derives its
    /// center-to-cylinder-endpoint span as `height / 2.0 - radius`.
    pub height: f32,
    /// Maximum vertical step the agent can climb in one tick, in world units.
    /// Seeded from `NavAgentParams::step_height` (NOT the radius as a
    /// stand-in). The agent controller's step-up / ground-stick logic reads
    /// this; it is a stored field so downstream steering uses the baked step
    /// the navmesh was eroded for.
    pub step_height: f32,
    /// Top horizontal speed the steering system drives the agent at, in
    /// world-units/sec. Passed to `attach_agent` at attach time.
    pub move_speed: f32,
    /// Live velocity (world space) the agent controller integrates and resolves
    /// each tick.
    pub velocity: Vec3,
    /// External velocity protected from path steering's horizontal replacement.
    #[serde(default)]
    pub knockback_velocity: Vec3,
    #[serde(default)]
    pub knockback: KnockbackResponse,
    /// Pre-collision, smoothed XZ path-following velocity integrated by the
    /// steering system. Separation is added transiently after this value and is
    /// never folded back into it, so heading/acceleration state stays free of
    /// collision and neighbor-response jitter.
    #[serde(default)]
    pub steer_velocity: Vec3,
    /// Consecutive ticks where the agent intended to move along a valid path
    /// but collision resolution produced too little goal-projected progress.
    /// The steering system resets this on real progress, idle/no-route/arrival
    /// gates, and recovery fire.
    #[serde(default)]
    pub stuck_ticks: u32,
    /// Remaining ticks in the bounded tangent-slide recovery window. While
    /// nonzero, stuck detection is suspended and steering adds a deterministic
    /// lateral bias before collision resolution.
    #[serde(default)]
    pub unstick_window_remaining: u32,
    /// What the agent's capsule rests on, resolved by the agent controller. The single
    /// source for mover carry, rider exemption, stuck suppression and link
    /// resolution; [`Self::is_grounded`] is its projection.
    #[serde(default)]
    pub ground: GroundRef,
    /// Current path: ordered world-space waypoints from the agent toward its
    /// destination, produced by the pathfinding query. Empty when the agent has
    /// no active path.
    pub path: Vec<Vec3>,
    /// Parallel to `path`: `true` marks clearance geometry that steering must
    /// reach without arrival-radius skipping or lookahead shortcutting. Empty
    /// is equivalent to all `false` for legacy/manual paths.
    #[serde(default)]
    pub mandatory_waypoints: Vec<bool>,
    /// Cursor into `path`: the index of the waypoint the agent is currently
    /// steering toward. Advanced on arrival-radius by the steering system.
    pub waypoint_cursor: usize,
    /// Where the agent is trying to go. `None` when idle (no destination set);
    /// the steering system plans a path to it when `Some`.
    pub destination: Option<Vec3>,
    /// The destination the current `path` was planned for. The steering tick
    /// replans when the live `destination` drifts beyond a threshold from THIS
    /// position (cumulative drift-from-the-plan, not the per-call delta), so a
    /// goal that creeps each tick keeps the existing path until the drift adds
    /// up; a stale path to the same goal is otherwise refreshed only after the
    /// staleness window. `None` when no plan has been attempted for the live
    /// destination yet (forces the first plan).
    pub planned_destination: Option<Vec3>,
    /// Ticks remaining before the agent may re-spend a replan-budget slot. The
    /// staleness gate: after a successful or FAILED plan, this is set to the
    /// staleness window so a single agent (and especially a permanently-blocked
    /// one) cannot re-qualify for a replan every tick. Decremented each tick.
    /// See the steering tick.
    pub replan_cooldown_ticks: u32,
    /// `true` once the agent has reached the end of its current path (within the
    /// arrival radius of the final waypoint). Cleared on a successful replan or
    /// when the destination is cleared. If the destination moves after arrival,
    /// the move is a DRIFT-driven replan: the steering tick prioritizes drift over
    /// staleness-only refreshes, so the agent re-acquires the moved target the
    /// same tick rather than waiting behind no-op refreshers. It can be deferred
    /// only in a true overload — MORE than the replan budget of agents genuinely
    /// drifting the same tick — never crowded out by a staleness refresh.
    pub arrived: bool,
    /// `true` when the agent has a destination, pathfinding found no route to
    /// it, AND the agent holds no path to keep following. A failed path
    /// REFRESH keeps the previous route instead (stale-but-moving), so
    /// `blocked` implies an empty `path` by construction. A pathless blocked
    /// agent holds position rather than walking into geometry; the state is
    /// transient — the steering tick keeps retrying under its replan cooldown
    /// and drift/topology gates. Cleared on a successful plan or a cleared
    /// destination.
    pub blocked: bool,
    /// The walk out of a pinning mover's course, while one runs. It overrides
    /// path steering and any motion verb.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pin_escape: Option<PinEscape>,
    /// The gated portals and links `path` crosses, in order, each with the
    /// waypoint it is crossed toward. Lets a gate change find the agents whose
    /// route ahead it closes without re-resolving their paths. Empty for a path
    /// that depends on no mover, and for a manual path.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub path_crossings: Vec<PathCrossing>,
    /// The ride the current path ends in, if any.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub planned_ride: Option<PlannedRide>,
    /// The ride under way, if any. While set the agent neither steers nor replans.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ride: Option<AgentRide>,
    /// Consecutive ticks a rider has spent airborne off its mover. A ride
    /// survives a short hop, as the blocking pass's rider grace does.
    #[serde(default, skip_serializing_if = "is_zero_u32")]
    pub ride_airborne_ticks: u32,
    /// The gate generation this agent last answered. Behind the live one while
    /// a gate change still owes it a replan, so any number of flips before
    /// admission cost one.
    #[serde(default, skip_serializing_if = "is_zero")]
    pub gate_generation: u64,
}

fn is_zero(value: &u64) -> bool {
    *value == 0
}

fn is_zero_u32(value: &u32) -> bool {
    *value == 0
}

impl AgentComponent {
    /// Construct an agent from explicit capsule dimensions plus the per-tick
    /// `step_height` and `move_speed`. Takes the geometry as plain arguments —
    /// it deliberately does NOT call `NavGraph::agent_params()`; the spawn call
    /// site reads the baked params and passes them down (see [`attach_agent`]).
    /// Velocity starts at rest, ungrounded (the agent controller re-acquires the
    /// floor on the first tick), with no path and no destination.
    pub fn new(radius: f32, height: f32, step_height: f32, move_speed: f32) -> Self {
        Self {
            radius,
            height,
            step_height,
            move_speed,
            velocity: Vec3::ZERO,
            knockback_velocity: Vec3::ZERO,
            knockback: KnockbackResponse::default(),
            steer_velocity: Vec3::ZERO,
            stuck_ticks: 0,
            unstick_window_remaining: 0,
            ground: GroundRef::Airborne,
            pin_escape: None,
            path_crossings: Vec::new(),
            planned_ride: None,
            ride: None,
            ride_airborne_ticks: 0,
            gate_generation: 0,
            path: Vec::new(),
            mandatory_waypoints: Vec::new(),
            waypoint_cursor: 0,
            destination: None,
            planned_destination: None,
            replan_cooldown_ticks: 0,
            arrived: false,
            blocked: false,
        }
    }

    /// Add a scaled world-space velocity impulse, atomically rejecting invalid sums.
    pub fn add_knockback(&mut self, impulse: Vec3) -> bool {
        let impulse = impulse * self.knockback.scale;
        let velocity = self.velocity + impulse;
        let knockback_velocity = self.knockback_velocity + impulse;
        if !impulse.is_finite()
            || impulse == Vec3::ZERO
            || !velocity.is_finite()
            || !knockback_velocity.is_finite()
            || !velocity.length_squared().is_finite()
            || !knockback_velocity.length_squared().is_finite()
        {
            return false;
        }
        self.velocity = velocity;
        self.knockback_velocity = knockback_velocity;
        // A mover ground stays until the next tick carries the body once more
        // and hands it the mover's release velocity; world contact has no
        // release, so it clears at once. The player's knockback does the same.
        if impulse.y > 0.0 && velocity.y > 0.0 && !matches!(self.ground, GroundRef::Mover(_)) {
            self.ground = GroundRef::Airborne;
        }
        true
    }

    /// Whether the capsule rests on a walkable floor, static or mover.
    pub fn is_grounded(&self) -> bool {
        self.ground.is_grounded()
    }

    /// Construct an agent from baked navmesh agent parameters. Seeds the capsule
    /// (`radius`, `height`) and `step_height` from `params`; the navmesh records
    /// the canonical agent the floor was eroded for, so an agent built from it
    /// fits the walkable surface.
    pub fn from_nav_params(params: &NavAgentParams, move_speed: f32) -> Self {
        Self::new(params.radius, params.height, params.step_height, move_speed)
    }

    /// Collision-capsule center-to-cylinder-endpoint span, excluding the
    /// hemispherical end cap. This mirrors the player capsule's
    /// `height / 2.0 - radius` world-collision semantics.
    pub fn half_height(&self) -> f32 {
        // A well-baked agent has `radius < height / 2`, so the cylinder section is
        // non-negative. A bad bake (`radius >= height / 2`) would invert the
        // capsule endpoints and silently mis-shape world collision; clamp at 0 so
        // the worst case degrades to a sphere rather than an inverted capsule.
        debug_assert!(
            self.height / 2.0 - self.radius >= 0.0,
            "agent capsule radius {} >= height/2 {} (bad navmesh bake)",
            self.radius,
            self.height / 2.0
        );
        (self.height / 2.0 - self.radius).max(0.0)
    }
}

/// Public spawn seam: insert an [`AgentComponent`] on an existing entity, with
/// its capsule and `step_height` seeded from the passed `NavAgentParams`. This
/// is the single attach point both the `dev-tools` debug spawner and the
/// data-archetype attach call (`scripting/builtins/data_archetype.rs`) use —
/// the baked-params read happens at the call site
/// (`NavGraph::agent_params()`), never inside the component or the harness.
///
/// Returns `GenerationMismatch` / `EntityNotFound` for a stale or unknown
/// entity (the registry's standard validation), matching the other registry
/// mutators. The entity keeps whatever other components it already carries —
/// `attach_agent` only inserts the agent column.
pub fn attach_agent(
    registry: &mut EntityRegistry,
    entity: EntityId,
    params: &NavAgentParams,
    move_speed: f32,
) -> Result<(), RegistryError> {
    registry.set_component(entity, AgentComponent::from_nav_params(params, move_speed))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::registry::Transform;

    fn sample_params() -> NavAgentParams {
        NavAgentParams {
            radius: 0.35,
            height: 1.8,
            step_height: 0.4,
            max_slope_deg: 45.0,
        }
    }

    #[test]
    fn new_seeds_geometry_and_starts_at_rest() {
        let agent = AgentComponent::new(0.3, 1.6, 0.35, 5.0);
        assert_eq!(agent.radius, 0.3);
        assert_eq!(agent.height, 1.6);
        assert_eq!(agent.step_height, 0.35);
        assert_eq!(agent.move_speed, 5.0);
        assert_eq!(agent.velocity, Vec3::ZERO);
        assert_eq!(agent.steer_velocity, Vec3::ZERO);
        assert_eq!(agent.stuck_ticks, 0);
        assert_eq!(agent.unstick_window_remaining, 0);
        assert!(!agent.is_grounded());
        assert!(agent.path.is_empty());
        assert!(agent.mandatory_waypoints.is_empty());
        assert_eq!(agent.waypoint_cursor, 0);
        assert_eq!(agent.destination, None);
        assert_eq!(agent.planned_destination, None);
        assert_eq!(agent.replan_cooldown_ticks, 0);
        assert!(!agent.arrived);
        assert!(!agent.blocked);
    }

    #[test]
    fn agent_serde_defaults_missing_back_compat_fields() {
        use crate::registry::ComponentValue;

        let value = ComponentValue::Agent(AgentComponent::new(0.3, 1.6, 0.35, 5.0));
        let mut json = serde_json::to_value(&value).unwrap();
        json.as_object_mut().unwrap().remove("steer_velocity");
        json.as_object_mut().unwrap().remove("stuck_ticks");
        json.as_object_mut()
            .unwrap()
            .remove("unstick_window_remaining");
        json.as_object_mut().unwrap().remove("mandatory_waypoints");

        let ComponentValue::Agent(back) = serde_json::from_value(json).unwrap() else {
            panic!("expected agent component");
        };
        assert_eq!(back.steer_velocity, Vec3::ZERO);
        assert_eq!(back.stuck_ticks, 0);
        assert_eq!(back.unstick_window_remaining, 0);
        assert!(back.mandatory_waypoints.is_empty());
    }

    #[test]
    fn from_nav_params_copies_capsule_and_step_height() {
        let agent = AgentComponent::from_nav_params(&sample_params(), 6.5);
        assert_eq!(agent.radius, 0.35);
        assert_eq!(agent.height, 1.8);
        assert_eq!(agent.step_height, 0.4, "step_height seeded from params");
        assert_eq!(agent.move_speed, 6.5);
    }

    // Regression: an upward shove dropped a mover ground at once, so the next
    // tick neither carried the agent nor handed it the mover's release velocity.
    #[test]
    fn an_upward_shove_keeps_a_mover_ground_and_clears_a_world_one() {
        let mut on_mover = AgentComponent::new(0.35, 1.8, 0.4, 5.0);
        on_mover.ground = GroundRef::Mover(3);
        assert!(on_mover.add_knockback(Vec3::Y * 4.0));
        assert_eq!(on_mover.ground, GroundRef::Mover(3));

        let mut on_floor = AgentComponent::new(0.35, 1.8, 0.4, 5.0);
        on_floor.ground = GroundRef::World;
        assert!(on_floor.add_knockback(Vec3::Y * 4.0));
        assert_eq!(on_floor.ground, GroundRef::Airborne);
    }

    #[test]
    fn half_height_excludes_end_cap_sphere() {
        // Engine-owned collision-capsule half-height = height/2 - radius.
        let agent = AgentComponent::new(0.35, 1.8, 0.4, 5.0);
        assert!((agent.half_height() - (0.9 - 0.35)).abs() < 1e-6);
    }

    #[test]
    fn attach_agent_inserts_component_seeded_from_params() {
        let mut reg = EntityRegistry::new();
        let id = reg.spawn(Transform::default());
        attach_agent(&mut reg, id, &sample_params(), 4.0).unwrap();

        let agent = reg.get_component::<AgentComponent>(id).unwrap();
        assert_eq!(agent.radius, 0.35);
        assert_eq!(agent.height, 1.8);
        assert_eq!(agent.step_height, 0.4);
        assert_eq!(agent.move_speed, 4.0);
    }

    #[test]
    fn attach_agent_rejects_stale_entity() {
        let mut reg = EntityRegistry::new();
        let id = reg.spawn(Transform::default());
        reg.despawn(id).unwrap();
        assert_eq!(
            attach_agent(&mut reg, id, &sample_params(), 4.0),
            Err(RegistryError::GenerationMismatch(id))
        );
    }
}
