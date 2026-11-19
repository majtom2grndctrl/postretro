// Per-activity `speed` through the fixed tick: AI resolves the innermost
// `speed` on its post-transition path, and steering, after AI, moves the agent
// at that multiple of its archetype's base `moveSpeed`. Knockback never scales.
// See: context/lib/entity_model.md §7c · context/lib/scripting.md §2

use std::cell::RefCell;
use std::collections::{BTreeMap, HashMap};
use std::rc::Rc;

use glam::Vec3;
use postretro_entities::components::agent::AgentComponent;
use postretro_entities::components::brain::BrainComponent;
use postretro_entities::components::mesh::{
    AnimationState, InterruptPolicy, MeshAnimation, MeshComponent,
};
use postretro_entities::{EntityId, EntityRegistry, Transform};
use postretro_foundation::{
    BRAIN_TIME_IN_ACTIVITY_MS_INPUT, BehaviorActivityDescriptor, BehaviorGraphDescriptor,
    BehaviorGraphEnvelope, BehaviorLayerDescriptor, GuardedRow, IrNode, IrValue, MotionVerb,
    PatrolDescriptor, PatrolMode,
};
use postretro_scripting_core::reaction_dispatch::ProgressTracker;

use super::determinism_tests::{
    floor_world, open_floor_nav_graph, run_driven_agent_sim_tick, spawn_player,
};
use crate::collision::CollisionWorld;
use crate::kinematic_mover::MoverTickStateTable;
use crate::nav::NavGraph;
use crate::scripting_systems::hit_zones::HitZoneStore;

const DT: f32 = 1.0 / 60.0;
/// Each timed activity's length: long enough to reach its top speed.
const PHASE_MS: f32 = 600.0;
/// Relative slack for "at its top speed": steering lands within a hair of it.
const SPEED_TOLERANCE: f32 = 0.02;
const ENEMY_START: Vec3 = Vec3::new(4.0, 1.21, 32.0);
/// Far down +X, so no arrival slowdown or combat-slot standoff reaches it.
const PLAYER_POSITION: Vec3 = Vec3::new(62.0, 1.21, 32.0);

fn leaf(animation: &str, motion: MotionVerb, speed: Option<f32>) -> BehaviorActivityDescriptor {
    BehaviorActivityDescriptor {
        pose: None,
        stagger_to: None,
        passes: None,
        sound: None,
        animation: Some(animation.to_string()),
        motion: Some(motion),
        action: None,
        on_enter: None,
        facing: None,
        reposition: None,
        attack_turn: None,
        speed,
        layers: BTreeMap::new(),
    }
}

fn chase(speed: Option<f32>) -> BehaviorActivityDescriptor {
    leaf("locomotion", MotionVerb::ChaseTarget, speed)
}

/// A row to `to` once the source has lasted `ms`.
fn after(to: &str, ms: f32) -> GuardedRow {
    GuardedRow {
        release_after: None,
        queue: None,
        after_moment: false,
        to: to.to_string(),
        when: IrNode::Ge {
            a: Box::new(IrNode::Input {
                name: BRAIN_TIME_IN_ACTIVITY_MS_INPUT.to_string(),
                owner: None,
            }),
            b: Box::new(IrNode::Const {
                value: IrValue::Number(ms),
            }),
        },
    }
}

fn envelope(
    initial: &str,
    activities: Vec<(&str, BehaviorActivityDescriptor)>,
    rows: Vec<(&str, GuardedRow)>,
) -> BehaviorGraphEnvelope {
    let mut transitions: BTreeMap<String, Vec<GuardedRow>> = BTreeMap::new();
    for (from, row) in rows {
        transitions.entry(from.to_string()).or_default().push(row);
    }
    BehaviorGraphEnvelope {
        initial: initial.to_string(),
        activities: activities
            .into_iter()
            .map(|(name, activity)| (name.to_string(), activity))
            .collect(),
        transitions,
    }
}

