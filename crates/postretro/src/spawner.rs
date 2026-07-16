//! Shared, VM-free state for fixed-tick `entity_spawner` execution.
//!
//! The context is session-owned because reaction registries retain closures
//! across level reloads. Its per-level interior is replaced atomically during
//! lifecycle install, leaving the later fixed-tick executor no reason to enter
//! a script context or data registry.

use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::rc::Rc;

use glam::Vec3;
use postretro_entities::components::spawner::SpawnerComponent;
use postretro_entities::provenance::DescriptorSpawnPath;
use postretro_entities::{ComponentKind, EntityId, EntityRegistry, Transform};
use postretro_foundation::NavAgentParams;
use postretro_scripting_core::data_descriptors::EntityTypeDescriptor;
use postretro_scripting_core::reaction_registry::ReactionPrimitiveRegistry;

use crate::scripting::builtins::data_archetype::attach_descriptor_components;
use crate::scripting::map_entity::MapEntity;

#[derive(Debug, Default)]
pub(crate) struct SpawnContextState {
    pub(crate) resolved_enemy_descriptors: HashMap<String, EntityTypeDescriptor>,
    pub(crate) agent_params: Option<NavAgentParams>,
    /// Task 2 uses this for one warning per missing spawner tag per level.
    pub(crate) warned_zero_match_tags: HashSet<String>,
    warned_capacity_exhaustion: bool,
}

/// Session-built shared handle supplied to both trigger and app-side command
/// routes. It is intentionally not an ECS component and therefore is not part
/// of the serde component vocabulary.
#[derive(Debug, Clone, Default)]
pub(crate) struct SpawnContext {
    state: Rc<RefCell<SpawnContextState>>,
}

impl SpawnContext {
    pub(crate) fn replace_level_data(
        &self,
        resolved_enemy_descriptors: HashMap<String, EntityTypeDescriptor>,
        agent_params: Option<NavAgentParams>,
    ) {
        *self.state.borrow_mut() = SpawnContextState {
            resolved_enemy_descriptors,
            agent_params,
            warned_zero_match_tags: HashSet::new(),
            warned_capacity_exhaustion: false,
        };
    }

    pub(crate) fn clear(&self) {
        *self.state.borrow_mut() = SpawnContextState::default();
    }

    pub(crate) fn state(&self) -> std::cell::Ref<'_, SpawnContextState> {
        self.state.borrow()
    }

    fn warn_zero_match_tag_once(&self, tag: &str) {
        if self
            .state
            .borrow_mut()
            .warned_zero_match_tags
            .insert(tag.to_string())
        {
            log::warn!("[Spawner] tag `{tag}` matched no entity_spawner entities; skipping");
        }
    }

    fn warn_capacity_exhaustion_once(&self) {
        let mut state = self.state.borrow_mut();
        if !state.warned_capacity_exhaustion {
            state.warned_capacity_exhaustion = true;
            log::warn!("[Spawner] entity registry exhausted while spawning; stopping batch");
        }
    }
}

/// Trigger tags resolve only against the Spawner column: an unrelated entity
/// sharing a tag never becomes a spawn target.
pub(crate) fn spawn_from_spawner_tag(
    registry: &mut EntityRegistry,
    tag: &str,
    context: &SpawnContext,
) {
    let targets: Vec<_> = registry
        .query_by_component_and_tag(ComponentKind::Spawner, Some(tag))
        .map(|(id, _)| id)
        .collect();
    if targets.is_empty() {
        context.warn_zero_match_tag_once(tag);
        return;
    }
    spawn_from_spawner_targets(registry, &targets, context);
}

/// The ordinary named-event drain already resolved Transform targets. Retain
/// only spawners, then converge on the same executor as the trigger route.
pub(crate) fn spawn_from_spawner_targets(
    registry: &mut EntityRegistry,
    targets: &[EntityId],
    context: &SpawnContext,
) {
    let spawners: Vec<_> = targets
        .iter()
        .copied()
        .filter_map(|id| {
            let spawner = registry.get_component::<SpawnerComponent>(id).ok()?.clone();
            let transform = *registry.get_component::<Transform>(id).ok()?;
            Some((id, spawner, transform))
        })
        .collect();
    spawn_resolved_spawners(registry, spawners, context);
}

