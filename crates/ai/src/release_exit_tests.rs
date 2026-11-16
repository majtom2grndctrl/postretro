// A stance release's exit through the AI tick: the moment gate on its own rows,
// one pass of its clip summed across variant restarts, untimed and defaulted
// moments, the impact flinch that staggers it into its ending stance, and the
// default recover. Graphs are hand-written in the lowered shape (stance
// lowering builds them later), on clip rigs at 60 Hz.
// See: context/lib/entity_model.md §7c (Stances and attacks, Release exit)

use super::*;
use postretro_entities::components::brain::activity_at_path;
use postretro_foundation::stance_defaults::DEFAULT_RECOVERY_MS;
use postretro_foundation::{BRAIN_ANIMATION_DONE_INPUT, BRAIN_DAMAGE_IN_ACTIVITY_INPUT};

const SLAM: &str = "slam";
/// The beginning stance's rest and the strike's hold, where P46 starts.
const OVERHEAD: &str = "overhead";
const HOLD: &str = "slam_overhead";
const RELEASE: &str = "slam_release";
const RECOVER: &str = "slam_recover";
/// The ending stance's rest: `staggerTo` on the release and recover.
const COIL: &str = "coil";
const COIL_SOUND: &str = "brute/stance_coil";
const FLINCH: &str = "flinch";
/// The strike's state: a one-second one-shot whose `hit` is at 0.5 s, so its
/// moment's tick is 30 and the release's rows open on 31.
const STRIKE: &str = "smash_strike";
/// A reaction clip longer than the default recovery cap.
const LONG_REACTION: &str = "hit_long";

fn strike_clip() -> ClipSpec {
    one_shot(STRIKE, 1.0, &[("hit", 0.5)])
}

/// The reaction clips an impact effect switches to: the rig's 0.4 s
/// `hit_reaction`, and a 0.9 s one past the cap.
fn reaction_clips() -> Vec<ClipSpec> {
    vec![one_shot(LONG_REACTION, 0.9, &[])]
}

fn always() -> IrNode {
    IrNode::Const {
        value: IrValue::Bool(true),
    }
}

fn or(a: IrNode, b: IrNode) -> IrNode {
    IrNode::Or {
        a: Box::new(a),
        b: Box::new(b),
    }
}

fn damage_at_least(points: f32) -> IrNode {
    IrNode::Ge {
        a: Box::new(brain_input(BRAIN_DAMAGE_IN_ACTIVITY_INPUT)),
        b: Box::new(IrNode::Const {
            value: IrValue::Number(points),
        }),
    }
}

/// `atDefault`: the `hit` lowering fills in for a release that names no `at`.
fn defaulted_hit() -> ActionVerb {
    ActionVerb::Attack {
        name: SLAM.to_string(),
        at: Some("hit".to_string()),
        at_default: true,
    }
}

/// A release's own row: it waits on the moment, and on `ms` when one is set,
/// as lowering writes it.
fn after_moment(to: &str, ms: Option<f32>) -> GuardedRow {
    GuardedRow {
        after_moment: true,
        ..edge(to, ms.map_or_else(always, after_ms))
    }
}

/// The default recover's way out: its clip plays through, or the default
/// recovery duration passes.
fn default_recover_rows() -> Vec<GuardedRow> {
    vec![edge(
        COIL,
        or(
            brain_input(BRAIN_ANIMATION_DONE_INPUT),
            after_ms(DEFAULT_RECOVERY_MS),
        ),
    )]
}

/// One strike from `overhead` landing in `coil`, lowered: hold → release →
/// recover → `coil`, the release and recover routing a flinch to `coil`.
struct Strike {
    action: ActionVerb,
    /// The release's authored `ms`.
    release_ms: Option<f32>,
    release_motion: MotionVerb,
    release_state: &'static str,
    release_facing: Option<FacingPolicy>,
    release_speed: Option<f32>,
    recover_state: &'static str,
    recover_action: bool,
    recover_rows: Vec<GuardedRow>,
    /// The hold's fixed release time.
    hold_ms: f32,
    wildcard: Vec<GuardedRow>,
    attack: AttackParams,
}

impl Default for Strike {
    fn default() -> Self {
        Self {
            action: timed(SLAM, "hit"),
            release_ms: None,
            release_motion: MotionVerb::Hold,
            release_state: STRIKE,
            release_facing: None,
            release_speed: None,
            recover_state: STRIKE,
            recover_action: false,
            recover_rows: default_recover_rows(),
            hold_ms: 95.0,
            wildcard: Vec::new(),
            attack: contact_attack(10.0),
        }
    }
}

impl Strike {
    fn graph(&self) -> BehaviorGraphDescriptor {
        let posed = |state: &str, pose: &str| {
            let mut activity = authored_state(state, MotionVerb::Hold, None);
            activity.pose = Some(pose.to_string());
            activity
        };
        let mut hold = posed("idle", OVERHEAD);
        hold.attack_turn = Some(SLAM.to_string());
        let mut release = authored_state(
            self.release_state,
            self.release_motion,
            Some(self.action.clone()),
        );
        release.attack_turn = Some(SLAM.to_string());
        release.stagger_to = Some(COIL.to_string());
        release.facing = self.release_facing;
        release.speed = self.release_speed;
        let mut recover = authored_state(
            self.recover_state,
            MotionVerb::Hold,
            self.recover_action.then(|| untimed(SLAM)),
        );
        recover.attack_turn = Some(SLAM.to_string());
        recover.stagger_to = Some(COIL.to_string());
        let mut coil = posed("idle", COIL);
        coil.sound = Some(COIL_SOUND.to_string());
        let mut transitions = vec![
            (
                HOLD,
                vec![GuardedRow {
                    release_after: Some([self.hold_ms, self.hold_ms]),
                    ..edge(RELEASE, always())
                }],
            ),
            (RELEASE, vec![after_moment(RECOVER, self.release_ms)]),
            (RECOVER, self.recover_rows.clone()),
            (FLINCH, vec![edge(COIL, after_ms(450.0))]),
        ];
        if !self.wildcard.is_empty() {
            transitions.push(("*", self.wildcard.clone()));
        }
        clip_graph(
            RELEASE,
            vec![
                (OVERHEAD, posed("idle", OVERHEAD)),
                (HOLD, hold),
                (RELEASE, release),
                (RECOVER, recover),
                (COIL, coil),
                (FLINCH, authored_state("idle", MotionVerb::Hold, None)),
            ],
            transitions,
            vec![(SLAM, self.attack.clone())],
        )
    }
}

/// What one tick left: the active state, the mesh's drawn state, whether a
/// contact attack hit the player, and the leaves the enemy announced entering.
#[derive(Debug)]
struct Tick {
    state: String,
    mesh: String,
    hit: bool,
    entered: Vec<String>,
}

