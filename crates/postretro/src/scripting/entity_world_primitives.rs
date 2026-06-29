// Entity/world scripting primitive handlers and registration.
// See: context/lib/scripting.md

use crate::scripting::ctx::ScriptCtx;
use crate::scripting::error::ScriptError;
use crate::scripting::primitives::entity::NullableString;
use crate::scripting::primitives::world::{JsonValue, WorldQueryFilterInput};
use crate::scripting::primitives_registry::{ContextScope, PrimitiveRegistry};
use crate::scripting::registry::{ComponentKind, ComponentValue, EntityId, Transform};

pub(crate) fn register_entity_primitives(registry: &mut PrimitiveRegistry, ctx: ScriptCtx) {
    registry
        .register("entityExists", {
            let ctx = ctx.clone();
            move |id: EntityId| -> Result<bool, ScriptError> { entity_exists(&ctx, id) }
        })
        .scope(ContextScope::Both)
        .doc("Returns true if the entity id refers to a live entity.")
        .param("id", "EntityId")
        .finish();

    registry
        .register("getEntityProperty", {
            let ctx = ctx.clone();
            move |id: EntityId, key: String| -> Result<NullableString, ScriptError> {
                get_entity_property(&ctx, id, &key).map(NullableString)
            }
        })
        .scope(ContextScope::Both)
        .doc("Reads a per-placement KVP value authored on the source `.map` entity. Returns null when the key is absent or the entity has no KVP bag (e.g. runtime-spawned). Available in definition and data contexts.")
        .param("id", "EntityId")
        .param("key", "String")
        .finish();
}

pub(crate) fn entity_exists(ctx: &ScriptCtx, id: EntityId) -> Result<bool, ScriptError> {
    Ok(ctx.registry.borrow().exists(id))
}

pub(crate) fn get_entity_property(
    ctx: &ScriptCtx,
    id: EntityId,
    key: &str,
) -> Result<Option<String>, ScriptError> {
    let reg = ctx.registry.borrow();
    Ok(reg.get_map_kvp(id, key)?)
}

/// Parsed and validated form of the filter passed to `worldQuery`.
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

/// Parse the filter object passed to `worldQuery`. Unknown component names
/// surface as `InvalidArgument`.
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

const WORLD_GET_GRAVITY_DOC: &str = "Return the current world gravity in m/s² (negative = downward; positive = upward). \
     Seeded from the worldspawn `initialGravity` KVP at level load and persists until the next level load or a `worldSetGravity` call. \
     The `world.ts` vocabulary module wraps this as `world.getGravity`.";

const WORLD_SET_GRAVITY_DOC: &str = "Set the world gravity in m/s² (negative = downward; positive = upward). \
     NaN and non-finite values are silently ignored (a warning is logged) so a misbehaving script cannot wedge particle physics. \
     Effect is immediate and persists until the next level load or another `worldSetGravity` call. \
     The `world.ts` vocabulary module wraps this as `world.setGravity`.";

/// Collect transform handles as JSON. Every live entity carries `Transform`,
/// so this is effectively an entity query filtered only by tag.
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

