// A repeating release (`repeat`, lowered to `passes`) and an attack's queue
// distance (`queueRange`) through the AI tick: one strike per pass under one
// turn and one cooldown, a flinch cancelling the passes left, a tell queuing
// out past its reach, and a timed strike out of reach whiffing.
// See: context/lib/entity_model.md §7c (Stances and attacks, Release exit)

use super::*;
use postretro_entities::components::mesh::ZeroPhaseStates;

/// The combo's state: an 800 ms loop with `hit` at 100 ms, played at 2.5x.
const JAB_QUICK: &str = "jab_quick";
const JAB_PASS: f32 = 0.8;
const JAB_HIT: f32 = 0.1;
const RATE: f32 = 2.5;
const PASSES: u32 = 3;
/// Longer than the whole combo, so a later pass would read it running.
const COOLDOWN_MS: f32 = 5000.0;

fn jab_loop() -> ClipSpec {
    looping(JAB_QUICK, JAB_PASS, &[("hit", JAB_HIT)])
}

fn combo() -> Strike {
    Strike {
        release_state: JAB_QUICK,
        recover_state: "idle",
        release_passes: Some(PASSES),
        attack: contact_attack(COOLDOWN_MS),
        ..Strike::default()
    }
}

/// An enemy seated in the combo's release, after `burned` other entities (so
/// each run draws a different phase), its mesh starting the combo's loop at
/// clip time 0 as the descriptor seam derives it from the graph.
fn combo_scene(burned: usize) -> (Scene, EntityId) {
    let mut scene = Scene::new(vec![jab_loop()]);
    let animation = scene.mesh.animation.take().expect("the rig animates");
    scene.mesh.animation = Some(animation.with_zero_phase_states(ZeroPhaseStates::of(
        postretro_foundation::repeating_release_states(&combo().graph()),
    )));
    for _ in 0..burned {
        scene.reg.spawn(Transform::default());
    }
    let enemy = scene.spawn(0, &combo().graph(), RELEASE);
    toughen(&mut scene, enemy);
    (scene, enemy)
}

/// [`loop_pass_ticks`] at a playback `rate`: the ticks, from the restart
/// tick (0), whose clip-time window holds a pass of the moment.
fn pass_ticks(phase: f32, rate: f32, ticks: usize) -> Vec<usize> {
    const SLACK: f64 = 1.0e-4;
    let (phase, event, duration) = (f64::from(phase), f64::from(JAB_HIT), f64::from(JAB_PASS));
    let step = f64::from(DT) * f64::from(rate);
    let passes = |time: f64| ((time + SLACK - event) / duration).floor();
    (0..ticks)
        .filter(|&k| {
            let (open, close) = if k == 0 {
                (phase - 2.0 * SLACK, phase)
            } else {
                (phase + step * (k - 1) as f64, phase + step * k as f64)
            };
            passes(close) > passes(open)
        })
        .collect()
}

/// The tick, from the release's entry tick (0), on which its clip has played
/// `passes` passes at `rate`: the release resolves there.
fn passes_played_tick(passes: u32, rate: f32) -> usize {
    let needed = f64::from(JAB_PASS) * f64::from(passes);
    let step = f64::from(DT) * f64::from(rate);
    (0..)
        .find(|&k| k as f64 * step + 1.0e-4 >= needed)
        .expect("a finite clip plays through")
}

/// Hold the combo's state at its fixed rate, as the pose stage does; any
/// other state (a reaction) plays at 1.
fn pose_rate(scene: &mut Scene, enemy: EntityId) {
    let rate = if enemy_animation(&scene.reg, enemy) == JAB_QUICK {
        RATE
    } else {
        1.0
    };
    scene.set_rate(enemy, rate);
}

fn place_player(scene: &mut Scene, x: f32) {
    let mut transform = *scene.reg.get_component::<Transform>(scene.pawn).unwrap();
    transform.position = Vec3::new(x, 0.0, 0.0);
    scene.reg.set_component(scene.pawn, transform).unwrap();
}