/// Run `ticks` ticks, calling `before` ahead of each with its index.
fn run(
    scene: &mut Scene,
    enemy: EntityId,
    ticks: usize,
    mut before: impl FnMut(&mut Scene, usize),
) -> Vec<Tick> {
    (0..ticks)
        .map(|tick| {
            before(scene, tick);
            let hp = player_hp(&scene.reg, scene.pawn);
            let result = scene.tick();
            Tick {
                state: scene.state(enemy),
                mesh: enemy_animation(&scene.reg, enemy),
                hit: player_hp(&scene.reg, scene.pawn) < hp,
                entered: entered(&result, enemy),
            }
        })
        .collect()
}

fn entered(result: &AiTickResult, enemy: EntityId) -> Vec<String> {
    result
        .events
        .iter()
        .filter(|emission| {
            matches!(emission.emitter, postretro_entities::Emitter::Entity { id, .. } if id == enemy)
        })
        .filter_map(|emission| match &emission.cue {
            postretro_entities::AiCue::Entered { graph, path } => {
                activity_at_path(graph, path).map(|(name, _)| name.to_string())
            }
            _ => None,
        })
        .collect()
}

fn hit_ticks(log: &[Tick]) -> Vec<usize> {
    log.iter()
        .enumerate()
        .filter(|(_, tick)| tick.hit)
        .map(|(index, _)| index)
        .collect()
}

/// The first tick that ends in `state`.
fn first_in(log: &[Tick], state: &str) -> Option<usize> {
    log.iter().position(|tick| tick.state == state)
}

fn never_in(log: &[Tick], state: &str) -> bool {
    first_in(log, state).is_none()
}

/// One enemy on `specs` (plus the reaction clips) seated in `start`.
fn strike_scene(specs: Vec<ClipSpec>, strike: &Strike, start: &str) -> (Scene, EntityId) {
    let mut all = reaction_clips();
    all.extend(specs);
    let mut scene = Scene::new(all);
    let enemy = scene.spawn(0, &strike.graph(), start);
    toughen(&mut scene, enemy);
    (scene, enemy)
}

/// Enough health that no test's damage kills the enemy.
fn toughen(scene: &mut Scene, enemy: EntityId) {
    let mut health = scene
        .reg
        .get_component::<HealthComponent>(enemy)
        .unwrap()
        .clone();
    health.max = 1000.0;
    health.current = 1000.0;
    scene.reg.set_component(enemy, health).unwrap();
}

/// The tick, counted from an entry on tick 0, on which an activity's own
/// timer first reads at or past `ms`, summed as the AI tick sums it.
fn timer_tick(ms: f32) -> usize {
    let dt_ms = DT * 1000.0;
    let mut elapsed = 0.0_f32;
    let mut tick = 0;
    while elapsed < ms {
        elapsed += dt_ms;
        tick += 1;
    }
    tick
}

/// The ticks a flinch lasts at most: the default recovery duration in clip
/// time, counted from the first switch.
fn stagger_cap() -> usize {
    ticks(DEFAULT_RECOVERY_MS / 1000.0)
}

/// The release's moment tick and the tick its rows open.
const MOMENT: usize = 30;
const LEAVES: usize = 31;

/// Seconds to ticks at 60 Hz, for a duration on a tick boundary.
fn ticks(seconds: f32) -> usize {
    (seconds * 60.0).round() as usize
}

// S14: a release at `hit` fires when its clip reaches the event, not on entry,
// and its lowered action and mark agree and load.
#[test]
fn a_release_at_hit_fires_at_its_moment_not_on_entry_and_its_lowered_shape_loads() {
    let strike = Strike::default();
    let json = serde_json::to_value(strike.graph()).expect("the lowered graph serializes");
    serde_json::from_value::<BehaviorGraphDescriptor>(json)
        .expect("the lowered graph parses")
        .validate()
        .expect("the release's `action.at` and its `attackTurn` agree, and load");

    let (mut scene, enemy) = strike_scene(vec![strike_clip()], &strike, RELEASE);
    let log = run(&mut scene, enemy, 45, |_, _| {});
    assert_eq!(hit_ticks(&log), vec![MOMENT], "at `hit`, never on entry");
}

// S15 / P42: with no `ms` the release leaves on the evaluation after its
// moment; an `ms` at or short of the moment still waits for it; a longer one
// leaves once it passes. The release's own rows never pass on or before the
// moment's tick.
#[test]
fn a_releases_ms_only_holds_it_past_its_moment() {
    for (ms, leaves) in [
        (None, LEAVES),
        (Some(100.0), LEAVES),
        (Some(500.0), LEAVES),
        (Some(790.0), timer_tick(790.0)),
    ] {
        let strike = Strike {
            release_ms: ms,
            ..Strike::default()
        };
        let (mut scene, enemy) = strike_scene(vec![strike_clip()], &strike, RELEASE);
        let log = run(&mut scene, enemy, 60, |_, _| {});
        assert_eq!(hit_ticks(&log), vec![MOMENT], "{ms:?}: fires at its moment");
        assert_eq!(
            first_in(&log, RECOVER),
            Some(leaves),
            "{ms:?}: leaves on the first evaluation after the moment and `ms`"
        );
    }
}

// S15 / P43: a release on a looping state fires at the next pass of its event
// from its drawn phase and leaves on the next evaluation, at every phase.
#[test]
fn a_looping_release_fires_at_its_next_pass_and_then_leaves_at_every_phase() {
    let strike = Strike {
        release_state: "strike_loop",
        recover_state: "strike_loop",
        ..Strike::default()
    };
    let mut fire_ticks = BTreeSet::new();
    for offset in 0..8 {
        let mut scene = Scene::new(vec![looping("strike_loop", 1.0, &[("hit", 0.25)])]);
        for _ in 0..offset {
            scene.reg.spawn(Transform::default());
        }
        let enemy = scene.spawn(0, &strike.graph(), RELEASE);
        let phase = instance_phase(enemy.to_raw(), 1.0);
        let expected = loop_pass_ticks(phase, 0.25, 1.0, 90)[0];
        let log = run(&mut scene, enemy, 90, |_, _| {});
        assert_eq!(hit_ticks(&log), vec![expected], "phase {phase}");
        assert_eq!(
            first_in(&log, RECOVER),
            Some(expected + 1),
            "phase {phase}: leaves on the next evaluation, never first"
        );
        fire_ticks.insert(expected);
    }
    assert!(
        fire_ticks.len() >= 3,
        "fixture: the phases spread the moment across the loop: {fire_ticks:?}"
    );
}

