// Built-in classname handler for `prop_mesh` map entities.
// See: context/lib/build_pipeline.md §Built-in Classname Routing
//
// `prop_mesh` is the data-driven spawn path for skinned models: a map entity
// with `classname "prop_mesh"` and a `model` key spawns one ECS entity carrying
// a `MeshComponent`. The level-load model sweep in `main.rs` later collects the
// distinct `model` strings off the spawned entities and uploads each once into
// the renderer cache (renderer owns GPU). This handler is GPU-free.

use glam::Vec3;

use super::MapEntity;
use crate::scripting::components::mesh::MeshComponent;
use crate::scripting::registry::{EntityId, EntityRegistry, Transform};

/// FGD `classname` this handler binds to.
pub(crate) const CLASSNAME: &str = "prop_mesh";

/// Read the `model` KVP. Absent or empty → logs a `[Loader]` warning naming the
/// key and origin, and returns `None`. The handler treats a missing model as a
/// log-and-continue condition (mirrors `billboard_emitter`'s log-and-fallback
/// policy): the entity still spawns, but with an empty `model` so the load is
/// never aborted. An empty handle simply renders nothing.
fn kvp_model<'a>(entity: &'a MapEntity, key: &str) -> Option<&'a str> {
    let raw = entity.key_values.get(key)?;
    if raw.is_empty() {
        log::warn!(
            "[Loader] {origin}: key `{key}` is empty; prop_mesh will render nothing",
            origin = entity.diagnostic_origin(),
        );
        return None;
    }
    Some(raw.as_str())
}

/// Resolve the `model` handle for a map entity, warning and falling back to an
/// empty string when the key is absent or empty.
fn model_from_entity(entity: &MapEntity) -> String {
    match kvp_model(entity, "model") {
        Some(model) => model.to_string(),
        None => {
            if !entity.key_values.contains_key("model") {
                log::warn!(
                    "[Loader] {origin}: required key `model` is absent; prop_mesh will render nothing",
                    origin = entity.diagnostic_origin(),
                );
            }
            String::new()
        }
    }
}

/// Spawn an ECS entity carrying a `MeshComponent` configured from the map
/// entity's `model` KVP. Returns `None` only when the registry is exhausted —
/// an absent/invalid `model` logs a warning and still spawns (rendering
/// nothing), so the level load is never aborted.
pub(crate) fn handle(entity: &MapEntity, registry: &mut EntityRegistry) -> Option<EntityId> {
    let model = model_from_entity(entity);

    let transform = Transform {
        position: entity.origin,
        rotation: entity.rotation_quat(),
        scale: Vec3::ONE,
    };
    let id = registry.try_spawn(transform, &entity.tags).or_else(|| {
        log::warn!(
            "[Loader] {origin}: entity registry exhausted; dropping prop_mesh",
            origin = entity.diagnostic_origin(),
        );
        None
    })?;

    // `set_component` only fails on a stale id — the id was just returned by
    // `try_spawn` so it must be live.
    let _ = registry.set_component(id, MeshComponent { model });
    // Tags are attached via `try_spawn`; per-placement KVP mirroring is
    // performed uniformly by `apply_classname_dispatch` after this handler
    // returns. Built-in handlers need not call `set_tags` or `set_map_kvps`
    // themselves.
    Some(id)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    fn entity_with_kvps(pairs: &[(&str, &str)]) -> MapEntity {
        let mut kv = HashMap::new();
        for (k, v) in pairs {
            kv.insert((*k).to_string(), (*v).to_string());
        }
        MapEntity {
            classname: CLASSNAME.to_string(),
            origin: Vec3::new(1.0, 2.0, 3.0),
            angles: Vec3::ZERO,
            key_values: kv,
            tags: vec![],
        }
    }

    #[test]
    fn model_kvp_lands_on_component() {
        let mut reg = EntityRegistry::new();
        let entity = entity_with_kvps(&[("model", "models/decraniated/scene.gltf")]);

        let id = handle(&entity, &mut reg).expect("spawn should succeed");
        let stored = reg.get_component::<MeshComponent>(id).unwrap();
        assert_eq!(stored.model, "models/decraniated/scene.gltf");
    }

    #[test]
    fn entity_spawns_at_map_origin() {
        let mut reg = EntityRegistry::new();
        let entity = entity_with_kvps(&[("model", "a.gltf")]);

        let id = handle(&entity, &mut reg).expect("spawn should succeed");
        let transform = reg.get_component::<Transform>(id).unwrap();
        assert_eq!(transform.position, Vec3::new(1.0, 2.0, 3.0));
    }

    #[test]
    fn tags_are_copied_onto_entity() {
        let mut reg = EntityRegistry::new();
        let mut entity = entity_with_kvps(&[("model", "a.gltf")]);
        entity.tags = vec!["props".into(), "set_a".into()];

        let id = handle(&entity, &mut reg).expect("spawn should succeed");
        let tags = reg.get_tags(id).unwrap();
        assert_eq!(tags, &["props".to_string(), "set_a".to_string()]);
    }

    #[test]
    fn absent_model_logs_and_still_spawns_with_empty_handle() {
        // Acceptance: absent `model` warns and the load continues — no panic,
        // no abort. The entity spawns with an empty model and renders nothing.
        let mut reg = EntityRegistry::new();
        let entity = entity_with_kvps(&[]);

        let id = handle(&entity, &mut reg).expect("spawn should succeed even without a model");
        let stored = reg.get_component::<MeshComponent>(id).unwrap();
        assert_eq!(stored.model, "");
    }

    #[test]
    fn empty_model_logs_and_still_spawns_with_empty_handle() {
        let mut reg = EntityRegistry::new();
        let entity = entity_with_kvps(&[("model", "")]);

        let id = handle(&entity, &mut reg).expect("spawn should succeed even with empty model");
        let stored = reg.get_component::<MeshComponent>(id).unwrap();
        assert_eq!(stored.model, "");
    }

    #[test]
    fn dispatch_routes_prop_mesh_classname_to_handler() {
        let mut dispatch = super::super::ClassnameDispatch::new();
        super::super::register_builtins(&mut dispatch);

        let entity = entity_with_kvps(&[("model", "models/x.gltf")]);
        let handler = dispatch
            .lookup(&entity.classname)
            .expect("prop_mesh should be registered");

        let mut reg = EntityRegistry::new();
        let id = handler(&entity, &mut reg).expect("spawn should succeed");
        let stored = reg.get_component::<MeshComponent>(id).unwrap();
        assert_eq!(stored.model, "models/x.gltf");
    }
}
