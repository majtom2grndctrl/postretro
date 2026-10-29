// A connected client's `wait` tails: which park, which steps land, on which
// clock, and what a late joiner converges. The host clock is driven by hand,
// one frame per tick, so landings are pinned to the tick.
// See: context/lib/scripting.md §12 (Steps after a `wait`)

use std::cell::Cell;
use std::rc::Rc;

use postretro_entities::{
    EntityId, FireOrigin, FireSubject, MachineSource, NamedReaction, ReplicationScope, ScriptCtx,
    SequenceStep, SequenceTarget, SlotOwnership, SlotRecord, SlotSchema, SlotType, SlotValue,
    SystemReactionCommand,
};
use postretro_scripting_core::data_descriptors::ReactionDescriptor;
use postretro_scripting_core::reaction_dispatch::{
    NamedEventDispatchContext, dispatch_deferred_named_events_with_sequences,
    fire_named_event_with_sequences,
};
use postretro_scripting_core::reaction_registry::{
    ReactionPrimitiveRegistry, SystemReactionRegistry,
};
use postretro_scripting_core::sequence::SequencedPrimitiveRegistry;
use postretro_test_log_capture::LogCapture;
use serde_json::json;

use super::{ReactionScheduler, SchedulerRole, register_reaction_control_primitives};
use crate::scripting_systems::system_reactions::register_system_reaction_primitives;

const TICK: f64 = 0.016_667;

fn wait(duration_ms: f64) -> SequenceStep {
    SequenceStep {
        id: SequenceTarget::Wait,
        primitive: "wait".into(),
        args: json!({ "durationMs": duration_ms, "interruptible": false }),
    }
}
fn interruptible_wait(duration_ms: f64) -> SequenceStep {
    SequenceStep {
        id: SequenceTarget::Wait,
        primitive: "wait".into(),
        args: json!({ "durationMs": duration_ms, "interruptible": true }),
    }
}
fn system(primitive: &str, args: serde_json::Value) -> SequenceStep {
    SequenceStep {
        id: SequenceTarget::System,
        primitive: primitive.into(),
        args,
    }
}
fn sound(name: &str) -> SequenceStep {
    system("playSound", json!({ "sound": name }))
}
fn set(slot: &str, value: serde_json::Value) -> SequenceStep {
    system("setState", json!({ "slot": slot, "value": value }))
}
fn show(tree: &str) -> SequenceStep {
    system("showDialog", json!({ "tree": tree }))
}
fn close() -> SequenceStep {
    system("closeDialog", json!({}))
}
fn fire(event: &str) -> SequenceStep {
    SequenceStep {
        id: SequenceTarget::Fire,
        primitive: "fire".into(),
        args: json!({ "event": event }),
    }
}
/// A mover member step, which only the host lands.
fn mover_start() -> SequenceStep {
    SequenceStep {
        id: SequenceTarget::Entity(EntityId::from_raw(1)),
        primitive: "moverStart".into(),
        args: json!({}),
    }
}
fn body(name: &str, steps: Vec<SequenceStep>) -> NamedReaction {
    NamedReaction {
        name: name.into(),
        descriptor: ReactionDescriptor::Sequence(steps),
    }
}

fn number_slot(network: ReplicationScope) -> SlotRecord {
    SlotRecord::new(SlotSchema {
        slot_type: SlotType::Number,
        default: Some(SlotValue::Number(0.0)),
        range: None,
        persist: false,
        readonly: false,
        ownership: SlotOwnership::Mod,
        network,
        per_owner: false,
        accumulate: None,
    })
}

/// One machine's reaction machinery under a scheduler role.
struct Machine {
    ctx: ScriptCtx,
    scheduler: ReactionScheduler,
    sequence: SequencedPrimitiveRegistry,
    reaction: ReactionPrimitiveRegistry,
    system: SystemReactionRegistry,
    /// How many times a `moverStart` member step ran here.
    movers: Rc<Cell<usize>>,
    /// Commands queued by every fire and landing so far, in order.
    commands: Vec<SystemReactionCommand>,
}