// S15 / P47: a `"*"` row passing on the moment's tick wins: the release is
// left and nothing fires.
#[test]
fn a_wildcard_row_on_the_moments_tick_wins_and_nothing_fires() {
    let strike = Strike {
        wildcard: vec![edge(COIL, after_ms(495.0))],
        ..Strike::default()
    };
    assert_eq!(
        timer_tick(495.0),
        MOMENT,
        "fixture: the row passes on the moment's tick"
    );
    let (mut scene, enemy) = strike_scene(vec![strike_clip()], &strike, RELEASE);
    let log = run(&mut scene, enemy, 45, |_, _| {});
    assert_eq!(log[MOMENT - 1].state, RELEASE);
    assert_eq!(
        log[MOMENT].state, COIL,
        "the `*` row wins on the moment's tick"
    );
    assert!(hit_ticks(&log).is_empty(), "nothing fires");
}

// S16 / P53: with no authored `at` (lowering's defaulted `hit`), a clip
// carrying `hit` fires there and lasts until it; a clip lacking it fires on
// entry, binds silently and lasts its clip; `at: false` fires on entry though
// the clip carries `hit`, and lasts its clip.
#[test]
fn a_defaulted_hit_fires_at_the_clips_hit_or_on_entry_and_binds_silently() {
    let capture = LogCapture::start();
    let lasts_the_clip = ticks(1.0) + 1;
    for (label, clip, action, fires, leaves) in [
        (
            "(a) clip carries `hit`",
            strike_clip(),
            defaulted_hit(),
            MOMENT,
            LEAVES,
        ),
        (
            "(b) clip lacks `hit`",
            one_shot(STRIKE, 1.0, &[]),
            defaulted_hit(),
            0,
            lasts_the_clip,
        ),
        (
            "(c) `at: false`",
            strike_clip(),
            untimed(SLAM),
            0,
            lasts_the_clip,
        ),
    ] {
        let strike = Strike {
            action,
            ..Strike::default()
        };
        let (mut scene, enemy) = strike_scene(vec![clip], &strike, RELEASE);
        let log = run(&mut scene, enemy, 75, |_, _| {});
        assert_eq!(hit_ticks(&log), vec![fires], "{label}: fires once, there");
        assert_eq!(first_in(&log, RECOVER), Some(leaves), "{label}: leaves");
    }
    capture.assert_not_logged(Level::Warn, "has no such event");
    capture.assert_not_logged(Level::Warn, "did not resolve");
}

// S17 / P51: a named `at` whose clip lacks the event, or did not resolve,
// warns once when clips bind and resolves on the entry tick, fired if its
// gates passed then; the release leaves on the next evaluation, or once `ms`
// passes.
#[test]
fn an_untimed_named_moment_resolves_on_the_entry_tick_and_warns() {
    let capture = LogCapture::start();
    for (label, clip, problem) in [
        (
            "(a) lacks `hit`",
            one_shot(STRIKE, 1.0, &[]),
            "has no such event",
        ),
        ("(b) unresolved", unresolved(STRIKE), "did not resolve"),
    ] {
        for (ms, leaves) in [(None, 1), (Some(290.0), timer_tick(290.0))] {
            let strike = Strike {
                release_ms: ms,
                ..Strike::default()
            };
            let (mut scene, enemy) = strike_scene(vec![clip_copy(&clip)], &strike, RELEASE);
            let log = run(&mut scene, enemy, 45, |_, _| {});
            assert_eq!(hit_ticks(&log), vec![0], "{label} {ms:?}: fires on entry");
            assert_eq!(first_in(&log, RECOVER), Some(leaves), "{label} {ms:?}");
            capture.assert_logged_once(
                Level::Warn,
                &format!("`brute` animation state `{STRIKE}`: clip `{STRIKE}_clip` {problem}"),
            );
            capture.clear();
        }
    }
}

fn clip_copy(spec: &ClipSpec) -> ClipSpec {
    ClipSpec {
        state: spec.state,
        clip: spec.clip,
        duration: spec.duration,
        looping: spec.looping,
        events: spec.events.clone(),
        resolved: spec.resolved,
    }
}

/// Out of reach on tick 0 only: the attack's gates fail on the entry tick and
/// pass from tick 1.
fn reach_from_tick_one(scene: &mut Scene, tick: usize) {
    let pawn = scene.pawn;
    match tick {
        0 => set_enemy_position(&mut scene.reg, pawn, Vec3::new(5.0, 0.0, 0.0)),
        1 => set_enemy_position(&mut scene.reg, pawn, player_spot()),
        _ => {}
    }
}

// S17 / P57(a): a named moment its clip cannot time whiffs when its gates fail
// on the entry tick, and the release leaves on the next evaluation.
// S19 / P57(b, c): `at: false`, or a defaulted `hit` its clip lacks, fires on
// the first tick its gates pass while the release lasts its clip.
#[test]
fn gates_late_on_an_untimed_release_whiff_a_named_moment_but_fire_a_free_one() {
    let lacks_hit = || one_shot(STRIKE, 1.0, &[]);
    let lasts_the_clip = ticks(1.0) + 1;
    for (label, action, hits, leaves) in [
        (
            "(a) named `hit` the clip lacks",
            timed(SLAM, "hit"),
            vec![],
            1,
        ),
        ("(b) `at: false`", untimed(SLAM), vec![1], lasts_the_clip),
        (
            "(c) defaulted `hit` the clip lacks",
            defaulted_hit(),
            vec![1],
            lasts_the_clip,
        ),
    ] {
        let strike = Strike {
            action,
            ..Strike::default()
        };
        let (mut scene, enemy) = strike_scene(vec![lacks_hit()], &strike, RELEASE);
        let log = run(&mut scene, enemy, 75, reach_from_tick_one);
        assert_eq!(hit_ticks(&log), hits, "{label}");
        assert_eq!(first_in(&log, RECOVER), Some(leaves), "{label}");
    }
}

/// A target-facing moving release on `base`, whose travel variants play their
/// own clips.
fn moving_strike(action: ActionVerb) -> Strike {
    Strike {
        action,
        release_motion: MotionVerb::ChaseTarget,
        release_state: "strike_loop",
        release_facing: Some(FacingPolicy::Target),
        recover_state: "idle",
        ..Strike::default()
    }
}

/// Every travel variant of a one-second loop carrying `hit` at 0.8 s.
fn variant_loops() -> Vec<ClipSpec> {
    ["strike_loop", "strike_loop_right", "strike_loop_left"]
        .into_iter()
        .map(|state| looping(state, 1.0, &[("hit", 0.8)]))
        .collect()
}

