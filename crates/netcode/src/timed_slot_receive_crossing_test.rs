// Regression: two applied snapshots could cross a timer threshold and cancel
// inside one frame, hiding the intermediate edge from frame-final detection.
use super::*;
use crate::frame_order::{self, ReplicatedStateFrame};
use postretro_entities::{
    CrossingCondition, CrossingDescriptor, DataRegistry, NamedReaction, NumericRange,
    PrimitiveDescriptor, ReactionDescriptor, ScriptCtx, SlotOwnership, SlotRecord, SlotSchema,
    SlotType,
};
use postretro_foundation::{IrNode, IrValue};
use postretro_scripting_core::reaction_registry::{
    ReactionPrimitiveRegistry, SystemReactionCommand, SystemReactionRegistry,
};
use postretro_scripting_core::sequence::SequencedPrimitiveRegistry;
use postretro_scripting_core::state_crossings::{CrossingDetector, CrossingFire};
use postretro_sim::scripting::reactions::dispatch_crossing_fires_with_sequences;
use postretro_sim::scripting_systems::system_reactions::{
    SystemReactionIrBindings, SystemReactionIrDispatch, register_system_reaction_primitives,
};

struct CrossingFrame {
    receiver: Receiver,
    ctx: ScriptCtx,
    detector: CrossingDetector,
    sequences: SequencedPrimitiveRegistry,
    reactions: ReactionPrimitiveRegistry,
    systems: SystemReactionRegistry,
    bindings: SystemReactionIrBindings,
    fires: Vec<CrossingFire>,
    observations: Vec<[Option<Option<f32>>; 4]>,
    /// Direction and ambient raw value at each reaction's immediate command drain.
    drained: Vec<(String, bool, f32)>,
}

fn local_number(value: f32) -> SlotRecord {
    SlotRecord::new(SlotSchema {
        slot_type: SlotType::Number,
        default: Some(SlotValue::Number(value)),
        range: Some(NumericRange { min: 0.0, max: 1.0 }),
        persist: false,
        readonly: false,
        ownership: SlotOwnership::Mod,
        network: Default::default(),
        per_owner: false,
        accumulate: None,
    })
}

impl CrossingFrame {
    fn new() -> Self {
        let ctx = ScriptCtx::new();
        {
            let mut table = ctx.slot_table.borrow_mut();
            for name in ["local", "direction", "ambient"] {
                table
                    .insert(format!("acceptance.{name}"), local_number(0.0))
                    .unwrap();
            }
        }
        let mut data = DataRegistry::new();
        let reactions = ["above", "both", "predicate", "other"]
            .into_iter()
            .flat_map(|name| {
                [
                    (
                        "acceptance.direction",
                        serde_json::json!({
                            "op": "select", "cond": {"op": "input", "name": "@rising"},
                            "a": {"op": "const", "value": 1}, "b": {"op": "const", "value": 0}
                        }),
                    ),
                    (
                        "acceptance.ambient",
                        serde_json::json!({
                            "op": "input", "name": "player.reloadProgress"
                        }),
                    ),
                ]
                .map(|(slot, value)| NamedReaction {
                    name: name.to_string(),
                    descriptor: ReactionDescriptor::Primitive(PrimitiveDescriptor {
                        primitive: "setState".to_string(),
                        target: None,
                        kind: None,
                        tag: None,
                        on_complete: None,
                        args: serde_json::json!({"slot": slot, "value": value}),
                    }),
                })
            })
            .collect();
        let threshold = |slot: &str, name: &str, both: bool| CrossingDescriptor {
            slot: Some(slot.to_string()),
            condition: CrossingCondition::Above { threshold: 0.5 },
            max: 1.0,
            edge: both.then(|| "both".to_string()),
            fire: vec![name.to_string()],
        };
        data.populate_level(
            reactions,
            vec![
                threshold("player.reloadProgress", "above", false),
                threshold("player.reloadProgress", "both", true),
                CrossingDescriptor {
                    slot: None,
                    condition: CrossingCondition::Ir(IrNode::Ge {
                        a: Box::new(IrNode::Input {
                            name: "acceptance.local".to_string(),
                            owner: None,
                        }),
                        b: Box::new(IrNode::Const {
                            value: IrValue::Number(0.5),
                        }),
                    }),
                    max: 1.0,
                    edge: None,
                    fire: vec!["predicate".to_string()],
                },
                threshold("acceptance.local", "other", false),
            ],
            &[],
        );
        let mut systems = SystemReactionRegistry::new();
        register_system_reaction_primitives(&mut systems);
        let mut bindings = SystemReactionIrBindings::default();
        bindings.rebuild(&data, &ctx);
        *ctx.data_registry.borrow_mut() = data;
        Self {
            receiver: Receiver::new(),
            ctx,
            detector: CrossingDetector::new(),
            sequences: SequencedPrimitiveRegistry::new(),
            reactions: ReactionPrimitiveRegistry::new(),
            systems,
            bindings,
            fires: Vec::new(),
            observations: Vec::new(),
            drained: Vec::new(),
        }
    }