impl Machine {
    fn new(role: SchedulerRole, reactions: Vec<NamedReaction>) -> Self {
        let ctx = ScriptCtx::new();
        {
            let mut slots = ctx.slot_table.borrow_mut();
            slots
                .insert(
                    "t.shared".into(),
                    number_slot(ReplicationScope::SharedGlobal),
                )
                .unwrap();
            slots
                .insert("t.local".into(), number_slot(ReplicationScope::None))
                .unwrap();
        }
        ctx.data_registry
            .borrow_mut()
            .populate_level(reactions, Vec::new(), &[]);
        let scheduler = ReactionScheduler::default();
        scheduler.set_role(role);
        let mut sequence = SequencedPrimitiveRegistry::new();
        register_reaction_control_primitives(&mut sequence, scheduler.clone());
        let movers = Rc::new(Cell::new(0));
        {
            let movers = movers.clone();
            sequence.register("moverStart", move |_, _| {
                movers.set(movers.get() + 1);
                Ok(())
            });
        }
        let mut system = SystemReactionRegistry::new();
        register_system_reaction_primitives(&mut system);
        Self {
            ctx,
            scheduler,
            sequence,
            reaction: ReactionPrimitiveRegistry::new(),
            system,
            movers,
            commands: Vec::new(),
        }
    }

    fn client(reactions: Vec<NamedReaction>) -> Self {
        Self::new(SchedulerRole::Client, reactions)
    }

    /// Fire `name` from a source of class `source`, as that source's drain
    /// does, follow-ups included.
    fn fire(&mut self, name: &str, origin: FireOrigin) {
        let data = self.ctx.data_registry.borrow();
        let chained = fire_named_event_with_sequences(
            name,
            &data,
            &self.sequence,
            &self.reaction,
            &self.system,
            &self.ctx,
            Some(NamedEventDispatchContext {
                origin,
                source: format!("named:{name}"),
                values: &[],
                emitter: None,
            }),
        );
        dispatch_deferred_named_events_with_sequences(
            chained,
            &data,
            &self.sequence,
            &self.reaction,
            &self.system,
            &self.ctx,
        );
        drop(data);
        self.collect();
    }

    fn level_load(&mut self) {
        self.fire(
            "levelLoad",
            FireOrigin::EveryMachine(MachineSource::HostTimeline),
        );
    }

    /// One frame of exactly one tick: the own clock advances, the host clock
    /// reads `host_time` (seconds), and the landings drain.
    fn frame(&mut self, host_time: Option<f64>) {
        self.scheduler.begin_frame();
        self.scheduler.evaluate(&[]);
        self.scheduler.advance_host_clock(host_time);
        self.scheduler.drain_landings(
            &self.ctx.data_registry.borrow(),
            &self.sequence,
            &self.reaction,
            &self.system,
            &self.ctx,
        );
        self.collect();
    }

    /// Frames for host ticks `from..to`, reading the host clock at the start
    /// of each tick, as a client a fixed latency behind would.
    fn host_ticks(&mut self, from: u64, to: u64) {
        for tick in from..to {
            self.frame(Some(tick as f64 * TICK + TICK / 2.0));
        }
    }

    /// Frames with no host clock (a slot that has not anchored, or an own-clock
    /// tail that does not read it).
    fn own_ticks(&mut self, count: usize) {
        for _ in 0..count {
            self.frame(None);
        }
    }

    fn collect(&mut self) {
        self.commands.extend(
            self.ctx
                .system_commands
                .take_ordered()
                .into_iter()
                .map(|(_, command)| command),
        );
    }

    fn plays(&self, name: &str) -> usize {
        self.commands
            .iter()
            .filter(|command| matches!(command, SystemReactionCommand::PlaySound { sound, .. } if sound == name))
            .count()
    }

