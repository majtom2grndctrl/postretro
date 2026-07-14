// State-crossing detector (M13 HUD dynamics): engine-side watchers composed in
// `DataRegistry` from matching mod-global crossings plus level-local
// `setupLevel().crossings`, then checked against the authoritative slot table
// after each frame's slot writes. Threshold watchers fire on their declared
// edge; IR predicates fire on false-to-true edges. Both dispatch a reaction list
// synchronously through the shared named-reaction vocabulary.
// See: context/lib/scripting.md §12

use super::ctx::ScriptCtx;
use super::data_descriptors::CrossingCondition;
use super::data_registry::DataRegistry;
use super::ir::{BakedIr, BoundProgram, CURRENT_IR_VERSION, IrType, IrValue, bind, eval_value};
use super::ir_scopes::StoreScope;
use super::slot_table::{SlotTable, SlotValue};

/// One active crossing watcher. A threshold watcher stores its condition as a
/// fraction of `max` and arms from the first observed normalized value. A
/// predicate watcher stores a StoreScope-bound Boolean program and arms after
/// observing false. See: context/lib/scripting.md §12.
enum Watcher {
    /// The shipped single-number-slot threshold watcher. Its data flow and
    /// edge behavior deliberately remain unchanged.
    Threshold {
        slot: String,
        condition: CrossingCondition,
        max: f32,
        fire: Vec<String>,
        /// Last observed normalized value (`raw / max`), or `None` before the
        /// first observation. A watcher cannot fire until this is `Some`.
        previous: Option<f32>,
    },
    /// A predicate-form crossing owns the program bound once at installation.
    /// `StoreScope` retains the live script context for eval without exposing
    /// any VM-coupled state through the entities descriptor boundary.
    Predicate {
        program: BoundProgram<StoreScope>,
        scope: StoreScope,
        fire: Vec<String>,
        previous: bool,
    },
}

/// Active state-crossing watchers for the current level. Built from the data
/// registry's `crossings` at level load and dropped on unload (the registry
/// clears them; this rebuilds from the fresh registry). Mirrors
/// [`super::reaction_dispatch::ProgressTracker`]'s "engine-side subscription
/// tracker fed by the data registry" shape.
#[derive(Default)]
pub struct CrossingDetector {
    watchers: Vec<Watcher>,
}

impl CrossingDetector {
    pub fn new() -> Self {
        Self::default()
    }

