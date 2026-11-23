//! Descriptor-aware remote snapshot materialization, after descriptor-blind apply.
//! See: context/lib/networking.md

use postretro_entities::components::brain::TravelVariant;
use postretro_entities::{EntityRegistry, EntityTypeDescriptor};
use postretro_foundation::NavAgentParams;

use super::client::ClientReplication;
use super::scripting_systems::hit_zones::HitZoneStore;
use super::{client, descriptor_class, remote_materialize, weapon_mounts};

/// The descriptor-aware half of one applied snapshot, in the order the mounts
/// depend on: the armed local pawn's body (mesh and inventory), then its weapon
/// mounts, which resolve its class from the arming record, then every remote
/// entity. Returns whether any presentation mesh materialized. Prediction
/// arming stays with the caller.
#[allow(clippy::too_many_arguments)]
pub(super) fn materialize_snapshot_presentation(
    outcome: &client::ApplyOutcome,
    descriptors: &[EntityTypeDescriptor],
    registry: &mut EntityRegistry,
    replication: &mut ClientReplication,
    server_tick: u32,
    agent_params: Option<NavAgentParams>,
    hit_zone_store: &HitZoneStore,
    host_movement_tuning: Option<&postretro_foundation::PlayerMovementDescriptor>,
    host_tuning: Option<&postretro_combat_model::TuningPayload>,
    rebuild_movement_tuning: bool,
    life_state_tables: &crate::life_state_tables::LifeStateTables,
) -> bool {
    let mut changed = false;
    // The materialization call-site glue lives in `remote_materialize` (the
    // focused seam also materializes remote enemies).
    if let Some(armed) = &outcome.armed_local_pawn {
        changed |= remote_materialize::materialize_armed_local_pawn(
            armed,
            descriptors,
            registry,
            host_movement_tuning,
            host_tuning,
            rebuild_movement_tuning,
            life_state_tables,
        );
    }
    // The local pawn's world body is shadow-only, but it still needs the same
    // third-person weapon silhouette as peers see. Its viewmodel remains separate
    // from this presentation-only attachment path.
    changed |= weapon_mounts::apply_local_weapon_attachments(
        &outcome.local_weapon_attachments,
        replication,
        registry,
        descriptors,
        hit_zone_store,
    );
    // Each non-local descriptor entity needing materialization is handled here, where
    // the shared descriptor table is in scope (the net-facing apply is descriptor-blind).
    // The host-stamped class distinguishes pawn paths from descriptor-world copies,
    // even when both descriptors author Movement. A display copy receives Mesh, Light,
    // Emitter, and query-only HitVolume, but never authority components. The local
    // pawn's weapon attachment above and retired-projectile flashes (the caller's)
    // use separate paths. Materialization is idempotent and unknown-class-tolerant,
    // so failed presentation still interpolates transform.
    changed |= materialize_remote_entities(
        &outcome.remote_entities,
        descriptors,
        registry,
        replication,
        server_tick,
        agent_params,
        hit_zone_store,
    );
    changed
}

