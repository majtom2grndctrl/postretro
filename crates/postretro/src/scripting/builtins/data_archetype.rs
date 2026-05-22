// Data-archetype spawn path: walks `world.map_entities` against the
// `DataRegistry.entities` table populated at `setupMod()` ingestion time,
// materializing each matching placement into an ECS entity with the
// descriptor's component presets attached.
//
// See: context/lib/build_pipeline.md §Built-in Classname Routing
//      context/lib/scripting.md §2 (data context lifecycle)
//
// The built-in classname-dispatch sweep runs first; this pass receives the
// set of classnames that were already handled and skips them. A classname
// that appears in BOTH dispatch tables logs a `warn!` once per classname and
// keeps the built-in result (built-in wins).

use std::collections::HashSet;

use glam::Vec3;

use super::MapEntity;
use crate::scripting::components::billboard_emitter::BillboardEmitterComponent;
use crate::scripting::components::light::{FalloffKind, LightComponent, LightKind};
use crate::scripting::components::player_movement::PlayerMovementComponent;
use crate::scripting::data_descriptors::{EntityTypeDescriptor, LightDescriptor};
use crate::scripting::registry::{EntityId, EntityRegistry, Transform};

/// Apply the `initial_<field>` KVP override convention to the descriptor's
/// component presets. Each scalar field (`f32`, `u32`) parses via `FromStr`;
/// `[f32; 3]` parses as three space-delimited floats. Parse failures
/// `warn!` with the diagnostic origin and offending key/value pair, leaving
/// the descriptor default in place.
fn apply_emitter_kvp_overrides(component: &mut BillboardEmitterComponent, entity: &MapEntity) {
    for (key, raw) in entity.key_values.iter() {
        let Some(field) = key.strip_prefix("initial_") else {
            continue;
        };
        match field {
            "rate" => parse_into_f32(raw, &mut component.rate, entity, key),
            "spread" => parse_into_f32(raw, &mut component.spread, entity, key),
            "lifetime" => parse_into_f32(raw, &mut component.lifetime, entity, key),
            "buoyancy" => parse_into_f32(raw, &mut component.buoyancy, entity, key),
            "drag" => parse_into_f32(raw, &mut component.drag, entity, key),
            "spin_rate" => parse_into_f32(raw, &mut component.spin_rate, entity, key),
            "burst" => match raw.trim().parse::<u32>() {
                Ok(v) => component.burst = Some(v),
                Err(_) => warn_parse(entity, key, raw),
            },
            "sprite" => {
                if raw.is_empty() {
                    warn_parse(entity, key, raw);
                } else {
                    component.sprite = raw.clone();
                }
            }
            "color" => parse_into_vec3(raw, &mut component.color, entity, key),
            "velocity" => parse_into_vec3(raw, &mut component.velocity, entity, key),
            _ => {}
        }
    }
}

fn apply_light_kvp_overrides(descriptor: &mut LightDescriptor, entity: &MapEntity) {
    for (key, raw) in entity.key_values.iter() {
        let Some(field) = key.strip_prefix("initial_") else {
            continue;
        };
        match field {
            // Mirror the validation applied at descriptor parse time: reject
            // negative or non-finite values at parse time so a bad override
            // never lands on the descriptor (e.g. `initial_intensity -5.0`).
            "intensity" => parse_into_nonneg_f32(raw, &mut descriptor.intensity, entity, key),
            "range" => parse_into_nonneg_f32(raw, &mut descriptor.range, entity, key),
            "is_dynamic" => match parse_bool(raw) {
                Some(v) => descriptor.is_dynamic = v,
                None => warn_parse(entity, key, raw),
            },
            "color" => parse_into_vec3(raw, &mut descriptor.color, entity, key),
            _ => {}
        }
    }
}

/// Parse a boolean KVP value. Accepts TrenchBroom's `"0"`/`"1"` switch
/// representation in addition to `"true"`/`"false"` (case-insensitive).
/// Any other input returns `None`, matching the parse-failure fallback used
/// by other field types.
fn parse_bool(raw: &str) -> Option<bool> {
    match raw.trim().to_ascii_lowercase().as_str() {
        "1" | "true" => Some(true),
        "0" | "false" => Some(false),
        _ => None,
    }
}

fn parse_into_f32(raw: &str, slot: &mut f32, entity: &MapEntity, key: &str) {
    match raw.trim().parse::<f32>() {
        Ok(v) if v.is_finite() => *slot = v,
        _ => warn_parse(entity, key, raw),
    }
}

/// Like `parse_into_f32` but additionally rejects negative values, mirroring
/// `LightDescriptor::validate()`. Bad values warn and leave the descriptor
/// default in place — the `slot` is only written on success.
fn parse_into_nonneg_f32(raw: &str, slot: &mut f32, entity: &MapEntity, key: &str) {
    match raw.trim().parse::<f32>() {
        Ok(v) if v.is_finite() && v >= 0.0 => *slot = v,
        _ => warn_parse(entity, key, raw),
    }
}

fn parse_into_vec3(raw: &str, slot: &mut [f32; 3], entity: &MapEntity, key: &str) {
    let parts: Vec<&str> = raw.split_whitespace().collect();
    if parts.len() != 3 {
        warn_parse(entity, key, raw);
        return;
    }
    let mut out = [0.0f32; 3];
    for (i, part) in parts.iter().enumerate() {
        match part.parse::<f32>() {
            Ok(v) if v.is_finite() => out[i] = v,
            _ => {
                warn_parse(entity, key, raw);
                return;
            }
        }
    }
    *slot = out;
}

