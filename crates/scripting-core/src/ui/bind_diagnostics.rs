// Tree-registration diagnostics for slot binds: a bind naming a slot the
// store does not hold warns once, when its trees register, never per frame.
// See: context/lib/ui.md (Diagnostics fire at tree build)

use std::collections::BTreeSet;

use crate::slot_table::SlotTable;

/// Warn once per slot name for every bind in `trees` that names a slot
/// `slots` does not hold: an undeclared store slot, a misspelled engine
/// slot, or `player.defense.<name>` for a layer no `defineMod` registers.
/// The bind still builds and reads absent. Returns the names warned about,
/// in first-seen order.
pub fn warn_unknown_slot_binds<'a, T: serde::Serialize + 'a>(
    trees: impl IntoIterator<Item = &'a T>,
    slots: &SlotTable,
) -> Vec<String> {
    let mut warned = Vec::new();
    let mut seen = BTreeSet::new();
    for tree in trees {
        if let Ok(value) = serde_json::to_value(tree) {
            collect(&value, slots, &mut seen, &mut warned);
        }
    }
    for name in &warned {
        log::warn!("[UI] bind names unknown slot `{name}`; it reads absent");
    }
    warned
}

/// Keys a store-slot binding object carries beside `slot`: a text or slider
/// bind's `format` and `tween`, and a predicate's `equals` and `compare`.
const BIND_KEYS: [&str; 5] = ["slot", "format", "tween", "equals", "compare"];

/// The slot a `{ slot }` binding object names. An object with any other key
/// is a widget or an unrelated record, not a binding.
fn bind_slot(object: &serde_json::Map<String, serde_json::Value>) -> Option<&str> {
    if !object.keys().all(|key| BIND_KEYS.contains(&key.as_str())) {
        return None;
    }
    object.get("slot")?.as_str()
}

fn collect(
    value: &serde_json::Value,
    slots: &SlotTable,
    seen: &mut BTreeSet<String>,
    warned: &mut Vec<String>,
) {
    match value {
        serde_json::Value::Object(object) => {
            if let Some(name) = bind_slot(object)
                && slots.get(name).is_none()
                && seen.insert(name.to_string())
            {
                warned.push(name.to_string());
            }
            for child in object.values() {
                collect(child, slots, seen, warned);
            }
        }
        serde_json::Value::Array(items) => {
            for child in items {
                collect(child, slots, seen, warned);
            }
        }
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::slot_table::{
        ReplicationScope, SlotOwnership, SlotRecord, SlotSchema, SlotType, SlotValue,
    };
    use postretro_test_log_capture::LogCapture;

    fn number_slot() -> SlotRecord {
        SlotRecord::new(SlotSchema {
            slot_type: SlotType::Number,
            default: Some(SlotValue::Number(0.0)),
            range: None,
            persist: false,
            readonly: false,
            ownership: SlotOwnership::Mod,
            network: ReplicationScope::None,
            per_owner: false,
            accumulate: None,
        })
    }

    fn bar(slot: &str) -> serde_json::Value {
        serde_json::json!({ "kind": "bar", "bind": { "slot": slot } })
    }

    // An unknown slot warns once, however many binds name it; a declared
    // store slot, an engine slot and a registered layer's slot pass.
    #[test]
    fn unknown_slot_bind_warns_once_at_tree_build() {
        let capture = LogCapture::start();
        let mut slots = SlotTable::new();
        slots.insert("hud.score".into(), number_slot()).unwrap();
        slots.replace_player_defense_slots(&["shield".to_string()]);
        let tree = serde_json::json!({ "root": { "kind": "panel", "children": [
            bar("hud.score"),
            bar("player.health"),
            bar("player.defense.shield"),
            bar("player.defense.shield.capacity"),
            bar("hud.scroe"),
            { "kind": "text", "bind": { "slot": "hud.scroe" } },
            { "kind": "bar", "bind": { "local": "draft" } },
            { "kind": "text", "bind": { "fact": "damage" } },
        ] } });
        let other = serde_json::json!({ "root": bar("hud.scroe") });

        let warned = warn_unknown_slot_binds([&tree, &other], &slots);

        assert_eq!(warned, ["hud.scroe"]);
        capture.assert_logged_once(log::Level::Warn, "bind names unknown slot `hud.scroe`");
    }

    // A bind to a layer no `defineMod` registers names an unknown
    // slot, so it warns the same way.
    #[test]
    fn unknown_layer_binding_warns_once_at_tree_build() {
        let capture = LogCapture::start();
        let mut slots = SlotTable::new();
        slots.replace_player_defense_slots(&["shield".to_string()]);
        let tree = serde_json::json!({ "root": { "kind": "panel", "children": [
            { "kind": "bar", "bind": { "slot": "player.defense.armor" },
              "max": { "slot": "player.defense.armor.capacity" } },
            bar("player.defense.armor"),
        ] } });

        let warned = warn_unknown_slot_binds([&tree], &slots);

        assert_eq!(
            warned,
            ["player.defense.armor", "player.defense.armor.capacity"]
        );
        capture.assert_logged_once(
            log::Level::Warn,
            "bind names unknown slot `player.defense.armor`;",
        );
    }

    // An object carrying `slot` beside non-binding keys is not a slot bind.
    #[test]
    fn unrelated_slot_key_does_not_warn() {
        let slots = SlotTable::new();
        let tree = serde_json::json!({ "root": { "kind": "panel", "slot": "not.a.slot",
            "children": [
                { "kind": "text", "bind": { "slot": "hud.x", "format": "{}",
                  "tween": { "durationMs": 1.0, "easing": "linear" } } },
            ] } });

        let warned = warn_unknown_slot_binds([&tree], &slots);

        assert_eq!(warned, ["hud.x"]);
    }
}
