// Entity-domain scripting primitives. Composition entry point lives in `mod.rs`.
// See: context/lib/scripting.md
//
// Every primitive captures `ScriptCtx` by `Rc` at registration time. New
// primitive domains belong in a new sibling module (e.g. `light.rs`), called
// from `mod.rs::register_all` — not added here.

use crate::scripting::ctx::ScriptCtx;
use crate::scripting::data_descriptors::EntityTypeDescriptor;
use crate::scripting::error::ScriptError;
use crate::scripting::primitives_registry::{ContextScope, PrimitiveRegistry};
use crate::scripting::registry::EntityId;
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
        .doc("Reads a per-placement KVP value authored on the source `.map` entity. Returns null when the key is absent or the entity has no KVP bag (e.g. runtime-spawned). Available in definition and data contexts.")
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

    let _ = ctx;
}
