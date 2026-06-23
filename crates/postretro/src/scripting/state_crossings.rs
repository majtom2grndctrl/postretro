// State-crossing detector (M13 HUD dynamics): engine-side watchers composed in
// `DataRegistry` from matching mod-global crossings plus level-local
// `setupLevel().crossings`, then checked against the authoritative slot table
// after each frame's slot writes. On a threshold crossing in the declared
// direction the detector fires a reaction list synchronously through the shared
// named-reaction vocabulary (Task 2's `fire_named_event_with_sequences`).
// See: context/lib/scripting.md §10.4

use super::data_descriptors::{CrossingCondition, CrossingDescriptor};
use super::data_registry::DataRegistry;
use super::slot_table::{SlotTable, SlotValue};

/// One active crossing watcher. The threshold is already a fraction of the
/// registration's `max` (computed at parse time); the watcher normalizes the
/// observed slot value by the same `max` before comparing, so `below`/`above`
/// fire on a fraction crossing. `previous` is the last observed normalized
/// value — `None` until the first observation, which is when it arms (no fire
/// on the first observed value).
#[derive(Debug, Clone, PartialEq)]
struct Watcher {
    slot: String,
    condition: CrossingCondition,
    max: f32,
    fire: Vec<String>,
    /// Last observed normalized value (`raw / max`), or `None` before the first
    /// observation. A watcher cannot fire until this is `Some`.
    previous: Option<f32>,
}

/// Active state-crossing watchers for the current level. Built from the data
/// registry's `crossings` at level load and dropped on unload (the registry
/// clears them; this rebuilds from the fresh registry). Mirrors
/// [`super::reaction_dispatch::ProgressTracker`]'s "engine-side subscription
/// tracker fed by the data registry" shape.
#[derive(Debug, Default)]
pub(crate) struct CrossingDetector {
    watchers: Vec<Watcher>,
}

impl CrossingDetector {
    pub(crate) fn new() -> Self {
        Self::default()
    }

    /// Build watchers from the registry's crossing descriptors. Callers
    /// `clear()` first (or use a fresh detector) to avoid duplicate watchers.
    ///
    /// A registration whose slot is not a `Number` slot (wrong type, or a slot
    /// the table does not know) warns and is skipped here, at registration
    /// time — it never enters the watch set. The previous value initializes to
    /// the slot's value at level start so the initial state never fires; a slot
    /// with no value yet stays unarmed (`previous = None`) until the first
    /// observed value.
    pub(crate) fn initialize(&mut self, data_registry: &DataRegistry, slot_table: &SlotTable) {
        for crossing in &data_registry.crossings {
            if !slot_is_number(slot_table, &crossing.slot) {
                log::warn!(
                    "[Scripting] onStateCrossing: slot `{}` is not a registered Number slot; \
                     crossing watcher skipped",
                    crossing.slot,
                );
                continue;
            }
            let previous = read_number(slot_table, &crossing.slot).map(|raw| raw / crossing.max);
            self.watchers
                .push(Watcher::from_descriptor(crossing, previous));
        }
    }

    /// Compare each watched slot's current value to its previous value and
    /// return the event names to fire (in watcher-declaration order, each
    /// watcher's `fire` list in order). Advances every watcher's `previous` to
    /// the value observed this call. The caller runs the returned names through
    /// [`super::reaction_dispatch::fire_named_event_with_sequences`].
    ///
    /// A watcher with no value yet (`previous == None`) arms on the first
    /// observed value without firing. A slot that loses its value (back to
    /// `None`) disarms without firing.
    pub(crate) fn detect(&mut self, slot_table: &SlotTable) -> Vec<String> {
        let mut to_fire = Vec::new();
        for watcher in &mut self.watchers {
            let current = read_number(slot_table, &watcher.slot).map(|raw| raw / watcher.max);
            // A crossing needs both endpoints. When either is `None` (arming on
            // the first observed value, or disarming when the value is gone) no
            // edge exists, so nothing fires — only `previous` advances.
            if let (Some(prev), Some(cur)) = (watcher.previous, current) {
                if watcher.crosses(prev, cur) {
                    to_fire.extend(watcher.fire.iter().cloned());
                }
            }
            watcher.previous = current;
        }
        to_fire
    }

    pub(crate) fn clear(&mut self) {
        self.watchers.clear();
    }

    #[cfg(test)]
    fn watcher_count(&self) -> usize {
        self.watchers.len()
    }
}

impl Watcher {
    fn from_descriptor(crossing: &CrossingDescriptor, previous: Option<f32>) -> Self {
        Self {
            slot: crossing.slot.clone(),
            condition: crossing.condition,
            max: crossing.max,
            fire: crossing.fire.clone(),
            previous,
        }
    }

    /// Whether a transition from `prev` to `cur` (both normalized fractions)
    /// crosses this watcher's threshold in its direction.
    fn crosses(&self, prev: f32, cur: f32) -> bool {
        match self.condition {
            CrossingCondition::Below { threshold } => prev >= threshold && cur < threshold,
            CrossingCondition::Above { threshold } => prev <= threshold && cur > threshold,
        }
    }
}

