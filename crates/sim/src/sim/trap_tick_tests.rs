// Trap request timing through the authoritative tick: which sources act on their tick, which latch.
// See: context/lib/scripting.md §12 (Entity addressing) · context/lib/entity_model.md §Components

use glam::Vec3;
use postretro_entities::TriggerCommand;
use postretro_entities::components::projectile_trap::ProjectileTrapComponent;

use super::member_tick_test_fixtures::*;

// A trigger-bound reaction's step before its first `wait` acts on
// the Enter tick, and a no-script `fire` on the same trap that tick dedupes.
#[test]
fn trigger_bound_trap_step_acts_on_the_enter_tick_and_dedupes_with_the_direct_fire() {
    let mut rig = Rig::new();
    let trap = rig.trap("a");
    rig.trigger("a", TriggerCommand::Fire, "plate");
    rig.bind(vec![reaction("plate", vec![step(trap, "fireTrap")])]);
    let pawn = rig.local(AWAY);
    rig.frames(2);
    rig.place(pawn, Vec3::ZERO);
    rig.frame();
    assert_eq!(
        rig.shot_ticks(trap),
        vec![2],
        "one execution on the Enter tick"
    );
}

// The direct command is issued first, then the bound step.
#[test]
fn direct_command_precedes_bound_trap_steps() {
    for (command, bound, running) in [
        (TriggerCommand::Start, "stopTrap", false),
        (TriggerCommand::Stop, "startTrap", true),
    ] {
        let mut rig = Rig::new();
        let trap = rig.trap("a");
        rig.trigger("a", command, "plate");
        rig.bind(vec![reaction("plate", vec![step(trap, bound)])]);
        let pawn = rig.local(AWAY);
        rig.frame();
        rig.place(pawn, Vec3::ZERO);
        rig.frame();
        assert_eq!(
            rig.running(trap),
            running,
            "{bound} after the direct command"
        );
        if !running {
            assert!(rig.shots.is_empty(), "start then stop fires nothing");
        }
    }
}

// A teleport-bound trap step acts on the teleport's tick.
#[test]
fn teleport_bound_trap_step_acts_on_the_teleport_tick() {
    let mut rig = Rig::new();
    let trap = rig.trap("a");
    let gate = Vec3::new(20.0, 0.0, 0.0);
    rig.teleporter(gate, "jump");
    rig.bind(vec![reaction("jump", vec![step(trap, "fireTrap")])]);
    let pawn = rig.local(AWAY);
    rig.frames(3);
    rig.place(pawn, gate);
    let events = rig.tick();
    assert_eq!(events.teleports.len(), 1, "the pawn teleported this tick");
    assert_eq!(rig.shot_ticks(trap), vec![3]);
}

// A post-`wait` step latches to the next authoritative tick, and
// landings from different ticks of one multi-tick frame all act on that one
// next tick. Both tails park at the Enter frame's drain.
#[test]
fn post_wait_steps_latch_to_the_next_tick_even_from_a_catch_up_frame() {
    let mut rig = Rig::new();
    let first = rig.trap("a");
    let second = rig.trap("b");
    let third = rig.trap("c");
    rig.trigger("plate", TriggerCommand::Start, "plate");
    rig.bind(vec![
        reaction(
            "plate",
            vec![step(first, "fireTrap"), wait(1.0), step(second, "fireTrap")],
        ),
        reaction(
            "plate",
            vec![wait(2.0 * DT * 1000.0 + 1.0), step(third, "fireTrap")],
        ),
    ]);
    let pawn = rig.local(AWAY);
    rig.frame();
    rig.place(pawn, Vec3::ZERO);
    rig.frame();
    assert_eq!(
        rig.shot_ticks(first),
        vec![1],
        "the pre-wait step on the Enter tick"
    );
    // One catch-up frame of ticks 2..=5: the waits land on ticks 2 and 4.
    rig.frame_of(4);
    assert!(
        rig.shot_ticks(second).is_empty() && rig.shot_ticks(third).is_empty(),
        "landings run at the frame end"
    );
    rig.frame();
    assert_eq!(rig.shot_ticks(second), vec![6], "acts on the next tick");
    assert_eq!(
        rig.shot_ticks(third),
        vec![6],
        "a landing from a later tick of that frame acts on the same next tick"
    );
}

// On a tick, latched requests apply before that tick's trigger-bound ones.
#[test]
fn latched_requests_apply_before_trigger_requests() {
    // A latched start, then a trigger stop on the next tick: nothing fires.
    let mut rig = Rig::new();
    let trap = rig.trap("a");
    rig.trigger("a", TriggerCommand::Stop, "");
    rig.bind(Vec::new());
    let pawn = rig.local(AWAY);
    rig.frame();
    rig.latch(trap, &start(trap, 100.0, 0.0));
    rig.place(pawn, Vec3::ZERO);
    rig.frames(20);
    assert!(rig.shots.is_empty() && !rig.running(trap));

    // A latched stop on a running trap, then a trigger start: re-anchored on
    // the Enter tick, which fires at phase 0.
    let mut rig = Rig::new();
    let trap = rig.trap("a");
    rig.trigger("a", TriggerCommand::Start, "");
    rig.bind(Vec::new());
    let pawn = rig.local(AWAY);
    rig.latch(trap, &start(trap, 1000.0, 0.0));
    rig.frames(10);
    assert_eq!(rig.shot_ticks(trap), vec![0]);
    rig.latch(trap, &step(trap, "stopTrap"));
    rig.place(pawn, Vec3::ZERO);
    rig.frame();
    assert_eq!(
        rig.shot_ticks(trap),
        vec![0, 10],
        "re-anchored on the Enter tick"
    );
}

