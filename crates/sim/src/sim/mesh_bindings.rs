//! Game-side mesh clip and socket binding resolution.

use crate::scripting_systems;
use crate::scripting_systems::hit_zones::HitZoneStore;
use postretro_entities::components::mesh::{AttachmentBinding, MeshComponent};
use postretro_entities::{ComponentKind, ComponentValue, EntityId, EntityRegistry};
use postretro_model::ModelHandle;
use postretro_model::gltf_loader::SocketBinding;

/// Resolve every animated mesh entity's declared state map against the level's
/// clip tables, filling each `AnimationState.clip_index` (name → glTF index),
/// and resolve descriptor-authored attachment sockets from the game-side loaded
/// model table. Clip resolution remains animation-gated; attachment resolution
/// deliberately also visits stateless and rigid holders.
///
/// Runs at level load with a mutable registry, after the model sweep built the
/// clip tables — so every state's index is concrete before the first frame.
pub fn resolve_mesh_entity_bindings(
    registry: &mut EntityRegistry,
    tables: &scripting_systems::mesh_anim::MeshClipTables,
    hit_zone_store: &HitZoneStore,
) {
    // Collect ids first so the mutable per-entity writes do not alias the
    // immutable iteration borrow. Mesh entity counts are small.
    let needing_resolution: Vec<EntityId> = registry
        .iter_with_kind(ComponentKind::Mesh)
        .filter_map(|(id, value)| match value {
            ComponentValue::Mesh(mesh)
                if mesh.animation.is_some() || !mesh.attachments.is_empty() =>
            {
                Some(id)
            }
            _ => None,
        })
        .collect();

    resolve_mesh_entity_bindings_for_entities(registry, tables, hit_zone_store, needing_resolution);
}

/// Resolve clip indices and attachment bindings for a known set of newly
/// materialized mesh entities. Runtime spawners call this through their
/// session-owned pending-id queue after descriptor attachment; their models were
/// already uploaded at level install.
pub fn resolve_mesh_entity_bindings_for_entities(
    registry: &mut EntityRegistry,
    tables: &scripting_systems::mesh_anim::MeshClipTables,
    hit_zone_store: &HitZoneStore,
    entity_ids: impl IntoIterator<Item = EntityId>,
) {
    for id in entity_ids {
        if !matches!(
            registry.has_component_kind(id, ComponentKind::Mesh),
            Ok(true)
        ) {
            continue;
        }
        let Ok(mut component) = registry.get_component::<MeshComponent>(id).cloned() else {
            continue;
        };
        let model_name = component.model.clone();
        let handle = ModelHandle::from(model_name.clone());
        if let Some(anim) = component.animation.as_mut() {
            match tables.get(&handle) {
                Some(table) => {
                    let missing =
                        scripting_systems::mesh_anim::resolve_state_clips(&mut anim.states, table);
                    for m in &missing {
                        log::warn!(
                            "[Model] animation state '{}' on model '{}' names clip '{}' absent from \
                             the model — state unusable (switching to it no-ops)",
                            m.state,
                            model_name,
                            m.clip,
                        );
                    }
                }
                None => {
                    log::warn!(
                        "[Model] mesh entity references uncached model '{}' — animation states \
                         unresolved",
                        model_name,
                    );
                    for state in anim.states.values_mut() {
                        state.clip_index = None;
                    }
                }
            }
        }

        for attachment in &mut component.attachments {
            if hit_zone_store.get_by_name(&attachment.model).is_none() {
                attachment.binding = AttachmentBinding::Unresolved;
                continue;
            }

            let binding = hit_zone_store
                .get(&handle)
                .and_then(|holder| holder.sockets.get(&attachment.socket));
            match binding {
                Some(SocketBinding::SkinnedJoint(joint)) => {
                    attachment.binding = AttachmentBinding::Skinned(*joint);
                }
                Some(SocketBinding::RigidRest(rest)) => {
                    attachment.binding = AttachmentBinding::Rigid(*rest);
                }
                None => {
                    attachment.binding = AttachmentBinding::Unresolved;
                    let warning_key = format!(
                        "attachment-socket:{model_name}:{}:{}",
                        attachment.socket, attachment.model
                    );
                    if hit_zone_store.mark_attachment_resolution_warning(warning_key) {
                        log::warn!(
                            "[Model] holder model '{}' has no socket '{}' for attachment model '{}' — attachment unresolved",
                            model_name,
                            attachment.socket,
                            attachment.model,
                        );
                    }
                }
            }
        }
        let _ = registry.set_component(id, component);
    }
}
