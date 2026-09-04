// Shared descriptor-presentation materialization and runtime attachment updates
// for local and remote player entities.
// See: context/lib/networking.md

use postretro_entities::components::mesh::{MeshAttachment, MeshComponent};
use postretro_entities::provenance::{DescriptorProvenance, DescriptorSpawnPath};
use postretro_entities::{EntityId, EntityRegistry, EntityTypeDescriptor};
use postretro_foundation::NavAgentParams;

use super::client::{ArmedLocalPawn, RemoteEntityMaterialize};
use super::descriptor_class::decode_replicated_descriptor_class;

/// Reserved socket for the dynamic third-person active-weapon prop. Descriptor
/// attachments on other sockets remain untouched when an active weapon changes.
pub(super) const ACTIVE_WEAPON_SOCKET: &str = "hand_r";

/// Whether a descriptor-backed player body is being presented to its owning
/// viewer or to another client. `shadowOnly` is authored for the first-person
/// owner view; peers must still see that same descriptor as a forward avatar.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PlayerViewerRole {
    Local,
    Remote,
}

impl PlayerViewerRole {
    fn shadow_only(self, descriptor_shadow_only: bool) -> bool {
        descriptor_shadow_only && matches!(self, Self::Local)
    }
}

fn apply_player_viewer_role(
    entity_class: &str,
    descriptors: &[EntityTypeDescriptor],
    registry: &mut EntityRegistry,
    id: EntityId,
    role: PlayerViewerRole,
) {
    let Some(descriptor_shadow_only) = descriptors
        .iter()
        .find(|descriptor| descriptor.canonical_name.as_deref() == Some(entity_class))
        .and_then(|descriptor| descriptor.mesh.as_ref())
        .map(|mesh| mesh.shadow_only)
    else {
        return;
    };
    let Ok(mut mesh) = registry.get_component::<MeshComponent>(id).cloned() else {
        return;
    };
    let shadow_only = role.shadow_only(descriptor_shadow_only);
    if mesh.shadow_only != shadow_only {
        mesh.shadow_only = shadow_only;
        let _ = registry.set_component(id, mesh);
    }
}

/// Apply the peer-view policy to a descriptor-backed player mesh that another
/// host-side spawn path already materialized. Listen-host slot pawns use the full
/// authoritative descriptor spawn, so they need this presentation-only override
/// without re-materializing any gameplay components.
pub(super) fn apply_remote_player_viewer_role(
    entity_class: &str,
    descriptors: &[EntityTypeDescriptor],
    registry: &mut EntityRegistry,
    id: EntityId,
) {
    apply_player_viewer_role(
        entity_class,
        descriptors,
        registry,
        id,
        PlayerViewerRole::Remote,
    );
}

/// Replace the dynamic active-weapon attachment on a player mesh. The descriptor
/// lookup deliberately resolves only the shared-visible canonical archetype; no
/// owner-private weapon component state crosses this presentation seam.
///
/// A model missing from the level's preloaded CPU model set clears the socket instead
/// of leaving a stale attachment behind. The caller runs the existing binding resolver
/// after a `true` return, which fills the transient [`AttachmentBinding::Skinned`]
/// cache from the holder's authored `hand_r` socket.
pub(super) fn update_active_weapon_attachment(
    registry: &mut EntityRegistry,
    pawn: EntityId,
    descriptors: &[EntityTypeDescriptor],
    active_weapon_archetype: Option<&str>,
    hit_zone_store: &crate::scripting_systems::hit_zones::HitZoneStore,
) -> bool {
    let Ok(mut mesh) = registry.get_component::<MeshComponent>(pawn).cloned() else {
        return false;
    };

    let desired_model = active_weapon_archetype
        .and_then(|archetype| {
            descriptors
                .iter()
                .find(|descriptor| descriptor.canonical_name.as_deref() == Some(archetype))
        })
        .and_then(|descriptor| descriptor.weapon.as_ref())
        .and_then(|weapon| weapon.third_person_model.as_deref())
        .filter(|model| !model.is_empty())
        .filter(|model| hit_zone_store.get_by_name(model).is_some());

    let socket_attachments: Vec<_> = mesh
        .attachments
        .iter()
        .filter(|attachment| attachment.socket == ACTIVE_WEAPON_SOCKET)
        .collect();
    let already_matches = match desired_model {
        Some(model) => {
            socket_attachments.len() == 1 && socket_attachments[0].model.as_str() == model
        }
        None => socket_attachments.is_empty(),
    };
    if already_matches {
        return false;
    }

    mesh.attachments
        .retain(|attachment| attachment.socket != ACTIVE_WEAPON_SOCKET);
    if let Some(model) = desired_model {
        mesh.attachments.push(MeshAttachment::unresolved(
            ACTIVE_WEAPON_SOCKET.to_string(),
            model.to_string(),
        ));
    }
    let _ = registry.set_component(pawn, mesh);
    true
}

