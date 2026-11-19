// VM-agnostic resolution of the manifest's `saves` block, shared by the
// QuickJS and Luau drains so both runtimes clamp and warn identically. The
// drains only classify raw VM values into `AuthoredValue`; every rule and
// warning lives here. Nothing here rejects the manifest.
// See: context/lib/scripting.md §5 (Durable State Store)

use super::input_block::AuthoredValue;
use crate::runtime::{MAX_CHARACTER_SLOTS, MAX_RUN_SLOTS, SavesManifest};

/// Resolve the authored `saves` block. Absent yields `None`: the mod declared
/// no block. A present block always yields counts: a value that is not an
/// object warns and uses one slot of each kind, since the author did declare
/// a block.
pub(crate) fn resolve_saves(scope: &str, raw: AuthoredValue) -> Option<SavesManifest> {
    if matches!(raw, AuthoredValue::Absent) {
        return None;
    }
    let Some(entries) = raw.into_entries() else {
        log::warn!(
            "[Scripting] {scope}: `saves` must be an object with `characterSlots` and `runSlots`; using 1 of each"
        );
        return Some(SavesManifest {
            character_slots: 1,
            run_slots: 1,
        });
    };
    let mut character_slots = AuthoredValue::Absent;
    let mut run_slots = AuthoredValue::Absent;
    for (key, value) in entries {
        match key.as_str() {
            "characterSlots" => character_slots = value,
            "runSlots" => run_slots = value,
            _ => {}
        }
    }
    Some(SavesManifest {
        character_slots: resolve_slot_count(
            scope,
            "characterSlots",
            character_slots,
            MAX_CHARACTER_SLOTS,
        ),
        run_slots: resolve_slot_count(scope, "runSlots", run_slots, MAX_RUN_SLOTS),
    })
}

/// One slot count: a whole number of at least 1, clamped to `cap` with a
/// warning. Missing, non-numeric, fractional or below 1 warns and uses 1.
fn resolve_slot_count(scope: &str, field: &str, raw: AuthoredValue, cap: usize) -> usize {
    let count = match raw {
        AuthoredValue::Number(count)
            if count.is_finite() && count.fract() == 0.0 && count >= 1.0 =>
        {
            count
        }
        AuthoredValue::Absent => {
            log::warn!("[Scripting] {scope}: `saves.{field}` is missing; using 1");
            return 1;
        }
        _ => {
            log::warn!(
                "[Scripting] {scope}: `saves.{field}` must be a whole number of at least 1; using 1"
            );
            return 1;
        }
    };
    if count > cap as f64 {
        log::warn!(
            "[Scripting] {scope}: `saves.{field}` {count} is above the engine cap of {cap}; clamping to {cap}"
        );
        return cap;
    }
    count as usize
}
