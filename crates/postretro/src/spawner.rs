//! Shared, VM-free state for fixed-tick `entity_spawner` execution.
//!
//! The context is session-owned because reaction registries retain closures
//! across level reloads. Its per-level interior is replaced atomically during
//! lifecycle install, leaving the later fixed-tick executor no reason to enter
//! a script context or data registry.
//!
//! See: context/lib/scripting.md §12 (reaction dispatch — `spawnFromSpawner`);
//!      context/lib/entity_model.md (entity materialization)

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
use crate::scripting::builtins::data_archetype::{
    ai_capsule_center_from_feet_offset, attach_descriptor_components,
};
use crate::scripting::map_entity::MapEntity;

#[derive(Debug)]
pub(crate) struct SpawnContextState {
    pub(crate) resolved_enemy_descriptors: HashMap<String, EntityTypeDescriptor>,
    pub(crate) agent_params: Option<NavAgentParams>,
    /// One zero-match warn per distinct tag per level; cleared on level refill.
    pub(crate) warned_zero_match_tags: HashSet<String>,
    /// Runtime-spawned mesh entities awaiting their already-built install-time
    /// clip table. Drained once by the host after attachment.
    pending_mesh_clip_resolves: Vec<EntityId>,
    warned_capacity_exhaustion: bool,
    /// Connected clients keep their local spawner context for map preload, but
    /// must materialize runtime enemies only from host snapshots.
    can_materialize_runtime_spawns: bool,
}

impl Default for SpawnContextState {
    fn default() -> Self {
        Self {
            resolved_enemy_descriptors: HashMap::new(),
            agent_params: None,
            warned_zero_match_tags: HashSet::new(),
            pending_mesh_clip_resolves: Vec::new(),
            warned_capacity_exhaustion: false,
            // Outside a connected-client session, spawning is authoritative.
            can_materialize_runtime_spawns: true,
        }
    }
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
        let can_materialize_runtime_spawns = self.state.borrow().can_materialize_runtime_spawns;
        *self.state.borrow_mut() = SpawnContextState {
            resolved_enemy_descriptors,
            agent_params,
            warned_zero_match_tags: HashSet::new(),
            pending_mesh_clip_resolves: Vec::new(),
            warned_capacity_exhaustion: false,
            can_materialize_runtime_spawns,
        };
    }

    pub(crate) fn clear(&self) {
        let can_materialize_runtime_spawns = self.state.borrow().can_materialize_runtime_spawns;
        *self.state.borrow_mut() = SpawnContextState {
            can_materialize_runtime_spawns,
            ..Default::default()
        };
    }

    pub(crate) fn set_runtime_spawn_authority(&self, enabled: bool) {
        self.state.borrow_mut().can_materialize_runtime_spawns = enabled;
    }

    pub(crate) fn state(&self) -> std::cell::Ref<'_, SpawnContextState> {
        self.state.borrow()
    }

    pub(crate) fn take_pending_mesh_clip_resolves(&self) -> Vec<EntityId> {
        std::mem::take(&mut self.state.borrow_mut().pending_mesh_clip_resolves)
    }

    fn queue_mesh_clip_resolve(&self, id: EntityId) {
        self.state.borrow_mut().pending_mesh_clip_resolves.push(id);
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

    fn can_materialize_runtime_spawns(&self) -> bool {
        self.state.borrow().can_materialize_runtime_spawns
    }
}

