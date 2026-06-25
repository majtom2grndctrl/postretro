// Client-apply call-site glue: routes applied snapshots to the descriptor-presentation
// materialization seam for both local and remote entities.
// See: context/lib/networking.md

use crate::scripting::data_descriptors::EntityTypeDescriptor;
use crate::scripting::registry::EntityRegistry;

use super::client::{ArmedLocalPawn, RemoteEnemyMaterialize};

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
/// This seam also carries the remote-enemy presentation call so descriptor
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
}

/// Materialize the descriptor-backed *presentation* for a non-local remote enemy a
/// snapshot just spawned (E10 Task 6). `apply_snapshot` spawned the entity
/// Transform-only and mapped its `NetworkId` (so it joins the Phase 2 remote
/// interpolation path); the host owns its AI/damage/death and replicates only its
/// position, so the client attaches ONLY the descriptor's mesh and NONE of
/// `Brain`/`Agent`/`Health`/`Weapon`/`PlayerMovement`.
///
/// The descriptor lookup lives here, NOT in `ClientReplication::apply_snapshot`: the
/// net-facing apply is descriptor-blind, and this is where the shared descriptor table
/// is in scope. The underlying helper is idempotent (a re-apply never resets live mesh
/// animation state) and unknown-class-tolerant (an unregistered class leaves the entity
/// transform-only, logged, and never rejects the snapshot — the entity still
/// interpolates from its mapped `Transform`).
pub(super) fn materialize_armed_remote_enemy(
    remote: &RemoteEnemyMaterialize,
    descriptors: &[EntityTypeDescriptor],
    registry: &mut EntityRegistry,
) {
    crate::scripting::builtins::net_descriptor::materialize_net_remote_enemy_presentation(
        &remote.entity_class,
        descriptors,
        registry,
        remote.entity_id,
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scripting::components::mesh::{AnimationState, InterruptPolicy, MeshComponent};
    use crate::scripting::data_descriptors::MeshDescriptor;
    use crate::scripting::registry::{ComponentKind, EntityId, Transform};
    use glam::{Quat, Vec3};
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
                animations: states,
                default_state: Some("idle".to_string()),
            }),
            health: None,
            ai: None,
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
            &RemoteEnemyMaterialize {
                entity_id: id,
                entity_class: "decraniated_mob".to_string(),
            },
            &descriptors,
            &mut reg,
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
            &RemoteEnemyMaterialize {
                entity_id: id,
                entity_class: "no_such_class".to_string(),
            },
            &descriptors,
            &mut reg,
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
        let request = RemoteEnemyMaterialize {
            entity_id: id,
            entity_class: "decraniated_mob".to_string(),
        };

        materialize_armed_remote_enemy(&request, &descriptors, &mut reg);

        // Drive the live animation state forward so a reset would be observable.
        {
            let mut mesh = reg.get_component::<MeshComponent>(id).unwrap().clone();
            mesh.animation.as_mut().unwrap().current_state = "moved".to_string();
            reg.set_component(id, mesh).unwrap();
        }

        materialize_armed_remote_enemy(&request, &descriptors, &mut reg);

        let mesh = reg.get_component::<MeshComponent>(id).unwrap();
        assert_eq!(
            mesh.animation.as_ref().unwrap().current_state,
            "moved",
            "a second materialize must not reset live animation state"
        );
    }
}
