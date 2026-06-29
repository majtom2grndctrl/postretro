// Data-context descriptors: Lua theme-token map readers.
// See: context/lib/scripting.md

use super::super::*;

/// Read a table-valued field as a `String → String` map. Absent → empty.
/// A non-table field is logged and degraded to empty. Malformed tokens are
/// logged and skipped (per-token degraded) so a single bad entry does not
/// abort the whole theme drain.
pub fn string_map_from_lua(
    table: &Table,
    field: &'static str,
) -> Result<HashMap<String, String>, DescriptorError> {
    let raw: LuaValue = table.get(field).map_err(lua_err)?;
    let map = match raw {
        LuaValue::Nil => return Ok(HashMap::new()),
        LuaValue::Table(t) => t,
        other => {
            log::warn!(
                "[Scripting] theme `{field}` must be a table, got {}; skipping field",
                other.type_name()
            );
            return Ok(HashMap::new());
        }
    };
    let mut out = HashMap::new();
    for pair in map.pairs::<String, LuaValue>() {
        let (key, value) = pair.map_err(lua_err)?;
        let s = match value {
            LuaValue::String(s) => s.to_str().map_err(lua_err)?.to_string(),
            other => {
                log::warn!(
                    "[Scripting] theme `{field}.{key}` must be a string, got {}; skipping token",
                    other.type_name()
                );
                continue;
            }
        };
        out.insert(key, s);
    }
    Ok(out)
}

/// Read a table-valued field as a `String → f32` map. Absent → empty.
/// A non-table field is logged and degraded to empty. Malformed tokens are
/// logged and skipped (per-token degraded) so a single bad entry does not
/// abort the whole theme drain.
pub fn f32_map_from_lua(
    table: &Table,
    field: &'static str,
) -> Result<HashMap<String, f32>, DescriptorError> {
    let raw: LuaValue = table.get(field).map_err(lua_err)?;
    let map = match raw {
        LuaValue::Nil => return Ok(HashMap::new()),
        LuaValue::Table(t) => t,
        other => {
            log::warn!(
                "[Scripting] theme `{field}` must be a table, got {}; skipping field",
                other.type_name()
            );
            return Ok(HashMap::new());
        }
    };
    let mut out = HashMap::new();
    for pair in map.pairs::<String, LuaValue>() {
        let (key, value) = pair.map_err(lua_err)?;
        match lua_value_as_f32(value, field) {
            Ok(f) => {
                out.insert(key, f);
            }
            Err(e) => {
                log::warn!("[Scripting] theme `{field}.{key}` is malformed and was skipped: {e}");
            }
        }
    }
    Ok(out)
}

/// Read a table-valued field as a `String → [f32; 4]` map (linear-RGBA color
/// tokens). Absent → empty. A non-table field is logged and degraded to empty.
/// Malformed tokens are logged and skipped (per-token degraded) so a single
/// bad entry does not abort the whole theme drain.
pub fn f32_array4_map_from_lua(
    table: &Table,
    field: &'static str,
) -> Result<HashMap<String, [f32; 4]>, DescriptorError> {
    let raw: LuaValue = table.get(field).map_err(lua_err)?;
    let map = match raw {
        LuaValue::Nil => return Ok(HashMap::new()),
        LuaValue::Table(t) => t,
        other => {
            log::warn!(
                "[Scripting] theme `{field}` must be a table, got {}; skipping field",
                other.type_name()
            );
            return Ok(HashMap::new());
        }
    };
    let mut out = HashMap::new();
    for pair in map.pairs::<String, LuaValue>() {
        let (key, value) = pair.map_err(lua_err)?;
        match read_f32_array_n_lua::<4>(value, field) {
            Ok(arr) => {
                out.insert(key, arr);
            }
            Err(e) => {
                log::warn!("[Scripting] theme `{field}.{key}` is malformed and was skipped: {e}");
            }
        }
    }
    Ok(out)
}
