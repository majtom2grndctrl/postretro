// Stance poses through the AI tick: a pose's facts and entry carried between
// its rest and holds, the hidden release draw, the self-pose row skip, and
// target retention under poses and attack marks. Graphs are hand-written in
// the lowered shape (stance lowering builds them later).
// See: context/lib/entity_model.md §7c (Stances and attacks)

use super::attack_turn_ticks::{held, set_draws, set_limits};
use super::*;
use postretro_foundation::{BRAIN_ACTIVITY_ROLL_INPUT, BRAIN_DAMAGE_IN_ACTIVITY_INPUT};

const DT: f32 = 0.016;
const DT_MS: f32 = 16.0;
const POSE: &str = "overhead";
const APPROACH: &str = "approach";
const REST: &str = "overhead";
const HOLD: &str = "slam_overhead";
const RELEASE: &str = "slam_release";
const RECOVER: &str = "slam_recover";
const FLINCH: &str = "flinch";
const SLAM: &str = "slam";
const SWEEP: &str = "sweep";
/// The stance's `breakOn.damage`.
const BREAK_DAMAGE: f32 = 25.0;
/// A release range no test outlasts.
const NEVER: [f32; 2] = [100_000.0, 100_000.0];

fn number(value: f32) -> Box<IrNode> {
    Box::new(IrNode::Const {
        value: IrValue::Number(value),
    })
}

fn ge(input: &str, value: f32) -> IrNode {
    IrNode::Ge {
        a: Box::new(brain_input(input)),
        b: number(value),
    }
}

fn lt(input: &str, value: f32) -> IrNode {
    IrNode::Lt {
        a: Box::new(brain_input(input)),
        b: number(value),
    }
}

fn not(node: IrNode) -> IrNode {
    IrNode::Not { x: Box::new(node) }
}

fn always() -> IrNode {
    IrNode::Const {
        value: IrValue::Bool(true),
    }
}

fn after_ms(ms: f32) -> IrNode {
    ge(BRAIN_TIME_IN_ACTIVITY_MS_INPUT, ms)
}

fn turn_held(attack: &str) -> IrNode {
    brain_input(&format!("@attackTurnHeld.{attack}"))
}

fn posed(mut activity: BehaviorActivityDescriptor) -> BehaviorActivityDescriptor {
    activity.pose = Some(POSE.to_string());
    activity
}

fn marked(mut activity: BehaviorActivityDescriptor, attack: &str) -> BehaviorActivityDescriptor {
    activity.attack_turn = Some(attack.to_string());
    activity
}

fn release_row(to: &str, range: [f32; 2]) -> GuardedRow {
    GuardedRow {
        release_after: Some(range),
        ..edge(to, always())
    }
}

/// The stance's `breakOn` row, which lowering puts first on the rest and on
/// every hold.
fn break_row() -> GuardedRow {
    edge(FLINCH, ge(BRAIN_DAMAGE_IN_ACTIVITY_INPUT, BREAK_DAMAGE))
}

/// A hold's rows in lowering's order: break, turn-lost, release.
fn hold_rows(attack: &str, release: &str, range: [f32; 2]) -> Vec<GuardedRow> {
    vec![
        break_row(),
        edge(REST, not(turn_held(attack))),
        release_row(release, range),
    ]
}

fn contact_slam() -> AttackParams {
    AttackParams {
        sound: None,
        weapon: None,
        damage: Some(1.0),
        max_range: Some(2.0),
        cooldown_ms: Some(0.0),
        engagement_radius: None,
        standoff_distance: None,
        damage_type: None,
        draws: None,
    }
}

fn root_graph(
    activities: Vec<(&str, BehaviorActivityDescriptor)>,
    transitions: Vec<(&str, Vec<GuardedRow>)>,
    attacks: Vec<&str>,
) -> BehaviorGraphDescriptor {
    test_behavior_graph!({
        initial: APPROACH.to_string(),
        activities: activities
            .into_iter()
            .map(|(name, activity)| (name.to_string(), activity))
            .collect(),
        transitions: transitions
            .into_iter()
            .filter(|(_, rows)| !rows.is_empty())
            .map(|(from, rows)| (from.to_string(), rows))
            .collect(),
        candidate_filter: None,
        retaliation: None,
        patrol: None,
        attacks: attacks
            .into_iter()
            .map(|name| (name.to_string(), contact_slam()))
            .collect(),
        engagement_radius: None,
        move_speed: TEST_MOVE_SPEED,
    })
}