    fn kinds(&self) -> Vec<String> {
        self.commands
            .iter()
            .map(|command| match command {
                SystemReactionCommand::PlaySound { sound, .. } => format!("play {sound}"),
                SystemReactionCommand::PushTree { tree, .. } => format!("show {tree}"),
                SystemReactionCommand::PopTree => "close".into(),
                SystemReactionCommand::SetState { slot, .. } => format!("set {slot}"),
                SystemReactionCommand::LoadLevel { .. } => "load".into(),
                SystemReactionCommand::ReturnToFrontend => "frontend".into(),
                other => format!("{other:?}"),
            })
            .collect()
    }
}

fn own_state() -> FireOrigin {
    FireOrigin::EveryMachine(MachineSource::OwnState)
}
fn host_mirrored() -> FireOrigin {
    FireOrigin::EveryMachine(MachineSource::HostMirrored)
}
fn warnings(capture: &LogCapture) -> Vec<String> {
    capture
        .records()
        .into_iter()
        .filter(|record| record.level <= log::Level::Warn)
        .map(|record| record.message)
        .collect()
}
fn tail_quiet(capture: &LogCapture) {
    assert!(
        capture
            .records()
            .iter()
            .all(|record| record.level > log::Level::Info),
        "{:?}",
        capture.records()
    );
}

// AC 10: a UI press plays its click, drops the host-only load after the wait
// and warns once, naming the reaction and step; a second press warns no
// further. Fired by a shared-state crossing it warns nothing above debug.
// `returnToFrontend` in its place lands on the client 300 ms later.
#[test]
fn ui_press_tail_drops_a_level_load_and_warns_once() {
    let press = || {
        body(
            "press",
            vec![
                sound("click"),
                wait(300.0),
                system("loadLevel", json!({ "map": "m" })),
            ],
        )
    };
    let mut client = Machine::client(vec![press()]);
    let capture = LogCapture::start();
    client.fire("press", own_state());
    assert_eq!(client.plays("click"), 1);
    client.own_ticks(20);
    assert_eq!(client.kinds(), ["play click"], "the load is the host's");
    assert_eq!(
        warnings(&capture),
        [
            "[Scheduler] reaction `press`: step `loadLevel` after a `wait` lands only on the host, and this own-state tail fires for this machine's player; it is dropped here"
        ]
    );
    client.fire("press", own_state());
    client.own_ticks(20);
    assert_eq!(
        warnings(&capture).len(),
        1,
        "a second press warns no further"
    );
    drop(capture);

    let mut client = Machine::client(vec![press()]);
    let capture = LogCapture::start();
    client.fire("press", host_mirrored());
    client.own_ticks(20);
    tail_quiet(&capture);
    drop(capture);

    let mut client = Machine::client(vec![body(
        "press",
        vec![
            sound("click"),
            wait(300.0),
            system("returnToFrontend", json!({})),
        ],
    )]);
    client.fire("press", own_state());
    client.own_ticks(17);
    assert_eq!(client.kinds(), ["play click"]);
    client.own_ticks(2);
    assert_eq!(
        client.kinds(),
        ["play click", "frontend"],
        "the client returns itself"
    );
}

// AC 11: a movement event's tail warns once naming the first dropped step,
// the mover, not the later replicated write; it re-arms at a level change.
#[test]
fn movement_event_tail_warns_once_per_level_naming_the_first_dropped_step() {
    let reactions = || {
        vec![body(
            "moved",
            vec![wait(300.0), mover_start(), set("t.shared", json!(1))],
        )]
    };
    let mut client = Machine::client(reactions());
    let capture = LogCapture::start();
    for _ in 0..3 {
        client.fire("moved", own_state());
        client.own_ticks(20);
    }
    let warned = warnings(&capture);
    assert_eq!(warned.len(), 1);
    assert!(
        warned[0].contains("reaction `moved`: step `moverStart`"),
        "{warned:?}"
    );
    assert_eq!(client.movers.get(), 0);
    assert!(
        client.kinds().is_empty(),
        "the replicated write is the host's"
    );
    client.scheduler.clear();
    client.fire("moved", own_state());
    client.own_ticks(20);
    assert_eq!(
        warnings(&capture).len(),
        2,
        "the first event after a level change warns again"
    );
}