// A release with `repeat: 3` on a loop at 2.5x starts its loop at clip time 0
// whatever the enemy's drawn phase, so its first hit lands on the first pass;
// it strikes at its moment on each of three passes and leaves on the
// evaluation after its third pass has played. One entry, one clip never restarted, one turn
// held throughout from one grant, and the cooldown its first hit starts never
// stops a later pass; each hit restarts it, so it runs from the last.
#[test]
fn a_repeating_release_strikes_once_per_pass_under_one_turn_and_one_cooldown() {
    let resolves = passes_played_tick(PASSES, RATE);
    let expected: Vec<usize> = pass_ticks(0.0, RATE, resolves + 1);
    assert_eq!(expected.len(), PASSES as usize, "fixture: three passes");
    assert!(
        expected[0] < passes_played_tick(1, RATE),
        "fixture: from clip time 0 the first hit falls in the first pass"
    );
    let mut drawn_phases = BTreeSet::new();
    for offset in 0..4 {
        let (mut scene, enemy) = combo_scene(offset);
        let phase = instance_phase(enemy.to_raw(), JAB_PASS);
        drawn_phases.insert(phase.to_bits());

        let mut grants = Vec::new();
        let mut cooldowns = Vec::new();
        let log = run(&mut scene, enemy, resolves + 10, |scene, tick| {
            if tick > 0 {
                let brain = scene.brain(enemy);
                grants.push(
                    brain
                        .attack_turns
                        .held()
                        .map(|turn| turn.granted_tick)
                        .collect::<Vec<_>>(),
                );
                cooldowns.push(
                    brain
                        .attack_cooldown_remaining_ms
                        .get(SLAM)
                        .copied()
                        .unwrap_or(0.0),
                );
            }
            pose_rate(scene, enemy);
        });

        assert_eq!(hit_ticks(&log), expected, "phase {phase}: one hit per pass");
        assert_eq!(
            first_in(&log, RECOVER),
            Some(resolves + 1),
            "phase {phase}: leaves once the third pass has played"
        );
        assert!(
            log[..=resolves]
                .iter()
                .all(|tick| tick.state == RELEASE && tick.mesh == JAB_QUICK),
            "phase {phase}: one entry, its looping clip never redrawn"
        );
        let entries = log
            .iter()
            .filter(|tick| tick.entered.iter().any(|name| name == RELEASE))
            .count();
        assert_eq!(entries, 1, "phase {phase}: the release is entered once");

        // `grants[k]` and `cooldowns[k]` read the brain as tick `k` left it.
        let first = expected[0];
        let held = &grants[first..resolves];
        assert!(
            held.iter()
                .all(|turns| turns.len() == 1 && turns == &held[0]),
            "phase {phase}: one turn, from one grant, across every pass: {held:?}"
        );
        for &hit in &expected {
            assert_eq!(
                cooldowns[hit], COOLDOWN_MS,
                "phase {phase}: the hit on tick {hit} (re)starts the cooldown"
            );
        }
    }
    assert!(
        drawn_phases.len() >= 3,
        "fixture: the enemies draw different phases, which the release ignores"
    );
}

// A flinch after the first hit cancels the passes left through the release's
// flinch route: no second or third hit, never the recover, and the reaction
// plays through into the ending stance's rest.
#[test]
fn a_flinch_after_the_first_pass_cancels_the_passes_left() {
    let (mut scene, enemy) = combo_scene(0);
    let first = pass_ticks(0.0, RATE, 60)[0];
    let flinch = first + 2;
    let log = run(&mut scene, enemy, 120, |scene, tick| {
        if tick == flinch {
            scene.switch(enemy, "hit_reaction");
        }
        pose_rate(scene, enemy);
    });
    assert_eq!(hit_ticks(&log), vec![first], "hits 2 and 3 are cancelled");
    assert!(never_in(&log, RECOVER), "the recover is never entered");
    assert_eq!(
        first_in(&log, COIL),
        Some(flinch + ticks(0.4)),
        "the reaction plays out into the ending rest"
    );
}

