// Entity-domain scripting primitives. Composition entry point lives in `mod.rs`.
// See: context/lib/scripting.md
//
// Every primitive captures `ScriptCtx` by `Rc` at registration time. New
// primitive domains belong in a new sibling module (e.g. `light.rs`), called
// from `mod.rs::register_all` — not added here.

use crate::scripting::components::billboard_emitter::BillboardEmitterComponent;
use crate::scripting::components::light::LightComponent;
use crate::scripting::components::particle::ParticleState;
use crate::scripting::components::sprite_visual::SpriteVisual;
use crate::scripting::ctx::{GAME_EVENTS_CAPACITY, GameEvent, ScriptCtx, ScriptEvent};
use crate::scripting::data_descriptors::EntityTypeDescriptor;
use crate::scripting::error::ScriptError;
use crate::scripting::primitives_registry::{ContextScope, PrimitiveRegistry};
use crate::scripting::registry::{
    ComponentKind, ComponentValue, EntityId, FogVolumeComponent, Transform,
};
use mlua::{IntoLua, Lua, Value as LuaValue};
use rquickjs::{Ctx, IntoJs, Value as JsValue};

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

/// Hand-rolled `spawnEntity` registration. Required because the optional
/// `tags` argument cannot ride through the generic `register()` path —
/// rquickjs `Function::new` derives a strict arity from the closure's
/// signature, so a generic registration with `tags: Option<Vec<String>>`
/// would still reject `spawnEntity(t)` with one argument.
#[allow(clippy::arc_with_non_send_sync)]
fn register_spawn_entity(registry: &mut PrimitiveRegistry, ctx: ScriptCtx) {
    use crate::scripting::primitives_registry::{
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

/// Register the entity-domain primitives (everything except `worldQuery`,
/// which lives in `mod.rs` because it spans multiple component domains).
pub(crate) fn register_entity_primitives(registry: &mut PrimitiveRegistry, ctx: ScriptCtx) {
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
                    ComponentKind::FogVolume => {
                        let f = reg.get_component::<FogVolumeComponent>(id)?;
                        Ok(ComponentValue::FogVolume(*f))
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
                    (ComponentKind::FogVolume, ComponentValue::FogVolume(_)) => {}
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
                    ComponentValue::FogVolume(f) => reg.set_component(id, f)?,
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