// AC 12: a weapon event's tail on a client plays its sound and drops the
// replicated write, saying nothing above debug: the host lands the write for
// every shot it fires `activate` for.
#[test]
fn weapon_event_tail_plays_its_sound_and_drops_the_write_quietly() {
    let reactions = || {
        vec![body(
            "activate",
            vec![wait(300.0), sound("x"), set("t.shared", json!(1))],
        )]
    };
    let mut client = Machine::client(reactions());
    let capture = LogCapture::start();
    client.fire("activate", host_mirrored());
    client.own_ticks(20);
    assert_eq!(client.kinds(), ["play x"]);
    tail_quiet(&capture);

    let mut host = Machine::new(SchedulerRole::Host, reactions());
    host.fire("activate", host_mirrored());
    host.own_ticks(20);
    assert_eq!(host.kinds(), ["play x", "set t.shared"]);
}

// S17: landings due on one host tick run in enrollment order on a client, as
// on the host, whatever their summed milliseconds.
#[test]
fn s17_same_frame_client_landings_run_in_enrollment_order() {
    let mut client = Machine::client(vec![
        body("levelLoad", vec![wait(1010.0), close()]),
        body("levelLoad", vec![wait(1001.0), show("d")]),
    ]);
    client.level_load();
    client.host_ticks(1, 61);
    assert!(client.kinds().is_empty(), "both count 61 whole host ticks");
    client.host_ticks(61, 62);
    assert_eq!(
        client.kinds(),
        ["close", "show d"],
        "A, then B: `d` ends shown"
    );
}

// S25: a client parks a tail whose first segment holds nothing it lands, and
// plays the second segment's sound on the host's tick.
#[test]
fn s25_two_waits_park_through_a_segment_with_nothing_to_land() {
    let mut client = Machine::client(vec![body(
        "levelLoad",
        vec![wait(1001.0), mover_start(), wait(1001.0), sound("x")],
    )]);
    client.level_load();
    assert_eq!(client.scheduler.pending_len(), 1);
    client.host_ticks(1, 62);
    assert_eq!((client.movers.get(), client.kinds().len()), (0, 0));
    assert_eq!(
        client.scheduler.pending_len(),
        1,
        "the second segment parked"
    );
    client.host_ticks(62, 122);
    assert_eq!(client.plays("x"), 0);
    client.host_ticks(122, 123);
    assert_eq!(client.plays("x"), 1, "61 + 61 host ticks");
}

// S35: a re-park enrolls before the `fire` child it lands with, so on one
// frame the closing step runs before the child's dialog.
#[test]
fn s35_re_park_lands_before_the_fire_child_on_one_frame() {
    let mut client = Machine::client(vec![
        body(
            "levelLoad",
            vec![wait(500.0), fire("r"), wait(500.0), close()],
        ),
        body("r", vec![wait(500.0), show("d")]),
    ]);
    client.level_load();
    client.host_ticks(1, 60);
    assert!(client.kinds().is_empty());
    client.host_ticks(60, 61);
    assert_eq!(client.kinds(), ["close", "show d"], "`d` ends shown");
}

// S18: a host-timeline tail lands at its host tick for a slow loader and a
// reading one tick early; a first reading one tick late is a late joiner's,
// which skips the sound at debug. The decision waits for the first reading.
#[test]
fn s18_host_timeline_tail_lands_on_the_hosts_tick_or_skips_for_a_late_joiner() {
    let level = || vec![body("levelLoad", vec![wait(5000.0), sound("x")])];
    // Slow loader: first reading at host 2 s.
    let mut client = Machine::client(level());
    client.level_load();
    client.own_ticks(30);
    assert_eq!(
        client.scheduler.pending_len(),
        1,
        "no reading yet: nothing decided"
    );
    client.host_ticks(120, 300);
    assert_eq!(client.plays("x"), 0);
    client.host_ticks(300, 301);
    assert_eq!(client.plays("x"), 1);

    // First reading one tick before the landing.
    let mut client = Machine::client(level());
    client.level_load();
    client.host_ticks(299, 301);
    assert_eq!(client.plays("x"), 1);

    // First reading one tick after: a late joiner.
    let mut client = Machine::client(level());
    client.level_load();
    let capture = LogCapture::start();
    client.host_ticks(301, 302);
    assert_eq!(client.plays("x"), 0);
    assert_eq!(client.scheduler.pending_len(), 0);
    capture.assert_logged(
        log::Level::Debug,
        "client landing did not apply `playSound`",
    );
    tail_quiet(&capture);
}

