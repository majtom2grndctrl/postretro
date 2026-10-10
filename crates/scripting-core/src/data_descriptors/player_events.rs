// `playerEvents` wire form and where its `levels` scope belongs. Each VM
// converts the array to JSON and both parse it here, so the two runtimes
// accept and reject exactly the same entries with the same diagnostics.
// See: context/lib/scripting.md §12 (Player events)

use super::*;
use postretro_entities::data_descriptors::{PlayerEventDescriptor, PlayerEventEdge};

/// The manifest a `playerEvents` array arrived in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum PlayerEventSite {
    /// `setupLevel()`: entries belong to that level, so `levels` is rejected.
    Level,
    /// `ModManifest`: `levels` scopes each entry.
    Mod,
}

/// Parse a converted `playerEvents` array. A malformed entry, or a level
/// entry carrying `levels`, warns naming its index and is skipped; its
/// siblings install.
pub(crate) fn player_events_from_json(
    raw: serde_json::Value,
    site: PlayerEventSite,
    scope: &str,
) -> Result<Vec<PlayerEventDescriptor>, DescriptorError> {
    let entries = match raw {
        serde_json::Value::Null => return Ok(Vec::new()),
        // An empty Luau table converts to an empty object.
        serde_json::Value::Object(map) if map.is_empty() => return Ok(Vec::new()),
        serde_json::Value::Array(entries) => entries,
        _ => {
            return Err(DescriptorError::InvalidShape {
                reason: format!("{scope}: `playerEvents` must be an array"),
            });
        }
    };
    let mut out = Vec::with_capacity(entries.len());
    for (index, entry) in entries.into_iter().enumerate() {
        match player_event_from_json(entry, site, index, scope) {
            Ok(Some(descriptor)) => out.push(descriptor),
            Ok(None) => {}
            Err(error) => log::warn!(
                "[Scripting] {scope}: playerEvents[{index}] is malformed and was skipped: {error}"
            ),
        }
    }
    Ok(out)
}

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

/// Drain `playerEvents` from a QuickJS manifest object.
pub fn drain_player_events_js<'js>(
    ctx: &Ctx<'js>,
    obj: &Object<'js>,
    site_is_level: bool,
    scope: &str,
) -> Result<Vec<PlayerEventDescriptor>, DescriptorError> {
    if !obj.contains_key("playerEvents").map_err(js_err)? {
        return Ok(Vec::new());
    }
    let raw: JsValue = obj.get("playerEvents").map_err(js_err)?;
    if raw.is_null() || raw.is_undefined() {
        return Ok(Vec::new());
    }
    let json = conv::js_to_json(ctx, raw).map_err(js_err)?;
    player_events_from_json(json, site(site_is_level), scope)
}

/// Drain `playerEvents` from a Luau manifest table. Mirrors
/// [`drain_player_events_js`].
pub fn drain_player_events_lua(
    table: &Table,
    site_is_level: bool,
    scope: &str,
) -> Result<Vec<PlayerEventDescriptor>, DescriptorError> {
    let raw: LuaValue = table.get("playerEvents").map_err(lua_err)?;
    if matches!(raw, LuaValue::Nil) {
        return Ok(Vec::new());
    }
    if let LuaValue::Table(entries) = &raw {
        validate_dense_lua_array(entries, "`playerEvents` field")?;
    }
    let json = conv::lua_to_json(raw).map_err(lua_err)?;
    player_events_from_json(json, site(site_is_level), scope)
}

fn site(site_is_level: bool) -> PlayerEventSite {
    if site_is_level {
        PlayerEventSite::Level
    } else {
        PlayerEventSite::Mod
    }
}
