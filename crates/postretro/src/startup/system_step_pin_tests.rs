// Ordering pins S1–S8 and S27 for system steps: order and reads within a body,
// scoped reads across a wait, and cancel and restart, through the production
// install validation, the trigger binder, the scheduler and the app drain.
// See: context/lib/scripting.md §10.4 (System steps), §12 (Steps after a `wait`)

use super::*;
use crate::App;
use crate::scripting_systems::reaction_scheduler::SchedulerRole;
use crate::startup::lifecycle::tests::{script_ctx, test_app};
use postretro_entities::{FireOrigin, FireSubject, SystemCommandFireContext, SystemReactionCommand};
use postretro_foundation::Seat;
use postretro_scripting_core::reaction_dispatch::{
    dispatch_deferred_named_events_with_sequences, fire_named_event_with_sequences,
};
use std::cell::RefCell;
use std::rc::Rc;

fn system_step(primitive: &str, args: serde_json::Value) -> SequenceStep {
    SequenceStep {
        id: SequenceTarget::System,
        primitive: primitive.to_string(),
        args,
    }
}
fn set(slot: &str, value: serde_json::Value) -> SequenceStep {
    system_step("setState", json!({ "slot": slot, "value": value }))
}
fn read(slot: &str) -> serde_json::Value {
    json!({ "op": "input", "name": slot })
}
fn sound(name: &str) -> SequenceStep {
    system_step("playSound", json!({ "sound": name }))
}

/// A host session wired as `Session::build` wires reactions: system
/// primitives, `wait`/`fire`, a host scheduler, and a `note` member step that
/// records the value of `pin.a` when it runs.
struct Rig {
    app: App,
    ctx: ScriptCtx,
    notes: Rc<RefCell<Vec<f32>>>,
    /// Every command the app drain received, in order.
    drained: Vec<SystemReactionCommand>,
}

impl Rig {
    fn new(reactions: impl FnOnce(&ScriptCtx) -> Vec<NamedReaction>) -> Self {
        let app = test_app();
        let ctx = script_ctx(&app);
        for slot in ["pin.a", "pin.b", "pin.s", "pin.alarm"] {
            insert_writable_number(&ctx, slot);
        }
        let reactions = reactions(&ctx);
        ctx.data_registry
            .borrow_mut()
            .populate_level(reactions, Vec::new(), &[]);
        let mut app = app;
        let notes = Rc::new(RefCell::new(Vec::new()));
        {
            let session = app.session.as_mut().unwrap();
            let scripting = &mut session.scripting;
            crate::scripting::reactions::system_commands::register_system_reaction_primitives(
                &mut scripting.system_registry,
            );
            register_reaction_control_primitives(
                &mut scripting.sequence_registry,
                scripting.scheduler.clone(),
            );
            let (notes, ctx) = (notes.clone(), ctx.clone());
            scripting.sequence_registry.register("note", move |_id, _args| {
                let a = ctx
                    .slot_table
                    .borrow()
                    .get("pin.a")
                    .and_then(|record| match record.value {
                        Some(SlotValue::Number(value)) => Some(value),
                        _ => None,
                    })
                    .unwrap_or(f32::NAN);
                notes.borrow_mut().push(a);
                Ok(())
            });
            scripting.scheduler.set_role(SchedulerRole::Host);
        }
        Self {
            app,
            ctx,
            notes,
            drained: Vec::new(),
        }
    }

    /// Install validation as a level install runs it; returns the trigger
    /// bindings it built.
    fn install(&mut self) -> TriggerBindingTable {
        let table = validated_bindings(&self.ctx);
        let data = self.ctx.data_registry.borrow();
        self.app
            .session
            .as_mut()
            .unwrap()
            .scripting
            .system_reaction_ir_bindings
            .rebuild(&data, &self.ctx);
        table
    }