// A frame of several ticks runs every tick's due check, and a mid-frame
// trigger-bound start anchors on its own tick: shot ticks match one tick per frame.
#[test]
fn catch_up_frame_matches_one_tick_per_frame() {
    let run = |ticks_per_frame: usize| {
        let mut rig = Rig::new();
        let repeating = rig.trap("a");
        let triggered = rig.trap("b");
        rig.trigger("b", TriggerCommand::Start, "");
        rig.bind(Vec::new());
        let pawn = rig.local(AWAY);
        rig.latch(repeating, &start(repeating, 1000.0 / 30.0 + 1.0, 0.0));
        let mut ticks = 0;
        while ticks < 60 {
            if ticks == 10 {
                rig.place(pawn, Vec3::ZERO);
            }
            rig.frame_of(ticks_per_frame);
            ticks += ticks_per_frame;
        }
        (rig.shot_ticks(repeating), rig.shot_ticks(triggered))
    };
    let single = run(1);
    assert_eq!(
        single.1.first(),
        Some(&10),
        "the start anchors on the Enter tick"
    );
    assert_eq!(run(5), single);
}

// Duplicate fire steps dedupe on a trap but not on a spawner, as do two
// activators entering a zero-re-arm `multiple` trigger on one tick.
#[test]
fn duplicate_fire_dedupes_on_traps_but_not_spawners() {
    let mut rig = Rig::new();
    let trap = rig.trap("closet");
    let spawner = rig.spawner("closet");
    rig.trigger("unused", TriggerCommand::Start, "plate");
    rig.bind(vec![reaction(
        "plate",
        vec![
            step(trap, "fireTrap"),
            step(trap, "fireTrap"),
            step(spawner, "spawnFromSpawner"),
            step(spawner, "spawnFromSpawner"),
        ],
    )]);
    let pawn = rig.local(AWAY);
    rig.frame();
    rig.place(pawn, Vec3::ZERO);
    rig.frame();
    assert_eq!(rig.shot_ticks(trap), vec![1]);
    assert_eq!(rig.spawned(), 2 * SPAWN_COUNT as usize);

    let mut rig = Rig::new();
    let trap = rig.trap("closet");
    rig.spawner("closet");
    rig.trigger("closet", TriggerCommand::Fire, "");
    rig.bind(Vec::new());
    let host = rig.local(AWAY);
    let client = rig.remote(AWAY, 7);
    rig.frame();
    rig.place(host, Vec3::ZERO);
    rig.place(client, Vec3::ZERO);
    rig.frame();
    assert_eq!(rig.shot_ticks(trap), vec![1], "the trap fires once");
    assert_eq!(
        rig.spawned(),
        2 * SPAWN_COUNT as usize,
        "the spawner spawns a batch per Enter"
    );
}

// Two players on a zero-re-arm sweep plate on one tick produce one sweep;
// a second player entering while the traps recover adds no shot.
#[test]
fn two_activators_on_a_zero_rearm_plate_fire_one_sweep() {
    let sweep = |rig: &mut Rig| {
        let first = rig.trap("a");
        let second = rig.trap("b");
        rig.trigger("plate", TriggerCommand::Start, "sweep");
        rig.bind(vec![reaction(
            "sweep",
            vec![
                step(first, "fireTrap"),
                wait(20.0),
                step(second, "fireTrap"),
            ],
        )]);
        (first, second)
    };
    let mut rig = Rig::new();
    let (first, second) = sweep(&mut rig);
    let host = rig.local(AWAY);
    let client = rig.remote(AWAY, 7);
    rig.frame();
    rig.place(host, Vec3::ZERO);
    rig.place(client, Vec3::ZERO);
    rig.frames(10);
    assert_eq!(rig.shot_ticks(first).len(), 1);
    assert_eq!(
        rig.shot_ticks(second).len(),
        1,
        "two parked tails, one execution"
    );

    let mut rig = Rig::new();
    let (first, second) = sweep(&mut rig);
    let host = rig.local(AWAY);
    let client = rig.remote(AWAY, 7);
    rig.frame();
    rig.place(host, Vec3::ZERO);
    rig.frames(2);
    rig.place(client, Vec3::ZERO);
    rig.frames(10);
    assert_eq!(
        rig.shot_ticks(first).len(),
        1,
        "the second Enter met recovery"
    );
    assert_eq!(rig.shot_ticks(second).len(), 1, "its tail met recovery too");
}

// A connected client issues no trap request from any member step.
#[test]
fn client_trap_steps_issue_and_latch_nothing() {
    let mut rig = Rig::new();
    let trap = rig.trap("a");
    rig.ctx.owner_slot_writes_enabled.set(false);
    for step in [step(trap, "fireTrap"), start(trap, 100.0, 0.0)] {
        rig.latch(trap, &step);
    }
    let pending = rig
        .ctx
        .registry
        .borrow()
        .get_component::<ProjectileTrapComponent>(trap)
        .unwrap()
        .pending;
    assert_eq!(pending, Default::default(), "nothing latched on a client");
}