/// Materialize the descriptor-backed presentation for a `local_player` baseline this
/// snapshot armed. `apply_snapshot` spawned the pawn
/// Transform-only; host movement tuning arrives independently on Control, so this
/// materializes or clears the `PlayerMovementComponent` when both halves meet.
///
/// Defaults to the `"player"` class when the host stamped none (defensive). Must run
/// BEFORE reconcile (which merges onto the existing component); the underlying helper is
/// idempotent, so a re-arm of the same pawn keeps its live state.
///
/// This seam also carries remote presentation calls so descriptor
/// materialization stays explicit inside the client receive/apply phase instead
/// of leaking into the registry-blind snapshot decoder.
pub(super) fn materialize_armed_local_pawn(
    armed: &ArmedLocalPawn,
    descriptors: &[EntityTypeDescriptor],
    registry: &mut EntityRegistry,
    host_movement: Option<&postretro_foundation::PlayerMovementDescriptor>,
    host_tuning: Option<&super::TuningPayload>,
    rebuild_movement: bool,
) -> bool {
    let entity_class = armed.entity_class.as_deref().unwrap_or("player");
    let entity_class = decode_replicated_descriptor_class(entity_class).canonical_name();
    let had_mesh = registry
        .has_component_kind(armed.entity_id, postretro_entities::ComponentKind::Mesh)
        .unwrap_or(false);
    if let Some(host_movement) = host_movement {
        // View feel is deliberately local presentation. The host payload carries
        // it as null, so recover only this one field from the local descriptor.
        let mut movement = host_movement.clone();
        movement.view_feel = descriptors
            .iter()
            .find(|descriptor| descriptor.canonical_name.as_deref() == Some(entity_class))
            .and_then(|descriptor| descriptor.movement.as_ref())
            .and_then(|descriptor| descriptor.view_feel.clone());
        crate::scripting::builtins::net_descriptor::materialize_net_local_movement_component_from_tuning(
            &movement,
            registry,
            armed.entity_id,
            rebuild_movement,
        );
    } else {
        if rebuild_movement {
            let _ = registry
                .remove_component::<postretro_foundation::PlayerMovementComponent>(armed.entity_id);
        }
        log::warn!("[Net] local pawn has no host movement tuning; movement prediction stays inert");
    }
    let _ = crate::scripting::builtins::net_descriptor::materialize_net_local_wieldable_inventory_from_tuning(
        entity_class,
        descriptors,
        registry,
        armed.entity_id,
        host_tuning,
    );
    crate::scripting::builtins::net_descriptor::materialize_net_mesh_presentation(
        entity_class,
        descriptors,
        registry,
        armed.entity_id,
        None,
    );
    apply_player_viewer_role(
        entity_class,
        descriptors,
        registry,
        armed.entity_id,
        PlayerViewerRole::Local,
    );
    !had_mesh
        && registry
            .has_component_kind(armed.entity_id, postretro_entities::ComponentKind::Mesh)
            .unwrap_or(false)
}

/// Materialize a remote descriptor-backed player as presentation only. The player
/// type signal is selected by the caller from the descriptor's `movement` block;
/// this seam deliberately attaches only the descriptor mesh. A remote client never
/// gains a simulation copy of another player's movement, weapon, or health state.
/// Unknown classes and meshless descriptors remain transform-only, while a repeated
/// call preserves the existing mesh animation state.
pub(super) fn materialize_armed_remote_player(
    remote: &RemoteEntityMaterialize,
    descriptors: &[EntityTypeDescriptor],
    registry: &mut EntityRegistry,
    agent_params: Option<NavAgentParams>,
) -> bool {
    let entity_class = decode_replicated_descriptor_class(&remote.entity_class).canonical_name();
    let materialized =
        crate::scripting::builtins::net_descriptor::materialize_net_mesh_presentation(
            entity_class,
            descriptors,
            registry,
            remote.entity_id,
            agent_params,
        );
    if materialized {
        apply_player_viewer_role(
            entity_class,
            descriptors,
            registry,
            remote.entity_id,
            PlayerViewerRole::Remote,
        );
    }
    materialized
}