fn graph(move_speed: f32, root: BehaviorGraphEnvelope) -> BehaviorGraphDescriptor {
    BehaviorGraphDescriptor {
        ai: Default::default(),
        knockback: Default::default(),
        envelope: root,
        candidate_filter: None,
        retaliation: None,
        patrol: None,
        attacks: BTreeMap::new(),
        engagement_radius: None,
        move_speed,
    }
}

/// `walk` → `sprint` (`speed: 2`) → `stroll`, each chasing, one `PHASE_MS`
/// apart.
fn sprint_graph(move_speed: f32) -> BehaviorGraphDescriptor {
    graph(
        move_speed,
        envelope(
            "walk",
            vec![
                ("walk", chase(None)),
                ("sprint", chase(Some(2.0))),
                ("stroll", chase(None)),
            ],
            vec![
                ("walk", after("sprint", PHASE_MS)),
                ("sprint", after("stroll", PHASE_MS)),
            ],
        ),
    )
}

/// One planted, chasing world: a far player and one enemy agent.
struct Arena {
    registry: Rc<RefCell<EntityRegistry>>,
    world: CollisionWorld,
    nav_graph: NavGraph,
    hit_zones: HitZoneStore,
    progress: ProgressTracker,
    ai_runtime: postretro_ai::AiRuntime,
    mover_states: MoverTickStateTable,
    ticks: u32,
}

impl Arena {
    fn new() -> Self {
        let registry = Rc::new(RefCell::new(EntityRegistry::new()));
        spawn_player(&mut registry.borrow_mut(), PLAYER_POSITION);
        Self {
            registry,
            world: floor_world(),
            nav_graph: open_floor_nav_graph(),
            hit_zones: HitZoneStore::new(),
            progress: ProgressTracker::new(),
            ai_runtime: postretro_ai::AiRuntime::noticing_on_sight(),
            mover_states: MoverTickStateTable::default(),
            ticks: 0,
        }
    }

    fn spawn_enemy(&mut self, graph: &BehaviorGraphDescriptor, position: Vec3) -> EntityId {
        let mut registry = self.registry.borrow_mut();
        let enemy = registry.spawn(Transform {
            position,
            ..Transform::default()
        });
        let mut brain = BrainComponent::from_graph(graph);
        brain.home_anchor = position;
        registry.set_component(enemy, brain).unwrap();
        registry
            .entity_state_mut(enemy)
            .expect("enemy carries entity state")
            .set(
                postretro_foundation::FACTION_STATE_FIELD,
                postretro_entities::DEFAULT_ENEMY_FACTION_INDEX,
            );
        registry
            .set_component(enemy, AgentComponent::new(0.35, 1.8, 0.4, graph.move_speed))
            .unwrap();
        enemy
    }

    fn tick(&mut self) {
        self.ticks += 1;
        run_driven_agent_sim_tick(
            self.registry.clone(),
            &self.world,
            &self.hit_zones,
            &self.nav_graph,
            f64::from(self.ticks) * f64::from(DT),
            &mut self.progress,
            &mut self.ai_runtime,
            &mut self.mover_states,
        );
    }

    fn agent(&self, enemy: EntityId) -> AgentComponent {
        self.registry
            .borrow()
            .get_component::<AgentComponent>(enemy)
            .expect("enemy keeps its agent")
            .clone()
    }

    /// The horizontal speed the tick's steering moved the agent at.
    fn speed_xz(&self, enemy: EntityId) -> f32 {
        let velocity = self.agent(enemy).velocity;
        Vec3::new(velocity.x, 0.0, velocity.z).length()
    }

    /// The deepest active activity's name.
    fn leaf(&self, enemy: EntityId) -> String {
        let registry = self.registry.borrow();
        let brain = registry
            .get_component::<BrainComponent>(enemy)
            .expect("enemy keeps its brain");
        brain
            .activity_at_depth(brain.active_depth() - 1)
            .expect("a seated path has a leaf")
            .0
            .to_string()
    }