/// One stance and one strike from it, lowered at the root level beside a
/// chase: `approach` chases into the rest; the rest carries the break row and
/// then `rest_rows`; the strike runs hold → release → recover → rest. The
/// release fires `slam` on entry. `wildcard` are the level's `"*"` rows.
fn stance_graph(
    rest_rows: Vec<GuardedRow>,
    release_after: [f32; 2],
    wildcard: Vec<GuardedRow>,
) -> BehaviorGraphDescriptor {
    let mut rest = vec![break_row()];
    rest.extend(rest_rows);
    root_graph(
        vec![
            (
                APPROACH,
                authored_state("walk", MotionVerb::ChaseTarget, None),
            ),
            (
                REST,
                posed(authored_state("attack", MotionVerb::Hold, None)),
            ),
            (
                HOLD,
                marked(
                    posed(authored_state("attack", MotionVerb::Hold, None)),
                    SLAM,
                ),
            ),
            (
                RELEASE,
                marked(
                    authored_state(
                        "attack_slam",
                        MotionVerb::Hold,
                        Some(ActionVerb::Attack {
                            name: SLAM.to_string(),
                            at: None,
                            at_default: false,
                        }),
                    ),
                    SLAM,
                ),
            ),
            (
                RECOVER,
                marked(authored_state("attack_slam", MotionVerb::Hold, None), SLAM),
            ),
            (FLINCH, authored_state("idle", MotionVerb::Hold, None)),
        ],
        vec![
            ("*", wildcard),
            (APPROACH, vec![edge(REST, target_within(3.0))]),
            (REST, rest),
            (HOLD, hold_rows(SLAM, RELEASE, release_after)),
            (RELEASE, vec![edge(RECOVER, after_ms(48.0))]),
            (RECOVER, vec![edge(REST, after_ms(48.0))]),
            (FLINCH, vec![edge(REST, after_ms(450.0))]),
        ],
        vec![SLAM],
    )
}

/// An enemy that holds a `slam` turn for ever: one marked activity, no rows.
fn hog_graph() -> BehaviorGraphDescriptor {
    test_behavior_graph!({
        initial: "hog".to_string(),
        activities: BTreeMap::from([(
            "hog".to_string(),
            marked(authored_state("attack", MotionVerb::Hold, None), SLAM),
        )]),
        transitions: BTreeMap::new(),
        candidate_filter: None,
        retaliation: None,
        patrol: None,
        attacks: BTreeMap::from([(SLAM.to_string(), contact_slam())]),
        engagement_radius: None,
        move_speed: TEST_MOVE_SPEED,
    })
}

fn spawn_posed(reg: &mut EntityRegistry, graph: &BehaviorGraphDescriptor, state: &str) -> EntityId {
    spawn_enemy(reg, Vec3::ZERO, authored_brain(graph, state), 500.0)
}

fn brain(reg: &EntityRegistry, enemy: EntityId) -> BrainComponent {
    reg.get_component::<BrainComponent>(enemy).unwrap().clone()
}

/// The root level's scope-relative facts: time, roll, damage sum and attack
/// count.
fn pose_facts(reg: &EntityRegistry, enemy: EntityId) -> (f32, f32, f32, u32) {
    let brain = brain(reg, enemy);
    (
        brain.activity_timer(0).unwrap(),
        brain.activity_roll(0).unwrap(),
        brain.damage_in_activity(0).unwrap(),
        brain.activity_attack_count(0).unwrap(),
    )
}

pub(super) fn mesh_switches(reg: &EntityRegistry, enemy: EntityId) -> u32 {
    reg.get_component::<MeshComponent>(enemy)
        .unwrap()
        .animation
        .as_ref()
        .unwrap()
        .switches
        .get()
}

/// Land `amount` on the enemy through the damage chokepoint, with no
/// attacker, as `applyDamage` does.
pub(super) fn hit(reg: &mut EntityRegistry, enemy: EntityId, amount: f32) {
    assert!(apply_damage_with_context(
        reg,
        enemy,
        &DamagePayload {
            amount,
            impulse: Vec3::ZERO,
            damage_type: None,
        },
        DamageContext {
            source_id: "test.stance".to_string(),
            attacker: None,
            weapon: None,
            zone: None,
            producer: DamageProducer::InTick,
            point: None,
        },
    ));
}

fn step_in(
    reg: &mut EntityRegistry,
    runtime: &mut AiRuntime,
    world: Option<&CollisionWorld>,
) -> AiTickResult {
    run_ai_tick_with_navigation_and_impact(
        reg,
        runtime,
        DT,
        AiTickInputs {
            mover_nav: crate::nav::MoverNav::none(),
            nav_graph: None,
            collision_world: world,
            descriptors: &[],
            descriptor_generation: 0,
            factions: &FactionRegistry::default(),
            faction_sentiment: &RefCell::new(FactionSentimentState::default()),
            clips: None,
        },
        |_| {},
    )
}

fn step(reg: &mut EntityRegistry, runtime: &mut AiRuntime) -> AiTickResult {
    step_in(reg, runtime, None)
}

/// The leaves `enemy` announced entering this tick: what plays an entry sound.
pub(super) fn announced(
    result: &AiTickResult,
    reg: &EntityRegistry,
    enemy: EntityId,
) -> Vec<String> {
    let _ = reg;
    result
        .events
        .iter()
        .filter(|emission| {
            matches!(emission.emitter, postretro_entities::Emitter::Entity { id, .. } if id == enemy)
        })
        .filter_map(|emission| match &emission.cue {
            postretro_entities::AiCue::Entered { graph, path } => {
                postretro_entities::components::brain::activity_at_path(graph, path)
                    .map(|(name, _)| name.to_string())
            }
            _ => None,
        })
        .collect()
}

/// Tick until `enemy` sits in `state`, returning that tick's result.
fn step_until(
    reg: &mut EntityRegistry,
    runtime: &mut AiRuntime,
    enemy: EntityId,
    state: &str,
    limit: usize,
) -> AiTickResult {
    for _ in 0..limit {
        let result = step(reg, runtime);
        if enemy_state_name(reg, enemy) == state {
            return result;
        }
    }
    panic!(
        "`{state}` not reached in {limit} ticks; at `{}`",
        enemy_state_name(reg, enemy)
    );
}