/// Materialize the descriptor-backed *presentation* for a non-local remote enemy a
/// snapshot just spawned. `apply_snapshot` spawned the entity
/// Transform-only and mapped its `NetworkId` (so it joins the Phase 2 remote
/// interpolation path). The host replicates finite Transform plus optional current
/// mesh-animation state, while descriptor mesh data stays local and authoritative
/// AI/damage/death stay host-only, so the client attaches ONLY presentation and NONE of
/// `Brain`/`Agent`/`Health`/`Weapon`/`PlayerMovement`.
///
/// The descriptor lookup lives here, NOT in `ClientReplication::apply_snapshot`: the
/// net-facing apply is descriptor-blind, and this is where the shared descriptor table
/// is in scope. The underlying helper is idempotent (a re-apply never resets live mesh
/// animation state) and unknown-class-tolerant (an unregistered class leaves the entity
/// transform-only, logged, and never rejects the snapshot — the entity still
/// interpolates from its mapped `Transform`).
pub(super) fn materialize_armed_remote_enemy(
    remote: &RemoteEntityMaterialize,
    descriptors: &[EntityTypeDescriptor],
    registry: &mut EntityRegistry,
    agent_params: Option<NavAgentParams>,
) -> bool {
    let entity_class = decode_replicated_descriptor_class(&remote.entity_class).canonical_name();
    crate::scripting::builtins::net_descriptor::materialize_net_mesh_presentation(
        entity_class,
        descriptors,
        registry,
        remote.entity_id,
        agent_params,
    )
}

/// Materialize a host-replicated projectile observer copy from the shared firing
/// weapon descriptor. This attaches only the body and optional trail presentation;
/// in particular it never creates a `ProjectileComponent`, `Health`, or weapon state
/// on a connected client.
pub(super) fn materialize_armed_remote_projectile(
    remote: &RemoteEntityMaterialize,
    descriptors: &[EntityTypeDescriptor],
    registry: &mut EntityRegistry,
    spawn_tick: u32,
) -> bool {
    let entity_class = decode_replicated_descriptor_class(&remote.entity_class).canonical_name();
    let Some(projectile) = descriptors
        .iter()
        .find(|descriptor| descriptor.canonical_name.as_deref() == Some(entity_class))
        .and_then(|descriptor| descriptor.weapon.as_ref())
        .and_then(|weapon| weapon.projectile.as_ref())
    else {
        return false;
    };
    super::projectile_presentation::attach_projectile_visual_components(
        registry,
        remote.entity_id,
        projectile,
        spawn_tick,
    );
    let _ = registry.set_component(
        remote.entity_id,
        DescriptorProvenance {
            canonical_name: entity_class.to_string(),
            owned_components: Default::default(),
            map_overrides: Default::default(),
            spawn_path: DescriptorSpawnPath::ProjectilePresentation,
        },
    );
    true
}

#[cfg(test)]
mod tests {
    use super::*;
    use glam::{Quat, Vec3};
    use postretro_entities::components::mesh::{
        AnimationState, AttachmentBinding, InterruptPolicy, MeshComponent,
    };
    use postretro_entities::{ComponentKind, EntityId, MeshDescriptor, Transform};
    use postretro_foundation::{
        AirParams, CapsuleParams, FallParams, FireMode, GroundParams, NavAgentParams,
        PlayerMovementDescriptor, ProjectileBodyVisual, ProjectileDescriptor,
        ProjectileTrailSpinAnimation, ProjectileTrailVisual, ProjectileVisual, ResolutionMode,
        SpeedParams, WeaponDescriptor,
    };
    use std::collections::HashMap;
    use std::sync::Arc;

    /// A minimal descriptor carrying only a two-state animated mesh, mirroring the
    /// validated descriptor shape a remote enemy materializes from.
    fn enemy_mesh_descriptor(classname: &str) -> EntityTypeDescriptor {
        let mut states = HashMap::new();
        states.insert(
            "idle".to_string(),
            AnimationState {
                clip: "idle_clip".to_string(),
                looping: true,
                crossfade_ms: 150.0,
                interrupt: InterruptPolicy::Smooth,
                travel_speed: None,
                clip_index: None,
            },
        );
        EntityTypeDescriptor {
            canonical_name: Some(classname.to_string()),
            inventory: None,
            light: None,
            emitter: None,
            movement: None,
            weapon: None,
            touchable: None,
            mesh: Some(MeshDescriptor {
                model: "decraniated".to_string(),
                shadow_only: false,
                attachments: Default::default(),
                shadow_bias_scale: 1.0,
                animations: states,
                default_state: Some("idle".to_string()),
                locomotion: None,
            }),
            health: None,
            behavior: None,
        }
    }