    fn position(&self, enemy: EntityId) -> Vec3 {
        self.registry
            .borrow()
            .get_component::<Transform>(enemy)
            .expect("enemy keeps its transform")
            .position
    }
}

/// One tick's post-tick leaf and steered speed.
struct TickTrace {
    leaf: String,
    speed: f32,
}

fn trace(arena: &mut Arena, enemy: EntityId, ticks: u32) -> Vec<TickTrace> {
    (0..ticks)
        .map(|_| {
            arena.tick();
            TickTrace {
                leaf: arena.leaf(enemy),
                speed: arena.speed_xz(enemy),
            }
        })
        .collect()
}

fn first_tick_in(trace: &[TickTrace], leaf: &str) -> usize {
    trace
        .iter()
        .position(|tick| tick.leaf == leaf)
        .unwrap_or_else(|| panic!("the run never entered `{leaf}`"))
}

fn assert_near(actual: f32, expected: f32, label: &str) {
    assert!(
        (actual - expected).abs() <= expected * SPEED_TOLERANCE,
        "{label}: expected {expected}, got {actual}"
    );
}

fn sprint_trace(move_speed: f32) -> Vec<TickTrace> {
    let mut arena = Arena::new();
    let enemy = arena.spawn_enemy(&sprint_graph(move_speed), ENEMY_START);
    trace(&mut arena, enemy, 140)
}

// AC Sp1: one `speed: 2` activity steers each archetype at twice its own
// `moveSpeed`, and at its `moveSpeed` once left.
#[test]
fn a_speed_two_activity_steers_each_archetype_at_twice_its_own_move_speed() {
    for move_speed in [2.5_f32, 4.0] {
        let trace = sprint_trace(move_speed);
        let entered = first_tick_in(&trace, "sprint");
        let left = first_tick_in(&trace, "stroll");
        assert_near(
            trace[entered - 1].speed,
            move_speed,
            &format!("moveSpeed {move_speed}: cruising before `sprint`"),
        );
        assert_near(
            trace[left - 1].speed,
            2.0 * move_speed,
            &format!("moveSpeed {move_speed}: top speed in `sprint`"),
        );
        let settled = trace[left..]
            .iter()
            .skip(10)
            .take(10)
            .map(|tick| tick.speed);
        for speed in settled {
            assert_near(
                speed,
                move_speed,
                &format!("moveSpeed {move_speed}: back at moveSpeed in `stroll`"),
            );
        }
    }
}

// AC Sp2 / pin P17: the activity's speed governs steering from the tick it is
// entered through the tick before it is left. Steering on the entry tick runs
// under the doubled cap and acceleration, so it already passes the base cap;
// steering on the tick the activity is left runs under the base cap again.
#[test]
fn an_activity_speed_governs_steering_from_its_entry_tick_through_the_tick_before_it_is_left() {
    let move_speed = 3.0_f32;
    let trace = sprint_trace(move_speed);
    let entered = first_tick_in(&trace, "sprint");
    let left = first_tick_in(&trace, "stroll");

    assert!(
        trace[entered - 1].speed <= move_speed * (1.0 + SPEED_TOLERANCE),
        "the tick before entry steers under the base cap: {}",
        trace[entered - 1].speed
    );
    // One tick's acceleration at 2× is 2 × 8 × moveSpeed × dt above the base.
    assert!(
        trace[entered].speed > move_speed * 1.2,
        "the entry tick already steers under the doubled cap and acceleration: {}",
        trace[entered].speed
    );
    for tick in &trace[entered..left] {
        assert!(
            tick.speed <= 2.0 * move_speed * (1.0 + SPEED_TOLERANCE),
            "inside `sprint` steering stays under twice moveSpeed: {}",
            tick.speed
        );
    }
    assert!(
        trace[left - 1].speed > 1.9 * move_speed,
        "the tick before leaving still steers at the doubled speed: {}",
        trace[left - 1].speed
    );
    assert!(
        trace[left].speed <= move_speed * (1.0 + SPEED_TOLERANCE),
        "the tick it is left steers under the base cap again: {}",
        trace[left].speed
    );
}

