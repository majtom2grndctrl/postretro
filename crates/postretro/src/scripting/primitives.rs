// Engine primitives registered at startup, and the shared type definitions they reference.
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
use super::ctx::{GAME_EVENTS_CAPACITY, GameEvent, ScriptCtx, ScriptEvent};
use super::data_descriptors::EntityTypeDescriptor;
use super::error::ScriptError;
use super::primitives_registry::{ContextScope, PrimitiveRegistry};
use super::registry::{ComponentKind, ComponentValue, EntityId, Transform};
use mlua::{FromLua, IntoLua, Lua, Value as LuaValue};
use rquickjs::{Ctx, FromJs, IntoJs, Value as JsValue};

/// Newtype that maps `None` to JS `null` (rather than `undefined`) and Lua
/// `nil`, so script-side `=== null` / `== nil` checks work without authors
/// having to know which sentinel rquickjs / mlua picks for `Option::None`. The
/// SDK type signature is `string | null` / `string?`; this newtype enforces
/// it at the wire boundary.
struct NullableString(Option<String>);

impl<'js> IntoJs<'js> for NullableString {
    fn into_js(self, ctx: &Ctx<'js>) -> rquickjs::Result<JsValue<'js>> {
        match self.0 {
            Some(s) => s.into_js(ctx),
            None => Ok(JsValue::new_null(ctx.clone())),
        }
    }
}

impl IntoLua for NullableString {
    fn into_lua(self, lua: &Lua) -> mlua::Result<LuaValue> {
        match self.0 {
            Some(s) => s.into_lua(lua),
            None => Ok(LuaValue::Nil),
        }
    }
}

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
    Light {
        tag: Option<String>,
    },
    Transform {
        tag: Option<String>,
    },
    Emitter {
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
        "particle" | "sprite_visual" => Ok(QueryFilter::AlwaysEmpty),
        other => Err(ScriptError::InvalidArgument {
            reason: format!(
                "worldQuery: unknown component `{other}`; supported: \
                 \"light\" | \"transform\" | \"emitter\" | \"particle\" | \"sprite_visual\""
            ),
        }),
    }
}

const WORLD_QUERY_DOC: &str = "Return an array of entity handles matching the filter. Available in behavior and data contexts. \
     Filter shape: { component: \"light\" | \"transform\" | \"emitter\" | \"particle\" | \"sprite_visual\", tag?: string }. \
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
                    QueryFilter::Transform { tag } => Ok(JsonValue(
                        collect_transform_handles_json(&ctx, tag.as_deref()),
                    )),
                    QueryFilter::Emitter { tag } => Ok(JsonValue(collect_emitter_handles_json(
                        &ctx,
                        tag.as_deref(),
                    ))),
                    QueryFilter::AlwaysEmpty => Ok(JsonValue(serde_json::Value::Array(Vec::new()))),
                }
            }
        })
        .scope(ContextScope::Both)
        .doc(WORLD_QUERY_DOC)
        .param("filter", "WorldQueryFilter")
        .finish();
}

