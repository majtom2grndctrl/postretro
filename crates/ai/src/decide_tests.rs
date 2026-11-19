// Decide ticks at production cadence: acquisition, guards and selectors run
// on an NPC's 15 Hz phase (and on its first run); the ticks between act on its
// last decide, and acquire nothing.
// See: context/plans/in-progress/E10--ai-tick-scheduling/index.md (P4, P5)
//      context/lib/entity_model.md §7c (decide ticks)

use super::*;
use crate::candidate_scope::REFRESHES;
use crate::decide::DECIDE_INTERVAL_TICKS;

/// A brain that toggles between two holds on every decide: each decide's
/// `acquisitionDue` is true, so a guard reading it passes only there.
fn toggling_graph() -> BehaviorGraphDescriptor {
    test_behavior_graph!({
        initial: "rest".to_string(),
        activities: BTreeMap::from([
            ("rest".to_string(), authored_state("idle", MotionVerb::Hold, None)),
            ("due".to_string(), authored_state("idle", MotionVerb::Hold, None)),
        ]),
        transitions: BTreeMap::from([
            (
                "rest".to_string(),
                vec![edge("due", brain_input(BRAIN_ACQUISITION_DUE_INPUT))],
            ),
            (
                "due".to_string(),
                vec![edge("rest", brain_input(BRAIN_ACQUISITION_DUE_INPUT))],
            ),
        ]),
        candidate_filter: None,
        retaliation: None,
        patrol: None,
        attacks: BTreeMap::new(),
        engagement_radius: None,
        move_speed: TEST_MOVE_SPEED,
    })
}

/// One AI tick, returning whether any candidate was refreshed in it: whether
/// target acquisition scanned.
fn tick_scanning(reg: &mut EntityRegistry, runtime: &mut AiRuntime) -> bool {
    REFRESHES.with(|log| *log.borrow_mut() = Some(Vec::new()));
    run_ai_tick(reg, runtime, 0.016);
    REFRESHES.with(|log| log.borrow_mut().take().is_some_and(|log| !log.is_empty()))
}

// P4 / P5: an idle NPC with a hostile in reach scans for targets and runs its
// guards on its first run and then on every fourth tick of its phase, and on
// no other tick, whatever tick it spawns on relative to its phase;
// `@brain.acquisitionDue` reads true on exactly those ticks.
#[test]
fn acquisition_and_guards_run_exactly_on_decide_ticks_for_every_spawn_offset() {
    let graph = toggling_graph();
    for phase in 0..DECIDE_INTERVAL_TICKS {
        for spawn_after in 0..DECIDE_INTERVAL_TICKS {
            let mut reg = EntityRegistry::new();
            let mut runtime = AiRuntime::new();
            for _ in 0..spawn_after {
                run_ai_tick(&mut reg, &mut runtime, 0.016);
            }
            spawn_player(&mut reg, Vec3::new(5.0, 0.0, 0.0));
            let mut brain = authored_brain(&graph, "rest");
            brain.schedule_identity = Some(identity_deciding_on(phase + 1));
            let enemy = spawn_enemy(&mut reg, Vec3::ZERO, brain, 50.0);

            let first = spawn_after + 1;
            let mut state = enemy_state_name(&reg, enemy);
            for tick in first..first + 4 * DECIDE_INTERVAL_TICKS {
                let decides = tick == first || (tick + 3 - phase) % DECIDE_INTERVAL_TICKS == 0;
                let scanned = tick_scanning(&mut reg, &mut runtime);
                let now = enemy_state_name(&reg, enemy);
                assert_eq!(
                    scanned, decides,
                    "phase {phase}, spawned after {spawn_after}: tick {tick} scan"
                );
                assert_eq!(
                    now != state,
                    decides,
                    "phase {phase}, spawned after {spawn_after}: tick {tick} guards"
                );
                state = now;
            }
        }
    }
}