/// Trigger tags resolve only against the Spawner column: an unrelated entity
/// sharing a tag never becomes a spawn target.
pub(crate) fn spawn_from_spawner_tag(
    registry: &mut EntityRegistry,
    tag: &str,
    context: &SpawnContext,
) {
    if tag.is_empty() {
        log::warn!("[Spawner] spawnFromSpawner requires a non-empty fire-time tag; skipping");
        return;
    }
    if !context.can_materialize_runtime_spawns() {
        return;
    }
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
    if !context.can_materialize_runtime_spawns() {
        return;
    }
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
            log::debug!(
                "[Spawner] {spawner_id} resolved archetype `{}` is absent from this level cache; skipping",
                spawner.archetype_name
            );
            continue;
        };

        let radius = agent_params
            .unwrap_or(crate::scripting::builtins::data_archetype::DEFAULT_AGENT_PARAMS)
            .radius;
        // The spawner origin authors the enemy's feet, matching the map-placement
        // path (`spawn_descriptor_instance`). Raise the base to the AI capsule
        // center so the shared `attach_descriptor_components` mesh offset and
        // hitbox rebase — both of which assume a center-origin Transform — land a
        // spawner-spawned enemy identically to a map-placed one of the same
        // archetype (otherwise it sinks ~half a capsule into the floor).
        let origin_shift = ai_capsule_center_from_feet_offset(&descriptor, agent_params);
        let right = spawner_transform.rotation * Vec3::X;
        for index in 0..spawner.count {
            // The spawner origin authors the enemy's feet; copies fan out along
            // the `right` axis. This feet position is the synthetic MapEntity
            // `origin` — the un-raised value — so descriptor components that stamp
            // straight from `MapEntity.origin` (e.g. a `light` block) land at the
            // feet, exactly as a map-placed instance does (`spawn_descriptor_instance`
            // passes the raw feet origin while raising only the Transform). The
            // Transform below is the raised capsule center.
            let feet_position = spawner_transform.position + right * (index as f32 * 2.0 * radius);
            let transform = Transform {
                position: feet_position + origin_shift,
                rotation: spawner_transform.rotation,
                scale: spawner_transform.scale,
            };
            let Some(enemy) = registry.try_spawn(transform, &[]) else {
                context.warn_capacity_exhaustion_once();
                return;
            };
            let synthetic_map_entity = MapEntity {
                classname: spawner.archetype_name.clone(),
                origin: feet_position,
                angles: Vec3::ZERO,
                key_values: [("enabled_on_spawn".to_string(), "true".to_string())]
                    .into_iter()
                    .collect(),
                tags: Vec::new(),
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
            if matches!(
                registry.has_component_kind(enemy, ComponentKind::Mesh),
                Ok(true)
            ) {
                // The model was uploaded and its clip table was built during
                // level install from the resolved spawner archetype. Queue only
                // this new entity for a host-side index fill; never upload in
                // the fixed-tick path.
                context.queue_mesh_clip_resolve(enemy);
            }
            // Behavior-graph attachment initializes cooldowns empty. Seed every
            // declared attack only after descriptor attachment so a newly spawned
            // enemy cannot attack before remote interpolation's maximum delay
            // has elapsed and the remote presentation has had time to arrive.
            if let Ok(mut brain) = registry.get_component::<BrainComponent>(enemy).cloned() {
                let windup_ms = MAX_DELAY_MICROS as f32 / 1000.0;
                for attack_name in brain.graph.attacks.keys() {
                    brain
                        .attack_cooldown_remaining_ms
                        .entry(attack_name.clone())
                        .and_modify(|remaining_ms| *remaining_ms = remaining_ms.max(windup_ms))
                        .or_insert(windup_ms);
                }
                let _ = registry.set_component(enemy, brain);
            }
        }
    }
}

