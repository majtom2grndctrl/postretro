// Data-context descriptors: QuickJS `triggerEvents` drains for the level
// script (volume-keyed) and the mod manifest (tag-keyed).
// See: context/lib/scripting.md §12 (Per-member sources)

use super::super::trigger_events::{
    TriggerEventEntry, level_trigger_event, mod_trigger_event, trigger_event_key_shape,
};
use super::super::*;
use postretro_entities::data_descriptors::VolumeTriggerEventDescriptor;

/// Drain a level script's `triggerEvents`: volume-keyed `{ trigger, event,
/// fire }` entries. A tag-keyed entry warns naming the level script and is
/// skipped; a malformed entry or unknown `event` is logged and skipped rather
/// than aborting the manifest.
pub fn drain_level_trigger_events_js<'js>(
    obj: &Object<'js>,
    scope: &str,
) -> Result<Vec<VolumeTriggerEventDescriptor>, DescriptorError> {
    Ok(trigger_event_entries_js(obj, scope)?
        .into_iter()
        .filter_map(|(i, entry)| level_trigger_event(entry, i, scope))
        .collect())
}

/// Drain the mod manifest's `triggerEvents`: tag-keyed `{ tag, event, fire,
/// levels? }` entries. A volume-keyed entry warns naming the manifest and is
/// skipped; a malformed entry or unknown `event` is logged and skipped.
pub fn drain_mod_trigger_events_js<'js>(
    obj: &Object<'js>,
    scope: &str,
) -> Result<Vec<TriggerEventDescriptor>, DescriptorError> {
    Ok(trigger_event_entries_js(obj, scope)?
        .into_iter()
        .filter_map(|(i, entry)| mod_trigger_event(entry, i, scope))
        .collect())
}

fn trigger_event_entries_js<'js>(
    obj: &Object<'js>,
    scope: &str,
) -> Result<Vec<(usize, TriggerEventEntry)>, DescriptorError> {
    let Some(arr) = optional_manifest_array_js(obj, "triggerEvents", scope)? else {
        return Ok(Vec::new());
    };
    let mut out = Vec::with_capacity(arr.len());
    for i in 0..arr.len() {
        let value: JsValue = arr.get(i).map_err(js_err)?;
        match trigger_event_from_js(value, i, scope) {
            Ok(Some(entry)) => out.push((i, entry)),
            Ok(None) => {}
            Err(e) => log::warn!(
                "[Scripting] {scope}: triggerEvents[{i}] is malformed and was skipped: {e}"
            ),
        }
    }
    Ok(out)
}

fn field_present_js(item: &Object<'_>, field: &str) -> Result<bool, DescriptorError> {
    if !item.contains_key(field).map_err(js_err)? {
        return Ok(false);
    }
    let raw: JsValue = item.get(field).map_err(js_err)?;
    Ok(!(raw.is_null() || raw.is_undefined()))
}

/// Parse one `triggerEvents` entry in either form. `Ok(None)` means the entry
/// parsed but its `event` is unrecognized (already logged); a malformed entry
/// returns `Err` for the caller to log and skip.
fn trigger_event_from_js<'js>(
    value: JsValue<'js>,
    i: usize,
    scope: &str,
) -> Result<Option<TriggerEventEntry>, DescriptorError> {
    let item = Object::from_value(value).map_err(|_| DescriptorError::InvalidShape {
        reason: "trigger-event entry must be an object".into(),
    })?;
    let event = get_required_string_js(&item, "event")?;
    if !matches!(event.as_str(), "enter" | "exit") {
        log::warn!(
            "[Scripting] {scope}: triggerEvents[{i}] has unknown event `{event}` and was skipped"
        );
        return Ok(None);
    }
    let has_trigger = field_present_js(&item, "trigger")?;
    trigger_event_key_shape(has_trigger, field_present_js(&item, "tag")?)?;
    let fire = string_array_from_js(&item, "fire")?;
    if has_trigger {
        return Ok(Some(TriggerEventEntry::Volume(
            VolumeTriggerEventDescriptor {
                trigger: EntityId::from_raw(get_required_u32_js(&item, "trigger")?),
                event,
                fire,
            },
        )));
    }
    Ok(Some(TriggerEventEntry::Tag(TriggerEventDescriptor {
        tag: get_required_string_js(&item, "tag")?,
        event,
        fire,
        levels: string_array_from_js(&item, "levels")?,
    })))
}