/// Collect billboard-emitter handles as JSON. `BillboardEmitterComponent` has
/// `#[serde(rename_all = "snake_case")]` so direct serialization gives the wire
/// field names without a manual mapping.
fn collect_emitter_handles_json(ctx: &ScriptCtx, tag: Option<&str>) -> serde_json::Value {
    use serde_json::{Map, Value};
    let reg = ctx.registry.borrow();
    let mut arr: Vec<Value> = Vec::new();
    for (id, value) in reg.query_by_component_and_tag(ComponentKind::BillboardEmitter, tag) {
        let ComponentValue::BillboardEmitter(e) = value else {
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

/// Collect fog-volume handles as JSON. The component object is hand-rolled via
/// `camel_fields()` rather than serde so the script-facing camelCase keys don't
/// require a wire-affecting `#[serde(rename)]` on the struct.
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
            let mut c = Map::with_capacity(7);
            for (key, value) in f.camel_fields() {
                c.insert(key.to_string(), Value::from(value as f64));
            }
            c.insert(
                "tint".to_string(),
                Value::Array(
                    f.tint
                        .iter()
                        .map(|x| Value::from(*x as f64))
                        .collect::<Vec<_>>(),
                ),
            );
            // `animation` crosses through serde so its camelCase wire shape
            // (periodMs, playCount) lands without manual mapping; absent
            // becomes JSON `null` (script-side `null` / Luau `nil`).
            let anim_json = match f.animation.as_ref() {
                Some(anim) => serde_json::to_value(anim).expect("FogAnimation always serializes"),
                None => Value::Null,
            };
            c.insert("animation".to_string(), anim_json);
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

/// Register the world-domain primitives: `worldQuery`, `worldGetGravity`, and
/// `worldSetGravity`. All three install in both definition and data contexts.
pub(crate) fn register_world_primitives(registry: &mut PrimitiveRegistry, ctx: ScriptCtx) {
    register_world_query(registry, ctx.clone());
    register_world_gravity(registry, ctx);
}

// Lives in world.rs because it dispatches across all component domains; per-domain helpers stay in their sibling primitive modules.
fn register_world_query(registry: &mut PrimitiveRegistry, ctx: ScriptCtx) {
    use crate::scripting::primitives::light;
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

fn register_world_gravity(registry: &mut PrimitiveRegistry, ctx: ScriptCtx) {
    // worldGetGravity ------------------------------------------------------
    registry
        .register("worldGetGravity", {
            let ctx = ctx.clone();
            move || -> Result<f32, ScriptError> { Ok(ctx.gravity.get()) }
        })
        .scope(ContextScope::Both)
        .doc(WORLD_GET_GRAVITY_DOC)
        .finish();

    // worldSetGravity ------------------------------------------------------
    registry
        .register("worldSetGravity", {
            move |value: f32| -> Result<(), ScriptError> {
                if !value.is_finite() {
                    log::warn!("[Scripting] world.setGravity: rejected non-finite value");
                    return Ok(());
                }
                ctx.gravity.set(value);
                Ok(())
            }
        })
        .scope(ContextScope::Both)
        .doc(WORLD_SET_GRAVITY_DOC)
        .param("value", "f32")
        .finish();
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scripting::primitives_registry::PrimitiveRegistry;

    fn registry_with_gravity() -> (PrimitiveRegistry, ScriptCtx) {
        let ctx = ScriptCtx::new();
        let mut r = PrimitiveRegistry::new();
        register_world_gravity(&mut r, ctx.clone());
        (r, ctx)
    }

    #[test]
    fn get_gravity_reflects_seeded_value_from_quickjs() {
        let (r, ctx) = registry_with_gravity();
        ctx.gravity.set(-7.5);

        let rt = rquickjs::Runtime::new().unwrap();
        let jsctx = rquickjs::Context::full(&rt).unwrap();
        jsctx.with(|jsctx| {
            for p in r.iter() {
                (p.quickjs_installer)(&jsctx).unwrap();
            }
            let got: f64 = jsctx.eval("worldGetGravity()").unwrap();
            assert!((got - -7.5).abs() < 1e-5, "got {got}");
        });
    }

    #[test]
    fn set_gravity_updates_value_via_quickjs() {
        let (r, ctx) = registry_with_gravity();
        ctx.gravity.set(-9.81);

        let rt = rquickjs::Runtime::new().unwrap();
        let jsctx = rquickjs::Context::full(&rt).unwrap();
        jsctx.with(|jsctx| {
            for p in r.iter() {
                (p.quickjs_installer)(&jsctx).unwrap();
            }
            let _: () = jsctx.eval("worldSetGravity(3.5)").unwrap();
        });
        assert!((ctx.gravity.get() - 3.5).abs() < 1e-5);
    }

    #[test]
    fn set_gravity_ignores_nan_and_infinity() {
        let (r, ctx) = registry_with_gravity();
        ctx.gravity.set(-2.0);

        let rt = rquickjs::Runtime::new().unwrap();
        let jsctx = rquickjs::Context::full(&rt).unwrap();
        jsctx.with(|jsctx| {
            for p in r.iter() {
                (p.quickjs_installer)(&jsctx).unwrap();
            }
            let _: () = jsctx.eval("worldSetGravity(NaN)").unwrap();
            let _: () = jsctx.eval("worldSetGravity(Infinity)").unwrap();
            let _: () = jsctx.eval("worldSetGravity(-Infinity)").unwrap();
        });
        assert_eq!(ctx.gravity.get(), -2.0);
    }

    #[test]
    fn get_gravity_callable_from_luau() {
        let (r, ctx) = registry_with_gravity();
        ctx.gravity.set(-12.0);

        let lua = mlua::Lua::new();
        for p in r.iter() {
            (p.luau_installer)(&lua).unwrap();
        }
        let got: f64 = lua.load("return worldGetGravity()").eval().unwrap();
        assert!((got - -12.0).abs() < 1e-5);
    }

    #[test]
    fn set_gravity_updates_value_via_luau() {
        let (r, ctx) = registry_with_gravity();
        ctx.gravity.set(-9.81);

        let lua = mlua::Lua::new();
        for p in r.iter() {
            (p.luau_installer)(&lua).unwrap();
        }
        let _: () = lua.load("worldSetGravity(-5.0)").eval().unwrap();
        assert!((ctx.gravity.get() - -5.0).abs() < 1e-6);
    }

    #[test]
    fn set_gravity_ignores_nan_and_infinity_via_luau() {
        let (r, ctx) = registry_with_gravity();
        ctx.gravity.set(-2.0);

        let lua = mlua::Lua::new();
        for p in r.iter() {
            (p.luau_installer)(&lua).unwrap();
        }
        let _: () = lua.load("worldSetGravity(math.huge)").eval().unwrap();
        let _: () = lua.load("worldSetGravity(-math.huge)").eval().unwrap();
        let _: () = lua.load("worldSetGravity(0/0)").eval().unwrap();
        assert!((ctx.gravity.get() - -2.0).abs() < 1e-6);
    }
}