    fn player_mesh_descriptor(classname: &str) -> EntityTypeDescriptor {
        let mut descriptor = enemy_mesh_descriptor(classname);
        descriptor.mesh.as_mut().unwrap().shadow_only = true;
        descriptor.movement = Some(PlayerMovementDescriptor {
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
        });
        descriptor
    }

    fn third_person_weapon_descriptor(classname: &str, model: &str) -> EntityTypeDescriptor {
        let mut descriptor = enemy_mesh_descriptor(classname);
        descriptor.mesh = None;
        descriptor.weapon = Some(WeaponDescriptor {
            damage: 1.0,
            pellet_count: 1,
            spread_degrees: 0.0,
            range: 1.0,
            cooldown_ms: 1.0,
            fire_mode: FireMode::Semi,
            resolution: ResolutionMode::Hitscan,
            projectile: None,
            credit_source: None,
            third_person_model: Some(model.to_string()),
            viewmodel: None,
            placement: None,
            muzzle_offset: None,
            resource: None,
            lower_ms: 0,
            raise_ms: 0,
            block_during_reload: None,
        });
        descriptor
    }

    fn projectile_weapon_descriptor(classname: &str) -> EntityTypeDescriptor {
        let mut descriptor = enemy_mesh_descriptor(classname);
        descriptor.mesh = None;
        descriptor.weapon = Some(WeaponDescriptor {
            damage: 1.0,
            pellet_count: 1,
            spread_degrees: 0.0,
            range: 16.0,
            cooldown_ms: 1.0,
            fire_mode: FireMode::Semi,
            resolution: ResolutionMode::Projectile,
            projectile: Some(ProjectileDescriptor {
                speed: 8.0,
                radius: 0.1,
                lifetime_ms: 1_000.0,
                visual: ProjectileVisual {
                    body: ProjectileBodyVisual::Sprite {
                        sprite: "sprites/projectiles/bolt.png".to_string(),
                        size: 0.4,
                        opacity: 1.0,
                        rotation: 0.0,
                        tint: [0.2, 0.8, 1.0],
                        emissive: 0.0,
                        frame_duration_ms: None,
                    },
                    trail: Some(ProjectileTrailVisual {
                        sprite: "sprites/projectiles/trail.png".to_string(),
                        rate: 20.0,
                        lifetime: 0.3,
                        burst: None,
                        spread: 0.0,
                        velocity: [0.0; 3],
                        buoyancy: 0.0,
                        drag: 0.0,
                        size_over_lifetime: vec![0.2, 0.0],
                        opacity_over_lifetime: vec![1.0, 0.0],
                        color: [1.0; 3],
                        spin_rate: 0.0,
                        spin_animation: Some(ProjectileTrailSpinAnimation {
                            duration: 0.5,
                            rate_curve: vec![0.0, 1.0],
                        }),
                    }),
                    light: None,
                    impact_light: None,
                },
            }),
            credit_source: None,
            third_person_model: None,
            viewmodel: None,
            placement: None,
            muzzle_offset: None,
            resource: None,
            lower_ms: 0,
            raise_ms: 0,
            block_during_reload: None,
        });
        descriptor
    }

    fn test_hit_zones(
        sockets: HashMap<String, postretro_model::gltf_loader::SocketBinding>,
    ) -> crate::scripting_systems::hit_zones::ModelHitZones {
        crate::scripting_systems::hit_zones::ModelHitZones {
            skeleton: Arc::new(postretro_model::skeleton::Skeleton::default()),
            clips: Arc::new(Vec::new()),
            joint_zones: Vec::new(),
            sockets,
            derived_bound: None,
            legs: Vec::new(),
            pose_stack: Arc::new(postretro_model::pose_modifier::PoseModifierStack::default()),
        }
    }