    fn fire(&mut self, name: &str) {
        let scripting = &self.app.session.as_ref().unwrap().scripting;
        let data = self.ctx.data_registry.borrow();
        let chained = fire_named_event_with_sequences(
            name,
            &data,
            &scripting.sequence_registry,
            &scripting.reaction_registry,
            &scripting.system_registry,
            &self.ctx,
            None,
        );
        dispatch_deferred_named_events_with_sequences(
            chained,
            &data,
            &scripting.sequence_registry,
            &scripting.reaction_registry,
            &scripting.system_registry,
            &self.ctx,
        );
    }

    /// Fire `trigger`'s Enter binding as the tick does (in-tick commands
    /// apply now), then drain its residual under the trigger's fire context,
    /// as the frame end does.
    fn enter(&mut self, table: &TriggerBindingTable, trigger: EntityId, pawn: EntityId) {
        let execution = {
            let mut registry = self.ctx.registry.borrow_mut();
            let mut slot_table = self.ctx.slot_table.borrow_mut();
            table.execute(
                trigger,
                TriggerEventEdge::Enter,
                &mut registry,
                &mut slot_table,
                &crate::trigger_commands::TriggerFireContext {
                    fired_trigger: Some(trigger),
                    activator: Some(pawn),
                    ..Default::default()
                },
            )
        };
        let Some(steps) = execution
            .residual()
            .and_then(|handle| table.residual(handle))
            .map(|residual| residual.steps().to_vec())
        else {
            return;
        };
        let scripting = &self.app.session.as_ref().unwrap().scripting;
        let _origin = scripting.scheduler.begin_origin(
            trigger,
            crate::trigger_system::PlayerId::Local(pawn),
            true,
        );
        let previous = self
            .ctx
            .system_commands
            .replace_fire_context(SystemCommandFireContext {
                source: format!("trigger:{trigger:?}"),
                origin: FireOrigin::HostOnly(FireSubject::Trigger {
                    player: Some(Seat(0)),
                    trigger,
                    occupants: vec![Seat(0)],
                }),
                ..Default::default()
            });
        fire_prepartitioned_reactions_with_sequences(
            &steps,
            &scripting.sequence_registry,
            &scripting.reaction_registry,
            &scripting.system_registry,
            &self.ctx,
            ResidualOrigin::TriggerBinding,
        );
        self.ctx.system_commands.replace_fire_context(previous);
    }

    /// One frame: a tick (with these trigger Exit fires), the landing drain,
    /// then the app's system-command drain.
    fn frame(&mut self, exits: &[(EntityId, EntityId)]) {
        let scripting = &self.app.session.as_ref().unwrap().scripting;
        scripting.scheduler.begin_frame();
        let exits: Vec<_> = exits
            .iter()
            .map(|(trigger, pawn)| (*trigger, crate::trigger_system::PlayerId::Local(*pawn)))
            .collect();
        scripting.scheduler.evaluate(&exits);
        scripting.scheduler.drain_landings(
            &self.ctx.data_registry.borrow(),
            &scripting.sequence_registry,
            &scripting.reaction_registry,
            &scripting.system_registry,
            &self.ctx,
        );
        self.drain();
    }

    /// The app's frame-end system-command drain, recording what it received.
    fn drain(&mut self) {
        let queued = self.ctx.system_commands.take_ordered();
        for (_, command) in queued {
            self.drained.push(command.clone());
            self.ctx.system_commands.push(command);
        }
        self.app.dispatch_system_commands();
    }

    fn frames(&mut self, count: usize) {
        for _ in 0..count {
            self.frame(&[]);
        }
    }

    fn number(&self, slot: &str) -> f32 {
        self.ctx
            .slot_table
            .borrow()
            .get(slot)
            .and_then(|record| match record.value {
                Some(SlotValue::Number(value)) => Some(value),
                _ => None,
            })
            .unwrap()
    }

    fn plays(&self, name: &str) -> usize {
        self.drained
            .iter()
            .filter(|command| matches!(command, SystemReactionCommand::PlaySound { sound, .. } if sound == name))
            .count()
    }

    fn pending(&self) -> usize {
        self.app.session.as_ref().unwrap().scripting.scheduler.pending_len()
    }
}

fn spawn_pawn(ctx: &ScriptCtx) -> EntityId {
    ctx.registry.borrow_mut().spawn(Transform::default())
}