/// Hand-rolled `spawnEntity` registration. Required because the optional
/// `tags` argument cannot ride through the generic `register()` path —
/// rquickjs `Function::new` derives a strict arity from the closure's
/// signature, so a generic registration with `tags: Option<Vec<String>>`
/// would still reject `spawnEntity(t)` with one argument.
#[allow(clippy::arc_with_non_send_sync)]
fn register_spawn_entity(registry: &mut PrimitiveRegistry, ctx: ScriptCtx) {
    use super::primitives_registry::{
        LuauInstaller, ParamInfo, PrimitiveSignature, QuickJsInstaller, ScriptPrimitive,
    };
    use rquickjs::function::Opt;
    use std::sync::Arc;

    const NAME: &str = "spawnEntity";
    const DOC: &str = "Spawns a new entity with the given transform and returns its id. Optional `tags` attaches a tag list at creation time.";

    let exhausted = || ScriptError::InvalidArgument {
        reason: "entity slots exhausted".into(),
    };

    let quickjs_installer: QuickJsInstaller = {
        let ctx = ctx.clone();
        Arc::new(move |js_ctx: &rquickjs::Ctx<'_>| -> rquickjs::Result<()> {
            let globals = js_ctx.globals();
            let ctx = ctx.clone();
            let f = rquickjs::Function::new(
                js_ctx.clone(),
                move |js_ctx: rquickjs::Ctx<'_>,
                      transform: Transform,
                      tags: Opt<Vec<String>>|
                      -> rquickjs::Result<EntityId> {
                    let ctx = ctx.clone();
                    let tag_slice: &[String] = match &tags.0 {
                        Some(v) => v.as_slice(),
                        None => &[],
                    };
                    match ctx.registry.borrow_mut().try_spawn(transform, tag_slice) {
                        Some(id) => Ok(id),
                        None => Err(rquickjs::Exception::from_message(
                            js_ctx.clone(),
                            &exhausted().to_string(),
                        )?
                        .throw()),
                    }
                },
            )?;
            globals.set(NAME, f)?;
            Ok(())
        }) as QuickJsInstaller
    };

    let luau_installer: LuauInstaller = {
        let ctx = ctx.clone();
        Arc::new(move |lua: &mlua::Lua| -> mlua::Result<()> {
            let globals = lua.globals();
            let ctx = ctx.clone();
            let f = lua.create_function(
                move |_lua: &mlua::Lua,
                      (transform, tags): (Transform, Option<Vec<String>>)|
                      -> mlua::Result<EntityId> {
                    let ctx = ctx.clone();
                    let tag_slice: &[String] = match &tags {
                        Some(v) => v.as_slice(),
                        None => &[],
                    };
                    match ctx.registry.borrow_mut().try_spawn(transform, tag_slice) {
                        Some(id) => Ok(id),
                        None => Err(mlua::Error::RuntimeError(exhausted().to_string())),
                    }
                },
            )?;
            globals.set(NAME, f)?;
            Ok(())
        }) as LuauInstaller
    };

    // Stub installers throw WrongContext from the wrong context.
    let quickjs_stub_installer: QuickJsInstaller = {
        Arc::new(move |js_ctx: &rquickjs::Ctx<'_>| -> rquickjs::Result<()> {
            let globals = js_ctx.globals();
            let f = rquickjs::Function::new(js_ctx.clone(), move |js_ctx: rquickjs::Ctx<'_>| {
                let err = ScriptError::WrongContext {
                    primitive: NAME,
                    current: "definition",
                };
                Err::<rquickjs::Value, _>(
                    rquickjs::Exception::from_message(js_ctx, &err.to_string())?.throw(),
                )
            })?;
            globals.set(NAME, f)?;
            Ok(())
        }) as QuickJsInstaller
    };
    let luau_stub_installer: LuauInstaller = {
        Arc::new(move |lua: &mlua::Lua| -> mlua::Result<()> {
            let globals = lua.globals();
            let f = lua.create_function(move |_lua: &mlua::Lua, _: mlua::MultiValue| {
                let err = ScriptError::WrongContext {
                    primitive: NAME,
                    current: "definition",
                };
                Err::<mlua::Value, _>(mlua::Error::RuntimeError(err.to_string()))
            })?;
            globals.set(NAME, f)?;
            Ok(())
        }) as LuauInstaller
    };

    let primitive = ScriptPrimitive {
        name: NAME,
        doc: DOC,
        signature: PrimitiveSignature {
            params: vec![
                ParamInfo {
                    name: "transform",
                    ty_name: "Transform",
                    optional: false,
                },
                // `optional: true` renders as `tags?: ReadonlyArray<string>` in
                // TypeScript and `tags: {string}?` in Luau. The inner type is
                // `Vec<String>` (no `Option<…>` wrapper) so the generator
                // controls the optional spelling — matches the runtime, which
                // accepts a one-arg call.
                ParamInfo {
                    name: "tags",
                    ty_name: "Vec<String>",
                    optional: true,
                },
            ],
            return_ty_name: "Result<EntityId, ScriptError>",
        },
        context_scope: ContextScope::BehaviorOnly,
        quickjs_installer,
        luau_installer,
        quickjs_stub_installer,
        luau_stub_installer,
    };
    registry.push_manual(primitive);
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
        .finish();
    registry
        .register_tagged_union("ComponentValue")
        .flat()
        .variant("transform", "Transform", "")
        .variant("light", "LightComponent", "")
        .variant("billboard_emitter", "BillboardEmitterComponent", "")
        .variant("particle_state", "ParticleState", "")
        .variant("sprite_visual", "SpriteVisual", "")
        .finish();
    registry
        .register_type("ScriptEvent")
        .field("kind", "String", "")
        .field("payload", "Any", "")
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
        .register_type("EntityTypeComponents")
        .doc("Optional bag of component presets carried by `EntityTypeDescriptor.components`.")
        .field("light?", "Option<LightDescriptor>", "")
        .field("emitter?", "Option<BillboardEmitterComponent>", "")
        .finish();
}