fn assert_near(actual: f32, expected: f32, context: &str) {
    assert!(
        (actual - expected).abs() < 1.0e-3,
        "{context}: {actual} vs {expected}"
    );
}

// P1 / P30: the rest and its hold are one activity to the engine. The move
// into the hold keeps the pose's timer, roll, damage sum and attack count,
// announces nothing (no second stance sound) and redraws no clip; 20 damage in
// the rest plus 20 in the hold trips `breakOn: 25`, which 20 alone did not.
#[test]
fn moving_from_a_rest_into_its_hold_carries_the_pose_and_announces_nothing() {
    let graph = stance_graph(vec![edge(HOLD, after_ms(96.0))], NEVER, vec![]);
    let mut reg = EntityRegistry::new();
    let mut runtime = AiRuntime::new();
    spawn_player(&mut reg, Vec3::new(1.5, 0.0, 0.0));
    let enemy = spawn_posed(&mut reg, &graph, APPROACH);

    let entry = step(&mut reg, &mut runtime);
    assert_eq!(enemy_state_name(&reg, enemy), REST);
    assert_eq!(
        announced(&entry, &reg, enemy),
        vec![REST],
        "taking the pose plays its sound"
    );
    let (_, rest_roll, _, _) = pose_facts(&reg, enemy);
    let switches = mesh_switches(&reg, enemy);
    let stamp = enemy_anim_entered_at(&reg, enemy);

    hit(&mut reg, enemy, 20.0);
    let mut before_hold = None;
    let mut into_hold = None;
    for _ in 0..12 {
        let facts = pose_facts(&reg, enemy);
        let result = step(&mut reg, &mut runtime);
        if enemy_state_name(&reg, enemy) == HOLD {
            before_hold = Some(facts);
            into_hold = Some(result);
            break;
        }
        assert_eq!(
            enemy_state_name(&reg, enemy),
            REST,
            "20 damage does not break"
        );
    }
    let (rest_time, _, _, _) = before_hold.expect("the rest takes its hold");
    let (hold_time, hold_roll, hold_damage, hold_count) = pose_facts(&reg, enemy);
    assert!(
        announced(&into_hold.unwrap(), &reg, enemy).is_empty(),
        "a move within the pose plays no second stance sound"
    );
    assert_near(hold_time, rest_time + DT_MS, "the pose's timer carries");
    assert_eq!(hold_roll, rest_roll, "the pose's roll carries");
    assert_near(hold_damage, 20.0, "the pose's damage sum carries");
    assert_eq!(hold_count, 0);
    assert_eq!(enemy_animation(&reg, enemy), "attack");
    assert_eq!(mesh_switches(&reg, enemy), switches, "no clip restarts");
    assert_eq!(enemy_anim_entered_at(&reg, enemy), stamp);

    hit(&mut reg, enemy, 20.0);
    let broken = step(&mut reg, &mut runtime);
    assert_eq!(
        enemy_state_name(&reg, enemy),
        FLINCH,
        "20 in the rest plus 20 in the hold trips the break"
    );
    assert_eq!(announced(&broken, &reg, enemy), vec![FLINCH]);
}

// P30 / P33(b): a hold whose turn cannot be granted returns to its rest on its
// next evaluation, and the pose stays unbroken across every bounce: its timer
// runs on, its roll and damage sum hold, and nothing announces or redraws.
#[test]
fn a_hold_that_loses_its_turn_returns_to_its_rest_with_the_pose_unbroken() {
    let graph = stance_graph(vec![edge(HOLD, after_ms(96.0))], NEVER, vec![]);
    let mut reg = EntityRegistry::new();
    let mut runtime = AiRuntime::new();
    set_limits(&mut runtime, &[1]);
    spawn_player(&mut reg, Vec3::new(1.5, 0.0, 0.0));
    let hog = spawn_enemy(
        &mut reg,
        Vec3::new(0.0, 0.0, 0.3),
        authored_brain(&hog_graph(), "hog"),
        500.0,
    );
    let enemy = spawn_posed(&mut reg, &graph, REST);
    set_draws(&reg, &mut runtime, &[], hog, SLAM, &[(0, 1)]);
    set_draws(&reg, &mut runtime, &[], enemy, SLAM, &[(0, 1)]);

    step(&mut reg, &mut runtime);
    assert_eq!(held(&reg, hog), vec![SLAM], "the hog takes the only room");
    hit(&mut reg, enemy, 10.0);
    let (_, roll, _, _) = pose_facts(&reg, enemy);
    let switches = mesh_switches(&reg, enemy);

    step_until(&mut reg, &mut runtime, enemy, HOLD, 12);
    let mut previous_time = pose_facts(&reg, enemy).0;
    let mut returns = 0;
    for _ in 0..8 {
        let result = step(&mut reg, &mut runtime);
        let state = enemy_state_name(&reg, enemy);
        assert_ne!(state, RELEASE);
        if state == REST {
            returns += 1;
        }
        assert!(
            announced(&result, &reg, enemy).is_empty(),
            "no stance sound"
        );
        let (time, now_roll, damage, _) = pose_facts(&reg, enemy);
        assert_near(time, previous_time + DT_MS, "the pose's timer runs on");
        assert_eq!(now_roll, roll);
        assert_near(damage, 10.0, "the damage sum holds");
        previous_time = time;
    }
    assert!(returns >= 3, "the hold returns to its rest: {returns}");
    assert!(held(&reg, enemy).is_empty());
    assert_eq!(mesh_switches(&reg, enemy), switches, "no clip restarts");
}

