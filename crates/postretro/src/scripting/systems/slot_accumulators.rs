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
        CrossingCondition, CrossingDescriptor, DataRegistry, NumericRange, ReplicationScope,
        SlotOwnership, SlotRecord, SlotSchema, SlotType, SlotValue,
    };
    use postretro_scripting_core::state_crossings::CrossingDetector;

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
    fn sixty_second_countdown_clamps_and_crosses_on_the_completion_tick() {
        let ctx = ScriptCtx::new();
        ctx.slot_table
            .borrow_mut()
            .insert(
                "timer.remaining".into(),
                number_slot(
                    60.0,
                    Some(NumericRange {
                        min: 0.0,
                        max: 60.0,
                    }),
                    IrNode::Mul {
                        a: Box::new(IrNode::Input { name: "@dt".into() }),
                        b: Box::new(IrNode::Const {
                            value: IrValue::Number(-1.0),
                        }),
                    },
                ),
            )
            .unwrap();

        let mut data = DataRegistry::new();
        data.populate_level(
            Vec::new(),
            vec![CrossingDescriptor {
                slot: None,
                condition: CrossingCondition::Ir(IrNode::Le {
                    a: Box::new(IrNode::Input {
                        name: "timer.remaining".into(),
                    }),
                    b: Box::new(IrNode::Const {
                        value: IrValue::Number(0.0),
                    }),
                }),
                max: 1.0,
                edge: None,
                fire: vec!["countdownComplete".into()],
            }],
            &[],
        );
        let mut detector = CrossingDetector::new();
        detector.initialize(&data, &ctx.slot_table.borrow(), &ctx);
        let mut bindings = SlotAccumulatorBindings::default();
        bindings.rebuild(&ctx);

        for elapsed_seconds in 1..=60 {
            // Production order: authoritative simulation tick, accumulator write,
            // then the frame's settled-state crossing detection.
            evaluate_slot_accumulators(&mut bindings, &ctx, 1.0);
            let fires = detector.detect(&ctx.slot_table.borrow());
            if elapsed_seconds < 60 {
                assert!(fires.is_empty(), "crossed early at {elapsed_seconds}s");
            } else {
                assert_eq!(fires.len(), 1);
                assert_eq!(fires[0].reaction, "countdownComplete");
                assert!(fires[0].rising, "predicate condition became true");
            }
        }

        evaluate_slot_accumulators(&mut bindings, &ctx, 1.0);
        assert_eq!(
            ctx.slot_table
                .borrow()
                .get("timer.remaining")
                .unwrap()
                .value,
            Some(SlotValue::Number(0.0)),
            "the existing store write path clamps the countdown at its minimum"
        );
        assert!(
            detector.detect(&ctx.slot_table.borrow()).is_empty(),
            "remaining at the clamp is not another predicate edge"
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

    #[test]
    fn accumulator_and_both_edge_timeline_is_deterministic_run_to_run() {
        fn run() -> (Vec<f32>, Vec<(usize, bool)>) {
            let ctx = ScriptCtx::new();
            let mut table = ctx.slot_table.borrow_mut();
            table
                .insert(
                    "determinism.value".into(),
                    number_slot(
                        -2.0,
                        Some(NumericRange {
                            min: -10.0,
                            max: 10.0,
                        }),
                        IrNode::Mul {
                            a: Box::new(IrNode::Input { name: "@dt".into() }),
                            b: Box::new(IrNode::Input {
                                name: "determinism.rate".into(),
                            }),
                        },
                    ),
                )
                .unwrap();
            table
                .insert(
                    "determinism.rate".into(),
                    SlotRecord::new(SlotSchema {
                        slot_type: SlotType::Number,
                        default: Some(SlotValue::Number(1.0)),
                        range: None,
                        persist: false,
                        readonly: false,
                        ownership: SlotOwnership::Mod,
                        network: ReplicationScope::None,
                        accumulate: None,
                    }),
                )
                .unwrap();
            drop(table);

            let mut data = DataRegistry::new();
            data.populate_level(
                Vec::new(),
                vec![CrossingDescriptor {
                    slot: Some("determinism.value".into()),
                    condition: CrossingCondition::Above { threshold: 0.0 },
                    max: 1.0,
                    edge: Some("both".into()),
                    fire: vec!["zeroEdge".into()],
                }],
                &[],
            );
            let mut detector = CrossingDetector::new();
            detector.initialize(&data, &ctx.slot_table.borrow(), &ctx);
            let mut bindings = SlotAccumulatorBindings::default();
            bindings.rebuild(&ctx);
            let mut timeline = Vec::new();
            let mut fires = Vec::new();

            for tick in 1..=6 {
                if tick == 4 {
                    ctx.slot_table
                        .borrow_mut()
                        .get_mut("determinism.rate")
                        .unwrap()
                        .value = Some(SlotValue::Number(-1.0));
                }
                evaluate_slot_accumulators(&mut bindings, &ctx, 1.0);
                let value = match ctx
                    .slot_table
                    .borrow()
                    .get("determinism.value")
                    .unwrap()
                    .value
                    .as_ref()
                {
                    Some(SlotValue::Number(value)) => *value,
                    other => panic!("expected deterministic number, got {other:?}"),
                };
                timeline.push(value);
                fires.extend(
                    detector
                        .detect(&ctx.slot_table.borrow())
                        .into_iter()
                        .map(|fire| (tick, fire.rising)),
                );
            }
            (timeline, fires)
        }

        let baseline = run();
        assert_eq!(run(), baseline);
        assert_eq!(baseline.0, vec![-1.0, 0.0, 1.0, 0.0, -1.0, -2.0]);
        assert_eq!(baseline.1, vec![(3, true), (5, false)]);
    }
}
