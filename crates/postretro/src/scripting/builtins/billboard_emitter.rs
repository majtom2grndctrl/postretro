// Built-in classname handler for `billboard_emitter` map entities.
// See: context/plans/in-progress/scripting-foundation/plan-3-emitter-entity.md §Sub-plan 6
//
// Naming intent (load-bearing — please don't speculatively rename):
//   "BillboardEmitter" is the built-in type name. The rendering primitive
//   (camera-facing billboard sprite) lives in the type name. "ParticleEmitter"
//   is reserved for future mesh-particle work. Future contributors: this
//   distinction is the whole point — billboard is one rendering primitive
//   among several we expect to add (mesh, ribbon, decal). Renaming this to a
//   generic "ParticleEmitter" would collapse that distinction and force a
//   later, breaking re-rename when mesh particles land.

use glam::{Quat, Vec3};

use super::MapEntity;
use crate::scripting::components::billboard_emitter::BillboardEmitterComponent;
use crate::scripting::registry::{EntityId, EntityRegistry, Transform};

/// FGD `classname` this handler binds to.
pub(crate) const CLASSNAME: &str = "billboard_emitter";

/// Default `BillboardEmitterComponent` for a map entity that omits every KVP.
/// Curve channels match `smokeEmitter` from sub-plan 7's preset table; FGD has
/// no key for them at this scope.
fn default_component() -> BillboardEmitterComponent {
    BillboardEmitterComponent {
        rate: 6.0,
        burst: None,
        spread: 0.4,
        lifetime: 3.0,
        initial_velocity: [0.0, 0.8, 0.0],
        buoyancy: 0.2,
        drag: 0.8,
        size_over_lifetime: vec![0.3, 1.5],
        opacity_over_lifetime: vec![0.0, 0.8, 0.6, 0.0],
        color: [1.0, 1.0, 1.0],
        sprite: "smoke".to_string(),
        spin_rate: 0.0,
        // `spin_animation` is runtime-only — set by reactions, never by FGD.
        spin_animation: None,
    }
}

/// Read a `f32` KVP. Absent → returns `None`. Present-but-malformed → logs a
/// `[Loader]` warning naming the key and origin and returns `None` so the
/// caller can fall back to the default. Matches data-script-setup's
/// log-and-continue policy.
fn kvp_f32(entity: &MapEntity, key: &str) -> Option<f32> {
    let raw = entity.key_values.get(key)?;
    match raw.trim().parse::<f32>() {
        Ok(v) if v.is_finite() => Some(v),
        Ok(v) => {
            log::warn!(
                "[Loader] {origin}: key `{key}` has non-finite value `{v}`; using default",
                origin = entity.diagnostic_origin(),
            );
            None
        }
        Err(_) => {
            log::warn!(
                "[Loader] {origin}: key `{key}` has invalid float value `{raw}`; using default",
                origin = entity.diagnostic_origin(),
            );
            None
        }
    }
}

/// Read a string KVP. Empty → treated as absent (and warned), so the default
/// kicks in — a zero-length sprite name would otherwise propagate a useless
/// value into the billboard renderer.
fn kvp_string<'a>(entity: &'a MapEntity, key: &str) -> Option<&'a str> {
    let raw = entity.key_values.get(key)?;
    if raw.is_empty() {
        log::warn!(
            "[Loader] {origin}: key `{key}` is empty; using default",
            origin = entity.diagnostic_origin(),
        );
        return None;
    }
    Some(raw.as_str())
}

/// Build a `BillboardEmitterComponent` from a map entity's KVPs, falling back
/// to documented defaults for absent or malformed values.
fn component_from_entity(entity: &MapEntity) -> BillboardEmitterComponent {
    let mut c = default_component();

    if let Some(v) = kvp_f32(entity, "rate") {
        c.rate = v;
    }
    if let Some(v) = kvp_f32(entity, "lifetime") {
        c.lifetime = v;
    }
    if let Some(v) = kvp_f32(entity, "spread") {
        c.spread = v;
    }
    if let Some(v) = kvp_f32(entity, "buoyancy") {
        c.buoyancy = v;
    }
    if let Some(v) = kvp_f32(entity, "drag") {
        c.drag = v;
    }
    if let Some(v) = kvp_string(entity, "sprite") {
        c.sprite = v.to_string();
    }
    if let Some(v) = kvp_f32(entity, "initial_velocity_x") {
        c.initial_velocity[0] = v;
    }
    if let Some(v) = kvp_f32(entity, "initial_velocity_y") {
        c.initial_velocity[1] = v;
    }
    if let Some(v) = kvp_f32(entity, "initial_velocity_z") {
        c.initial_velocity[2] = v;
    }
    if let Some(v) = kvp_f32(entity, "color_r") {
        c.color[0] = v;
    }
    if let Some(v) = kvp_f32(entity, "color_g") {
        c.color[1] = v;
    }
    if let Some(v) = kvp_f32(entity, "color_b") {
        c.color[2] = v;
    }
    if let Some(v) = kvp_f32(entity, "spin_rate") {
        c.spin_rate = v;
    }

    c
}