// P30 / R2: the release row reads the pose's carried time, so a hold entered
// 208 ms into its pose with a fixed 300 ms release leaves on the first
// evaluation at or past 300 ms of pose, not of hold.
#[test]
fn a_hold_releases_once_its_pose_has_lasted_the_drawn_time() {
    let graph = stance_graph(vec![edge(HOLD, after_ms(200.0))], [300.0, 300.0], vec![]);
    let mut reg = EntityRegistry::new();
    let mut runtime = AiRuntime::new();
    spawn_player(&mut reg, Vec3::new(1.5, 0.0, 0.0));
    let enemy = spawn_posed(&mut reg, &graph, REST);

    step_until(&mut reg, &mut runtime, enemy, HOLD, 20);
    assert_near(
        brain(&reg, enemy).hold_release_ms(0).unwrap(),
        300.0,
        "`[x, x]` draws x",
    );
    let mut hold_ticks = 0;
    let mut last_hold_time = pose_facts(&reg, enemy).0;
    loop {
        step(&mut reg, &mut runtime);
        if enemy_state_name(&reg, enemy) == RELEASE {
            break;
        }
        assert_eq!(enemy_state_name(&reg, enemy), HOLD);
        last_hold_time = pose_facts(&reg, enemy).0;
        hold_ticks += 1;
        assert!(hold_ticks < 20, "the hold never released");
    }
    assert!(
        last_hold_time < 300.0 && last_hold_time + DT_MS >= 300.0,
        "released on the first evaluation at or past 300 ms of pose: {last_hold_time}"
    );
    assert!(
        hold_ticks < 7,
        "timed from the rest's entry, not the hold's: {hold_ticks} hold ticks"
    );
}

// P3 / P34, from a recover: re-entering the rest from outside the pose plays
// its sound and starts the timer, damage sum and roll fresh.
#[test]
fn entering_the_rest_from_a_recover_starts_the_pose_fresh_and_announces_it() {
    let graph = stance_graph(vec![edge(HOLD, after_ms(32.0))], [0.0, 0.0], vec![]);
    let mut reg = EntityRegistry::new();
    let mut runtime = AiRuntime::new();
    spawn_player(&mut reg, Vec3::new(1.5, 0.0, 0.0));
    let enemy = spawn_posed(&mut reg, &graph, REST);
    step(&mut reg, &mut runtime);
    hit(&mut reg, enemy, 10.0);
    let (_, first_roll, _, _) = pose_facts(&reg, enemy);

    step_until(&mut reg, &mut runtime, enemy, RECOVER, 40);
    let back = step_until(&mut reg, &mut runtime, enemy, REST, 10);
    assert_eq!(announced(&back, &reg, enemy), vec![REST]);
    let (time, roll, damage, count) = pose_facts(&reg, enemy);
    assert_eq!(time, 0.0);
    assert_eq!(damage, 0.0);
    assert_eq!(count, 0);
    assert_ne!(roll, first_roll, "a fresh entry draws its own roll");
}

// P3 / P34, through an ancestor: the same stance's copies in two levels are
// separate poses, and an ancestor transition reseating a level starts its
// copy fresh, announced, both ways.
#[test]
fn an_ancestor_reseat_and_another_levels_copy_start_the_pose_fresh() {
    let copy = || {
        BehaviorLayerDescriptor::Graph(BehaviorGraphEnvelope {
            initial: REST.to_string(),
            activities: BTreeMap::from([(
                REST.to_string(),
                posed(authored_state("attack", MotionVerb::Hold, None)),
            )]),
            transitions: BTreeMap::new(),
        })
    };
    let composite = |layer| BehaviorActivityDescriptor {
        layers: BTreeMap::from([("offense".to_string(), layer)]),
        motion: None,
        animation: None,
        ..authored_state("attack", MotionVerb::Hold, None)
    };
    let graph = test_behavior_graph!({
        initial: "engage".to_string(),
        activities: BTreeMap::from([
            ("engage".to_string(), composite(copy())),
            ("press".to_string(), composite(copy())),
        ]),
        transitions: BTreeMap::from([
            ("engage".to_string(), vec![edge("press", after_ms(64.0))]),
            ("press".to_string(), vec![edge("engage", after_ms(64.0))]),
        ]),
        candidate_filter: None,
        retaliation: None,
        patrol: None,
        attacks: BTreeMap::new(),
        engagement_radius: None,
        move_speed: TEST_MOVE_SPEED,
    });
    let mut reg = EntityRegistry::new();
    let mut runtime = AiRuntime::new();
    spawn_player(&mut reg, Vec3::new(1.5, 0.0, 0.0));
    let enemy = spawn_posed(&mut reg, &graph, "engage");
    step(&mut reg, &mut runtime);
    assert_eq!(enemy_state_path(&reg, enemy), "engage/offense/overhead");

    for level in ["press", "engage"] {
        hit(&mut reg, enemy, 10.0);
        let roll = brain(&reg, enemy).activity_roll(1).unwrap();
        let entered = step_until(&mut reg, &mut runtime, enemy, level, 10);
        assert_eq!(announced(&entered, &reg, enemy), vec![REST], "{level}");
        let brain = brain(&reg, enemy);
        assert_eq!(brain.activity_timer(1), Some(0.0), "{level}");
        assert_eq!(brain.damage_in_activity(1), Some(0.0), "{level}");
        assert_ne!(brain.activity_roll(1), Some(roll), "{level}");
    }
}

