// Queued rows through the tick: a stance's rest requests its strike's turn
// while the row's `when`, range and cooldown pass, keeps the pose and its
// place in line while it waits, and strikes once the turn reads held. Graphs
// are hand-written in the lowered shape (stance lowering builds them later).
// Each test names its pin.
// See: context/lib/entity_model.md §7c (Attack budget; Stances and attacks)

use std::collections::BTreeMap;

use postretro_entities::components::brain::AttackPoolId;
use postretro_foundation::BRAIN_DAMAGE_IN_ACTIVITY_INPUT;
use serde_json::json;

use super::refresh::install_budgets;
use super::*;
use crate::ai_tests::stance_pose::{announced, hit, mesh_switches};
use crate::attack_turns::{RequestSources, attack_name, next_request};

const APPROACH: &str = "approach";
const REST: &str = "overhead";
const FLINCH: &str = "flinch";
const SLAM: &str = "slam";
const SWEEP: &str = "sweep";
/// Registered ids under [`install_budgets`]: `defaultToken`, `melee`, `heavy`.
const MELEE_ID: AttackTokenId = AttackTokenId(1);
/// A release range no test outlasts.
const NEVER: [f32; 2] = [100_000.0, 100_000.0];
/// The stance's `breakOn.damage`.
const BREAK_DAMAGE: f32 = 25.0;

fn hold(attack: &str) -> String {
    format!("{attack}_overhead")
}

fn release(attack: &str) -> String {
    format!("{attack}_release")
}

fn recover(attack: &str) -> String {
    format!("{attack}_recover")
}

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

fn always() -> IrNode {
    IrNode::Const {
        value: IrValue::Bool(true),
    }
}

fn after_ms(ms: f32) -> IrNode {
    ge(BRAIN_TIME_IN_ACTIVITY_MS_INPUT, ms)
}

/// A per-entity flag the test raises: `@state.<name>` at 1.
fn flag(name: &str) -> IrNode {
    ge(&format!("@state.{name}"), 1.0)
}

fn unflagged(name: &str) -> IrNode {
    lt(&format!("@state.{name}"), 1.0)
}

fn set_flag(reg: &mut EntityRegistry, enemy: EntityId, name: &str, on: bool) {
    reg.entity_state_mut(enemy)
        .expect("the enemy carries entity state")
        .set(name, if on { 1.0 } else { 0.0 });
}

/// A rest's row to `attack`'s hold, as lowering writes it.
fn queued(attack: &str, when: IrNode) -> GuardedRow {
    GuardedRow {
        queue: Some(attack.to_string()),
        ..edge(&hold(attack), when)
    }
}

fn posed(mut activity: BehaviorActivityDescriptor) -> BehaviorActivityDescriptor {
    activity.pose = Some(REST.to_string());
    activity
}

/// A contact strike of `max_range` and `cooldown_ms`, drawing `draws`.
fn strike(max_range: f32, cooldown_ms: f32, draws: Vec<TokenDraw>) -> AttackParams {
    AttackParams {
        max_range: Some(max_range),
        draws: Some(draws),
        ..contact(1.0, cooldown_ms)
    }
}

/// One stance, `overhead`, at the root level beside a chase, and one strike
/// from it per attack: the rest carries the break row, then `rest_rows`; each
/// strike runs hold (break, turn-lost, release) → release (fires on entry) →
/// recover → rest, every phase marked for its attack.
fn queue_graph(
    rest_rows: Vec<GuardedRow>,
    attacks: Vec<(&str, AttackParams)>,
    release_after: [f32; 2],
) -> BehaviorGraphDescriptor {
    unvalidated_queue_graph(rest_rows, attacks, release_after)
        .validate()
        .expect("the lowered shape validates")
}

/// [`queue_graph`] before validation, for a test that rebuilds around it.
fn unvalidated_queue_graph(
    rest_rows: Vec<GuardedRow>,
    attacks: Vec<(&str, AttackParams)>,
    release_after: [f32; 2],
) -> BehaviorGraphDescriptor {
    let break_row = || edge(FLINCH, ge(BRAIN_DAMAGE_IN_ACTIVITY_INPUT, BREAK_DAMAGE));
    let mut activity_map = BTreeMap::from([
        (
            APPROACH.to_string(),
            authored_state("walk", MotionVerb::ChaseTarget, None),
        ),
        (
            REST.to_string(),
            posed(authored_state("attack", MotionVerb::Hold, None)),
        ),
        (
            FLINCH.to_string(),
            authored_state("idle", MotionVerb::Hold, None),
        ),
    ]);
    let mut rest = vec![break_row()];
    rest.extend(rest_rows);
    let mut transition_map = BTreeMap::from([
        (APPROACH.to_string(), vec![edge(REST, unflagged("leave"))]),
        (REST.to_string(), rest),
        (FLINCH.to_string(), vec![edge(REST, after_ms(450.0))]),
    ]);
    for (attack, _) in &attacks {
        activity_map.insert(
            hold(attack),
            marked(
                posed(authored_state("attack", MotionVerb::Hold, None)),
                attack,
            ),
        );
        activity_map.insert(release(attack), marked(striking(attack), attack));
        activity_map.insert(
            recover(attack),
            marked(authored_state("idle", MotionVerb::Hold, None), attack),
        );
        transition_map.insert(
            hold(attack),
            vec![
                break_row(),
                edge(
                    REST,
                    IrNode::Not {
                        x: Box::new(brain_input(&format!("@attackTurnHeld.{attack}"))),
                    },
                ),
                GuardedRow {
                    release_after: Some(release_after),
                    ..edge(&release(attack), always())
                },
            ],
        );
        transition_map.insert(
            release(attack),
            vec![edge(&recover(attack), after_ms(48.0))],
        );
        transition_map.insert(recover(attack), vec![edge(REST, after_ms(48.0))]);
    }
    test_behavior_graph!({
        initial: APPROACH.to_string(),
        activities: activity_map,
        transitions: transition_map,
        candidate_filter: None,
        retaliation: None,
        patrol: None,
        attacks: attacks
            .into_iter()
            .map(|(name, attack)| (name.to_string(), attack))
            .collect(),
        engagement_radius: None,
        move_speed: TEST_MOVE_SPEED,
    })
}

/// An enemy that holds `attack`'s turn, drawing `draws`, until it is moved
/// to `idle`.
fn hog_graph(attack: &str, draws: Vec<TokenDraw>) -> BehaviorGraphDescriptor {
    marked_graph(vec![(attack, held_attack(draws))])
}

fn spawn_resting(
    reg: &mut EntityRegistry,
    index: usize,
    graph: &BehaviorGraphDescriptor,
) -> EntityId {
    spawn_at(reg, index, graph, REST)
}

fn brain(reg: &EntityRegistry, enemy: EntityId) -> BrainComponent {
    reg.get_component::<BrainComponent>(enemy).unwrap().clone()
}