fn spawn_resolved_spawners(
    registry: &mut EntityRegistry,
    spawners: Vec<(EntityId, SpawnerComponent, Transform)>,
    context: &SpawnContext,
) {
    for (spawner_id, spawner, spawner_transform) in spawners {
        if !spawner.resolved || spawner.count == 0 {
            continue;
        }
        let Some((descriptor, agent_params)) = ({
            let state = context.state();
            state
                .resolved_enemy_descriptors
                .get(&spawner.archetype_name)
                .cloned()
                .map(|descriptor| (descriptor, state.agent_params))
        }) else {
            log::warn!(
                "[Spawner] {spawner_id} resolved archetype `{}` is absent from this level cache; skipping",
                spawner.archetype_name
            );
            continue;
        };

        let radius = agent_params
            .unwrap_or(crate::scripting::builtins::data_archetype::DEFAULT_AGENT_PARAMS)
            .radius;
        let right = spawner_transform.rotation * Vec3::X;
        let synthetic_map_entity = MapEntity {
            classname: spawner.archetype_name.clone(),
            origin: spawner_transform.position,
            angles: Vec3::ZERO,
            key_values: [("enabled_on_spawn".to_string(), "true".to_string())]
                .into_iter()
                .collect(),
            tags: Vec::new(),
        };

        for index in 0..spawner.count {
            let transform = Transform {
                position: spawner_transform.position + right * (index as f32 * 2.0 * radius),
                rotation: spawner_transform.rotation,
                scale: spawner_transform.scale,
            };
            let Some(enemy) = registry.try_spawn(transform, &[]) else {
                context.warn_capacity_exhaustion_once();
                return;
            };
            attach_descriptor_components(
                registry,
                enemy,
                &descriptor,
                &synthetic_map_entity,
                false,
                DescriptorSpawnPath::RuntimeSpawn,
                agent_params,
            );
        }
    }
}