    fn attachment_store() -> crate::scripting_systems::hit_zones::HitZoneStore {
        use postretro_model::skeleton::{Joint, RestLocal, Skeleton};

        let mut store = crate::scripting_systems::hit_zones::HitZoneStore::new();
        let mut holder = test_hit_zones(HashMap::from([(
            ACTIVE_WEAPON_SOCKET.to_string(),
            postretro_model::gltf_loader::SocketBinding::SkinnedJoint(3),
        )]));
        holder.skeleton = Arc::new(Skeleton {
            joints: vec![
                Joint {
                    parent: None,
                    inverse_bind: glam::Mat4::IDENTITY.to_cols_array_2d(),
                    rest_local: RestLocal::default(),
                },
                Joint {
                    parent: Some(0),
                    inverse_bind: glam::Mat4::IDENTITY.to_cols_array_2d(),
                    rest_local: RestLocal {
                        translation: Vec3::new(0.25, 0.0, 0.0),
                        ..RestLocal::default()
                    },
                },
                Joint {
                    parent: Some(1),
                    inverse_bind: glam::Mat4::IDENTITY.to_cols_array_2d(),
                    rest_local: RestLocal {
                        translation: Vec3::new(0.0, 0.5, 0.0),
                        ..RestLocal::default()
                    },
                },
                Joint {
                    parent: Some(2),
                    inverse_bind: glam::Mat4::IDENTITY.to_cols_array_2d(),
                    rest_local: RestLocal {
                        translation: Vec3::new(0.0, 0.0, 0.75),
                        ..RestLocal::default()
                    },
                },
            ],
        });
        store.insert_for_test(postretro_model::ModelHandle::from("decraniated"), holder);
        store.insert_for_test(
            postretro_model::ModelHandle::from("models/pistol/model.gltf"),
            test_hit_zones(HashMap::new()),
        );
        store
    }

    fn resolved_attachment_transform(
        mesh: &MeshComponent,
        store: &crate::scripting_systems::hit_zones::HitZoneStore,
    ) -> glam::Mat4 {
        let mut instances = Vec::new();
        let mut world_pose = Vec::new();
        let mut attachment_handles = HashMap::new();
        let holder = postretro_model::ModelHandle::from(mesh.model.as_str());
        let resolver = crate::scripting_systems::attachments::SocketPoseResolver::new(store);
        assert!(crate::scripting_systems::attachments::emit_for_holder(
            &mut instances,
            &mut world_pose,
            &mut attachment_handles,
            &resolver,
            &holder,
            glam::Mat4::from_translation(Vec3::new(1.0, 2.0, 3.0)),
            postretro_model::sample_params::MeshSampleParams::rigid(),
            mesh.pose_inputs,
            mesh.shadow_bias_scale,
            9,
            true,
            true,
            &mesh.attachments,
        ));
        assert_eq!(
            instances.len(),
            1,
            "one resolved hand attachment emits one prop"
        );
        instances[0].transform
    }

    fn assert_mat4_approx(actual: glam::Mat4, expected: glam::Mat4, context: &str) {
        for (index, (actual, expected)) in actual
            .to_cols_array()
            .into_iter()
            .zip(expected.to_cols_array())
            .enumerate()
        {
            assert!(
                (actual - expected).abs() < 1.0e-5,
                "{context}: matrix element {index}: expected {expected}, got {actual}",
            );
        }
    }

    fn spawn_transform_only(reg: &mut EntityRegistry) -> EntityId {
        reg.try_spawn(
            Transform {
                position: Vec3::new(1.0, 2.0, 3.0),
                rotation: Quat::IDENTITY,
                scale: Vec3::ONE,
            },
            &[],
        )
        .expect("registry has room for one entity")
    }

    // The remote-enemy glue resolves the request's class against the shared descriptor
    // table and attaches ONLY the descriptor's mesh — never authoritative AI state.
    #[test]
    fn materialize_armed_remote_enemy_attaches_mesh_only() {
        let descriptors = vec![enemy_mesh_descriptor("decraniated_mob")];
        let mut reg = EntityRegistry::new();
        let id = spawn_transform_only(&mut reg);

        materialize_armed_remote_enemy(
            &RemoteEntityMaterialize {
                network_id: postretro_net::wire::NetworkId(7),
                entity_id: id,
                // Regression: semantic descriptor tags reached canonical lookup
                // consumers verbatim and left replicated entities meshless.
                entity_class: "descriptor:decraniated_mob".to_string(),
                initial_animation_state: None,
                active_weapon_archetype: None,
                weapon_attachment_changed: false,
            },
            &descriptors,
            &mut reg,
            Some(NavAgentParams {
                radius: 0.4,
                height: 1.6,
                step_height: 0.3,
                max_slope_deg: 45.0,
            }),
        );

        // Presentation mesh is present and renders the descriptor model.
        let mesh = reg
            .get_component::<MeshComponent>(id)
            .expect("remote enemy renders its descriptor mesh");
        assert_eq!(mesh.model, "decraniated");
        // The Transform survives (interpolation still flows through the mapped entity).
        assert_eq!(
            reg.get_component::<Transform>(id).unwrap().position,
            Vec3::new(1.0, 2.0, 3.0)
        );
        // No authoritative state crosses to the client viewer.
        for kind in [
            ComponentKind::Brain,
            ComponentKind::Agent,
            ComponentKind::Health,
            ComponentKind::Weapon,
            ComponentKind::PlayerMovement,
        ] {
            assert_eq!(
                reg.has_component_kind(id, kind),
                Ok(false),
                "remote enemy presentation must not attach {kind:?}"
            );
        }
    }