pub(crate) fn register_spawner_reaction_primitives(
    registry: &mut ReactionPrimitiveRegistry,
    context: SpawnContext,
) {
    registry.register_tagged("spawnFromSpawner", move |registry, tag, targets, _args| {
        if tag.is_empty() {
            log::warn!("[Spawner] spawnFromSpawner requires a non-empty fire-time tag; skipping");
            return Ok(());
        }
        if targets.iter().any(|id| {
            registry
                .has_component_kind(*id, ComponentKind::Spawner)
                .unwrap_or(false)
        }) {
            spawn_from_spawner_targets(registry, targets, &context);
        } else {
            // The generic app-side dispatcher resolves Transform targets. Use the
            // authored tag to retain zero-match diagnostics when those targets
            // are absent or all belong to non-spawner entities.
            spawn_from_spawner_tag(registry, tag, &context);
        }
        Ok(())
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    use glam::Quat;
    use postretro_entities::components::agent::AgentComponent;
    use postretro_entities::components::brain::BrainComponent;
    use postretro_entities::components::health::HealthComponent;
    use postretro_entities::components::light::LightComponent;
    use postretro_entities::components::player_movement::PlayerMovementComponent;
    use postretro_entities::provenance::DescriptorProvenance;
    use postretro_scripting_core::data_descriptors::{
        ActionVerb, AirParams, AttackParams, BehaviorActivityDescriptor, CapsuleParams,
        EntityTypeDescriptor, FallParams, GroundParams, LightDescriptor, MotionVerb, NamedReaction,
        PlayerMovementDescriptor, ProgressDescriptor, ReactionDescriptor, SpeedParams,
    };
    use postretro_scripting_core::data_registry::DataRegistry;
    use postretro_scripting_core::reaction_dispatch::ProgressTracker;

    use crate::scripting::builtins::data_archetype_test_fixtures::behavior_enemy_descriptor;
    use crate::scripting_systems::ai::run_ai_tick;

    const TAG: &str = "closet";

    fn context() -> SpawnContext {
        context_with_descriptor(behavior_enemy_descriptor("cultist"))
    }

    fn test_agent_params() -> NavAgentParams {
        NavAgentParams {
            radius: 0.4,
            height: 1.8,
            step_height: 0.4,
            max_slope_deg: 45.0,
        }
    }

    /// The feet→center raise the materialization path applies to the `cultist`
    /// AI archetype under the shared test agent params. Computed via the same
    /// offset helper the production path uses so these assertions track any
    /// params change rather than hard-coding the raise.
    fn cultist_feet_to_center_shift() -> Vec3 {
        ai_capsule_center_from_feet_offset(
            &behavior_enemy_descriptor("cultist"),
            Some(test_agent_params()),
        )
    }

    fn context_with_descriptor(descriptor: EntityTypeDescriptor) -> SpawnContext {
        let context = SpawnContext::default();
        context.replace_level_data(
            [("cultist".to_string(), descriptor)].into_iter().collect(),
            Some(test_agent_params()),
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
            slide: None,
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
                    pending_kill_credit: None,
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
    fn spawned_enemy_latches_first_eligible_fire_after_interpolation_windup() {
        let mut registry = EntityRegistry::new();
        add_spawner(
            &mut registry,
            TAG,
            1,
            true,
            Transform {
                rotation: Quat::from_rotation_y(std::f32::consts::FRAC_PI_2),
                ..Transform::default()
            },
        );
        let mut descriptor = behavior_enemy_descriptor("cultist");
        let behavior = descriptor
            .behavior
            .as_mut()
            .expect("behavior enemy fixture declares a graph");
        behavior.attacks.insert(
            "slam".to_string(),
            AttackParams {
                weapon: None,
                damage: Some(12.0),
                max_range: Some(2.0),
                cooldown_ms: Some(0.0),
                engagement_radius: None,
                standoff_distance: None,
            },
        );
        behavior
            .envelope
            .transitions
            .get_mut("idle")
            .expect("fixture declares its initial activity")[0]
            .to = "slam".to_string();
        behavior.envelope.activities.insert(
            "slam".to_string(),
            BehaviorActivityDescriptor {
                animation: Some("attack".to_string()),
                motion: Some(MotionVerb::ChaseTarget),
                action: Some(ActionVerb::Attack("slam".to_string())),
                on_enter: None,
                layers: Default::default(),
            },
        );
        let declared_attack_names: Vec<_> = behavior.attacks.keys().cloned().collect();
        assert_eq!(
            declared_attack_names.len(),
            2,
            "the fixture exercises a two-attack graph"
        );
        let context = context_with_descriptor(descriptor);
        let player = spawn_attackable_player(&mut registry);

        spawn_from_spawner_tag(&mut registry, TAG, &context);
        let enemy = spawned(&registry).pop().expect("one spawned enemy");
        let spawned_forward = registry
            .get_component::<Transform>(enemy)
            .expect("spawned enemy retains its transform")
            .rotation
            * Vec3::Z;
        assert!(
            (spawned_forward - Vec3::X).length() < 1e-5,
            "the fixture starts the spawned enemy facing its live target"
        );
        let seed = MAX_DELAY_MICROS as f32 / 1000.0;
        let cooldowns = &registry
            .get_component::<BrainComponent>(enemy)
            .expect("spawned enemy retains its brain")
            .attack_cooldown_remaining_ms;
        assert_eq!(
            cooldowns.len(),
            declared_attack_names.len(),
            "the spawn windup creates one cooldown entry for every declared attack"
        );
        for attack_name in &declared_attack_names {
            assert!(
                cooldowns.get(attack_name).copied().unwrap_or_default() >= seed,
                "the descriptor attachment must seed `{attack_name}` with the interpolation windup"
            );
        }

        let mut warned = crate::scripting_systems::ai::AiRuntime::new();
        let dt_secs = 0.05;
        let windup_ticks = (seed / (dt_secs * 1000.0)).ceil() as usize;
        for tick in 1..windup_ticks {
            assert!(
                run_ai_tick(&mut registry, &mut warned, dt_secs).is_empty(),
                "no attack event may land while the {} ms interpolation windup gate is closed",
                seed
            );
            if tick == 1 {
                assert_eq!(
                    registry
                        .get_component::<BrainComponent>(enemy)
                        .expect("spawned enemy retains its brain")
                        .state_name(),
                    Some("slam"),
                    "the spawned enemy routes into the second, non-initial firing state"
                );
            }
        }
        assert_eq!(
            registry
                .get_component::<HealthComponent>(player)
                .unwrap()
                .current,
            100.0,
            "the player remains unharmed before the windup expires"
        );

        // The only gate that changes in this bounded scenario is the spawned
        // cooldown. The player stays alive, no collision world means clear LOS,
        // and the spawner authored the enemy already facing the target. The
        // active `slam` leaf must therefore fire on its first open dwell tick.
        let events = run_ai_tick(&mut registry, &mut warned, dt_secs);
        assert_eq!(
            events
                .iter()
                .map(|event| event.as_ref())
                .collect::<Vec<_>>(),
            vec![crate::scripting_systems::ai::ENEMY_ATTACK_EVENT],
            "the first post-windup eligible tick fires exactly once"
        );
        assert_eq!(
            registry
                .get_component::<HealthComponent>(player)
                .unwrap()
                .current,
            88.0,
            "the first open firing dwell tick applies the declared damage"
        );

        // `slam` has no authored cooldown after firing. Empty later ticks
        // therefore demonstrate the active leaf's one-fire latch, not a
        // separate cooldown gate.
        for tick in 1..=2 {
            assert!(
                run_ai_tick(&mut registry, &mut warned, dt_secs).is_empty(),
                "same firing-leaf dwell does not emit a second event on later tick {tick}"
            );
            assert_eq!(
                registry
                    .get_component::<HealthComponent>(player)
                    .unwrap()
                    .current,
                88.0,
                "same firing-leaf dwell does not apply a second hit on later tick {tick}"
            );
        }
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
        // The base sits at the AI capsule center (feet→center raise); the
        // horizontal `right`-axis fan-out spacing is unchanged.
        let shift = cultist_feet_to_center_shift();
        assert!((first.position - (transform.position + shift)).length() < 1e-5);
        assert!(
            (second.position - (transform.position + shift + transform.rotation * Vec3::X * 0.8))
                .length()
                < 1e-5
        );
    }

    #[test]
    fn spawn_raises_feet_origin_to_ai_capsule_center() {
        // A spawner authors its origin at the enemy's feet; materialization must
        // raise the Transform to the capsule center so the shared
        // `attach_descriptor_components` mesh offset and hitbox rebase (both of
        // which assume a center-origin Transform) land a spawner-spawned enemy
        // identically to a map-placed one. Without the raise the enemy sinks
        // ~half a capsule into the floor. Asserting against the same offset
        // helper the production path uses keeps this honest if params change.
        let mut registry = EntityRegistry::new();
        let origin = Vec3::new(5.0, 1.0, -2.0);
        add_spawner(
            &mut registry,
            TAG,
            1,
            true,
            Transform {
                position: origin,
                ..Transform::default()
            },
        );
        let context = context();

        spawn_from_spawner_tag(&mut registry, TAG, &context);

        let enemy = spawned(&registry).pop().expect("one spawned enemy");
        let position = registry.get_component::<Transform>(enemy).unwrap().position;
        let shift = cultist_feet_to_center_shift();
        assert!(
            shift.length() > 1e-5,
            "an `ai` archetype must raise off its feet — a zero shift would make this test vacuous"
        );
        assert!(
            (position - (origin + shift)).length() < 1e-5,
            "the spawned Transform sits at the capsule center, not the raw feet origin"
        );
        assert!(
            (position - origin).length() > 1e-5,
            "the raw spawner origin would sink the enemy ~half a capsule into the floor"
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
                .dispatch_tagged(
                    "spawnFromSpawner",
                    &mut registry,
                    TAG,
                    &[spawner],
                    &serde_json::json!({})
                )
                .unwrap()
        );
        assert_eq!(spawned(&registry).len(), 1);
    }

    #[test]
    fn app_drain_zero_or_non_spawner_matches_warn_once_by_authored_tag() {
        let mut registry = EntityRegistry::new();
        let non_spawner = registry
            .try_spawn(Transform::default(), &[TAG.to_string()])
            .unwrap();
        let context = context();
        let mut reactions = ReactionPrimitiveRegistry::new();
        register_spawner_reaction_primitives(&mut reactions, context.clone());
        let no_targets = [];
        let non_spawner_targets = [non_spawner];

        for targets in [&no_targets[..], &non_spawner_targets[..]] {
            assert!(
                reactions
                    .dispatch_tagged(
                        "spawnFromSpawner",
                        &mut registry,
                        TAG,
                        targets,
                        &serde_json::json!({}),
                    )
                    .unwrap()
            );
        }

        assert!(spawned(&registry).is_empty());
        assert_eq!(
            context.state().warned_zero_match_tags,
            [TAG.to_string()].into()
        );
    }

    #[test]
    fn client_authority_gate_keeps_spawners_without_materializing_enemies() {
        let mut registry = EntityRegistry::new();
        add_spawner(&mut registry, TAG, 1, true, Transform::default());
        let context = context();
        context.set_runtime_spawn_authority(false);

        spawn_from_spawner_tag(&mut registry, TAG, &context);

        assert!(spawned(&registry).is_empty());
    }

    #[test]
    fn descriptor_lights_follow_each_offset_runtime_spawn() {
        let mut descriptor = behavior_enemy_descriptor("cultist");
        descriptor.light = Some(LightDescriptor {
            color: [1.0, 0.5, 0.25],
            intensity: 3.0,
            range: 12.0,
            is_dynamic: true,
        });
        let context = context_with_descriptor(descriptor);
        let mut registry = EntityRegistry::new();
        let transform = Transform {
            position: Vec3::new(4.0, 2.0, -1.0),
            rotation: Quat::from_rotation_y(std::f32::consts::FRAC_PI_2),
            scale: Vec3::ONE,
        };
        add_spawner(&mut registry, TAG, 2, true, transform);

        spawn_from_spawner_tag(&mut registry, TAG, &context);

        let lights: Vec<_> = registry
            .iter_with_kind(ComponentKind::Light)
            .filter_map(|(id, _)| registry.get_component::<LightComponent>(id).ok())
            .collect();
        assert_eq!(lights.len(), 2);
        // A descriptor light stamps from the synthetic `MapEntity.origin`, which
        // mirrors map placement: the FEET position (no capsule-center raise), so
        // a spawner-spawned light lands identically to a map-placed one of the
        // same archetype. The capsule shift raises only the Transform, not the
        // light. The horizontal `right`-axis fan-out spacing is preserved.
        let right = transform.rotation * Vec3::X;
        assert!((Vec3::from_array(lights[0].origin) - transform.position).length() < 1e-5);
        assert!(
            (Vec3::from_array(lights[1].origin) - (transform.position + right * 0.8)).length()
                < 1e-5
        );
    }
}
