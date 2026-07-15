//! Install-time binding and authoritative per-tick evaluation for state-slot
//! accumulators. Raw expressions remain on `SlotSchema`; this binary-side table
//! owns the scope-specialized programs for the active level.

use std::collections::BTreeMap;

use postretro_entities::ScriptCtx;
use postretro_foundation::{
    BakedIr, BoundProgram, CURRENT_IR_VERSION, IrNode, IrType, IrValue, bind, eval_and_write,
};
use postretro_scripting_core::ir_scopes::DispatchScope;

const TICK_INPUTS: &[(&str, IrType)] = &[("@dt", IrType::Number)];

/// Bound accumulator programs keyed by their fully-qualified slot name.
/// `BTreeMap` pins deterministic evaluation order without a per-tick sort.
#[derive(Default)]
pub(crate) struct SlotAccumulatorBindings {
    programs: BTreeMap<String, BoundProgram<DispatchScope>>,
    scope: Option<DispatchScope>,
}

impl SlotAccumulatorBindings {
    /// Rebuild bindings against the committed store and the active level's
    /// ambient slots. A rejected program is inert for this level; other slots
    /// remain bound and operational.
    pub(crate) fn rebuild(&mut self, script_ctx: &ScriptCtx) {
        self.programs.clear();
        self.scope = None;
        let declarations = script_ctx
            .slot_table
            .borrow()
            .iter()
            .filter_map(|(name, record)| {
                record
                    .schema
                    .accumulate
                    .clone()
                    .map(|expr| (name.to_string(), expr))
            })
            .collect::<Vec<_>>();

        let scope = DispatchScope::script(script_ctx.clone(), TICK_INPUTS);
        for (slot, delta) in declarations {
            let baked = BakedIr {
                version: CURRENT_IR_VERSION,
                output: Some(slot.clone()),
                root: IrNode::Add {
                    a: Box::new(IrNode::Input { name: slot.clone() }),
                    b: Box::new(delta),
                },
            };
            match bind(&baked, &scope) {
                Ok(program) => {
                    self.programs.insert(slot, program);
                }
                Err(error) => log::warn!(
                    "[Scripting] slot accumulator `{slot}` is inert for this level: {error}"
                ),
            }
        }
        self.scope = Some(scope);
    }

    #[cfg(test)]
    fn len(&self) -> usize {
        self.programs.len()
    }
}

/// Evaluate every active accumulator after one authoritative simulation tick.
/// Callers own the authority guard; connected clients must never invoke this.
pub(crate) fn evaluate_slot_accumulators(
    bindings: &mut SlotAccumulatorBindings,
    _script_ctx: &ScriptCtx,
    tick_dt: f32,
) {
    let SlotAccumulatorBindings { programs, scope } = bindings;
    let Some(scope) = scope.as_mut() else {
        return;
    };
    if let Err(error) = scope.seed("@dt", IrValue::Number(tick_dt)) {
        log::warn!("[Scripting] slot accumulator tick input was not seeded: {error:?}");
        return;
    }
    for program in programs.values() {
        eval_and_write(program, scope);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use postretro_entities::{
        NumericRange, ReplicationScope, SlotOwnership, SlotRecord, SlotSchema, SlotType, SlotValue,
    };

    fn number_slot(default: f32, range: Option<NumericRange>, accumulate: IrNode) -> SlotRecord {
        SlotRecord::new(SlotSchema {
            slot_type: SlotType::Number,
            default: Some(SlotValue::Number(default)),
            range,
            persist: false,
            readonly: false,
            ownership: SlotOwnership::Mod,
            network: ReplicationScope::None,
            accumulate: Some(accumulate),
        })
    }

    #[test]
    fn accumulator_composes_delta_and_clamps_through_store_write() {
        let ctx = ScriptCtx::new();
        ctx.slot_table
            .borrow_mut()
            .insert(
                "timer.remaining".into(),
                number_slot(
                    1.0,
                    Some(NumericRange { min: 0.0, max: 1.0 }),
                    IrNode::Mul {
                        a: Box::new(IrNode::Input { name: "@dt".into() }),
                        b: Box::new(IrNode::Const {
                            value: IrValue::Number(-2.0),
                        }),
                    },
                ),
            )
            .unwrap();
        let mut bindings = SlotAccumulatorBindings::default();
        bindings.rebuild(&ctx);

        evaluate_slot_accumulators(&mut bindings, &ctx, 1.0);

        assert_eq!(
            ctx.slot_table
                .borrow()
                .get("timer.remaining")
                .unwrap()
                .value,
            Some(SlotValue::Number(0.0))
        );
    }

    #[test]
    fn missing_ambient_input_leaves_only_that_accumulator_inert() {
        let ctx = ScriptCtx::new();
        let mut table = ctx.slot_table.borrow_mut();
        table
            .insert(
                "timer.good".into(),
                number_slot(1.0, None, IrNode::Input { name: "@dt".into() }),
            )
            .unwrap();
        table
            .insert(
                "timer.missing".into(),
                number_slot(
                    1.0,
                    None,
                    IrNode::Input {
                        name: "trigger.absent.occupants".into(),
                    },
                ),
            )
            .unwrap();
        drop(table);
        let mut bindings = SlotAccumulatorBindings::default();
        bindings.rebuild(&ctx);
        assert_eq!(bindings.len(), 1);

        evaluate_slot_accumulators(&mut bindings, &ctx, 0.25);
        assert_eq!(
            ctx.slot_table.borrow().get("timer.good").unwrap().value,
            Some(SlotValue::Number(1.25))
        );
        assert_eq!(
            ctx.slot_table.borrow().get("timer.missing").unwrap().value,
            Some(SlotValue::Number(1.0))
        );
    }

    #[test]
    fn accumulators_evaluate_in_sorted_slot_name_order() {
        let ctx = ScriptCtx::new();
        let mut table = ctx.slot_table.borrow_mut();
        table
            .insert(
                "order.z_source".into(),
                number_slot(1.0, None, IrNode::Input { name: "@dt".into() }),
            )
            .unwrap();
        table
            .insert(
                "order.a_observer".into(),
                number_slot(
                    0.0,
                    None,
                    IrNode::Input {
                        name: "order.z_source".into(),
                    },
                ),
            )
            .unwrap();
        drop(table);
        let mut bindings = SlotAccumulatorBindings::default();
        bindings.rebuild(&ctx);

        evaluate_slot_accumulators(&mut bindings, &ctx, 1.0);

        assert_eq!(
            ctx.slot_table
                .borrow()
                .get("order.a_observer")
                .unwrap()
                .value,
            Some(SlotValue::Number(1.0)),
            "a_observer must read z_source before z_source's later sorted write"
        );
        assert_eq!(
            ctx.slot_table.borrow().get("order.z_source").unwrap().value,
            Some(SlotValue::Number(2.0))
        );
    }
}