/// `true` only when the slot exists and is declared a `Number` slot. Used at
/// registration to skip non-numeric watchers (the value-type guard).
fn slot_is_number(slot_table: &SlotTable, name: &str) -> bool {
    use super::slot_table::SlotType;
    slot_table
        .get(name)
        .is_some_and(|record| record.schema.slot_type == SlotType::Number)
}

/// Read the slot's current numeric value, or `None` when the slot is absent,
/// has no value yet, or holds a non-`Number` value.
fn read_number(slot_table: &SlotTable, name: &str) -> Option<f32> {
    match slot_table
        .get(name)
        .and_then(|record| record.value.as_ref())
    {
        Some(SlotValue::Number(v)) => Some(*v),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scripting::data_descriptors::{
        CrossingCondition, CrossingDescriptor, LevelManifest,
    };
    use crate::scripting::slot_table::{
        NumericRange, SlotOwnership, SlotRecord, SlotSchema, SlotType, SlotValue,
    };

    /// A mod-owned writable Number slot under a fresh namespace, with an initial
    /// value. Built directly (not via `defineStore`) to keep the test minimal.
    fn number_slot(value: Option<f32>) -> SlotRecord {
        let mut record = SlotRecord::new(SlotSchema {
            slot_type: SlotType::Number,
            default: None,
            range: Some(NumericRange {
                min: 0.0,
                max: 100.0,
            }),
            persist: false,
            readonly: false,
            ownership: SlotOwnership::Mod,
            network: crate::scripting::slot_table::ReplicationScope::None,
        });
        record.value = value.map(SlotValue::Number);
        record
    }

    fn table_with(slot: &str, value: Option<f32>) -> SlotTable {
        let mut table = SlotTable::new();
        table
            .insert(slot.to_string(), number_slot(value))
            .expect("test slot should be vacant");
        table
    }

    fn set(table: &mut SlotTable, slot: &str, value: f32) {
        table.get_mut(slot).unwrap().value = Some(SlotValue::Number(value));
    }

    fn below_crossing(
        slot: &str,
        raw_threshold: f32,
        max: f32,
        fire: &[&str],
    ) -> CrossingDescriptor {
        CrossingDescriptor {
            slot: slot.to_string(),
            condition: CrossingCondition::Below {
                threshold: raw_threshold / max,
            },
            max,
            fire: fire.iter().map(|s| s.to_string()).collect(),
        }
    }

    fn above_crossing(
        slot: &str,
        raw_threshold: f32,
        max: f32,
        fire: &[&str],
    ) -> CrossingDescriptor {
        CrossingDescriptor {
            slot: slot.to_string(),
            condition: CrossingCondition::Above {
                threshold: raw_threshold / max,
            },
            max,
            fire: fire.iter().map(|s| s.to_string()).collect(),
        }
    }

    fn registry_with(crossings: Vec<CrossingDescriptor>) -> DataRegistry {
        let mut reg = DataRegistry::new();
        reg.populate_from_manifest(
            LevelManifest {
                reactions: Vec::new(),
                crossings,
                ui_trees: Vec::new(),
            },
            &[],
        );
        reg
    }

    #[test]
    fn below_fires_once_on_downward_crossing() {
        // `below: 0.2` of max 100 ⇒ fraction threshold 0.2. Start at 100 (1.0),
        // a single tick below 20 fires exactly once.
        let mut table = table_with("test.health", Some(100.0));
        let reg = registry_with(vec![below_crossing(
            "test.health",
            20.0,
            100.0,
            &["lowHealth"],
        )]);
        let mut detector = CrossingDetector::new();
        detector.initialize(&reg, &table);

        // Still above threshold: no fire.
        set(&mut table, "test.health", 50.0);
        assert!(detector.detect(&table).is_empty());

        // Cross below: fires once.
        set(&mut table, "test.health", 15.0);
        assert_eq!(detector.detect(&table), vec!["lowHealth".to_string()]);

        // Stay below: no re-fire (no fresh crossing).
        set(&mut table, "test.health", 10.0);
        assert!(detector.detect(&table).is_empty());
    }

    #[test]
    fn below_rearms_only_after_recrossing_back_above() {
        let mut table = table_with("test.health", Some(100.0));
        let reg = registry_with(vec![below_crossing(
            "test.health",
            20.0,
            100.0,
            &["lowHealth"],
        )]);
        let mut detector = CrossingDetector::new();
        detector.initialize(&reg, &table);

        set(&mut table, "test.health", 10.0);
        assert_eq!(detector.detect(&table), vec!["lowHealth".to_string()]);

        // Recross back above the threshold: no fire (that's the `above` event,
        // which we did not register), but it re-arms the `below` watcher.
        set(&mut table, "test.health", 80.0);
        assert!(detector.detect(&table).is_empty());

        // Cross below again: fires again.
        set(&mut table, "test.health", 5.0);
        assert_eq!(detector.detect(&table), vec!["lowHealth".to_string()]);
    }

    #[test]
    fn does_not_fire_when_starting_below_threshold() {
        // The slot already sits below the threshold at registration. The initial
        // state must NOT fire; only a fresh downward crossing fires.
        let table = table_with("test.health", Some(10.0));
        let reg = registry_with(vec![below_crossing(
            "test.health",
            20.0,
            100.0,
            &["lowHealth"],
        )]);
        let mut detector = CrossingDetector::new();
        detector.initialize(&reg, &table);

        // First detect at the same below-threshold value: prev == cur, no edge.
        assert!(detector.detect(&table).is_empty());
    }

    #[test]
    fn above_fires_on_upward_crossing() {
        let mut table = table_with("test.shield", Some(0.0));
        let reg = registry_with(vec![above_crossing(
            "test.shield",
            50.0,
            100.0,
            &["shielded"],
        )]);
        let mut detector = CrossingDetector::new();
        detector.initialize(&reg, &table);

        set(&mut table, "test.shield", 30.0);
        assert!(detector.detect(&table).is_empty());

        set(&mut table, "test.shield", 60.0);
        assert_eq!(detector.detect(&table), vec!["shielded".to_string()]);
    }

    #[test]
    fn fire_list_dispatches_every_named_event_in_order() {
        let mut table = table_with("test.health", Some(100.0));
        let reg = registry_with(vec![below_crossing(
            "test.health",
            20.0,
            100.0,
            &["playAlarm", "flashRed"],
        )]);
        let mut detector = CrossingDetector::new();
        detector.initialize(&reg, &table);

        set(&mut table, "test.health", 10.0);
        assert_eq!(
            detector.detect(&table),
            vec!["playAlarm".to_string(), "flashRed".to_string()]
        );
    }

    #[test]
    fn raw_value_comparison_when_max_is_one() {
        // No `max` ⇒ default 1.0 ⇒ the threshold is the raw value, so a slot
        // whose raw value crosses 3.0 fires regardless of any schema range.
        let mut table = table_with("test.charges", Some(5.0));
        let reg = registry_with(vec![below_crossing(
            "test.charges",
            3.0,
            1.0,
            &["lowCharges"],
        )]);
        let mut detector = CrossingDetector::new();
        detector.initialize(&reg, &table);

        set(&mut table, "test.charges", 2.0);
        assert_eq!(detector.detect(&table), vec!["lowCharges".to_string()]);
    }

    #[test]
    fn non_number_slot_warns_and_skips_at_registration() {
        // A Boolean slot under the watched name: the watcher must not register.
        let mut table = SlotTable::new();
        table
            .insert(
                "test.flag".to_string(),
                SlotRecord::new(SlotSchema {
                    slot_type: SlotType::Boolean,
                    default: Some(SlotValue::Boolean(true)),
                    range: None,
                    persist: false,
                    readonly: false,
                    ownership: SlotOwnership::Mod,
                    network: crate::scripting::slot_table::ReplicationScope::None,
                }),
            )
            .unwrap();
        let reg = registry_with(vec![below_crossing("test.flag", 0.5, 1.0, &["never"])]);
        let mut detector = CrossingDetector::new();
        detector.initialize(&reg, &table);

        assert_eq!(detector.watcher_count(), 0, "non-Number slot is skipped");
        assert!(detector.detect(&table).is_empty());
    }

    #[test]
    fn slot_with_no_value_arms_on_first_observed_value_without_firing() {
        // The slot has no value at registration: the watcher stays unarmed and
        // cannot fire until a value exists, then arms on it (no fire), then
        // fires on the next crossing.
        let mut table = table_with("test.health", None);
        let reg = registry_with(vec![below_crossing(
            "test.health",
            20.0,
            100.0,
            &["lowHealth"],
        )]);
        let mut detector = CrossingDetector::new();
        detector.initialize(&reg, &table);

        // First observed value is already below threshold: arming, no fire.
        set(&mut table, "test.health", 10.0);
        assert!(detector.detect(&table).is_empty());

        // Move above, then below: now a real crossing fires.
        set(&mut table, "test.health", 50.0);
        assert!(detector.detect(&table).is_empty());
        set(&mut table, "test.health", 5.0);
        assert_eq!(detector.detect(&table), vec!["lowHealth".to_string()]);
    }

    // NOTE: the AC-3 contract test (styleRanges display value vs. crossing
    // authoritative slot diverging mid-tween) lives in
    // `render/ui/style_ranges.rs` — it crosses the render/scripting seam, and
    // `render` is not in scope in the `gen-script-types` bin that re-includes
    // this `scripting` module tree.

    #[test]
    fn clear_drops_all_watchers() {
        let table = table_with("test.health", Some(100.0));
        let reg = registry_with(vec![below_crossing(
            "test.health",
            20.0,
            100.0,
            &["lowHealth"],
        )]);
        let mut detector = CrossingDetector::new();
        detector.initialize(&reg, &table);
        assert_eq!(detector.watcher_count(), 1);

        detector.clear();
        assert_eq!(detector.watcher_count(), 0);
        assert!(detector.detect(&table).is_empty());
    }
}