// S1: a member step runs during the walk; system steps apply at the drain in
// authored order, each reading at its own landing.
#[test]
fn s1_mixed_body_runs_member_steps_in_the_walk_and_system_steps_in_authored_order() {
    let mut rig = Rig::new(|ctx| {
        vec![sequence(
            "mixed",
            vec![
                note_step(ctx),
                set("pin.a", json!(1)),
                sound("x"),
                set("pin.b", read("pin.a")),
            ],
        )]
    });
    let _ = rig.install();
    rig.fire("mixed");
    assert_eq!(*rig.notes.borrow(), [0.0], "the member step ran in the walk");
    assert_eq!(rig.number("pin.a"), 0.0, "system steps wait for the drain");
    rig.drain();
    assert_eq!(rig.number("pin.a"), 1.0);
    assert_eq!(rig.number("pin.b"), 1.0, "`b` reads `a` after `a` landed");
    assert_eq!(rig.plays("x"), 1, "the sound drains the same frame");
}

// S2: a trigger-bound pre-wait write applies in the firing tick, the
// pre-wait sound at that frame's drain, and the post-wait sound at landing.
#[test]
fn s2_trigger_bound_pre_wait_write_lands_in_tick_and_the_tail_at_landing() {
    let mut rig = Rig::new(|_| {
        vec![sequence(
            "plate",
            vec![
                set("pin.alarm", json!(1)),
                sound("x"),
                wait_step(json!(800), false),
                sound("y"),
            ],
        )]
    });
    let trigger = spawn_trigger(&rig.ctx, "plate", "", TriggerFireMode::Multiple, &[]);
    let pawn = spawn_pawn(&rig.ctx);
    let table = rig.install();
    rig.enter(&table, trigger, pawn);
    assert_eq!(rig.number("pin.alarm"), 1.0, "`alarm` is written in the firing tick");
    rig.frame(&[]);
    assert_eq!((rig.plays("x"), rig.plays("y")), (1, 0));
    rig.frames(46);
    assert_eq!(rig.plays("y"), 0, "800 ms is 48 ticks");
    rig.frames(2);
    assert_eq!(rig.plays("y"), 1, "`y` lands with the tail");
}

// S3: an IR-valued write after a wait reads at its landing, binds at install
// like the same body does, and a per-owner target is rejected the same way.
#[test]
fn s3_post_wait_ir_write_reads_at_its_landing() {
    let mut rig = Rig::new(|_| {
        vec![sequence(
            "scoped",
            vec![
                set("pin.a", json!(1)),
                wait_step(json!(300), false),
                set("pin.b", read("pin.a")),
            ],
        )]
    });
    let _ = rig.install();
    let capture = LogCapture::start();
    rig.fire("scoped");
    rig.frame(&[]);
    assert_eq!(rig.number("pin.a"), 1.0);
    rig.ctx.system_commands.push(SystemReactionCommand::SetState {
        slot: "pin.a".into(),
        value: json!(2),
        dispatch_source: "test".into(),
        dispatch_values: Vec::new(),
    });
    rig.drain();
    rig.frames(20);
    assert_eq!(rig.number("pin.b"), 2.0, "`b` reads `a` at landing");
    capture.assert_not_logged(log::Level::Warn, "not bound at level install");
    drop(capture);

    // A per-owner target warns at install exactly as the same body does.
    for descriptor in [
        ReactionDescriptor::Sequence(vec![set("pin.owned", read("pin.a"))]),
        set_state("owned", "pin.owned", read("pin.a")).descriptor,
    ] {
        let ctx = ScriptCtx::new();
        ctx.slot_table
            .borrow_mut()
            .insert(
                "pin.owned".into(),
                SlotRecord::new(SlotSchema {
                    slot_type: SlotType::Number,
                    default: Some(SlotValue::Number(0.0)),
                    range: None,
                    persist: false,
                    readonly: false,
                    ownership: SlotOwnership::Mod,
                    network: Default::default(),
                    per_owner: true,
                    accumulate: None,
                }),
            )
            .unwrap();
        ctx.data_registry.borrow_mut().populate_level(
            vec![NamedReaction {
                name: "owned".into(),
                descriptor,
            }],
            Vec::new(),
            &[],
        );
        let capture = LogCapture::start();
        let _ = built_system_bindings(&ctx);
        capture.assert_logged_once(
            log::Level::Warn,
            "setState reaction `owned` rejects per-owner slot `pin.owned`",
        );
    }
}