/// A scene whose enemy's drawn phase never meets `hit` at 0.8 within half a
/// loop of a restart, so a variant flipping every half second never reaches
/// it.
fn flipping_scene(strike: &Strike) -> (Scene, EntityId) {
    for offset in 0..64 {
        let mut scene = Scene::new(variant_loops());
        for _ in 0..offset {
            scene.reg.spawn(Transform::default());
        }
        let enemy = scene.spawn(0, &strike.graph(), RELEASE);
        let phase = instance_phase(enemy.to_raw(), 1.0);
        if !(0.25..0.85).contains(&phase) {
            return (scene, enemy);
        }
    }
    panic!("fixture: no spawn offset draws a phase clear of the moment");
}

/// Travel along +Z or -Z while facing the player along +X: the right- or
/// left-hand variant, flipping every half second from tick 0.
fn flip_every_half_second(scene: &mut Scene, enemy: EntityId, tick: usize) {
    if tick.is_multiple_of(30) {
        let direction = if (tick / 30).is_multiple_of(2) {
            1.0
        } else {
            -1.0
        };
        scene.set_agent(enemy, |agent| {
            agent.velocity = Vec3::new(0.0, 0.0, 2.0 * direction)
        });
    }
}

// S17 / P48 and S18 / P54(b): a moving release whose variant restarts its clip
// before each pass of its event whiffs once one pass of clip time, summed
// across the restarts, has passed without the moment, and leaves.
#[test]
fn a_moment_its_flipping_variant_never_reaches_whiffs_after_one_summed_pass() {
    let (mut scene, enemy) = flipping_scene(&moving_strike(timed(SLAM, "hit")));
    let log = run(&mut scene, enemy, 90, |scene, tick| {
        flip_every_half_second(scene, enemy, tick)
    });
    assert!(
        log[29].mesh.starts_with("strike_loop_") && log[45].mesh != log[29].mesh,
        "fixture: the variant flips: {} then {}",
        log[29].mesh,
        log[45].mesh
    );
    assert!(hit_ticks(&log).is_empty(), "the moment never comes");
    assert_eq!(
        first_in(&log, RECOVER),
        Some(ticks(1.0) + 1),
        "whiffed after one second of summed clip time, left on the next evaluation"
    );
}

// S18 / P54(a): a moving release planted against a wall, its stride playing at
// half rate, still reaches its moment, at its next pass from its drawn phase
// in clip time, and leaves on the next evaluation.
#[test]
fn a_release_at_half_rate_still_fires_at_its_moment() {
    let mut scene = Scene::new(vec![looping("strike_loop", 1.0, &[("hit", 0.8)])]);
    let enemy = scene.spawn(0, &moving_strike(timed(SLAM, "hit")).graph(), RELEASE);
    let phase = f64::from(instance_phase(enemy.to_raw(), 1.0));
    // The first tick whose half-rate window holds a pass of 0.8.
    let step = f64::from(DT) * 0.5;
    let passes = |time: f64| ((time + 1.0e-4 - 0.8) / 1.0).floor();
    let expected = (1..240)
        .find(|&k| passes(phase + step * k as f64) > passes(phase + step * (k - 1) as f64))
        .unwrap();
    assert!(expected <= 120, "fixture: within 2 s of game time");
    let log = run(&mut scene, enemy, 130, |scene, _| {
        scene.set_rate(enemy, 0.5)
    });
    assert_eq!(hit_ticks(&log), vec![expected]);
    assert_eq!(first_in(&log, RECOVER), Some(expected + 1));
}

// S18 / P54(c): a moving release with no moment and no `ms` whose variant
// flips every half second leaves once one second of clip time, summed across
// the restarts, has passed.
#[test]
fn a_release_with_no_moment_leaves_after_one_summed_pass_across_restarts() {
    let (mut scene, enemy) = flipping_scene(&moving_strike(untimed(SLAM)));
    let log = run(&mut scene, enemy, 90, |scene, tick| {
        flip_every_half_second(scene, enemy, tick)
    });
    assert_eq!(hit_ticks(&log), vec![0], "the contact fires on entry");
    assert_eq!(first_in(&log, RECOVER), Some(ticks(1.0) + 1));
}

// S12: an omitted `recover` holds the release's state, its clip time running
// on from the release's entry, and leaves on the first evaluation its clip
// reads played through, or once the default duration passes if that comes
// first; on a looping state, only the duration.
#[test]
fn the_default_recover_runs_the_releases_clip_out_capped_by_the_default_duration() {
    let cap = timer_tick(DEFAULT_RECOVERY_MS);
    for (label, clip, leaves) in [
        ("clip ends first", strike_clip(), ticks(1.0) + 1),
        (
            "cap first",
            one_shot(STRIKE, 2.0, &[("hit", 0.5)]),
            LEAVES + cap,
        ),
        ("looping", looping(STRIKE, 1.0, &[("hit", 0.5)]), usize::MAX),
    ] {
        let strike = Strike::default();
        let (mut scene, enemy) = strike_scene(vec![clip], &strike, RELEASE);
        let mut recover_time = None;
        let log = run(&mut scene, enemy, 140, |scene, tick| {
            if tick == LEAVES + 1 && recover_time.is_none() {
                recover_time = Some(scene.brain(enemy).clip_clock.time);
            }
        });
        let entered = first_in(&log, RECOVER).expect("enters the recover");
        assert_eq!(log[entered].mesh, STRIKE, "{label}: on the release's state");
        let leaves = if leaves == usize::MAX {
            // A loop never reads done, so the cap, from the recover's entry.
            entered + cap
        } else {
            leaves
        };
        assert_eq!(first_in(&log, COIL), Some(leaves), "{label}");
        if label != "looping" {
            let time = recover_time.unwrap();
            assert!(
                (time - f64::from(LEAVES as f32 * DT)).abs() < 1.0e-3,
                "{label}: the recover continues the release's clip time, not restarting: {time}"
            );
        }
    }
}

// S13: an authored `recover` without `ms` on its own state leaves when its clip
// plays through or the default duration passes; on a looping state only the
// duration; an authored `recover.ms` sets its duration.
#[test]
fn an_authored_recover_leaves_on_its_clip_or_the_default_duration_or_its_ms() {
    let cap = timer_tick(DEFAULT_RECOVERY_MS);
    for (label, clip, rows, after_entry) in [
        (
            "own one-shot",
            one_shot("smash_recover", 0.5, &[]),
            default_recover_rows(),
            ticks(0.5) + 1,
        ),
        (
            "own loop",
            looping("smash_recover", 0.5, &[]),
            default_recover_rows(),
            cap,
        ),
        (
            "`ms: 245`",
            one_shot("smash_recover", 0.5, &[]),
            vec![edge(COIL, after_ms(245.0))],
            timer_tick(245.0),
        ),
    ] {
        let strike = Strike {
            recover_state: "smash_recover",
            recover_rows: rows,
            ..Strike::default()
        };
        let (mut scene, enemy) = strike_scene(vec![strike_clip(), clip], &strike, RELEASE);
        let log = run(&mut scene, enemy, 120, |_, _| {});
        assert_eq!(first_in(&log, RECOVER), Some(LEAVES), "{label}");
        assert_eq!(first_in(&log, COIL), Some(LEAVES + after_entry), "{label}");
    }
}