// `queueRange` lets a tell wait in line and request its turn from out past
// the attack's reach; `maxRange` still bounds the hit. Out of reach at the
// moment the strike whiffs, and the release still leaves, starting its
// cooldown as a hit would; back in reach by the moment it lands. Without
// `queueRange` the same tell never queues.
#[test]
fn a_tell_queues_at_its_queue_range_but_strikes_only_within_its_reach() {
    const FAR: f32 = 4.0;
    const TICKS: usize = 120;
    let run_tell = |queue_range: Option<f32>, close_in_at: Option<usize>| {
        let mut strike = Strike {
            attack: contact_attack(COOLDOWN_MS),
            ..Strike::default()
        };
        strike.attack.queue_range = queue_range;
        let mut graph = strike.graph();
        graph.envelope.transitions.insert(
            OVERHEAD.to_string(),
            vec![GuardedRow {
                queue: Some(SLAM.to_string()),
                ..edge(HOLD, always())
            }],
        );
        let mut scene = Scene::new(vec![strike_clip()]);
        let enemy = scene.spawn(0, &graph, OVERHEAD);
        toughen(&mut scene, enemy);
        place_player(&mut scene, FAR);
        let mut entered_release = None;
        let log = run(&mut scene, enemy, TICKS, |scene, tick| {
            if entered_release.is_none() && scene.state(enemy) == RELEASE {
                entered_release = Some(tick - 1);
            }
            // `close_in_at` ticks after the release's entry tick.
            if close_in_at
                .zip(entered_release)
                .is_some_and(|(at, entry)| tick == entry + at)
            {
                place_player(scene, player_spot().x);
            }
        });
        (log, scene.brain(enemy))
    };

    let (unqueued, _) = run_tell(None, None);
    assert!(
        unqueued.iter().all(|tick| tick.state == OVERHEAD),
        "fixture: out of reach and without `queueRange`, the tell never queues"
    );

    let (whiffed, brain) = run_tell(Some(6.0), None);
    let release = first_in(&whiffed, RELEASE).expect("the tell queues from 4 m and strikes");
    assert!(first_in(&whiffed, HOLD).is_some_and(|hold| hold < release));
    assert!(
        hit_ticks(&whiffed).is_empty(),
        "out of reach at its moment, it whiffs"
    );
    let recover = release + LEAVES;
    assert_eq!(
        first_in(&whiffed, RECOVER),
        Some(recover),
        "the whiffed release still resolves and leaves"
    );
    let left = brain
        .attack_cooldown_remaining_ms
        .get(SLAM)
        .copied()
        .unwrap_or(0.0);
    let expected = COOLDOWN_MS - (TICKS - 1 - recover) as f32 * DT * 1000.0;
    assert!(
        (left - expected).abs() <= 0.5,
        "the whiff started the cooldown as it left the release: {left} ms left, {expected} expected"
    );

    let (landed, _) = run_tell(Some(6.0), Some(MOMENT - 5));
    let release = first_in(&landed, RELEASE).expect("the tell queues from 4 m");
    assert_eq!(
        hit_ticks(&landed),
        vec![release + MOMENT],
        "back within `maxRange` by its moment, the strike lands"
    );
}

// A timed release whose target is out of reach at its moment whiffs: the
// target coming back in reach on a later tick of the same entry draws no late
// hit, and the release resolves and leaves on the evaluation after its moment
// as a hit would.
#[test]
fn a_timed_release_out_of_reach_at_its_moment_whiffs_and_still_resolves() {
    let strike = Strike::default();
    let (mut scene, enemy) = strike_scene(vec![strike_clip()], &strike, RELEASE);
    place_player(&mut scene, 4.0);
    let log = run(&mut scene, enemy, 90, |scene, tick| {
        if tick == MOMENT + 1 {
            place_player(scene, player_spot().x);
        }
    });
    assert!(
        hit_ticks(&log).is_empty(),
        "a whiff, and no late hit: {log:?}"
    );
    assert_eq!(first_in(&log, RECOVER), Some(LEAVES));
}