    #[test]
    fn materialize_armed_remote_projectile_attaches_visuals_without_gameplay_state() {
        let descriptors = vec![projectile_weapon_descriptor("plasma_rifle")];
        let mut reg = EntityRegistry::new();
        let id = spawn_transform_only(&mut reg);
        let request = RemoteEntityMaterialize {
            network_id: postretro_net::wire::NetworkId(12),
            entity_id: id,
            entity_class: "projectile:plasma_rifle".to_string(),
            initial_animation_state: None,
            active_weapon_archetype: None,
            weapon_attachment_changed: false,
        };

        assert!(materialize_armed_remote_projectile(
            &request,
            &descriptors,
            &mut reg,
            42,
        ));
        assert_eq!(
            reg.projectile_presentation_age(id)
                .expect("materialization retains its authoritative fixed-tick stamp")
                .spawn_tick,
            42
        );
        assert_eq!(
            reg.get_component::<postretro_entities::components::sprite_visual::SpriteVisual>(id)
                .unwrap()
                .sprite,
            "sprites/projectiles/bolt.png"
        );
        let trail = reg
            .get_component::<postretro_entities::components::billboard_emitter::BillboardEmitterComponent>(id)
            .unwrap();
        assert_eq!(trail.sprite, "sprites/projectiles/trail.png");
        let animation = trail
            .spin_animation
            .as_ref()
            .expect("remote trail keeps descriptor spin animation");
        assert!((animation.duration - 0.5).abs() <= f32::EPSILON);
        assert_eq!(animation.rate_curve, [0.0, 1.0]);
        assert_eq!(
            reg.get_component::<DescriptorProvenance>(id)
                .expect("projectile presentation keeps descriptor provenance")
                .canonical_name,
            "plasma_rifle",
            "semantic representation must not leak into canonical provenance"
        );
        for kind in [
            ComponentKind::Projectile,
            ComponentKind::Health,
            ComponentKind::Weapon,
            ComponentKind::PlayerMovement,
        ] {
            assert_eq!(reg.has_component_kind(id, kind), Ok(false));
        }
    }

    // An unknown descriptor class leaves the entity transform-only — the glue logs and
    // returns without attaching a mesh, and the entity keeps its Transform.
    #[test]
    fn materialize_armed_remote_enemy_unknown_class_leaves_transform_only() {
        let descriptors = vec![enemy_mesh_descriptor("decraniated_mob")];
        let mut reg = EntityRegistry::new();
        let id = spawn_transform_only(&mut reg);

        materialize_armed_remote_enemy(
            &RemoteEntityMaterialize {
                network_id: postretro_net::wire::NetworkId(7),
                entity_id: id,
                entity_class: "no_such_class".to_string(),
                initial_animation_state: None,
                active_weapon_archetype: None,
                weapon_attachment_changed: false,
            },
            &descriptors,
            &mut reg,
            None,
        );

        assert_eq!(
            reg.has_component_kind(id, ComponentKind::Mesh),
            Ok(false),
            "unknown class leaves the entity transform-only"
        );
        assert_eq!(
            reg.get_component::<Transform>(id).unwrap().position,
            Vec3::new(1.0, 2.0, 3.0),
            "the Transform survives so the entity still interpolates"
        );
    }

    // A second materialize call for the same entity does not reset live mesh-animation
    // state (idempotent through the helper).
    #[test]
    fn materialize_armed_remote_enemy_is_idempotent() {
        let descriptors = vec![enemy_mesh_descriptor("decraniated_mob")];
        let mut reg = EntityRegistry::new();
        let id = spawn_transform_only(&mut reg);
        let request = RemoteEntityMaterialize {
            network_id: postretro_net::wire::NetworkId(7),
            entity_id: id,
            entity_class: "decraniated_mob".to_string(),
            initial_animation_state: None,
            active_weapon_archetype: None,
            weapon_attachment_changed: false,
        };

        materialize_armed_remote_enemy(&request, &descriptors, &mut reg, None);

        // Drive the live animation state forward so a reset would be observable.
        {
            let mut mesh = reg.get_component::<MeshComponent>(id).unwrap().clone();
            mesh.animation.as_mut().unwrap().current_state = "moved".to_string();
            reg.set_component(id, mesh).unwrap();
        }

        materialize_armed_remote_enemy(&request, &descriptors, &mut reg, None);

        let mesh = reg.get_component::<MeshComponent>(id).unwrap();
        assert_eq!(
            mesh.animation.as_ref().unwrap().current_state,
            "moved",
            "a second materialize must not reset live animation state"
        );
    }