// S4: `at: on.emitter` before a wait installs; after one it drops the
// reaction at install, naming it.
#[test]
fn s4_emitter_read_survives_only_before_a_wait() {
    let at = |sound| {
        system_step(
            "playSound",
            json!({ "sound": sound, "at": postretro_entities::EMITTER_AT_TOKEN }),
        )
    };
    let ctx = ctx_with_reactions(vec![
        sequence("before", vec![at("x"), wait_step(json!(300), false), sound("y")]),
        sequence("after", vec![sound("x"), wait_step(json!(300), false), at("y")]),
    ]);
    let capture = LogCapture::start();
    validate_reaction_bodies_pass_a(&ctx);
    assert!(!is_dropped(&ctx, "before"));
    assert!(is_dropped(&ctx, "after"));
    capture.assert_logged_once(log::Level::Error, "reaction `after` step 2 (`playSound`)");
}

// S5: a dispatch input read after a wait drops the reaction; before it,
// the crossing's direction binds.
#[test]
fn s5_dispatch_input_read_survives_only_before_a_wait() {
    let rising = json!({ "op": "input", "name": "@rising" });
    let ctx = ctx_with_reactions(vec![
        sequence(
            "after",
            vec![wait_step(json!(300), false), set("pin.s", rising.clone())],
        ),
        sequence(
            "before",
            vec![set("pin.s", rising.clone()), wait_step(json!(300), false)],
        ),
    ]);
    insert_writable_number(&ctx, "pin.s");
    let capture = LogCapture::start();
    validate_reaction_bodies_pass_a(&ctx);
    assert!(is_dropped(&ctx, "after"));
    assert!(!is_dropped(&ctx, "before"));
    capture.assert_logged_once(log::Level::Error, "reaction `after` step 1 (`setState`)");
}

// S6: a `fire` target holding a scoped system step drops the firing
// reaction, wherever the `fire` sits (V4b over steps).
#[test]
fn s6_fire_of_a_reaction_with_a_scoped_system_step_drops_the_firer() {
    let rising = json!({ "op": "input", "name": "@rising" });
    for firer in [
        vec![fire_step("scoped")],
        vec![wait_step(json!(300), false), fire_step("scoped")],
    ] {
        let ctx = ctx_with_reactions(vec![
            sequence("scoped", vec![set("pin.s", rising.clone())]),
            sequence("firer", firer),
        ]);
        insert_writable_number(&ctx, "pin.s");
        let capture = LogCapture::start();
        validate_reaction_bodies_pass_a(&ctx);
        run_pass_b(&ctx);
        assert!(is_dropped(&ctx, "firer"));
        capture.assert_logged(log::Level::Error, "`firer`");
    }
}

// S7: an Exit before landing cancels the tail's system steps; an accepted
// re-fire restarts the body, so its pre-wait sound plays once more.
#[test]
fn s7_exit_cancels_a_tail_of_system_steps_and_a_refire_restarts_the_body() {
    let mut rig = Rig::new(|_| {
        vec![sequence(
            "plate",
            vec![
                sound("z"),
                wait_step(json!(800), true),
                system_step("screenShake", json!({ "amplitude": 4, "durationMs": 100 })),
                set("pin.s", json!(1)),
            ],
        )]
    });
    let trigger = spawn_trigger(&rig.ctx, "plate", "", TriggerFireMode::Multiple, &[]);
    let pawn = spawn_pawn(&rig.ctx);
    let table = rig.install();
    rig.enter(&table, trigger, pawn);
    rig.frames(10);
    rig.frame(&[(trigger, pawn)]);
    rig.frames(60);
    let shakes = |rig: &Rig| {
        rig.drained
            .iter()
            .filter(|command| matches!(command, SystemReactionCommand::ScreenShake { .. }))
            .count()
    };
    assert_eq!(shakes(&rig), 0, "no tail step runs after the Exit");
    assert_eq!(rig.number("pin.s"), 0.0);

    rig.enter(&table, trigger, pawn);
    rig.frames(10);
    rig.enter(&table, trigger, pawn);
    rig.frames(60);
    assert_eq!(rig.plays("z"), 3, "each fire plays `z`, the restart included");
    assert_eq!(shakes(&rig), 1, "the restarted tail lands once");
    assert_eq!(rig.number("pin.s"), 1.0);
}