    fn run(&mut self, frame: u64, local_write: bool) -> Vec<String> {
        let applied = frame_order::run_snapshot_apply_stage(self, frame, 1.0 / 60.0);
        // Match App: local writes settle between apply and crossing dispatch.
        if local_write {
            self.ctx
                .slot_table
                .borrow_mut()
                .get_mut("acceptance.local")
                .unwrap()
                .write_value(Some(SlotValue::Number(1.0)));
        }
        frame_order::run_crossing_stage(self, frame, applied)
    }

    fn arm(&mut self) {
        let baseline = self.receiver.timer_snapshot(1, 100, 0.375);
        self.receiver.enqueue(wire::encode(&baseline));
        assert!(self.run(1, false).is_empty());
        self.detector.initialize(
            &self.ctx.data_registry.borrow(),
            &self.ctx.slot_table.borrow(),
            &self.ctx,
        );
    }
}

impl ReplicatedStateFrame for CrossingFrame {
    fn apply_received_snapshots(&mut self, _: f32) {
        let receiver = &mut self.receiver;
        receiver.client.update_connections(FRAME);
        receiver.client.drain_control();
        client_receive_and_apply(
            &mut receiver.registry,
            &mut self.ctx.slot_table.borrow_mut(),
            &FactionRegistry::default(),
            &RefCell::new(FactionSentimentState::default()),
            &ReplicatedSlotIdentity::default(),
            &mut receiver.client,
            &mut receiver.replication,
            &mut receiver.slots,
            &mut receiver.prediction,
            &[],
            &HitZoneStore::default(),
            None,
            &CollisionWorld::new(),
            &EnvironmentVolumes::default(),
            1.0 / 60.0,
            Duration::ZERO,
            None,
            None,
            None,
            false,
            &mut NoPresentationBindings,
            0.0,
            None,
        );
        self.observations = receiver.slots.timed_slot_pending_observations().to_vec();
    }

    fn dispatch_state_crossings(&mut self) -> Vec<String> {
        self.fires = self
            .receiver
            .slots
            .detect_state_crossings(&mut self.detector, &self.ctx.slot_table.borrow());
        let names = dispatch_crossing_fires_with_sequences(
            self.fires.clone(),
            &self.ctx.data_registry.borrow(),
            &self.sequences,
            &self.reactions,
            &self.systems,
            &self.ctx,
        );
        // App immediately drains these commands after the crossing stage.
        for command in self.ctx.system_commands.take() {
            let SystemReactionCommand::SetState {
                slot,
                value,
                dispatch_source,
                dispatch_values,
            } = command
            else {
                panic!("unexpected crossing command")
            };
            assert_eq!(
                self.bindings.dispatch(
                    &slot,
                    &value,
                    &dispatch_source,
                    &dispatch_values,
                    &self.ctx
                ),
                SystemReactionIrDispatch::Evaluated
            );
            if slot == "acceptance.ambient" {
                let table = self.ctx.slot_table.borrow();
                let value = |name| match table.get(name).unwrap().value.as_ref() {
                    Some(SlotValue::Number(value)) => *value,
                    _ => panic!("expected numeric reaction output"),
                };
                self.drained.push((
                    dispatch_source,
                    value("acceptance.direction") > 0.5,
                    value("acceptance.ambient"),
                ));
            }
        }
        names
    }
}