pub(super) fn materialize_remote_entities(
    remotes: &[client::RemoteEntityMaterialize],
    descriptors: &[EntityTypeDescriptor],
    registry: &mut EntityRegistry,
    replication: &mut ClientReplication,
    server_tick: u32,
    agent_params: Option<NavAgentParams>,
    hit_zone_store: &HitZoneStore,
) -> bool {
    let mut changed = false;
    for remote in remotes {
        let representation =
            descriptor_class::decode_replicated_descriptor_class(&remote.entity_class);
        let entity_class = representation.canonical_name();
        replication.cache_remote_entity_class(remote.network_id, &remote.entity_class);
        let descriptor = descriptors
            .iter()
            .find(|descriptor| descriptor.canonical_name.as_deref() == Some(entity_class));
        let descriptor_entity = matches!(
            representation,
            descriptor_class::ReplicatedDescriptorClass::Descriptor(_)
        );
        let materialized = if matches!(
            representation,
            descriptor_class::ReplicatedDescriptorClass::Projectile(_)
        ) {
            remote_materialize::materialize_armed_remote_projectile(
                remote,
                descriptors,
                registry,
                server_tick,
                replication.projectile_presentation(remote.network_id),
            )
        } else if matches!(
            representation,
            descriptor_class::ReplicatedDescriptorClass::Pawn(_)
        ) {
            let player_locomotion = descriptor.and_then(|descriptor| {
                let movement = descriptor.movement.as_ref()?;
                let mesh = descriptor.mesh.as_ref()?;
                let idle_state = mesh.default_state.clone()?;
                let states = crate::sim::PlayerLocomotionStates::from_declared(|state| {
                    mesh.animations.contains_key(state)
                })?;
                let model =
                    hit_zone_store.get(&postretro_model::ModelHandle::from(mesh.model.clone()));
                let derived_travel_speeds = states
                    .state_names()
                    .filter_map(|state_name| {
                        let state = mesh.animations.get(state_name)?;
                        let speed = model?.state_clip(None, &state.clip)?.travel_speed?;
                        Some((state_name.to_string(), speed))
                    })
                    .collect();
                // The host's clip lookup with no resolved index (a descriptor has
                // none), so both peers end a jump one-shot after the same length.
                let clip_durations = states
                    .one_shot_names()
                    .filter_map(|state_name| {
                        let state = mesh.animations.get(state_name)?;
                        let duration = model?.state_clip(None, &state.clip)?.duration;
                        Some((state_name.to_string(), duration))
                    })
                    .collect();
                Some(client::RemotePlayerLocomotionReference {
                    idle_state,
                    states,
                    walk_speed: movement.ground.speed.walk,
                    run_speed: movement.ground.speed.run,
                    crouch_speed: movement.ground.speed.crouch,
                    derived_travel_speeds,
                    clip_durations,
                })
            });
            replication.cache_remote_player_locomotion(remote.network_id, player_locomotion);
            replication.cache_remote_player_capsule_radius(
                remote.network_id,
                descriptor
                    .and_then(|descriptor| descriptor.movement.as_ref())
                    .map(|movement| movement.capsule.radius),
            );
            remote_materialize::materialize_armed_remote_player(
                remote,
                descriptors,
                registry,
                agent_params,
            )
        } else {
            // Remote AI has no local AgentComponent, so derive its stride rate
            // from exactly the motion this client presents.
            let stride_reference = descriptor
                .and_then(|descriptor| remote_enemy_stride_reference(descriptor, hit_zone_store));
            replication.cache_remote_enemy_walk_playback(remote.network_id, stride_reference);
            remote_materialize::materialize_armed_remote_enemy(
                remote,
                descriptors,
                registry,
                agent_params,
            )
        };
        let complete = if descriptor_entity {
            materialized
        } else {
            registry
                .has_component_kind(remote.entity_id, postretro_entities::ComponentKind::Mesh)
                .unwrap_or(false)
        };
        replication.cache_remote_presentation_complete(remote.network_id, complete);
        if materialized && let Some(state) = remote.initial_animation_state.as_deref() {
            client::apply_mesh_animation_state(registry, remote.entity_id, state, true);
            replication.seed_remote_player_direction(
                registry,
                remote.network_id,
                remote.entity_id,
                state,
            );
        }
        let attachment_changed = remote.weapon_attachment_changed
            && weapon_mounts::update_weapon_mount_attachments(
                registry,
                remote.entity_id,
                entity_class,
                descriptors,
                remote.active_weapon_archetype.as_deref(),
                hit_zone_store,
            );
        changed |= materialized || attachment_changed;
    }
    changed
}