/// The targets `enemy` holds `attack`'s turn against.
fn held_on(reg: &EntityRegistry, enemy: EntityId, attack: &str) -> Vec<EntityId> {
    let brain = brain(reg, enemy);
    brain
        .attack_turns
        .held()
        .filter(|turn| attack_name(&brain.graph, turn.attack) == Some(attack))
        .map(|turn| turn.target)
        .collect()
}

fn set_cooldown(reg: &mut EntityRegistry, enemy: EntityId, attack: &str, ms: f32) {
    let mut brain = brain(reg, enemy);
    brain
        .attack_cooldown_remaining_ms
        .insert(attack.to_string(), ms);
    reg.set_component(enemy, brain).unwrap();
}

fn cooldown(reg: &EntityRegistry, enemy: EntityId, attack: &str) -> f32 {
    brain(reg, enemy)
        .attack_cooldown_remaining_ms
        .get(attack)
        .copied()
        .unwrap_or(0.0)
}

fn melee_refreshing(runtime: &AiRuntime, target: EntityId) -> u32 {
    runtime
        .turns
        .refreshing_now((AttackPoolId::Default, target), MELEE_ID)
}

/// `melee` and `heavy` at (limit, refreshMs).
fn budgets(reg: &mut EntityRegistry, melee: (u32, f32), heavy: (u32, f32)) {
    install_budgets(reg, melee, heavy, json!({}));
}

// W1: a queued row requests only while its `when`, its attack's range and
// its cooldown pass. With the target a metre past `maxRange` the rest requests
// nothing and keeps the pose; stepping into range, it requests, is granted,
// and takes the hold on the next evaluation.
#[test]
fn a_rest_requests_nothing_with_its_target_out_of_range() {
    let graph = queue_graph(
        vec![queued(SLAM, always())],
        vec![(SLAM, strike(2.0, 1.0, vec![token("melee")]))],
        NEVER,
    );
    let mut reg = EntityRegistry::new();
    budgets(&mut reg, (3, 0.0), (1, 0.0));
    let mut runtime = AiRuntime::deciding_every_tick();
    let pawn = spawn_player(&mut reg, Vec3::new(3.0, 0.0, 0.0));
    let enemy = spawn_resting(&mut reg, 0, &graph);
    for tick_index in 0..8 {
        tick(&mut reg, &mut runtime);
        assert_eq!(state(&reg, enemy), REST, "tick {tick_index}");
        assert_eq!(enemy_acquired_target(&reg, enemy), Some(pawn));
        assert_eq!(requested(&reg, enemy), None, "tick {tick_index}");
        assert!(held(&reg, enemy).is_empty(), "tick {tick_index}");
        assert_eq!(wait_since(&reg, enemy), None, "no place in line taken");
    }

    set_enemy_position(&mut reg, pawn, Vec3::new(1.8, 0.0, 0.0));
    tick(&mut reg, &mut runtime);
    assert_eq!(state(&reg, enemy), REST);
    assert_eq!(
        held(&reg, enemy),
        vec![SLAM],
        "in range, requested and granted"
    );
    tick(&mut reg, &mut runtime);
    assert_eq!(state(&reg, enemy), hold(SLAM));
}

// W1: with its `when` false the rest requests nothing; the tick it turns
// true it requests and is granted.
#[test]
fn a_rest_requests_nothing_while_its_rows_when_is_false() {
    let graph = queue_graph(
        vec![queued(SLAM, flag("go"))],
        vec![(SLAM, strike(2.0, 1.0, vec![token("melee")]))],
        NEVER,
    );
    let mut reg = EntityRegistry::new();
    budgets(&mut reg, (3, 0.0), (1, 0.0));
    let mut runtime = AiRuntime::deciding_every_tick();
    spawn_player(&mut reg, player_spot());
    let enemy = spawn_resting(&mut reg, 0, &graph);
    for tick_index in 0..8 {
        tick(&mut reg, &mut runtime);
        assert_eq!(state(&reg, enemy), REST, "tick {tick_index}");
        assert_eq!(requested(&reg, enemy), None, "tick {tick_index}");
        assert!(held(&reg, enemy).is_empty(), "tick {tick_index}");
    }

    set_flag(&mut reg, enemy, "go", true);
    tick(&mut reg, &mut runtime);
    assert_eq!(held(&reg, enemy), vec![SLAM]);
    tick(&mut reg, &mut runtime);
    assert_eq!(state(&reg, enemy), hold(SLAM));
}

// W1: an attack declaring no `maxRange` (a weapon attack) adds no range
// gate: a rest whose target stands far off still requests, and takes the hold
// once granted.
#[test]
fn an_attack_with_no_max_range_adds_no_range_gate() {
    let shoot = AttackParams {
        draws: Some(vec![token("melee")]),
        ..weapon("rifle")
    };
    assert_eq!(shoot.max_range, None);
    let graph = queue_graph(vec![queued(SLAM, always())], vec![(SLAM, shoot)], NEVER);
    let mut reg = EntityRegistry::new();
    budgets(&mut reg, (3, 0.0), (1, 0.0));
    let mut runtime = AiRuntime::deciding_every_tick();
    spawn_player(&mut reg, Vec3::new(9.0, 0.0, 0.0));
    let enemy = spawn_resting(&mut reg, 0, &graph);
    let mut entered = false;
    for _ in 0..16 {
        tick(&mut reg, &mut runtime);
        if state(&reg, enemy) == hold(SLAM) {
            entered = true;
            break;
        }
    }
    assert!(entered, "the far target is struck at: no range gate");
}

// W2 / P26(a): a rest's grant on tick N is not released in apply as an
// unmarked grant is; it survives into N+1, where its row passes, and the
// hold's mark keeps it. Nothing refreshes.
#[test]
fn a_rests_grant_survives_into_the_next_evaluation_and_the_hold_keeps_it() {
    let graph = queue_graph(
        vec![queued(SLAM, always())],
        vec![(SLAM, strike(2.0, 1.0, vec![token("melee")]))],
        NEVER,
    );
    let mut reg = EntityRegistry::new();
    budgets(&mut reg, (3, 1000.0), (1, 0.0));
    let mut runtime = AiRuntime::deciding_every_tick();
    let pawn = spawn_player(&mut reg, player_spot());
    let enemy = spawn_resting(&mut reg, 0, &graph);

    tick(&mut reg, &mut runtime);
    assert_eq!(state(&reg, enemy), REST, "granted on N, still resting");
    assert_eq!(
        held_on(&reg, enemy, SLAM),
        vec![pawn],
        "the grant survives apply"
    );
    assert_eq!(requested(&reg, enemy), None);

    tick(&mut reg, &mut runtime);
    assert_eq!(state(&reg, enemy), hold(SLAM), "N+1: the row passes");
    assert_eq!(held_on(&reg, enemy, SLAM), vec![pawn]);
    for _ in 0..4 {
        tick(&mut reg, &mut runtime);
        assert_eq!(state(&reg, enemy), hold(SLAM));
        assert_eq!(held_on(&reg, enemy, SLAM), vec![pawn], "the mark keeps it");
    }
    assert_eq!(melee_refreshing(&runtime, pawn), 0, "no turn ended");
}