// S19: the first reading is per level. After a teardown the next level's
// first reading decides afresh, so a reading from the previous level's
// clock cannot make the new level's tail late. (The transport never hands a
// previous level's snapshot to apply: `participation_epoch_rejects_stale_
// cross_boundary_traffic_and_accepts_current`.)
#[test]
fn s19_each_level_takes_its_own_first_host_reading() {
    let mut client = Machine::client(vec![body("levelLoad", vec![wait(5000.0), sound("x")])]);
    client.host_ticks(1800, 1801);
    client.scheduler.clear();
    client.level_load();
    client.host_ticks(120, 301);
    assert_eq!(client.plays("x"), 1, "at the new level's host 5 s");
}

// S26: a late joiner's first reading finds a segment due: it closes the
// dialog its own install opened and skips the sound at debug.
#[test]
fn s26_late_joiner_converges_state_and_skips_presentation() {
    let mut client = Machine::client(vec![body(
        "levelLoad",
        vec![show("d"), wait(3000.0), close(), sound("k")],
    )]);
    client.level_load();
    assert_eq!(client.kinds(), ["show d"], "the joiner's install opens `d`");
    client.host_ticks(600, 601);
    assert_eq!(client.kinds(), ["show d", "close"], "`k` never plays");
}

// S29: a joiner between two waits writes the due local slot at its reading,
// skips the due sound, and lands the later segment on schedule.
#[test]
fn s29_joiner_between_two_waits_converges_then_lands_on_schedule() {
    let mut client = Machine::client(vec![body(
        "levelLoad",
        vec![
            wait(300.0),
            set("t.local", json!(1)),
            sound("x"),
            wait(300.0),
            sound("y"),
        ],
    )]);
    client.level_load();
    client.host_ticks(27, 28);
    assert_eq!(client.kinds(), ["set t.local"]);
    client.host_ticks(28, 36);
    assert_eq!(client.plays("y"), 0);
    client.host_ticks(36, 37);
    assert_eq!(
        client.kinds(),
        ["set t.local", "play y"],
        "600 ms is 36 ticks"
    );
}

// S30: a `fire` from a client `levelLoad` tail counts on the host timeline
// from the due tick of the step that fired it.
#[test]
fn s30_fire_from_a_level_load_tail_counts_on_the_host_timeline() {
    let mut client = Machine::client(vec![
        body("levelLoad", vec![wait(5000.0), fire("r")]),
        body("r", vec![wait(5000.0), sound("x")]),
    ]);
    client.level_load();
    client.host_ticks(120, 600);
    assert_eq!(client.plays("x"), 0);
    client.host_ticks(600, 601);
    assert_eq!(client.plays("x"), 1);
}

// S32: a `levelLoad` fire of `r` and a crossing's fire of `r` park
// separately: one on the host timeline, one on the client's own clock.
#[test]
fn s32_two_clocks_park_two_instances() {
    let mut client = Machine::client(vec![
        body("levelLoad", vec![wait(1000.0), fire("r")]),
        body("r", vec![wait(2000.0), sound("x")]),
    ]);
    client.level_load();
    client.host_ticks(1, 120);
    client.fire("r", host_mirrored());
    assert_eq!(client.scheduler.pending_len(), 2);
    client.host_ticks(120, 181);
    assert_eq!(client.plays("x"), 1, "the host-timeline instance at 3 s");
    client.host_ticks(181, 241);
    assert_eq!(
        client.plays("x"),
        2,
        "the crossing's, 2 s after the crossing"
    );
}

