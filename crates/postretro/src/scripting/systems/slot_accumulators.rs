//! Owns install-time accumulator bindings and authoritative post-tick
//! evaluation. See `context/lib/scripting.md` §§11–12.

use std::collections::BTreeMap;

use postretro_entities::ScriptCtx;
use postretro_foundation::{
    BakedIr, BoundProgram, CURRENT_IR_VERSION, IrType, IrValue, bind, eval_value,
};
use postretro_scripting_core::ir_scopes::DispatchScope;
use postretro_scripting_core::store_bridge::write_store_slot;

const TICK_INPUTS: &[(&str, IrType)] = &[("@dt", IrType::Number)];

/// Bound accumulator programs keyed by their fully-qualified slot name.
/// `BTreeMap` pins deterministic evaluation order without a per-tick sort.
#[derive(Default)]
pub(crate) struct SlotAccumulatorBindings {
    programs: BTreeMap<String, AccumulatorProgram>,
    scope: Option<DispatchScope>,
    script_ctx: Option<ScriptCtx>,
}

struct AccumulatorProgram {
    delta: BoundProgram<DispatchScope>,
    precise_value: f64,
    last_write_generation: u64,
}

impl SlotAccumulatorBindings {
    /// Rebuild bindings against the committed store and the active level's
    /// ambient slots. A rejected program is inert for this level; other slots
    /// remain bound and operational.
    pub(crate) fn rebuild(&mut self, script_ctx: &ScriptCtx) {
        self.clear();
        let declarations = script_ctx
            .slot_table
            .borrow()
            .iter()
            .filter_map(|(name, record)| {
                record.schema.accumulate.clone().map(|expr| {
                    let initial = match record.value.as_ref() {
                        Some(postretro_entities::SlotValue::Number(value)) => *value,
                        _ => 0.0,
                    };
                    (name.to_string(), expr, initial, record.write_generation())
                })
            })
            .collect::<Vec<_>>();

        let scope = DispatchScope::script(script_ctx.clone(), TICK_INPUTS);
        for (slot, delta, initial, write_generation) in declarations {
            let baked = BakedIr {
                version: CURRENT_IR_VERSION,
                output: Some(slot.clone()),
                root: delta,
            };
            match bind(&baked, &scope) {
                Ok(program) => {
                    self.programs.insert(
                        slot,
                        AccumulatorProgram {
                            delta: program,
                            precise_value: f64::from(initial),
                            last_write_generation: write_generation,
                        },
                    );
                }
                Err(error) => log::warn!(
                    "[Scripting] slot accumulator `{slot}` is inert for this level: {error}"
                ),
            }
        }
        self.scope = Some(scope);
        self.script_ctx = Some(script_ctx.clone());
    }

    /// Drop every binding derived from the active level.
    pub(crate) fn clear(&mut self) {
        self.programs.clear();
        self.scope = None;
        self.script_ctx = None;
    }

    #[cfg(test)]
    fn len(&self) -> usize {
        self.programs.len()
    }

    #[cfg(test)]
    pub(crate) fn is_empty(&self) -> bool {
        self.programs.is_empty() && self.scope.is_none() && self.script_ctx.is_none()
    }
}

/// Evaluate every active accumulator after one authoritative simulation tick.
/// Callers own the authority guard; connected clients must never invoke this.
pub(crate) fn evaluate_slot_accumulators(bindings: &mut SlotAccumulatorBindings, tick_dt: f32) {
    let SlotAccumulatorBindings {
        programs,
        scope,
        script_ctx,
    } = bindings;
    let (Some(scope), Some(script_ctx)) = (scope.as_mut(), script_ctx.as_ref()) else {
        return;
    };
    if let Err(error) = scope.seed("@dt", IrValue::Number(tick_dt)) {
        log::warn!("[Scripting] slot accumulator tick input was not seeded: {error:?}");
        return;
    }
    for (slot, accumulator) in programs {
        let (current, current_generation) = match script_ctx_number(script_ctx, slot) {
            Some(snapshot) => snapshot,
            None => continue,
        };
        if current_generation != accumulator.last_write_generation {
            accumulator.precise_value = f64::from(current);
            accumulator.last_write_generation = current_generation;
        }

        let IrValue::Number(delta) = eval_value(&accumulator.delta, scope) else {
            continue;
        };
        accumulator.precise_value += f64::from(delta);
        let next = accumulator.precise_value as f32;

        // Accumulate below f32 precision, then narrow once per tick. This keeps
        // repeated fixed-tick deltas on their simulated-time boundary without
        // moving any arbitrary value a delta early through an epsilon snap.
        if write_store_slot(
            script_ctx,
            slot,
            postretro_entities::SlotValue::Number(next),
        )
        .is_err()
        {
            // The store validator rejects non-finite results. Preserve the last
            // accepted visible value and discard the unrepresentable f64 state
            // so later finite deltas cannot teleport the slot from hidden state.
            accumulator.precise_value = f64::from(current);
            accumulator.last_write_generation = current_generation;
            continue;
        }
        let (written, written_generation) =
            script_ctx_number(script_ctx, slot).unwrap_or((next, current_generation));
        accumulator.last_write_generation = written_generation;
        if written != next {
            accumulator.precise_value = f64::from(written);
        }
    }
}