// W2 / P26(b): when N+1 takes another row instead, the turn ends there and
// its refresh starts; nothing fired.
#[test]
fn a_rests_grant_ends_with_its_refresh_when_the_next_evaluation_takes_another_row() {
    let graph = queue_graph(
        vec![edge(APPROACH, flag("leave")), queued(SLAM, always())],
        vec![(SLAM, strike(2.0, 1.0, vec![token("melee")]))],
        NEVER,
    );
    let mut reg = EntityRegistry::new();
    budgets(&mut reg, (3, 1000.0), (1, 0.0));
    let mut runtime = AiRuntime::deciding_every_tick();
    let pawn = spawn_player(&mut reg, player_spot());
    let enemy = spawn_resting(&mut reg, 0, &graph);

    tick(&mut reg, &mut runtime);
    assert_eq!(held_on(&reg, enemy, SLAM), vec![pawn], "granted on N");
    set_flag(&mut reg, enemy, "leave", true);
    tick(&mut reg, &mut runtime);
    assert_eq!(state(&reg, enemy), APPROACH, "N+1 takes the earlier row");
    assert!(held(&reg, enemy).is_empty(), "the turn ends on N+1");
    assert_eq!(
        melee_refreshing(&runtime, pawn),
        1,
        "and its refresh starts"
    );
    assert_eq!(player_hp(&reg, pawn), 100.0, "nothing fired");
}

// W3 / P27: between the grant and the hold, a co-op re-rank inside the
// target-switch margin does not switch the resting enemy's target, so the
// grant is not wasted: the hold is entered on it and the turn survives.
#[test]
fn a_rerank_inside_the_margin_between_grant_and_hold_keeps_the_target_and_the_grant() {
    let graph = queue_graph(
        vec![queued(SLAM, always())],
        vec![(SLAM, strike(2.0, 1.0, vec![token("melee")]))],
        NEVER,
    );
    let mut reg = EntityRegistry::new();
    budgets(&mut reg, (3, 1000.0), (1, 0.0));
    let mut runtime = AiRuntime::deciding_every_tick();
    let first = spawn_player(&mut reg, Vec3::new(1.5, 0.0, 0.0));
    let enemy = spawn_at(&mut reg, 0, &graph, REST);
    set_enemy_position(&mut reg, enemy, Vec3::ZERO);
    tick(&mut reg, &mut runtime);
    assert_eq!(held_on(&reg, enemy, SLAM), vec![first], "granted on N");

    let second = spawn_player(&mut reg, Vec3::new(0.0, 0.0, 1.2));
    const { assert!(1.5 - 1.2 < TARGET_SWITCH_HYSTERESIS_DISTANCE) };
    tick(&mut reg, &mut runtime);
    assert_eq!(enemy_acquired_target(&reg, enemy), Some(first));
    assert_eq!(state(&reg, enemy), hold(SLAM), "the hold is entered");
    assert_eq!(held_on(&reg, enemy, SLAM), vec![first], "the turn survives");
    assert_eq!(melee_refreshing(&runtime, first), 0);
    assert_ne!(enemy_acquired_target(&reg, enemy), Some(second));
}

// W3 / P39(a): a player nearer by more than the margin takes the target on
// N+1. The hold is still entered on the turn read held, the old turn ends
// with its refresh on the old target, and the hold's mark re-requests
// against the new one.
#[test]
fn a_switch_past_the_margin_between_grant_and_hold_moves_the_turn_to_the_new_target() {
    let graph = queue_graph(
        vec![queued(SLAM, always())],
        vec![(SLAM, strike(2.0, 1.0, vec![token("melee")]))],
        NEVER,
    );
    let mut reg = EntityRegistry::new();
    budgets(&mut reg, (3, 1000.0), (1, 0.0));
    let mut runtime = AiRuntime::deciding_every_tick();
    let first = spawn_player(&mut reg, Vec3::new(1.5, 0.0, 0.0));
    let enemy = spawn_at(&mut reg, 0, &graph, REST);
    set_enemy_position(&mut reg, enemy, Vec3::ZERO);
    tick(&mut reg, &mut runtime);
    assert_eq!(held_on(&reg, enemy, SLAM), vec![first], "granted on N");

    let second = spawn_player(&mut reg, Vec3::new(0.0, 0.0, 0.3));
    tick(&mut reg, &mut runtime);
    assert_eq!(enemy_acquired_target(&reg, enemy), Some(second));
    assert_eq!(
        state(&reg, enemy),
        hold(SLAM),
        "entered on the turn read held"
    );
    assert_eq!(
        melee_refreshing(&runtime, first),
        1,
        "refresh on the old target"
    );
    assert_eq!(
        held_on(&reg, enemy, SLAM),
        vec![second],
        "the mark's re-request is granted on the new target"
    );
}

// W4 / P28: starvation. Limit 3 on `melee`; two sweepers keep two `melee`
// turns held and re-request one tick after each ends; a brute resting in
// `overhead`, slam first (`melee` cost 2 plus `heavy`), in a later wait slot,
// queues from its rest. While a hog holds `heavy` the slam cannot be granted,
// yet as head of line it reserves its `melee` cost, so a sweeper that ends
// cannot take that room back; once `heavy` frees the slam is granted.
// Under the old turn-free gate with no request the brute joined the line only
// on a tick `attackTurnFree` already read true, so a sweeper re-requesting on
// the same tick with an earlier slot won every tie and the slam waited
// forever.
#[test]
fn a_costly_queued_strike_reserves_its_tokens_as_head_of_line_and_is_never_starved() {
    let sweeper = graph(
        "sweep_act",
        vec![
            ("sweep_act", marked(striking(SWEEP), SWEEP)),
            ("idle", idle()),
        ],
        vec![
            ("sweep_act", vec![edge("idle", after_ms(160.0))]),
            ("idle", vec![edge("sweep_act", after_ms(16.0))]),
        ],
        vec![(SWEEP, drawing(contact(0.0, 0.0), vec![token("melee")]))],
    );
    let brute = queue_graph(
        vec![queued(SLAM, flag("ready"))],
        vec![(
            SLAM,
            strike(2.0, 1.0, vec![costing("melee", 2), token("heavy")]),
        )],
        NEVER,
    );
    let mut reg = EntityRegistry::new();
    budgets(&mut reg, (3, 0.0), (1, 0.0));
    let mut runtime = AiRuntime::deciding_every_tick();
    spawn_player(&mut reg, player_spot());
    let sweepers = [
        spawn_at(&mut reg, 0, &sweeper, "sweep_act"),
        spawn_at(&mut reg, 1, &sweeper, "sweep_act"),
    ];
    let hog = spawn_at(
        &mut reg,
        2,
        &hog_graph("heavy", vec![token("heavy")]),
        "heavy_act",
    );
    let brute_id = spawn_resting(&mut reg, 3, &brute);
    for _ in 0..3 {
        tick(&mut reg, &mut runtime);
    }
    assert_eq!(holders(&reg, &sweepers), 2, "both sweepers hold");
    assert_eq!(held(&reg, hog), vec!["heavy"]);

    set_flag(&mut reg, brute_id, "ready", true);
    tick(&mut reg, &mut runtime);
    assert_eq!(requested(&reg, brute_id).as_deref(), Some(SLAM));
    let brute_since = wait_since(&reg, brute_id).expect("the brute waits in line");

    // Through a whole sweeper turnover the brute stays head of line: each
    // sweeper that ends re-requests, and none is granted the room the slam
    // reserves.
    let mut refused_sweeper = false;
    for _ in 0..30 {
        tick(&mut reg, &mut runtime);
        assert_eq!(state(&reg, brute_id), REST);
        assert!(held(&reg, brute_id).is_empty(), "`heavy` is held");
        assert_eq!(wait_since(&reg, brute_id), Some(brute_since));
        for &sweeper in &sweepers {
            if requested(&reg, sweeper).as_deref() == Some(SWEEP) {
                refused_sweeper = true;
                assert!(
                    wait_since(&reg, sweeper) > Some(brute_since),
                    "a sweeper re-requests behind the brute"
                );
            }
        }
        assert!(
            holders(&reg, &sweepers) <= 1 || !refused_sweeper,
            "a refused sweeper never retakes the room"
        );
    }
    assert!(refused_sweeper, "a sweeper re-requested behind the slam");
    assert!(holders(&reg, &sweepers) < 2, "the reserved room stays free");

    enter(&mut reg, hog, "idle");
    tick(&mut reg, &mut runtime);
    assert_eq!(held(&reg, brute_id), vec![SLAM], "granted once room frees");
    tick(&mut reg, &mut runtime);
    assert_eq!(state(&reg, brute_id), hold(SLAM));
}