// P5: a `"*"` row into the pose the enemy already holds is skipped in the
// rest and in the hold, while the rest's own row into its hold passes; the
// same row abandons a release and a recover, which are outside the pose, and
// enters the rest fresh.
#[test]
fn a_wildcard_row_into_the_current_pose_is_skipped_but_leaves_a_release_or_recover() {
    let graph = stance_graph(
        vec![edge(HOLD, after_ms(48.0))],
        [96.0, 96.0],
        vec![edge(REST, always())],
    );
    let mut reg = EntityRegistry::new();
    let mut runtime = AiRuntime::new();
    spawn_player(&mut reg, Vec3::new(1.5, 0.0, 0.0));
    let enemy = spawn_posed(&mut reg, &graph, REST);

    let mut states = Vec::new();
    for _ in 0..12 {
        step(&mut reg, &mut runtime);
        let state = enemy_state_name(&reg, enemy);
        if state == RELEASE {
            break;
        }
        states.push(state);
    }
    assert_eq!(
        enemy_state_name(&reg, enemy),
        RELEASE,
        "the hold reached its release: {states:?}"
    );
    let first_hold = states.iter().position(|state| state == HOLD).unwrap();
    assert!(
        states[first_hold..].iter().all(|state| state == HOLD),
        "the `*` row never pulls the hold back to its rest: {states:?}"
    );

    let left = step(&mut reg, &mut runtime);
    assert_eq!(
        enemy_state_name(&reg, enemy),
        REST,
        "the release is abandoned"
    );
    assert_eq!(announced(&left, &reg, enemy), vec![REST]);
    assert_eq!(pose_facts(&reg, enemy).0, 0.0, "a fresh pose");

    let recovering = spawn_posed(&mut reg, &graph, RECOVER);
    step(&mut reg, &mut runtime);
    step(&mut reg, &mut runtime);
    assert_eq!(
        enemy_state_name(&reg, recovering),
        REST,
        "the recover is abandoned"
    );
}

/// Two attacks sharing the pose: the rest strikes `slam` on a roll below 0.5,
/// else `sweep`; each hold releases on a `release_after` draw, and the
/// release and recover each last one evaluation.
fn mix_up_graph(release_after: [f32; 2]) -> BehaviorGraphDescriptor {
    let mut activities = vec![
        (
            APPROACH,
            authored_state("walk", MotionVerb::ChaseTarget, None),
        ),
        (
            REST,
            posed(authored_state("attack", MotionVerb::Hold, None)),
        ),
    ];
    let mut transitions = vec![(
        REST,
        vec![
            edge(HOLD, lt(BRAIN_ACTIVITY_ROLL_INPUT, 0.5)),
            edge("sweep_overhead", always()),
        ],
    )];
    for (attack, hold, release, recover) in [
        (SLAM, HOLD, RELEASE, RECOVER),
        (SWEEP, "sweep_overhead", "sweep_release", "sweep_recover"),
    ] {
        activities.push((
            hold,
            marked(
                posed(authored_state("attack", MotionVerb::Hold, None)),
                attack,
            ),
        ));
        activities.push((
            release,
            marked(
                authored_state("attack_slam", MotionVerb::Hold, None),
                attack,
            ),
        ));
        activities.push((
            recover,
            marked(
                authored_state("attack_slam", MotionVerb::Hold, None),
                attack,
            ),
        ));
        transitions.push((
            hold,
            vec![
                edge(REST, not(turn_held(attack))),
                release_row(release, release_after),
            ],
        ));
        transitions.push((release, vec![edge(recover, always())]));
        transitions.push((recover, vec![edge(REST, always())]));
    }
    root_graph(activities, transitions, vec![SLAM, SWEEP])
}

/// One strike of the mix-up: which attack the roll chose, the pose's roll,
/// the hold's release draw, and the pose time of the evaluation that released.
struct Strike {
    attack: &'static str,
    roll: f32,
    draw: f32,
    released_at: f32,
}