    #[test]
    fn materialize_armed_remote_player_is_forward_visible_without_authority() {
        let descriptors = vec![player_mesh_descriptor("co_op_avatar")];
        let mut reg = EntityRegistry::new();
        let id = spawn_transform_only(&mut reg);
        let request = RemoteEntityMaterialize {
            network_id: postretro_net::wire::NetworkId(9),
            entity_id: id,
            entity_class: "co_op_avatar".to_string(),
            initial_animation_state: None,
            active_weapon_archetype: None,
            weapon_attachment_changed: false,
        };

        assert!(materialize_armed_remote_player(
            &request,
            &descriptors,
            &mut reg,
            None,
        ));
        assert!(
            !reg.get_component::<MeshComponent>(id).unwrap().shadow_only,
            "descriptor shadowOnly applies to the owning viewer, not remote peers",
        );
        for kind in [
            ComponentKind::PlayerMovement,
            ComponentKind::Weapon,
            ComponentKind::Health,
        ] {
            assert_eq!(reg.has_component_kind(id, kind), Ok(false));
        }
    }

    #[test]
    fn materialize_armed_remote_player_is_idempotent_and_unknown_stays_transform_only() {
        let descriptors = vec![player_mesh_descriptor("co_op_avatar")];
        let mut reg = EntityRegistry::new();
        let id = spawn_transform_only(&mut reg);
        let request = RemoteEntityMaterialize {
            network_id: postretro_net::wire::NetworkId(11),
            entity_id: id,
            entity_class: "co_op_avatar".to_string(),
            initial_animation_state: None,
            active_weapon_archetype: None,
            weapon_attachment_changed: false,
        };

        assert!(materialize_armed_remote_player(
            &request,
            &descriptors,
            &mut reg,
            None,
        ));
        let mut mesh = reg.get_component::<MeshComponent>(id).unwrap().clone();
        mesh.animation.as_mut().unwrap().current_state = "moved".to_string();
        reg.set_component(id, mesh).unwrap();
        assert!(materialize_armed_remote_player(
            &request,
            &descriptors,
            &mut reg,
            None,
        ));
        assert_eq!(
            reg.get_component::<MeshComponent>(id)
                .unwrap()
                .animation
                .as_ref()
                .unwrap()
                .current_state,
            "moved"
        );
        assert!(
            !reg.get_component::<MeshComponent>(id).unwrap().shadow_only,
            "re-materialization must preserve remote-view forward visibility",
        );

        let unknown_id = spawn_transform_only(&mut reg);
        assert!(!materialize_armed_remote_player(
            &RemoteEntityMaterialize {
                network_id: postretro_net::wire::NetworkId(12),
                entity_id: unknown_id,
                entity_class: "not_a_class".to_string(),
                initial_animation_state: None,
                active_weapon_archetype: None,
                weapon_attachment_changed: false,
            },
            &descriptors,
            &mut reg,
            None,
        ));
        assert_eq!(
            reg.has_component_kind(unknown_id, ComponentKind::Mesh),
            Ok(false)
        );
    }