/// Register all engine primitives and shared types. Called at engine startup,
/// before any script runtime is created.
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
    // Hand-rolled rather than going through the generic register() because
    // the second parameter (`tags`) is truly optional at the call site:
    // rquickjs's auto-derived FromParams enforces the function arity
    // strictly, so a closure of the form `|t: Transform, tags: Option<...>|`
    // would still reject `spawnEntity(t)` with one argument. Using
    // `function::Opt<T>` for the QuickJS side and an `Option<Vec<String>>` for
    // the second arg on the Lua side gives the script-level optional shape this
    // primitive promises in its SDK signature.
    register_spawn_entity(registry, ctx.clone());

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
                // FromJs already rejects non-Transform variants; this guard catches kind/value disagreement only.
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
    // Pushes the event to two destinations: (1) the broadcast handler queue
    // for in-engine listeners, (2) the bounded `game_events` ring buffer
    // that main.rs drains at the end of the Game logic phase as
    // observability log lines. Capacity is enforced by popping the oldest
    // entry when full so the most-recent emissions always survive.
    registry
        .register("emitEvent", {
            let ctx = ctx.clone();
            move |event: ScriptEvent| -> Result<(), ScriptError> {
                let game_event = GameEvent {
                    kind: event.kind.clone(),
                    frame: ctx.frame.get(),
                    payload: event.payload.clone(),
                };
                ctx.events.borrow_mut().broadcast.push_back(event);
                let mut buf = ctx.game_events.borrow_mut();
                while buf.len() >= GAME_EVENTS_CAPACITY {
                    buf.pop_front();
                }
                buf.push_back(game_event);
                Ok(())
            }
        })
        .scope(ContextScope::BehaviorOnly)
        .doc("Broadcasts an event to all listeners; drains at end of game logic.")
        .param("event", "ScriptEvent")
        .finish();

    // getEntityProperty ----------------------------------------------------
    // Reads a per-placement KVP from the FGD-authored map entity that spawned
    // this entity. Returns null when the key is absent or the entity carries
    // no KVP bag (e.g. spawned at runtime, not from a `.map` file).
    registry
        .register("getEntityProperty", {
            let ctx = ctx.clone();
            move |id: EntityId, key: String| -> Result<NullableString, ScriptError> {
                let reg = ctx.registry.borrow();
                Ok(NullableString(reg.get_map_kvp(id, &key)?))
            }
        })
        .scope(ContextScope::Both)
        .doc("Reads a per-placement KVP value authored on the source `.map` entity. Returns null when the key is absent or the entity has no KVP bag (e.g. runtime-spawned). Available in both behavior and data contexts.")
        .param("id", "EntityId")
        .param("key", "String")
        .finish();

    // registerEntity (data-context only) ----------------------------------
    // Writes into the engine-global `DataRegistry.entities`. Identical
    // re-inserts under the same classname are silent no-ops; differing
    // re-inserts overwrite and log at `debug!`. Definition-only so behavior
    // scripts never grow the registry mid-level.
    registry
        .register("registerEntity", {
            let ctx = ctx.clone();
            move |descriptor: EntityTypeDescriptor| -> Result<(), ScriptError> {
                ctx.data_registry
                    .borrow_mut()
                    .upsert_entity_type(descriptor);
                Ok(())
            }
        })
        .scope(ContextScope::DefinitionOnly)
        .doc("Register an entity type with optional component presets. Definition context only. Survives level unload.")
        .param("descriptor", "EntityTypeDescriptor")
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
        // 7 day-one primitives + `registerHandler` + `worldQuery` +
        // `setLightAnimation` + `getEntityProperty` + `registerEntity`.
        assert_eq!(r.len(), 12);
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
            "getEntityProperty",
            "registerEntity",
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
    fn emit_event_pushes_to_both_broadcast_queue_and_game_events_ring() {
        // Two destinations, one call: every `emitEvent` lands in the
        // broadcast queue (handler dispatch) AND the bounded `game_events`
        // ring buffer (engine observability tap). The ring entry carries the
        // current `frame` stamp.
        let (r, ctx) = registry_with_day_one();
        ctx.frame.set(7);

        let rt = rquickjs::Runtime::new().unwrap();
        let jsctx = rquickjs::Context::full(&rt).unwrap();
        jsctx.with(|jsctx| {
            for p in r.iter() {
                (p.quickjs_installer)(&jsctx).unwrap();
            }
            let _: () = jsctx
                .eval(r#"emitEvent({ kind: "damage", payload: { amount: 10 } })"#)
                .unwrap();
        });

        assert_eq!(ctx.events.borrow().broadcast.len(), 1);
        let buf = ctx.game_events.borrow();
        assert_eq!(buf.len(), 1);
        assert_eq!(buf[0].kind, "damage");
        assert_eq!(buf[0].frame, 7);
        assert_eq!(buf[0].payload, serde_json::json!({ "amount": 10 }));
    }

    #[test]
    fn emit_event_drops_oldest_when_ring_buffer_at_capacity() {
        // Capacity is 1024; pushing 1025 must drop the oldest so the most-
        // recent emissions survive to the next end-of-tick drain.
        use crate::scripting::ctx::GAME_EVENTS_CAPACITY;
        let (r, ctx) = registry_with_day_one();

        let rt = rquickjs::Runtime::new().unwrap();
        let jsctx = rquickjs::Context::full(&rt).unwrap();
        jsctx.with(|jsctx| {
            for p in r.iter() {
                (p.quickjs_installer)(&jsctx).unwrap();
            }
            let n = GAME_EVENTS_CAPACITY + 1;
            let _: () = jsctx
                .eval(format!(
                    "for (let i = 0; i < {n}; i++) {{ emitEvent({{ kind: 'k', payload: i }}); }}"
                ))
                .unwrap();
        });

        let buf = ctx.game_events.borrow();
        assert_eq!(buf.len(), GAME_EVENTS_CAPACITY);
        // First survivor is the second emission (`i = 1`); the `i = 0`
        // entry was popped when the 1025th push exceeded capacity.
        assert_eq!(buf.front().unwrap().payload, serde_json::json!(1));
        assert_eq!(
            buf.back().unwrap().payload,
            serde_json::json!(GAME_EVENTS_CAPACITY)
        );
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