// Pin P17, reseat half: a reseat seats `sprint` (`speed: 2`) and its own row
// leaves it in the same AI pass, so steering that tick never moves under it.
#[test]
fn an_activity_reseated_and_left_in_one_pass_never_steers_at_its_speed() {
    let move_speed = 3.0_f32;
    let graph = graph(
        move_speed,
        envelope(
            "sprint",
            vec![("sprint", chase(Some(2.0))), ("walk", chase(None))],
            vec![("sprint", after("walk", 0.0))],
        ),
    );
    let mut arena = Arena::new();
    let enemy = arena.spawn_enemy(&graph, ENEMY_START);
    // Leaves the spawn-seated `sprint` at once, then cruises in `walk`.
    let cruise = trace(&mut arena, enemy, 60);
    assert_eq!(cruise.last().unwrap().leaf, "walk");
    assert_near(
        cruise.last().unwrap().speed,
        move_speed,
        "cruising in `walk`",
    );

    // A restored brain with a malformed path reseats to `initial` on the next
    // AI pass, before any row runs.
    {
        let mut registry = arena.registry.borrow_mut();
        let brain = registry.get_component::<BrainComponent>(enemy).unwrap();
        let mut value = serde_json::to_value(brain).unwrap();
        value
            .as_object_mut()
            .expect("brain serializes as an object")
            .insert(
                "active_activity_path_len".to_string(),
                serde_json::json!(usize::MAX),
            );
        let malformed: BrainComponent = serde_json::from_value(value).unwrap();
        registry.set_component(enemy, malformed).unwrap();
    }
    arena.tick();

    assert_eq!(
        arena.leaf(enemy),
        "walk",
        "the reseat left `sprint` at once"
    );
    assert_eq!(arena.agent(enemy).activity_speed, 1.0);
    assert!(
        arena.speed_xz(enemy) <= move_speed * (1.0 + SPEED_TOLERANCE),
        "the reseated-and-left `sprint` never steered at its speed: {}",
        arena.speed_xz(enemy)
    );
}

// AC Sp1: an inner activity's `speed` overrides an outer one's; an inner
// activity without one inherits it.
#[test]
fn an_inner_activity_speed_overrides_the_outer_one_and_an_unset_one_inherits_it() {
    let move_speed = 3.0_f32;
    let offense = envelope(
        "inherit",
        vec![("inherit", chase(None)), ("override", chase(Some(0.5)))],
        vec![("inherit", after("override", PHASE_MS))],
    );
    let engage = BehaviorActivityDescriptor {
        pose: None,
        stagger_to: None,
        passes: None,
        sound: None,
        animation: None,
        motion: None,
        action: None,
        on_enter: None,
        facing: None,
        reposition: None,
        attack_turn: None,
        speed: Some(2.0),
        layers: BTreeMap::from([(
            "offense".to_string(),
            BehaviorLayerDescriptor::Graph(offense),
        )]),
    };
    let graph = graph(
        move_speed,
        envelope("engage", vec![("engage", engage)], vec![]),
    );
    let mut arena = Arena::new();
    let enemy = arena.spawn_enemy(&graph, ENEMY_START);
    let trace = trace(&mut arena, enemy, 90);
    let overridden = first_tick_in(&trace, "override");

    assert_near(
        trace[overridden - 1].speed,
        2.0 * move_speed,
        "`inherit` steers at its composite's 2×",
    );
    assert_near(
        trace.last().unwrap().speed,
        0.5 * move_speed,
        "`override` steers at its own 0.5×",
    );
}