/// A remote enemy's stride-rate reference from the shared descriptor: its base
/// `moveSpeed` and its graph's rate-scaled states. Neither belongs on the
/// snapshot wire format. `None` without a mesh, a behavior graph, or either a
/// scaled state the mesh declares or a state with a fixed `playbackRate`, so
/// the client leaves that enemy's rate alone.
///
/// The states are the host tick's playback-rate set, derived once here by the
/// same foundation function (`RateScaledStates`): the graph's locomotion state
/// and each state an activity plays under a motion that steers. The brain's
/// graph is the descriptor's authored `behavior` block, so both ends agree on
/// what this enemy carries. Directional variants scale too, each against its
/// own clip, and a state stays when the mesh declares its base or any variant,
/// because the host matches every presented variant of a set state.
fn remote_enemy_stride_reference(
    descriptor: &EntityTypeDescriptor,
    hit_zone_store: &HitZoneStore,
) -> Option<client::RemoteEnemyWalkPlayback> {
    let mesh = descriptor.mesh.as_ref()?;
    let graph = descriptor.behavior.as_ref()?;
    let model = hit_zone_store.get(&postretro_model::ModelHandle::from(mesh.model.clone()));
    let scaled_states: Vec<client::RemoteScaledState> =
        postretro_foundation::RateScaledStates::of(graph)
            .iter()
            .filter_map(|state| {
                // The same clip pick as the host's `effective_travel_speed`; a
                // descriptor state has no resolved clip index.
                let declared = TravelVariant::ALL
                    .map(|variant| mesh.animations.get(&format!("{state}{}", variant.suffix())));
                // A state the mesh declares in no variant has nothing to scale.
                declared
                    .iter()
                    .any(Option::is_some)
                    .then(|| client::RemoteScaledState {
                        state: state.to_string(),
                        derived_travel_speeds: declared
                            .map(|declared| model?.state_clip(None, &declared?.clip)?.travel_speed),
                    })
            })
            .collect();
    // A state with a fixed `playbackRate` needs the per-frame rate pass even
    // when nothing in the graph strides.
    let fixed_rates = mesh
        .animations
        .values()
        .any(|state| state.playback_rate.is_some());
    (!scaled_states.is_empty() || fixed_rates).then_some(client::RemoteEnemyWalkPlayback {
        move_speed: graph.move_speed,
        scaled_states,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    use postretro_entities::ComponentKind;
    use postretro_entities::components::billboard_emitter::BillboardEmitterComponent;
    use postretro_entities::data_descriptors::{
        HealthDescriptor, HitboxDescriptor, LightDescriptor,
    };
    use postretro_net::replication::ServerReplication;
    use postretro_net::wire::{self, RawSnapshotMessage};

    use crate::replication::{
        ReplicableSet, host_register_descriptor_entities, produce_owned_snapshots,
    };
    use crate::scripting::builtins::data_archetype::apply_data_archetype_dispatch;
    use crate::scripting::builtins::data_archetype_test_fixtures::{mesh_descriptor, placement};
    use crate::scripting::builtins::filter_out_client_host_replicated_placements;
    use crate::{HostCommandQueues, MovementOwners, NetworkIdAllocator};

    // Regression: Movement on a map placement selected the mesh-only pawn builder,
    // losing Light, Emitter and HitVolume, even caching a meshed copy as complete.
    #[test]
    fn mixed_movement_world_baseline_materializes_full_display_and_completes() {
        for with_mesh in [false, true] {
            let mut descriptor = mesh_descriptor("moving_health_target", false);
            if !with_mesh {
                descriptor.mesh = None;
            }
            descriptor.movement =
                Some(crate::predict_reconcile_harness_test_fixtures::player_descriptor());
            descriptor.health = Some(HealthDescriptor {
                max: 48.0,
                hitbox: Some(HitboxDescriptor {
                    half_extents: [0.5, 1.0, 0.5],
                    offset: None,
                }),
                zone_multipliers: Default::default(),
                ..Default::default()
            });
            descriptor.light = Some(LightDescriptor {
                color: [1.0, 0.5, 0.25],
                intensity: 2.0,
                range: 8.0,
                is_dynamic: true,
            });
            descriptor.emitter = Some(BillboardEmitterComponent {
                rate: 3.0,
                burst: None,
                spread: 0.0,
                lifetime: 1.0,
                velocity: [0.0; 3],
                buoyancy: 0.0,
                drag: 0.0,
                size_over_lifetime: [1.0].into(),
                opacity_over_lifetime: [1.0].into(),
                color: [1.0; 3],
                sprite: "smoke".into(),
                spin_rate: 0.0,
                spin_animation: None,
            });
            let descriptors = [descriptor];
            let placements = [placement("moving_health_target", &[])];
            assert!(
                filter_out_client_host_replicated_placements(&placements, &descriptors).is_empty()
            );
            let mut host = EntityRegistry::new();
            apply_data_archetype_dispatch(
                &placements,
                &descriptors,
                &HashSet::new(),
                &mut host,
                None,
            );
            let host_world = host
                .iter_with_kind(ComponentKind::DescriptorProvenance)
                .next()
                .unwrap()
                .0;
            // The same descriptor spawned through the actual slot-pawn path keeps
            // the narrower player presentation, regardless of its extra fields.
            let host_pawn = crate::scripting::builtins::net_descriptor::spawn_net_slot_pawn_with_carried_loadout(
                &placement("player_spawn", &[("entity_class", "moving_health_target")]),
                &descriptors,
                &mut host,
                None,
                None,
            )
            .unwrap();
            let mut allocator = NetworkIdAllocator::new();
            let mut replicable = ReplicableSet::new();
            host_register_descriptor_entities(
                &host,
                &mut allocator,
                &mut replicable,
                &mut HashSet::new(),
                &mut Default::default(),
            );
            replicable.register(host_pawn);
            let world_id = allocator.stamp(host_world);
            let pawn_id = allocator.stamp(host_pawn);
            let owned = produce_owned_snapshots(
                &host,
                &replicable,
                &mut allocator,
                &MovementOwners::new(),
                &HostCommandQueues::new(),
            );
            assert_eq!(owned.len(), 2);
            assert_eq!(
                owned
                    .iter()
                    .find(|record| record.network_id == world_id.0)
                    .unwrap()
                    .entity_class
                    .as_deref(),
                Some("descriptor:moving_health_target")
            );
            assert_eq!(
                owned
                    .iter()
                    .find(|record| record.network_id == pawn_id.0)
                    .unwrap()
                    .entity_class
                    .as_deref(),
                Some("pawn:moving_health_target")
            );
            let mut server = ServerReplication::new();
            server.register_client(7);
            server.ingest_tick(owned);
            let raw = server.encode_for_client(7, 1).unwrap();
            let mut snapshot = wire::decode::<RawSnapshotMessage>(&wire::encode(&raw))
                .unwrap()
                .validate()
                .unwrap();
            let mut registry = EntityRegistry::new();
            let mut replication = ClientReplication::new();
            let first = replication.apply_snapshot(&mut registry, &snapshot);
            assert_eq!(first.remote_entities.len(), 2);
            let remote = replication.entity_for_network_id(world_id).unwrap();
            let pawn = replication.entity_for_network_id(pawn_id).unwrap();
            let store = HitZoneStore::new();
            // Unknown descriptors stay retryable until the shared table arrives.
            assert!(!materialize_remote_entities(
                &first.remote_entities,
                &[],
                &mut registry,
                &mut replication,
                1,
                None,
                &store,
            ));
            snapshot.sequence += 1;
            let retry = replication.apply_snapshot(&mut registry, &snapshot);
            assert_eq!(retry.remote_entities.len(), 2);
            assert!(materialize_remote_entities(
                &retry.remote_entities,
                &descriptors,
                &mut registry,
                &mut replication,
                1,
                None,
                &store,
            ));
            for kind in [
                ComponentKind::Light,
                ComponentKind::BillboardEmitter,
                ComponentKind::HitVolume,
            ] {
                assert_eq!(registry.has_component_kind(remote, kind), Ok(true));
            }
            assert_eq!(
                registry.has_component_kind(remote, ComponentKind::Mesh),
                Ok(with_mesh)
            );
            for kind in [
                ComponentKind::Health,
                ComponentKind::Brain,
                ComponentKind::Agent,
                ComponentKind::Weapon,
                ComponentKind::Touchable,
                ComponentKind::PlayerMovement,
            ] {
                assert_eq!(registry.has_component_kind(remote, kind), Ok(false));
                assert_eq!(registry.has_component_kind(pawn, kind), Ok(false));
            }
            assert_eq!(
                registry.has_component_kind(pawn, ComponentKind::Mesh),
                Ok(with_mesh)
            );
            for kind in [
                ComponentKind::Light,
                ComponentKind::BillboardEmitter,
                ComponentKind::HitVolume,
            ] {
                assert_eq!(registry.has_component_kind(pawn, kind), Ok(false));
            }
            snapshot.sequence += 1;
            let complete = replication.apply_snapshot(&mut registry, &snapshot);
            assert!(
                complete
                    .remote_entities
                    .iter()
                    .all(|request| request.network_id != world_id)
            );
            assert_eq!(complete.remote_entities.len(), usize::from(!with_mesh));
            assert_eq!(registry.iter_with_kind(ComponentKind::HitVolume).count(), 1);
        }
    }

    // The client keeps the host's rate-scaled set: a state whose base the mesh
    // never declares stays when it declares a directional variant, so a mesh
    // with `run_left` but no `run` scales `run_left` on host and client alike.
    // Position goals count, and a state the mesh declares in no variant drops.
    #[test]
    fn a_remote_stride_reference_keeps_every_host_scaled_state_the_mesh_declares_in_any_variant() {
        use postretro_entities::components::brain::BrainComponent;
        use postretro_entities::components::mesh::{AnimationState, InterruptPolicy};

        let graph: postretro_foundation::BehaviorGraphDescriptor =
            serde_json::from_value(serde_json::json!({
                "initial": "idle",
                "moveSpeed": 3.0,
                "patrol": { "mode": "loop", "points": [[0.0, 0.0], [6.0, 0.0]] },
                "activities": {
                    "idle": { "animation": "idle", "motion": "hold" },
                    "engage": { "animation": "run", "motion": "chaseTarget" },
                    "wander": { "animation": "walk", "motion": "patrol" },
                    "retreat": { "animation": "limp", "motion": "moveToAnchor" }
                },
                "transitions": { "idle": [] }
            }))
            .expect("graph deserializes");
        let state = |clip: &str| AnimationState {
            clip: clip.to_string(),
            looping: true,
            crossfade_ms: 0.0,
            interrupt: InterruptPolicy::Smooth,
            travel_speed: None,
            max_playback_rate: None,
            scale_with_speed: true,
            playback_rate: None,
            clip_index: None,
        };
        let mut descriptor = mesh_descriptor("strider", true);
        let mesh = descriptor.mesh.as_mut().expect("fixture has a mesh");
        mesh.animations = ["idle", "walk", "run_left"]
            .into_iter()
            .map(|name| (name.to_string(), state(name)))
            .collect();
        descriptor.behavior = Some(graph.clone());

        let reference = remote_enemy_stride_reference(&descriptor, &HitZoneStore::new())
            .expect("the mesh declares scaled states");
        assert!(
            (reference.move_speed - 3.0).abs() < 1e-6,
            "{}",
            reference.move_speed
        );
        let mut kept: Vec<&str> = reference
            .scaled_states
            .iter()
            .map(|scaled| scaled.state.as_str())
            .collect();
        kept.sort_unstable();
        assert_eq!(kept, ["run", "walk"], "`limp` is declared in no variant");

        let host = BrainComponent::from_graph(&graph);
        for presented in ["run_left", "walk"] {
            let client_scales = reference
                .scaled_states
                .iter()
                .any(|scaled| TravelVariant::of_state(&scaled.state, presented).is_some());
            let host_scales = host
                .rate_scaled_states()
                .iter()
                .any(|scaled| TravelVariant::of_state(scaled, presented).is_some());
            assert!(client_scales && host_scales, "`{presented}` scales on both");
        }
    }
}
