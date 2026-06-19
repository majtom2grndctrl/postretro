// Movable navigation agent component: live kinematic state for an entity that
// follows a baked-navmesh path through the collision world. Engine-internal —
// never reachable through `worldQuery` (the `PlayerMovement` precedent).
//
// Carries the agent's collision capsule (radius / height / step_height), its
// current velocity and grounded flag, the path it is following (an ordered list
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

use crate::nav::NavAgentParams;
use crate::scripting::registry::{EntityId, EntityRegistry, RegistryError};

/// Live state for one movable navigation agent.
///
/// The capsule geometry (`radius`, `height`, `step_height`) is fixed at attach
/// time from the baked agent parameters and never mutated by the steering tick;
/// `velocity`, `is_grounded`, the path/cursor, and `destination` are the live
/// fields the steering system writes each tick.
///
/// `radius`/`height` are the *total* capsule dimensions. The parry `Capsule`
/// the collide-and-slide harness builds takes a HALF-HEIGHT (center to one
/// endpoint, excluding the end-cap sphere): `height / 2.0 - radius`, mirroring
/// the player movement capsule construction (`movement::integrate_collision`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub(crate) struct AgentComponent {
    /// Capsule radius, in world units. Seeded from `NavAgentParams::radius`.
    pub(crate) radius: f32,
    /// Total capsule height (end-cap to end-cap), in world units. Seeded from
    /// `NavAgentParams::height`. Convert to a parry half-height with
    /// `height / 2.0 - radius` before building a `parry3d::shape::Capsule`.
    pub(crate) height: f32,
    /// Maximum vertical step the agent can climb in one tick, in world units.
    /// Seeded from `NavAgentParams::step_height` (NOT the radius as a
    /// stand-in). The collide-and-slide harness's step-up / ground-stick logic
    /// reads this; it is a stored field so downstream steering uses the baked
    /// step the navmesh was eroded for.
    pub(crate) step_height: f32,
    /// Top horizontal speed the steering system drives the agent at, in
    /// world-units/sec. Passed to `attach_agent` at attach time.
    pub(crate) move_speed: f32,
    /// Live velocity (world space) the collide-and-slide harness integrates and
    /// resolves each tick.
    pub(crate) velocity: Vec3,
    /// Grounded flag resolved by the harness's ground-stick down-cast. `true`
    /// when the agent's capsule rests on a walkable floor.
    pub(crate) is_grounded: bool,
    /// Current path: ordered world-space waypoints from the agent toward its
    /// destination, produced by the pathfinding query. Empty when the agent has
    /// no active path.
    pub(crate) path: Vec<Vec3>,
    /// Cursor into `path`: the index of the waypoint the agent is currently
    /// steering toward. Advanced on arrival-radius by the steering system.
    pub(crate) waypoint_cursor: usize,
    /// Where the agent is trying to go. `None` when idle (no destination set);
    /// the steering system plans a path to it when `Some`.
    pub(crate) destination: Option<Vec3>,
    /// The destination the current `path` was planned for. Lets the steering
    /// system detect a destination move (replan immediately) versus a stale
    /// path to the same goal (replan only after the staleness window). `None`
    /// when no plan has been attempted for the live destination yet.
    pub(crate) planned_destination: Option<Vec3>,
    /// Ticks remaining before the agent may re-spend a replan-budget slot. The
    /// staleness gate: after a successful or FAILED plan, this is set to the
    /// staleness window so a single agent (and especially a permanently-blocked
    /// one) cannot re-qualify for a replan every tick. Decremented each tick;
    /// a destination move resets it to 0 (replan now). See the steering tick.
    pub(crate) replan_cooldown_ticks: u32,
    /// `true` once the agent has reached its destination (within the arrival
    /// radius of the final waypoint). Cleared when a new destination is set.
    pub(crate) arrived: bool,
    /// `true` when the agent has a destination but pathfinding found no route
    /// to it (an empty path that survived a replan attempt). The agent holds
    /// position rather than walking into geometry. Cleared on a new
    /// destination or a successful plan.
    pub(crate) blocked: bool,
}

impl AgentComponent {
    /// Construct an agent from explicit capsule dimensions plus the per-tick
    /// `step_height` and `move_speed`. Takes the geometry as plain arguments —
    /// it deliberately does NOT call `NavGraph::agent_params()`; the spawn call
    /// site reads the baked params and passes them down (see [`attach_agent`]).
    /// Velocity starts at rest, ungrounded (the harness re-acquires the floor on
    /// the first tick), with no path and no destination.
    pub(crate) fn new(radius: f32, height: f32, step_height: f32, move_speed: f32) -> Self {
        Self {
            radius,
            height,
            step_height,
            move_speed,
            velocity: Vec3::ZERO,
            is_grounded: false,
            path: Vec::new(),
            waypoint_cursor: 0,
            destination: None,
            planned_destination: None,
            replan_cooldown_ticks: 0,
            arrived: false,
            blocked: false,
        }
    }

    /// Construct an agent from baked navmesh agent parameters. Seeds the capsule
    /// (`radius`, `height`) and `step_height` from `params`; the navmesh records
    /// the canonical agent the floor was eroded for, so an agent built from it
    /// fits the walkable surface.
    pub(crate) fn from_nav_params(params: &NavAgentParams, move_speed: f32) -> Self {
        Self::new(params.radius, params.height, params.step_height, move_speed)
    }

    /// parry half-height for this capsule: distance from the capsule center to
    /// one cylinder endpoint, EXCLUDING the hemispherical end cap. Mirrors the
    /// player movement capsule (`height / 2.0 - radius`). The collide-and-slide
    /// harness builds its `parry3d::shape::Capsule` from this.
    pub(crate) fn half_height(&self) -> f32 {
        // A well-baked agent has `radius < height / 2`, so the cylinder section is
        // non-negative. A bad bake (`radius >= height / 2`) would invert the parry
        // capsule endpoints and silently mis-shape collide-and-slide; clamp at 0 so
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
/// is the single attach point both the `dev-tools` debug spawner and plan 2's
/// data-archetype attach call — the baked-params read happens at the call site
/// (`NavGraph::agent_params()`), never inside the component or the harness.
///
/// Returns `GenerationMismatch` / `EntityNotFound` for a stale or unknown
/// entity (the registry's standard validation), matching the other registry
/// mutators. The entity keeps whatever other components it already carries —
/// `attach_agent` only inserts the agent column.
pub(crate) fn attach_agent(
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
    use crate::scripting::registry::Transform;

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
        assert!(!agent.is_grounded);
        assert!(agent.path.is_empty());
        assert_eq!(agent.waypoint_cursor, 0);
        assert_eq!(agent.destination, None);
        assert_eq!(agent.planned_destination, None);
        assert_eq!(agent.replan_cooldown_ticks, 0);
        assert!(!agent.arrived);
        assert!(!agent.blocked);
    }

    #[test]
    fn from_nav_params_copies_capsule_and_step_height() {
        let agent = AgentComponent::from_nav_params(&sample_params(), 6.5);
        assert_eq!(agent.radius, 0.35);
        assert_eq!(agent.height, 1.8);
        assert_eq!(agent.step_height, 0.4, "step_height seeded from params");
        assert_eq!(agent.move_speed, 6.5);
    }

    #[test]
    fn half_height_excludes_end_cap_sphere() {
        // parry half-height = height/2 - radius (player-capsule convention).
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