#[test]
fn timed_slot_receive_crossings_preserve_order_and_dispatch_after_local_writes() {
    let mut frame = CrossingFrame::new();
    frame.arm();
    for (sequence, tick, progress) in [(2, 102, 0.75), (3, 104, 0.0)] {
        let snapshot = frame.receiver.timer_snapshot(sequence, tick, progress);
        frame.receiver.enqueue(wire::encode(&snapshot));
    }
    assert_eq!(
        frame.run(2, true),
        ["above", "both", "both", "predicate", "other"]
    );
    assert_eq!(
        frame
            .fires
            .iter()
            .map(|fire| (fire.source_id, fire.rising))
            .collect::<Vec<_>>(),
        [(0, true), (1, true), (1, false), (2, true), (3, true)]
    );
    assert_eq!(frame.drained.len(), 5);
    for (index, (source, direction, raw)) in frame.drained.iter().enumerate() {
        assert_eq!(
            source,
            &format!("crossing:{}", frame.fires[index].source_id)
        );
        assert_eq!(*direction, frame.fires[index].rising);
        close(*raw, 0.0);
    }
    let table = frame.ctx.slot_table.borrow();
    let mut display = table
        .iter()
        .filter_map(|(name, record)| record.value.clone().map(|value| (name.to_string(), value)))
        .collect();
    frame.receiver.slots.apply_timed_slot_targets(&mut display);
    assert!(matches!(table.get("player.reloadProgress").unwrap().value,
        Some(SlotValue::Number(value)) if value.abs() < 1e-6));
    assert!(matches!(display.get("player.reloadProgress"),
        Some(SlotValue::Number(value)) if value.abs() < 1e-6));
    drop(table);
    assert!(frame.run(3, false).is_empty());
    assert_eq!(frame.drained.len(), 5, "no next-frame repeat");
}

#[test]
fn timed_slot_receive_crossings_skip_rejected_duplicate_older_deferred_and_refresh_slots() {
    let mut frame = CrossingFrame::new();
    frame.arm();
    let mut rejected = frame.receiver.timer_snapshot(2, 102, 0.75);
    rejected.state_schema_fingerprint[0] ^= 1;
    frame.receiver.enqueue(wire::encode(&rejected));
    let mut refresh = frame.receiver.timer_snapshot(3, 104, 0.75);
    let record = &mut refresh.state_records[0];
    record.kind = STATE_RECORD_KIND_DELTA;
    record.has_baseline_ref = true;
    record.baseline_id = 1;
    record.baseline_ref = 1;
    frame.receiver.enqueue(wire::encode(&refresh));
    assert!(frame.run(2, false).is_empty());
    assert_eq!(
        frame.observations.len(),
        1,
        "rejected batch adds no observations"
    );
    assert!(
        frame.observations[0][0].is_none(),
        "refresh-held reload is excluded"
    );
    // Duplicate/older sequence gates reject otherwise valid crossing values.
    for sequence in [3, 2] {
        let snapshot = frame.receiver.timer_snapshot(sequence, 106, 0.75);
        frame.receiver.enqueue(wire::encode(&snapshot));
    }
    let mut deferred = frame.receiver.timer_snapshot(4, 108, 0.75);
    deferred.records.push(descriptor_record());
    frame.receiver.enqueue(wire::encode(&deferred));
    assert!(frame.run(3, false).is_empty());
    assert!(
        frame.observations.is_empty(),
        "duplicate, older and deferred batches add no observations"
    );
    let valid = frame.receiver.state_snapshot(5, 110, &[]);
    frame.receiver.enqueue(wire::encode(&valid));
    assert!(
        frame.run(4, false).is_empty(),
        "held observations do not fabricate edges"
    );
    assert_eq!(frame.observations.len(), 1);
    close(frame.observations[0][0].flatten().unwrap(), 0.375);
    // A held-baseline resend is also a held-value observation, despite its
    // carried payload being ignored by state apply.
    let mut resend = frame.receiver.timer_snapshot(6, 112, 0.9);
    let record = &mut resend.state_records[0];
    record.kind = STATE_RECORD_KIND_DELTA;
    record.has_baseline_ref = true;
    record.baseline_ref = 2;
    record.baseline_id = 2;
    frame.receiver.enqueue(wire::encode(&resend));
    assert!(frame.run(5, false).is_empty());
    assert_eq!(frame.observations.len(), 1);
    close(frame.observations[0][0].flatten().unwrap(), 0.375);
    let applied = frame.receiver.timer_snapshot(7, 114, 0.75);
    frame.receiver.enqueue(wire::encode(&applied));
    assert_eq!(frame.run(6, false), ["above", "both"]);
}