fn warn_parse(entity: &MapEntity, key: &str, raw: &str) {
    log::warn!(
        "[Loader] {origin}: key `{key}` has invalid value `{raw}`; using descriptor default",
        origin = entity.diagnostic_origin(),
    );
}

/// Find an `EntityTypeDescriptor` whose `canonical_name` equals `classname`.
/// Linear scan — descriptor lists are small (one entry per registered class)
/// so a HashMap would be premature. Descriptors with `canonical_name = None`
/// have no map-placement form and are always skipped here.
///
/// Roadmap: composable archetypes, FGD generated from the registry, and a
/// `mob_spawn` marker (mirroring `player_spawn`) will sit on this lookup.
/// The abstraction grows from this single match site.
fn find_descriptor<'a>(
    descriptors: &'a [EntityTypeDescriptor],
    classname: &str,
) -> Option<&'a EntityTypeDescriptor> {
    descriptors
        .iter()
        .find(|d| d.canonical_name.as_deref() == Some(classname))
}

/// Attach emitter, light, and movement components from a descriptor to an
/// already-spawned entity. `initial_*` KVP overrides are applied to `emitter`
/// and `light` before attachment; `movement` receives descriptor values
/// verbatim — no per-placement override mechanism exists for movement params.
/// The light is always forced dynamic regardless of the descriptor's
/// `is_dynamic` field (baked indirect lighting is not supported for
/// descriptor-spawned lights), with a `warn!` if the descriptor had it set to
/// `false`.
fn attach_descriptor_components(
    registry: &mut EntityRegistry,
    id: EntityId,
    descriptor: &EntityTypeDescriptor,
    entity: &MapEntity,
) {
    if let Some(emitter) = descriptor.emitter.clone() {
        let mut component = emitter;
        apply_emitter_kvp_overrides(&mut component, entity);
        // `set_component` only fails on a stale id — the id was just returned.
        let _ = registry.set_component(id, component);
    }

    if let Some(light_desc) = descriptor.light.clone() {
        let mut light_desc = light_desc;
        apply_light_kvp_overrides(&mut light_desc, entity);

        if !light_desc.is_dynamic {
            log::warn!(
                "[Loader] {origin}: descriptor-spawned light `{cls}` was authored \
                 `is_dynamic = false`; forcing dynamic (baked indirect not supported \
                 for descriptor-spawned lights)",
                origin = entity.diagnostic_origin(),
                cls = entity.classname,
            );
        }

        let component = LightComponent {
            origin: [entity.origin.x, entity.origin.y, entity.origin.z],
            light_type: LightKind::Point,
            intensity: light_desc.intensity,
            color: light_desc.color,
            falloff_model: FalloffKind::InverseSquared,
            falloff_range: light_desc.range,
            cone_angle_inner: None,
            cone_angle_outer: None,
            cone_direction: None,
            cast_shadows: false,
            is_dynamic: true,
            animation: None,
        };
        let _ = registry.set_component(id, component);
    }

    if let Some(movement_desc) = descriptor.movement.as_ref() {
        let component = PlayerMovementComponent::from_descriptor(movement_desc);
        let _ = registry.set_component(id, component);
    }
}

/// Spawn descriptor-driven entities for every `MapEntity` whose classname
/// matches a registered descriptor AND was not already handled by the
/// built-in dispatch.
///
/// # Returns
///
/// The set of classnames for which at least one descriptor-spawned entity was
/// successfully materialized (i.e. `registry.try_spawn` returned `Some`).
/// Classnames that were skipped because they appear in `handled_by_builtin`
/// are excluded — even if no entity actually landed via the built-in path
/// (registry exhausted), the classname is still considered owned by built-in
/// dispatch and is not included here. This means callers must not union this
/// set with the built-in set to derive "all claimed classnames": the built-in
/// set already covers the collision cases. Contrast with
/// [`super::apply_classname_dispatch`], which includes every classname a
/// handler was attempted for, independent of spawn success.
///
/// Placements whose classname is not found in either the descriptor list or
/// `handled_by_builtin`, and is not in the structural exclusion set
/// (`worldspawn`, `player_spawn`), log a `warn!` once per distinct classname
/// per sweep.
pub(crate) fn apply_data_archetype_dispatch(
    entities: &[MapEntity],
    descriptors: &[EntityTypeDescriptor],
    handled_by_builtin: &HashSet<String>,
    registry: &mut EntityRegistry,
) -> HashSet<String> {
    // Warn-once tracking for descriptor/built-in collisions and for placements
    // referencing a classname that has no descriptor match.
    // Scoped per sweep; current callers run exactly once per level load.
    let mut collision_warned: HashSet<String> = HashSet::new();
    let mut unknown_warned: HashSet<String> = HashSet::new();
    let mut handled: HashSet<String> = HashSet::new();

    for entity in entities {
        let Some(descriptor) = find_descriptor(descriptors, &entity.classname) else {
            // No descriptor match. The built-in dispatch already handled this
            // classname (or attempted to) when it appears in `handled_by_builtin`;
            // otherwise the classname is either an unmodeled placement or a
            // structural marker (worldspawn, player_spawn) routed elsewhere.
            // Warn once per distinct unknown classname per sweep.
            let cls = entity.classname.as_str();
            let is_structural = cls == "worldspawn" || cls == PLAYER_START_CLASSNAME;
            if !handled_by_builtin.contains(cls)
                && !is_structural
                && unknown_warned.insert(cls.to_string())
            {
                log::warn!(
                    "[Loader] {origin}: classname `{cls}` has no registered descriptor; placement dropped",
                    origin = entity.diagnostic_origin(),
                );
            }
            continue;
        };

        if handled_by_builtin.contains(&entity.classname) {
            if collision_warned.insert(entity.classname.clone()) {
                log::warn!(
                    "[Loader] {origin}: classname `{}` is registered both as a built-in handler \
                     and a data-script entity descriptor; built-in handler wins",
                    entity.classname,
                    origin = entity.diagnostic_origin(),
                );
            }
            // Intentionally skips descriptor spawn even if the built-in
            // returned None (registry exhausted) — built-in attempted is
            // treated as built-in handled.
            continue;
        }

        let transform = Transform {
            position: entity.origin,
            rotation: entity.rotation_quat(),
            scale: Vec3::ONE,
        };

        let Some(id) = registry.try_spawn(transform, &entity.tags) else {
            log::warn!(
                "[Loader] {origin}: entity registry exhausted; dropping descriptor-spawned `{cls}`",
                origin = entity.diagnostic_origin(),
                cls = entity.classname,
            );
            continue;
        };

        attach_descriptor_components(registry, id, descriptor, entity);

        // Mirror the per-placement KVP bag so `getEntityProperty` works
        // uniformly across spawn paths. Always write — even an empty bag —
        // to honor the invariant that every map-spawned entity has a
        // `kvp_table` entry (matches the built-in dispatch path).
        let _ = registry.set_map_kvps(id, entity.key_values.clone());

        handled.insert(entity.classname.clone());
    }

    handled
}

