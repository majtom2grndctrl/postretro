// Scripting primitives composition root.
// See: context/lib/scripting.md
//
// Per-domain primitive registration lives in sibling modules (`entity`,
// `light`); this file owns the shared types, the cross-domain `worldQuery`
// primitive, and the `register_all` entry point that the engine and tests
// converge on.

pub(crate) mod entity;
pub(crate) mod light;

use crate::scripting::conv::{json_to_js, json_to_lua};
use crate::scripting::ctx::ScriptCtx;
use crate::scripting::error::ScriptError;
use crate::scripting::primitives_registry::{ContextScope, PrimitiveRegistry};
use crate::scripting::registry::{ComponentKind, ComponentValue, Transform};
use mlua::{FromLua, IntoLua, Lua, Value as LuaValue};
use rquickjs::{Ctx, FromJs, IntoJs, Value as JsValue};

// --- worldQuery -------------------------------------------------------------
//
// `worldQuery` is a generic ECS query primitive. It lives here at the
// composition root because it spans multiple component domains (light,
// transform, emitter, fog volume); per-domain helpers live in the sibling
// `entity` and `light` modules.

/// Opaque newtype implementing `IntoJs` and `IntoLua` so we can return a
/// serde_json-shaped value from a primitive closure without the caller having
/// to write the `for<'js>` lifetime on the closure by hand — rquickjs'
/// `IntoJsFunc` derives the HRTB from the impl.
struct JsonValue(serde_json::Value);