// P9 / P45: `release.ms: 0` with a moment at 0 fires on the entry tick and
// leaves on the next evaluation; with one past 0 it waits, fires there and
// leaves on the next evaluation.
// P9 / P56: `release.ms: 0` with `at: false` fires at most once, from its
// entry tick, and leaves once its clip plays through, one pass of a loop.
// P9: `recover.ms: 0` leaves on its first evaluation after entry.
#[test]
fn zero_release_and_recover_ms() {
    let one_pass = ticks(1.0) + 1;
    for (label, clip, action, fires, leaves) in [
        (
            "P45(a) event at 0",
            one_shot(STRIKE, 1.0, &[("hit", 0.0)]),
            timed(SLAM, "hit"),
            0,
            1,
        ),
        (
            "P45(b) event past 0",
            strike_clip(),
            timed(SLAM, "hit"),
            MOMENT,
            LEAVES,
        ),
        ("P56(a) one-shot", strike_clip(), untimed(SLAM), 0, one_pass),
        (
            "P56(b) loop",
            looping(STRIKE, 1.0, &[]),
            untimed(SLAM),
            0,
            one_pass,
        ),
    ] {
        let strike = Strike {
            action,
            release_ms: Some(0.0),
            recover_rows: vec![edge(COIL, after_ms(0.0))],
            ..Strike::default()
        };
        let (mut scene, enemy) = strike_scene(vec![clip], &strike, RELEASE);
        let log = run(&mut scene, enemy, 75, |_, _| {});
        assert_eq!(hit_ticks(&log), vec![fires], "{label}: at most once");
        assert_eq!(first_in(&log, RECOVER), Some(leaves), "{label}");
        assert_eq!(
            first_in(&log, COIL),
            Some(leaves + 1),
            "{label}: a zero recover leaves on its first evaluation"
        );
    }
}

/// An impact effect switching the enemy to `reaction` ahead of tick `at`.
fn flinch_at(at: usize, reaction: &'static str) -> impl FnMut(&mut Scene, EntityId, usize) {
    move |scene, enemy, tick| {
        if tick == at {
            scene.switch(enemy, reaction);
        }
    }
}

fn run_flinched(
    strike: &Strike,
    specs: Vec<ClipSpec>,
    start: &str,
    ticks: usize,
    mut flinches: impl FnMut(&mut Scene, EntityId, usize),
) -> Vec<Tick> {
    let (mut scene, enemy) = strike_scene(specs, strike, start);
    run(&mut scene, enemy, ticks, |scene, tick| {
        flinches(scene, enemy, tick)
    })
}

// S21: an impact switch before the moment cancels the hit, and the same
// release unflinched fires at its moment (P52 base). The reaction then plays
// through on the game tick, the strike never drawn back over it, and the enemy
// enters its ending stance's rest, playing its stance sound, never its
// recover; the turn ends on leaving the strike.
#[test]
fn a_flinch_before_the_moment_cancels_it_plays_out_and_enters_the_ending_rest() {
    let strike = Strike::default();
    let unflinched = run_flinched(&strike, vec![strike_clip()], RELEASE, 45, |_, _, _| {});
    assert_eq!(
        hit_ticks(&unflinched),
        vec![MOMENT],
        "fixture: unflinched, it fires"
    );

    let reaction = ticks(0.4);
    for (label, clip, ends) in [
        (
            "a 400 ms reaction plays through",
            "hit_reaction",
            10 + reaction,
        ),
        (
            "a 900 ms reaction is capped at 600 ms",
            LONG_REACTION,
            10 + stagger_cap(),
        ),
    ] {
        let (mut scene, enemy) = strike_scene(vec![strike_clip()], &strike, RELEASE);
        one_token(&mut scene, 1);
        draw_token_zero(&mut scene, enemy, SLAM);
        let mut held = Vec::new();
        let log = run(&mut scene, enemy, 90, |scene, tick| {
            held.push(holds_turn(scene, enemy));
            flinch_at(10, clip)(scene, enemy, tick);
        });
        assert!(
            hit_ticks(&log).is_empty(),
            "{label}: the moment is cancelled"
        );
        assert!(
            never_in(&log, RECOVER),
            "{label}: the recover is never entered"
        );
        assert_eq!(first_in(&log, COIL), Some(ends), "{label}");
        assert!(
            log[10..ends]
                .iter()
                .all(|tick| tick.mesh == clip && tick.state == RELEASE),
            "{label}: the reaction stays on screen, no switch back to the strike: {:?}",
            &log[10..ends]
        );
        assert_eq!(log[ends].mesh, "idle", "{label}: the rest's own state");
        assert_eq!(
            log[ends].entered,
            vec![COIL.to_string()],
            "{label}: a fresh entry of the pose, its stance sound played"
        );
        assert!(held[5], "fixture: the strike holds its turn");
        assert!(
            !held[ends + 1],
            "{label}: the turn ends on leaving the strike"
        );
    }
}

// S21 / P46: a switch landing before the AI pass on the tick the release is
// entered does not cancel it; one on the next tick does.
#[test]
fn a_flinch_before_the_ai_on_the_releases_entry_tick_does_not_cancel_it() {
    let strike = Strike::default();
    let entry = timer_tick(strike.hold_ms);
    let log = run_flinched(&strike, vec![strike_clip()], HOLD, 60, |_, _, _| {});
    assert_eq!(
        first_in(&log, RELEASE),
        Some(entry),
        "fixture: the hold releases"
    );
    assert_eq!(hit_ticks(&log), vec![entry + MOMENT]);

    let log = run_flinched(
        &strike,
        vec![strike_clip()],
        HOLD,
        60,
        flinch_at(entry, "hit_reaction"),
    );
    assert_eq!(first_in(&log, RELEASE), Some(entry));
    assert_eq!(
        hit_ticks(&log),
        vec![entry + MOMENT],
        "the entry draws over the flinch, and its moment fires"
    );
    assert!(never_in(&log, COIL), "no stagger");

    let log = run_flinched(
        &strike,
        vec![strike_clip()],
        HOLD,
        60,
        flinch_at(entry + 1, "hit_reaction"),
    );
    assert!(
        hit_ticks(&log).is_empty(),
        "a flinch on the next tick cancels it"
    );
    assert_eq!(first_in(&log, COIL), Some(entry + 1 + ticks(0.4)));
}