fn run_strikes(release_after: [f32; 2], count: usize) -> Vec<Strike> {
    let graph = mix_up_graph(release_after);
    let mut reg = EntityRegistry::new();
    let mut runtime = AiRuntime::new();
    spawn_player(&mut reg, Vec3::new(1.5, 0.0, 0.0));
    let enemy = spawn_posed(&mut reg, &graph, REST);
    let mut strikes = Vec::new();
    let mut holding: Option<(&'static str, f32, f32, f32)> = None;
    for _ in 0..count * 40 {
        step(&mut reg, &mut runtime);
        let state = enemy_state_name(&reg, enemy);
        let (time, roll, _, _) = pose_facts(&reg, enemy);
        match (state.as_str(), holding) {
            (HOLD | "sweep_overhead", Some(_)) => {
                holding = holding.map(|(attack, roll, draw, _)| (attack, roll, draw, time));
            }
            (HOLD | "sweep_overhead", None) => {
                let attack = if state == HOLD { SLAM } else { SWEEP };
                let draw = brain(&reg, enemy).hold_release_ms(0).unwrap();
                holding = Some((attack, roll, draw, time));
            }
            (RELEASE | "sweep_release", Some((attack, roll, draw, last))) => {
                strikes.push(Strike {
                    attack,
                    roll,
                    draw,
                    released_at: last + DT_MS,
                });
                holding = None;
                if strikes.len() == count {
                    return strikes;
                }
            }
            (other, Some(_)) => panic!("a hold left for `{other}` without releasing"),
            _ => {}
        }
    }
    panic!("only {} strikes in the run", strikes.len());
}

// P4 / P32 and R1: across many strikes from a rest that branches on
// `activityRoll`, each attack's release times span the whole hold range,
// uncorrelated with the roll that chose the strike, and every hold releases
// on the first evaluation at or past its own draw.
#[test]
fn release_times_span_the_hold_range_whatever_roll_chose_the_strike() {
    let [min, max] = [64.0_f32, 320.0];
    let strikes = run_strikes([min, max], 300);
    for strike in &strikes {
        assert!(
            (min..=max).contains(&strike.draw),
            "a draw stays in its range: {}",
            strike.draw
        );
        assert!(
            strike.released_at >= strike.draw && strike.released_at - DT_MS < strike.draw,
            "released on the first evaluation at or past the draw: {} vs {}",
            strike.released_at,
            strike.draw
        );
        assert_eq!(strike.attack == SLAM, strike.roll < 0.5, "the roll chose");
    }
    let span = max - min;
    for attack in [SLAM, SWEEP] {
        let draws: Vec<f32> = strikes
            .iter()
            .filter(|strike| strike.attack == attack)
            .map(|strike| strike.draw)
            .collect();
        assert!(draws.len() >= 100, "{attack}: {} strikes", draws.len());
        let low = draws.iter().copied().fold(f32::MAX, f32::min);
        let high = draws.iter().copied().fold(f32::MIN, f32::max);
        assert!(
            low < min + 0.1 * span && high > max - 0.1 * span,
            "{attack}'s release times span the whole range: [{low}, {high}]"
        );
    }
    // The draw is independent of the roll it rides beside.
    let n = strikes.len() as f32;
    let mean = |values: &dyn Fn(&Strike) -> f32| strikes.iter().map(values).sum::<f32>() / n;
    let (roll_mean, draw_mean) = (mean(&|s| s.roll), mean(&|s| s.draw));
    let covariance = mean(&|s| (s.roll - roll_mean) * (s.draw - draw_mean));
    let roll_sd = mean(&|s| (s.roll - roll_mean).powi(2)).sqrt();
    let draw_sd = mean(&|s| (s.draw - draw_mean).powi(2)).sqrt();
    let correlation = covariance / (roll_sd * draw_sd);
    assert!(
        correlation.abs() < 0.2,
        "release draws do not track the roll: r = {correlation}"
    );
}

// R1 / R2: a fixed hold never varies, and `holdMs: 0` leaves on the first
// evaluation after its hold's entry.
#[test]
fn fixed_and_zero_hold_ranges_release_on_their_bound_every_time() {
    let fixed = run_strikes([200.0, 200.0], 12);
    assert!(fixed.iter().all(|strike| strike.draw == 200.0));
    let zero = run_strikes([0.0, 0.0], 12);
    for strike in &zero {
        assert_eq!(strike.draw, 0.0);
    }

    let graph = stance_graph(vec![edge(HOLD, after_ms(48.0))], [0.0, 0.0], vec![]);
    let mut reg = EntityRegistry::new();
    let mut runtime = AiRuntime::new();
    spawn_player(&mut reg, Vec3::new(1.5, 0.0, 0.0));
    let enemy = spawn_posed(&mut reg, &graph, REST);
    step_until(&mut reg, &mut runtime, enemy, HOLD, 10);
    step(&mut reg, &mut runtime);
    assert_eq!(
        enemy_state_name(&reg, enemy),
        RELEASE,
        "`holdMs: 0` leaves on the first evaluation after entry"
    );
}

/// An enemy just entered into its hold whose turn can never be granted: a
/// hog holds the only room on the same player. The hold's first evaluation
/// is the next tick.
fn starved_hold(release_after: [f32; 2]) -> (EntityRegistry, AiRuntime, EntityId) {
    let graph = stance_graph(vec![edge(HOLD, always())], release_after, vec![]);
    let mut reg = EntityRegistry::new();
    let mut runtime = AiRuntime::new();
    set_limits(&mut runtime, &[1]);
    spawn_player(&mut reg, Vec3::new(1.5, 0.0, 0.0));
    let hog = spawn_enemy(
        &mut reg,
        Vec3::new(0.0, 0.0, 0.3),
        authored_brain(&hog_graph(), "hog"),
        500.0,
    );
    set_draws(&reg, &mut runtime, &[], hog, SLAM, &[(0, 1)]);
    step(&mut reg, &mut runtime);
    assert_eq!(held(&reg, hog), vec![SLAM]);
    let enemy = spawn_posed(&mut reg, &graph, REST);
    set_draws(&reg, &mut runtime, &[], enemy, SLAM, &[(0, 1)]);
    step(&mut reg, &mut runtime);
    assert_eq!(
        enemy_state_name(&reg, enemy),
        HOLD,
        "the rest takes its hold"
    );
    assert!(held(&reg, enemy).is_empty(), "no room: nothing granted");
    (reg, runtime, enemy)
}

// P7 / P22: the break and the missing turn on one evaluation (with the
// release due too): the break row comes first and wins.
#[test]
fn a_break_and_a_missing_turn_on_one_evaluation_leave_for_the_break() {
    let (mut reg, mut runtime, enemy) = starved_hold([0.0, 0.0]);
    hit(&mut reg, enemy, 30.0);
    step(&mut reg, &mut runtime);
    assert_eq!(enemy_state_name(&reg, enemy), FLINCH);
}

// P8 / P16: the pose's timer and the missing turn on one evaluation: the
// turn-lost row precedes the release, so the hold returns to its rest and
// the release is never entered.
#[test]
fn a_due_release_and_a_missing_turn_on_one_evaluation_return_to_the_rest() {
    let (mut reg, mut runtime, enemy) = starved_hold([0.0, 0.0]);
    assert!(
        brain(&reg, enemy).hold_release_due(0),
        "fixture: the release is due"
    );
    step(&mut reg, &mut runtime);
    assert_eq!(enemy_state_name(&reg, enemy), REST);
    for _ in 0..6 {
        step(&mut reg, &mut runtime);
        assert_ne!(
            enemy_state_name(&reg, enemy),
            RELEASE,
            "the release never runs"
        );
    }
}

// P2 / P33(a): a hold whose target switches mid-tell carries its tell on when
// the mark's re-request against the new target is granted, however far the new
// target stands, and its release may whiff.
#[test]
fn a_hold_whose_target_switches_carries_its_tell_when_the_new_target_is_granted() {
    let graph = stance_graph(vec![edge(HOLD, always())], [160.0, 160.0], vec![]);
    let mut reg = EntityRegistry::new();
    let mut runtime = AiRuntime::new();
    let near = spawn_player(&mut reg, Vec3::new(1.5, 0.0, 0.0));
    let far = spawn_player(&mut reg, Vec3::new(0.0, 0.0, 6.0));
    let enemy = spawn_posed(&mut reg, &graph, REST);
    step(&mut reg, &mut runtime);
    assert_eq!(enemy_state_name(&reg, enemy), HOLD);
    assert_eq!(enemy_acquired_target(&reg, enemy), Some(near));
    assert_eq!(held(&reg, enemy), vec![SLAM]);

    reg.despawn(near).unwrap();
    let mut states = Vec::new();
    for _ in 0..12 {
        step(&mut reg, &mut runtime);
        states.push(enemy_state_name(&reg, enemy));
        if states.last().unwrap() == RELEASE {
            break;
        }
        assert_eq!(enemy_acquired_target(&reg, enemy), Some(far));
        assert_eq!(
            held(&reg, enemy),
            vec![SLAM],
            "re-granted on the new target"
        );
    }
    assert_eq!(
        states.last().map(String::as_str),
        Some(RELEASE),
        "{states:?}"
    );
    assert!(!states.iter().any(|state| state == REST), "{states:?}");
    assert_eq!(
        player_hp(&reg, far),
        100.0,
        "the far target is out of reach: a whiff"
    );
}

// P2 / P33(b): with no room on the new target, the hold returns to its rest
// on the next evaluation, pose unbroken.
#[test]
fn a_hold_whose_new_target_has_no_room_returns_to_its_rest_unbroken() {
    let graph = stance_graph(vec![edge(HOLD, always())], NEVER, vec![]);
    let mut reg = EntityRegistry::new();
    let mut runtime = AiRuntime::new();
    set_limits(&mut runtime, &[1]);
    let near = spawn_player(&mut reg, Vec3::new(1.5, 0.0, 0.0));
    let far = spawn_player(&mut reg, Vec3::new(0.0, 0.0, 6.0));
    let hog = spawn_enemy(
        &mut reg,
        Vec3::new(0.0, 0.0, 6.3),
        authored_brain(&hog_graph(), "hog"),
        500.0,
    );
    let enemy = spawn_posed(&mut reg, &graph, REST);
    set_draws(&reg, &mut runtime, &[], hog, SLAM, &[(0, 1)]);
    set_draws(&reg, &mut runtime, &[], enemy, SLAM, &[(0, 1)]);
    step(&mut reg, &mut runtime);
    step(&mut reg, &mut runtime);
    assert_eq!(enemy_acquired_target(&reg, hog), Some(far));
    assert_eq!(held(&reg, hog), vec![SLAM]);
    assert_eq!(held(&reg, enemy), vec![SLAM], "room on the near player");
    hit(&mut reg, enemy, 10.0);
    let (time, roll, _, _) = pose_facts(&reg, enemy);

    reg.despawn(near).unwrap();
    let switched = step(&mut reg, &mut runtime);
    assert_eq!(
        enemy_state_name(&reg, enemy),
        HOLD,
        "the switch tick re-requests"
    );
    assert!(announced(&switched, &reg, enemy).is_empty());
    let returned = step(&mut reg, &mut runtime);
    assert_eq!(
        enemy_state_name(&reg, enemy),
        REST,
        "no room on the new target"
    );
    assert!(
        announced(&returned, &reg, enemy).is_empty(),
        "no stance sound"
    );
    let (now, now_roll, damage, _) = pose_facts(&reg, enemy);
    assert_near(now, time + 2.0 * DT_MS, "the pose's timer runs on");
    assert_eq!(now_roll, roll);
    assert_near(damage, 10.0, "the damage sum holds");
}

// P2 / P33(b): with no target left, the hold returns to its rest on the next
// evaluation, pose unbroken.
#[test]
fn a_hold_with_no_target_left_returns_to_its_rest_unbroken() {
    let graph = stance_graph(vec![edge(HOLD, always())], NEVER, vec![]);
    let mut reg = EntityRegistry::new();
    let mut runtime = AiRuntime::new();
    let pawn = spawn_player(&mut reg, Vec3::new(1.5, 0.0, 0.0));
    let enemy = spawn_posed(&mut reg, &graph, REST);
    step(&mut reg, &mut runtime);
    assert_eq!(enemy_state_name(&reg, enemy), HOLD);
    assert_eq!(held(&reg, enemy), vec![SLAM]);
    let (time, roll, _, _) = pose_facts(&reg, enemy);

    reg.despawn(pawn).unwrap();
    step(&mut reg, &mut runtime);
    let returned = step(&mut reg, &mut runtime);
    assert_eq!(enemy_state_name(&reg, enemy), REST);
    assert!(announced(&returned, &reg, enemy).is_empty());
    let (now, now_roll, _, _) = pose_facts(&reg, enemy);
    assert_near(now, time + 2.0 * DT_MS, "the pose's timer runs on");
    assert_eq!(now_roll, roll);
}

// P6 / P23: a strike at the root level beside a chase, under no engaging
// ancestor, keeps its target and committed aim through a co-op re-rank inside
// the switch margin and a tick out of sight; its turn holds and it releases
// on the target it took.
#[test]
fn a_root_strike_beside_a_chase_keeps_its_target_through_a_rerank_and_a_tick_out_of_sight() {
    let graph = stance_graph(vec![edge(HOLD, always())], [96.0, 96.0], vec![]);
    let mut reg = EntityRegistry::new();
    let mut runtime = AiRuntime::new();
    let open = CollisionWorld::new();
    let wall = wall_world(0.5, -1.0, 2.0);
    let first = spawn_player(&mut reg, Vec3::new(1.5, 0.0, 0.0));
    let enemy = spawn_posed(&mut reg, &graph, REST);
    step_in(&mut reg, &mut runtime, Some(&open));
    assert_eq!(enemy_state_name(&reg, enemy), HOLD);
    assert_eq!(enemy_acquired_target(&reg, enemy), Some(first));
    assert_eq!(held(&reg, enemy), vec![SLAM]);

    // Nearer, but by less than the switch margin, and never behind the wall.
    let second = spawn_player(&mut reg, Vec3::new(0.0, 0.0, 1.2));
    const { assert!(1.5 - 1.2 < TARGET_SWITCH_HYSTERESIS_DISTANCE) };
    let mut released = false;
    for tick in 0..12 {
        let world = if tick == 0 { &wall } else { &open };
        step_in(&mut reg, &mut runtime, Some(world));
        assert_eq!(
            enemy_acquired_target(&reg, enemy),
            Some(first),
            "tick {tick}: the strike keeps its target"
        );
        if enemy_state_name(&reg, enemy) == RELEASE {
            released = true;
            break;
        }
        assert_eq!(enemy_state_name(&reg, enemy), HOLD, "tick {tick}");
        assert_eq!(held(&reg, enemy), vec![SLAM], "tick {tick}: the turn holds");
    }
    assert!(released, "the strike releases");
    assert!(
        player_hp(&reg, first) < 100.0,
        "and lands on its own target"
    );
    assert_eq!(player_hp(&reg, second), 100.0);
}

// W8 and P39(b): a resting enemy keeps its target and turns its aim onto it
// through a co-op re-rank inside the switch margin and a tick out of sight,
// as a marked activity does; a player nearer by more than the margin takes
// the target, since retention is hysteresis, not a lock.
#[test]
fn a_resting_enemy_keeps_its_target_and_aim_until_a_player_is_nearer_past_the_margin() {
    let graph = stance_graph(vec![edge(HOLD, after_ms(100_000.0))], NEVER, vec![]);
    let mut reg = EntityRegistry::new();
    let mut runtime = AiRuntime::new();
    let open = CollisionWorld::new();
    let wall = wall_world(0.5, -1.0, 2.0);
    let first = spawn_player(&mut reg, Vec3::new(1.5, 0.0, 0.0));
    let enemy = spawn_posed(&mut reg, &graph, REST);
    set_enemy_yaw(&mut reg, enemy, -std::f32::consts::FRAC_PI_2);
    step_in(&mut reg, &mut runtime, Some(&open));
    assert_eq!(enemy_acquired_target(&reg, enemy), Some(first));

    let second = spawn_player(&mut reg, Vec3::new(0.0, 0.0, 1.2));
    for tick in 0..16 {
        let world = if tick == 0 { &wall } else { &open };
        step_in(&mut reg, &mut runtime, Some(world));
        assert_eq!(enemy_state_name(&reg, enemy), REST);
        assert_eq!(
            enemy_acquired_target(&reg, enemy),
            Some(first),
            "tick {tick}: the rest keeps its target"
        );
    }
    assert_faces(
        enemy_forward_xz(&reg, enemy),
        Vec3::X,
        "the rest's committed aim turns onto its target",
    );

    set_enemy_position(&mut reg, second, Vec3::new(0.0, 0.0, 0.3));
    step_in(&mut reg, &mut runtime, Some(&open));
    assert_eq!(
        enemy_acquired_target(&reg, enemy),
        Some(second),
        "nearer by more than the margin takes the target"
    );
}