impl<'js> IntoJs<'js> for JsonValue {
    fn into_js(self, ctx: &Ctx<'js>) -> rquickjs::Result<JsValue<'js>> {
        json_to_js(ctx, &self.0)
    }
}

impl IntoLua for JsonValue {
    fn into_lua(self, lua: &Lua) -> mlua::Result<LuaValue> {
        json_to_lua(lua, &self.0)
    }
}

/// Filter object adapter with FromJs / FromLua impls so the primitive
/// declares a typed parameter instead of manually walking an `Object`.
struct WorldQueryFilterInput {
    component: String,
    tag: Option<String>,
}

impl<'js> FromJs<'js> for WorldQueryFilterInput {
    fn from_js(_ctx: &Ctx<'js>, value: JsValue<'js>) -> rquickjs::Result<Self> {
        let obj = rquickjs::Object::from_value(value)
            .map_err(|_| rquickjs::Error::new_from_js("value", "WorldQueryFilter object"))?;
        let component: String = obj.get("component")?;
        let tag: Option<String> = obj.get("tag")?;
        Ok(Self { component, tag })
    }
}

impl FromLua for WorldQueryFilterInput {
    fn from_lua(value: LuaValue, _lua: &Lua) -> mlua::Result<Self> {
        let t = match value {
            LuaValue::Table(t) => t,
            other => {
                return Err(mlua::Error::FromLuaConversionError {
                    from: other.type_name(),
                    to: "WorldQueryFilter".to_string(),
                    message: Some("expected a table".to_string()),
                });
            }
        };
        let component: String = t.get("component")?;
        let tag: Option<String> = t.get("tag")?;
        Ok(Self { component, tag })
    }
}

/// Parsed and validated form of the filter passed to `worldQuery`. Extend
/// with new variants as additional queryable component types are added.
enum QueryFilter {
    Light {
        tag: Option<String>,
    },
    Transform {
        tag: Option<String>,
    },
    Emitter {
        tag: Option<String>,
    },
    FogVolume {
        tag: Option<String>,
    },
    /// Always returns an empty array. Particles and sprite-visuals are
    /// engine-managed; scripts have no business iterating individual ones.
    AlwaysEmpty,
}

/// Parse the filter object passed to `worldQuery`. Returns the component
/// kind and the optional tag string. Unknown component names surface as
/// `InvalidArgument`.
fn parse_query_filter(component: &str, tag: Option<String>) -> Result<QueryFilter, ScriptError> {
    match component {
        "light" => Ok(QueryFilter::Light { tag }),
        "transform" => Ok(QueryFilter::Transform { tag }),
        "emitter" => Ok(QueryFilter::Emitter { tag }),
        "fog_volume" => Ok(QueryFilter::FogVolume { tag }),
        "particle" | "sprite_visual" => Ok(QueryFilter::AlwaysEmpty),
        other => Err(ScriptError::InvalidArgument {
            reason: format!(
                "worldQuery: unknown component `{other}`; supported: \
                 \"light\" | \"transform\" | \"emitter\" | \"fog_volume\" | \"particle\" | \"sprite_visual\""
            ),
        }),
    }
}

const WORLD_QUERY_DOC: &str = "Return an array of entity handles matching the filter. Available in definition and data contexts. \
     Filter shape: { component: \"light\" | \"transform\" | \"emitter\" | \"fog_volume\" | \"particle\" | \"sprite_visual\", tag?: string }. \
     `\"particle\"` and `\"sprite_visual\"` always return `[]` (engine-managed; scripts never iterate individual particles). \
     Unknown component values raise InvalidArgument. \
     The `world.ts` vocabulary module wraps this as `world.query`.";

/// Build the JSON shape `[{ id, position, tags }, ...]` for every entity that
/// carries a `Transform` component (every live entity does), filtered by tag
/// if `tag` is `Some`.
fn collect_transform_handles_json(ctx: &ScriptCtx, tag: Option<&str>) -> serde_json::Value {
    use serde_json::{Map, Value};
    let reg = ctx.registry.borrow();
    let mut arr: Vec<Value> = Vec::new();
    for (id, value) in reg.query_by_component_and_tag(ComponentKind::Transform, tag) {
        let ComponentValue::Transform(t) = value else {
            continue;
        };
        let tags = reg.get_tags(id).unwrap_or(&[]).to_vec();
        let mut obj = Map::with_capacity(3);
        obj.insert("id".to_string(), Value::from(id.to_raw()));
        let mut position = Map::with_capacity(3);
        position.insert("x".to_string(), Value::from(t.position.x as f64));
        position.insert("y".to_string(), Value::from(t.position.y as f64));
        position.insert("z".to_string(), Value::from(t.position.z as f64));
        obj.insert("position".to_string(), Value::Object(position));
        obj.insert(
            "tags".to_string(),
            Value::Array(tags.into_iter().map(Value::String).collect()),
        );
        arr.push(Value::Object(obj));
    }
    Value::Array(arr)
}

/// Build the JSON shape `[{ id, position, tags, component: {...} }, ...]` for
/// every entity carrying a `BillboardEmitterComponent`. `BillboardEmitterComponent`
/// already has `#[serde(rename_all = "snake_case")]` so direct serialization
/// gives the wire field names.
fn collect_emitter_handles_json(ctx: &ScriptCtx, tag: Option<&str>) -> serde_json::Value {
    use serde_json::{Map, Value};
    let reg = ctx.registry.borrow();
    let mut arr: Vec<Value> = Vec::new();
    for (id, value) in reg.query_by_component_and_tag(ComponentKind::BillboardEmitter, tag) {
        let ComponentValue::BillboardEmitter(e) = value else {
            continue;
        };
        let tags = reg.get_tags(id).unwrap_or(&[]).to_vec();
        // Position lives on the entity's Transform component, not on the
        // emitter — read it separately.
        let position = match reg.get_component::<Transform>(id) {
            Ok(t) => {
                let mut p = Map::with_capacity(3);
                p.insert("x".to_string(), Value::from(t.position.x as f64));
                p.insert("y".to_string(), Value::from(t.position.y as f64));
                p.insert("z".to_string(), Value::from(t.position.z as f64));
                Value::Object(p)
            }
            Err(_) => Value::Null,
        };
        let comp = serde_json::to_value(e).expect("BillboardEmitterComponent always serializes");
        let mut obj = Map::with_capacity(4);
        obj.insert("id".to_string(), Value::from(id.to_raw()));
        obj.insert("position".to_string(), position);
        obj.insert(
            "tags".to_string(),
            Value::Array(tags.into_iter().map(Value::String).collect()),
        );
        obj.insert("component".to_string(), comp);
        arr.push(Value::Object(obj));
    }
    Value::Array(arr)
}

/// Build the JSON shape `[{ id, position, tags, component: { density,
/// scatter, edgeSoftness, falloff } }, ...]` for every entity carrying a
/// `FogVolumeComponent`. Position is read from the entity's `Transform`
/// (volume center, baked at level load). The component object is hand-rolled
/// rather than driven by serde so the script-facing camelCase boundary
/// (`edgeSoftness`) does not require a wire-affecting `#[serde(rename)]`.
fn collect_fog_volume_handles_json(ctx: &ScriptCtx, tag: Option<&str>) -> serde_json::Value {
    use serde_json::{Map, Value};
    let reg = ctx.registry.borrow();
    let mut arr: Vec<Value> = Vec::new();
    for (id, value) in reg.query_by_component_and_tag(ComponentKind::FogVolume, tag) {
        let ComponentValue::FogVolume(f) = value else {
            continue;
        };
        let tags = reg.get_tags(id).unwrap_or(&[]).to_vec();
        let position = match reg.get_component::<Transform>(id) {
            Ok(t) => {
                let mut p = Map::with_capacity(3);
                p.insert("x".to_string(), Value::from(t.position.x as f64));
                p.insert("y".to_string(), Value::from(t.position.y as f64));
                p.insert("z".to_string(), Value::from(t.position.z as f64));
                Value::Object(p)
            }
            Err(_) => Value::Null,
        };
        let comp = {
            let mut c = Map::with_capacity(4);
            for (key, value) in f.camel_fields() {
                c.insert(key.to_string(), Value::from(value as f64));
            }
            Value::Object(c)
        };
        let mut obj = Map::with_capacity(4);
        obj.insert("id".to_string(), Value::from(id.to_raw()));
        obj.insert("position".to_string(), position);
        obj.insert(
            "tags".to_string(),
            Value::Array(tags.into_iter().map(Value::String).collect()),
        );
        obj.insert("component".to_string(), comp);
        arr.push(Value::Object(obj));
    }
    Value::Array(arr)
}

pub(crate) fn register_world_query(registry: &mut PrimitiveRegistry, ctx: ScriptCtx) {
    // Generic registration path: because `WorldQueryFilterInput: FromJs + FromLua`
    // and the returned `JsonValue: IntoJs + IntoLua`, rquickjs / mlua both
    // derive the HRTB lifetime bounds from their respective conversion traits.
    // That side-steps the `'_`-inside-closure lifetime problem writing a raw
    // `rquickjs::Ctx<'_> -> Value<'_>` closure would hit.
    registry
        .register("worldQuery", {
            let ctx = ctx.clone();
            move |filter: WorldQueryFilterInput| -> Result<JsonValue, ScriptError> {
                let filter = parse_query_filter(&filter.component, filter.tag)?;
                match filter {
                    QueryFilter::Light { tag } => {
                        let handles = light::collect_light_handles(&ctx, tag.as_deref());
                        Ok(JsonValue(light::handles_to_json(handles)))
                    }
                    QueryFilter::Transform { tag } => Ok(JsonValue(
                        collect_transform_handles_json(&ctx, tag.as_deref()),
                    )),
                    QueryFilter::Emitter { tag } => Ok(JsonValue(collect_emitter_handles_json(
                        &ctx,
                        tag.as_deref(),
                    ))),
                    QueryFilter::FogVolume { tag } => Ok(JsonValue(
                        collect_fog_volume_handles_json(&ctx, tag.as_deref()),
                    )),
                    QueryFilter::AlwaysEmpty => Ok(JsonValue(serde_json::Value::Array(Vec::new()))),
                }
            }
        })
        .scope(ContextScope::Both)
        .doc(WORLD_QUERY_DOC)
        .param("filter", "WorldQueryFilter")
        .finish();
}

/// Register the shared types referenced by day-one primitive signatures. These
/// feed the typedef generator (see: context/lib/scripting.md §7).
pub(crate) fn register_shared_types(registry: &mut PrimitiveRegistry) {
    registry.register_type("EntityId").brand("number").finish();
    registry
        .register_type("Vec3")
        .field("x", "f32", "")
        .field("y", "f32", "")
        .field("z", "f32", "")
        .finish();
    registry
        .register_type("EulerDegrees")
        .field("pitch", "f32", "")
        .field("yaw", "f32", "")
        .field("roll", "f32", "")
        .finish();
    registry
        .register_type("Transform")
        .field("position", "Vec3", "")
        .field("rotation", "EulerDegrees", "")
        .field("scale", "Vec3", "")
        .finish();
    registry
        .register_enum("ComponentKind")
        .variant("transform", "")
        .variant("light", "")
        .variant("billboard_emitter", "")
        .variant("particle_state", "")
        .variant("sprite_visual", "")
        .variant("fog_volume", "")
        .finish();
    registry
        .register_tagged_union("ComponentValue")
        .flat()
        .variant("transform", "Transform", "")
        .variant("light", "LightComponent", "")
        .variant("billboard_emitter", "BillboardEmitterComponent", "")
        .variant("particle_state", "ParticleState", "")
        .variant("sprite_visual", "SpriteVisual", "")
        .variant("fog_volume", "FogVolumeComponent", "")
        .finish();
    registry
        .register_type("LightDescriptor")
        .doc("Authored light component preset attached to `EntityTypeDescriptor.components.light`. Field names are snake_case across the FFI.")
        .field("color", "Vec3", "RGB color in [0, 1].")
        .field("intensity", "f32", "Static intensity scalar.")
        .field(
            "range",
            "f32",
            "Falloff range (maps onto LightComponent.falloffRange at spawn).",
        )
        .field(
            "is_dynamic",
            "bool",
            "Author hint; descriptor-spawned lights are always treated as dynamic at spawn (baked indirect not supported).",
        )
        .finish();
    registry
        .register_type("EntityTypeDescriptor")
        .doc("Argument shape for `registerEntity`. `components` is an optional sub-object carrying typed component presets.")
        .field("classname", "String", "FGD classname this descriptor binds to.")
        .field(
            "components?",
            "EntityTypeComponents",
            "Optional component presets attached at level-load spawn.",
        )
        .finish();
    registry
        .register_type("BillboardEmitterComponent")
        .doc("Engine-managed billboard emitter component shape. Carried by `BillboardEmitter` ECS entities and produced by SDK `emitter()`/`smokeEmitter()`/etc.")
        .field("rate", "f32", "Continuous spawn rate (particles/sec). 0 = inactive.")
        .field("burst", "Option<u32>", "")
        .field("spread", "f32", "")
        .field("lifetime", "f32", "")
        .field("velocity", "Vec3", "")
        .field("buoyancy", "f32", "")
        .field("drag", "f32", "")
        .field("size_over_lifetime", "Vec<f32>", "")
        .field("opacity_over_lifetime", "Vec<f32>", "")
        .field("color", "Vec3", "")
        .field("sprite", "String", "")
        .field("spin_rate", "f32", "")
        .field("spin_animation", "Option<SpinAnimation>", "")
        .finish();
    registry
        .register_type("SpinAnimation")
        .doc("Spin tween shape consumed by `setSpinRate`.")
        .field("duration", "f32", "")
        .field("rate_curve", "Vec<f32>", "")
        .finish();
    registry
        .register_type("FogVolumeComponent")
        .doc("Script-facing fog-volume component shape. Carried by `FogVolume` ECS entities; the AABB is baked at level load and lives in the FogVolumeBridge side-table — it is not exposed here because it is not runtime-settable.")
        .field("density", "f32", "Volumetric fog density inside the AABB.")
        .field("scatter", "f32", "Fraction of in-scattering toward the camera.")
        .field("edgeSoftness", "f32", "Edge softness in world units: 0 = hard cutoff at the brush face, larger = wider linear ramp inward from each face.")
        .field("falloff", "f32", "Radial falloff exponent. Consulted by the radial (`fog_lamp`, `fog_tube`) and ellipsoid (`fog_ellipsoid`) shader paths; stored but ignored by the plane-sweep `fog_volume` path.")
        .finish();
    registry
        .register_type("FogVolumeEntity")
        .doc("Entity handle returned by `world.query` when filtering for fog-volume entities.")
        .field("id", "EntityId", "")
        .field(
            "position",
            "Vec3",
            "Volume center at query time (AABB midpoint, baked at level load).",
        )
        .field(
            "tags",
            "Vec<String>",
            "The entity's tags at query time. Empty array if untagged.",
        )
        .field(
            "component",
            "FogVolumeComponent",
            "Full fog-volume component snapshot at query time.",
        )
        .finish();
    registry
        .register_type("EntityTypeComponents")
        .doc("Optional bag of component presets carried by `EntityTypeDescriptor.components`.")
        .field("light?", "Option<LightDescriptor>", "")
        .field("emitter?", "Option<BillboardEmitterComponent>", "")
        .finish();
    registry
        .register_type("ModManifest")
        .doc("Object returned from `setupMod()` in `start-script.{ts,luau}`. Identifies the mod to the engine.")
        .field("name", "String", "Human-readable mod name. Required.")
        .finish();
}

/// Register all engine primitives and shared types. Called at engine startup,
/// before any script runtime is created.
pub(crate) fn register_all(registry: &mut PrimitiveRegistry, ctx: ScriptCtx) {
    register_shared_types(registry);
    light::register_shared_types(registry);
    light::register_light_entity_primitives(registry, ctx.clone());
    register_world_query(registry, ctx.clone());
    entity::register_entity_primitives(registry, ctx);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scripting::ctx::ScriptCtx;

    fn registry_with_day_one() -> (PrimitiveRegistry, ScriptCtx) {
        let ctx = ScriptCtx::new();
        let mut r = PrimitiveRegistry::new();
        register_all(&mut r, ctx.clone());
        (r, ctx)
    }

    #[test]
    fn register_all_installs_expected_primitives() {
        let (r, _ctx) = registry_with_day_one();
        let names: Vec<_> = r.iter().map(|p| p.name).collect();
        for expected in [
            "entityExists",
            "worldQuery",
            "setLightAnimation",
            "getEntityProperty",
            "registerEntity",
        ] {
            assert!(names.contains(&expected), "missing primitive {expected}");
        }
        // The Live VM primitives are gone — they must NOT appear.
        for forbidden in [
            "spawnEntity",
            "despawnEntity",
            "getComponent",
            "setComponent",
            "emitEvent",
            "sendEvent",
            "registerHandler",
        ] {
            assert!(
                !names.contains(&forbidden),
                "primitive {forbidden} must be removed",
            );
        }
    }

    #[test]
    fn entity_exists_callable_from_quickjs_and_matches_registry() {
        let (r, ctx) = registry_with_day_one();
        // Seed a live entity from Rust so we have a known-valid id.
        let id = ctx.registry.borrow_mut().spawn(Transform::default());
        let raw = id.to_raw();

        let rt = rquickjs::Runtime::new().unwrap();
        let jsctx = rquickjs::Context::full(&rt).unwrap();
        jsctx.with(|jsctx| {
            for p in r.iter() {
                (p.quickjs_installer)(&jsctx).unwrap();
            }
            let got_live: bool = jsctx.eval(format!("entityExists({raw})")).unwrap();
            assert!(got_live);

            let got_bogus: bool = jsctx
                .eval(format!("entityExists({})", raw.wrapping_add(1)))
                .unwrap();
            // raw+1 changes the low-16 index bits — a different, unallocated slot.
            assert!(!got_bogus);
        });
    }

    #[test]
    fn get_entity_property_returns_value_from_quickjs_when_set() {
        use std::collections::HashMap;
        let (r, ctx) = registry_with_day_one();

        // Spawn a fresh entity from Rust and seed its KVP bag the way a
        // built-in classname handler would after a level-load dispatch.
        let id = ctx.registry.borrow_mut().spawn(Transform::default());
        let mut kv = HashMap::new();
        kv.insert("wave".to_string(), "3".to_string());
        ctx.registry.borrow_mut().set_map_kvps(id, kv).unwrap();
        let raw = id.to_raw();

        let rt = rquickjs::Runtime::new().unwrap();
        let jsctx = rquickjs::Context::full(&rt).unwrap();
        jsctx.with(|jsctx| {
            for p in r.iter() {
                (p.quickjs_installer)(&jsctx).unwrap();
            }
            let got: String = jsctx
                .eval(format!("getEntityProperty({raw}, 'wave')"))
                .unwrap();
            assert_eq!(got, "3");
        });
    }

    #[test]
    fn get_entity_property_returns_null_for_unknown_key() {
        let (r, ctx) = registry_with_day_one();
        let id = ctx.registry.borrow_mut().spawn(Transform::default());
        let raw = id.to_raw();

        let rt = rquickjs::Runtime::new().unwrap();
        let jsctx = rquickjs::Context::full(&rt).unwrap();
        jsctx.with(|jsctx| {
            for p in r.iter() {
                (p.quickjs_installer)(&jsctx).unwrap();
            }
            // Entity exists but has no KVP bag — script sees `null`.
            let got: bool = jsctx
                .eval(format!("getEntityProperty({raw}, 'missing') === null"))
                .unwrap();
            assert!(got);
        });
    }

    #[test]
    fn get_entity_property_returns_null_for_entity_with_empty_kvp_bag() {
        // An entity spawned from a map placement but with an empty KVP map
        // writes no entry to the KVP side-table. `getEntityProperty` must
        // return null (not an error) for any key on such an entity — the
        // code path differs from "key absent from a non-empty bag".
        use std::collections::HashMap;
        let (r, ctx) = registry_with_day_one();

        let id = ctx.registry.borrow_mut().spawn(Transform::default());
        // Install an empty KVP bag — simulates a map entity with no authored properties.
        ctx.registry
            .borrow_mut()
            .set_map_kvps(id, HashMap::new())
            .unwrap();
        let raw = id.to_raw();

        let rt = rquickjs::Runtime::new().unwrap();
        let jsctx = rquickjs::Context::full(&rt).unwrap();
        jsctx.with(|jsctx| {
            for p in r.iter() {
                (p.quickjs_installer)(&jsctx).unwrap();
            }
            let got: bool = jsctx
                .eval(format!("getEntityProperty({raw}, 'anyKey') === null"))
                .unwrap();
            assert!(got);
        });
    }

    #[test]
    fn entity_exists_callable_from_luau_and_matches_registry() {
        let (r, ctx) = registry_with_day_one();
        let id = ctx.registry.borrow_mut().spawn(Transform::default());
        let raw = id.to_raw();

        let lua = mlua::Lua::new();
        for p in r.iter() {
            (p.luau_installer)(&lua).unwrap();
        }
        let got_live: bool = lua
            .load(format!("return entityExists({raw})"))
            .eval()
            .unwrap();
        assert!(got_live);
    }
}
