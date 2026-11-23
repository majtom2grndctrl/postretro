// Shared descriptor-presentation materialization for local and remote player
// entities.
// See: context/lib/networking.md

use postretro_combat_model::TuningPayload;
use postretro_entities::components::mesh::{BodyFrame, MeshComponent};
use postretro_entities::provenance::{DescriptorProvenance, DescriptorSpawnPath};
use postretro_entities::{EntityId, EntityRegistry, EntityTypeDescriptor};
use postretro_foundation::NavAgentParams;

use super::client::{ArmedLocalPawn, RemoteEntityMaterialize};
use super::descriptor_class::decode_replicated_descriptor_class;

/// Whether a descriptor-backed player body is being presented to its owning
/// viewer or to another client. `shadowOnly` is authored for the first-person
/// owner view; peers must still see that same descriptor as a forward avatar.
/// On a client either role marks the body [`BodyFrame::View`]: a client reaches
/// a role only for a player pawn, whose transform stays in the view frame, so
/// its model front takes the half turn at the body seam. On the host the frame
/// is fixed when the pawn's transform spawns (`descriptor_body_frame` in sim's
/// `data_archetype.rs`), and the role step leaves it alone.
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
    body_frame: Option<BodyFrame>,
) {
    let Some(descriptor) = descriptors
        .iter()
        .find(|descriptor| descriptor.canonical_name.as_deref() == Some(entity_class))
    else {
        return;
    };
    let Some(descriptor_shadow_only) = descriptor.mesh.as_ref().map(|mesh| mesh.shadow_only) else {
        return;
    };
    let Ok(mut mesh) = registry.get_component::<MeshComponent>(id).cloned() else {
        return;
    };
    let shadow_only = role.shadow_only(descriptor_shadow_only);
    // `None` keeps the frame the spawn chose. The frame must agree with the
    // frame the transform spawned in, and a re-accept (or a hot reload that
    // adds or removes `movement`) must not re-derive it from descriptors.
    // Set on every call, not only when the mesh was just built: a re-arm or
    // re-apply keeps an existing mesh, so the frame must not wait on a rebuild.
    let body_frame = body_frame.unwrap_or(mesh.body_frame);
    if mesh.shadow_only != shadow_only || mesh.body_frame != body_frame {
        mesh.shadow_only = shadow_only;
        mesh.body_frame = body_frame;
        let _ = registry.set_component(id, mesh);
    }
}

/// Apply the peer-view policy to a descriptor-backed player mesh that another
/// host-side spawn path already materialized. Listen-host slot pawns use the full
/// authoritative descriptor spawn, so they need this presentation-only override
/// without re-materializing any gameplay components. That spawn already fixed
/// the body frame with the transform (a movement pawn [`BodyFrame::View`], a
/// slot body without movement [`BodyFrame::Model`]); this step keeps it.
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
        None,
    );
}

