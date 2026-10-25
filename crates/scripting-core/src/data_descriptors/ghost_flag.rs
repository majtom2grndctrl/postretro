// Descriptor `ghost` opt-ins (top-level and `projectile.ghost`), read by the QuickJS and
// Luau parsers so both runtimes reject the same values with the same diagnostic.
// See: context/lib/entity_model.md §7d · context/lib/scripting.md §1 (parser twins)

use mlua::{Table, Value as LuaValue};
use rquickjs::{Object, Value as JsValue};

use super::{DescriptorError, js_err, lua_err};

/// Path of the top-level descriptor attribute, beside `canonicalName`.
pub(crate) const DESCRIPTOR_GHOST_PATH: &str = "ghost";
/// Path of the weapon projectile's own opt-in.
pub(crate) const PROJECTILE_GHOST_PATH: &str = "components.weapon.projectile.ghost";

fn not_a_boolean(path: &str) -> DescriptorError {
    DescriptorError::InvalidShape {
        reason: format!("`{path}` must be a boolean when supplied"),
    }
}

/// `object.ghost`: absent, `null` and `undefined` are false; anything but a
/// boolean is an error naming `path`.
pub(crate) fn ghost_flag_js(object: &Object<'_>, path: &str) -> Result<bool, DescriptorError> {
    if !object.contains_key("ghost").map_err(js_err)? {
        return Ok(false);
    }
    let raw: JsValue = object.get("ghost").map_err(js_err)?;
    if raw.is_null() || raw.is_undefined() {
        return Ok(false);
    }
    raw.as_bool().ok_or_else(|| not_a_boolean(path))
}

/// Luau mirror of [`ghost_flag_js`]: `nil` is false.
pub(crate) fn ghost_flag_lua(table: &Table, path: &str) -> Result<bool, DescriptorError> {
    match table.get::<LuaValue>("ghost").map_err(lua_err)? {
        LuaValue::Nil => Ok(false),
        LuaValue::Boolean(value) => Ok(value),
        _ => Err(not_a_boolean(path)),
    }
}
