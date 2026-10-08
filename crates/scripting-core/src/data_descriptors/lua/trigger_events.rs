// Data-context descriptors: Luau `triggerEvents` drains for the level script
// (volume-keyed) and the mod manifest (tag-keyed). Mirrors
// `js/trigger_events.rs`.
// See: context/lib/scripting.md §12 (Per-member sources)

use super::super::trigger_events::{
    TriggerEventEntry, level_trigger_event, mod_trigger_event, trigger_event_key_shape,
};
use super::super::*;
use postretro_entities::data_descriptors::VolumeTriggerEventDescriptor;

/// Drain a level script's `triggerEvents`. Mirrors
/// [`drain_level_trigger_events_js`].
pub fn drain_level_trigger_events_lua(
    table: &Table,
    scope: &str,
) -> Result<Vec<VolumeTriggerEventDescriptor>, DescriptorError> {
    Ok(trigger_event_entries_lua(table, scope)?
        .into_iter()
        .filter_map(|(i, entry)| level_trigger_event(entry, i, scope))
        .collect())
}

/// Drain the mod manifest's `triggerEvents`. Mirrors
/// [`drain_mod_trigger_events_js`].
pub fn drain_mod_trigger_events_lua(
    table: &Table,
    scope: &str,
) -> Result<Vec<TriggerEventDescriptor>, DescriptorError> {
    Ok(trigger_event_entries_lua(table, scope)?
        .into_iter()
        .filter_map(|(i, entry)| mod_trigger_event(entry, i, scope))
        .collect())
}

fn trigger_event_entries_lua(
    table: &Table,
    scope: &str,
) -> Result<Vec<(i64, TriggerEventEntry)>, DescriptorError> {
    let Some(arr) = optional_manifest_array_lua(table, "triggerEvents", scope)? else {
        return Ok(Vec::new());
    };
    let len = validate_dense_lua_array(&arr, "`triggerEvents` field")?;
    let mut out = Vec::with_capacity(len);
    for i in 1..=(len as i64) {
        let item: LuaValue = arr.get(i).map_err(lua_err)?;
        match trigger_event_from_lua(item, i, scope) {
            Ok(Some(entry)) => out.push((i, entry)),
            Ok(None) => {}
            Err(e) => log::warn!(
                "[Scripting] {scope}: triggerEvents[{i}] is malformed and was skipped: {e}"
            ),
        }
    }
    Ok(out)
}

/// Parse one `triggerEvents` entry in either form. `Ok(None)` means the entry
/// parsed but its `event` is unrecognized (already logged); a malformed entry
/// returns `Err` for the caller to log and skip.
fn trigger_event_from_lua(
    value: LuaValue,
    i: i64,
    scope: &str,
) -> Result<Option<TriggerEventEntry>, DescriptorError> {
    let item = lua_table(value, "trigger-event entry")?;
    let event = get_required_string_lua(&item, "event")?;
    if !matches!(event.as_str(), "enter" | "exit") {
        log::warn!(
            "[Scripting] {scope}: triggerEvents[{i}] has unknown event `{event}` and was skipped"
        );
        return Ok(None);
    }
    let has_trigger = item.contains_key("trigger").map_err(lua_err)?;
    trigger_event_key_shape(has_trigger, item.contains_key("tag").map_err(lua_err)?)?;
    let fire = string_array_from_lua(&item, "fire")?;
    if has_trigger {
        return Ok(Some(TriggerEventEntry::Volume(
            VolumeTriggerEventDescriptor {
                trigger: EntityId::from_raw(get_required_u32_lua(&item, "trigger")?),
                event,
                fire,
            },
        )));
    }
    Ok(Some(TriggerEventEntry::Tag(TriggerEventDescriptor {
        tag: get_required_string_lua(&item, "tag")?,
        event,
        fire,
        levels: string_array_from_lua(&item, "levels")?,
    })))
}
