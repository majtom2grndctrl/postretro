// Corpse motion: a quiescent agent keeps gravity, mover carry and displacement,
// and takes no steering.
// See: context/lib/movement.md §6 · context/lib/entity_model.md §7

use glam::Vec3;

use crate::collision::moving::CombinedCollisionWorld;
use crate::movement::agent_motion::{AgentCapsule, settle_corpse};
use crate::scripting_systems::health::is_quiescent;
use postretro_collision::EnvironmentView;
use postretro_entities::components::agent::AgentComponent;
use postretro_entities::{ComponentKind, EntityId, EntityRegistry, Transform};

/// Move every corpse one tick. Its path and destination stay as they were, so a
/// revived agent resumes them; the list stays unallocated in a tick with no
/// corpses.
pub(super) fn tick_corpses(
    registry: &mut EntityRegistry,
    collision: &CombinedCollisionWorld<'_>,
    environment: EnvironmentView<'_>,
    dt: f32,
) {
    let corpses: Vec<EntityId> = registry
        .iter_with_kind(ComponentKind::Agent)
        .map(|(id, _)| id)
        .filter(|id| !registry.is_inventory_owned(*id) && is_quiescent(registry, *id))
        .collect();
    for id in corpses {
        move_corpse(registry, id, collision, environment, dt);
    }
}

fn move_corpse(
    registry: &mut EntityRegistry,
    id: EntityId,
    collision: &CombinedCollisionWorld<'_>,
    environment: EnvironmentView<'_>,
    dt: f32,
) {
    let Ok(component) = registry.get_component::<AgentComponent>(id) else {
        return;
    };
    let Ok(transform) = registry.get_component::<Transform>(id).copied() else {
        return;
    };
    let mut agent = component.clone();
    let capsule = AgentCapsule {
        radius: agent.radius,
        half_height: agent.half_height(),
        step_height: agent.step_height,
    };
    // Only the fall survives death: `velocity` holds knockback in its total.
    let vertical = agent.velocity.y - agent.knockback_velocity.y;
    agent.knockback_velocity = Vec3::ZERO;
    agent.steer_velocity = Vec3::ZERO;
    agent.stuck_ticks = 0;
    agent.unstick_window_remaining = 0;
    agent.pin_escape = None;
    let result = settle_corpse(
        collision,
        &capsule,
        transform.position,
        agent.ground,
        vertical,
        environment,
        dt,
    );
    agent.velocity = Vec3::new(0.0, result.velocity.y, 0.0);
    agent.ground = result.ground;
    if let Some(overlap) = &result.overlap {
        crate::sim::report_static_overlap(collision.static_world, registry, id, overlap);
    }
    let _ = registry.set_component(
        id,
        Transform {
            position: result.position,
            ..transform
        },
    );
    let _ = registry.set_component(id, agent);
}
