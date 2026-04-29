// Day-one primitives registered at engine startup, and the shared type
// definitions they reference.
// See: context/lib/scripting.md
//
// Every primitive captures `ScriptCtx` by `Rc` at registration time. To add a
// new subsystem to the scripting surface: add one field to `ScriptCtx` and a
// few `.register(...)` lines here.

use super::components::billboard_emitter::BillboardEmitterComponent;
use super::components::light::LightComponent;
use super::components::particle::ParticleState;
use super::components::sprite_visual::SpriteVisual;
use super::conv::{json_to_js, json_to_lua};
use super::ctx::{ScriptCtx, ScriptEvent};
use super::error::ScriptError;
use super::primitives_registry::{ContextScope, PrimitiveRegistry};
use super::registry::{ComponentKind, ComponentValue, EntityId, Transform};
use mlua::{FromLua, IntoLua, Lua, Value as LuaValue};
use rquickjs::{Ctx, FromJs, IntoJs, Value as JsValue};

// --- worldQuery -------------------------------------------------------------
//
// `worldQuery` is a generic ECS query primitive. It lives here alongside the
// other entity primitives (`entityExists`, `spawnEntity`, `getComponent`)
// so that adding a second queryable component type only requires editing this
// file, not the light-specific one.

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
    Light { tag: Option<String> },
}

/// Parse the filter object passed to `worldQuery`. Returns the component
/// kind and the optional tag string. Unknown component names surface as
/// `InvalidArgument`.
fn parse_query_filter(component: &str, tag: Option<String>) -> Result<QueryFilter, ScriptError> {
    match component {
        "light" => Ok(QueryFilter::Light { tag }),
        other => Err(ScriptError::InvalidArgument {
            reason: format!(
                "worldQuery: unknown component `{other}`; supported components: \"light\""
            ),
        }),
    }
}

const WORLD_QUERY_DOC: &str = "Return an array of entity handles matching the filter. Behavior context only. \
     Filter shape: { component: string, tag?: string } where `component` names the \
     component type to query. Only \"light\" is supported in the current build; \
     other values return an InvalidArgument error. \
     The `world.ts` vocabulary module wraps this as `world.query`.";

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
                        let handles =
                            super::primitives_light::collect_light_handles(&ctx, tag.as_deref());
                        Ok(JsonValue(super::primitives_light::handles_to_json(handles)))
                    }
                }
            }
        })
        .scope(ContextScope::BehaviorOnly)
        .doc(WORLD_QUERY_DOC)
        .param("filter", "WorldQueryFilter")
        .finish();
}

/// Register the shared types referenced by day-one primitive signatures. These
/// feed the typedef generator (see: context/lib/scripting.md §7). No type-level
/// or field-level doc strings on day-one types — docs land per-plan as field
/// semantics are pinned down (e.g. plan 2 light types).
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
        .variant("Transform", "")
        .variant("Light", "")
        .finish();
    registry
        .register_tagged_union("ComponentValue")
        .variant("Transform", "Transform", "")
        .variant("Light", "LightComponent", "")
        .finish();
    registry
        .register_type("ScriptEvent")
        .field("kind", "String", "")
        .field("payload", "Any", "")
        .finish();
}