// W5 / P29: one request per enemy, the first open row's; when that row's
// `when` stops passing the request moves to the next row's attack and keeps
// the enemy's place in line, and so does leaving the rest and re-entering it
// on the same target: once room frees, the brute is granted ahead of a
// waiter that queued after it.
#[test]
fn a_rest_keeps_its_place_in_line_across_a_change_of_attack_and_a_re_entry() {
    let graph = queue_graph(
        vec![
            edge(APPROACH, flag("leave")),
            queued(SLAM, unflagged("swap")),
            queued(SWEEP, always()),
        ],
        vec![
            (SLAM, strike(2.0, 1.0, vec![token("melee")])),
            (SWEEP, strike(2.0, 1.0, vec![token("melee")])),
        ],
        NEVER,
    );
    let late = hog_graph(SLAM, vec![token("melee")]);
    let mut reg = EntityRegistry::new();
    budgets(&mut reg, (1, 0.0), (1, 0.0));
    let mut runtime = AiRuntime::deciding_every_tick();
    spawn_player(&mut reg, player_spot());
    let hog = spawn_at(
        &mut reg,
        0,
        &hog_graph(SLAM, vec![token("melee")]),
        "slam_act",
    );
    let brute = spawn_resting(&mut reg, 1, &graph);
    let later = spawn_at(&mut reg, 2, &late, "idle");

    tick(&mut reg, &mut runtime);
    assert_eq!(held(&reg, hog), vec![SLAM]);
    assert_eq!(
        requested(&reg, brute).as_deref(),
        Some(SLAM),
        "both rows pass: the first requests"
    );
    let since = wait_since(&reg, brute).unwrap();
    enter(&mut reg, later, "slam_act");
    tick(&mut reg, &mut runtime);
    assert!(wait_since(&reg, later) > Some(since), "a later waiter");

    set_flag(&mut reg, brute, "swap", true);
    tick(&mut reg, &mut runtime);
    assert_eq!(
        requested(&reg, brute).as_deref(),
        Some(SWEEP),
        "the request moves"
    );
    assert_eq!(wait_since(&reg, brute), Some(since), "its place is kept");

    set_flag(&mut reg, brute, "leave", true);
    tick(&mut reg, &mut runtime);
    assert_eq!(state(&reg, brute), APPROACH);
    assert_eq!(
        requested(&reg, brute),
        None,
        "away from the rest, no request"
    );
    tick(&mut reg, &mut runtime);
    set_flag(&mut reg, brute, "leave", false);
    tick(&mut reg, &mut runtime);
    assert_eq!(state(&reg, brute), REST, "back in the rest");
    tick(&mut reg, &mut runtime);
    assert_eq!(requested(&reg, brute).as_deref(), Some(SWEEP));
    assert_eq!(
        wait_since(&reg, brute),
        Some(since),
        "re-entry keeps its place"
    );

    enter(&mut reg, hog, "idle");
    tick(&mut reg, &mut runtime);
    assert_eq!(
        held(&reg, brute),
        vec![SWEEP],
        "granted ahead of the later waiter"
    );
    assert!(held(&reg, later).is_empty());
}

// W6 / P31 and S25 / P49(b): with `slam` first and its `heavy` refreshing, a
// brute resting in `overhead` keeps requesting `slam`, never `sweep`, until
// `slam`'s `when` stops passing; a token refresh does not make a row
// ineligible to request.
#[test]
fn a_first_row_whose_token_refreshes_keeps_the_request() {
    let graph = queue_graph(
        vec![queued(SLAM, unflagged("swap")), queued(SWEEP, always())],
        vec![
            (SLAM, strike(2.0, 1.0, vec![token("melee"), token("heavy")])),
            (SWEEP, strike(2.0, 1.0, vec![token("melee")])),
        ],
        NEVER,
    );
    let mut reg = EntityRegistry::new();
    budgets(&mut reg, (3, 0.0), (1, 4000.0));
    let mut runtime = AiRuntime::deciding_every_tick();
    let pawn = spawn_player(&mut reg, player_spot());
    let hog = spawn_at(
        &mut reg,
        0,
        &hog_graph("heavy", vec![token("heavy")]),
        "heavy_act",
    );
    let brute = spawn_resting(&mut reg, 1, &graph);
    tick(&mut reg, &mut runtime);
    assert_eq!(held(&reg, hog), vec!["heavy"]);
    enter(&mut reg, hog, "idle");
    tick(&mut reg, &mut runtime);
    assert!(held(&reg, hog).is_empty());
    assert_eq!(
        runtime
            .turns
            .refreshing_now((AttackPoolId::Default, pawn), AttackTokenId(2)),
        1,
        "`heavy` refreshes"
    );

    for tick_index in 0..40 {
        tick(&mut reg, &mut runtime);
        assert_eq!(state(&reg, brute), REST, "tick {tick_index}");
        assert_eq!(
            requested(&reg, brute).as_deref(),
            Some(SLAM),
            "tick {tick_index}: keeps requesting the first row's attack"
        );
        assert!(held(&reg, brute).is_empty(), "tick {tick_index}");
    }

    set_flag(&mut reg, brute, "swap", true);
    tick(&mut reg, &mut runtime);
    assert_eq!(held(&reg, brute), vec![SWEEP], "only now the second row");
}