/// Classname for the FGD `player_spawn` point entity. Spawn points are
/// extracted from `world.map_entities` before the built-in / data-archetype
/// dispatch sweeps and processed by [`spawn_from_player_starts`].
pub(crate) const PLAYER_START_CLASSNAME: &str = "player_spawn";

/// Spawn one entity per `player_spawn` placement, using each placement's
/// `entity_class` KVP (default `"player"`) to look up an
/// [`EntityTypeDescriptor`]. Component attachment mirrors
/// [`apply_data_archetype_dispatch`] — emitter, light (forced dynamic), and
/// movement (when present in the descriptor) — and the per-placement KVP bag
/// is forwarded with `entity_class` stripped so it
/// is not confused with an `initial_*`-style override. Tags from the
/// `player_spawn` placement are passed directly to `try_spawn`.
pub(crate) fn spawn_from_player_starts(
    spawn_points: &[MapEntity],
    descriptors: &[EntityTypeDescriptor],
    registry: &mut EntityRegistry,
) {
    let mut spawned = 0usize;

    for entity in spawn_points {
        let entity_class = entity
            .key_values
            .get("entity_class")
            .map(String::as_str)
            .unwrap_or("player");

        let Some(descriptor) = find_descriptor(descriptors, entity_class) else {
            log::warn!(
                "[Loader] {origin}: entity_class `{entity_class}` not registered; skipping spawn point",
                origin = entity.diagnostic_origin(),
            );
            continue;
        };

        let transform = Transform {
            position: entity.origin,
            rotation: entity.rotation_quat(),
            scale: Vec3::ONE,
        };

        let Some(id) = registry.try_spawn(transform, &entity.tags) else {
            log::warn!(
                "[Loader] {origin}: entity registry exhausted; dropping player spawn `{entity_class}`",
                origin = entity.diagnostic_origin(),
            );
            continue;
        };

        attach_descriptor_components(registry, id, descriptor, entity);

        // Forward the per-placement KVP bag (sans `entity_class`, which is a
        // routing hint, not a runtime property) so `getEntityProperty` works
        // uniformly for player-start-spawned entities.
        let mut kvps = entity.key_values.clone();
        kvps.remove("entity_class");
        let _ = registry.set_map_kvps(id, kvps);

        spawned += 1;
    }

    if !spawn_points.is_empty() {
        log::info!(
            "[Loader] spawned {spawned} player(s) from {total} player_spawn entries",
            total = spawn_points.len(),
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    fn placement(classname: &str, kvps: &[(&str, &str)]) -> MapEntity {
        let mut kv = HashMap::new();
        for (k, v) in kvps {
            kv.insert((*k).to_string(), (*v).to_string());
        }
        MapEntity {
            classname: classname.to_string(),
            origin: Vec3::new(1.0, 2.0, 3.0),
            angles: Vec3::ZERO,
            key_values: kv,
            tags: vec![],
        }
    }

    fn light_descriptor(classname: &str, is_dynamic: bool) -> EntityTypeDescriptor {
        EntityTypeDescriptor {
            canonical_name: Some(classname.to_string()),
            light: Some(LightDescriptor {
                color: [0.5, 0.5, 0.5],
                intensity: 1.0,
                range: 8.0,
                is_dynamic,
            }),
            emitter: None,
            movement: None,
        }
    }

    #[test]
    fn descriptor_spawn_creates_entity_with_light_component() {
        let mut reg = EntityRegistry::new();
        let descriptors = vec![light_descriptor("torch", true)];
        let placements = vec![placement("torch", &[])];
        let handled =
            apply_data_archetype_dispatch(&placements, &descriptors, &HashSet::new(), &mut reg);
        assert_eq!(handled.len(), 1);

        let (id, _) = reg
            .iter_with_kind(crate::scripting::registry::ComponentKind::Light)
            .next()
            .expect("light spawned");
        let light = reg.get_component::<LightComponent>(id).unwrap();
        assert!(light.is_dynamic);
        assert_eq!(light.falloff_range, 8.0);
        assert_eq!(light.origin, [1.0, 2.0, 3.0]);
    }

    #[test]
    fn descriptor_spawn_forces_dynamic_when_descriptor_was_baked() {
        let mut reg = EntityRegistry::new();
        let descriptors = vec![light_descriptor("torch", false)];
        let placements = vec![placement("torch", &[])];
        apply_data_archetype_dispatch(&placements, &descriptors, &HashSet::new(), &mut reg);
        let (id, _) = reg
            .iter_with_kind(crate::scripting::registry::ComponentKind::Light)
            .next()
            .unwrap();
        assert!(reg.get_component::<LightComponent>(id).unwrap().is_dynamic);
    }

    #[test]
    fn descriptor_spawn_skips_classnames_handled_by_builtin() {
        let mut reg = EntityRegistry::new();
        let descriptors = vec![light_descriptor("torch", true)];
        let placements = vec![placement("torch", &[])];
        let mut handled = HashSet::new();
        handled.insert("torch".to_string());
        let result = apply_data_archetype_dispatch(&placements, &descriptors, &handled, &mut reg);
        assert_eq!(result.len(), 0);
    }

    #[test]
    fn initial_prefix_kvp_overrides_descriptor_field() {
        let mut reg = EntityRegistry::new();
        let descriptors = vec![light_descriptor("torch", true)];
        let placements = vec![placement(
            "torch",
            &[("initial_intensity", "5.5"), ("initial_range", "20")],
        )];
        apply_data_archetype_dispatch(&placements, &descriptors, &HashSet::new(), &mut reg);
        let (id, _) = reg
            .iter_with_kind(crate::scripting::registry::ComponentKind::Light)
            .next()
            .unwrap();
        let light = reg.get_component::<LightComponent>(id).unwrap();
        assert_eq!(light.intensity, 5.5);
        assert_eq!(light.falloff_range, 20.0);
    }

    #[test]
    fn initial_color_kvp_overrides_via_space_delimited_floats() {
        let mut reg = EntityRegistry::new();
        let descriptors = vec![light_descriptor("torch", true)];
        let placements = vec![placement("torch", &[("initial_color", "1.0 0.5 0.25")])];
        apply_data_archetype_dispatch(&placements, &descriptors, &HashSet::new(), &mut reg);
        let (id, _) = reg
            .iter_with_kind(crate::scripting::registry::ComponentKind::Light)
            .next()
            .unwrap();
        let light = reg.get_component::<LightComponent>(id).unwrap();
        assert_eq!(light.color, [1.0, 0.5, 0.25]);
    }

    #[test]
    fn initial_is_dynamic_kvp_accepts_trenchbroom_zero_one() {
        // TrenchBroom's checkbox/switch widget writes `"0"` and `"1"` into
        // the .map file. Both must parse correctly; `"true"` and `"false"`
        // remain accepted (case-insensitive) for hand-authored .map files.
        for (raw, expected) in [
            ("0", false),
            ("1", true),
            ("true", true),
            ("false", false),
            ("TRUE", true),
            ("False", false),
        ] {
            let mut reg = EntityRegistry::new();
            let descriptors = vec![light_descriptor("torch", false)];
            let placements = vec![placement("torch", &[("initial_is_dynamic", raw)])];
            apply_data_archetype_dispatch(&placements, &descriptors, &HashSet::new(), &mut reg);
            // The descriptor light is forced dynamic at spawn regardless of the
            // descriptor's `is_dynamic`. To verify the parse landed on the
            // descriptor field itself, we exercise `apply_light_kvp_overrides`
            // directly:
            let mut desc = LightDescriptor {
                color: [1.0, 1.0, 1.0],
                intensity: 1.0,
                range: 1.0,
                is_dynamic: !expected,
            };
            let entity = placement("torch", &[("initial_is_dynamic", raw)]);
            apply_light_kvp_overrides(&mut desc, &entity);
            assert_eq!(
                desc.is_dynamic, expected,
                "raw `{raw}` should parse to {expected}",
            );
        }
    }

    #[test]
    fn initial_is_dynamic_kvp_falls_back_on_unrecognized_value() {
        // Any non-"0"/"1"/"true"/"false" value warns and leaves the
        // descriptor default in place.
        let mut desc = LightDescriptor {
            color: [1.0, 1.0, 1.0],
            intensity: 1.0,
            range: 1.0,
            is_dynamic: true,
        };
        let entity = placement("torch", &[("initial_is_dynamic", "yes")]);
        apply_light_kvp_overrides(&mut desc, &entity);
        assert!(desc.is_dynamic);
    }

    #[test]
    fn initial_intensity_negative_falls_back_to_descriptor_default() {
        // Validation applied at descriptor parse time rejects negative
        // intensity. The KVP override path must apply the same check, so a
        // map author writing `initial_intensity = -5.0` does not produce
        // a descriptor that would have been rejected at ingestion time.
        let mut reg = EntityRegistry::new();
        let descriptors = vec![light_descriptor("torch", true)];
        let placements = vec![placement("torch", &[("initial_intensity", "-5.0")])];
        apply_data_archetype_dispatch(&placements, &descriptors, &HashSet::new(), &mut reg);
        let (id, _) = reg
            .iter_with_kind(crate::scripting::registry::ComponentKind::Light)
            .next()
            .unwrap();
        let light = reg.get_component::<LightComponent>(id).unwrap();
        // Descriptor default (1.0) preserved despite the bad override.
        assert_eq!(light.intensity, 1.0);
    }

    #[test]
    fn initial_range_negative_falls_back_to_descriptor_default() {
        let mut reg = EntityRegistry::new();
        let descriptors = vec![light_descriptor("torch", true)];
        let placements = vec![placement("torch", &[("initial_range", "-2.0")])];
        apply_data_archetype_dispatch(&placements, &descriptors, &HashSet::new(), &mut reg);
        let (id, _) = reg
            .iter_with_kind(crate::scripting::registry::ComponentKind::Light)
            .next()
            .unwrap();
        let light = reg.get_component::<LightComponent>(id).unwrap();
        assert_eq!(light.falloff_range, 8.0);
    }

    #[test]
    fn malformed_initial_kvp_falls_back_to_descriptor_default() {
        let mut reg = EntityRegistry::new();
        let descriptors = vec![light_descriptor("torch", true)];
        let placements = vec![placement("torch", &[("initial_intensity", "not-a-number")])];
        apply_data_archetype_dispatch(&placements, &descriptors, &HashSet::new(), &mut reg);
        let (id, _) = reg
            .iter_with_kind(crate::scripting::registry::ComponentKind::Light)
            .next()
            .unwrap();
        let light = reg.get_component::<LightComponent>(id).unwrap();
        assert_eq!(light.intensity, 1.0);
    }

    #[test]
    fn descriptor_spawn_skips_all_instances_of_a_conflicting_classname() {
        // Multiple placements with the same conflicting classname must all be
        // dropped (built-in wins). The warn-once dedup is exercised by
        // running enough placements that the guard would fire repeatedly if
        // the dedup were broken; we verify zero descriptor spawns and
        // — to actually drive the dedup path — also re-invoke dispatch with
        // another colliding placement and confirm the count stays at zero.
        // The `warn!` itself is logged via `log::warn!` and not captured here;
        // logging is verified manually.
        let mut reg = EntityRegistry::new();
        let descriptors = vec![light_descriptor("torch", true)];
        let placements = vec![
            placement("torch", &[]),
            placement("torch", &[]),
            placement("torch", &[]),
        ];
        let mut handled = HashSet::new();
        handled.insert("torch".to_string());

        let result = apply_data_archetype_dispatch(&placements, &descriptors, &handled, &mut reg);

        assert_eq!(
            result.len(),
            0,
            "all colliding placements must be skipped by the conflict guard"
        );
        assert!(
            reg.iter_with_kind(crate::scripting::registry::ComponentKind::Light)
                .next()
                .is_none(),
            "no descriptor-spawned light should land for a colliding classname"
        );

        // Second invocation: the dedup state is per-call, so the guard must
        // continue to drop colliding placements on a fresh dispatch pass.
        let more = vec![placement("torch", &[])];
        let result2 = apply_data_archetype_dispatch(&more, &descriptors, &handled, &mut reg);
        assert_eq!(result2.len(), 0);
    }

    #[test]
    fn emitter_initial_velocity_kvp_overrides_velocity_field() {
        // The component field is named `velocity` so the `initial_<field>`
        // KVP convention spells the override `initial_velocity` cleanly,
        // without a redundant prefix.
        let mut reg = EntityRegistry::new();
        let descriptors = vec![EntityTypeDescriptor {
            canonical_name: Some("campfire".to_string()),
            light: None,
            emitter: Some(BillboardEmitterComponent {
                rate: 6.0,
                burst: None,
                spread: 0.4,
                lifetime: 3.0,
                velocity: [0.0, 1.0, 0.0],
                buoyancy: 0.2,
                drag: 0.5,
                size_over_lifetime: vec![1.0],
                opacity_over_lifetime: vec![1.0, 0.0],
                color: [1.0, 1.0, 1.0],
                sprite: "smoke".to_string(),
                spin_rate: 0.0,
                spin_animation: None,
            }),
            movement: None,
        }];
        let placements = vec![placement(
            "campfire",
            &[("initial_velocity", "1.0 2.0 3.0")],
        )];
        apply_data_archetype_dispatch(&placements, &descriptors, &HashSet::new(), &mut reg);
        let (id, _) = reg
            .iter_with_kind(crate::scripting::registry::ComponentKind::BillboardEmitter)
            .next()
            .expect("emitter should spawn");
        let component = reg.get_component::<BillboardEmitterComponent>(id).unwrap();
        assert_eq!(component.velocity, [1.0, 2.0, 3.0]);
    }

    #[test]
    fn emitter_bare_velocity_kvp_is_not_an_alias() {
        // Bare `velocity` (without the `initial_` prefix) must not consume the
        // KVP — that key is reserved for behavior-script `getEntityProperty`
        // consumption. Only `initial_velocity` writes to the field.
        let mut reg = EntityRegistry::new();
        let descriptors = vec![EntityTypeDescriptor {
            canonical_name: Some("campfire".to_string()),
            light: None,
            emitter: Some(BillboardEmitterComponent {
                rate: 6.0,
                burst: None,
                spread: 0.4,
                lifetime: 3.0,
                velocity: [0.0, 1.0, 0.0],
                buoyancy: 0.2,
                drag: 0.5,
                size_over_lifetime: vec![1.0],
                opacity_over_lifetime: vec![1.0, 0.0],
                color: [1.0, 1.0, 1.0],
                sprite: "smoke".to_string(),
                spin_rate: 0.0,
                spin_animation: None,
            }),
            movement: None,
        }];
        let placements = vec![placement("campfire", &[("velocity", "9.0 9.0 9.0")])];
        apply_data_archetype_dispatch(&placements, &descriptors, &HashSet::new(), &mut reg);
        let (id, _) = reg
            .iter_with_kind(crate::scripting::registry::ComponentKind::BillboardEmitter)
            .next()
            .expect("emitter should spawn");
        let component = reg.get_component::<BillboardEmitterComponent>(id).unwrap();
        // Descriptor default preserved — the bare `velocity` key did not
        // override the field.
        assert_eq!(component.velocity, [0.0, 1.0, 0.0]);
    }

    #[test]
    fn emitter_initial_kvp_overrides_scalar_field() {
        // `initial_rate` overrides the descriptor's `rate` default at spawn.
        let mut reg = EntityRegistry::new();
        let descriptors = vec![EntityTypeDescriptor {
            canonical_name: Some("campfire".to_string()),
            light: None,
            emitter: Some(BillboardEmitterComponent {
                rate: 6.0,
                burst: None,
                spread: 0.4,
                lifetime: 3.0,
                velocity: [0.0, 1.0, 0.0],
                buoyancy: 0.2,
                drag: 0.5,
                size_over_lifetime: vec![1.0],
                opacity_over_lifetime: vec![1.0, 0.0],
                color: [1.0, 1.0, 1.0],
                sprite: "smoke".to_string(),
                spin_rate: 0.0,
                spin_animation: None,
            }),
            movement: None,
        }];
        let placements = vec![placement("campfire", &[("initial_rate", "20.5")])];
        apply_data_archetype_dispatch(&placements, &descriptors, &HashSet::new(), &mut reg);
        let (id, _) = reg
            .iter_with_kind(crate::scripting::registry::ComponentKind::BillboardEmitter)
            .next()
            .expect("emitter should spawn");
        let component = reg.get_component::<BillboardEmitterComponent>(id).unwrap();
        assert_eq!(component.rate, 20.5);
    }

    #[test]
    fn emitter_initial_burst_kvp_overrides_u32_field() {
        let mut reg = EntityRegistry::new();
        let descriptors = vec![EntityTypeDescriptor {
            canonical_name: Some("burstfire".to_string()),
            light: None,
            emitter: Some(BillboardEmitterComponent {
                rate: 0.0,
                burst: None,
                spread: 0.4,
                lifetime: 0.6,
                velocity: [0.0, 2.0, 0.0],
                buoyancy: -1.0,
                drag: 0.1,
                size_over_lifetime: vec![1.0],
                opacity_over_lifetime: vec![1.0, 0.0],
                color: [1.0, 0.8, 0.3],
                sprite: "spark".to_string(),
                spin_rate: 0.0,
                spin_animation: None,
            }),
            movement: None,
        }];
        let placements = vec![placement("burstfire", &[("initial_burst", "24")])];
        apply_data_archetype_dispatch(&placements, &descriptors, &HashSet::new(), &mut reg);
        let (id, _) = reg
            .iter_with_kind(crate::scripting::registry::ComponentKind::BillboardEmitter)
            .next()
            .expect("emitter should spawn");
        let component = reg.get_component::<BillboardEmitterComponent>(id).unwrap();
        assert_eq!(component.burst, Some(24));
    }

    #[test]
    fn emitter_malformed_initial_kvp_falls_back_to_descriptor_default() {
        // A bad value for a known `initial_*` key on the emitter should warn
        // but leave the descriptor's value untouched. No crash.
        let mut reg = EntityRegistry::new();
        let descriptors = vec![EntityTypeDescriptor {
            canonical_name: Some("smolder".to_string()),
            light: None,
            emitter: Some(BillboardEmitterComponent {
                rate: 6.0,
                burst: None,
                spread: 0.4,
                lifetime: 3.0,
                velocity: [0.0, 1.0, 0.0],
                buoyancy: 0.2,
                drag: 0.5,
                size_over_lifetime: vec![1.0],
                opacity_over_lifetime: vec![1.0, 0.0],
                color: [1.0, 1.0, 1.0],
                sprite: "smoke".to_string(),
                spin_rate: 0.0,
                spin_animation: None,
            }),
            movement: None,
        }];
        let placements = vec![placement(
            "smolder",
            &[("initial_rate", "not-a-float"), ("initial_burst", "noisy")],
        )];
        apply_data_archetype_dispatch(&placements, &descriptors, &HashSet::new(), &mut reg);
        let (id, _) = reg
            .iter_with_kind(crate::scripting::registry::ComponentKind::BillboardEmitter)
            .next()
            .expect("emitter should still spawn");
        let component = reg.get_component::<BillboardEmitterComponent>(id).unwrap();
        // Bad scalar value left descriptor default in place.
        assert_eq!(component.rate, 6.0);
        // Bad burst value left descriptor default (None) in place.
        assert_eq!(component.burst, None);
    }

    /// Guards built-in priority over data-archetype dispatch end-to-end: a
    /// classname registered both as a built-in AND via `setupMod()` ingestion
    /// must spawn through the built-in path only.
    /// Drives both dispatch sweeps in the same order `main.rs` does:
    /// `apply_classname_dispatch` first, then `apply_data_archetype_dispatch`
    /// with the returned `handled` set. Asserts exactly one entity exists.
    #[test]
    fn dual_registered_classname_spawns_through_builtin_only() {
        use crate::scripting::builtins::{
            ClassnameDispatch, apply_classname_dispatch, register_builtins,
        };
        use crate::scripting::registry::ComponentKind;

        // Built-in dispatch already covers `billboard_emitter`.
        let mut dispatch = ClassnameDispatch::new();
        register_builtins(&mut dispatch);

        // Register a data-archetype descriptor for the same classname.
        let descriptors = vec![EntityTypeDescriptor {
            canonical_name: Some("billboard_emitter".to_string()),
            light: Some(LightDescriptor {
                color: [1.0, 0.0, 0.0],
                intensity: 5.0,
                range: 10.0,
                is_dynamic: true,
            }),
            emitter: None,
            movement: None,
        }];

        let placements = vec![placement("billboard_emitter", &[])];

        let mut reg = EntityRegistry::new();
        let handled = apply_classname_dispatch(&placements, &dispatch, &mut reg);
        let descriptor_handled =
            apply_data_archetype_dispatch(&placements, &descriptors, &handled, &mut reg);

        assert_eq!(
            descriptor_handled.len(),
            0,
            "data-archetype path must skip a classname owned by built-in dispatch",
        );
        // Built-in handler spawned exactly one billboard_emitter.
        let emitter_count = reg.iter_with_kind(ComponentKind::BillboardEmitter).count();
        assert_eq!(
            emitter_count, 1,
            "exactly one billboard_emitter entity should exist (built-in only)",
        );
        // And no descriptor-driven light landed.
        assert_eq!(
            reg.iter_with_kind(ComponentKind::Light).count(),
            0,
            "descriptor's light must not have been attached — built-in path wins",
        );
    }

    #[test]
    fn descriptor_spawn_with_no_matching_descriptor_skips_silently() {
        let mut reg = EntityRegistry::new();
        let descriptors = vec![light_descriptor("torch", true)];
        let placements = vec![placement("ungoverned", &[])];
        let handled =
            apply_data_archetype_dispatch(&placements, &descriptors, &HashSet::new(), &mut reg);
        assert_eq!(handled.len(), 0);
    }

    #[test]
    fn descriptor_with_no_canonical_name_is_unreachable_from_direct_placement() {
        // A descriptor with `canonical_name = None` has no direct map-placement
        // form. Two `.map` placements naming "ghost" in a single dispatch pass
        // must not spawn the descriptor — and the unknown-classname warn must
        // fire exactly once per distinct classname per sweep (verified by the
        // warn-dedup `HashSet` contract; the `log::warn!` itself is not
        // captured here).
        let mut reg = EntityRegistry::new();
        let descriptors = vec![EntityTypeDescriptor {
            canonical_name: None,
            light: Some(LightDescriptor {
                color: [1.0, 1.0, 1.0],
                intensity: 1.0,
                range: 5.0,
                is_dynamic: true,
            }),
            emitter: None,
            movement: None,
        }];
        let placements = vec![placement("ghost", &[]), placement("ghost", &[])];
        let handled =
            apply_data_archetype_dispatch(&placements, &descriptors, &HashSet::new(), &mut reg);
        assert_eq!(handled.len(), 0);
        assert!(
            reg.iter_with_kind(crate::scripting::registry::ComponentKind::Light)
                .next()
                .is_none(),
            "descriptor with no canonical_name must not spawn from direct placement",
        );
    }

    #[test]
    fn descriptor_with_some_canonical_name_spawns_from_direct_placement() {
        // Regression guard: the canonical_name = Some(...) path still routes a
        // direct map placement to the matching descriptor.
        let mut reg = EntityRegistry::new();
        let descriptors = vec![light_descriptor("foo", true)];
        let placements = vec![placement("foo", &[])];
        let handled =
            apply_data_archetype_dispatch(&placements, &descriptors, &HashSet::new(), &mut reg);
        assert_eq!(handled.len(), 1);
        assert!(handled.contains("foo"));
        assert!(
            reg.iter_with_kind(crate::scripting::registry::ComponentKind::Light)
                .next()
                .is_some(),
        );
    }

    #[test]
    fn player_spawn_marker_routes_to_named_descriptor_via_entity_class() {
        // A `player_spawn` marker resolves its target via `entity_class` (or
        // the default `"player"`). A descriptor with canonical_name =
        // Some("player") receives the spawn — exactly one entity lands.
        let mut reg = EntityRegistry::new();
        let descriptors = vec![stub_descriptor("player")];
        let points = vec![spawn_point(&[])];

        spawn_from_player_starts(&points, &descriptors, &mut reg);

        assert_eq!(live_count(&reg), 1);
    }

    // --- spawn_from_player_starts -------------------------------------------

    /// A descriptor with no components — sufficient as a stub `"player"` entry
    /// for spawn-point tests that only care about transform / tags / KVPs.
    fn stub_descriptor(classname: &str) -> EntityTypeDescriptor {
        EntityTypeDescriptor {
            canonical_name: Some(classname.to_string()),
            light: None,
            emitter: None,
            movement: None,
        }
    }

    fn spawn_point(kvps: &[(&str, &str)]) -> MapEntity {
        placement(PLAYER_START_CLASSNAME, kvps)
    }

    fn spawn_point_at(origin: Vec3, angles: Vec3, kvps: &[(&str, &str)]) -> MapEntity {
        let mut e = spawn_point(kvps);
        e.origin = origin;
        e.angles = angles;
        e
    }

    fn live_count(reg: &EntityRegistry) -> usize {
        reg.iter_with_kind(crate::scripting::registry::ComponentKind::Transform)
            .count()
    }

    #[test]
    fn single_spawn_point_spawns_one_entity_at_position_and_facing() {
        use crate::scripting::conv::EulerDegrees;

        let mut reg = EntityRegistry::new();
        let descriptors = vec![stub_descriptor("player")];
        let origin = Vec3::new(4.0, 5.0, 6.0);
        // pitch=10°, yaw=-30°, roll=0° — exercises two axes without hitting
        // 90° boundaries where YXZ vs other orderings collapse.
        let pitch_deg: f32 = 10.0;
        let yaw_deg: f32 = -30.0;
        let roll_deg: f32 = 0.0;
        let angles = Vec3::new(
            pitch_deg.to_radians(),
            yaw_deg.to_radians(),
            roll_deg.to_radians(),
        );
        let points = vec![spawn_point_at(origin, angles, &[])];

        spawn_from_player_starts(&points, &descriptors, &mut reg);

        assert_eq!(live_count(&reg), 1);
        let (id, _) = reg
            .iter_with_kind(crate::scripting::registry::ComponentKind::Transform)
            .next()
            .unwrap();
        let t = reg.get_component::<Transform>(id).unwrap();
        assert_eq!(t.position, origin);
        // Assert rotation against a known degree input via EulerDegrees::to_quat,
        // not against rotation_quat() itself (which would be a tautology).
        let expected = EulerDegrees {
            pitch: pitch_deg,
            yaw: yaw_deg,
            roll: roll_deg,
        }
        .to_quat();
        let eps = 1e-5;
        assert!(
            (t.rotation.x - expected.x).abs() < eps
                && (t.rotation.y - expected.y).abs() < eps
                && (t.rotation.z - expected.z).abs() < eps
                && (t.rotation.w - expected.w).abs() < eps,
            "rotation mismatch: got {:?}, expected {:?}",
            t.rotation,
            expected,
        );
    }

    #[test]
    fn multiple_spawn_points_spawn_one_entity_each() {
        let mut reg = EntityRegistry::new();
        let descriptors = vec![stub_descriptor("player")];
        let points = vec![spawn_point(&[]), spawn_point(&[]), spawn_point(&[])];

        spawn_from_player_starts(&points, &descriptors, &mut reg);

        assert_eq!(live_count(&reg), 3);
    }

    #[test]
    fn entity_class_defaults_to_player_when_kvp_absent() {
        let mut reg = EntityRegistry::new();
        let descriptors = vec![stub_descriptor("player")];
        let points = vec![spawn_point(&[])];

        spawn_from_player_starts(&points, &descriptors, &mut reg);

        assert_eq!(live_count(&reg), 1);
    }

    #[test]
    fn entity_class_kvp_routes_to_named_descriptor() {
        let mut reg = EntityRegistry::new();
        let descriptors = vec![stub_descriptor("player"), stub_descriptor("spectator")];
        let points = vec![
            spawn_point(&[("entity_class", "player")]),
            spawn_point(&[("entity_class", "spectator")]),
        ];

        spawn_from_player_starts(&points, &descriptors, &mut reg);

        // Both spawn — exactly one per spawn point regardless of routing.
        assert_eq!(live_count(&reg), 2);
    }

    #[test]
    fn unknown_entity_class_is_skipped() {
        let mut reg = EntityRegistry::new();
        let descriptors = vec![stub_descriptor("player")];
        let points = vec![
            spawn_point(&[("entity_class", "ghost")]),
            spawn_point(&[]), // defaults to "player" — should still spawn
        ];

        spawn_from_player_starts(&points, &descriptors, &mut reg);

        assert_eq!(
            live_count(&reg),
            1,
            "only the spawn point with a registered entity_class should land",
        );
    }

    #[test]
    fn empty_spawn_points_list_is_a_noop() {
        let mut reg = EntityRegistry::new();
        let descriptors = vec![stub_descriptor("player")];
        spawn_from_player_starts(&[], &descriptors, &mut reg);
        assert_eq!(live_count(&reg), 0);
    }

    #[test]
    fn tags_are_forwarded_to_spawned_entity() {
        let mut reg = EntityRegistry::new();
        let descriptors = vec![stub_descriptor("player")];
        let mut sp = spawn_point(&[]);
        sp.tags = vec!["co-op".to_string(), "team-red".to_string()];
        let points = vec![sp];

        spawn_from_player_starts(&points, &descriptors, &mut reg);

        let (id, _) = reg
            .iter_with_kind(crate::scripting::registry::ComponentKind::Transform)
            .next()
            .unwrap();
        let tags = reg.get_tags(id).unwrap();
        assert_eq!(tags, &["co-op".to_string(), "team-red".to_string()]);
    }

    #[test]
    fn custom_kvps_are_forwarded_with_entity_class_stripped() {
        let mut reg = EntityRegistry::new();
        let descriptors = vec![stub_descriptor("player")];
        let points = vec![spawn_point(&[
            ("entity_class", "player"),
            ("loadout", "shotgun"),
        ])];

        spawn_from_player_starts(&points, &descriptors, &mut reg);

        let (id, _) = reg
            .iter_with_kind(crate::scripting::registry::ComponentKind::Transform)
            .next()
            .unwrap();
        // Custom KVP available via the same bag data-archetype-spawned entities use.
        assert_eq!(
            reg.get_map_kvp(id, "loadout").unwrap().as_deref(),
            Some("shotgun"),
        );
        // `entity_class` is a routing hint, not a runtime property — stripped.
        assert_eq!(reg.get_map_kvp(id, "entity_class").unwrap(), None);
    }

    #[test]
    fn descriptor_components_attach_to_player_start_spawn() {
        // A `"player"` archetype carrying a light descriptor should produce a
        // light-bearing entity at the spawn point.
        let mut reg = EntityRegistry::new();
        let descriptors = vec![light_descriptor("player", true)];
        let points = vec![spawn_point_at(Vec3::new(7.0, 0.0, 0.0), Vec3::ZERO, &[])];

        spawn_from_player_starts(&points, &descriptors, &mut reg);

        let (id, _) = reg
            .iter_with_kind(crate::scripting::registry::ComponentKind::Light)
            .next()
            .expect("descriptor light should attach to spawn-point entity");
        let light = reg.get_component::<LightComponent>(id).unwrap();
        assert!(light.is_dynamic);
        assert_eq!(light.origin, [7.0, 0.0, 0.0]);
    }
}