    #[test]
    fn active_weapon_attachment_uses_hand_socket_without_first_person_placement() {
        let mut descriptors = vec![
            player_mesh_descriptor("co_op_avatar"),
            third_person_weapon_descriptor("reference_pistol", "models/pistol/model.gltf"),
            third_person_weapon_descriptor("missing_pistol", "models/missing/model.gltf"),
        ];
        let descriptors_without_placement = descriptors.clone();
        descriptors[1].weapon.as_mut().unwrap().placement =
            Some(postretro_foundation::WeaponPlacementDescriptor {
                offset: postretro_foundation::PlacementOffset {
                    right: 0.5,
                    up: -0.4,
                    forward: 0.8,
                },
                rotation: postretro_foundation::PlacementRotation {
                    yaw: 20.0,
                    pitch: -10.0,
                    roll: 5.0,
                },
            });
        let store = attachment_store();
        let mut baseline_registry = EntityRegistry::new();
        let baseline_pawn = spawn_transform_only(&mut baseline_registry);
        let baseline_request = RemoteEntityMaterialize {
            network_id: postretro_net::wire::NetworkId(9),
            entity_id: baseline_pawn,
            entity_class: "co_op_avatar".to_string(),
            initial_animation_state: None,
            active_weapon_archetype: None,
            weapon_attachment_changed: false,
        };

        assert!(materialize_armed_remote_player(
            &baseline_request,
            &descriptors_without_placement,
            &mut baseline_registry,
            None,
        ));
        assert!(update_active_weapon_attachment(
            &mut baseline_registry,
            baseline_pawn,
            &descriptors_without_placement,
            Some("reference_pistol"),
            &store,
        ));
        crate::resolve_mesh_entity_bindings_for_entities(
            &mut baseline_registry,
            &crate::scripting_systems::mesh_anim::MeshClipTables::default(),
            &store,
            [baseline_pawn],
        );
        let baseline_transform = resolved_attachment_transform(
            baseline_registry
                .get_component::<MeshComponent>(baseline_pawn)
                .unwrap(),
            &store,
        );

        let mut registry = EntityRegistry::new();
        let pawn = spawn_transform_only(&mut registry);
        let request = RemoteEntityMaterialize {
            network_id: postretro_net::wire::NetworkId(9),
            entity_id: pawn,
            entity_class: "co_op_avatar".to_string(),
            initial_animation_state: None,
            active_weapon_archetype: None,
            weapon_attachment_changed: false,
        };
        assert!(materialize_armed_remote_player(
            &request,
            &descriptors,
            &mut registry,
            None,
        ));
        assert!(update_active_weapon_attachment(
            &mut registry,
            pawn,
            &descriptors,
            Some("reference_pistol"),
            &store,
        ));
        crate::resolve_mesh_entity_bindings_for_entities(
            &mut registry,
            &crate::scripting_systems::mesh_anim::MeshClipTables::default(),
            &store,
            [pawn],
        );
        let mesh = registry.get_component::<MeshComponent>(pawn).unwrap();
        assert_eq!(mesh.attachments.len(), 1);
        assert_eq!(mesh.attachments[0].socket, ACTIVE_WEAPON_SOCKET);
        assert_eq!(mesh.attachments[0].model, "models/pistol/model.gltf");
        assert_eq!(mesh.attachments[0].binding, AttachmentBinding::Skinned(3));
        let authored_transform = resolved_attachment_transform(mesh, &store);
        assert_mat4_approx(
            authored_transform,
            baseline_transform,
            "third-person hand attachment ignores first-person placement",
        );

        assert!(update_active_weapon_attachment(
            &mut registry,
            pawn,
            &descriptors,
            Some("missing_pistol"),
            &store,
        ));
        assert!(
            registry
                .get_component::<MeshComponent>(pawn)
                .unwrap()
                .attachments
                .is_empty(),
            "a descriptor model absent from the preloaded CPU store clears the old prop"
        );
    }

    #[test]
    fn materialize_armed_local_pawn_attaches_descriptor_mesh_with_movement() {
        let descriptors = vec![player_mesh_descriptor("co_op_avatar")];
        let mut reg = EntityRegistry::new();
        let id = spawn_transform_only(&mut reg);

        materialize_armed_local_pawn(
            &ArmedLocalPawn {
                network_id: postretro_net::wire::NetworkId(10),
                entity_id: id,
                entity_class: Some("descriptor:co_op_avatar".to_string()),
            },
            &descriptors,
            &mut reg,
            descriptors[0].movement.as_ref(),
            None,
            false,
        );

        assert!(
            reg.has_component_kind(id, ComponentKind::PlayerMovement)
                .unwrap()
        );
        assert!(reg.get_component::<MeshComponent>(id).unwrap().shadow_only);
    }

    // Regression: a host retune from movement Some to None left the previously
    // materialized component active on the client.
    #[test]
    fn materialize_armed_local_pawn_clears_stale_movement_when_host_tuning_is_none() {
        let descriptors = vec![player_mesh_descriptor("co_op_avatar")];
        let mut reg = EntityRegistry::new();
        let id = spawn_transform_only(&mut reg);
        let armed = ArmedLocalPawn {
            network_id: postretro_net::wire::NetworkId(10),
            entity_id: id,
            entity_class: Some("co_op_avatar".to_string()),
        };

        materialize_armed_local_pawn(
            &armed,
            &descriptors,
            &mut reg,
            descriptors[0].movement.as_ref(),
            None,
            false,
        );
        assert!(
            reg.has_component_kind(id, ComponentKind::PlayerMovement)
                .unwrap()
        );

        materialize_armed_local_pawn(&armed, &descriptors, &mut reg, None, None, true);

        assert_eq!(
            reg.has_component_kind(id, ComponentKind::PlayerMovement),
            Ok(false)
        );
    }
}
