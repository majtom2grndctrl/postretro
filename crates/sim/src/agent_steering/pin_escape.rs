// Pinned agents step clear: a living agent a non-crush mover yields to walks out
// of that mover's course, whatever its motion verb or knockback response, so a
// hold always ends. With nowhere to go it yields as a corpse does.
// See: context/lib/movement.md §6

use glam::Vec3;

use crate::collision::CollisionCapsule;
use crate::collision::moving::CombinedCollisionWorld;
use crate::kinematic_mover::{AgentMoverPair, find_mover_clear_point};
use crate::nav::distance_xz;
use postretro_entities::components::agent::{AgentComponent, PinEscape};
use postretro_entities::{EntityRegistry, Transform};

/// How far a pinned agent searches, as seconds of its own travel. Short of the
/// two seconds a mover may wait on it, with room left for the mover to finish.
const ESCAPE_REACH_SECONDS: f32 = 1.25;

/// Ticks an agent may spend stepping clear before it yields as a corpse, so a
/// crowd or a second mover in the way cannot hold the first one forever.
const ESCAPE_TIMEOUT_TICKS: u32 = 90;

/// Arrival band around the clear point, in metres.
const ESCAPE_ARRIVAL_RADIUS: f32 = 0.1;

/// Answer this tick's yields: each living agent newly pinned gets a clear point
/// to walk to, or joins `corpse_yields` when it has none. Search runs on the pin
/// edge, so an agent already stepping clear costs a lookup.
pub(super) fn plan_escapes(
    registry: &mut EntityRegistry,
    collision: &CombinedCollisionWorld<'_>,
    yields: &[AgentMoverPair],
    corpse_yields: &mut Vec<AgentMoverPair>,
) {
    for &(id, mover_id) in yields {
        let Ok(agent) = registry.get_component::<AgentComponent>(id) else {
            continue;
        };
        if agent.pin_escape.is_some() {
            continue;
        }
        let Ok(position) = registry.get_component::<Transform>(id).map(|t| t.position) else {
            continue;
        };
        let clear_point = find_mover_clear_point(
            registry,
            collision,
            mover_id,
            position,
            CollisionCapsule::new(agent.radius, agent.half_height()),
            agent.step_height,
            agent.move_speed * ESCAPE_REACH_SECONDS,
        );
        let Some(target) = clear_point else {
            corpse_yields.push((id, mover_id));
            continue;
        };
        let mut updated = agent.clone();
        updated.pin_escape = Some(PinEscape {
            mover_id,
            target,
            ticks_left: ESCAPE_TIMEOUT_TICKS,
        });
        // A latched `blocked` is no reason to stand under a mover. Replan from
        // wherever the walk ends.
        updated.blocked = false;
        updated.planned_destination = None;
        let _ = registry.set_component(id, updated);
    }
}

/// The mover and velocity that walk `agent` to its clear point this tick, or `None` once
/// the walk is over and authored motion resumes. A timed-out walk joins
/// `corpse_yields`. Ignores `control` and knockback scale on purpose: both can
/// be zero, and a hold must still end.
pub(super) fn escape_velocity(
    id: postretro_entities::EntityId,
    agent: &mut AgentComponent,
    position: Vec3,
    corpse_yields: &mut Vec<AgentMoverPair>,
) -> Option<(u32, Vec3)> {
    let escape = agent.pin_escape.as_mut()?;
    let remaining = escape.target - position;
    if distance_xz(escape.target, position) <= ESCAPE_ARRIVAL_RADIUS {
        agent.pin_escape = None;
        return None;
    }
    if escape.ticks_left == 0 {
        corpse_yields.push((id, escape.mover_id));
        agent.pin_escape = None;
        return None;
    }
    escape.ticks_left -= 1;
    let walk = Vec3::new(remaining.x, 0.0, remaining.z).normalize_or_zero() * agent.move_speed;
    Some((escape.mover_id, walk))
}