/// One knockback slide: the enemy launched by the same impulse, then traced.
fn slide(motion: MotionVerb, speed: Option<f32>) -> (Vec<Vec3>, Vec<Vec3>, f32) {
    let move_speed = 3.0_f32;
    let graph = graph(
        move_speed,
        envelope(
            "act",
            vec![("act", leaf("locomotion", motion, speed))],
            vec![],
        ),
    );
    let mut arena = Arena::new();
    let enemy = arena.spawn_enemy(&graph, ENEMY_START);
    // Settle on the floor and, when chasing, reach top speed.
    for _ in 0..40 {
        arena.tick();
    }
    let multiplier = arena.agent(enemy).activity_speed;
    {
        let mut registry = arena.registry.borrow_mut();
        let mut agent = registry
            .get_component::<AgentComponent>(enemy)
            .unwrap()
            .clone();
        assert!(agent.add_knockback(Vec3::new(0.0, 0.0, 9.0)));
        registry.set_component(enemy, agent).unwrap();
    }
    let start = arena.position(enemy);
    let mut offsets = Vec::new();
    let mut knockback = Vec::new();
    for _ in 0..30 {
        arena.tick();
        offsets.push(arena.position(enemy) - start);
        knockback.push(arena.agent(enemy).knockback_velocity);
    }
    (offsets, knockback, multiplier)
}

// AC Sp4: `speed` scales only the agent's own steered travel. A planted
// enemy's knockback slide is identical under `speed: 2` and none, and a
// chasing enemy's knockback decays identically while its steered pace differs.
#[test]
fn a_knockback_slide_is_the_same_under_speed_two_as_under_none() {
    let (planted, planted_knockback, multiplier) = slide(MotionVerb::Hold, None);
    let (planted_fast, planted_fast_knockback, fast_multiplier) =
        slide(MotionVerb::Hold, Some(2.0));
    assert_eq!(multiplier, 1.0);
    assert_eq!(
        fast_multiplier, 2.0,
        "the planted enemy's agent carries its activity's speed"
    );
    assert!(
        planted.last().unwrap().z > 0.5,
        "the impulse slid the enemy: {:?}",
        planted.last()
    );
    assert_eq!(
        planted, planted_fast,
        "the slide is the same under `speed: 2`"
    );
    assert_eq!(planted_knockback, planted_fast_knockback);

    let (_, chasing_knockback, _) = slide(MotionVerb::ChaseTarget, None);
    let (_, chasing_fast_knockback, chasing_multiplier) = slide(MotionVerb::ChaseTarget, Some(2.0));
    assert_eq!(chasing_multiplier, 2.0);
    // Collision resolution along a different steered path leaves only float
    // noise on the cross axis.
    for (slow, fast) in chasing_knockback.iter().zip(&chasing_fast_knockback) {
        assert!(
            (*slow - *fast).length() <= 1.0e-5,
            "knockback decays the same while steering runs at 2×: {slow} vs {fast}"
        );
    }
}

/// Where each position goal leads: far enough down +X that the enemy is
/// still cruising when sampled.
const GOAL: Vec3 = Vec3::new(40.0, 1.21, 32.0);
/// The position goal's `speed`, below 1 and above the rate clamp's floor.
const STROLL: f32 = 0.8;
/// The archetype's base `moveSpeed`.
const STROLL_MOVE_SPEED: f32 = 3.0;

/// `wander` (initial) plays `walk` under `motion` at `speed: 0.8`, toward
/// [`GOAL`] whichever position goal it is. `approach`, never entered, makes
/// `run` the graph's locomotion state, so only the shared rule (every motion
/// that steers) can rate-scale `walk`.
fn position_goal_graph(motion: MotionVerb) -> BehaviorGraphDescriptor {
    let mut descriptor = graph(
        STROLL_MOVE_SPEED,
        envelope(
            "wander",
            vec![
                ("approach", leaf("run", MotionVerb::ChaseTarget, None)),
                ("wander", leaf("walk", motion, Some(STROLL))),
            ],
            vec![],
        ),
    );
    descriptor.patrol = Some(PatrolDescriptor {
        // Patrol points are offsets from the home anchor (`GOAL`).
        points: vec![[0.0, 0.0], [0.0, 8.0]],
        mode: PatrolMode::Loop,
    });
    descriptor
}

