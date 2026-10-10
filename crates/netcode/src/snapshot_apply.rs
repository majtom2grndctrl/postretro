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
        } else if matches!(descriptor, Some(descriptor) if descriptor.movement.is_some()) {
            let player_locomotion = descriptor.and_then(|descriptor| {
                let movement = descriptor.movement.as_ref()?;
                let mesh = descriptor.mesh.as_ref()?;
                let idle_state = mesh.default_state.clone()?;
                let walk_state = ["walk_forward", "walk"]
                    .into_iter()
                    .find(|state| mesh.animations.contains_key(*state))?
                    .to_string();
                let run_state = mesh
                    .animations
                    .contains_key("run")
                    .then(|| "run".to_string());
                let derived_travel_speed = |state_name: &str| {
                    let state = mesh.animations.get(state_name)?;
                    hit_zone_store
                        .get(&postretro_model::ModelHandle::from(mesh.model.clone()))
                        .and_then(|model| {
                            model
                                .clips
                                .iter()
                                .find(|clip| clip.name == state.clip)
                                .and_then(|clip| clip.travel_speed)
                        })
                };
                Some(client::RemotePlayerLocomotionReference {
                    idle_state,
                    walk_derived_travel_speed: derived_travel_speed(&walk_state),
                    run_derived_travel_speed: run_state.as_deref().and_then(derived_travel_speed),
                    walk_state,
                    run_state,
                    walk_speed: movement.ground.speed.walk,
                    run_speed: movement.ground.speed.run,
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
        if materialized && let Some(state) = remote.initial_animation_state.as_deref() {
            client::apply_mesh_animation_state(registry, remote.entity_id, state, true);
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
