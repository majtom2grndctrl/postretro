// Client-apply call-site glue: routes applied snapshots to the descriptor-presentation
// materialization seam for both local and remote entities.
// See: context/lib/networking.md

use postretro_entities::{EntityRegistry, EntityTypeDescriptor};
use postretro_foundation::NavAgentParams;

use super::client::{ArmedLocalPawn, RemoteEntityMaterialize};

/// Materialize the descriptor-backed presentation for a `local_player` baseline this
/// snapshot armed (M15 Phase 3 Task 3 + Task 7). `apply_snapshot` spawned the pawn
/// Transform-only; the descriptor-immutable movement tuning never crosses the wire, so
/// the client materializes the matching `PlayerMovementComponent` locally from the same
/// descriptor table both peers share — then the wire mutable subset has a component to
/// merge onto and prediction/reconciliation light up.
///
/// Defaults to the `"player"` class when the host stamped none (defensive). Must run
/// BEFORE reconcile (which merges onto the existing component); the underlying helper is
/// idempotent, so a re-arm of the same pawn keeps its live state.
///
/// This seam also carries remote presentation calls so descriptor
/// materialization for replicated entities lives in one focused place, off the
/// `client_receive_and_apply` hot path.
pub(super) fn materialize_armed_local_pawn(
    armed: &ArmedLocalPawn,
    descriptors: &[EntityTypeDescriptor],
    registry: &mut EntityRegistry,
) {
    let entity_class = armed.entity_class.as_deref().unwrap_or("player");
    crate::scripting::builtins::net_descriptor::materialize_net_local_movement_component(
        entity_class,
        descriptors,
        registry,
        armed.entity_id,
    );
    crate::scripting::builtins::net_descriptor::materialize_net_mesh_presentation(
        entity_class,
        descriptors,
        registry,
        armed.entity_id,
        None,
    );
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
    crate::scripting::builtins::net_descriptor::materialize_net_mesh_presentation(
        &remote.entity_class,
        descriptors,
        registry,
        remote.entity_id,
        agent_params,
    )
}

/// Materialize the descriptor-backed *presentation* for a non-local remote enemy a
/// snapshot just spawned (E10 Task 6). `apply_snapshot` spawned the entity
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
    crate::scripting::builtins::net_descriptor::materialize_net_mesh_presentation(
        &remote.entity_class,
        descriptors,
        registry,
        remote.entity_id,
        agent_params,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use glam::{Quat, Vec3};
    use postretro_entities::components::mesh::{AnimationState, InterruptPolicy, MeshComponent};
    use postretro_entities::{ComponentKind, EntityId, MeshDescriptor, Transform};
    use postretro_foundation::{
        AirParams, CapsuleParams, FallParams, GroundParams, NavAgentParams,
        PlayerMovementDescriptor, SpeedParams,
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
                clip_index: None,
            },
        );
        EntityTypeDescriptor {
            canonical_name: Some(classname.to_string()),
            default_weapon: None,
            light: None,
            emitter: None,
            movement: None,
            weapon: None,
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
            ai: None,
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
            view_feel: None,
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
                entity_class: "decraniated_mob".to_string(),
                initial_animation_state: None,
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
    fn materialize_armed_remote_player_attaches_shadow_only_mesh_without_authority() {
        let descriptors = vec![player_mesh_descriptor("co_op_avatar")];
        let mut reg = EntityRegistry::new();
        let id = spawn_transform_only(&mut reg);
        let request = RemoteEntityMaterialize {
            network_id: postretro_net::wire::NetworkId(9),
            entity_id: id,
            entity_class: "co_op_avatar".to_string(),
            initial_animation_state: None,
        };

        assert!(materialize_armed_remote_player(
            &request,
            &descriptors,
            &mut reg,
            None,
        ));
        assert!(reg.get_component::<MeshComponent>(id).unwrap().shadow_only);
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

        let unknown_id = spawn_transform_only(&mut reg);
        assert!(!materialize_armed_remote_player(
            &RemoteEntityMaterialize {
                network_id: postretro_net::wire::NetworkId(12),
                entity_id: unknown_id,
                entity_class: "not_a_class".to_string(),
                initial_animation_state: None,
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
                entity_class: Some("co_op_avatar".to_string()),
            },
            &descriptors,
            &mut reg,
        );

        assert!(
            reg.has_component_kind(id, ComponentKind::PlayerMovement)
                .unwrap()
        );
        assert!(reg.get_component::<MeshComponent>(id).unwrap().shadow_only);
    }
}