/// Materialize the descriptor-backed presentation for a `local_player` baseline this
/// snapshot armed. `apply_snapshot` spawned the pawn
/// Transform-only; host movement tuning arrives independently on Control, so this
/// materializes or clears the `PlayerMovementComponent`, and composes the wieldable
/// inventory from the payload alone, when both halves meet.
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
    host_tuning: Option<&TuningPayload>,
    rebuild_movement: bool,
    life_state_tables: &crate::life_state_tables::LifeStateTables,
) -> bool {
    let entity_class = armed.entity_class.as_deref().unwrap_or("player");
    let entity_class = decode_replicated_descriptor_class(entity_class).canonical_name();
    let had_mesh = registry
        .has_component_kind(armed.entity_id, postretro_entities::ComponentKind::Mesh)
        .unwrap_or(false);
    if let Some(host_movement) = host_movement {
        // View feel and movement sounds are deliberately local presentation.
        // The host payload carries them as null, so recover only these fields
        // from the local descriptor.
        let mut movement = host_movement.clone();
        let local = descriptors
            .iter()
            .find(|descriptor| descriptor.canonical_name.as_deref() == Some(entity_class))
            .and_then(|descriptor| descriptor.movement.as_ref());
        movement.view_feel = local.and_then(|descriptor| descriptor.view_feel.clone());
        movement.sounds = local.and_then(|descriptor| descriptor.sounds.clone());
        // A rebuild changes descriptor-owned tuning only (a weapon pickup, a host
        // reload): the live tick state carries onto the new component, as the
        // host's own pawn keeps it, so prediction neither resets the pawn nor
        // re-takes a swim edge before the next reconcile.
        let live = rebuild_movement
            .then(|| {
                registry
                    .get_component::<postretro_foundation::PlayerMovementComponent>(armed.entity_id)
                    .ok()
                    .cloned()
            })
            .flatten();
        crate::scripting::builtins::net_descriptor::materialize_net_local_movement_component_from_tuning(
            &movement,
            registry,
            armed.entity_id,
            rebuild_movement,
        );
        if let Some(live) = live
            && let Ok(rebuilt) = registry
                .get_component::<postretro_foundation::PlayerMovementComponent>(armed.entity_id)
        {
            let mut rebuilt = rebuilt.clone();
            super::movement_state::carry_live_tick_state(&mut rebuilt, &live);
            life_state_tables.restamp_carried(&mut rebuilt);
            let _ = registry.set_component(armed.entity_id, rebuilt);
        }
    } else {
        // Every owner record re-arms the pawn, and a still pawn's arrive every
        // snapshot, so only the install that left it inert warns.
        if rebuild_movement {
            let _ = registry
                .remove_component::<postretro_foundation::PlayerMovementComponent>(armed.entity_id);
            log::warn!(
                "[Net] local pawn has no host movement tuning; movement prediction stays inert"
            );
        } else {
            log::debug!(
                "[Net] local pawn awaits host movement tuning; movement prediction stays inert"
            );
        }
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
        Some(BodyFrame::View),
    );
    !had_mesh
        && registry
            .has_component_kind(armed.entity_id, postretro_entities::ComponentKind::Mesh)
            .unwrap_or(false)
}