/// An uncalibrated mesh: no state has a `travelSpeed` and no model derives a
/// stride, so a scaled state's rate is steered speed over base `moveSpeed`.
fn strolling_mesh() -> MeshComponent {
    let state = |clip: &str| AnimationState {
        clip: clip.to_string(),
        looping: true,
        crossfade_ms: 0.0,
        interrupt: InterruptPolicy::Smooth,
        travel_speed: None,
        max_playback_rate: None,
        scale_with_speed: true,
        playback_rate: None,
        clip_index: None,
    };
    MeshComponent::animated(
        "stroller".to_string(),
        MeshAnimation::new(
            HashMap::from([
                ("idle".to_string(), state("idle")),
                ("walk".to_string(), state("walk")),
                ("run".to_string(), state("run")),
            ]),
            "walk".to_string(),
        ),
    )
}

// The shared rate-scale rule counts position goals: a state an activity plays
// under `patrol`, `moveToLastKnown` or `moveToAnchor` strides at its steered
// speed on the host, as the brute's strolling `walk` does. Uncalibrated, that
// rate is `speed` × base `moveSpeed` over base `moveSpeed`: 0.8, not the
// authored 1.0 an unscaled state rests at.
#[test]
fn a_state_played_under_a_position_goal_rate_scales_with_its_steered_speed() {
    for motion in [
        MotionVerb::Patrol,
        MotionVerb::MoveToLastKnown,
        MotionVerb::MoveToAnchor,
    ] {
        let mut arena = Arena::new();
        let enemy = arena.spawn_enemy(&position_goal_graph(motion), ENEMY_START);
        {
            let mut registry = arena.registry.borrow_mut();
            let mut brain = registry
                .get_component::<BrainComponent>(enemy)
                .expect("enemy has a brain")
                .clone();
            brain.home_anchor = GOAL;
            brain.last_known_target_pos = Some(GOAL);
            registry.set_component(enemy, brain).unwrap();
            registry.set_component(enemy, strolling_mesh()).unwrap();
        }
        for _ in 0..60 {
            arena.tick();
        }

        let speed = arena.speed_xz(enemy);
        assert_near(
            speed,
            STROLL * STROLL_MOVE_SPEED,
            &format!("{motion:?}: cruising at its activity speed"),
        );
        let animation = arena
            .registry
            .borrow()
            .get_component::<MeshComponent>(enemy)
            .expect("enemy keeps its mesh")
            .animation
            .clone()
            .expect("the mesh is animated");
        assert_eq!(animation.current_state, "walk", "{motion:?}");
        assert!(
            (animation.rate - speed / STROLL_MOVE_SPEED).abs() <= 0.02,
            "{motion:?}: `walk` strides at {} for a steered {speed} over base {STROLL_MOVE_SPEED}",
            animation.rate
        );
    }
}

// --- per-state rate policy: maxPlaybackRate, scaleWithSpeed, playbackRate ---

const CHARGE_MOVE_SPEED: f32 = 3.0;
/// A `speed: 2` chase steers at 2× base: an uncalibrated stride ratio of 2.0,
/// past the default 1.5 cap.
const CHARGE: f32 = 2.0;

/// `charge` chases at `speed: 2` on `run`; `jab`, never entered unless it is
/// the initial activity, holds on `jab`.
fn charge_graph(initial: &str) -> BehaviorGraphDescriptor {
    graph(
        CHARGE_MOVE_SPEED,
        envelope(
            initial,
            vec![
                ("charge", leaf("run", MotionVerb::ChaseTarget, Some(CHARGE))),
                ("jab", leaf("jab", MotionVerb::Hold, None)),
            ],
            vec![],
        ),
    )
}

/// An uncalibrated mesh whose `run` and `jab` states carry `edit`'s policy.
fn policy_mesh(initial: &str, edit: impl Fn(&str, &mut AnimationState)) -> MeshComponent {
    let state = |clip: &str| {
        let mut state = AnimationState {
            clip: clip.to_string(),
            looping: true,
            crossfade_ms: 0.0,
            interrupt: InterruptPolicy::Smooth,
            travel_speed: None,
            max_playback_rate: None,
            scale_with_speed: true,
            playback_rate: None,
            clip_index: None,
        };
        edit(clip, &mut state);
        state
    };
    MeshComponent::animated(
        "charger".to_string(),
        MeshAnimation::new(
            HashMap::from([
                ("run".to_string(), state("run")),
                ("jab".to_string(), state("jab")),
            ]),
            initial.to_string(),
        ),
    )
}