fn script_ctx_number(script_ctx: &ScriptCtx, slot: &str) -> Option<(f32, u64)> {
    let table = script_ctx.slot_table.borrow();
    let record = table.get(slot)?;
    match record.value.as_ref()? {
        postretro_entities::SlotValue::Number(value) => Some((*value, record.write_generation())),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use postretro_entities::{
        CrossingCondition, CrossingDescriptor, DataRegistry, NumericRange, ReplicationScope,
        SlotOwnership, SlotRecord, SlotSchema, SlotType, SlotValue,
    };
    use postretro_foundation::IrNode;
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
            per_owner: false,
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

        evaluate_slot_accumulators(&mut bindings, 1.0);

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

        let tick_dt = crate::frame_timing::TICK_DURATION.as_secs_f32();
        for tick in 1..=3_600 {
            // Production order: authoritative simulation tick, accumulator write,
            // then the frame's settled-state crossing detection.
            evaluate_slot_accumulators(&mut bindings, tick_dt);
            let fires = detector.detect(&ctx.slot_table.borrow());
            if tick < 3_600 {
                assert!(fires.is_empty(), "crossed early at tick {tick}");
            } else {
                assert_eq!(fires.len(), 1);
                assert_eq!(fires[0].reaction, "countdownComplete");
                assert!(fires[0].rising, "predicate condition became true");
            }
        }

        evaluate_slot_accumulators(&mut bindings, tick_dt);
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

        evaluate_slot_accumulators(&mut bindings, 0.25);
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

        evaluate_slot_accumulators(&mut bindings, 1.0);

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
    fn equal_value_authoritative_write_resets_hidden_precision() {
        let ctx = ScriptCtx::new();
        let visible = 16_777_216.0;
        ctx.slot_table
            .borrow_mut()
            .insert(
                "counter.value".into(),
                number_slot(
                    visible,
                    None,
                    IrNode::Const {
                        value: IrValue::Number(0.25),
                    },
                ),
            )
            .unwrap();
        let mut bindings = SlotAccumulatorBindings::default();
        bindings.rebuild(&ctx);

        // Three deltas remain hidden below this f32 value's ULP.
        for _ in 0..3 {
            evaluate_slot_accumulators(&mut bindings, 1.0);
        }
        write_store_slot(&ctx, "counter.value", SlotValue::Number(visible)).unwrap();

        // Regression: value comparison could not observe the equal-value write,
        // so the old hidden 0.75 residue made the second tick jump by one ULP.
        evaluate_slot_accumulators(&mut bindings, 1.0);
        evaluate_slot_accumulators(&mut bindings, 1.0);
        assert_eq!(
            ctx.slot_table.borrow().get("counter.value").unwrap().value,
            Some(SlotValue::Number(visible))
        );
    }

    #[test]
    fn different_value_authoritative_write_resets_hidden_precision() {
        let ctx = ScriptCtx::new();
        ctx.slot_table
            .borrow_mut()
            .insert(
                "counter.value".into(),
                number_slot(
                    16_777_216.0,
                    None,
                    IrNode::Const {
                        value: IrValue::Number(0.25),
                    },
                ),
            )
            .unwrap();
        let mut bindings = SlotAccumulatorBindings::default();
        bindings.rebuild(&ctx);
        for _ in 0..3 {
            evaluate_slot_accumulators(&mut bindings, 1.0);
        }

        write_store_slot(&ctx, "counter.value", SlotValue::Number(100.0)).unwrap();
        evaluate_slot_accumulators(&mut bindings, 1.0);

        assert_eq!(
            ctx.slot_table.borrow().get("counter.value").unwrap().value,
            Some(SlotValue::Number(100.25))
        );
    }

    #[test]
    fn non_finite_accumulator_results_preserve_last_valid_value() {
        for delta in [f32::MAX, f32::INFINITY] {
            let ctx = ScriptCtx::new();
            ctx.slot_table
                .borrow_mut()
                .insert(
                    "counter.value".into(),
                    number_slot(
                        f32::MAX,
                        None,
                        IrNode::Const {
                            value: IrValue::Number(delta),
                        },
                    ),
                )
                .unwrap();
            let mut bindings = SlotAccumulatorBindings::default();
            bindings.rebuild(&ctx);

            evaluate_slot_accumulators(&mut bindings, 1.0);
            evaluate_slot_accumulators(&mut bindings, 1.0);

            // Regression: rejected non-finite totals were replaced with zero,
            // bypassing the store validator and discarding the valid value.
            assert_eq!(
                ctx.slot_table.borrow().get("counter.value").unwrap().value,
                Some(SlotValue::Number(f32::MAX))
            );
        }
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
                        per_owner: false,
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
                evaluate_slot_accumulators(&mut bindings, 1.0);
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