/// Spawn an ECS entity carrying a `BillboardEmitterComponent` configured from
/// the map entity's KVPs. Returns `None` when the registry is exhausted.
pub(crate) fn handle(entity: &MapEntity, registry: &mut EntityRegistry) -> Option<EntityId> {
    let component = component_from_entity(entity);

    let transform = Transform {
        position: entity.origin,
        rotation: Quat::IDENTITY,
        scale: Vec3::ONE,
    };
    let id = registry.try_spawn(transform).or_else(|| {
        log::warn!(
            "[Loader] {origin}: entity registry exhausted; dropping billboard_emitter",
            origin = entity.diagnostic_origin(),
        );
        None
    })?;

    // `set_component` only fails on a stale id — the id was just returned by
    // `try_spawn` so it must be live.
    let _ = registry.set_component(id, component);
    if !entity.tags.is_empty() {
        let _ = registry.set_tags(id, entity.tags.clone());
    }
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
            key_values: kv,
            tags: vec![],
        }
    }

    #[test]
    fn rate_kvp_lands_on_component() {
        let mut reg = EntityRegistry::new();
        let entity = entity_with_kvps(&[("rate", "12")]);

        let id = handle(&entity, &mut reg).expect("spawn should succeed");
        let stored = reg.get_component::<BillboardEmitterComponent>(id).unwrap();
        assert_eq!(stored.rate, 12.0);
    }

    #[test]
    fn absent_kvps_use_documented_defaults() {
        let mut reg = EntityRegistry::new();
        let entity = entity_with_kvps(&[]);

        let id = handle(&entity, &mut reg).expect("spawn should succeed");
        let stored = reg.get_component::<BillboardEmitterComponent>(id).unwrap();
        let expected = default_component();

        assert_eq!(stored.rate, expected.rate);
        assert_eq!(stored.lifetime, expected.lifetime);
        assert_eq!(stored.spread, expected.spread);
        assert_eq!(stored.buoyancy, expected.buoyancy);
        assert_eq!(stored.drag, expected.drag);
        assert_eq!(stored.sprite, expected.sprite);
        assert_eq!(stored.initial_velocity, expected.initial_velocity);
        assert_eq!(stored.color, expected.color);
        assert_eq!(stored.spin_rate, expected.spin_rate);
        assert_eq!(stored.size_over_lifetime, expected.size_over_lifetime);
        assert_eq!(stored.opacity_over_lifetime, expected.opacity_over_lifetime);
        assert!(stored.spin_animation.is_none());
        assert!(stored.burst.is_none());
    }

    #[test]
    fn invalid_float_kvp_falls_back_to_default() {
        let mut reg = EntityRegistry::new();
        let entity = entity_with_kvps(&[("rate", "bad"), ("buoyancy", "0.5")]);

        let id = handle(&entity, &mut reg).expect("spawn should succeed");
        let stored = reg.get_component::<BillboardEmitterComponent>(id).unwrap();
        // `rate` was malformed → falls back to the default (6.0).
        assert_eq!(stored.rate, 6.0);
        // `buoyancy` was valid → applied.
        assert_eq!(stored.buoyancy, 0.5);
    }

    #[test]
    fn empty_sprite_string_falls_back_to_default() {
        let mut reg = EntityRegistry::new();
        let entity = entity_with_kvps(&[("sprite", "")]);

        let id = handle(&entity, &mut reg).expect("spawn should succeed");
        let stored = reg.get_component::<BillboardEmitterComponent>(id).unwrap();
        assert_eq!(stored.sprite, "smoke");
    }

    #[test]
    fn vec3_split_kvps_assemble_into_initial_velocity() {
        let mut reg = EntityRegistry::new();
        let entity = entity_with_kvps(&[
            ("initial_velocity_x", "1.0"),
            ("initial_velocity_y", "2.0"),
            ("initial_velocity_z", "-3.0"),
        ]);

        let id = handle(&entity, &mut reg).expect("spawn should succeed");
        let stored = reg.get_component::<BillboardEmitterComponent>(id).unwrap();
        assert_eq!(stored.initial_velocity, [1.0, 2.0, -3.0]);
    }

    #[test]
    fn color_split_kvps_assemble_into_color() {
        let mut reg = EntityRegistry::new();
        let entity = entity_with_kvps(&[
            ("color_r", "0.25"),
            ("color_g", "0.5"),
            ("color_b", "0.75"),
        ]);

        let id = handle(&entity, &mut reg).expect("spawn should succeed");
        let stored = reg.get_component::<BillboardEmitterComponent>(id).unwrap();
        assert_eq!(stored.color, [0.25, 0.5, 0.75]);
    }

    #[test]
    fn entity_spawns_at_map_origin() {
        let mut reg = EntityRegistry::new();
        let entity = entity_with_kvps(&[]);

        let id = handle(&entity, &mut reg).expect("spawn should succeed");
        let transform = reg.get_component::<Transform>(id).unwrap();
        assert_eq!(transform.position, Vec3::new(1.0, 2.0, 3.0));
    }

    #[test]
    fn tags_are_copied_onto_entity() {
        let mut reg = EntityRegistry::new();
        let mut entity = entity_with_kvps(&[]);
        entity.tags = vec!["smoke_a".into(), "campfires".into()];

        let id = handle(&entity, &mut reg).expect("spawn should succeed");
        let tags = reg.get_tags(id).unwrap();
        assert_eq!(tags, &["smoke_a".to_string(), "campfires".to_string()]);
    }

    #[test]
    fn dispatch_routes_billboard_emitter_classname_to_handler() {
        // End-to-end through the dispatch table: simulates what sub-plan 8 will
        // do when the level loader walks map entities and looks up classnames.
        let mut dispatch = super::super::ClassnameDispatch::new();
        super::super::register_builtins(&mut dispatch);

        let entity = entity_with_kvps(&[("rate", "9.5")]);
        let handler = dispatch
            .lookup(&entity.classname)
            .expect("billboard_emitter should be registered");

        let mut reg = EntityRegistry::new();
        let id = handler(&entity, &mut reg).expect("spawn should succeed");
        let stored = reg.get_component::<BillboardEmitterComponent>(id).unwrap();
        assert_eq!(stored.rate, 9.5);
    }
}