/// Materialize a remote descriptor-backed player as presentation only. The player
/// identity is selected by the caller from the host-stamped pawn class;
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
            Some(BodyFrame::View),
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
    crate::scripting::builtins::net_descriptor::materialize_net_descriptor_presentation(
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
    facts: Option<postretro_net::wire::WireProjectilePresentation>,
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
    if let Some(facts) = facts {
        super::projectile_presentation::apply_projectile_presentation_facts(
            registry,
            remote.entity_id,
            facts,
        );
    }
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
    use crate::sprite_collection::derive_collection_id;
    use glam::{Quat, Vec3};
    use postretro_entities::components::mesh::{AnimationState, InterruptPolicy, MeshComponent};
    use postretro_entities::{ComponentKind, EntityId, MeshDescriptor, Transform};
    use postretro_foundation::{
        AirParams, CapsuleParams, FallParams, GroundParams, NavAgentParams,
        PlayerMovementDescriptor, ProjectileBodyVisual, ProjectileDescriptor,
        ProjectileTrailSpinAnimation, ProjectileTrailVisual, ProjectileVisual, ResolutionMode,
        SpeedParams, WeaponDescriptor,
    };
    use std::collections::HashMap;

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
                max_playback_rate: None,
                scale_with_speed: true,
                playback_rate: None,
                clip_index: None,
            },
        );
        EntityTypeDescriptor {
            life_states: None,
            ghost: false,
            faction: None,
            tolerance: None,
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
                weapon_mounts: None,
            }),
            health: None,
            behavior: None,
        }
    }

    fn player_mesh_descriptor(classname: &str) -> EntityTypeDescriptor {
        let mut descriptor = enemy_mesh_descriptor(classname);
        descriptor.mesh.as_mut().unwrap().shadow_only = true;
        descriptor.movement = Some(PlayerMovementDescriptor {
            sounds: None,
            knockback: Default::default(),
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
                friction: GroundParams::DEFAULT_FRICTION,
                stop_speed: GroundParams::DEFAULT_STOP_SPEED,
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
            swim: None,
            vault: None,
            view_feel: None,
        });
        descriptor
    }

    fn projectile_weapon_descriptor(classname: &str) -> EntityTypeDescriptor {
        let mut descriptor = enemy_mesh_descriptor(classname);
        descriptor.mesh = None;
        descriptor.weapon = Some(WeaponDescriptor {
            sounds: None,
            knockback: None,
            damage: 1.0,
            pellet_count: 1,
            spread_degrees: 0.0,
            bloom_per_shot_degrees: 0.0,
            bloom_max_degrees: 0.0,
            bloom_decay_degrees_per_second: 0.0,
            bloom_decay_delay_ms: 0.0,
            movement_spread_degrees: 0.0,
            spread_vertical_bias: 0.0,
            range: 16.0,
            primary: postretro_foundation::WeaponActivationDescriptor::single(
                postretro_foundation::ActivationTrigger::Press,
                1.0,
            ),
            secondary: None,
            resolution: ResolutionMode::Projectile,
            projectile: Some(ProjectileDescriptor {
                ghost: false,
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
            splash: None,
            credit_source: None,
            third_person_model: None,
            viewmodel: None,
            placement: None,
            muzzle_offset: None,
            resource: None,
            lower_ms: 0,
            raise_ms: 0,
            block_during_reload: None,
            damage_type: None,
        });
        descriptor
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
    // table and attaches its display components without authoritative AI state.
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
            Some(postretro_net::wire::WireProjectilePresentation {
                sprite_size: Some(2.0),
                model_scale: 2.0,
                shot_id: Some(postretro_net::wire::WireShotId {
                    pawn: 7,
                    start_tick: 10,
                    lane: 1,
                    ordinal: 0
                }),
            }),
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
                .collection,
            match &descriptors[0]
                .weapon
                .as_ref()
                .expect("fixture weapon has a projectile")
                .projectile
                .as_ref()
                .expect("fixture weapon uses a projectile")
                .visual
                .body
            {
                ProjectileBodyVisual::Sprite {
                    sprite,
                    emissive,
                    frame_duration_ms,
                    ..
                } => derive_collection_id(sprite, None, *frame_duration_ms, *emissive, None, None,),
                ProjectileBodyVisual::Model { .. } =>
                    panic!("fixture projectile uses a sprite body"),
            }
        );
        assert!(
            (reg.get_component::<postretro_entities::components::sprite_visual::SpriteVisual>(id)
                .unwrap()
                .size
                - 2.0)
                .abs()
                < 1.0e-6
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
    fn materialize_armed_local_pawn_attaches_descriptor_mesh_with_movement() {
        let descriptors = vec![player_mesh_descriptor("co_op_avatar")];
        let mut reg = EntityRegistry::new();
        let id = spawn_transform_only(&mut reg);

        materialize_armed_local_pawn(
            &ArmedLocalPawn {
                network_id: postretro_net::wire::NetworkId(10),
                entity_id: id,
                entity_class: Some("pawn:co_op_avatar".to_string()),
            },
            &descriptors,
            &mut reg,
            descriptors[0].movement.as_ref(),
            None,
            false,
            &Default::default(),
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
            &Default::default(),
        );
        assert!(
            reg.has_component_kind(id, ComponentKind::PlayerMovement)
                .unwrap()
        );

        materialize_armed_local_pawn(
            &armed,
            &descriptors,
            &mut reg,
            None,
            None,
            true,
            &Default::default(),
        );

        assert_eq!(
            reg.has_component_kind(id, ComponentKind::PlayerMovement),
            Ok(false)
        );
    }

    /// A snapshot-spawned entity whose transform rotation is `rotation`.
    fn spawn_rotated(reg: &mut EntityRegistry, rotation: Quat) -> EntityId {
        let id = spawn_transform_only(reg);
        reg.set_component(
            id,
            Transform {
                position: Vec3::new(1.0, 2.0, 3.0),
                rotation,
                scale: Vec3::ONE,
            },
        )
        .unwrap();
        id
    }

    fn remote_request(
        entity_id: EntityId,
        network_id: u32,
        class: &str,
    ) -> RemoteEntityMaterialize {
        RemoteEntityMaterialize {
            network_id: postretro_net::wire::NetworkId(network_id),
            entity_id,
            entity_class: class.to_string(),
            initial_animation_state: None,
            active_weapon_archetype: None,
            weapon_attachment_changed: false,
        }
    }

    // The client roles. The host tags both player bodies `pawn:`, and each
    // viewer-role step marks the body the view frame: the client's own pawn and
    // a remote avatar front model +Z along the view, along the aim yaw until a
    // lower-body heading lands, then along it. A world copy of the same player
    // descriptor arrives tagged `descriptor:` with a model-frame transform and
    // draws as placed (pin R3). The frame comes from the local materialization;
    // nothing on the wire carries it.
    #[test]
    fn client_player_bodies_front_model_plus_z_along_the_view_and_a_world_copy_stays_model() {
        use postretro_entities::PoseInputs;
        use postretro_entities::components::mesh::{BodyFrame, MODEL_FRONT_TO_VIEW_FORWARD};

        let descriptors = vec![player_mesh_descriptor("co_op_avatar")];
        let (aim, heading) = (1.3_f32, 0.4_f32);
        let mut reg = EntityRegistry::new();

        let own = spawn_rotated(&mut reg, Quat::from_rotation_y(aim));
        materialize_armed_local_pawn(
            &ArmedLocalPawn {
                network_id: postretro_net::wire::NetworkId(10),
                entity_id: own,
                entity_class: Some("pawn:co_op_avatar".to_string()),
            },
            &descriptors,
            &mut reg,
            descriptors[0].movement.as_ref(),
            None,
            false,
            &Default::default(),
        );
        let avatar = spawn_rotated(&mut reg, Quat::from_rotation_y(aim));
        assert!(materialize_armed_remote_player(
            &remote_request(avatar, 11, "pawn:co_op_avatar"),
            &descriptors,
            &mut reg,
            None,
        ));

        for (pawn, role) in [(own, "client own pawn"), (avatar, "client remote avatar")] {
            let rotation = reg.get_component::<Transform>(pawn).unwrap().rotation;
            let mut mesh = reg.get_component::<MeshComponent>(pawn).unwrap().clone();
            assert_eq!(mesh.body_frame, BodyFrame::View, "{role}");
            assert!(
                (mesh.presentation_rotation(rotation) * Vec3::Z)
                    .abs_diff_eq(Quat::from_rotation_y(aim) * Vec3::NEG_Z, 1e-6),
                "{role}: model +Z along Ry(aim)*(-Z) on its first drawn frame"
            );
            mesh.pose_inputs = Some(PoseInputs {
                aim_yaw: aim,
                heading_yaw: heading,
                ..Default::default()
            });
            assert!(
                (mesh.presentation_rotation(rotation) * Vec3::Z)
                    .abs_diff_eq(Quat::from_rotation_y(heading) * Vec3::NEG_Z, 1e-6),
                "{role}: model +Z along Ry(heading)*(-Z)"
            );
        }

        let placed = Quat::from_rotation_y(aim) * MODEL_FRONT_TO_VIEW_FORWARD;
        let copy = spawn_rotated(&mut reg, placed);
        assert!(materialize_armed_remote_enemy(
            &remote_request(copy, 12, "descriptor:co_op_avatar"),
            &descriptors,
            &mut reg,
            None,
        ));
        let copy_mesh = reg.get_component::<MeshComponent>(copy).unwrap();
        assert_eq!(copy_mesh.body_frame, BodyFrame::Model);
        assert_eq!(
            copy_mesh.presentation_rotation(placed),
            placed,
            "a world copy of a player descriptor draws as placed, unturned"
        );
    }

    // A re-arm or re-apply keeps a mesh that already exists (the mesh helper
    // returns early), so the viewer-role step marks the body frame on every
    // call, never only when it builds the mesh. A rejoin's re-arm, or a body
    // some earlier step materialized, still draws turned and keeps its live
    // animation state.
    #[test]
    fn client_viewer_role_marks_an_existing_player_mesh_view_without_rebuilding_it() {
        use postretro_entities::components::mesh::BodyFrame;

        let descriptors = vec![player_mesh_descriptor("co_op_avatar")];
        let mut reg = EntityRegistry::new();
        let own = spawn_transform_only(&mut reg);
        let avatar = spawn_transform_only(&mut reg);
        for id in [own, avatar] {
            assert!(
                crate::scripting::builtins::net_descriptor::materialize_net_mesh_presentation(
                    "co_op_avatar",
                    &descriptors,
                    &mut reg,
                    id,
                    None,
                )
            );
            let mut mesh = reg.get_component::<MeshComponent>(id).unwrap().clone();
            assert_eq!(
                mesh.body_frame,
                BodyFrame::Model,
                "fixture: the shared descriptor mesh carries no frame"
            );
            mesh.animation.as_mut().unwrap().current_state = "moved".to_string();
            reg.set_component(id, mesh).unwrap();
        }

        let armed = ArmedLocalPawn {
            network_id: postretro_net::wire::NetworkId(10),
            entity_id: own,
            entity_class: Some("pawn:co_op_avatar".to_string()),
        };
        for rebuild_movement in [false, true] {
            materialize_armed_local_pawn(
                &armed,
                &descriptors,
                &mut reg,
                descriptors[0].movement.as_ref(),
                None,
                rebuild_movement,
                &Default::default(),
            );
        }
        assert!(materialize_armed_remote_player(
            &remote_request(avatar, 11, "pawn:co_op_avatar"),
            &descriptors,
            &mut reg,
            None,
        ));

        for (id, role) in [(own, "own pawn"), (avatar, "remote avatar")] {
            let mesh = reg.get_component::<MeshComponent>(id).unwrap();
            assert_eq!(mesh.body_frame, BodyFrame::View, "{role}");
            assert_eq!(
                mesh.animation.as_ref().unwrap().current_state,
                "moved",
                "{role}: the live mesh was kept, not rebuilt"
            );
        }
    }

    // The host's role step keeps the frame the slot body's transform spawned
    // in. A re-accept after a hot reload that removed or added `movement`
    // must not re-derive it from the current descriptors, or the body would
    // draw half a turn from its transform.
    #[test]
    fn host_viewer_role_keeps_the_spawned_body_frame_whatever_the_descriptor_says_now() {
        use postretro_entities::components::mesh::BodyFrame;

        let with_movement = vec![player_mesh_descriptor("avatar")];
        let mut without_movement = with_movement.clone();
        without_movement[0].movement = None;
        for (spawned, descriptors) in [
            (BodyFrame::View, &without_movement),
            (BodyFrame::Model, &with_movement),
        ] {
            let mut reg = EntityRegistry::new();
            let id = spawn_transform_only(&mut reg);
            assert!(
                crate::scripting::builtins::net_descriptor::materialize_net_mesh_presentation(
                    "avatar",
                    descriptors,
                    &mut reg,
                    id,
                    None,
                )
            );
            let mut mesh = reg.get_component::<MeshComponent>(id).unwrap().clone();
            mesh.body_frame = spawned;
            reg.set_component(id, mesh).unwrap();

            apply_remote_player_viewer_role("avatar", descriptors, &mut reg, id);

            let mesh = reg.get_component::<MeshComponent>(id).unwrap();
            assert_eq!(mesh.body_frame, spawned, "the spawn's frame stands");
            assert!(!mesh.shadow_only, "a remote viewer still draws the body");
        }
    }

    // A remote enemy's mesh derives its zero-phase loops from the
    // shared descriptor's graph, as the host's does: the client draws a
    // repeating release's state from clip time 0 on entry, while its other
    // loops keep the drawn phase. Nothing about it crosses the wire.
    #[test]
    fn a_remote_enemy_starts_a_repeating_releases_loop_at_phase_zero() {
        let mut descriptor = enemy_mesh_descriptor("brute");
        let mesh = descriptor.mesh.as_mut().unwrap();
        let mut jab = mesh.animations["idle"].clone();
        jab.clip = "jab_quick".to_string();
        mesh.animations.insert("jab_quick".to_string(), jab);
        descriptor.behavior = Some(
            crate::scripting::builtins::data_archetype_test_fixtures::repeating_release_graph(
                "jab_quick",
            ),
        );
        let descriptors = vec![descriptor];
        let mut reg = EntityRegistry::new();
        let id = spawn_transform_only(&mut reg);
        materialize_armed_remote_enemy(
            &RemoteEntityMaterialize {
                network_id: postretro_net::wire::NetworkId(9),
                entity_id: id,
                entity_class: "brute".to_string(),
                initial_animation_state: Some("jab_quick".to_string()),
                active_weapon_archetype: None,
                weapon_attachment_changed: false,
            },
            &descriptors,
            &mut reg,
            None,
        );
        let animation = reg
            .get_component::<MeshComponent>(id)
            .expect("the remote enemy renders its mesh")
            .animation
            .clone()
            .expect("animated");
        assert_eq!(animation.loop_phase("jab_quick", 0.4), 0.0);
        assert_eq!(animation.loop_phase("idle", 0.4), 0.4);
    }
}