// S22 / P55(a): a second switch during the reaction never extends it past the
// cap counted from the first: the enemy enters its ending rest 600 ms after
// the first switch, whatever the later reaction's length, and steady flinches
// never hold it longer.
#[test]
fn a_later_flinch_never_extends_the_stagger_past_the_cap_from_the_first() {
    let strike = Strike::default();
    let cap_ends = 10 + stagger_cap();
    let log = run_flinched(
        &strike,
        vec![strike_clip()],
        RELEASE,
        90,
        |scene, enemy, tick| match tick {
            10 => scene.switch(enemy, "hit_reaction"),
            28 => scene.switch(enemy, LONG_REACTION),
            _ => {}
        },
    );
    assert_eq!(
        first_in(&log, COIL),
        Some(cap_ends),
        "a 900 ms reaction at 300 ms"
    );

    let log = run_flinched(
        &strike,
        vec![strike_clip()],
        RELEASE,
        90,
        |scene, enemy, tick| {
            if tick >= 10 && tick % 6 == 4 {
                let reaction = if (tick / 6) % 2 == 0 {
                    "hit_reaction"
                } else {
                    LONG_REACTION
                };
                scene.switch(enemy, reaction);
            }
        },
    );
    assert_eq!(first_in(&log, COIL), Some(cap_ends), "steady flinches");
    assert!(hit_ticks(&log).is_empty());
}

// S22 / P55(b): a level `"*"` row passing during the reaction leaves at once;
// nothing fires.
#[test]
fn a_wildcard_row_during_the_reaction_leaves_at_once() {
    let strike = Strike {
        wildcard: vec![edge(OVERHEAD, after_ms(290.0))],
        ..Strike::default()
    };
    let log = run_flinched(
        &strike,
        vec![strike_clip()],
        RELEASE,
        60,
        flinch_at(6, "hit_reaction"),
    );
    let row = timer_tick(290.0);
    assert!(row < 6 + ticks(0.4), "fixture: the row passes mid-reaction");
    assert_eq!(
        first_in(&log, OVERHEAD),
        Some(row),
        "the `*` row wins at once"
    );
    assert!(never_in(&log, COIL));
    assert!(hit_ticks(&log).is_empty(), "nothing fires");
}

// S23 / P52: a switch after the moment, during an `at: false` release, or
// during a recover fires nothing further, plays its reaction out and enters
// the ending rest; a release with `ms: 2000` flinched at 200 ms by a 400 ms
// reaction enters it at 600 ms.
#[test]
fn a_flinch_after_the_moment_on_a_free_release_or_in_a_recover_plays_out_into_the_rest() {
    let reaction = ticks(0.4);
    for (label, strike, flinch, hits) in [
        (
            "after the moment, `ms: 790` holding it",
            Strike {
                release_ms: Some(790.0),
                ..Strike::default()
            },
            35,
            vec![MOMENT],
        ),
        (
            "an `at: false` release",
            Strike {
                action: untimed(SLAM),
                ..Strike::default()
            },
            10,
            vec![0],
        ),
        (
            "a recover",
            Strike {
                recover_action: false,
                ..Strike::default()
            },
            40,
            vec![MOMENT],
        ),
        (
            "`ms: 2000` flinched at 200 ms",
            Strike {
                release_ms: Some(2000.0),
                ..Strike::default()
            },
            ticks(0.2),
            vec![],
        ),
    ] {
        let log = run_flinched(
            &strike,
            vec![strike_clip()],
            RELEASE,
            90,
            flinch_at(flinch, "hit_reaction"),
        );
        assert_eq!(hit_ticks(&log), hits, "{label}: nothing further fires");
        assert_eq!(first_in(&log, COIL), Some(flinch + reaction), "{label}");
        assert!(
            log[flinch..flinch + reaction]
                .iter()
                .all(|tick| tick.mesh == "hit_reaction"),
            "{label}: the reaction plays through"
        );
        assert_eq!(log[flinch + reaction].entered, vec![COIL.to_string()]);
    }
    assert_eq!(
        ticks(0.2) + reaction,
        ticks(0.6),
        "fixture: the rest at 600 ms"
    );
}

// S23 / P52: a flinched moving release stops steering, and its strike clip is
// never redrawn before it enters the rest, though it stops, its stride
// resumes and its travel variant changes under the reaction.
#[test]
fn a_flinched_moving_release_stops_steering_and_never_redraws_its_strike() {
    let strike = moving_strike(timed(SLAM, "hit"));
    let (mut scene, enemy) = strike_scene(variant_loops(), &strike, RELEASE);
    let mut destinations = Vec::new();
    let log = run(&mut scene, enemy, 60, |scene, tick| {
        destinations.push(scene.agent(enemy).destination);
        match tick {
            10 => scene.switch(enemy, "hit_reaction"),
            15 => scene.set_agent(enemy, |agent| agent.velocity = Vec3::new(0.0, 0.0, 2.0)),
            20 => scene.set_agent(enemy, |agent| agent.velocity = Vec3::ZERO),
            25 => scene.set_agent(enemy, |agent| agent.velocity = Vec3::new(0.0, 0.0, -2.0)),
            _ => {}
        }
    });
    let ends = 10 + ticks(0.4);
    assert!(
        destinations[5].is_some(),
        "fixture: the release chases its target"
    );
    assert!(
        destinations[11..=ends].iter().all(Option::is_none),
        "planted from the flinch: {destinations:?}"
    );
    assert!(
        log[10..ends].iter().all(|tick| tick.mesh == "hit_reaction"),
        "the strike is never drawn over the reaction: {:?}",
        &log[10..ends]
    );
    assert_eq!(first_in(&log, COIL), Some(ends));
}

/// Land `amount` on the enemy through the damage chokepoint, as
/// `applyDamage` does.
fn land(scene: &mut Scene, enemy: EntityId, amount: f32) {
    assert!(apply_damage_with_context(
        &mut scene.reg,
        enemy,
        &DamagePayload {
            amount,
            impulse: Vec3::ZERO,
            damage_type: None,
        },
        DamageContext {
            source_id: "test.release_exit".to_string(),
            attacker: None,
            weapon: None,
            zone: None,
            producer: DamageProducer::InTick,
            point: None,
        },
    ));
}

/// The hold with the stance's `breakOn: 25` row first, then its release row.
fn breaking_strike(hold_ms: f32) -> BehaviorGraphDescriptor {
    let strike = Strike {
        hold_ms,
        ..Strike::default()
    };
    let mut graph = strike.graph();
    graph
        .envelope
        .transitions
        .get_mut(HOLD)
        .unwrap()
        .insert(0, edge(FLINCH, damage_at_least(25.0)));
    graph
}