// An idle NPC 40 m from a player in view, and an engaged one whose target is
// 60 m away, scan on every decide: no distance band thins them out.
#[test]
fn acquisition_runs_every_decide_at_any_distance() {
    let mut reg = EntityRegistry::new();
    let mut runtime = AiRuntime::new();
    let graph = test_graph_with(200.0, 200.0);
    spawn_player(&mut reg, Vec3::new(40.0, 0.0, 0.0));
    // A second, farther candidate keeps every later decide's scan observable
    // once the first is retained.
    spawn_player(&mut reg, Vec3::new(70.0, 0.0, 0.0));
    let mut idle = brain_with(graph.clone(), TEST_IDLE_STATE);
    idle.schedule_identity = Some(identity_deciding_on(1));
    let idle = spawn_enemy(&mut reg, Vec3::ZERO, idle, 50.0);

    let mut other = EntityRegistry::new();
    let mut other_runtime = AiRuntime::new();
    let far = spawn_player(&mut other, Vec3::new(60.0, 0.0, 0.0));
    spawn_player(&mut other, Vec3::new(90.0, 0.0, 0.0));
    let mut engaged = brain_with(graph, TEST_ALERT_STATE);
    engaged.acquired_target = Some(far);
    engaged.schedule_identity = Some(identity_deciding_on(1));
    let engaged = spawn_enemy(&mut other, Vec3::ZERO, engaged, 50.0);

    for tick in 1..=12_u64 {
        let decides = tick % DECIDE_INTERVAL_TICKS == 1;
        assert_eq!(
            tick_scanning(&mut reg, &mut runtime),
            decides,
            "idle, tick {tick}"
        );
        assert_eq!(
            tick_scanning(&mut other, &mut other_runtime),
            decides,
            "engaged, tick {tick}"
        );
    }
    assert_eq!(
        enemy_state_name(&reg, idle),
        TEST_ALERT_STATE,
        "the idle one acquired"
    );
    assert_eq!(enemy_acquired_target(&other, engaged), Some(far));
}

// A perceived idle NPC on a patrol route leaves each reached point on the
// tick it arrives, between decides too: route following acts every tick.
#[test]
fn a_patroller_leaves_each_point_on_its_arrival_tick_between_decides() {
    let mut registry = EntityRegistry::new();
    let mut runtime = AiRuntime::new();
    let graph = patrol_graph(
        vec![[0.0, 0.0], [1.0, 0.0], [2.0, 0.0]],
        PatrolMode::PingPong,
    );
    let mut brain = authored_brain(&graph, "position");
    brain.schedule_identity = Some(identity_deciding_on(1));
    let enemy = spawn_enemy(&mut registry, Vec3::ZERO, brain, 50.0);
    // Tick 1 decides; the arrivals below fall on ticks 2, 3, 4 and 6, between
    // decides, and on 5, a decide.
    run_ai_tick(&mut registry, &mut runtime, 0.016);
    for (tick, position, cursor, destination) in [
        (2, Vec3::new(1.0, 0.0, 0.0), 2, Vec3::new(2.0, 0.0, 0.0)),
        (3, Vec3::new(2.0, 0.0, 0.0), 1, Vec3::new(1.0, 0.0, 0.0)),
        (4, Vec3::new(1.0, 0.0, 0.0), 0, Vec3::ZERO),
        (5, Vec3::ZERO, 1, Vec3::new(1.0, 0.0, 0.0)),
        (6, Vec3::new(1.0, 0.0, 0.0), 2, Vec3::new(2.0, 0.0, 0.0)),
    ] {
        set_enemy_position(&mut registry, enemy, position);
        run_ai_tick(&mut registry, &mut runtime, 0.016);
        let brain = registry.get_component::<BrainComponent>(enemy).unwrap();
        assert_eq!(brain.patrol_cursor, cursor, "tick {tick}");
        assert_eq!(
            enemy_destination(&registry, enemy),
            Some(destination),
            "tick {tick}: it left the point it reached"
        );
    }
}