// W7: one request per enemy, by precedence: a marked activity's request
// outranks a rest's queued row, which outranks a pending unmarked attack. A
// queued request takes a mark's lifetime (`marked`), a pending one does not.
#[test]
fn a_marks_request_outranks_a_rests_which_outranks_a_pending_unmarked_attack() {
    let graph = queue_graph(
        vec![],
        vec![
            (SLAM, strike(2.0, 1.0, vec![token("melee")])),
            (SWEEP, strike(2.0, 1.0, vec![token("heavy")])),
            ("jab", contact(1.0, 1.0)),
        ],
        NEVER,
    );
    let mut reg = EntityRegistry::new();
    budgets(&mut reg, (3, 0.0), (1, 0.0));
    let mut runtime = AiRuntime::deciding_every_tick();
    let pawn = spawn_player(&mut reg, player_spot());
    let enemy = spawn_resting(&mut reg, 0, &graph);
    runtime
        .programs
        .sync(&reg, &[], 0, &mut std::collections::HashSet::new());
    let brain = brain(&reg, enemy);
    let index = |name| attack_index(&brain.graph, name).unwrap();
    let pending = PendingAttack {
        attack_name: "jab".to_string(),
        cooldown_ms: 0.0,
        effect: AttackOutcome::Contact {
            damage: 1.0,
            damage_type: None,
        },
    };
    let request = |mark, queued, pending| {
        next_request(
            &AttackTurnState::default(),
            &brain.graph,
            RequestSources {
                mark,
                queued,
                pending,
            },
            runtime.programs.get(enemy),
            pawn,
            0,
            AttackPoolId::Default,
        )
        .map(|(request, _)| {
            (
                attack_name(&brain.graph, request.attack).unwrap(),
                request.marked,
            )
        })
    };
    let (slam, sweep) = (Some(index(SLAM)), Some(index(SWEEP)));
    assert_eq!(request(slam, sweep, Some(&pending)), Some((SLAM, true)));
    assert_eq!(request(None, sweep, Some(&pending)), Some((SWEEP, true)));
    assert_eq!(request(None, None, Some(&pending)), Some(("jab", false)));
    assert_eq!(request(None, sweep, None), Some((SWEEP, true)));
}

// W9 / P24: two enemies resting in one shared stance, queued under one
// `melee` token. Both request on N; the first is granted and strikes. The
// other keeps resting with its request standing, its pose unbroken (no clip
// restart, no second stance sound) and its place in line kept, and is
// granted once the first's turn ends, ahead of the striker's next request.
#[test]
fn two_enemies_in_one_shared_stance_take_turns_and_the_waiter_keeps_its_pose() {
    let graph = queue_graph(
        vec![queued(SLAM, always())],
        vec![(SLAM, strike(2.0, 300.0, vec![token("melee")]))],
        [0.0, 0.0],
    );
    let mut reg = EntityRegistry::new();
    budgets(&mut reg, (1, 0.0), (1, 0.0));
    let mut runtime = AiRuntime::deciding_every_tick();
    let pawn = spawn_player(&mut reg, player_spot());
    let first = spawn_resting(&mut reg, 0, &graph);
    let second = spawn_resting(&mut reg, 1, &graph);

    tick(&mut reg, &mut runtime);
    assert_eq!(held(&reg, first), vec![SLAM], "the first is granted on N");
    assert_eq!(requested(&reg, second).as_deref(), Some(SLAM));
    let since = wait_since(&reg, second).unwrap();
    let switches = mesh_switches(&reg, second);

    let mut strike_states = Vec::new();
    loop {
        let result = tick_with(&mut reg, &mut runtime, &[]);
        strike_states.push(state(&reg, first));
        assert!(
            announced(&result, &reg, second).is_empty(),
            "no second stance sound"
        );
        if !held(&reg, second).is_empty() {
            break;
        }
        assert_eq!(state(&reg, second), REST, "the waiter keeps the pose");
        assert_eq!(requested(&reg, second).as_deref(), Some(SLAM));
        assert_eq!(wait_since(&reg, second), Some(since));
        assert_eq!(mesh_switches(&reg, second), switches, "no clip restarts");
        assert!(strike_states.len() < 40, "never granted: {strike_states:?}");
    }
    assert_eq!(strike_states[0], hold(SLAM), "the first strikes on N+1");
    assert!(strike_states.contains(&release(SLAM)), "{strike_states:?}");
    assert_eq!(player_hp(&reg, pawn), 99.0, "one strike landed");
    assert_eq!(
        strike_states.last().map(String::as_str),
        Some(REST),
        "granted the tick the first's chain lands in its rest"
    );
    assert!(held(&reg, first).is_empty());
    tick(&mut reg, &mut runtime);
    assert_eq!(
        state(&reg, second),
        hold(SLAM),
        "the waiter strikes in turn"
    );
}

// W10 / P36: a rest's grant on N whose next evaluation takes no row (target
// out of range, `when` false, or the first row now naming another attack)
// ends the turn on N+1 and starts its refresh; the enemy's next request
// queues behind a waiter that was already in line.
#[test]
fn a_rests_grant_whose_next_evaluation_takes_no_row_ends_and_requeues_behind_earlier_waiters() {
    #[derive(Debug, Clone, Copy)]
    enum Lapse {
        OutOfRange,
        WhenFalse,
        AnotherAttackFirst,
    }
    for lapse in [
        Lapse::OutOfRange,
        Lapse::WhenFalse,
        Lapse::AnotherAttackFirst,
    ] {
        let mut rows = vec![queued(SLAM, unflagged("calm"))];
        if matches!(lapse, Lapse::AnotherAttackFirst) {
            rows.push(queued(SWEEP, always()));
        }
        let graph = queue_graph(
            rows,
            vec![
                (SLAM, strike(2.0, 1.0, vec![token("melee")])),
                (SWEEP, strike(2.0, 1.0, vec![token("melee")])),
            ],
            NEVER,
        );
        let mut reg = EntityRegistry::new();
        budgets(&mut reg, (1, 160.0), (1, 0.0));
        let mut runtime = AiRuntime::deciding_every_tick();
        let pawn = spawn_player(&mut reg, player_spot());
        let brute = spawn_resting(&mut reg, 0, &graph);
        let waiter = spawn_at(
            &mut reg,
            1,
            &hog_graph(SLAM, vec![token("melee")]),
            "slam_act",
        );

        tick(&mut reg, &mut runtime);
        assert_eq!(held(&reg, brute), vec![SLAM], "{lapse:?}: granted on N");
        assert!(
            wait_since(&reg, waiter).is_some(),
            "{lapse:?}: a waiter in line"
        );
        match lapse {
            Lapse::OutOfRange => set_enemy_position(&mut reg, pawn, Vec3::new(2.6, 0.0, 0.0)),
            Lapse::WhenFalse | Lapse::AnotherAttackFirst => set_flag(&mut reg, brute, "calm", true),
        }
        tick(&mut reg, &mut runtime);
        assert_eq!(state(&reg, brute), REST, "{lapse:?}: no row on N+1");
        assert!(held(&reg, brute).is_empty(), "{lapse:?}: the turn ends");
        assert_eq!(
            melee_refreshing(&runtime, pawn),
            1,
            "{lapse:?}: refresh starts"
        );
        match lapse {
            Lapse::AnotherAttackFirst => {
                assert_eq!(requested(&reg, brute).as_deref(), Some(SWEEP), "{lapse:?}");
            }
            Lapse::OutOfRange => {
                assert_eq!(requested(&reg, brute), None, "{lapse:?}");
                set_enemy_position(&mut reg, pawn, player_spot());
            }
            Lapse::WhenFalse => {
                assert_eq!(requested(&reg, brute), None, "{lapse:?}");
                set_flag(&mut reg, brute, "calm", false);
            }
        }
        let mut granted_waiter = false;
        for _ in 0..30 {
            tick(&mut reg, &mut runtime);
            assert!(
                held(&reg, brute).is_empty(),
                "{lapse:?}: queued behind the waiter"
            );
            if !held(&reg, waiter).is_empty() {
                granted_waiter = true;
                break;
            }
        }
        assert!(
            granted_waiter,
            "{lapse:?}: the earlier waiter is granted first"
        );
        assert!(requested(&reg, brute).is_some(), "{lapse:?}: still in line");
    }
}