// S34: a late joiner meets a due `fire`: at a reading of 10 s, `r` opens and
// closes `d` at that reading and skips `k`; at 3.5 s, `r` opens `d` at the
// reading and lands the rest at 4 s, counted from the firing step's due tick.
#[test]
fn s34_late_joiner_meets_a_due_fire() {
    let level = || {
        vec![
            body("levelLoad", vec![wait(3000.0), fire("r")]),
            body("r", vec![show("d"), wait(1000.0), close(), sound("k")]),
        ]
    };
    let mut client = Machine::client(level());
    client.level_load();
    client.host_ticks(600, 601);
    assert_eq!(client.kinds(), ["show d", "close"]);

    let mut client = Machine::client(level());
    client.level_load();
    client.host_ticks(210, 211);
    assert_eq!(client.kinds(), ["show d"]);
    client.host_ticks(211, 240);
    assert_eq!(client.kinds(), ["show d"]);
    client.host_ticks(240, 241);
    assert_eq!(client.kinds(), ["show d", "close", "play k"]);
}

// S39: a `levelLoad` body's pre-wait `fire` parks its target on the host
// timeline: a slow loader plays at 5 s, a joiner after 5 s never does.
#[test]
fn s39_pre_wait_fire_from_level_load_counts_on_the_host_timeline() {
    let level = || {
        vec![
            body("levelLoad", vec![fire("r")]),
            body("r", vec![wait(5000.0), sound("x")]),
        ]
    };
    let mut client = Machine::client(level());
    client.level_load();
    client.host_ticks(120, 300);
    assert_eq!(client.plays("x"), 0);
    client.host_ticks(300, 301);
    assert_eq!(client.plays("x"), 1);

    let mut client = Machine::client(level());
    client.level_load();
    client.host_ticks(330, 331);
    assert_eq!(client.plays("x"), 0);
    assert_eq!(client.scheduler.pending_len(), 0);
}

// S20 (scheduler half): a shared-state crossing's tail counts the client's
// own clock from its own crossing, the same-key rule applying to the edges
// this machine sees; an interruptible wait lands as non-interruptible there,
// silently.
#[test]
fn s20_crossing_tail_counts_the_clients_own_clock() {
    let mut client = Machine::client(vec![body("edge", vec![wait(500.0), sound("x")])]);
    let capture = LogCapture::start();
    client.fire("edge", host_mirrored());
    client.own_ticks(29);
    assert_eq!(client.plays("x"), 0);
    client.own_ticks(1);
    assert_eq!(client.plays("x"), 1, "500 ms after its own crossing");
    client.own_ticks(1);
    client.fire("edge", host_mirrored());
    client.own_ticks(30);
    assert_eq!(client.plays("x"), 2, "an edge outside the wait parks again");
    tail_quiet(&capture);
    drop(capture);

    let mut client = Machine::client(vec![body("r", vec![interruptible_wait(800.0), sound("x")])]);
    let capture = LogCapture::start();
    client.fire("r", host_mirrored());
    client.own_ticks(48);
    assert_eq!(client.plays("x"), 1);
    tail_quiet(&capture);
}

// AC 15: unloading, suspending or hot-reloading drops a parked client tail at
// debug, and nothing lands into the next level.
#[test]
fn client_tail_is_dropped_at_teardown_and_never_lands_later() {
    let mut client = Machine::client(vec![body("levelLoad", vec![wait(500.0), sound("x")])]);
    client.level_load();
    assert_eq!(client.scheduler.pending_len(), 1);
    let capture = LogCapture::start();
    client.scheduler.clear();
    assert_eq!(client.scheduler.pending_len(), 0);
    capture.assert_logged_once(log::Level::Debug, "dropping 1 client wait tail(s)");
    tail_quiet(&capture);
    client.host_ticks(1, 120);
    assert_eq!(client.plays("x"), 0);
}

// A host-only fire never parks a tail on a client (its source does not fire
// there), and the subject routing of a host landing does not apply.
#[test]
fn host_only_source_parks_nothing_on_a_client() {
    let mut client = Machine::client(vec![body("beat", vec![wait(300.0), sound("x")])]);
    client.fire("beat", FireOrigin::HostOnly(FireSubject::None));
    assert_eq!(client.scheduler.pending_len(), 0);
}
