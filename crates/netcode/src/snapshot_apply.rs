//! Descriptor-aware remote snapshot materialization, after descriptor-blind apply.
//! See: context/lib/networking.md

use postretro_entities::{EntityRegistry, EntityTypeDescriptor};
use postretro_foundation::NavAgentParams;

use super::client::ClientReplication;
use super::scripting_systems::hit_zones::HitZoneStore;
use super::{client, descriptor_class, remote_materialize};

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
                        let speed = model?
                            .clips
                            .iter()
                            .find(|clip| clip.name == state.clip)?
                            .travel_speed?;
                        Some((state_name.to_string(), speed))
                    })
                    .collect();
                Some(client::RemotePlayerLocomotionReference {
                    idle_state,
                    states,
                    walk_speed: movement.ground.speed.walk,
                    run_speed: movement.ground.speed.run,
                    derived_travel_speeds,
                })
            });
            replication.cache_remote_player_locomotion(remote.network_id, player_locomotion);
            remote_materialize::materialize_armed_remote_player(
                remote,
                descriptors,
                registry,
                agent_params,
            )
        } else {
            // Remote AI has no local AgentComponent, so derive its walk rate from
            // exactly the motion this client presents. The shared descriptor supplies
            // the immutable reference speed and the locomotion state; neither belongs
            // on the snapshot wire format.
            //
            // "The walking state" is picked by the same rule the host tick's
            // animation substitution uses: the graph's locomotion state, which chases
            // without an action of its own. The brain's graph is the descriptor's
            // authored `behavior` block, so both ends agree on what this enemy carries.
            let walk_reference = descriptor.and_then(|descriptor| {
                let mesh = descriptor.mesh.as_ref()?;
                let graph = descriptor.behavior.as_ref()?;
                let locomotion = postretro_foundation::locomotion_animation(graph)?;
                let state = mesh.animations.get(locomotion)?;
                let derived_travel_speed = hit_zone_store
                    .get(&postretro_model::ModelHandle::from(mesh.model.clone()))
                    .and_then(|model| {
                        model
                            .clips
                            .iter()
                            .find(|clip| clip.name == state.clip)
                            .and_then(|clip| clip.travel_speed)
                    });
                Some((
                    graph.move_speed,
                    locomotion.to_string(),
                    derived_travel_speed,
                ))
            });
            replication.cache_remote_enemy_walk_playback(remote.network_id, walk_reference);
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
            && remote_materialize::update_active_weapon_attachment(
                registry,
                remote.entity_id,
                descriptors,
                remote.active_weapon_archetype.as_deref(),
                hit_zone_store,
            );
        changed |= materialized || attachment_changed;
    }
    changed
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
}