    /// Build watchers from the registry's crossing descriptors. Callers
    /// `clear()` first (or use a fresh detector) to avoid duplicate watchers.
    ///
    /// A threshold registration whose slot is not a `Number` slot (wrong type,
    /// or a slot the table does not know) warns and is skipped here, at
    /// registration time. An IR predicate is bound once against
    /// [`StoreScope::script`]; invalid or non-Boolean roots likewise warn and
    /// never enter the watch set. Initial state only arms a watcher — it never
    /// fires an event.
    pub fn initialize(
        &mut self,
        data_registry: &DataRegistry,
        slot_table: &SlotTable,
        script_ctx: &ScriptCtx,
    ) {
        for crossing in &data_registry.crossings {
            match &crossing.condition {
                CrossingCondition::Below { .. } | CrossingCondition::Above { .. } => {
                    let Some(slot) = crossing.slot.as_deref() else {
                        log::warn!(
                            "[Scripting] onStateCrossing: threshold crossing has no slot; \
                             crossing watcher skipped"
                        );
                        continue;
                    };
                    if !slot_is_number(slot_table, slot) {
                        log::warn!(
                            "[Scripting] onStateCrossing: slot `{slot}` is not a registered Number slot; \
                             crossing watcher skipped",
                        );
                        continue;
                    }
                    let previous = read_number(slot_table, slot).map(|raw| raw / crossing.max);
                    self.watchers.push(Watcher::Threshold {
                        slot: slot.to_string(),
                        condition: crossing.condition.clone(),
                        max: crossing.max,
                        fire: crossing.fire.clone(),
                        previous,
                    });
                }
                CrossingCondition::Ir(root) => {
                    let scope = StoreScope::script(script_ctx.clone());
                    let baked = BakedIr {
                        version: CURRENT_IR_VERSION,
                        output: None,
                        root: root.clone(),
                    };
                    let program = match bind(&baked, &scope) {
                        Ok(program) if program.root_type == IrType::Bool => program,
                        Ok(program) => {
                            log::warn!(
                                "[Scripting] onStateCrossing: predicate must produce Bool, but produces {}; \
                                 crossing watcher skipped",
                                ir_type_label(program.root_type),
                            );
                            continue;
                        }
                        Err(error) => {
                            log::warn!(
                                "[Scripting] onStateCrossing: predicate failed to bind ({error}); \
                                 crossing watcher skipped"
                            );
                            continue;
                        }
                    };
                    let previous = matches!(eval_value(&program, &scope), IrValue::Bool(true));
                    self.watchers.push(Watcher::Predicate {
                        program,
                        scope,
                        fire: crossing.fire.clone(),
                        previous,
                    });
                }
            }
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
    pub fn detect(&mut self, slot_table: &SlotTable) -> Vec<String> {
        let mut to_fire = Vec::new();
        for watcher in &mut self.watchers {
            match watcher {
                Watcher::Threshold {
                    slot,
                    condition,
                    max,
                    fire,
                    previous,
                } => {
                    let current = read_number(slot_table, slot).map(|raw| raw / *max);
                    // A crossing needs both endpoints. When either is `None`
                    // (arming on the first observed value, or disarming when
                    // the value is gone) no edge exists, so nothing fires.
                    if let (Some(prev), Some(cur)) = (*previous, current) {
                        if threshold_crosses(condition, prev, cur) {
                            to_fire.extend(fire.iter().cloned());
                        }
                    }
                    *previous = current;
                }
                Watcher::Predicate {
                    program,
                    scope,
                    fire,
                    previous,
                } => {
                    let current = matches!(eval_value(program, scope), IrValue::Bool(true));
                    if !*previous && current {
                        to_fire.extend(fire.iter().cloned());
                    }
                    // A true-to-false evaluation re-arms the next false-to-true
                    // edge; no slot shortcut or stale value is retained.
                    *previous = current;
                }
            }
        }
        to_fire
    }

    pub fn clear(&mut self) {
        self.watchers.clear();
    }

    #[cfg(test)]
    fn watcher_count(&self) -> usize {
        self.watchers.len()
    }
}

/// Whether a threshold transition from `prev` to `cur` (both normalized
/// fractions) crosses in its registered direction.
fn threshold_crosses(condition: &CrossingCondition, prev: f32, cur: f32) -> bool {
    match condition {
        CrossingCondition::Below { threshold } => prev >= *threshold && cur < *threshold,
        CrossingCondition::Above { threshold } => prev <= *threshold && cur > *threshold,
        CrossingCondition::Ir(_) => false,
    }
}

fn ir_type_label(ty: IrType) -> &'static str {
    match ty {
        IrType::Number => "Number",
        IrType::Bool => "Bool",
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
    use crate::data_descriptors::{CrossingCondition, CrossingDescriptor};
    use crate::ir::IrNode;
    use crate::slot_table::{
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
            network: crate::slot_table::ReplicationScope::None,
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
            slot: Some(slot.to_string()),
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
            slot: Some(slot.to_string()),
            condition: CrossingCondition::Above {
                threshold: raw_threshold / max,
            },
            max,
            fire: fire.iter().map(|s| s.to_string()).collect(),
        }
    }

    fn predicate_crossing(predicate: IrNode, fire: &[&str]) -> CrossingDescriptor {
        CrossingDescriptor {
            slot: None,
            condition: CrossingCondition::Ir(predicate),
            max: 1.0,
            fire: fire.iter().map(|event| (*event).to_string()).collect(),
        }
    }

    fn number(value: f32) -> Box<IrNode> {
        Box::new(IrNode::Const {
            value: IrValue::Number(value),
        })
    }

    fn input(name: &str) -> Box<IrNode> {
        Box::new(IrNode::Input {
            name: name.to_string(),
        })
    }

    /// `a >= 2 && b >= 1` expressed with the shipped comparison and select
    /// nodes: if `a` clears its comparison, return `b`'s comparison; otherwise
    /// return false.
    fn both_thresholds_predicate() -> IrNode {
        IrNode::Select {
            cond: Box::new(IrNode::Ge {
                a: input("test.a"),
                b: number(2.0),
            }),
            a: Box::new(IrNode::Ge {
                a: input("test.b"),
                b: number(1.0),
            }),
            b: Box::new(IrNode::Const {
                value: IrValue::Bool(false),
            }),
        }
    }

    fn predicate_ctx(a: f32, b: f32) -> ScriptCtx {
        let ctx = ScriptCtx::new();
        let mut table = ctx.slot_table.borrow_mut();
        table
            .insert("test.a".to_string(), number_slot(Some(a)))
            .expect("test.a should be vacant");
        table
            .insert("test.b".to_string(), number_slot(Some(b)))
            .expect("test.b should be vacant");
        drop(table);
        ctx
    }

    fn set_ctx(ctx: &ScriptCtx, slot: &str, value: f32) {
        ctx.slot_table.borrow_mut().get_mut(slot).unwrap().value = Some(SlotValue::Number(value));
    }

    fn registry_with(crossings: Vec<CrossingDescriptor>) -> DataRegistry {
        let mut reg = DataRegistry::new();
        reg.populate_level(Vec::new(), crossings, &[]);
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
        detector.initialize(&reg, &table, &ScriptCtx::new());

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
        detector.initialize(&reg, &table, &ScriptCtx::new());

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
        detector.initialize(&reg, &table, &ScriptCtx::new());

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
        detector.initialize(&reg, &table, &ScriptCtx::new());

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
        detector.initialize(&reg, &table, &ScriptCtx::new());

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
        detector.initialize(&reg, &table, &ScriptCtx::new());

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
                    network: crate::slot_table::ReplicationScope::None,
                }),
            )
            .unwrap();
        let reg = registry_with(vec![below_crossing("test.flag", 0.5, 1.0, &["never"])]);
        let mut detector = CrossingDetector::new();
        detector.initialize(&reg, &table, &ScriptCtx::new());

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
        detector.initialize(&reg, &table, &ScriptCtx::new());

        // First observed value is already below threshold: arming, no fire.
        set(&mut table, "test.health", 10.0);
        assert!(detector.detect(&table).is_empty());

        // Move above, then below: now a real crossing fires.
        set(&mut table, "test.health", 50.0);
        assert!(detector.detect(&table).is_empty());
        set(&mut table, "test.health", 5.0);
        assert_eq!(detector.detect(&table), vec!["lowHealth".to_string()]);
    }

    #[test]
    fn predicate_uses_live_store_slots_and_rearms_after_false() {
        let ctx = predicate_ctx(0.0, 0.0);
        let reg = registry_with(vec![predicate_crossing(
            both_thresholds_predicate(),
            &["bothReady"],
        )]);
        let mut detector = CrossingDetector::new();
        detector.initialize(&reg, &ctx.slot_table.borrow(), &ctx);
        assert_eq!(detector.watcher_count(), 1);

        // The first source alone does not satisfy the nested select's true arm.
        set_ctx(&ctx, "test.a", 2.0);
        assert!(detector.detect(&ctx.slot_table.borrow()).is_empty());

        // The second live slot makes the full predicate false -> true.
        set_ctx(&ctx, "test.b", 1.0);
        assert_eq!(
            detector.detect(&ctx.slot_table.borrow()),
            vec!["bothReady".to_string()]
        );
        assert!(detector.detect(&ctx.slot_table.borrow()).is_empty());

        // Returning false re-arms; the next true edge fires exactly once.
        set_ctx(&ctx, "test.a", 0.0);
        assert!(detector.detect(&ctx.slot_table.borrow()).is_empty());
        set_ctx(&ctx, "test.a", 2.0);
        assert_eq!(
            detector.detect(&ctx.slot_table.borrow()),
            vec!["bothReady".to_string()]
        );
    }

    #[test]
    fn non_boolean_predicate_is_rejected_at_bind() {
        let ctx = predicate_ctx(0.0, 0.0);
        let reg = registry_with(vec![predicate_crossing(
            IrNode::Add {
                a: number(1.0),
                b: number(2.0),
            },
            &["never"],
        )]);
        let mut detector = CrossingDetector::new();
        detector.initialize(&reg, &ctx.slot_table.borrow(), &ctx);

        assert_eq!(
            detector.watcher_count(),
            0,
            "Number-root predicates must be rejected rather than registered"
        );
        assert!(detector.detect(&ctx.slot_table.borrow()).is_empty());
    }

    // NOTE: the AC-3 contract test (styleRanges display value vs. crossing
    // authoritative slot diverging mid-tween) lives in
    // `postretro-ui` tree style-ranges tests. It crosses the UI CPU model /
    // scripting-core boundary, so it does not belong in this detector test set.

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
        detector.initialize(&reg, &table, &ScriptCtx::new());
        assert_eq!(detector.watcher_count(), 1);

        detector.clear();
        assert_eq!(detector.watcher_count(), 0);
        assert!(detector.detect(&table).is_empty());
    }
}
