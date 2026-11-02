// Reaction argument conversion with the respawn health degradation contract.
// See: context/lib/scripting.md

use serde_json::Value;

use super::{Ctx, DescriptorError, JsValue, LuaValue, conv, js_err, lua_err};

// JSON cannot carry NaN or infinities. Only respawn.health lowers them to 0:
// it is always invalid health, so the authoritative drain still warns and
// restores the pawn's current maximum. Other fields keep generic rejection.
const INVALID_RESPAWN_HEALTH: i32 = 0;

pub(super) fn reaction_args_from_js<'js>(
    ctx: &Ctx<'js>,
    primitive: &str,
    value: JsValue<'js>,
) -> Result<Value, DescriptorError> {
    if primitive == "respawn"
        && !value.is_array()
        && let Some(object) = value.as_object()
    {
        let health: JsValue = object.get("health").map_err(js_err)?;
        if health.as_number().is_some_and(|number| !number.is_finite()) {
            // Copy into a fresh object; authored objects may be frozen or
            // reused by another reaction with a different numeric contract.
            let copy = rquickjs::Object::new(ctx.clone()).map_err(js_err)?;
            for entry in object.props::<String, JsValue>() {
                let (key, field) = entry.map_err(js_err)?;
                if key == "health" {
                    copy.set(key, INVALID_RESPAWN_HEALTH).map_err(js_err)?;
                } else {
                    copy.set(key, field).map_err(js_err)?;
                }
            }
            return conv::js_to_json(ctx, copy.into_value()).map_err(js_err);
        }
    }
    conv::js_to_json(ctx, value).map_err(js_err)
}

pub(super) fn reaction_args_from_lua(
    primitive: &str,
    value: LuaValue,
) -> Result<Value, DescriptorError> {
    if primitive == "respawn"
        && let LuaValue::Table(table) = &value
    {
        let health: LuaValue = table.raw_get("health").map_err(lua_err)?;
        if matches!(health, LuaValue::Number(number) if !number.is_finite()) {
            // A fresh table keeps generic traversal, depth limits and error
            // paths intact. Never rewrite the author's shared/frozen table.
            let lua = table.weak_lua().upgrade();
            let copy = lua.create_table().map_err(lua_err)?;
            for entry in table.pairs::<LuaValue, LuaValue>() {
                let (key, field) = entry.map_err(lua_err)?;
                let is_health = match &key {
                    LuaValue::String(key) => key.to_str().map_err(lua_err)?.as_ref() == "health",
                    _ => false,
                };
                if is_health {
                    copy.raw_set(key, INVALID_RESPAWN_HEALTH).map_err(lua_err)?;
                } else {
                    copy.raw_set(key, field).map_err(lua_err)?;
                }
            }
            return conv::lua_to_json(LuaValue::Table(copy)).map_err(lua_err);
        }
    }
    conv::lua_to_json(value).map_err(lua_err)
}
