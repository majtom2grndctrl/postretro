// `playerEvents` wire form and where its `levels` scope belongs. Each VM
// converts one entry at a time to JSON and both parse it here, so the two
// runtimes accept and reject exactly the same entries with the same
// diagnostics, and one unconvertible entry costs only itself.
// See: context/lib/scripting.md §12 (Player events)

use super::*;
use postretro_entities::data_descriptors::{PlayerEventDescriptor, PlayerEventEdge};

/// The manifest a `playerEvents` array arrived in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PlayerEventSite {
    /// `setupLevel()`: entries belong to that level, so an entry carrying
    /// `levels` is skipped with a warning; its siblings install.
    Level,
    /// `ModManifest`: `levels` scopes each entry.
    Mod,
}

/// Collect one `playerEvents` entry at authored `index`, given its JSON
/// conversion. A malformed entry — including one its VM could not convert —
/// warns naming its index and is skipped; its siblings install.
fn push_player_event(
    out: &mut Vec<PlayerEventDescriptor>,
    converted: Result<serde_json::Value, DescriptorError>,
    site: PlayerEventSite,
    index: usize,
    scope: &str,
) {
    match converted.and_then(|entry| player_event_from_json(entry, site, index, scope)) {
        Ok(Some(descriptor)) => out.push(descriptor),
        Ok(None) => {}
        Err(error) => log::warn!(
            "[Scripting] {scope}: playerEvents[{index}] is malformed and was skipped: {error}"
        ),
    }
}

/// A QuickJS conversion failure as the bridge's own reason, without the VM's
/// wrapper text, so both runtimes log one diagnostic for one authored value.
fn js_conversion_error(error: rquickjs::Error) -> DescriptorError {
    match error {
        rquickjs::Error::FromJs {
            message: Some(reason),
            ..
        } => DescriptorError::InvalidShape { reason },
        other => js_err(other),
    }
}

/// Luau twin of [`js_conversion_error`].
fn lua_conversion_error(error: mlua::Error) -> DescriptorError {
    match error {
        mlua::Error::FromLuaConversionError {
            message: Some(reason),
            ..
        }
        | mlua::Error::RuntimeError(reason) => DescriptorError::InvalidShape { reason },
        other => lua_err(other),
    }
}

fn not_an_array(scope: &str) -> DescriptorError {
    DescriptorError::InvalidShape {
        reason: format!("{scope}: `playerEvents` must be an array"),
    }
}

/// Parse one converted entry. `Ok(None)` means a level entry carrying
/// `levels`, already logged.
fn player_event_from_json(
    entry: serde_json::Value,
    site: PlayerEventSite,
    index: usize,
    scope: &str,
) -> Result<Option<PlayerEventDescriptor>, DescriptorError> {
    let serde_json::Value::Object(mut entry) = entry else {
        return Err(DescriptorError::InvalidShape {
            reason: "player-event entry must be an object".into(),
        });
    };
    let edge = match entry.get("edge") {
        Some(serde_json::Value::String(word)) => {
            PlayerEventEdge::from_wire(word).ok_or_else(|| DescriptorError::InvalidShape {
                reason: format!("unknown edge `{word}`; expected `becomes` or `ceases`"),
            })?
        }
        _ => {
            return Err(DescriptorError::InvalidShape {
                reason: "`edge` must be `becomes` or `ceases`".into(),
            });
        }
    };
    let condition = ir_node_from_json(
        entry.remove("condition").unwrap_or(serde_json::Value::Null),
        "player-event entry `condition`",
    )?;
    let fire = string_list(entry.remove("fire"), "fire")?.unwrap_or_default();
    let levels = string_list(entry.remove("levels"), "levels")?;
    if site == PlayerEventSite::Level && levels.is_some() {
        log::warn!(
            "[Scripting] level script `{scope}`: playerEvents[{index}] carries `levels`; a level's player events belong to that level, and `levels` scopes only ModManifest entries. Skipped"
        );
        return Ok(None);
    }
    Ok(Some(PlayerEventDescriptor {
        edge,
        condition,
        fire,
        levels: levels.unwrap_or_default(),
        authored_index: index,
    }))
}

/// `None` when the field is absent or null.
fn string_list(
    value: Option<serde_json::Value>,
    field: &str,
) -> Result<Option<Vec<String>>, DescriptorError> {
    let items = match value {
        None | Some(serde_json::Value::Null) => return Ok(None),
        Some(serde_json::Value::Array(items)) => items,
        // An empty Luau table converts to an empty object.
        Some(serde_json::Value::Object(map)) if map.is_empty() => Vec::new(),
        Some(_) => {
            return Err(DescriptorError::InvalidShape {
                reason: format!("`{field}` must be an array of strings"),
            });
        }
    };
    items
        .into_iter()
        .map(|item| match item {
            serde_json::Value::String(name) => Ok(name),
            _ => Err(DescriptorError::InvalidShape {
                reason: format!("`{field}` elements must be strings"),
            }),
        })
        .collect::<Result<Vec<_>, _>>()
        .map(Some)
}

/// Drain `playerEvents` from a QuickJS manifest object. Each entry converts
/// on its own, so an unconvertible one is skipped like any malformed entry.
pub fn drain_player_events_js<'js>(
    ctx: &Ctx<'js>,
    obj: &Object<'js>,
    site: PlayerEventSite,
    scope: &str,
) -> Result<Vec<PlayerEventDescriptor>, DescriptorError> {
    if !obj.contains_key("playerEvents").map_err(js_err)? {
        return Ok(Vec::new());
    }
    let raw: JsValue = obj.get("playerEvents").map_err(js_err)?;
    if raw.is_null() || raw.is_undefined() {
        return Ok(Vec::new());
    }
    let Some(entries) = raw.as_array() else {
        // An empty object reads as an empty list, as an empty Luau table does.
        if raw
            .as_object()
            .is_some_and(|object| object.keys::<String>().next().is_none())
        {
            return Ok(Vec::new());
        }
        return Err(not_an_array(scope));
    };
    let mut out = Vec::with_capacity(entries.len());
    for index in 0..entries.len() {
        let entry: JsValue = entries.get(index).map_err(js_err)?;
        let converted = conv::js_to_json(ctx, entry).map_err(js_conversion_error);
        push_player_event(&mut out, converted, site, index, scope);
    }
    Ok(out)
}

/// Drain `playerEvents` from a Luau manifest table. Mirrors
/// [`drain_player_events_js`]; an empty table is an empty array.
pub fn drain_player_events_lua(
    table: &Table,
    site: PlayerEventSite,
    scope: &str,
) -> Result<Vec<PlayerEventDescriptor>, DescriptorError> {
    let raw: LuaValue = table.get("playerEvents").map_err(lua_err)?;
    let entries = match raw {
        LuaValue::Nil => return Ok(Vec::new()),
        LuaValue::Table(entries) => entries,
        _ => return Err(not_an_array(scope)),
    };
    let len = validate_dense_lua_array(&entries, "`playerEvents` field")?;
    let mut out = Vec::with_capacity(len);
    for slot in 1..=len {
        let entry: LuaValue = entries.get(slot).map_err(lua_err)?;
        let converted = conv::lua_to_json(entry).map_err(lua_conversion_error);
        // Diagnostics count entries from 0, matching the QuickJS drain.
        push_player_event(&mut out, converted, site, slot - 1, scope);
    }
    Ok(out)
}
