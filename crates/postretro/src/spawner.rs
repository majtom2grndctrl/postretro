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
use postretro_entities::components::brain::BrainComponent;
use postretro_entities::components::spawner::SpawnerComponent;
use postretro_entities::provenance::DescriptorSpawnPath;
use postretro_entities::{ComponentKind, EntityId, EntityRegistry, Transform};
use postretro_foundation::NavAgentParams;
use postretro_scripting_core::data_descriptors::EntityTypeDescriptor;
use postretro_scripting_core::reaction_registry::ReactionPrimitiveRegistry;

use crate::netcode::MAX_DELAY_MICROS;
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
            // `attach_brain` initializes this timer to zero. Seed only after the
            // descriptor has attached its components so a newly spawned enemy
            // cannot attack before remote interpolation's maximum delay has
            // elapsed and the remote presentation has had time to arrive.
            if let Ok(mut brain) = registry.get_component::<BrainComponent>(enemy).cloned() {
                brain.attack_cooldown_remaining_ms = brain
                    .attack_cooldown_remaining_ms
                    .max(MAX_DELAY_MICROS as f32 / 1000.0);
                let _ = registry.set_component(enemy, brain);
            }
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
    use postretro_entities::components::brain::BrainComponent;
    use postretro_entities::components::health::HealthComponent;
    use postretro_entities::components::player_movement::PlayerMovementComponent;
    use postretro_entities::provenance::DescriptorProvenance;
    use postretro_scripting_core::data_descriptors::{
        AirParams, CapsuleParams, FallParams, GroundParams, NamedReaction,
        PlayerMovementDescriptor, ProgressDescriptor, ReactionDescriptor, SpeedParams,
    };
    use postretro_scripting_core::data_registry::DataRegistry;
    use postretro_scripting_core::reaction_dispatch::ProgressTracker;

    use crate::scripting::builtins::data_archetype::data_archetype_test_fixtures::ai_enemy_descriptor;
    use crate::scripting::systems::ai::{ENEMY_ATTACK_EVENT, run_ai_tick};

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

    fn player_movement() -> PlayerMovementComponent {
        PlayerMovementComponent::from_descriptor(&PlayerMovementDescriptor {
            capsule: CapsuleParams {
                radius: 0.4,
                half_height: 0.8,
                eye_height: 0.5,
            },
            ground: GroundParams {
                speed: SpeedParams {
                    walk: 7.0,
                    run: 11.0,
                    crouch: 3.0,
                },
                accel: 10.0,
                step_height: 0.3,
                max_slope: 45.0,
            },
            air: AirParams {
                forward_steer: 0.0,
                accel: 0.7,
                max_control_speed: 0.5,
                bunny_hop: false,
                jumps: 0,
                jump_velocity: 5.5,
                jump_ceiling: 0.0,
            },
            fall: FallParams {
                terminal_velocity: 40.0,
            },
            stuck_stop_enabled: PlayerMovementDescriptor::DEFAULT_STUCK_STOP_ENABLED,
            stuck_stop_threshold: PlayerMovementDescriptor::DEFAULT_STUCK_STOP_THRESHOLD,
            dash: None,
            forgiveness: None,
            crouch: None,
            view_feel: None,
        })
    }

    fn spawn_attackable_player(registry: &mut EntityRegistry) -> EntityId {
        let player = registry.spawn(Transform {
            position: Vec3::X,
            ..Transform::default()
        });
        registry.set_component(player, player_movement()).unwrap();
        registry
            .set_component(
                player,
                HealthComponent {
                    max: 100.0,
                    current: 100.0,
                    hitbox: None,
                    death_handled: false,
                    zone_multipliers: Default::default(),
                    contributor_ledger: Default::default(),
                },
            )
            .unwrap();
        player
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
    fn spawned_enemy_cannot_attack_before_interpolation_windup_expires() {
        let mut registry = EntityRegistry::new();
        add_spawner(&mut registry, TAG, 1, true, Transform::default());
        let context = context();
        let player = spawn_attackable_player(&mut registry);

        spawn_from_spawner_tag(&mut registry, TAG, &context);
        let enemy = spawned(&registry).pop().expect("one spawned enemy");
        let seed = MAX_DELAY_MICROS as f32 / 1000.0;
        assert!(
            registry
                .get_component::<BrainComponent>(enemy)
                .unwrap()
                .attack_cooldown_remaining_ms
                >= seed,
            "the descriptor attachment must not overwrite the interpolation windup"
        );

        let mut warned = HashSet::new();
        let dt_secs = 0.05;
        for _ in 0..4 {
            assert!(
                run_ai_tick(&mut registry, &mut warned, dt_secs).is_empty(),
                "no attack may land before the {} ms windup floor",
                seed
            );
        }
        assert_eq!(
            registry
                .get_component::<HealthComponent>(player)
                .unwrap()
                .current,
            100.0,
            "the player remains unharmed before the windup expires"
        );

        assert_eq!(
            run_ai_tick(&mut registry, &mut warned, dt_secs),
            vec![ENEMY_ATTACK_EVENT],
            "the enemy attacks once exactly when the seeded windup reaches zero"
        );
        assert!(
            registry
                .get_component::<HealthComponent>(player)
                .unwrap()
                .current
                < 100.0,
            "the final tick proves the test exercised attack behavior rather than merely no target"
        );
    }

    #[test]
    fn runtime_spawn_kill_cannot_advance_install_scoped_tag_progress() {
        let mut registry = EntityRegistry::new();
        registry
            .try_spawn(Transform::default(), &["wave".to_string()])
            .unwrap();
        add_spawner(&mut registry, TAG, 1, true, Transform::default());
        let context = context();

        let mut data = DataRegistry::new();
        data.reactions.push(NamedReaction {
            name: "waveProgress".to_string(),
            descriptor: ReactionDescriptor::Progress(ProgressDescriptor {
                tag: "wave".to_string(),
                at: 1.0,
                fire: "release".to_string(),
            }),
        });
        let mut progress = ProgressTracker::new();
        progress.initialize(&data, &registry);

        spawn_from_spawner_tag(&mut registry, TAG, &context);
        let enemy = spawned(&registry).pop().expect("one runtime-spawned enemy");
        assert_eq!(
            registry
                .get_component::<DescriptorProvenance>(enemy)
                .unwrap()
                .spawn_path,
            DescriptorSpawnPath::RuntimeSpawn
        );
        assert!(registry.get_tags(enemy).unwrap().is_empty());
        assert!(
            progress
                .on_entity_killed(registry.get_tags(enemy).unwrap())
                .is_empty(),
            "an untagged runtime-spawn kill cannot decrement a tag-keyed progress total"
        );
        assert_eq!(
            progress.on_entity_killed(&["wave".to_string()]),
            vec!["release".to_string()],
            "the install-scoped total still requires the original tagged entity kill"
        );
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
