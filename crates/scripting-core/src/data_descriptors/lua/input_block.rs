// Luau drain for the manifest's optional `input` block. Classifies raw Luau
// values; the shared resolver owns every rule and warning.
// See: context/lib/scripting.md §1 · context/lib/input.md §2

use super::super::input_block::{
    AuthoredValue, MAX_INPUT_CONTAINER_DEPTH, resolve_authored_input_block,
};
use super::super::*;

/// Luau twin of [`drain_input_block_js`]. Luau tables carry no key order, so
/// keyed entries (and therefore commands) come out sorted by key.
pub fn drain_input_block_lua(
    table: &Table,
    scope: &str,
) -> Result<Option<ModInputBlock>, DescriptorError> {
    let raw: LuaValue = table.get("input").map_err(lua_err)?;
    Ok(resolve_authored_input_block(
        scope,
        authored_value_lua(raw, 0)?,
    ))
}

pub(crate) fn authored_value_lua(
    value: LuaValue,
    depth: usize,
) -> Result<AuthoredValue, DescriptorError> {
    Ok(match value {
        LuaValue::Nil => AuthoredValue::Absent,
        LuaValue::Boolean(flag) => AuthoredValue::Bool(flag),
        LuaValue::Integer(number) => AuthoredValue::Number(number as f64),
        LuaValue::Number(number) => AuthoredValue::Number(number),
        LuaValue::String(string) => match string.to_str() {
            Ok(string) => AuthoredValue::String(string.to_string()),
            Err(_) => AuthoredValue::Other,
        },
        LuaValue::Table(table) if depth <= MAX_INPUT_CONTAINER_DEPTH => {
            authored_table_lua(table, depth)?
        }
        _ => AuthoredValue::Other,
    })
}

/// Classify a table: no keys is [`AuthoredValue::EmptyTable`], all string keys
/// an object sorted by key, a dense `1..=n` sequence an array, and anything
/// else (mixed or sparse) [`AuthoredValue::Other`].
fn authored_table_lua(table: Table, depth: usize) -> Result<AuthoredValue, DescriptorError> {
    let mut keyed = Vec::new();
    let mut indexed = Vec::new();
    for pair in table.pairs::<LuaValue, LuaValue>() {
        let (key, value) = pair.map_err(lua_err)?;
        match key {
            LuaValue::String(key) => {
                let Ok(key) = key.to_str() else {
                    return Ok(AuthoredValue::Other);
                };
                keyed.push((key.to_string(), authored_value_lua(value, depth + 1)?));
            }
            LuaValue::Integer(index) if index >= 1 => {
                indexed.push((index, authored_value_lua(value, depth + 1)?));
            }
            _ => return Ok(AuthoredValue::Other),
        }
    }
    Ok(match (keyed.is_empty(), indexed.is_empty()) {
        (true, true) => AuthoredValue::EmptyTable,
        (false, true) => {
            keyed.sort_by(|(a, _), (b, _)| a.cmp(b));
            AuthoredValue::Object(keyed)
        }
        (true, false) => {
            indexed.sort_by_key(|(index, _)| *index);
            let dense = indexed
                .iter()
                .enumerate()
                .all(|(position, (index, _))| *index == position as i64 + 1);
            if dense {
                AuthoredValue::Array(indexed.into_iter().map(|(_, item)| item).collect())
            } else {
                AuthoredValue::Other
            }
        }
        (false, false) => AuthoredValue::Other,
    })
}