// W11 / P35: when a granted strike row and the stance's `breakOn` pass on one
// evaluation in the rest, the break wins; the turn ends and its refresh
// starts, and nothing fires.
#[test]
fn a_break_beats_a_granted_strike_row_and_ends_the_turn() {
    let graph = queue_graph(
        vec![queued(SLAM, always())],
        vec![(SLAM, strike(2.0, 1.0, vec![token("melee")]))],
        [0.0, 0.0],
    );
    let mut reg = EntityRegistry::new();
    budgets(&mut reg, (3, 1000.0), (1, 0.0));
    let mut runtime = AiRuntime::deciding_every_tick();
    let pawn = spawn_player(&mut reg, player_spot());
    let enemy = spawn_resting(&mut reg, 0, &graph);
    tick(&mut reg, &mut runtime);
    assert_eq!(held(&reg, enemy), vec![SLAM], "granted on N");

    hit(&mut reg, enemy, BREAK_DAMAGE + 5.0);
    tick(&mut reg, &mut runtime);
    assert_eq!(state(&reg, enemy), FLINCH, "the break wins");
    assert!(held(&reg, enemy).is_empty(), "the turn ends");
    assert_eq!(melee_refreshing(&runtime, pawn), 1, "its refresh starts");
    for _ in 0..4 {
        tick(&mut reg, &mut runtime);
    }
    assert_eq!(player_hp(&reg, pawn), 100.0, "nothing fired");
}

// S24 / P44: an attack re-struck from its rest while its own cooldown runs
// requests nothing and keeps the pose, and its row never passes. Once the
// cooldown elapses the rest requests, is granted, and the strike fires.
#[test]
fn a_rest_waits_out_its_attacks_cooldown_before_requesting() {
    let graph = queue_graph(
        vec![queued(SLAM, always())],
        vec![(SLAM, strike(2.0, 400.0, vec![token("melee")]))],
        [0.0, 0.0],
    );
    let mut reg = EntityRegistry::new();
    budgets(&mut reg, (3, 0.0), (1, 0.0));
    let mut runtime = AiRuntime::deciding_every_tick();
    let pawn = spawn_player(&mut reg, player_spot());
    let enemy = spawn_resting(&mut reg, 0, &graph);

    let mut ticks = 0;
    while state(&reg, enemy) != recover(SLAM) {
        tick(&mut reg, &mut runtime);
        ticks += 1;
        assert!(ticks < 20, "the first strike never recovered");
    }
    assert_eq!(player_hp(&reg, pawn), 99.0, "the first strike landed");
    while state(&reg, enemy) != REST {
        tick(&mut reg, &mut runtime);
    }
    assert!(
        cooldown(&reg, enemy, SLAM) > 0.0,
        "back in the rest, cooling down"
    );

    let mut cooling_ticks = 0;
    while cooldown(&reg, enemy, SLAM) > 0.0 {
        assert_eq!(
            state(&reg, enemy),
            REST,
            "the row never passes while cooling"
        );
        assert_eq!(
            requested(&reg, enemy),
            None,
            "nothing requested while cooling"
        );
        assert!(held(&reg, enemy).is_empty());
        tick(&mut reg, &mut runtime);
        cooling_ticks += 1;
    }
    assert!(cooling_ticks > 3, "{cooling_ticks}");
    assert_eq!(state(&reg, enemy), REST);
    assert_eq!(
        held(&reg, enemy),
        vec![SLAM],
        "requested and granted once elapsed"
    );
    while state(&reg, enemy) != recover(SLAM) {
        tick(&mut reg, &mut runtime);
    }
    assert_eq!(player_hp(&reg, pawn), 98.0, "the second strike fires");
}

// S25 / P49(a): with the first row's attack cooling down and the second's
// ready, the rest requests the second; once the first's cooldown elapses it
// requests the first instead, keeping its wait clock.
#[test]
fn a_first_row_cooling_down_hands_the_request_to_the_next_and_takes_it_back() {
    let graph = queue_graph(
        vec![queued(SLAM, always()), queued(SWEEP, always())],
        vec![
            (SLAM, strike(2.0, 1.0, vec![token("melee")])),
            (SWEEP, strike(2.0, 1.0, vec![token("melee")])),
        ],
        NEVER,
    );
    let mut reg = EntityRegistry::new();
    budgets(&mut reg, (1, 0.0), (1, 0.0));
    let mut runtime = AiRuntime::deciding_every_tick();
    spawn_player(&mut reg, player_spot());
    let hog = spawn_at(
        &mut reg,
        0,
        &hog_graph(SLAM, vec![token("melee")]),
        "slam_act",
    );
    let brute = spawn_resting(&mut reg, 1, &graph);
    set_cooldown(&mut reg, brute, SLAM, 160.0);

    tick(&mut reg, &mut runtime);
    assert_eq!(held(&reg, hog), vec![SLAM]);
    assert_eq!(
        requested(&reg, brute).as_deref(),
        Some(SWEEP),
        "slam cools down"
    );
    let since = wait_since(&reg, brute).unwrap();
    while cooldown(&reg, brute, SLAM) > 0.0 {
        tick(&mut reg, &mut runtime);
        if cooldown(&reg, brute, SLAM) > 0.0 {
            assert_eq!(requested(&reg, brute).as_deref(), Some(SWEEP));
        }
    }
    assert_eq!(
        requested(&reg, brute).as_deref(),
        Some(SLAM),
        "slam is ready again"
    );
    assert_eq!(
        wait_since(&reg, brute),
        Some(since),
        "its wait clock is kept"
    );
}

