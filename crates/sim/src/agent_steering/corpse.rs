// Corpse motion: a quiescent agent keeps gravity, mover carry and displacement,
// and takes no steering.
// See: context/lib/movement.md §6 · context/lib/entity_model.md §7

use glam::Vec3;

use crate::collision::moving::CombinedCollisionWorld;
use crate::movement::agent_motion::{AgentCapsule, settle_corpse};
use crate::scripting_systems::health::is_quiescent;
use postretro_collision::EnvironmentView;
use postretro_entities::{ComponentKind, ComponentValue, EntityId, EntityRegistry, Transform};

/// Corpses gathered per registry scan. Moving one needs the registry mutably, so
/// ids are gathered first; a fixed batch keeps that list off the heap, and a
/// level with more corpses rescans from where the batch stopped.
const CORPSE_BATCH: usize = 32;

/// Move every corpse one tick. Its path and destination stay as they were, so a
/// revived agent resumes them. Allocates nothing.
pub(super) fn tick_corpses(
    registry: &mut EntityRegistry,
    collision: &CombinedCollisionWorld<'_>,
    environment: EnvironmentView<'_>,
    dt: f32,
) {
    // Agents already scanned. Moving a corpse changes no entity's agent column
    // or quiescence, so the scan order is stable across batches.
    let mut scanned = 0;
    loop {
        let mut batch = [EntityId::from_raw(0); CORPSE_BATCH];
        let mut gathered = 0;
        let mut exhausted = true;
        for (id, _) in registry.iter_with_kind(ComponentKind::Agent).skip(scanned) {
            if gathered == CORPSE_BATCH {
                exhausted = false;
                break;
            }
            scanned += 1;
            if !registry.is_inventory_owned(id) && is_quiescent(registry, id) {
                batch[gathered] = id;
                gathered += 1;
            }
        }
        for &id in &batch[..gathered] {
            move_corpse(registry, id, collision, environment, dt);
        }
        if exhausted {
            return;
        }
    }
}

fn move_corpse(
    registry: &mut EntityRegistry,
    id: EntityId,
    collision: &CombinedCollisionWorld<'_>,
    environment: EnvironmentView<'_>,
    dt: f32,
) {
    let Ok(transform) = registry.get_component::<Transform>(id).copied() else {
        return;
    };
    // Mutated in place: the component keeps its path for a revival, and a clone
    // would copy it every tick.
    let Ok(ComponentValue::Agent(agent)) =
        registry.get_component_value_mut(id, ComponentKind::Agent)
    else {
        return;
    };
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
    agent.planned_ride = None;
    agent.ride = None;
    agent.ride_airborne_ticks = 0;
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
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::alloc_probe::AllocSnapshot;
    use crate::collision::{CollisionWorld, SKIN_DISTANCE};
    use postretro_entities::components::agent::{AgentComponent, CrossingKind, PathCrossing};
    use postretro_entities::components::health::HealthComponent;

    const DT: f32 = 1.0 / 60.0;
    const ENV: EnvironmentView<'static> = EnvironmentView::level_only(-20.0);

    fn floor_world() -> CollisionWorld {
        let span = 60.0;
        CollisionWorld::from_triangles_for_test(
            vec![
                Vec3::new(-span, 0.0, -span),
                Vec3::new(span, 0.0, -span),
                Vec3::new(span, 0.0, span),
                Vec3::new(-span, 0.0, span),
            ],
            vec![[0, 2, 1], [0, 3, 2]],
        )
    }

    fn spawn_corpse(registry: &mut EntityRegistry, position: Vec3) -> EntityId {
        let id = registry.spawn(Transform {
            position,
            ..Transform::default()
        });
        let mut agent = AgentComponent::new(0.35, 1.8, 0.4, 4.0);
        // Kept for a revival, so a per-tick clone would allocate.
        agent.path = vec![position, position + Vec3::X];
        agent.mandatory_waypoints = vec![false, false];
        agent.path_crossings = vec![PathCrossing {
            kind: CrossingKind::Portal,
            index: 0,
            waypoint: 1,
        }];
        registry.set_component(id, agent).unwrap();
        registry
            .set_component(
                id,
                HealthComponent {
                    max: 100.0,
                    current: 0.0,
                    hitbox: None,
                    death_handled: false,
                    pending_kill_credit: None,
                    zone_multipliers: Default::default(),
                    contributor_ledger: Default::default(),
                },
            )
            .unwrap();
        id
    }

    // More corpses than one batch, so the rescan is inside the window too.
    #[test]
    fn moving_corpses_allocates_nothing_and_keeps_their_paths() {
        let world = floor_world();
        let collision = CombinedCollisionWorld::static_only(&world);
        let mut registry = EntityRegistry::new();
        let corpses: Vec<EntityId> = (0..CORPSE_BATCH + 3)
            .map(|index| {
                let x = index as f32 * 2.0 - 35.0;
                spawn_corpse(&mut registry, Vec3::new(x, 0.9 + SKIN_DISTANCE + 4.0, 0.0))
            })
            .collect();
        for _ in 0..3 {
            tick_corpses(&mut registry, &collision, ENV, DT);
        }
        let height = |registry: &EntityRegistry, id: EntityId| {
            registry.get_component::<Transform>(id).unwrap().position.y
        };
        let before: Vec<f32> = corpses.iter().map(|&id| height(&registry, id)).collect();

        let control = AllocSnapshot::arm();
        let boxed = std::hint::black_box(Box::new(7_u64));
        assert_eq!(control.allocs_since(), 1, "the control window reads one");
        drop(boxed);

        let window = AllocSnapshot::arm();
        tick_corpses(&mut registry, &collision, ENV, DT);
        let allocations = window.allocs_since();
        assert_eq!(allocations, 0, "corpse motion allocates nothing");

        for (&id, &was) in corpses.iter().zip(&before) {
            assert!(height(&registry, id) < was, "every corpse fell this tick");
            let agent = registry.get_component::<AgentComponent>(id).unwrap();
            assert_eq!(agent.path.len(), 2, "the path waits for a revival");
        }
    }
}