// D3 / P1: the break row comes first, so when the hold's release time and the
// break threshold pass on one evaluation, the break wins and the release never
// fires. `breakOn` leaves on the first evaluation at or past the threshold,
// never below it.
#[test]
fn a_break_beats_a_due_release_and_trips_at_its_threshold_never_below() {
    let due = timer_tick(290.0);
    let mut scene = Scene::new(vec![strike_clip()]);
    let enemy = scene.spawn(0, &breaking_strike(290.0), HOLD);
    toughen(&mut scene, enemy);
    let log = run(&mut scene, enemy, 60, |scene, tick| {
        if tick == due {
            land(scene, enemy, 25.0);
        }
    });
    assert_eq!(log[due - 1].state, HOLD);
    assert_eq!(first_in(&log, FLINCH), Some(due), "the break wins");
    assert!(never_in(&log, RELEASE) && hit_ticks(&log).is_empty());

    let mut scene = Scene::new(vec![strike_clip()]);
    let enemy = scene.spawn(0, &breaking_strike(100_000.0), HOLD);
    toughen(&mut scene, enemy);
    let log = run(&mut scene, enemy, 20, |scene, tick| match tick {
        5 => land(scene, enemy, 24.0),
        10 => land(scene, enemy, 1.0),
        _ => {}
    });
    assert!(
        log[..10].iter().all(|tick| tick.state == HOLD),
        "24 of 25 never breaks"
    );
    assert_eq!(
        first_in(&log, FLINCH),
        Some(10),
        "the first evaluation at 25"
    );
}

// D4: a release has no break row, so damage that would break its stance does
// not break the strike; a level `"*"` row passing during the release or the
// recover leaves it, outranking the strike's own rows; damage taken in the
// flinch does not carry into the rest it resumes.
#[test]
fn a_release_does_not_break_a_wildcard_row_leaves_it_and_flinch_damage_stays_behind() {
    let mut scene = Scene::new(vec![strike_clip()]);
    let enemy = scene.spawn(0, &breaking_strike(95.0), RELEASE);
    toughen(&mut scene, enemy);
    let log = run(&mut scene, enemy, 45, |scene, tick| {
        if tick == 10 {
            land(scene, enemy, 100.0);
        }
    });
    assert_eq!(hit_ticks(&log), vec![MOMENT], "the strike lands");
    assert_eq!(first_in(&log, RECOVER), Some(LEAVES), "and runs its course");
    assert!(never_in(&log, FLINCH));

    let wildcard = Strike {
        wildcard: vec![edge(FLINCH, damage_at_least(50.0))],
        ..Strike::default()
    };
    for (label, at, flinches) in [
        ("on the release's own leaving tick", LEAVES, LEAVES),
        ("during the recover", 40, 40),
    ] {
        let (mut scene, enemy) = strike_scene(vec![strike_clip()], &wildcard, RELEASE);
        let log = run(&mut scene, enemy, 70, |scene, tick| {
            if tick == at {
                land(scene, enemy, 60.0);
            }
            if tick == flinches + 5 {
                land(scene, enemy, 30.0);
            }
        });
        assert_eq!(first_in(&log, FLINCH), Some(flinches), "{label}");
        if at == LEAVES {
            assert!(
                never_in(&log, RECOVER),
                "{label}: the `*` row outranks the strike's"
            );
        }
        let resumed = first_in(&log, COIL).expect("the flinch resumes the rest");
        assert_eq!(resumed, flinches + timer_tick(450.0), "{label}");
        assert_eq!(
            scene.brain(enemy).damage_in_activity(0),
            Some(0.0),
            "{label}: the flinch's 30 damage stays behind"
        );
    }
}

// Sp6 / P18: a charge (a moving release, `speed: 2`, a contact attack with no
// `at`) hits on reach once per entry: a target already in reach is hit on the
// entry tick; one never reached before the later of `ms` and one pass of its
// clip sees no hit, and the recover, with no action, never fires.
#[test]
fn a_charge_hits_on_reach_once_or_leaves_after_ms_and_one_pass_with_no_hit() {
    let charge = |ms: f32| Strike {
        action: untimed(SLAM),
        release_ms: Some(ms),
        release_motion: MotionVerb::ChaseTarget,
        release_state: "run",
        release_speed: Some(2.0),
        recover_state: "idle",
        recover_rows: vec![edge(COIL, after_ms(590.0))],
        ..Strike::default()
    };
    let run_clip = || looping("run", 1.0, &[]);

    let (mut scene, enemy) = strike_scene(vec![run_clip()], &charge(1490.0), RELEASE);
    let log = run(&mut scene, enemy, 120, |_, _| {});
    assert_eq!(
        hit_ticks(&log),
        vec![0],
        "(a) in reach on entry: one hit, on entry"
    );

    let one_pass = ticks(1.0) + 1;
    for (ms, leaves) in [(1490.0, timer_tick(1490.0)), (490.0, one_pass)] {
        let (mut scene, enemy) = strike_scene(vec![run_clip()], &charge(ms), RELEASE);
        let pawn = scene.pawn;
        set_enemy_position(&mut scene.reg, pawn, Vec3::new(10.0, 0.0, 0.0));
        let log = run(&mut scene, enemy, leaves + 30, |scene, tick| {
            if tick == leaves + 2 {
                set_enemy_position(&mut scene.reg, pawn, player_spot());
            }
        });
        assert_eq!(
            first_in(&log, RECOVER),
            Some(leaves),
            "(b) `ms: {ms}`: the later of `ms` and one pass"
        );
        assert!(
            hit_ticks(&log).is_empty(),
            "(b) `ms: {ms}`: no hit, and the recover never fires though back in reach"
        );
    }
}

// S23 / P52: a flinch before an `at: false` release's gates first pass fires
// nothing that entry, though the target walks into reach mid-reaction.
#[test]
fn a_flinch_before_a_free_release_first_reaches_fires_nothing() {
    let strike = Strike {
        action: untimed(SLAM),
        ..Strike::default()
    };
    let (mut scene, enemy) = strike_scene(vec![strike_clip()], &strike, RELEASE);
    let pawn = scene.pawn;
    let log = run(&mut scene, enemy, 60, |scene, tick| match tick {
        0 => set_enemy_position(&mut scene.reg, pawn, Vec3::new(5.0, 0.0, 0.0)),
        10 => scene.switch(enemy, "hit_reaction"),
        15 => set_enemy_position(&mut scene.reg, pawn, player_spot()),
        _ => {}
    });
    assert!(hit_ticks(&log).is_empty(), "nothing fires after the flinch");
    assert_eq!(first_in(&log, COIL), Some(10 + ticks(0.4)));
}