pub(crate) fn register_spawner_reaction_primitives(
    registry: &mut ReactionPrimitiveRegistry,
    context: SpawnContext,
) {
    registry.register("spawnFromSpawner", move |registry, targets, _args| {
        spawn_from_spawner_targets(registry, targets, &context);
        Ok(())
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    use glam::Quat;
    use postretro_entities::components::agent::AgentComponent;
    use postretro_entities::components::brain::BrainComponent;
    use postretro_entities::provenance::DescriptorProvenance;

    use crate::scripting::builtins::data_archetype::data_archetype_test_fixtures::ai_enemy_descriptor;

    const TAG: &str = "closet";

    fn context() -> SpawnContext {
        let context = SpawnContext::default();
        context.replace_level_data(
            [("cultist".to_string(), ai_enemy_descriptor("cultist"))]
                .into_iter()
                .collect(),
            Some(NavAgentParams {
                radius: 0.4,
                height: 1.8,
                step_height: 0.4,
                max_slope_deg: 45.0,
            }),
        );
        context
    }

    fn add_spawner(
        registry: &mut EntityRegistry,
        tag: &str,
        count: u32,
        resolved: bool,
        transform: Transform,
    ) -> EntityId {
        let id = registry.try_spawn(transform, &[tag.to_string()]).unwrap();
        registry
            .set_component(
                id,
                SpawnerComponent {
                    archetype_name: "cultist".to_string(),
                    count,
                    resolved,
                },
            )
            .unwrap();
        id
    }

    fn spawned(registry: &EntityRegistry) -> Vec<EntityId> {
        registry
            .iter_with_kind(ComponentKind::Brain)
            .map(|(id, _)| id)
            .collect()
    }

    #[test]
    fn repeated_fire_is_stateless_and_spawns_untagged_runtime_enemies() {
        let mut registry = EntityRegistry::new();
        add_spawner(&mut registry, TAG, 2, true, Transform::default());
        let context = context();

        spawn_from_spawner_tag(&mut registry, TAG, &context);
        spawn_from_spawner_tag(&mut registry, TAG, &context);

        let enemies = spawned(&registry);
        assert_eq!(enemies.len(), 4);
        for enemy in enemies {
            assert!(registry.get_tags(enemy).unwrap().is_empty());
            assert_eq!(
                registry
                    .get_component::<DescriptorProvenance>(enemy)
                    .unwrap()
                    .spawn_path,
                DescriptorSpawnPath::RuntimeSpawn
            );
            assert!(registry.get_component::<AgentComponent>(enemy).is_ok());
        }
    }

    #[test]
    fn zero_match_is_deduped_and_non_spawner_targets_do_not_qualify() {
        let mut registry = EntityRegistry::new();
        registry
            .try_spawn(Transform::default(), &[TAG.to_string()])
            .unwrap();
        let context = context();

        spawn_from_spawner_tag(&mut registry, TAG, &context);
        spawn_from_spawner_tag(&mut registry, TAG, &context);

        assert!(spawned(&registry).is_empty());
        assert_eq!(
            context.state().warned_zero_match_tags,
            [TAG.to_string()].into()
        );
    }

    #[test]
    fn authored_rotation_sets_facing_and_right_axis_spawn_offset() {
        let mut registry = EntityRegistry::new();
        let transform = Transform {
            position: Vec3::new(10.0, 2.0, -3.0),
            rotation: Quat::from_rotation_y(std::f32::consts::FRAC_PI_2),
            scale: Vec3::ONE,
        };
        add_spawner(&mut registry, TAG, 2, true, transform);
        let context = context();

        spawn_from_spawner_tag(&mut registry, TAG, &context);

        let enemies = spawned(&registry);
        assert_eq!(enemies.len(), 2);
        let first = registry.get_component::<Transform>(enemies[0]).unwrap();
        let second = registry.get_component::<Transform>(enemies[1]).unwrap();
        assert_eq!(first.rotation, transform.rotation);
        assert_eq!(second.rotation, transform.rotation);
        assert!((first.position - transform.position).length() < 1e-5);
        assert!(
            (second.position - (transform.position + transform.rotation * Vec3::X * 0.8)).length()
                < 1e-5
        );
    }

    #[test]
    fn unresolved_and_zero_count_spawners_are_inert() {
        let mut registry = EntityRegistry::new();
        add_spawner(&mut registry, "unresolved", 2, false, Transform::default());
        add_spawner(&mut registry, "zero", 0, true, Transform::default());
        let context = context();

        spawn_from_spawner_tag(&mut registry, "unresolved", &context);
        spawn_from_spawner_tag(&mut registry, "zero", &context);

        assert!(spawned(&registry).is_empty());
    }

    #[test]
    fn capacity_exhaustion_spawns_what_fits_without_panicking() {
        let mut registry = EntityRegistry::new();
        add_spawner(&mut registry, TAG, 3, true, Transform::default());
        registry.set_test_capacity_limit(2);
        let context = context();

        spawn_from_spawner_tag(&mut registry, TAG, &context);

        assert_eq!(spawned(&registry).len(), 1);
        assert!(context.state().warned_capacity_exhaustion);
    }

    #[test]
    fn app_drain_registration_uses_the_same_spawner_executor() {
        let mut registry = EntityRegistry::new();
        let spawner = add_spawner(&mut registry, TAG, 1, true, Transform::default());
        let context = context();
        let mut reactions = ReactionPrimitiveRegistry::new();
        register_spawner_reaction_primitives(&mut reactions, context);

        assert!(
            reactions
                .dispatch(
                    "spawnFromSpawner",
                    &mut registry,
                    &[spawner],
                    &serde_json::json!({})
                )
                .unwrap()
        );
        assert_eq!(spawned(&registry).len(), 1);
    }
}