/// Run `initial` for 60 ticks on `mesh`; returns the settled state and rate.
fn settled_rate(initial: &str, mesh: MeshComponent) -> (String, f32) {
    let mut arena = Arena::new();
    let enemy = arena.spawn_enemy(&charge_graph(initial), ENEMY_START);
    arena
        .registry
        .borrow_mut()
        .set_component(enemy, mesh)
        .unwrap();
    for _ in 0..60 {
        arena.tick();
    }
    let animation = arena
        .registry
        .borrow()
        .get_component::<MeshComponent>(enemy)
        .expect("enemy keeps its mesh")
        .animation
        .clone()
        .expect("the mesh is animated");
    (animation.current_state, animation.rate)
}

#[test]
fn a_default_state_clamps_the_charge_stride_at_one_point_five() {
    let (state, rate) = settled_rate("charge", policy_mesh("run", |_, _| {}));
    assert_eq!(state, "run");
    assert!(
        (rate - postretro_entities::components::mesh::RATE_MAX).abs() < 1e-6,
        "an uncapped `run` clamps at RATE_MAX, got {rate}"
    );
}

#[test]
fn a_raised_max_playback_rate_lets_the_charge_stride_past_one_point_five() {
    let (state, rate) = settled_rate(
        "charge",
        policy_mesh("run", |clip, state| {
            if clip == "run" {
                state.max_playback_rate = Some(3.0);
            }
        }),
    );
    assert_eq!(state, "run");
    assert!(
        (rate - CHARGE).abs() <= 0.03,
        "`run` strides at its steered ratio {CHARGE} under a 3.0 cap, got {rate}"
    );

    // A cap below the ratio still clamps, at the raised value.
    let (_, rate) = settled_rate(
        "charge",
        policy_mesh("run", |clip, state| {
            if clip == "run" {
                state.max_playback_rate = Some(1.75);
            }
        }),
    );
    assert!(
        (rate - 1.75).abs() < 1e-6,
        "clamps at the 1.75 cap, got {rate}"
    );
}

#[test]
fn scale_with_speed_false_holds_a_steering_state_at_its_fixed_rate() {
    let (state, rate) = settled_rate(
        "charge",
        policy_mesh("run", |clip, state| {
            if clip == "run" {
                state.scale_with_speed = false;
                state.max_playback_rate = Some(3.0);
            }
        }),
    );
    assert_eq!(state, "run");
    assert_eq!(rate, 1.0, "an opted-out state plays at 1x while it steers");

    let (_, rate) = settled_rate(
        "charge",
        policy_mesh("run", |clip, state| {
            if clip == "run" {
                state.scale_with_speed = false;
                state.playback_rate = Some(0.8);
            }
        }),
    );
    assert_eq!(rate, 0.8, "an opted-out state plays its playbackRate");
}

#[test]
fn playback_rate_sets_an_unscaled_state_and_is_ignored_while_stride_scaled() {
    let fast = |clip: &str, state: &mut AnimationState| {
        if clip == "jab" || clip == "run" {
            state.playback_rate = Some(2.5);
        }
    };
    let (state, rate) = settled_rate("jab", policy_mesh("jab", fast));
    assert_eq!(state, "jab");
    assert_eq!(
        rate, 2.5,
        "a held state plays its playbackRate, past RATE_MAX"
    );

    let (state, rate) = settled_rate("charge", policy_mesh("run", fast));
    assert_eq!(state, "run");
    assert!(
        (rate - postretro_entities::components::mesh::RATE_MAX).abs() < 1e-6,
        "the stride rule decides a scaled state's rate, got {rate}"
    );
}