// S23 / P52: a flinch after a weapon release's moment cancels the burst in
// flight: no shot after the switch.
#[test]
fn a_flinch_cancels_a_weapon_burst_in_flight() {
    let shots = |flinch: Option<usize>| {
        let mut volley = contact_attack(0.0);
        volley.weapon = Some("enemy.rifle".to_string());
        (volley.damage, volley.max_range, volley.cooldown_ms) = (None, None, None);
        let strike = Strike {
            release_ms: Some(1490.0),
            attack: volley,
            ..Strike::default()
        };
        let (mut scene, enemy) = strike_scene(vec![strike_clip()], &strike, RELEASE);
        scene.descriptors = vec![burst_weapon("enemy.rifle", 3, 100.0)];
        (0..80)
            .filter(|&tick| {
                if Some(tick) == flinch {
                    scene.switch(enemy, "hit_reaction");
                }
                !scene.tick().projectile_spawns.is_empty()
            })
            .collect::<Vec<usize>>()
    };
    let unflinched = shots(None);
    assert_eq!(
        unflinched.len(),
        3,
        "fixture: a three-shot burst: {unflinched:?}"
    );
    assert_eq!(unflinched[0], MOMENT, "fixture: from the moment");
    assert_eq!(
        shots(Some(MOMENT + 2)),
        vec![MOMENT],
        "the shots after the flinch never fire"
    );
}

/// A release on `smash_strike` whose attack is a three-shot burst from its
/// moment, 100 ms apart, held long past it by `ms`.
fn burst_strike(wildcard: Vec<GuardedRow>) -> Strike {
    let mut volley = contact_attack(0.0);
    volley.weapon = Some("enemy.rifle".to_string());
    (volley.damage, volley.max_range, volley.cooldown_ms) = (None, None, None);
    Strike {
        release_ms: Some(1490.0),
        attack: volley,
        wildcard,
        ..Strike::default()
    }
}

// Regression: a flinch whose own tick left the release, on a `"*"` row keyed
// on the same hit or on the flinch route of a reaction already played out,
// cleared its stagger before apply read it, so the burst in flight fired on.
// The flinch cancels it whatever left the strike: no shot after the switch.
#[test]
fn a_flinch_that_leaves_the_release_on_its_own_tick_still_cancels_the_burst() {
    const PAIN: f32 = 60.0;
    let flinch = MOMENT + 2;
    for (label, wildcard, reaction, damage, leaves_to) in [
        (
            "a `*` pain row on the same hit",
            vec![edge(OVERHEAD, damage_at_least(50.0))],
            "hit_reaction",
            PAIN,
            OVERHEAD,
        ),
        (
            "a reaction played out on its own tick",
            Vec::new(),
            "hit_instant",
            0.0,
            COIL,
        ),
    ] {
        let strike = burst_strike(wildcard);
        let specs = vec![strike_clip(), one_shot("hit_instant", 0.00005, &[])];
        let (mut scene, enemy) = strike_scene(specs, &strike, RELEASE);
        scene.descriptors = vec![burst_weapon("enemy.rifle", 3, 100.0)];
        let mut left_to = String::new();
        let shots: Vec<usize> = (0..80)
            .filter(|&tick| {
                if tick == flinch {
                    scene.switch(enemy, reaction);
                    if damage > 0.0 {
                        land(&mut scene, enemy, damage);
                    }
                }
                let fired = !scene.tick().projectile_spawns.is_empty();
                if tick == flinch {
                    left_to = scene.state(enemy);
                }
                fired
            })
            .collect();
        assert_eq!(
            left_to, leaves_to,
            "{label}: left the release on the flinch's tick"
        );
        assert_eq!(
            shots,
            vec![MOMENT],
            "{label}: the shots after the flinch never fire"
        );
    }
}

// Regression: a release whose moment its entry clip times fired mid-swing as
// without `at` once a travel variant whose clip lacks the event drew. The
// moment stays pending on the variant and whiffs once one pass of clip time,
// summed across the restart, has passed.
#[test]
fn a_travel_variant_lacking_the_event_leaves_the_releases_moment_pending_until_it_whiffs() {
    let strike = Strike {
        release_facing: Some(FacingPolicy::Target),
        ..Strike::default()
    };
    let specs = || vec![strike_clip(), one_shot("smash_strike_right", 1.0, &[])];
    let (mut scene, enemy) = strike_scene(specs(), &strike, RELEASE);
    let log = run(&mut scene, enemy, 45, |_, _| {});
    assert_eq!(
        hit_ticks(&log),
        vec![MOMENT],
        "fixture: on its own clip it fires at its moment"
    );

    let (mut scene, enemy) = strike_scene(specs(), &strike, RELEASE);
    let log = run(&mut scene, enemy, 75, |scene, tick| {
        if tick == 10 {
            scene.set_agent(enemy, |agent| agent.velocity = Vec3::new(0.0, 0.0, 2.0));
        }
    });
    assert_eq!(
        log[15].mesh, "smash_strike_right",
        "fixture: the variant draws"
    );
    assert!(
        hit_ticks(&log).is_empty(),
        "never fired mid-swing: {:?}",
        hit_ticks(&log)
    );
    assert_eq!(
        first_in(&log, RECOVER),
        Some(ticks(1.0) + 1),
        "whiffed after one summed pass, left on the next evaluation"
    );
}

// A fresh entry of a release starts its own gate and drops a finished flinch:
// a strike flinched into its ending rest and struck again from it waits for
// its new moment, fires there, and leaves on the next evaluation.
#[test]
fn a_release_entered_again_waits_for_its_own_moment_after_a_flinch() {
    let strike = Strike::default();
    let mut graph = strike.graph();
    graph
        .envelope
        .transitions
        .insert(COIL.to_string(), vec![edge(RELEASE, after_ms(95.0))]);
    let mut scene = Scene::new(vec![strike_clip()]);
    let enemy = scene.spawn(0, &graph, RELEASE);
    let mut flinch = flinch_at(10, "hit_reaction");
    let log = run(&mut scene, enemy, 120, |scene, tick| {
        flinch(scene, enemy, tick)
    });
    let rested = 10 + ticks(0.4);
    assert_eq!(
        first_in(&log, COIL),
        Some(rested),
        "fixture: the flinch ends in the rest"
    );
    let again = rested + timer_tick(95.0);
    assert_eq!(
        log[again].state, RELEASE,
        "fixture: struck again from the rest"
    );
    assert_eq!(
        hit_ticks(&log),
        vec![again + MOMENT],
        "the new entry fires at its moment"
    );
    assert_eq!(log[again + LEAVES].state, RECOVER);
    assert!(
        log[again..again + LEAVES]
            .iter()
            .all(|tick| tick.state == RELEASE)
    );
}