// S25 / P50: a cooldown that elapses during tick N's evaluation (10 ms left
// at its start, a 16 ms tick) lets the rest request and be granted on N; its
// row passes on N+1. On N−1 it requests nothing. The gate reads the targeted
// attack's own cooldown: the rest has no action, so the path's
// `attackCooldownMs` reads 0 throughout.
#[test]
fn a_cooldown_elapsing_during_a_ticks_evaluation_requests_and_is_granted_that_tick() {
    let graph = queue_graph(
        vec![queued(SLAM, always())],
        vec![(SLAM, strike(2.0, 1.0, vec![token("melee")]))],
        NEVER,
    );
    let mut reg = EntityRegistry::new();
    budgets(&mut reg, (3, 0.0), (1, 0.0));
    let mut runtime = AiRuntime::deciding_every_tick();
    spawn_player(&mut reg, player_spot());
    let enemy = spawn_resting(&mut reg, 0, &graph);
    set_cooldown(&mut reg, enemy, SLAM, 26.0);

    tick(&mut reg, &mut runtime);
    assert_eq!(cooldown(&reg, enemy, SLAM), 10.0, "N−1 leaves 10 ms");
    assert_eq!(requested(&reg, enemy), None, "N−1 requests nothing");
    assert!(held(&reg, enemy).is_empty());

    tick(&mut reg, &mut runtime);
    assert_eq!(cooldown(&reg, enemy, SLAM), 0.0);
    assert_eq!(state(&reg, enemy), REST);
    assert_eq!(held(&reg, enemy), vec![SLAM], "requested and granted on N");

    tick(&mut reg, &mut runtime);
    assert_eq!(state(&reg, enemy), hold(SLAM), "the row passes on N+1");
}

// Regression: a weapon burst outlasting its recover carried its turn into the
// rest, where the queued row passed on it; the hold's mark then kept it across
// every repeat, so it never ended or refreshed and a waiter on its token
// starved. A carried turn opens no queued row: it ends in the rest once the
// burst does, its refresh starts, and the waiter is granted ahead of the
// brute's re-request.
#[test]
fn a_burst_outlasting_its_recover_ends_its_turn_in_the_rest_and_a_waiter_is_granted() {
    // Three shots 160 ms apart outlast the release's and recover's 48 ms
    // each; each shot's 16 ms cooldown elapses before the next tick's rows.
    let descriptors = [burst_weapon("enemy.burst", 3, 160.0, 16.0)];
    let shoot = AttackParams {
        draws: Some(vec![token("melee")]),
        ..weapon("enemy.burst")
    };
    let graph = queue_graph(
        vec![queued(SLAM, always())],
        vec![(SLAM, shoot)],
        [0.0, 0.0],
    );
    let mut reg = EntityRegistry::new();
    budgets(&mut reg, (1, 64.0), (1, 0.0));
    let mut runtime = AiRuntime::deciding_every_tick();
    let pawn = spawn_player(&mut reg, player_spot());
    let brute = spawn_resting(&mut reg, 0, &graph);
    let waiter = spawn_at(&mut reg, 1, &hog_graph(SWEEP, vec![token("melee")]), "idle");

    tick_with(&mut reg, &mut runtime, &descriptors);
    assert_eq!(held(&reg, brute), vec![SLAM], "granted in the rest");
    enter(&mut reg, waiter, "sweep_act");

    struct Seen {
        state: String,
        brute_holds: bool,
        waiter_holds: bool,
        refreshing: u32,
    }
    let mut seen = Vec::new();
    let mut shots = 0;
    for _ in 0..80 {
        let result = tick_with(&mut reg, &mut runtime, &descriptors);
        shots += result.projectile_spawns.len();
        seen.push(Seen {
            state: state(&reg, brute),
            brute_holds: !held(&reg, brute).is_empty(),
            waiter_holds: !held(&reg, waiter).is_empty(),
            refreshing: melee_refreshing(&runtime, pawn),
        });
    }
    let states: Vec<&str> = seen.iter().map(|tick| tick.state.as_str()).collect();
    assert_eq!(shots, 3, "one burst, never struck again: {states:?}");
    let recovered = states
        .iter()
        .position(|state| *state == recover(SLAM))
        .expect("fixture: the strike recovers");
    let rested = recovered
        + states[recovered..]
            .iter()
            .position(|state| *state == REST)
            .expect("fixture: the strike lands in its rest");
    assert!(
        states[rested..].iter().all(|state| *state == REST),
        "the carried turn never opens the queued row: {states:?}"
    );
    assert!(
        seen[rested].brute_holds,
        "fixture: the burst carries the turn into the rest"
    );
    let ended = rested
        + seen[rested..]
            .iter()
            .position(|tick| !tick.brute_holds)
            .expect("the carried turn ends in the rest");
    assert_eq!(seen[ended].refreshing, 1, "its refresh starts");
    assert!(
        seen[..ended].iter().all(|tick| !tick.waiter_holds),
        "the waiter waits while the burst holds the token"
    );
    let granted = seen
        .iter()
        .position(|tick| tick.waiter_holds)
        .expect("the waiter on the token is granted");
    assert!(granted >= ended);
    assert!(
        seen[ended..].iter().all(|tick| !tick.brute_holds),
        "the waiter is granted ahead of the brute's re-request"
    );
    assert_eq!(
        requested(&reg, brute).as_deref(),
        Some(SLAM),
        "still in line"
    );
}

/// The activity active at the deepest level of `enemy`'s path.
fn leaf(reg: &EntityRegistry, enemy: EntityId) -> String {
    let brain = brain(reg, enemy);
    brain
        .activity_at_depth(brain.active_depth() - 1)
        .map(|(name, _)| name.to_string())
        .expect("the brain path names declared activities")
}

/// [`queue_graph`]'s `slam` stance nested under `engage`, a composite marked
/// for `slam` itself. The stance opens on an unposed `settle` that takes the
/// rest after `settle_ms`.
fn stance_under_a_slam_mark(settle_ms: f32) -> BehaviorGraphDescriptor {
    let flat = unvalidated_queue_graph(
        vec![queued(SLAM, always())],
        vec![(SLAM, strike(2.0, 1.0, vec![token("melee")]))],
        [0.0, 0.0],
    );
    let mut stance = flat.envelope;
    stance.activities.insert(APPROACH.to_string(), idle());
    stance
        .transitions
        .insert(APPROACH.to_string(), vec![edge(REST, after_ms(settle_ms))]);
    let layer = BehaviorLayerDescriptor::Graph(stance);
    let engage = BehaviorActivityDescriptor {
        animation: None,
        motion: None,
        layers: BTreeMap::from([("stance".to_string(), layer)]),
        ..marked(idle(), SLAM)
    };
    let graph = test_behavior_graph!({
        initial: "engage".to_string(),
        activities: BTreeMap::from([("engage".to_string(), engage)]),
        transitions: BTreeMap::new(),
        candidate_filter: None,
        retaliation: None,
        patrol: None,
        attacks: flat.attacks,
        engagement_radius: None,
        move_speed: TEST_MOVE_SPEED,
    });
    graph
        .validate()
        .expect("a mark may enclose a stance striking its own attack")
}