// S8: an Exit and a re-entry on one tick: the old tail writes nothing, the
// re-entry plays its pre-wait steps once, and its fresh tail writes once.
#[test]
fn s8_exit_and_enter_on_one_tick_replace_the_tail() {
    let mut rig = Rig::new(|_| {
        vec![sequence(
            "plate",
            vec![
                set("pin.alarm", json!(1)),
                sound("x"),
                wait_step(json!(800), true),
                system_step(
                    "setState",
                    json!({ "slot": "pin.s", "value": {
                        "op": "add",
                        "a": read("pin.s"),
                        "b": { "op": "const", "value": 1.0 }
                    } }),
                ),
            ],
        )]
    });
    let trigger = spawn_trigger(&rig.ctx, "plate", "", TriggerFireMode::Multiple, &[]);
    let pawn = spawn_pawn(&rig.ctx);
    let table = rig.install();
    rig.enter(&table, trigger, pawn);
    rig.frames(10);
    assert_eq!(rig.pending(), 1);
    // The tick's Exit cancels before the countdown advances; the re-entry's
    // residual drains after it, in the same frame.
    rig.set_alarm_zero();
    rig.enter_after_exit(&table, trigger, pawn);
    assert_eq!(rig.number("pin.alarm"), 1.0, "the re-entry writes `alarm`");
    assert_eq!(rig.plays("x"), 2, "`x` once per entry");
    rig.frames(60);
    assert_eq!(rig.number("pin.s"), 1.0, "only the fresh tail writes `s`");
}

impl Rig {
    fn set_alarm_zero(&mut self) {
        self.ctx.system_commands.push(SystemReactionCommand::SetState {
            slot: "pin.alarm".into(),
            value: json!(0),
            dispatch_source: "test".into(),
            dispatch_values: Vec::new(),
        });
        self.drain();
    }

    /// One frame whose tick carries an Exit for `(trigger, pawn)` and a
    /// re-entry's Enter: cancels first, then the fresh residual, then landings.
    fn enter_after_exit(&mut self, table: &TriggerBindingTable, trigger: EntityId, pawn: EntityId) {
        {
            let scripting = &self.app.session.as_ref().unwrap().scripting;
            scripting.scheduler.begin_frame();
            scripting
                .scheduler
                .evaluate(&[(trigger, crate::trigger_system::PlayerId::Local(pawn))]);
        }
        self.enter(table, trigger, pawn);
        let scripting = &self.app.session.as_ref().unwrap().scripting;
        scripting.scheduler.drain_landings(
            &self.ctx.data_registry.borrow(),
            &scripting.sequence_registry,
            &scripting.reaction_registry,
            &scripting.system_registry,
            &self.ctx,
        );
        self.drain();
    }
}

// S27: a `fire` target's steps apply after every step of the firing body.
#[test]
fn s27_fire_target_steps_apply_after_the_firing_body() {
    let mut rig = Rig::new(|_| {
        vec![
            sequence(
                "outer",
                vec![
                    set("pin.a", json!(1)),
                    fire_step("inner"),
                    set("pin.b", read("pin.a")),
                ],
            ),
            sequence("inner", vec![set("pin.a", json!(2))]),
        ]
    });
    let _ = rig.install();
    rig.fire("outer");
    rig.drain();
    assert_eq!(rig.number("pin.b"), 1.0, "`b` reads 1: the firing body's steps first");
    assert_eq!(rig.number("pin.a"), 2.0, "then `inner`'s");
}