/// Register the seven day-one primitives. Called at engine startup, before
/// any script runtime is created.
pub(crate) fn register_all(registry: &mut PrimitiveRegistry, ctx: ScriptCtx) {
    register_shared_types(registry);
    super::event_dispatch::register_shared_types(registry);
    super::event_dispatch::register_register_handler(registry, ctx.handlers.clone());
    super::primitives_light::register_shared_types(registry);
    super::primitives_light::register_light_entity_primitives(registry, ctx.clone());
    register_world_query(registry, ctx.clone());

    // entityExists ---------------------------------------------------------
    registry
        .register("entityExists", {
            let ctx = ctx.clone();
            move |id: EntityId| -> Result<bool, ScriptError> {
                Ok(ctx.registry.borrow().exists(id))
            }
        })
        .scope(ContextScope::Both)
        .doc("Returns true if the entity id refers to a live entity.")
        .param("id", "EntityId")
        .finish();

    // spawnEntity ----------------------------------------------------------
    registry
        .register("spawnEntity", {
            let ctx = ctx.clone();
            move |transform: Transform| -> Result<EntityId, ScriptError> {
                ctx.registry
                    .borrow_mut()
                    .try_spawn(transform)
                    .ok_or_else(|| ScriptError::InvalidArgument {
                        reason: "entity slots exhausted".into(),
                    })
            }
        })
        .scope(ContextScope::BehaviorOnly)
        .doc("Spawns a new entity with the given transform and returns its id.")
        .param("transform", "Transform")
        .finish();

    // despawnEntity --------------------------------------------------------
    registry
        .register("despawnEntity", {
            let ctx = ctx.clone();
            move |id: EntityId| -> Result<(), ScriptError> {
                ctx.registry.borrow_mut().despawn(id)?;
                Ok(())
            }
        })
        .scope(ContextScope::BehaviorOnly)
        .doc("Despawns a previously-spawned entity. Errors if the id is stale.")
        .param("id", "EntityId")
        .finish();

    // getComponent ---------------------------------------------------------
    registry
        .register("getComponent", {
            let ctx = ctx.clone();
            move |id: EntityId, kind: ComponentKind| -> Result<ComponentValue, ScriptError> {
                let reg = ctx.registry.borrow();
                match kind {
                    ComponentKind::Transform => {
                        let t = reg.get_component::<Transform>(id)?;
                        Ok(ComponentValue::Transform(*t))
                    }
                    ComponentKind::Light => {
                        let l = reg.get_component::<LightComponent>(id)?;
                        Ok(ComponentValue::Light(l.clone()))
                    }
                    ComponentKind::BillboardEmitter => {
                        let e = reg.get_component::<BillboardEmitterComponent>(id)?;
                        Ok(ComponentValue::BillboardEmitter(e.clone()))
                    }
                    ComponentKind::ParticleState => {
                        let p = reg.get_component::<ParticleState>(id)?;
                        Ok(ComponentValue::ParticleState(p.clone()))
                    }
                    ComponentKind::SpriteVisual => {
                        let s = reg.get_component::<SpriteVisual>(id)?;
                        Ok(ComponentValue::SpriteVisual(s.clone()))
                    }
                }
            }
        })
        .scope(ContextScope::BehaviorOnly)
        .doc("Reads a component of the given kind from an entity.")
        .param("id", "EntityId")
        .param("kind", "ComponentKind")
        .finish();

    // setComponent ---------------------------------------------------------
    registry
        .register("setComponent", {
            let ctx = ctx.clone();
            move |id: EntityId,
                  kind: ComponentKind,
                  value: ComponentValue|
                  -> Result<(), ScriptError> {
                // Enforce the `kind` and `value` discriminants agree. Mismatches
                // are a script-side bug; we fail with a clear message rather
                // than silently using one of the two.
                match (kind, &value) {
                    (ComponentKind::Transform, ComponentValue::Transform(_)) => {}
                    (ComponentKind::Light, ComponentValue::Light(_)) => {}
                    (ComponentKind::BillboardEmitter, ComponentValue::BillboardEmitter(_)) => {}
                    (ComponentKind::ParticleState, ComponentValue::ParticleState(_)) => {}
                    (ComponentKind::SpriteVisual, ComponentValue::SpriteVisual(_)) => {}
                    _ => {
                        return Err(ScriptError::InvalidArgument {
                            reason: format!(
                                "setComponent: kind {:?} does not match value discriminant",
                                kind
                            ),
                        });
                    }
                }
                let mut reg = ctx.registry.borrow_mut();
                match value {
                    ComponentValue::Transform(t) => reg.set_component(id, t)?,
                    ComponentValue::Light(l) => reg.set_component(id, l)?,
                    ComponentValue::BillboardEmitter(e) => reg.set_component(id, e)?,
                    ComponentValue::ParticleState(p) => reg.set_component(id, p)?,
                    ComponentValue::SpriteVisual(s) => reg.set_component(id, s)?,
                }
                Ok(())
            }
        })
        .scope(ContextScope::BehaviorOnly)
        .doc("Writes a component of the given kind onto an entity.")
        .param("id", "EntityId")
        .param("kind", "ComponentKind")
        .param("value", "ComponentValue")
        .finish();

    // emitEvent (broadcast) -----------------------------------------------
    registry
        .register("emitEvent", {
            let ctx = ctx.clone();
            move |event: ScriptEvent| -> Result<(), ScriptError> {
                ctx.events.borrow_mut().broadcast.push_back(event);
                Ok(())
            }
        })
        .scope(ContextScope::BehaviorOnly)
        .doc("Broadcasts an event to all listeners; drains at end of game logic.")
        .param("event", "ScriptEvent")
        .finish();

    // sendEvent (targeted) ------------------------------------------------
    registry
        .register("sendEvent", {
            let ctx = ctx.clone();
            move |target: EntityId, event: ScriptEvent| -> Result<(), ScriptError> {
                ctx.events.borrow_mut().targeted.push_back((target, event));
                Ok(())
            }
        })
        .scope(ContextScope::BehaviorOnly)
        .doc("Sends an event to a single entity; drains at end of game logic.")
        .param("target", "EntityId")
        .param("event", "ScriptEvent")
        .finish();
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
        // 7 day-one primitives + `registerHandler` (SP5) + `worldQuery` and
        // `setLightAnimation` (SP6).
        assert_eq!(r.len(), 10);
        let names: Vec<_> = r.iter().map(|p| p.name).collect();
        for expected in [
            "entityExists",
            "spawnEntity",
            "despawnEntity",
            "getComponent",
            "setComponent",
            "emitEvent",
            "sendEvent",
            "registerHandler",
            "worldQuery",
            "setLightAnimation",
        ] {
            assert!(names.contains(&expected), "missing primitive {expected}");
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