// Regression: a stance nested under an ancestor marked for the attack it
// strikes. The ancestor's mark holds the turn across the whole stance (no turn
// time limit), so no new grant comes while it holds; the rest's queued row
// opens on that turn at any age. Before the fix the row opened only on the
// evaluation after a grant: a rest first reached two or more ticks after the
// ancestor's grant never struck, and one reached at once struck only once.
#[test]
fn a_stance_under_a_mark_for_its_own_attack_strikes_again_and_again() {
    for settle_ms in [0.0, 96.0] {
        let graph = stance_under_a_slam_mark(settle_ms);
        let mut reg = EntityRegistry::new();
        budgets(&mut reg, (3, 1000.0), (1, 0.0));
        let mut runtime = AiRuntime::deciding_every_tick();
        let pawn = spawn_player(&mut reg, player_spot());
        let enemy = spawn_at(&mut reg, 0, &graph, "engage");

        let mut granted_at = None;
        let mut rested_at = None;
        for tick_index in 0..80 {
            tick(&mut reg, &mut runtime);
            if granted_at.is_none() && !held(&reg, enemy).is_empty() {
                granted_at = Some(tick_index);
            }
            if rested_at.is_none() && leaf(&reg, enemy) == REST {
                rested_at = Some(tick_index);
            }
        }
        let granted_at = granted_at.expect("the ancestor's mark is granted");
        let rested_at = rested_at.expect("the stance reaches its rest");
        if settle_ms > 0.0 {
            assert!(
                rested_at >= granted_at + 2,
                "fixture: the rest comes 2+ ticks after the grant ({granted_at}, {rested_at})"
            );
        }
        assert!(
            player_hp(&reg, pawn) <= 97.0,
            "settle {settle_ms}: the stance strikes again and again, hp {}",
            player_hp(&reg, pawn)
        );
        assert_eq!(
            held_on(&reg, enemy, SLAM),
            vec![pawn],
            "settle {settle_ms}: the ancestor's mark keeps the one turn"
        );
        assert_eq!(
            melee_refreshing(&runtime, pawn),
            0,
            "settle {settle_ms}: the turn never ended"
        );
    }
}

// P1 / P27: a queued grant made on a rest's last run before the schedule
// skips it opens its row on its next run, however many ticks it sat out.
#[test]
fn a_queued_grant_opens_on_the_next_run_after_a_skip() {
    let graph = queue_graph(
        vec![queued(SLAM, always())],
        vec![(SLAM, strike(2.0, 1.0, vec![token("melee")]))],
        NEVER,
    );
    let mut reg = EntityRegistry::new();
    budgets(&mut reg, (3, 0.0), (1, 0.0));
    let mut runtime = AiRuntime::deciding_every_tick();
    spawn_player(&mut reg, Vec3::new(1.8, 0.0, 0.0));
    let enemy = spawn_resting(&mut reg, 0, &graph);
    tick(&mut reg, &mut runtime);
    assert_eq!(state(&reg, enemy), REST);
    assert_eq!(held(&reg, enemy), vec![SLAM], "requested and granted");

    for tick_index in 0..29 {
        tick_skipping(&mut reg, &mut runtime, &[enemy]);
        assert_eq!(state(&reg, enemy), REST, "tick {tick_index}");
        assert_eq!(held(&reg, enemy), vec![SLAM], "tick {tick_index}");
    }
    tick(&mut reg, &mut runtime);
    assert_eq!(state(&reg, enemy), hold(SLAM), "the row opened on its run");
}

// P27 at production cadence, for every pair of decide phases: a queued grant
// opens its rest's row at that enemy's first decide after the grant, so no
// turn is held across a decide with its row still closed; the waiter is
// granted on the tick the first's turn ends, as at 60 Hz; and both strike.
#[test]
fn a_queued_grant_opens_at_the_next_decide_and_the_waiter_follows_at_every_phase() {
    let graph = queue_graph(
        vec![queued(SLAM, always())],
        vec![(SLAM, strike(2.0, 300.0, vec![token("melee")]))],
        [0.0, 0.0],
    );
    let interval = crate::decide::DECIDE_INTERVAL_TICKS;
    let identity_on = crate::decide::identity_deciding_on;
    for first_phase in 0..interval {
        for second_phase in 0..interval {
            let mut reg = EntityRegistry::new();
            budgets(&mut reg, (1, 0.0), (1, 0.0));
            let mut runtime = AiRuntime::noticing_on_sight();
            let pawn = spawn_player(&mut reg, player_spot());
            let enemies = [(0, first_phase), (1, second_phase)].map(|(index, phase)| {
                let enemy = spawn_resting(&mut reg, index, &graph);
                let mut brain = brain(&reg, enemy);
                brain.schedule_identity = Some(identity_on(phase));
                reg.set_component(enemy, brain).unwrap();
                (enemy, identity_on(phase))
            });
            let label = format!("phases {first_phase}/{second_phase}");
            // Per enemy: the tick a grant arrived while it rested, until its
            // row opens.
            let mut awaiting: [Option<u64>; 2] = [None, None];
            let mut was_held = [false, false];
            for tick in 1..=240_u64 {
                tick_with(&mut reg, &mut runtime, &[]);
                let held_now = enemies.map(|(enemy, _)| !held(&reg, enemy).is_empty());
                for (slot, &(enemy, identity)) in enemies.iter().enumerate() {
                    let decides = tick == 1 || crate::decide::decides_on(tick, identity, interval);
                    let resting = state(&reg, enemy) == REST;
                    if let Some(granted) = awaiting[slot] {
                        if !resting {
                            awaiting[slot] = None;
                        } else {
                            assert!(
                                !decides || tick == granted,
                                "{label}: enemy {slot} granted on {granted} still rests at its decide {tick}"
                            );
                        }
                    }
                    if held_now[slot] && !was_held[slot] && resting {
                        awaiting[slot] = Some(tick);
                    }
                }
                if held_now[1] && !was_held[1] {
                    assert!(
                        !held_now[0] && was_held[0],
                        "{label}: the waiter is granted on the tick the first's turn ends ({tick})"
                    );
                }
                was_held = held_now;
                if player_hp(&reg, pawn) <= 98.0 {
                    break;
                }
            }
            assert!(
                player_hp(&reg, pawn) <= 98.0,
                "{label}: both strike, none starves: hp {}",
                player_hp(&reg, pawn)
            );
        }
    }
}