// One turn covers a repeating release. A target switch after the first hit
// ends that turn, and the release requests no other: the passes left whiff
// against the new target, in reach though it is, and no second turn is
// granted while the release lasts.
#[test]
fn a_target_switch_after_the_first_hit_whiffs_the_passes_left_under_no_new_turn() {
    let (mut scene, enemy) = combo_scene(0);
    let original = scene.pawn;
    let other = spawn_player(&mut scene.reg, player_spot() + Vec3::new(0.0, 0.0, 0.3));
    let resolves = passes_played_tick(PASSES, RATE);
    let first = pass_ticks(0.0, RATE, resolves + 1)[0];
    let mut other_hits = Vec::new();
    let mut grants = BTreeSet::new();
    for tick in 0..=resolves + 1 {
        if tick == first + 1 {
            assert!(
                player_hp(&scene.reg, original) < 100.0,
                "fixture: the first jab lands on the first target"
            );
            scene.reg.despawn(original).unwrap();
        }
        pose_rate(&mut scene, enemy);
        let hp = player_hp(&scene.reg, other);
        scene.tick();
        if player_hp(&scene.reg, other) < hp {
            other_hits.push(tick);
        }
        if scene.state(enemy) == RELEASE {
            grants.extend(
                scene
                    .brain(enemy)
                    .attack_turns
                    .held()
                    .map(|turn| turn.granted_tick),
            );
        }
    }
    assert_eq!(
        enemy_acquired_target(&scene.reg, enemy),
        Some(other),
        "fixture: the enemy switched to the second target"
    );
    assert!(
        other_hits.is_empty(),
        "the passes left whiff: {other_hits:?}"
    );
    assert_eq!(
        grants.len(),
        1,
        "one grant across the release, none after the switch: {grants:?}"
    );
}

/// An enemy seated in a three-pass release of `action` on `spec`, ticked
/// once so its clips bind and are checked.
fn bind_repeating_release(spec: ClipSpec, action: ActionVerb) {
    let strike = Strike { action, ..combo() };
    let mut scene = Scene::new(vec![spec]);
    let enemy = scene.spawn(0, &strike.graph(), RELEASE);
    toughen(&mut scene, enemy);
    scene.tick();
    scene.tick();
}

// A repeating release whose moment its clip cannot time strikes once, not
// once per pass, whether the moment is named or the defaulted `hit`: it warns
// once per entity class and attack when its clips bind.
#[test]
fn a_repeating_release_its_clip_cannot_time_warns_that_it_strikes_once() {
    let untimed =
        "`brute` attack `slam`: its release repeats 3 passes on state `jab_quick`, but clip";
    let capture = LogCapture::start();
    for action in [timed(SLAM, "hit"), defaulted_hit()] {
        bind_repeating_release(looping(JAB_QUICK, JAB_PASS, &[]), action);
        capture.assert_logged_once(Level::Warn, untimed);
        capture.assert_logged_once(Level::Warn, "strikes once, not once per pass");
        capture.clear();
    }
    bind_repeating_release(jab_loop(), timed(SLAM, "hit"));
    capture.assert_not_logged(Level::Warn, untimed);
}

// A repeating release whose pass (its clip over its `playbackRate`) is
// shorter than a tick can reach only one moment a tick: it warns once.
#[test]
fn a_repeating_release_with_a_pass_shorter_than_a_tick_warns() {
    let short = "shorter than one 16.7 ms tick";
    let capture = LogCapture::start();
    bind_repeating_release(
        looping(JAB_QUICK, 0.01, &[("hit", 0.005)]),
        timed(SLAM, "hit"),
    );
    capture.assert_logged_once(Level::Warn, short);
    capture.assert_logged_once(
        Level::Warn,
        "`brute` attack `slam`: its release repeats 3 passes",
    );
    capture.clear();
    bind_repeating_release(jab_loop(), timed(SLAM, "hit"));
    capture.assert_not_logged(Level::Warn, short);
}
