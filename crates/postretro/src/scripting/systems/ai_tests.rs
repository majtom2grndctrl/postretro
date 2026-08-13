// Unit tests for the engine-owned enemy brain tick. The integration tests build
// a minimal registry with a player pawn (PlayerMovement + Transform + Health),
// an enemy (Brain + Transform + Agent + Mesh), and assert observable outcomes
// — destination via `path_state`, HP deltas via the chokepoint, the selected
// animation name, and stride gating. Every fixture is an authored behavior
// graph.

use std::borrow::Cow;
use std::collections::BTreeMap;

use glam::Vec3;
use parry3d::math::{Isometry, Point};
use parry3d::shape::TriMesh;
use postretro_level_format::navmesh::{NAVMESH_VERSION, NavMeshSection, NavPortal, NavRegion};

use super::candidate_scope::CandidateScope;
use super::combat_slots::COMBAT_SLOT_HOLD_TICKS;
use super::engine_floor::{TARGET_SWITCH_HYSTERESIS_DISTANCE, think_stride_for_distance};
use super::facing::{FACING_TURN_RATE, slew_yaw, yaw_from_rotation, yaw_rotation_toward};
use super::*;
use crate::agent_steering;
use crate::collision::CollisionWorld;
use crate::impact_policy::ImpactPolicyRuntime;
use crate::nav::{NavGraph, distance_xz};
use postretro_entities::components::agent::AgentComponent;
use postretro_entities::components::brain::{BrainComponent, graph_state_index};
use postretro_entities::components::health::HealthComponent;
use postretro_entities::components::mesh::{
    AnimationState, InterruptPolicy, MeshAnimation, MeshComponent,
};
use postretro_entities::components::player_movement::PlayerMovementComponent;
use postretro_entities::registry::{EntityId, EntityRegistry, Transform};
use postretro_entities::{EntityStateComponent, ScriptCtx};
use postretro_foundation::{
    ActionVerb, AttackParams, BRAIN_ACQUISITION_DUE_INPUT, BRAIN_DISTANCE_FROM_ANCHOR_INPUT,
    BRAIN_HAS_TARGET_INPUT, BRAIN_TARGET_DIED_INPUT, BRAIN_TARGET_DISTANCE_INPUT,
    BRAIN_TARGET_HOSTILE_INPUT, BRAIN_TARGET_REACHABLE_INPUT, BRAIN_TIME_IN_STATE_MS_INPUT,
    BakedIr, BehaviorGraphDescriptor, BehaviorStateDescriptor, BindingScope, BoundProgram,
    CANDIDATE_DIED_INPUT, CANDIDATE_DISTANCE_INPUT, CURRENT_IR_VERSION, ImpactEventDescriptor,
    IrNode, IrValue, MotionVerb, PatrolDescriptor, PatrolMode, TransitionDescriptor, bind,
};
use postretro_scripting_core::data_descriptors::{
    AirParams, CapsuleParams, FallParams, ForgivenessParams, GroundParams,
    PlayerMovementDescriptor, SpeedParams,
};

// ---------------------------------------------------------------------------
// Fixtures
// ---------------------------------------------------------------------------

const EPS: f32 = 1e-6;
const TEST_DETECTION_RANGE: f32 = 18.0;
const TEST_ATTACK_RANGE: f32 = 2.0;
const TEST_AGGRO_RANGE: f32 = 26.0;
const TEST_ATTACK_DAMAGE: f32 = 8.0;
const TEST_ATTACK_COOLDOWN_MS: f32 = 1000.0;
const TEST_MOVE_SPEED: f32 = 3.5;
const TEST_IDLE_STATE: &str = "idle";
const TEST_ALERT_STATE: &str = "alert";
const TEST_ATTACK_STATE: &str = "attack";
const TEST_DEATH_STATE: &str = "death";

fn yaw_distance(from: f32, to: f32) -> f32 {
    ((to - from + std::f32::consts::PI).rem_euclid(std::f32::consts::TAU) - std::f32::consts::PI)
        .abs()
}

/// Direct graph used by the general engine-behavior fixtures. It spells the
/// test policy in the same authoring vocabulary shipped content uses: a
/// candidate filter bounds fresh acquisition, and ordered interrupts own all
/// stand-down behavior.
fn test_graph_with(detection_range: f32, aggro_range: f32) -> BehaviorGraphDescriptor {
    BehaviorGraphDescriptor {
        initial: TEST_IDLE_STATE.to_string(),
        states: BTreeMap::from([
            (
                TEST_IDLE_STATE.to_string(),
                authored_state(
                    "idle",
                    MotionVerb::Hold,
                    None,
                    vec![
                        edge(
                            TEST_ATTACK_STATE,
                            when_acquisition_due(target_within(TEST_ATTACK_RANGE)),
                        ),
                        edge(
                            TEST_ALERT_STATE,
                            when_acquisition_due(target_within(detection_range)),
                        ),
                    ],
                ),
            ),
            (
                TEST_ALERT_STATE.to_string(),
                authored_state(
                    "locomotion",
                    MotionVerb::ChaseTarget,
                    None,
                    vec![edge(TEST_ATTACK_STATE, target_within(TEST_ATTACK_RANGE))],
                ),
            ),
            (
                TEST_ATTACK_STATE.to_string(),
                authored_state(
                    "attack",
                    MotionVerb::ChaseTarget,
                    Some(ActionVerb::Attack),
                    vec![edge(TEST_ALERT_STATE, target_beyond(TEST_ATTACK_RANGE))],
                ),
            ),
            (
                TEST_DEATH_STATE.to_string(),
                authored_state("death", MotionVerb::Freeze, None, Vec::new()),
            ),
        ]),
        interrupts: vec![
            edge(TEST_IDLE_STATE, target_lost()),
            edge(TEST_IDLE_STATE, brain_input(BRAIN_TARGET_DIED_INPUT)),
            edge(TEST_IDLE_STATE, target_beyond(aggro_range)),
        ],
        candidate_filter: Some(candidate_is_alive_within(aggro_range)),
        patrol: None,
        attack: Some(AttackParams {
            damage: TEST_ATTACK_DAMAGE,
            range: TEST_ATTACK_RANGE,
            cooldown_ms: TEST_ATTACK_COOLDOWN_MS,
        }),
        engagement_radius: Some(TEST_ATTACK_RANGE),
        move_speed: TEST_MOVE_SPEED,
    }
}

fn tuning() -> BehaviorGraphDescriptor {
    test_graph_with(TEST_DETECTION_RANGE, TEST_AGGRO_RANGE)
}

/// Seed a direct graph in a named state instead of its initial state.
fn brain_with(graph: BehaviorGraphDescriptor, state: &str) -> BrainComponent {
    let mut brain = BrainComponent::from_graph(&graph);
    brain.state_index =
        graph_state_index(&graph, state).expect("the test graph declares the requested state");
    brain
}

/// A usable (clip-resolved) animation state so `switch_animation_state` accepts
/// switches in the integration tests.
fn usable_state(clip: &str, idx: usize) -> AnimationState {
    AnimationState {
        clip: clip.into(),
        looping: true,
        crossfade_ms: 0.0,
        interrupt: InterruptPolicy::Smooth,
        travel_speed: None,
        clip_index: Some(idx),
    }
}

/// A mesh declaring the tuning's animation names, all resolved. `walk` is the
/// SHIPPED reference enemy's travel-state name (`locomotion` is this file's own
/// fixture spelling); both resolve to the same clip so a graph authored either
/// way is drivable here.
fn enemy_mesh() -> MeshComponent {
    let mut states = std::collections::HashMap::new();
    states.insert("idle".to_string(), usable_state("idle_clip", 0));
    states.insert("locomotion".to_string(), usable_state("walk_clip", 1));
    states.insert("walk".to_string(), usable_state("walk_clip", 1));
    states.insert("attack".to_string(), usable_state("attack_clip", 2));
    states.insert("death".to_string(), usable_state("death_clip", 3));
    MeshComponent {
        model: "grunt".into(),
        animation: Some(MeshAnimation::new(states, "idle".into())),
        origin_offset: Vec3::ZERO,
        shadow_bias_scale: 1.0,
        shadow_only: false,
        attachments: Vec::new(),
        pose_inputs: None,
    }
}

/// Minimal valid player-movement descriptor (no dash/crouch/view-feel) so the
/// pawn carries a real `PlayerMovement` component — what `iter_with_kind`
/// targets for the player POSITION lookup.
fn player_movement_descriptor() -> PlayerMovementDescriptor {
    PlayerMovementDescriptor {
        capsule: CapsuleParams {
            radius: 0.4,
            half_height: 0.8,
            eye_height: 0.5,
        },
        ground: GroundParams {
            speed: SpeedParams {
                walk: 7.0,
                run: 11.0,
                crouch: 3.0,
            },
            accel: 10.0,
            step_height: 0.3,
            max_slope: 45.0,
        },
        air: AirParams {
            forward_steer: 0.0,
            accel: 0.7,
            max_control_speed: 0.5,
            bunny_hop: false,
            jumps: 0,
            jump_velocity: 5.5,
            jump_ceiling: 0.0,
        },
        fall: FallParams {
            terminal_velocity: 40.0,
        },
        stuck_stop_enabled: PlayerMovementDescriptor::DEFAULT_STUCK_STOP_ENABLED,
        stuck_stop_threshold: PlayerMovementDescriptor::DEFAULT_STUCK_STOP_THRESHOLD,
        dash: None,
        forgiveness: Some(ForgivenessParams {
            coyote_ms: 0.0,
            jump_buffer_ms: 0.0,
        }),
        crouch: None,
        view_feel: None,
    }
}

/// Spawn the player pawn at `pos` with PlayerMovement (position lookup) and a
/// 100-HP Health (damage target). Returns the pawn id.
fn spawn_player(reg: &mut EntityRegistry, pos: Vec3) -> EntityId {
    let id = reg.spawn(Transform {
        position: pos,
        ..Transform::default()
    });
    reg.set_component(
        id,
        PlayerMovementComponent::from_descriptor(&player_movement_descriptor()),
    )
    .unwrap();
    reg.set_component(
        id,
        HealthComponent {
            max: 100.0,
            current: 100.0,
            hitbox: None,
            death_handled: false,
            pending_kill_credit: None,
            zone_multipliers: std::collections::HashMap::new(),
            contributor_ledger: Default::default(),
        },
    )
    .unwrap();
    id
}

fn spawn_player_without_health(reg: &mut EntityRegistry, pos: Vec3) -> EntityId {
    let id = spawn_player(reg, pos);
    reg.remove_component::<HealthComponent>(id).unwrap();
    id
}

/// Spawn an enemy at `pos` carrying a Brain, an Agent (steering target), a Mesh,
/// and its own Health. Returns the enemy id.
fn spawn_enemy(
    reg: &mut EntityRegistry,
    pos: Vec3,
    brain: BrainComponent,
    enemy_hp: f32,
) -> EntityId {
    let id = reg.spawn(Transform {
        position: pos,
        ..Transform::default()
    });
    let mut brain = brain;
    brain.home_anchor = pos;
    reg.set_component(id, brain).unwrap();
    reg.entity_state_mut(id)
        .expect("spawned enemy carries entity state")
        .set(FACTION_STATE_FIELD, ENEMY_DEFAULT_FACTION);
    reg.set_component(id, AgentComponent::new(0.35, 1.8, 0.4, 3.5))
        .unwrap();
    reg.set_component(id, enemy_mesh()).unwrap();
    reg.set_component(
        id,
        HealthComponent {
            max: enemy_hp,
            current: enemy_hp,
            hitbox: None,
            death_handled: false,
            pending_kill_credit: None,
            zone_multipliers: std::collections::HashMap::new(),
            contributor_ledger: Default::default(),
        },
    )
    .unwrap();
    id
}

fn player_hp(reg: &EntityRegistry, pawn: EntityId) -> f32 {
    reg.get_component::<HealthComponent>(pawn).unwrap().current
}

/// The enemy's current direct-graph state name.
fn enemy_state_name(reg: &EntityRegistry, enemy: EntityId) -> String {
    reg.get_component::<BrainComponent>(enemy)
        .unwrap()
        .state_name()
        .expect("the brain sits in a declared state")
        .to_string()
}

/// Overwrite an entity's current HP (the recovery tests heal a dead enemy back
/// above zero between ticks; the live damage chokepoint floors at zero, so a
/// direct write is the only way to restore HP).
fn set_hp(reg: &mut EntityRegistry, id: EntityId, current: f32) {
    let mut h = reg.get_component::<HealthComponent>(id).unwrap().clone();
    h.current = current;
    reg.set_component(id, h).unwrap();
}

fn enemy_animation(reg: &EntityRegistry, enemy: EntityId) -> String {
    reg.get_component::<MeshComponent>(enemy)
        .unwrap()
        .animation
        .as_ref()
        .unwrap()
        .current_state
        .clone()
}

/// The enemy's current entry stamp (`entered_at`) — `None` when pending (a fresh
/// switch/restart re-stamps it pending until the resolve pass fills it).
fn enemy_anim_entered_at(reg: &EntityRegistry, enemy: EntityId) -> Option<f64> {
    reg.get_component::<MeshComponent>(enemy)
        .unwrap()
        .animation
        .as_ref()
        .unwrap()
        .entered_at
}

fn enemy_locomotion_moving(reg: &EntityRegistry, enemy: EntityId) -> bool {
    reg.get_component::<BrainComponent>(enemy)
        .unwrap()
        .locomotion_moving
}

fn enemy_acquired_target(reg: &EntityRegistry, enemy: EntityId) -> Option<EntityId> {
    reg.get_component::<BrainComponent>(enemy)
        .unwrap()
        .acquired_target
}

fn set_enemy_aggro_armed(reg: &mut EntityRegistry, enemy: EntityId, aggro_armed: bool) {
    let mut brain = reg.get_component::<BrainComponent>(enemy).unwrap().clone();
    brain.aggro_armed = aggro_armed;
    reg.set_component(enemy, brain).unwrap();
}

/// The enemy MESH's yaw-only VISUAL forward vector in the XZ plane, derived from
/// its `Transform.rotation`. The skinned reference characters are authored facing
/// `+Z` in model space (`MESH_FORWARD` in `ai.rs`), and the renderer applies the
/// Transform rotation straight to the model matrix with no axis flip — so the
/// model's front points wherever `rotation * (+Z)` points. Rotating that base
/// model-forward by the stored quaternion gives WHERE the model's FACE looks,
/// letting a facing test assert the enemy looks AT the target (not away from it).
fn enemy_forward_xz(reg: &EntityRegistry, enemy: EntityId) -> Vec3 {
    let rot = reg.get_component::<Transform>(enemy).unwrap().rotation;
    let fwd = rot * Vec3::Z;
    Vec3::new(fwd.x, 0.0, fwd.z).normalize()
}

fn enemy_yaw(reg: &EntityRegistry, enemy: EntityId) -> f32 {
    let rot = reg.get_component::<Transform>(enemy).unwrap().rotation;
    yaw_from_rotation(rot)
}

fn set_enemy_yaw(reg: &mut EntityRegistry, enemy: EntityId, yaw: f32) {
    let mut transform = *reg.get_component::<Transform>(enemy).unwrap();
    transform.rotation = glam::Quat::from_rotation_y(yaw);
    reg.set_component(enemy, transform).unwrap();
}

fn set_enemy_position(reg: &mut EntityRegistry, enemy: EntityId, position: Vec3) {
    let mut transform = *reg.get_component::<Transform>(enemy).unwrap();
    transform.position = position;
    reg.set_component(enemy, transform).unwrap();
}

/// Force the enemy agent's live velocity (what `path_state` reports), so a facing
/// test can stage a "moving" agent without running the steering tick.
fn set_agent_velocity(reg: &mut EntityRegistry, enemy: EntityId, velocity: Vec3) {
    let mut agent = reg.get_component::<AgentComponent>(enemy).unwrap().clone();
    agent.velocity = velocity;
    reg.set_component(enemy, agent).unwrap();
}

fn set_enemy_animation(reg: &mut EntityRegistry, enemy: EntityId, state: &str) {
    let mut mesh = reg.get_component::<MeshComponent>(enemy).unwrap().clone();
    let anim = mesh.animation.as_mut().unwrap();
    anim.current_state = state.to_string();
    anim.entered_at = Some(1.0);
    anim.previous_state = None;
    anim.previous_entered_at = None;
    reg.set_component(enemy, mesh).unwrap();
}

// ---------------------------------------------------------------------------
// Direct-graph transition core
// ---------------------------------------------------------------------------

/// Resolve one hypothetical tick of the direct graph: the state a brain
/// sitting in `current` selects at `distance` from its target, plus the
/// steering intent that state carries. Staying put reports the current state,
/// matching a same-state graph evaluation.
///
/// The registry exists only because [`BrainScope::refresh`] reads health and
/// per-entity state through it; nothing here runs the tick.
fn step_graph(
    graph: &BehaviorGraphDescriptor,
    current: &str,
    distance: f32,
    acquisition_due: bool,
) -> (String, SteeringIntent) {
    let mut reg = EntityRegistry::new();
    let enemy = reg.spawn(Transform::default());
    reg.set_component(enemy, BrainComponent::from_graph(graph))
        .expect("fresh entity is live");

    let mut programs = BrainPrograms::new();
    let mut warned = HashSet::new();
    programs.sync(&reg, &mut warned);
    assert!(warned.is_empty(), "every generated guard binds");
    programs.scope_mut().refresh(
        &reg,
        enemy,
        BrainFacts {
            target: Some((enemy, distance)),
            time_in_state_ms: 0.0,
            attack_cooldown_ms: 0.0,
            acquisition_due,
            distance_from_anchor: 0.0,
            target_hostile: true,
            target_reachable: true,
        },
    );

    let current_index = graph_state_index(graph, current).expect("declared state");
    let bound = programs.get(enemy).expect("the spawned brain is bound");
    let next_index =
        select_transition(graph, bound, programs.scope(), current_index).unwrap_or(current_index);
    let motion = state_at(graph, next_index)
        .expect("the selected index is declared")
        .motion;
    let name = graph
        .states
        .keys()
        .nth(next_index)
        .expect("the selected index is declared")
        .clone();
    (name, steering_for(motion))
}

/// A graph whose authored transition makes the runtime reachability fact
/// observable: a selected but unrouteable target moves from chase to hold.
fn reachability_graph() -> BehaviorGraphDescriptor {
    BehaviorGraphDescriptor {
        initial: "chase".to_string(),
        states: BTreeMap::from([
            (
                "chase".to_string(),
                authored_state(
                    "locomotion",
                    MotionVerb::ChaseTarget,
                    None,
                    vec![edge("hold", target_is_unreachable())],
                ),
            ),
            (
                "hold".to_string(),
                authored_state("idle", MotionVerb::Hold, None, Vec::new()),
            ),
        ]),
        interrupts: Vec::new(),
        candidate_filter: None,
        patrol: None,
        attack: None,
        engagement_radius: None,
        move_speed: TEST_MOVE_SPEED,
    }
}

/// A chase-only graph keeps the selected target retained across stride bands,
/// letting the cache test move it between a due and non-due tick.
fn reachability_cache_graph() -> BehaviorGraphDescriptor {
    BehaviorGraphDescriptor {
        initial: "chase".to_string(),
        states: BTreeMap::from([(
            "chase".to_string(),
            authored_state("locomotion", MotionVerb::ChaseTarget, None, Vec::new()),
        )]),
        interrupts: Vec::new(),
        candidate_filter: None,
        patrol: None,
        attack: None,
        engagement_radius: None,
        move_speed: TEST_MOVE_SPEED,
    }
}

fn target_is_unreachable() -> IrNode {
    IrNode::Select {
        cond: Box::new(brain_input(BRAIN_TARGET_REACHABLE_INPUT)),
        a: Box::new(IrNode::Const {
            value: IrValue::Bool(false),
        }),
        b: Box::new(IrNode::Const {
            value: IrValue::Bool(true),
        }),
    }
}

fn reachability_nav_graph(regions: Vec<NavRegion>) -> NavGraph {
    NavGraph::from_section(&NavMeshSection {
        version: NAVMESH_VERSION,
        origin: [0.0, 0.0, 0.0],
        cell_size: 1.0,
        dim_x: 64,
        dim_z: 64,
        agent_radius: 0.35,
        agent_height: 1.8,
        step_height: 0.4,
        max_slope_deg: 45.0,
        regions,
        portals: Vec::new(),
    })
}

#[test]
fn target_reachability_is_false_without_a_selected_target() {
    let graph = reachability_cache_graph();
    let mut registry = EntityRegistry::new();
    let mut brain = BrainComponent::from_graph(&graph);
    brain.target_reachable = true;
    let enemy = spawn_enemy(&mut registry, Vec3::new(1.0, 0.0, 1.0), brain, 50.0);
    let mut runtime = AiRuntime::new();

    run_ai_tick(&mut registry, &mut runtime, 0.016);

    assert!(
        !registry
            .get_component::<BrainComponent>(enemy)
            .expect("enemy keeps its brain")
            .target_reachable,
        "losing the selected target clears the cached reachability verdict"
    );
}

#[test]
fn target_reachability_is_false_without_a_navmesh_even_off_stride_with_a_restored_cache() {
    let graph = reachability_graph();
    let mut registry = EntityRegistry::new();
    let target = spawn_player(&mut registry, Vec3::new(17.0, 0.0, 1.0));
    let mut brain = BrainComponent::from_graph(&graph);
    brain.acquired_target = Some(target);
    brain.target_reachable = true;
    // A 16 m retained target uses the four-tick band; this is a non-due tick.
    brain.think_stride_counter = 0;
    let enemy = spawn_enemy(&mut registry, Vec3::new(1.0, 0.0, 1.0), brain, 50.0);
    let mut runtime = AiRuntime::new();

    run_ai_tick(&mut registry, &mut runtime, 0.016);

    let brain = registry
        .get_component::<BrainComponent>(enemy)
        .expect("enemy keeps its brain");
    assert!(
        !brain.target_reachable,
        "an absent navmesh clears a restored cached verdict before a non-due tick can reuse it"
    );
    assert_eq!(brain.state_name(), Some("hold"));
}

#[test]
fn target_reachability_uses_the_nav_pathfinder_verdict() {
    let single_region = reachability_nav_graph(vec![NavRegion {
        x0: 0,
        z0: 0,
        x1: 8,
        z1: 8,
        floor_y_min: 0.0,
        floor_y_max: 0.25,
    }]);
    let disconnected_regions = reachability_nav_graph(vec![
        NavRegion {
            x0: 0,
            z0: 0,
            x1: 4,
            z1: 4,
            floor_y_min: 0.0,
            floor_y_max: 0.25,
        },
        NavRegion {
            x0: 5,
            z0: 0,
            x1: 8,
            z1: 4,
            floor_y_min: 0.0,
            floor_y_max: 0.25,
        },
    ]);

    for (nav_graph, expected_reachable, expected_state) in [
        (&single_region, true, "chase"),
        (&disconnected_regions, false, "hold"),
    ] {
        let graph = reachability_graph();
        let mut registry = EntityRegistry::new();
        spawn_player(&mut registry, Vec3::new(6.0, 0.0, 1.0));
        let enemy = spawn_enemy(
            &mut registry,
            Vec3::new(1.0, 0.0, 1.0),
            BrainComponent::from_graph(&graph),
            50.0,
        );
        let mut runtime = AiRuntime::new();

        run_ai_tick_with_navigation(&mut registry, &mut runtime, 0.016, Some(nav_graph), None);

        let brain = registry
            .get_component::<BrainComponent>(enemy)
            .expect("enemy keeps its brain");
        assert_eq!(brain.target_reachable, expected_reachable);
        assert_eq!(brain.state_name(), Some(expected_state));
    }
}

#[test]
fn target_reachability_cache_holds_until_the_next_acquisition_tick() {
    let graph = reachability_cache_graph();
    let nav_graph = reachability_nav_graph(vec![
        NavRegion {
            x0: 0,
            z0: 0,
            x1: 30,
            z1: 10,
            floor_y_min: 0.0,
            floor_y_max: 0.25,
        },
        NavRegion {
            x0: 0,
            z0: 40,
            x1: 30,
            z1: 50,
            floor_y_min: 0.0,
            floor_y_max: 0.25,
        },
    ]);
    let mut registry = EntityRegistry::new();
    let target = spawn_player(&mut registry, Vec3::new(17.0, 0.0, 1.0));
    let mut brain = BrainComponent::from_graph(&graph);
    brain.think_stride_counter = 3;
    let enemy = spawn_enemy(&mut registry, Vec3::new(1.0, 0.0, 1.0), brain, 50.0);
    let mut runtime = AiRuntime::new();

    // Distance 16 uses the four-tick band. Counter 3 makes this first tick due,
    // so the reachable starting location seeds the cache.
    run_ai_tick_with_navigation(&mut registry, &mut runtime, 0.016, Some(&nav_graph), None);
    assert!(
        registry
            .get_component::<BrainComponent>(enemy)
            .expect("enemy keeps its brain")
            .target_reachable
    );

    // The retained target moves to a disconnected region. Its new far-band
    // stride is not due on this tick, so the fact must reuse the cached true
    // verdict instead of issuing another path query.
    set_enemy_position(&mut registry, target, Vec3::new(17.0, 0.0, 41.0));
    run_ai_tick_with_navigation(&mut registry, &mut runtime, 0.016, Some(&nav_graph), None);
    let brain = registry
        .get_component::<BrainComponent>(enemy)
        .expect("enemy keeps its brain");
    assert_eq!(brain.acquired_target, Some(target));
    assert!(
        brain.target_reachable,
        "non-due tick retains the cached verdict"
    );

    // Set the existing stride counter to its next far-band due boundary. This
    // is the first tick allowed to refresh the cache against the moved target.
    let mut brain = brain.clone();
    brain.think_stride_counter = 11;
    registry.set_component(enemy, brain).expect("enemy is live");
    run_ai_tick_with_navigation(&mut registry, &mut runtime, 0.016, Some(&nav_graph), None);
    assert!(
        !registry
            .get_component::<BrainComponent>(enemy)
            .expect("enemy keeps its brain")
            .target_reachable,
        "the next due tick replaces the cache with the disconnected-path verdict"
    );
}

#[test]
fn idle_transitions_to_alert_when_player_enters_detection_range() {
    // Player 10 units away (inside detection 18, outside attack 2): alert+chase.
    assert_eq!(
        step_graph(&tuning(), TEST_IDLE_STATE, 10.0, true),
        (TEST_ALERT_STATE.to_string(), SteeringIntent::Chase)
    );
}

#[test]
fn idle_transitions_straight_to_attack_when_already_in_contact_range() {
    // The direct graph declares the contact edge first, so first-true-wins
    // reaches attack when a candidate is already in range.
    assert_eq!(
        step_graph(&tuning(), TEST_IDLE_STATE, 1.0, true),
        (TEST_ATTACK_STATE.to_string(), SteeringIntent::Chase)
    );
}

#[test]
fn idle_stays_idle_and_clears_when_player_outside_detection_range() {
    assert_eq!(
        step_graph(&tuning(), TEST_IDLE_STATE, 50.0, true),
        (TEST_IDLE_STATE.to_string(), SteeringIntent::Clear)
    );
}

#[test]
fn idle_detection_is_suppressed_when_acquisition_is_gated_off() {
    // Detection is the acquisition-gated edge: in range, but not a think tick.
    assert_eq!(
        step_graph(&tuning(), TEST_IDLE_STATE, 10.0, false),
        (TEST_IDLE_STATE.to_string(), SteeringIntent::Clear)
    );
}

#[test]
fn alert_transitions_to_idle_when_player_exceeds_the_authored_aggro_range() {
    // Player 30 units away exceeds the graph's 26 m stand-down interrupt.
    assert_eq!(
        step_graph(&tuning(), TEST_ALERT_STATE, 30.0, true),
        (TEST_IDLE_STATE.to_string(), SteeringIntent::Clear)
    );
}

#[test]
fn alert_transitions_to_attack_within_attack_range() {
    assert_eq!(
        step_graph(&tuning(), TEST_ALERT_STATE, 1.0, true),
        (TEST_ATTACK_STATE.to_string(), SteeringIntent::Chase)
    );
}

#[test]
fn attack_falls_back_to_alert_when_leaving_attack_range() {
    assert_eq!(
        step_graph(&tuning(), TEST_ATTACK_STATE, 5.0, true),
        (TEST_ALERT_STATE.to_string(), SteeringIntent::Chase)
    );
}

#[test]
fn alert_keeps_chasing_when_acquisition_gated_off_and_still_engaged() {
    // Inside the authored aggro range but acquisition NOT evaluated this tick: must not drop the
    // target — it keeps chasing.
    assert_eq!(
        step_graph(&tuning(), TEST_ALERT_STATE, 10.0, false),
        (TEST_ALERT_STATE.to_string(), SteeringIntent::Chase)
    );
}

#[test]
fn alert_stands_down_when_the_authored_aggro_interrupt_is_true() {
    // Stand-down is graph policy, not acquisition policy: the interrupt fires
    // even when a fresh acquisition scan is not due.
    assert_eq!(
        step_graph(&tuning(), TEST_ALERT_STATE, 30.0, false),
        (TEST_IDLE_STATE.to_string(), SteeringIntent::Clear)
    );
}

#[test]
fn attack_range_entry_is_evaluated_even_when_acquisition_gated_off() {
    // The strided-gap-must-not-suppress-attack contract at the pure level:
    // acquisition off, but the player is inside attack range — still attacks.
    assert_eq!(
        step_graph(&tuning(), TEST_ALERT_STATE, 1.0, false).0,
        TEST_ATTACK_STATE
    );
}

#[test]
fn a_freeze_state_with_no_edges_is_terminal() {
    // The direct graph's freeze state holds its state and steering.
    for acquisition_due in [false, true] {
        assert_eq!(
            step_graph(&tuning(), TEST_DEATH_STATE, 0.0, acquisition_due),
            (TEST_DEATH_STATE.to_string(), SteeringIntent::Hold)
        );
    }
}

// ---------------------------------------------------------------------------
// Pure facing-slew helpers
// ---------------------------------------------------------------------------

#[test]
fn slew_yaw_large_turn_clamps_each_step_and_arrives_at_target() {
    let mut current = 0.0_f32;
    let target = 180.0_f32.to_radians();
    let max_delta = 15.0_f32.to_radians();
    let mut steps = 0;

    while current.to_bits() != target.to_bits() && steps < 32 {
        let previous = current;
        current = slew_yaw(current, target, max_delta);
        steps += 1;
        assert!(
            yaw_distance(previous, current) <= max_delta + EPS,
            "slew advanced more than the angular budget on step {steps}: {previous} -> {current}",
        );
    }

    assert!(
        steps > 1,
        "large turn should require multiple clamped steps"
    );
    assert_eq!(
        current.to_bits(),
        target.to_bits(),
        "slew should converge to the exact target yaw"
    );
}

#[test]
fn slew_yaw_takes_shortest_arc_across_pi_seam() {
    let current = (-170.0_f32).to_radians();
    let target = 170.0_f32.to_radians();
    let max_delta = 5.0_f32.to_radians();

    let next = slew_yaw(current, target, max_delta);

    assert!(
        next < current,
        "shortest arc from -170 deg to +170 deg should rotate through 180, not back through 0"
    );
    assert!(
        (yaw_distance(current, next) - max_delta).abs() < EPS,
        "first seam-crossing step should consume the clamped angular budget"
    );
}

#[test]
fn slew_yaw_returns_target_exactly_when_within_max_delta() {
    let current = 40.0_f32.to_radians();
    let target = 46.0_f32.to_radians();
    let next = slew_yaw(current, target, 10.0_f32.to_radians());

    assert_eq!(
        next.to_bits(),
        target.to_bits(),
        "arrival within the angular budget should return the target exactly"
    );
}

#[test]
fn slew_yaw_sequence_is_deterministic() {
    fn run_sequence() -> Vec<u32> {
        let mut current = (-35.0_f32).to_radians();
        let samples = [
            (160.0_f32.to_radians(), 22.5_f32.to_radians()),
            (160.0_f32.to_radians(), 22.5_f32.to_radians()),
            ((-175.0_f32).to_radians(), 30.0_f32.to_radians()),
            (15.0_f32.to_radians(), 12.0_f32.to_radians()),
            (15.0_f32.to_radians(), 12.0_f32.to_radians()),
        ];
        let mut yaws = Vec::new();
        for (target, max_delta) in samples {
            current = slew_yaw(current, target, max_delta);
            yaws.push(current.to_bits());
        }
        yaws
    }

    assert_eq!(run_sequence(), run_sequence());
}

// Regression: `NaN <= MIN_XZ_LEN_SQ` is false, so a NaN direction fell through
// the zero-length guard into `atan2(NaN, NaN)` and wrote a NaN yaw.
#[test]
fn yaw_rotation_toward_reports_no_heading_for_a_non_finite_direction() {
    for dir in [
        Vec3::new(f32::NAN, 0.0, 1.0),
        Vec3::new(1.0, 0.0, f32::NAN),
        Vec3::new(f32::NAN, f32::NAN, f32::NAN),
        // Y is ignored by the yaw-only math, but a corrupt vector usually
        // carries it — the XZ pair alone must still decide.
        Vec3::new(1.0, f32::NAN, 0.0),
    ] {
        let rotation = yaw_rotation_toward(dir);
        match rotation {
            None => {}
            Some(quat) => assert!(
                quat.x.is_finite()
                    && quat.y.is_finite()
                    && quat.z.is_finite()
                    && quat.w.is_finite(),
                "a non-finite direction {dir:?} must not produce a NaN rotation, got {quat:?}",
            ),
        }
    }
    assert_eq!(
        yaw_rotation_toward(Vec3::new(f32::NAN, 0.0, 1.0)),
        None,
        "a NaN XZ component yields no heading, so the caller leaves facing alone",
    );

    // `+inf` is deliberately still a usable heading: `atan2(inf, inf)` is a
    // finite π/4, so there is nothing to guard against.
    let infinite = yaw_rotation_toward(Vec3::new(f32::INFINITY, 0.0, f32::INFINITY))
        .expect("an infinite direction still yields a finite diagonal heading");
    assert!(
        infinite.x.is_finite()
            && infinite.y.is_finite()
            && infinite.z.is_finite()
            && infinite.w.is_finite(),
    );
}

// Regression: `NaN.signum()` is NaN, so a non-finite yaw was ABSORBING — the
// corrupt rotation was written back and read as the next tick's current yaw.
#[test]
fn slew_yaw_is_total_over_non_finite_yaws() {
    let current = 0.5_f32;
    for target in [f32::NAN, f32::INFINITY, f32::NEG_INFINITY] {
        assert_eq!(
            slew_yaw(current, target, 0.25).to_bits(),
            current.to_bits(),
            "an unusable target yaw must hold the existing facing, not corrupt it",
        );
    }

    // A current yaw that is already corrupt must RE-SEAT rather than stay
    // wedged: the entity gets its facing back on the first tick with a usable
    // target, instead of reading its own NaN forever.
    let target = 1.25_f32;
    for corrupt in [f32::NAN, f32::INFINITY, f32::NEG_INFINITY] {
        let next = slew_yaw(corrupt, target, 0.25);
        assert!(next.is_finite(), "re-seating must produce a usable yaw");
        assert_eq!(
            next.to_bits(),
            target.to_bits(),
            "there is nothing to slew FROM, so the facing seats at the target",
        );
    }
}

#[test]
fn locomotion_intent_uses_xz_speed_and_shared_epsilon() {
    let below = LocomotionIntent::from_velocity(Vec3::new(MOVE_SPEED_EPSILON * 0.5, 99.0, 0.0));
    assert!(!below.moving, "below epsilon is stopped");
    assert_eq!(below.speed_xz_sq, (MOVE_SPEED_EPSILON * 0.5).powi(2));

    let at_epsilon = LocomotionIntent::from_velocity(Vec3::new(MOVE_SPEED_EPSILON, 0.0, 0.0));
    assert!(!at_epsilon.moving, "at epsilon is still stopped");

    let above = LocomotionIntent::from_velocity(Vec3::new(MOVE_SPEED_EPSILON * 1.1, 0.0, 0.0));
    assert!(above.moving, "above epsilon is moving");
}

/// The animation the tick would request for a direct graph state at a given
/// locomotion intent, driven through the graph substitution rule.
fn animation_for_test_state(state: &str, moving: bool) -> String {
    let graph = tuning();
    let index = graph_state_index(&graph, state).unwrap();
    animation_for_state(&graph, index, moving)
        .expect("every test graph state resolves an animation")
        .to_string()
}

#[test]
fn a_stationary_locomotion_state_substitutes_the_rest_animation() {
    assert_eq!(
        animation_for_test_state(TEST_ALERT_STATE, false),
        "idle",
        "a chasing state with no action of its own yields to the graph's rest \
         animation at a standstill",
    );
    assert_eq!(
        animation_for_test_state(TEST_ALERT_STATE, true),
        "locomotion",
        "in motion it plays its own travel cycle",
    );
}

#[test]
fn non_locomotion_states_keep_their_own_animation_regardless_of_speed() {
    assert_eq!(animation_for_test_state(TEST_IDLE_STATE, true), "idle");
    assert_eq!(
        animation_for_test_state(TEST_ATTACK_STATE, false),
        "attack",
        "a chasing state that declares an action is not locomotion",
    );
    assert_eq!(animation_for_test_state(TEST_ATTACK_STATE, true), "attack");
    assert_eq!(animation_for_test_state(TEST_DEATH_STATE, true), "death");
}

#[test]
fn animation_switch_trigger_fires_on_state_or_locomotion_change_only() {
    assert!(should_switch_animation(true, false, false));
    assert!(should_switch_animation(true, true, false));
    assert!(should_switch_animation(false, true, false));
    assert!(should_switch_animation(false, false, true));
    assert!(!should_switch_animation(false, false, false));
    assert!(!should_switch_animation(false, true, true));
}

fn bind_candidate_filter_for_test(node: IrNode) -> BoundProgram<CandidateScope> {
    bind(
        &BakedIr {
            version: CURRENT_IR_VERSION,
            output: None,
            root: node,
        },
        &CandidateScope::for_validation(),
    )
    .expect("candidate filter binds")
}

#[test]
fn select_target_returns_single_player_pawn() {
    let mut reg = EntityRegistry::new();
    let pawn = spawn_player(&mut reg, Vec3::new(5.0, 0.0, 0.0));

    let (_, target) = select_target(
        &reg,
        Vec3::ZERO,
        ENEMY_DEFAULT_FACTION,
        None,
        None,
        None,
        &mut CandidateScope::for_validation(),
    );
    let target = target.expect("player target");

    assert_eq!(target.entity, pawn);
    assert!(target.position.abs_diff_eq(Vec3::new(5.0, 0.0, 0.0), EPS));
}

#[test]
fn select_target_chooses_nearer_remote_pawn_over_marked_local_pawn() {
    let mut reg = EntityRegistry::new();
    let local = spawn_player(&mut reg, Vec3::new(20.0, 0.0, 0.0));
    let remote = spawn_player(&mut reg, Vec3::new(3.0, 0.0, 0.0));
    reg.mark_local_player_pawn(local).unwrap();

    let (_, target) = select_target(
        &reg,
        Vec3::ZERO,
        ENEMY_DEFAULT_FACTION,
        None,
        None,
        None,
        &mut CandidateScope::for_validation(),
    );
    let target = target.expect("player target");

    assert_eq!(
        target.entity, remote,
        "nearest PlayerMovement pawn should win even when it is not local_player_pawn",
    );
    assert!(target.position.abs_diff_eq(Vec3::new(3.0, 0.0, 0.0), EPS));
}

#[test]
fn select_target_keeps_retained_target_when_other_pawn_is_only_slightly_nearer() {
    let mut reg = EntityRegistry::new();
    let retained = spawn_player(&mut reg, Vec3::new(10.0, 0.0, 0.0));
    let slightly_nearer = spawn_player(&mut reg, Vec3::new(9.5, 0.0, 0.0));

    let (_, target) = select_target(
        &reg,
        Vec3::ZERO,
        ENEMY_DEFAULT_FACTION,
        Some(retained),
        None,
        None,
        &mut CandidateScope::for_validation(),
    );
    let target = target.expect("player target");

    assert_eq!(
        target.entity, retained,
        "sticky target hysteresis keeps the retained pawn unless another is > \
         {TARGET_SWITCH_HYSTERESIS_DISTANCE} unit closer",
    );
    assert_ne!(target.entity, slightly_nearer);
}

#[test]
fn select_target_switches_when_other_pawn_is_meaningfully_closer() {
    let mut reg = EntityRegistry::new();
    let retained = spawn_player(&mut reg, Vec3::new(10.0, 0.0, 0.0));
    let closer = spawn_player(&mut reg, Vec3::new(8.5, 0.0, 0.0));

    let (_, target) = select_target(
        &reg,
        Vec3::ZERO,
        ENEMY_DEFAULT_FACTION,
        Some(retained),
        None,
        None,
        &mut CandidateScope::for_validation(),
    );
    let target = target.expect("player target");

    assert_eq!(
        target.entity, closer,
        "a pawn more than the hysteresis distance closer should steal aggro",
    );
}

#[test]
fn select_target_replaces_retained_target_when_retained_is_no_longer_player_pawn() {
    let mut reg = EntityRegistry::new();
    let retained = spawn_player(&mut reg, Vec3::new(5.0, 0.0, 0.0));
    let replacement = spawn_player(&mut reg, Vec3::new(9.0, 0.0, 0.0));
    reg.remove_component::<PlayerMovementComponent>(retained)
        .unwrap();

    let (_, target) = select_target(
        &reg,
        Vec3::ZERO,
        ENEMY_DEFAULT_FACTION,
        Some(retained),
        None,
        None,
        &mut CandidateScope::for_validation(),
    );
    let target = target.expect("player target");

    assert_eq!(
        target.entity, replacement,
        "a retained id without PlayerMovement is invalid and cannot stay sticky",
    );
}

#[test]
fn candidate_filter_excludes_dead_offered_pawn_but_allows_live_one() {
    let mut registry = EntityRegistry::new();
    let dead = spawn_player(&mut registry, Vec3::new(2.0, 0.0, 0.0));
    let live = spawn_player(&mut registry, Vec3::new(4.0, 0.0, 0.0));
    let mut health = registry
        .get_component::<HealthComponent>(dead)
        .unwrap()
        .clone();
    health.death_handled = true;
    registry.set_component(dead, health).unwrap();
    let filter = bind_candidate_filter_for_test(IrNode::Select {
        cond: Box::new(IrNode::Input {
            name: "@candidate.died".into(),
            owner: None,
        }),
        a: Box::new(IrNode::Const {
            value: IrValue::Bool(false),
        }),
        b: Box::new(IrNode::Const {
            value: IrValue::Bool(true),
        }),
    });
    let (_, target) = select_target(
        &registry,
        Vec3::ZERO,
        ENEMY_DEFAULT_FACTION,
        None,
        None,
        Some(&filter),
        &mut CandidateScope::for_validation(),
    );
    assert_eq!(target.expect("live candidate").entity, live);
}

#[test]
fn candidate_filter_is_never_evaluated_for_a_retained_target() {
    let mut registry = EntityRegistry::new();
    let retained = spawn_player(&mut registry, Vec3::new(2.0, 0.0, 0.0));
    let filter = bind_candidate_filter_for_test(IrNode::Gt {
        a: Box::new(IrNode::Input {
            name: CANDIDATE_DISTANCE_INPUT.into(),
            owner: None,
        }),
        b: Box::new(IrNode::Const {
            value: IrValue::Number(100.0),
        }),
    });
    let mut candidate_scope = CandidateScope::for_validation();
    let (_, target) = select_target(
        &registry,
        Vec3::ZERO,
        ENEMY_DEFAULT_FACTION,
        Some(retained),
        None,
        Some(&filter),
        &mut candidate_scope,
    );
    assert_eq!(target.expect("retained candidate stays").entity, retained);
    let distance = candidate_scope
        .resolve_input(CANDIDATE_DISTANCE_INPUT)
        .expect("candidate distance input")
        .handle;
    assert_eq!(
        candidate_scope.read(&distance),
        IrValue::Number(0.0),
        "the untouched validation snapshot proves the retained pawn never entered filter evaluation"
    );
}

#[test]
fn candidate_distance_filter_honors_the_acquisition_boundary() {
    const RANGE: f32 = 10.0;
    const EPSILON: f32 = 0.01;
    let filter = bind_candidate_filter_for_test(IrNode::Le {
        a: Box::new(IrNode::Input {
            name: "@candidate.distance".into(),
            owner: None,
        }),
        b: Box::new(IrNode::Const {
            value: IrValue::Number(RANGE),
        }),
    });
    let mut inside = EntityRegistry::new();
    let inside_pawn = spawn_player(&mut inside, Vec3::new(RANGE - EPSILON, 0.0, 0.0));
    let (_, selected) = select_target(
        &inside,
        Vec3::ZERO,
        ENEMY_DEFAULT_FACTION,
        None,
        None,
        Some(&filter),
        &mut CandidateScope::for_validation(),
    );
    assert_eq!(
        selected.expect("R - epsilon is eligible").entity,
        inside_pawn
    );

    let mut outside = EntityRegistry::new();
    spawn_player(&mut outside, Vec3::new(RANGE + EPSILON, 0.0, 0.0));
    let (_, selected) = select_target(
        &outside,
        Vec3::ZERO,
        ENEMY_DEFAULT_FACTION,
        None,
        None,
        Some(&filter),
        &mut CandidateScope::for_validation(),
    );
    assert!(selected.is_none(), "R + epsilon is ineligible");
}

#[test]
fn faction_seed_is_transparent_and_target_hostility_tracks_a_retained_target() {
    let mut graph = tuning();
    graph.states.get_mut(TEST_IDLE_STATE).unwrap().transitions = vec![edge(
        TEST_ALERT_STATE,
        brain_input(BRAIN_TARGET_HOSTILE_INPUT),
    )];
    graph.interrupts = vec![
        edge(TEST_IDLE_STATE, target_lost()),
        edge(
            TEST_IDLE_STATE,
            IrNode::Select {
                cond: Box::new(brain_input(BRAIN_TARGET_HOSTILE_INPUT)),
                a: Box::new(IrNode::Const {
                    value: IrValue::Bool(false),
                }),
                b: Box::new(IrNode::Const {
                    value: IrValue::Bool(true),
                }),
            },
        ),
    ];
    graph.candidate_filter = None;

    let mut registry = EntityRegistry::new();
    let mut runtime = AiRuntime::new();
    let player = spawn_player(&mut registry, Vec3::new(5.0, 0.0, 0.0));
    let enemy = spawn_enemy(
        &mut registry,
        Vec3::ZERO,
        authored_brain(&graph, TEST_IDLE_STATE),
        50.0,
    );
    assert_eq!(
        registry
            .get_component::<EntityStateComponent>(enemy)
            .expect("spawned enemy carries entity state")
            .get(FACTION_STATE_FIELD),
        ENEMY_DEFAULT_FACTION,
        "the shared enemy assembly helper seeds faction one"
    );

    run_ai_tick(&mut registry, &mut runtime, 0.016);
    assert_eq!(
        enemy_state_name(&registry, enemy),
        TEST_ALERT_STATE,
        "an absent player faction reads zero, so the default enemy faction acquires it as hostile"
    );
    assert_eq!(enemy_acquired_target(&registry, enemy), Some(player));

    // This models a trigger/touch reaction: its faction write happened before
    // the AI compute pass, so the retained-target fact sees it this same tick.
    registry
        .entity_state_mut(player)
        .expect("player carries entity state")
        .set(FACTION_STATE_FIELD, ENEMY_DEFAULT_FACTION);
    run_ai_tick(&mut registry, &mut runtime, 0.016);

    assert_eq!(
        enemy_state_name(&registry, enemy),
        TEST_IDLE_STATE,
        "the retained target is still selected for this tick, but targetHostile lets the authored interrupt stand down"
    );
    assert_eq!(
        enemy_acquired_target(&registry, enemy),
        None,
        "the idle state disengages after the authored targetHostile interrupt"
    );
}

#[test]
fn impact_time_faction_write_reaches_all_brains_on_the_next_tick() {
    let mut graph = tuning();
    graph.interrupts = vec![
        edge(TEST_IDLE_STATE, target_lost()),
        edge(TEST_IDLE_STATE, target_is_not_hostile()),
    ];
    graph.candidate_filter = None;

    let mut registry = EntityRegistry::new();
    let mut runtime = AiRuntime::new();
    let player = spawn_player(&mut registry, Vec3::new(1.0, 0.0, 0.0));
    let first = spawn_enemy(
        &mut registry,
        Vec3::ZERO,
        authored_brain(&graph, TEST_ATTACK_STATE),
        50.0,
    );
    let second = spawn_enemy(
        &mut registry,
        Vec3::new(0.0, 0.0, 1.0),
        authored_brain(&graph, TEST_ATTACK_STATE),
        50.0,
    );

    // AI snapshots every outcome before it enters this apply-pass callback.
    // The first impact makes the player friendly, but both precomputed guards
    // still see the old hostile fact and complete the current attack tick.
    let events = run_ai_tick_with_navigation_and_impact(
        &mut registry,
        &mut runtime,
        0.016,
        None,
        None,
        |registry| {
            registry
                .entity_state_mut(player)
                .expect("player carries entity state")
                .set(FACTION_STATE_FIELD, ENEMY_DEFAULT_FACTION);
        },
    );
    assert_eq!(events, vec![ENEMY_ATTACK_EVENT, ENEMY_ATTACK_EVENT]);
    assert_eq!(enemy_state_name(&registry, first), TEST_ATTACK_STATE);
    assert_eq!(enemy_state_name(&registry, second), TEST_ATTACK_STATE);

    // Next tick's compute pass reads the new faction uniformly. Retention is
    // still available long enough for the authored targetHostile interrupt to
    // stand both brains down; fresh-scan hostility never re-gates retention.
    run_ai_tick(&mut registry, &mut runtime, 0.016);
    assert_eq!(enemy_state_name(&registry, first), TEST_IDLE_STATE);
    assert_eq!(enemy_state_name(&registry, second), TEST_IDLE_STATE);
}

// ---------------------------------------------------------------------------
// Acceptance: direct graph acquisition sets destination; its authored
// stand-down interrupt clears it (via path_state).
// ---------------------------------------------------------------------------

#[test]
fn distance_from_anchor_guard_tracks_spawn_anchor_without_a_selected_target() {
    let mut graph = tuning();
    let beyond_anchor = IrNode::Gt {
        a: Box::new(brain_input(BRAIN_DISTANCE_FROM_ANCHOR_INPUT)),
        b: Box::new(IrNode::Const {
            value: IrValue::Number(3.0),
        }),
    };
    graph
        .states
        .get_mut(TEST_IDLE_STATE)
        .expect("fixture declares idle")
        .transitions = vec![edge(TEST_ALERT_STATE, beyond_anchor)];
    graph.interrupts.clear();
    graph.candidate_filter = None;

    let mut reg = EntityRegistry::new();
    let mut runtime = AiRuntime::new();
    let enemy = spawn_enemy(
        &mut reg,
        Vec3::new(10.0, 7.0, -4.0),
        brain_with(graph, TEST_IDLE_STATE),
        50.0,
    );

    // No player exists: the fact must still read zero at the spawn anchor.
    run_ai_tick(&mut reg, &mut runtime, 0.016);
    assert_eq!(enemy_state_name(&reg, enemy), TEST_IDLE_STATE);
    assert_eq!(enemy_acquired_target(&reg, enemy), None);

    // An external transform move leaves the spawn anchor fixed. Y does not
    // contribute, so this is exactly four XZ units from the original anchor.
    let mut transform = *reg
        .get_component::<Transform>(enemy)
        .expect("enemy has a transform");
    transform.position = Vec3::new(14.0, 99.0, -4.0);
    reg.set_component(enemy, transform)
        .expect("enemy remains live");
    run_ai_tick(&mut reg, &mut runtime, 0.016);

    assert_eq!(enemy_state_name(&reg, enemy), TEST_ALERT_STATE);
    assert_eq!(enemy_acquired_target(&reg, enemy), None);
}

#[test]
fn direct_graph_acquisition_sets_destination_and_authored_stand_down_clears_it() {
    let mut reg = EntityRegistry::new();
    let mut warned = AiRuntime::new();

    // A short authored aggro range (8) keeps the stand-down tick in the near
    // stride band, isolating the graph interrupt from stride gating.
    let enemy = spawn_enemy(
        &mut reg,
        Vec3::ZERO,
        brain_with(test_graph_with(18.0, 8.0), TEST_IDLE_STATE),
        50.0,
    );

    // Player crosses into detection range (5 units away): the tick must set the
    // agent destination to the player. Assert via the path_state read.
    let pawn = spawn_player(&mut reg, Vec3::new(5.0, 0.0, 0.0));
    run_ai_tick(&mut reg, &mut warned, 0.016);
    assert_eq!(enemy_state_name(&reg, enemy), TEST_ALERT_STATE);
    assert_eq!(
        enemy_acquired_target(&reg, enemy),
        Some(pawn),
        "detection persists the acquired target identity",
    );
    assert!(
        agent_steering::path_state(&reg, enemy)
            .expect("agent present")
            .has_destination,
        "detection must set a destination",
    );

    // Player exceeds the graph's authored aggro range (10 > 8): the interrupt
    // must clear the destination.
    let mut t = *reg.get_component::<Transform>(pawn).unwrap();
    t.position = Vec3::new(10.0, 0.0, 0.0);
    reg.set_component(pawn, t).unwrap();
    run_ai_tick(&mut reg, &mut warned, 0.016);
    assert_eq!(enemy_state_name(&reg, enemy), TEST_IDLE_STATE);
    assert_eq!(
        enemy_acquired_target(&reg, enemy),
        None,
        "the authored stand-down clears the retained target identity",
    );
    assert!(
        !agent_steering::path_state(&reg, enemy)
            .expect("agent present")
            .has_destination,
        "the authored stand-down must clear the destination",
    );
}

#[test]
fn direct_graph_candidate_filter_rejects_a_pawn_outside_its_authored_range() {
    let mut reg = EntityRegistry::new();
    let mut warned = AiRuntime::new();
    let enemy = spawn_enemy(
        &mut reg,
        Vec3::ZERO,
        brain_with(test_graph_with(18.0, 8.0), TEST_IDLE_STATE),
        50.0,
    );

    // This pawn is inside detection but outside the graph's candidate filter.
    spawn_player(&mut reg, Vec3::new(10.0, 0.0, 0.0));

    for tick in 0..16 {
        run_ai_tick(&mut reg, &mut warned, 0.016);
        assert_eq!(
            enemy_state_name(&reg, enemy),
            TEST_IDLE_STATE,
            "tick {tick}: an ineligible candidate must not move the resting brain",
        );
        assert_eq!(
            enemy_acquired_target(&reg, enemy),
            None,
            "tick {tick}: the graph's candidate filter bounds fresh acquisition",
        );
        assert!(
            !agent_steering::path_state(&reg, enemy)
                .expect("agent present")
                .has_destination,
            "tick {tick}: an unacquired candidate must not request a destination",
        );
    }
}

// ---------------------------------------------------------------------------
// Acceptance: host-authoritative aggro gate holds an enemy in place.
// ---------------------------------------------------------------------------

#[test]
fn sealed_enemy_does_not_acquire_adjacent_player_or_write_transform() {
    let mut reg = EntityRegistry::new();
    let mut warned = AiRuntime::new();
    let start = Transform {
        position: Vec3::ZERO,
        rotation: glam::Quat::from_rotation_y(0.7),
        ..Transform::default()
    };
    let mut brain = brain_with(tuning(), TEST_IDLE_STATE);
    brain.aggro_armed = false;
    let enemy = spawn_enemy(&mut reg, start.position, brain, 50.0);
    reg.set_component(enemy, start).unwrap();
    spawn_player(&mut reg, Vec3::new(1.0, 0.0, 0.0));

    // Detection does not currently require line of sight. Keeping the player
    // adjacent here proves the sealed gate also prevents the thin-wall case.
    run_ai_tick(&mut reg, &mut warned, 0.016);

    assert_eq!(enemy_state_name(&reg, enemy), TEST_IDLE_STATE);
    assert_eq!(enemy_acquired_target(&reg, enemy), None);
    assert!(
        !agent_steering::path_state(&reg, enemy)
            .expect("agent present")
            .has_destination,
        "a sealed brain must not issue a chase destination",
    );
    assert_eq!(
        *reg.get_component::<Transform>(enemy).unwrap(),
        start,
        "the AI tick must preserve a sealed enemy's placement and facing",
    );
    assert_eq!(enemy_animation(&reg, enemy), "idle");
}

#[test]
fn opening_sealed_enemy_allows_pursuit_on_next_think_tick() {
    let mut reg = EntityRegistry::new();
    let mut warned = AiRuntime::new();
    let mut brain = brain_with(tuning(), TEST_IDLE_STATE);
    brain.aggro_armed = false;
    let enemy = spawn_enemy(&mut reg, Vec3::ZERO, brain, 50.0);
    let pawn = spawn_player(&mut reg, Vec3::new(5.0, 0.0, 0.0));

    run_ai_tick(&mut reg, &mut warned, 0.016);
    set_enemy_aggro_armed(&mut reg, enemy, true);
    run_ai_tick(&mut reg, &mut warned, 0.016);

    assert_eq!(enemy_state_name(&reg, enemy), TEST_ALERT_STATE);
    assert_eq!(enemy_acquired_target(&reg, enemy), Some(pawn));
    assert!(
        agent_steering::path_state(&reg, enemy)
            .expect("agent present")
            .has_destination,
        "opening the gate should let the next normal think issue pursuit",
    );
}

#[test]
fn closing_mid_chase_clears_steering_and_holds_idempotently() {
    let mut reg = EntityRegistry::new();
    let mut warned = AiRuntime::new();
    let enemy = spawn_enemy(
        &mut reg,
        Vec3::ZERO,
        brain_with(tuning(), TEST_IDLE_STATE),
        50.0,
    );
    spawn_player(&mut reg, Vec3::new(5.0, 0.0, 0.0));
    run_ai_tick(&mut reg, &mut warned, 0.016);
    set_enemy_yaw(&mut reg, enemy, 0.9);
    set_agent_velocity(&mut reg, enemy, Vec3::X);
    let held_transform = *reg.get_component::<Transform>(enemy).unwrap();

    set_enemy_aggro_armed(&mut reg, enemy, false);
    run_ai_tick(&mut reg, &mut warned, 0.016);
    let first_closed_animation = reg.get_component::<MeshComponent>(enemy).unwrap().clone();

    assert_eq!(enemy_state_name(&reg, enemy), TEST_IDLE_STATE);
    assert_eq!(enemy_acquired_target(&reg, enemy), None);
    assert!(
        !agent_steering::path_state(&reg, enemy)
            .expect("agent present")
            .has_destination,
    );
    assert_eq!(enemy_animation(&reg, enemy), "idle");
    assert_eq!(
        *reg.get_component::<Transform>(enemy).unwrap(),
        held_transform
    );

    run_ai_tick(&mut reg, &mut warned, 0.016);
    assert_eq!(
        reg.get_component::<MeshComponent>(enemy).unwrap(),
        &first_closed_animation,
        "a repeatedly closed gate must not keep re-writing Idle animation state",
    );
    assert_eq!(
        *reg.get_component::<Transform>(enemy).unwrap(),
        held_transform
    );
}

#[test]
fn gated_zero_hp_enemy_remains_present_and_idle() {
    let mut reg = EntityRegistry::new();
    let mut warned = AiRuntime::new();
    let mut brain = brain_with(tuning(), TEST_IDLE_STATE);
    brain.aggro_armed = false;
    let enemy = spawn_enemy(&mut reg, Vec3::ZERO, brain, 0.0);
    spawn_player(&mut reg, Vec3::new(1.0, 0.0, 0.0));

    for _ in 0..3 {
        run_ai_tick(&mut reg, &mut warned, 1.0);
    }

    assert_eq!(enemy_state_name(&reg, enemy), TEST_IDLE_STATE);
    assert_eq!(enemy_acquired_target(&reg, enemy), None);
    assert!(
        !agent_steering::path_state(&reg, enemy)
            .expect("agent present")
            .has_destination,
    );
    assert!(reg.exists(enemy), "zero HP must not despawn a gated brain");
}

#[test]
fn retained_target_is_consumed_for_chase_when_other_pawn_is_only_slightly_nearer() {
    let mut reg = EntityRegistry::new();
    let mut warned = AiRuntime::new();

    let retained = spawn_player(&mut reg, Vec3::new(10.0, 0.0, 0.0));
    spawn_player(&mut reg, Vec3::new(9.5, 0.0, 0.0));
    let mut brain = brain_with(tuning(), TEST_ALERT_STATE);
    brain.acquired_target = Some(retained);
    let enemy = spawn_enemy(&mut reg, Vec3::ZERO, brain, 50.0);

    run_ai_tick(&mut reg, &mut warned, 0.016);

    assert_eq!(
        enemy_acquired_target(&reg, enemy),
        Some(retained),
        "the retained target identity stays persisted while engaged",
    );
    assert_eq!(enemy_state_name(&reg, enemy), TEST_ALERT_STATE);
    let path = agent_steering::path_state(&reg, enemy).expect("agent present");
    assert!(path.has_destination, "engaged target sets steering");
    assert!(
        (path.distance_to_destination - 10.0).abs() < EPS,
        "steering should consume the retained pawn position, not the slightly \
         nearer candidate (distance was {})",
        path.distance_to_destination,
    );
}

#[test]
fn off_stride_retained_target_does_not_switch_to_meaningfully_closer_pawn() {
    let mut reg = EntityRegistry::new();
    let mut warned = AiRuntime::new();

    let retained = spawn_player(&mut reg, Vec3::new(35.0, 0.0, 0.0));
    let closer = spawn_player(&mut reg, Vec3::new(1.0, 0.0, 0.0));
    let mut brain = brain_with(test_graph_with(18.0, 60.0), TEST_ALERT_STATE);
    brain.acquired_target = Some(retained);
    let enemy = spawn_enemy(&mut reg, Vec3::ZERO, brain, 50.0);

    run_ai_tick(&mut reg, &mut warned, 0.016);

    assert_eq!(
        enemy_acquired_target(&reg, enemy),
        Some(retained),
        "off-stride engaged enemies must consume the retained target instead of reranking",
    );
    assert_ne!(enemy_acquired_target(&reg, enemy), Some(closer));
    let path = agent_steering::path_state(&reg, enemy).expect("agent present");
    assert!(
        path.has_destination,
        "retained target keeps steering active"
    );
    assert!(
        (path.distance_to_destination - 35.0).abs() < EPS,
        "steering should keep chasing the retained pawn on a non-acquisition tick"
    );
}

#[test]
fn idle_brain_ignores_stale_retained_target_when_acquiring() {
    let mut reg = EntityRegistry::new();
    let mut warned = AiRuntime::new();

    let stale_far = spawn_player(&mut reg, Vec3::new(25.0, 0.0, 0.0));
    let near = spawn_player(&mut reg, Vec3::new(5.0, 0.0, 0.0));
    let mut brain = brain_with(tuning(), TEST_IDLE_STATE);
    brain.acquired_target = Some(stale_far);
    let enemy = spawn_enemy(&mut reg, Vec3::ZERO, brain, 50.0);

    run_ai_tick(&mut reg, &mut warned, 0.016);

    assert_eq!(enemy_state_name(&reg, enemy), TEST_ALERT_STATE);
    assert_eq!(
        enemy_acquired_target(&reg, enemy),
        Some(near),
        "idle acquisition should rank fresh targets instead of honoring stale retained state",
    );
}

#[test]
fn retained_target_beyond_the_authored_range_stands_down_instead_of_switching() {
    let mut reg = EntityRegistry::new();
    let mut warned = AiRuntime::new();

    let retained = spawn_player(&mut reg, Vec3::new(20.0, 0.0, 0.0));
    let replacement = spawn_player(&mut reg, Vec3::new(35.0, 0.0, 0.0));
    let mut brain = brain_with(test_graph_with(40.0, 10.0), TEST_ALERT_STATE);
    brain.acquired_target = Some(retained);
    brain.think_stride_counter = 3; // 20 units is mid band: 4 % 4 == acquisition tick.
    let enemy = spawn_enemy(&mut reg, Vec3::ZERO, brain, 50.0);
    agent_steering::set_destination(&mut reg, enemy, Vec3::new(20.0, 0.0, 0.0));

    run_ai_tick(&mut reg, &mut warned, 0.016);

    assert_eq!(
        enemy_state_name(&reg, enemy),
        TEST_IDLE_STATE,
        "the graph's stand-down interrupt should drop aggro when no replacement is eligible"
    );
    assert_eq!(
        enemy_acquired_target(&reg, enemy),
        None,
        "an out-of-range replacement must not be persisted as the new target"
    );
    assert_ne!(enemy_acquired_target(&reg, enemy), Some(replacement));
    assert!(
        !agent_steering::path_state(&reg, enemy)
            .expect("agent present")
            .has_destination,
        "dropping aggro clears stale steering"
    );
}

#[test]
fn retained_target_beyond_the_authored_range_clears_stale_destination_off_stride() {
    let mut reg = EntityRegistry::new();
    let mut warned = AiRuntime::new();

    let retained = spawn_player(&mut reg, Vec3::new(35.0, 0.0, 0.0));
    let mut brain = brain_with(test_graph_with(40.0, 10.0), TEST_ALERT_STATE);
    brain.acquired_target = Some(retained);
    brain.think_stride_counter = 0; // 35 units is far band: 1 % 12 is off-stride.
    let enemy = spawn_enemy(&mut reg, Vec3::ZERO, brain, 50.0);
    agent_steering::set_destination(&mut reg, enemy, Vec3::new(-20.0, 0.0, 0.0));

    run_ai_tick(&mut reg, &mut warned, 0.016);

    assert_eq!(
        enemy_state_name(&reg, enemy),
        TEST_IDLE_STATE,
        "the graph interrupt should stand down immediately, not wait for target acquisition stride"
    );
    assert_eq!(
        enemy_acquired_target(&reg, enemy),
        None,
        "the graph interrupt clears the retained target identity immediately"
    );
    assert!(
        !agent_steering::path_state(&reg, enemy)
            .expect("agent present")
            .has_destination,
        "the graph interrupt must clear stale steering destinations even off-stride"
    );
}

#[test]
fn out_of_range_retained_target_still_prices_the_think_stride() {
    let mut reg = EntityRegistry::new();
    let mut warned = AiRuntime::new();
    let retained_position = Vec3::new(35.0, 0.0, 0.0);
    let retained = spawn_player(&mut reg, retained_position);
    let in_range_replacement = spawn_player(&mut reg, Vec3::new(5.0, 0.0, 0.0));
    let mut brain = brain_with(test_graph_with(40.0, 10.0), TEST_ALERT_STATE);
    brain.acquired_target = Some(retained);
    brain.think_stride_counter = 0;

    let retained_distance = distance_xz(retained_position, Vec3::ZERO);
    let stride = think_stride_for_distance(retained_distance);
    assert!(
        stride > 1,
        "the retained target must use a strided distance band"
    );
    assert_ne!(
        brain.think_stride_counter.wrapping_add(1) % stride,
        0,
        "the first tick must be off-stride for the retained target's distance band",
    );
    assert!(
        !acquisition_due(&brain, Some(retained_distance)),
        "the first tick must be off-stride according to the retained target's distance",
    );

    let enemy = spawn_enemy(&mut reg, Vec3::ZERO, brain, 50.0);
    run_ai_tick(&mut reg, &mut warned, 0.016);

    assert_eq!(
        enemy_acquired_target(&reg, enemy),
        None,
        "the off-stride graph stand-down must not replace the retained target yet",
    );
    assert_ne!(
        enemy_acquired_target(&reg, enemy),
        Some(in_range_replacement),
        "the nearer eligible pawn proves that an empty stride distance would have replaced it",
    );
}

// ---------------------------------------------------------------------------
// Acceptance: damage exactly once per cooldown via the chokepoint
// ---------------------------------------------------------------------------

#[test]
fn attack_applies_configured_damage_once_per_cooldown() {
    let mut reg = EntityRegistry::new();
    let mut warned = AiRuntime::new();

    // Player inside attack range (1 unit). Enemy idle → detection puts it in
    // attack this tick (already in attack range), cooldown ready → one hit.
    let pawn = spawn_player(&mut reg, Vec3::new(1.0, 0.0, 0.0));
    let enemy = spawn_enemy(
        &mut reg,
        Vec3::ZERO,
        brain_with(tuning(), TEST_IDLE_STATE),
        50.0,
    );

    // dt = 0.1s = 100ms; cooldown is 1000ms → ~10 ticks between hits.
    let dt = 0.1;

    // Tick 1: attacks once (8 damage), arms cooldown to 1000ms.
    let events = run_ai_tick(&mut reg, &mut warned, dt);
    assert_eq!(events, vec![ENEMY_ATTACK_EVENT]);
    assert_eq!(player_hp(&reg, pawn), 92.0, "one hit lands");
    {
        let health = reg.get_component::<HealthComponent>(pawn).unwrap();
        let entries = health.contributor_ledger.entries();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].source_id, "enemy.attack");
        assert_eq!(entries[0].accumulated_damage, 8.0);
        assert_eq!(entries[0].hit_count, 1);
        assert_eq!(entries[0].last_hit_damage, 8.0);
        assert_eq!(entries[0].last_attacker, Some(enemy));
        assert_eq!(entries[0].last_weapon, None);
        assert_eq!(entries[0].last_hit_zone, None);
    }

    // Next ticks: still in attack range but cooldown not elapsed → no further
    // damage. Each tick subtracts 100ms first; from the armed 1000ms it takes 10
    // subtractions to reach 0. Ticks 2..=10 (9 ticks) leave remaining 900..100.
    for _ in 0..9 {
        let events = run_ai_tick(&mut reg, &mut warned, dt);
        assert!(events.is_empty(), "no attack during cooldown");
    }
    assert_eq!(player_hp(&reg, pawn), 92.0, "no damage during cooldown");

    // Tick 11: the 10th subtraction brings remaining to 0 → the next attack
    // lands exactly once and re-arms the cooldown.
    let events = run_ai_tick(&mut reg, &mut warned, dt);
    assert_eq!(
        events,
        vec![ENEMY_ATTACK_EVENT],
        "attack resumes after cooldown"
    );
    assert_eq!(player_hp(&reg, pawn), 84.0, "second hit lands once");
    {
        let health = reg.get_component::<HealthComponent>(pawn).unwrap();
        let entries = health.contributor_ledger.entries();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].source_id, "enemy.attack");
        assert_eq!(entries[0].accumulated_damage, 16.0);
        assert_eq!(entries[0].hit_count, 2);
        assert_eq!(entries[0].last_hit_damage, 8.0);
        assert_eq!(entries[0].last_attacker, Some(enemy));
    }
}

#[test]
fn attack_does_not_damage_remote_health_when_marked_local_pawn_lacks_health() {
    let mut reg = EntityRegistry::new();
    let mut warned = AiRuntime::new();

    let remote = spawn_player(&mut reg, Vec3::new(100.0, 0.0, 0.0));
    let local = spawn_player_without_health(&mut reg, Vec3::new(1.0, 0.0, 0.0));
    reg.mark_local_player_pawn(local).unwrap();
    spawn_enemy(
        &mut reg,
        Vec3::ZERO,
        brain_with(tuning(), TEST_ATTACK_STATE),
        50.0,
    );

    let events = run_ai_tick(&mut reg, &mut warned, 1.0 / 60.0);

    assert!(
        events.is_empty(),
        "enemy should target the nearest pawn's position but not attack a different pawn's health"
    );
    assert_eq!(
        player_hp(&reg, remote),
        100.0,
        "remote pawn health must not be used as fallback damage target"
    );
}

#[test]
fn attack_damages_selected_remote_target_not_marked_local_pawn() {
    let mut reg = EntityRegistry::new();
    let mut warned = AiRuntime::new();

    let local = spawn_player(&mut reg, Vec3::new(100.0, 0.0, 0.0));
    let remote = spawn_player(&mut reg, Vec3::new(1.0, 0.0, 0.0));
    reg.mark_local_player_pawn(local).unwrap();
    spawn_enemy(
        &mut reg,
        Vec3::ZERO,
        brain_with(tuning(), TEST_IDLE_STATE),
        50.0,
    );

    let events = run_ai_tick(&mut reg, &mut warned, 0.1);

    assert_eq!(
        events,
        vec![ENEMY_ATTACK_EVENT],
        "the selected remote pawn is alive and in attack range"
    );
    assert_eq!(
        player_hp(&reg, remote),
        92.0,
        "damage lands on the selected remote pawn"
    );
    assert_eq!(
        player_hp(&reg, local),
        100.0,
        "marked/local pawn health must not receive another pawn's attack"
    );
}

#[test]
fn selected_dead_target_suppresses_attack_even_when_other_pawn_is_alive() {
    let mut reg = EntityRegistry::new();
    let mut warned = AiRuntime::new();

    let selected_dead = spawn_player(&mut reg, Vec3::new(1.0, 0.0, 0.0));
    set_hp(&mut reg, selected_dead, 0.0);
    let other_alive = spawn_player(&mut reg, Vec3::new(1.5, 0.0, 0.0));
    spawn_enemy(
        &mut reg,
        Vec3::ZERO,
        brain_with(tuning(), TEST_IDLE_STATE),
        50.0,
    );

    let events = run_ai_tick(&mut reg, &mut warned, 0.1);

    assert!(
        events.is_empty(),
        "attack/event generation is gated by the selected target's live Health"
    );
    assert_eq!(
        player_hp(&reg, selected_dead),
        0.0,
        "dead selected target remains at zero HP"
    );
    assert_eq!(
        player_hp(&reg, other_alive),
        100.0,
        "another live pawn must not be damaged for the selected dead target"
    );
}

#[test]
fn no_damage_when_player_below_attack_range() {
    let mut reg = EntityRegistry::new();
    let mut warned = AiRuntime::new();

    // Player at 10 units: inside detection, outside attack range → no damage.
    let pawn = spawn_player(&mut reg, Vec3::new(10.0, 0.0, 0.0));
    spawn_enemy(
        &mut reg,
        Vec3::ZERO,
        brain_with(tuning(), TEST_IDLE_STATE),
        50.0,
    );

    let events = run_ai_tick(&mut reg, &mut warned, 0.1);
    assert!(events.is_empty(), "no attack event out of range");
    assert_eq!(player_hp(&reg, pawn), 100.0, "no damage out of range");
}

#[test]
fn no_attack_or_event_when_player_already_dead() {
    let mut reg = EntityRegistry::new();
    let mut warned = AiRuntime::new();

    // Player inside attack range but already dead (HP 0, still present — the
    // respawn flow is owned elsewhere). The enemy must not keep swinging at it
    // or spamming the attack event.
    let pawn = spawn_player(&mut reg, Vec3::new(1.0, 0.0, 0.0));
    reg.set_component(
        pawn,
        HealthComponent {
            max: 100.0,
            current: 0.0,
            hitbox: None,
            death_handled: false,
            pending_kill_credit: None,
            zone_multipliers: std::collections::HashMap::new(),
            contributor_ledger: Default::default(),
        },
    )
    .unwrap();
    let enemy = spawn_enemy(
        &mut reg,
        Vec3::ZERO,
        brain_with(tuning(), TEST_IDLE_STATE),
        50.0,
    );

    for _ in 0..5 {
        let events = run_ai_tick(&mut reg, &mut warned, 0.1);
        assert!(events.is_empty(), "no attack event against a dead player");
        assert_eq!(player_hp(&reg, pawn), 0.0, "a dead player takes no damage");
        assert_eq!(
            enemy_acquired_target(&reg, enemy),
            Some(pawn),
            "selection remains aliveness-free; only the attack gate rejects the pawn"
        );
    }
}

// ---------------------------------------------------------------------------
// Acceptance: each direct graph state selects its declared animation name
// ---------------------------------------------------------------------------

#[test]
fn each_direct_graph_state_switches_to_its_declared_animation() {
    let mut reg = EntityRegistry::new();
    let mut warned = AiRuntime::new();

    let enemy = spawn_enemy(
        &mut reg,
        Vec3::ZERO,
        brain_with(tuning(), TEST_IDLE_STATE),
        50.0,
    );
    set_agent_velocity(&mut reg, enemy, Vec3::new(1.0, 0.0, 0.0));
    let pawn = spawn_player(&mut reg, Vec3::new(10.0, 0.0, 0.0));

    // idle starts as the mesh default; entering alert selects "locomotion".
    run_ai_tick(&mut reg, &mut warned, 0.016);
    assert_eq!(enemy_state_name(&reg, enemy), TEST_ALERT_STATE);
    assert_eq!(enemy_animation(&reg, enemy), "locomotion");

    // Move the player into attack range → attack selects "attack".
    let mut t = *reg.get_component::<Transform>(pawn).unwrap();
    t.position = Vec3::new(1.0, 0.0, 0.0);
    reg.set_component(pawn, t).unwrap();
    run_ai_tick(&mut reg, &mut warned, 0.016);
    assert_eq!(enemy_state_name(&reg, enemy), TEST_ATTACK_STATE);
    assert_eq!(enemy_animation(&reg, enemy), "attack");

    // Player exceeds the graph's authored aggro range. The ordered interrupt
    // stands attack down directly to idle instead of spending an intermediate
    // alert tick chasing a stale destination.
    let mut t = *reg.get_component::<Transform>(pawn).unwrap();
    t.position = Vec3::new(30.0, 0.0, 0.0);
    reg.set_component(pawn, t).unwrap();
    run_ai_tick(&mut reg, &mut warned, 0.016);
    assert_eq!(enemy_state_name(&reg, enemy), TEST_IDLE_STATE);
    assert_eq!(enemy_animation(&reg, enemy), "idle");
}

#[test]
fn unmapped_animation_warns_once_and_keeps_prior_state() {
    // The direct graph maps alert→"locomotion" but the mesh does NOT declare
    // it: the switch fails, the prior animation is kept, and the warn latch
    // records the name exactly once.
    let mut reg = EntityRegistry::new();
    let mut warned = AiRuntime::new();

    let mut graph = tuning();
    graph.states.get_mut(TEST_ALERT_STATE).unwrap().animation = "missing_clip".into();
    let enemy = spawn_enemy(
        &mut reg,
        Vec3::ZERO,
        brain_with(graph, TEST_IDLE_STATE),
        50.0,
    );
    set_agent_velocity(&mut reg, enemy, Vec3::new(1.0, 0.0, 0.0));
    spawn_player(&mut reg, Vec3::new(10.0, 0.0, 0.0));

    run_ai_tick(&mut reg, &mut warned, 0.016);
    assert_eq!(
        enemy_state_name(&reg, enemy),
        TEST_ALERT_STATE,
        "direct graph state still advances"
    );
    assert_eq!(
        enemy_animation(&reg, enemy),
        "idle",
        "failed switch keeps the prior animation",
    );
    assert!(
        warned.warned.contains("anim:missing_clip"),
        "warn latch records the namespaced animation name",
    );
    assert_eq!(warned.warned.len(), 1, "exactly one distinct name warned");
}

#[test]
fn stationary_alert_selects_idle_animation_and_latches_stopped() {
    let mut reg = EntityRegistry::new();
    let mut warned = AiRuntime::new();

    spawn_player(&mut reg, Vec3::new(10.0, 0.0, 0.0));
    let mut brain = brain_with(tuning(), TEST_ALERT_STATE);
    brain.locomotion_moving = true;
    let enemy = spawn_enemy(&mut reg, Vec3::ZERO, brain, 50.0);
    set_enemy_animation(&mut reg, enemy, "locomotion");
    set_agent_velocity(&mut reg, enemy, Vec3::ZERO);

    run_ai_tick(&mut reg, &mut warned, 0.016);

    assert_eq!(enemy_state_name(&reg, enemy), TEST_ALERT_STATE);
    assert_eq!(
        enemy_animation(&reg, enemy),
        "idle",
        "stationary Alert uses the idle animation",
    );
    assert!(
        !enemy_locomotion_moving(&reg, enemy),
        "the stopped latch is persisted after the animation block",
    );
}

#[test]
fn alert_locomotion_stop_and_resume_switch_once_per_intent_change() {
    let mut reg = EntityRegistry::new();
    let mut warned = AiRuntime::new();

    spawn_player(&mut reg, Vec3::new(10.0, 0.0, 0.0));
    let mut brain = brain_with(tuning(), TEST_ALERT_STATE);
    brain.locomotion_moving = true;
    let enemy = spawn_enemy(&mut reg, Vec3::ZERO, brain, 50.0);
    set_enemy_animation(&mut reg, enemy, "locomotion");

    set_agent_velocity(&mut reg, enemy, Vec3::ZERO);
    run_ai_tick(&mut reg, &mut warned, 0.016);
    assert_eq!(enemy_animation(&reg, enemy), "idle");
    assert!(!enemy_locomotion_moving(&reg, enemy));
    let stopped_entered_at = enemy_anim_entered_at(&reg, enemy);

    run_ai_tick(&mut reg, &mut warned, 0.016);
    assert_eq!(enemy_animation(&reg, enemy), "idle");
    assert!(!enemy_locomotion_moving(&reg, enemy));
    assert_eq!(
        enemy_anim_entered_at(&reg, enemy),
        stopped_entered_at,
        "unchanged stopped intent does not re-stamp the idle switch",
    );

    set_agent_velocity(&mut reg, enemy, Vec3::new(1.0, 0.0, 0.0));
    run_ai_tick(&mut reg, &mut warned, 0.016);
    assert_eq!(enemy_animation(&reg, enemy), "locomotion");
    assert!(enemy_locomotion_moving(&reg, enemy));
    let moving_entered_at = enemy_anim_entered_at(&reg, enemy);

    run_ai_tick(&mut reg, &mut warned, 0.016);
    assert_eq!(enemy_animation(&reg, enemy), "locomotion");
    assert!(enemy_locomotion_moving(&reg, enemy));
    assert_eq!(
        enemy_anim_entered_at(&reg, enemy),
        moving_entered_at,
        "unchanged moving intent does not re-stamp the walk switch",
    );
}

#[test]
fn unresolved_locomotion_switch_still_persists_latch() {
    let mut reg = EntityRegistry::new();
    let mut warned = AiRuntime::new();

    let mut graph = tuning();
    graph.states.get_mut(TEST_ALERT_STATE).unwrap().animation = "missing_clip".into();
    spawn_player(&mut reg, Vec3::new(10.0, 0.0, 0.0));
    let enemy = spawn_enemy(
        &mut reg,
        Vec3::ZERO,
        brain_with(graph, TEST_ALERT_STATE),
        50.0,
    );
    set_agent_velocity(&mut reg, enemy, Vec3::new(1.0, 0.0, 0.0));

    run_ai_tick(&mut reg, &mut warned, 0.016);
    assert_eq!(
        enemy_animation(&reg, enemy),
        "idle",
        "unresolved walk switch keeps the prior animation",
    );
    assert!(
        enemy_locomotion_moving(&reg, enemy),
        "moving latch persists even when the switch is unresolved",
    );
    assert!(warned.warned.contains("anim:missing_clip"));

    run_ai_tick(&mut reg, &mut warned, 0.016);
    assert!(
        enemy_locomotion_moving(&reg, enemy),
        "unchanged moving intent remains latched on the following tick",
    );
    assert_eq!(
        warned.warned.len(),
        1,
        "unchanged unresolved locomotion does not add further warnings",
    );
}

// ---------------------------------------------------------------------------
// Acceptance: stride gating does not suppress in-stride attacks
// ---------------------------------------------------------------------------

#[test]
fn near_enemy_evaluates_detection_every_tick() {
    // A near enemy (within STRIDE_NEAR_DISTANCE) uses stride 1: detection is
    // evaluated on the very first tick after the player appears.
    let mut reg = EntityRegistry::new();
    let mut warned = AiRuntime::new();

    let enemy = spawn_enemy(
        &mut reg,
        Vec3::ZERO,
        brain_with(tuning(), TEST_IDLE_STATE),
        50.0,
    );
    // 5 units: near band, inside detection.
    spawn_player(&mut reg, Vec3::new(5.0, 0.0, 0.0));

    run_ai_tick(&mut reg, &mut warned, 0.016);
    assert_eq!(
        enemy_state_name(&reg, enemy),
        TEST_ALERT_STATE,
        "near enemy acquires on the first tick",
    );
}

#[test]
fn distant_enemy_strides_detection_but_attack_still_fires() {
    // A distant enemy (far band, stride 12) does NOT re-acquire detection every
    // tick. The attack-in-range/cooldown check still runs every tick, even in a
    // mid-stride gap.

    // 1) Stride-gated detection: a far enemy in IDLE with the player far (but
    // inside detection) does NOT flip to alert on the first (non-think) tick.
    {
        let mut reg = EntityRegistry::new();
        let mut warned = AiRuntime::new();
        // Detection and candidate eligibility are both wide enough to include
        // a far-band player.
        let enemy = spawn_enemy(
            &mut reg,
            Vec3::ZERO,
            brain_with(test_graph_with(40.0, 41.0), TEST_IDLE_STATE),
            50.0,
        );
        // 35 units: far band (> STRIDE_MID_DISTANCE 30), inside detection 40.
        spawn_player(&mut reg, Vec3::new(35.0, 0.0, 0.0));

        // think_stride_counter starts at 0 → becomes 1 after the first tick;
        // 1 % 12 != 0 so acquisition is gated OFF this tick → stays idle.
        run_ai_tick(&mut reg, &mut warned, 0.016);
        assert_eq!(
            enemy_state_name(&reg, enemy),
            TEST_IDLE_STATE,
            "far enemy's detection is strided: no acquire on a non-think tick",
        );
    }

    // 2) Zero HP does not force a direct graph into a death state, even on a
    // non-think tick. Seed the already-engaged remote pawn so the graph is
    // exercising its retained-target path rather than standing down because a
    // fresh acquisition is strided off.
    {
        let mut reg = EntityRegistry::new();
        let mut warned = AiRuntime::new();
        let pawn = spawn_player(&mut reg, Vec3::new(35.0, 0.0, 0.0));
        let mut brain = brain_with(test_graph_with(40.0, 41.0), TEST_ALERT_STATE);
        brain.acquired_target = Some(pawn);
        let enemy = spawn_enemy(&mut reg, Vec3::ZERO, brain, 0.0);
        run_ai_tick(&mut reg, &mut warned, 0.016);
        assert_eq!(
            enemy_state_name(&reg, enemy),
            TEST_ALERT_STATE,
            "zero HP leaves the brain's normal FSM state untouched",
        );
    }

    // 3) In-stride ATTACK still fires: a far-positioned enemy ALREADY in attack
    // state with the player within attack range damages on a non-think tick
    // (attack-range + cooldown are not strided). Here the enemy sits at origin
    // and the player is at attack range, but we force the far stride by starting
    // the counter such that acquisition is gated; the attack check ignores that.
    {
        let mut reg = EntityRegistry::new();
        let mut warned = AiRuntime::new();
        let pawn = spawn_player(&mut reg, Vec3::new(1.0, 0.0, 0.0));
        // Enemy in Attack already, cooldown ready, player in attack range.
        let mut brain = brain_with(tuning(), TEST_ATTACK_STATE);
        // Counter at 5 → after increment 6; 6 % stride(near=1) == 0 anyway, but
        // the attack path does not depend on the acquisition gate at all.
        brain.think_stride_counter = 5;
        spawn_enemy(&mut reg, Vec3::ZERO, brain, 50.0);

        let events = run_ai_tick(&mut reg, &mut warned, 0.1);
        assert_eq!(events, vec![ENEMY_ATTACK_EVENT]);
        assert_eq!(player_hp(&reg, pawn), 92.0, "in-range attack fires");
    }
}

#[test]
fn no_player_pawn_leaves_enemy_idle_and_clears_steering() {
    // With no player pawn, the tick still runs: the enemy resolves to idle and
    // any stale destination is cleared.
    let mut reg = EntityRegistry::new();
    let mut warned = AiRuntime::new();

    let enemy = spawn_enemy(
        &mut reg,
        Vec3::ZERO,
        brain_with(tuning(), TEST_ALERT_STATE),
        50.0,
    );
    // Pre-seed a destination so we can observe it being cleared.
    agent_steering::set_destination(&mut reg, enemy, Vec3::new(5.0, 0.0, 0.0));

    let events = run_ai_tick(&mut reg, &mut warned, 0.016);
    assert!(events.is_empty());
    assert_eq!(enemy_state_name(&reg, enemy), TEST_IDLE_STATE);
    assert_eq!(
        enemy_acquired_target(&reg, enemy),
        None,
        "no target clears any retained target identity",
    );
    assert!(
        !agent_steering::path_state(&reg, enemy)
            .expect("agent present")
            .has_destination,
        "no pawn clears any stale destination",
    );
}

// ---------------------------------------------------------------------------
// Acceptance: a queued positive-health recovery gives a zero-HP brain an
// explicit nonterminal downed state; bare zero HP remains active.
// ---------------------------------------------------------------------------

#[test]
fn zero_hp_brain_remains_active_without_despawn() {
    let mut reg = EntityRegistry::new();
    let mut warned = AiRuntime::new();
    let pawn = spawn_player(&mut reg, Vec3::new(1.0, 0.0, 0.0));
    let enemy = spawn_enemy(
        &mut reg,
        Vec3::ZERO,
        brain_with(tuning(), TEST_ALERT_STATE),
        50.0,
    );
    set_hp(&mut reg, enemy, 0.0);

    let events = run_ai_tick(&mut reg, &mut warned, 0.016);

    assert!(reg.exists(enemy), "zero HP alone must not remove the brain");
    assert_eq!(
        enemy_state_name(&reg, enemy),
        TEST_ATTACK_STATE,
        "zero HP must not force the terminal death state",
    );
    assert_eq!(events, vec![ENEMY_ATTACK_EVENT]);
    assert_eq!(player_hp(&reg, pawn), 92.0);
}

#[test]
fn queued_despawn_quiesces_brain_without_overwriting_animation() {
    let mut reg = EntityRegistry::new();
    let mut warned = AiRuntime::new();
    spawn_player(&mut reg, Vec3::new(1.0, 0.0, 0.0));
    let enemy = spawn_enemy(
        &mut reg,
        Vec3::ZERO,
        brain_with(tuning(), TEST_ALERT_STATE),
        50.0,
    );
    agent_steering::set_destination(&mut reg, enemy, Vec3::new(5.0, 0.0, 0.0));
    crate::impact_effects::play_animation(&mut reg, enemy, "death");
    crate::impact_effects::despawn(&mut reg, enemy, Some(500.0));

    let events = run_ai_tick(&mut reg, &mut warned, 0.016);

    assert!(events.is_empty(), "a queued despawn must stop attacks");
    assert_eq!(enemy_state_name(&reg, enemy), TEST_ALERT_STATE);
    assert_eq!(
        enemy_animation(&reg, enemy),
        "death",
        "AI must not overwrite the modder-owned death presentation",
    );
    assert!(
        agent_steering::path_state(&reg, enemy)
            .expect("agent present")
            .has_destination,
        "a queued despawn must hold steering",
    );
}

#[test]
fn queued_positive_health_recovery_quiesces_zero_hp_brain() {
    let mut reg = EntityRegistry::new();
    let mut warned = AiRuntime::new();
    let pawn = spawn_player(&mut reg, Vec3::new(1.0, 0.0, 0.0));
    let enemy = spawn_enemy(
        &mut reg,
        Vec3::ZERO,
        brain_with(tuning(), TEST_ATTACK_STATE),
        50.0,
    );
    set_hp(&mut reg, enemy, 0.0);
    crate::impact_effects::play_animation(&mut reg, enemy, "death");
    crate::impact_effects::set_health(&mut reg, enemy, 25.0, Some(500.0));

    let events = run_ai_tick(&mut reg, &mut warned, 0.016);

    assert!(events.is_empty(), "a downed enemy must not attack");
    assert_eq!(player_hp(&reg, pawn), 100.0);
    assert_eq!(
        enemy_animation(&reg, enemy),
        "death",
        "AI must not overwrite the downed enemy's death presentation",
    );
    assert!(
        reg.get_component::<postretro_entities::DeferredEffectComponent>(enemy)
            .unwrap()
            .pending
            .iter()
            .all(|effect| effect.kind != postretro_entities::DeferredEffectKind::Despawn),
        "the downed recovery carries no terminal despawn",
    );
}

#[test]
fn queued_health_change_does_not_quiesce_live_brain() {
    let mut reg = EntityRegistry::new();
    let mut warned = AiRuntime::new();
    let pawn = spawn_player(&mut reg, Vec3::new(1.0, 0.0, 0.0));
    let enemy = spawn_enemy(
        &mut reg,
        Vec3::ZERO,
        brain_with(tuning(), TEST_ATTACK_STATE),
        50.0,
    );
    crate::impact_effects::set_health(&mut reg, enemy, 25.0, Some(500.0));

    let events = run_ai_tick(&mut reg, &mut warned, 0.016);

    assert_eq!(events, vec![ENEMY_ATTACK_EVENT]);
    assert_eq!(player_hp(&reg, pawn), 92.0);
}

#[test]
fn elapsed_health_recovery_reactivates_downed_brain() {
    let mut reg = EntityRegistry::new();
    let mut warned = AiRuntime::new();
    let pawn = spawn_player(&mut reg, Vec3::new(1.0, 0.0, 0.0));
    let enemy = spawn_enemy(
        &mut reg,
        Vec3::ZERO,
        brain_with(tuning(), TEST_ATTACK_STATE),
        50.0,
    );
    set_hp(&mut reg, enemy, 0.0);
    crate::impact_effects::play_animation(&mut reg, enemy, "death");
    crate::impact_effects::set_health(&mut reg, enemy, 25.0, Some(10.0));

    assert!(
        run_ai_tick(&mut reg, &mut warned, 0.016).is_empty(),
        "the recovery delay keeps the zero-HP brain downed",
    );
    crate::impact_effects::tick_deferred_effects(&mut reg, 0.010);
    assert_eq!(
        enemy_animation(&reg, enemy),
        "idle",
        "recovery must immediately leave the death pose before AI movement resumes",
    );

    let events = run_ai_tick(&mut reg, &mut warned, 0.016);
    assert_eq!(events, vec![ENEMY_ATTACK_EVENT]);
    assert_eq!(player_hp(&reg, pawn), 92.0);
    assert_eq!(
        enemy_animation(&reg, enemy),
        "attack",
        "the resumed Attack brain must replace the recovery idle pose",
    );
}

#[test]
fn elapsed_health_recovery_reselects_moving_alert_animation() {
    let mut reg = EntityRegistry::new();
    let mut warned = AiRuntime::new();
    spawn_player(&mut reg, Vec3::new(10.0, 0.0, 0.0));
    let mut brain = brain_with(tuning(), TEST_ALERT_STATE);
    brain.locomotion_moving = true;
    let enemy = spawn_enemy(&mut reg, Vec3::ZERO, brain, 50.0);
    set_agent_velocity(&mut reg, enemy, Vec3::new(1.0, 0.0, 0.0));
    set_hp(&mut reg, enemy, 0.0);
    crate::impact_effects::play_animation(&mut reg, enemy, "death");
    crate::impact_effects::set_health(&mut reg, enemy, 25.0, Some(10.0));

    crate::impact_effects::tick_deferred_effects(&mut reg, 0.010);
    assert_eq!(enemy_animation(&reg, enemy), "idle");

    run_ai_tick(&mut reg, &mut warned, 0.016);
    assert_eq!(enemy_state_name(&reg, enemy), TEST_ALERT_STATE);
    assert_eq!(
        enemy_animation(&reg, enemy),
        "locomotion",
        "the resumed moving Alert brain must replace the recovery idle pose",
    );
}

// ---------------------------------------------------------------------------
// Regression: the integrated no-navigation fallback loop (run_ai_tick +
// agent_steering::tick without combat-position selector inputs) must not freeze
// chasers beyond the replan budget, and a stationary target must not force a
// replan per tick.
//
// Bug: `set_destination` wiped the path on every call. The FSM re-issues the
// raw target position every fallback chase tick, so with more than
// REPLAN_BUDGET_PER_TICK chasers, the overflow chasers ended each tick with an
// empty path → goal_velocity ZERO → permanent freeze. Fix: `set_destination`
// only records the target; the path is (re)built solely inside `tick`'s
// budget-gated replan block, so a budget-deferred agent keeps its
// stale-but-valid path and keeps moving.
// ---------------------------------------------------------------------------

const STEER_DT: f32 = 1.0 / 60.0;
const STEER_GRAVITY: f32 = -20.0;

/// Open flat floor `[0,40] x [0,40]` at y=0, covered by a single navmesh region
/// so any in-bounds destination is routable. One description drives both the
/// collision trimesh and the navmesh, matching the agent_steering fixture
/// precedent (geometry and navmesh agree).
struct OpenFloor {
    extent: f32,
}

impl OpenFloor {
    fn new() -> Self {
        OpenFloor { extent: 40.0 }
    }

    /// Collision world: the single floor quad (two triangles), so agents are
    /// grounded and slide freely across it.
    fn collision_world(&self) -> CollisionWorld {
        let points = vec![
            Point::new(0.0, 0.0, 0.0),
            Point::new(self.extent, 0.0, 0.0),
            Point::new(self.extent, 0.0, self.extent),
            Point::new(0.0, 0.0, self.extent),
        ];
        let tris = vec![[0u32, 1, 2], [0, 2, 3]];
        CollisionWorld {
            mesh: TriMesh::new(points, tris),
            isometry: Isometry::identity(),
        }
    }

    /// Single navmesh region covering the whole floor. Unit cells, origin at
    /// world zero, so cell coords equal world coords.
    fn navmesh(&self) -> NavMeshSection {
        NavMeshSection {
            version: NAVMESH_VERSION,
            origin: [0.0, 0.0, 0.0],
            cell_size: 1.0,
            dim_x: 64,
            dim_z: 64,
            agent_radius: 0.35,
            agent_height: 1.8,
            step_height: 0.4,
            max_slope_deg: 45.0,
            regions: vec![NavRegion {
                x0: 0,
                z0: 0,
                x1: self.extent as u32,
                z1: self.extent as u32,
                floor_y_min: 0.0,
                floor_y_max: 0.25,
            }],
            portals: vec![],
        }
    }

    fn nav_graph(&self) -> NavGraph {
        NavGraph::from_section(&self.navmesh())
    }
}

/// Resting capsule-center height above the floor for the canonical agent, so a
/// spawned chaser starts grounded and gravity does not dominate the first ticks.
fn chaser_rest_y() -> f32 {
    use crate::collision::SKIN_DISTANCE;
    let (radius, height) = (0.35_f32, 1.8_f32);
    let half_height = height / 2.0 - radius;
    half_height + radius + SKIN_DISTANCE
}

/// Spawn a grounded enemy already in the direct graph's `alert` state at world `(x, _, z)`. The
/// agent capsule matches the navmesh's baked agent so it routes cleanly.
fn spawn_chaser(reg: &mut EntityRegistry, x: f32, z: f32) -> EntityId {
    let pos = Vec3::new(x, chaser_rest_y(), z);
    spawn_enemy(reg, pos, brain_with(tuning(), TEST_ALERT_STATE), 50.0)
}

fn enemy_combat_slot(reg: &EntityRegistry, enemy: EntityId) -> Option<Vec3> {
    reg.get_component::<BrainComponent>(enemy)
        .unwrap()
        .combat_slot
}

fn assert_approx_distance(actual: f32, expected: f32, message: &str) {
    assert!(
        (actual - expected).abs() <= 1.0e-4,
        "{message}: expected {expected}, got {actual}"
    );
}

#[test]
fn closed_enemy_with_no_destination_is_not_separation_nudged() {
    let floor = OpenFloor::new();
    let world = floor.collision_world();
    let graph = floor.nav_graph();
    let mut reg = EntityRegistry::new();

    let mut sealed_brain = brain_with(tuning(), TEST_IDLE_STATE);
    sealed_brain.aggro_armed = false;
    let sealed = spawn_enemy(
        &mut reg,
        Vec3::new(8.0, chaser_rest_y(), 8.0),
        sealed_brain,
        50.0,
    );
    // The second agent overlaps the sealed enemy and has a destination, so its
    // own steering executes the normal separation path. The sealed enemy must
    // still take the destination-less idle-settle early continue.
    let moving = spawn_enemy(
        &mut reg,
        Vec3::new(8.1, chaser_rest_y(), 8.0),
        brain_with(tuning(), TEST_IDLE_STATE),
        50.0,
    );
    agent_steering::set_destination(&mut reg, moving, Vec3::new(16.0, chaser_rest_y(), 8.0));
    let start = *reg.get_component::<Transform>(sealed).unwrap();

    agent_steering::tick(&mut reg, &world, Some(&graph), STEER_GRAVITY, STEER_DT);

    let end = reg.get_component::<Transform>(sealed).unwrap();
    assert!(
        (end.position.x - start.position.x).abs() <= EPS
            && (end.position.z - start.position.z).abs() <= EPS,
        "a closed brain's cleared destination exempts it from separation movement; \
         start={:?}, end={:?}",
        start.position,
        end.position,
    );
    assert_eq!(
        end.rotation, start.rotation,
        "steering must not change facing"
    );
}

#[test]
fn separation_excludes_the_chase_target_so_enemies_close_to_contact() {
    // INV3: inter-agent separation keeps ENEMIES apart from each other only.
    // The player pawn carries PlayerMovement — never an Agent component — so
    // the steering separation pass (which scans agent snapshots) must not hold
    // an enemy off its target. Drive a single enemy straight at a player
    // closer than the separation comfort band (2.5 * radius = 0.875): if the
    // player were in the separation set the enemy would be pushed AWAY;
    // instead it must close monotonically into the arrival band.
    let floor = OpenFloor::new();
    let world = floor.collision_world();
    let graph = floor.nav_graph();
    let mut reg = EntityRegistry::new();

    let player_pos = Vec3::new(20.0, chaser_rest_y(), 20.0);
    spawn_player(&mut reg, player_pos);
    let enemy = spawn_chaser(&mut reg, 20.7, 20.0); // inside the would-be comfort band
    agent_steering::set_destination(&mut reg, enemy, player_pos);

    let mut last = distance_xz(
        agent_steering::path_state(&reg, enemy).unwrap().position,
        player_pos,
    );
    for tick_index in 0..120 {
        agent_steering::tick(&mut reg, &world, Some(&graph), STEER_GRAVITY, STEER_DT);
        let state = agent_steering::path_state(&reg, enemy).unwrap();
        let dist = distance_xz(state.position, player_pos);
        assert!(
            dist <= last + 1.0e-4,
            "enemy was pushed away from its chase target on tick {tick_index}: {last} -> {dist}"
        );
        last = dist;
        if state.arrived {
            break;
        }
    }
    assert!(
        last <= 0.6,
        "enemy must close into the arrival band of its target, ended at {last}"
    );
}

fn small_nav_graph(regions: Vec<NavRegion>) -> NavGraph {
    NavGraph::from_section(&NavMeshSection {
        version: NAVMESH_VERSION,
        origin: [0.0, 0.0, 0.0],
        cell_size: 1.0,
        dim_x: 8,
        dim_z: 8,
        agent_radius: 0.35,
        agent_height: 1.8,
        step_height: 0.4,
        max_slope_deg: 45.0,
        regions,
        portals: vec![],
    })
}

#[test]
fn ai_combat_positioning_assigns_distinct_slots_around_selected_target() {
    let floor = OpenFloor::new();
    let world = floor.collision_world();
    let graph = floor.nav_graph();

    let mut reg = EntityRegistry::new();
    let mut warned = AiRuntime::new();
    let player_pos = Vec3::new(20.0, chaser_rest_y(), 20.0);
    spawn_player(&mut reg, player_pos);

    let enemies = [
        spawn_chaser(&mut reg, 14.0, 30.0),
        spawn_chaser(&mut reg, 15.0, 30.5),
        spawn_chaser(&mut reg, 16.0, 31.0),
    ];

    run_ai_tick_with_navigation(&mut reg, &mut warned, STEER_DT, Some(&graph), Some(&world));

    let slots: Vec<Vec3> = enemies
        .iter()
        .map(|&enemy| enemy_combat_slot(&reg, enemy).expect("combat slot"))
        .collect();
    for (index, slot) in slots.iter().enumerate() {
        assert!(
            distance_xz(*slot, player_pos) >= TEST_ATTACK_RANGE * 0.75 - 1.0e-4,
            "slot {index} should be on an engagement ring, got {slot:?}"
        );
        assert!(
            distance_xz(*slot, player_pos) > 0.5,
            "slot {index} must not be the target center"
        );
        let path = agent_steering::path_state(&reg, enemies[index]).expect("agent");
        assert_approx_distance(
            path.distance_to_destination,
            distance_xz(path.position, *slot),
            "steering destination should be the selected combat slot",
        );
    }
    for (index, slot) in slots.iter().enumerate() {
        assert!(
            slots[..index]
                .iter()
                .all(|previous| distance_xz(*previous, *slot) > 0.35),
            "slots should be distinct: {slots:?}"
        );
    }
}

#[test]
fn ai_combat_positioning_near_enemy_uses_engagement_band_not_target_center() {
    let floor = OpenFloor::new();
    let world = floor.collision_world();
    let graph = floor.nav_graph();

    let mut reg = EntityRegistry::new();
    let mut warned = AiRuntime::new();
    let player_pos = Vec3::new(20.0, chaser_rest_y(), 20.0);
    spawn_player(&mut reg, player_pos);
    let enemy = spawn_chaser(&mut reg, 20.4, 20.0);

    run_ai_tick_with_navigation(&mut reg, &mut warned, STEER_DT, Some(&graph), Some(&world));

    let slot = enemy_combat_slot(&reg, enemy).expect("combat slot");
    assert!(
        distance_xz(slot, player_pos) >= TEST_ATTACK_RANGE * 0.75 - 1.0e-4,
        "near enemy should steer back to the engagement band, got {slot:?}"
    );
    let path = agent_steering::path_state(&reg, enemy).expect("agent");
    assert!(
        path.distance_to_destination > distance_xz(path.position, player_pos),
        "destination should be a band slot, not a push into the player capsule"
    );
}

#[test]
fn ai_combat_positioning_scarce_slots_leave_extras_on_raw_target_chase() {
    let world = OpenFloor { extent: 4.0 }.collision_world();
    let graph = small_nav_graph(vec![NavRegion {
        x0: 2,
        z0: 0,
        x1: 3,
        z1: 2,
        floor_y_min: 0.0,
        floor_y_max: 0.25,
    }]);

    let mut reg = EntityRegistry::new();
    let mut warned = AiRuntime::new();
    let player_pos = Vec3::new(1.0, chaser_rest_y(), 1.0);
    spawn_player(&mut reg, player_pos);
    let enemies = [
        spawn_chaser(&mut reg, 2.2, 0.2),
        spawn_chaser(&mut reg, 2.8, 0.2),
    ];

    run_ai_tick_with_navigation(&mut reg, &mut warned, STEER_DT, Some(&graph), Some(&world));

    let slots: Vec<Option<Vec3>> = enemies
        .iter()
        .map(|&enemy| enemy_combat_slot(&reg, enemy))
        .collect();
    assert_eq!(
        slots.iter().filter(|slot| slot.is_some()).count(),
        1,
        "only the single valid slot should be claimed: {slots:?}"
    );
    assert_eq!(
        slots.iter().filter(|slot| slot.is_none()).count(),
        1,
        "the extra enemy should fall back instead of duplicating a slot"
    );

    for (&enemy, slot) in enemies.iter().zip(slots.iter()) {
        let path = agent_steering::path_state(&reg, enemy).expect("agent");
        let expected = slot.unwrap_or(player_pos);
        assert_approx_distance(
            path.distance_to_destination,
            distance_xz(path.position, expected),
            "scarce-slot destination",
        );
    }
}

#[test]
fn ai_combat_positioning_uses_each_enemy_selected_target_position() {
    let floor = OpenFloor::new();
    let world = floor.collision_world();
    let graph = floor.nav_graph();

    let mut reg = EntityRegistry::new();
    let mut warned = AiRuntime::new();
    let player_a_pos = Vec3::new(8.0, chaser_rest_y(), 8.0);
    let player_b_pos = Vec3::new(30.0, chaser_rest_y(), 30.0);
    let player_a = spawn_player(&mut reg, player_a_pos);
    let player_b = spawn_player(&mut reg, player_b_pos);
    let enemy_a = spawn_chaser(&mut reg, 8.0, 16.0);
    let enemy_b = spawn_chaser(&mut reg, 30.0, 22.0);

    run_ai_tick_with_navigation(&mut reg, &mut warned, STEER_DT, Some(&graph), Some(&world));

    let brain_a = reg.get_component::<BrainComponent>(enemy_a).unwrap();
    let brain_b = reg.get_component::<BrainComponent>(enemy_b).unwrap();
    assert_eq!(brain_a.acquired_target, Some(player_a));
    assert_eq!(brain_b.acquired_target, Some(player_b));

    let slot_a = brain_a.combat_slot.expect("slot around player A");
    let slot_b = brain_b.combat_slot.expect("slot around player B");
    assert!(
        distance_xz(slot_a, player_a_pos) <= TEST_ATTACK_RANGE * 1.25 + 1.0e-4
            && distance_xz(slot_a, player_b_pos) > 10.0,
        "enemy A slot should be generated around its selected target: {slot_a:?}"
    );
    assert!(
        distance_xz(slot_b, player_b_pos) <= TEST_ATTACK_RANGE * 1.25 + 1.0e-4
            && distance_xz(slot_b, player_a_pos) > 10.0,
        "enemy B slot should be generated around its selected target: {slot_b:?}"
    );
}

#[test]
fn ai_combat_positioning_retains_same_target_incumbent_and_decrements_hold() {
    let floor = OpenFloor::new();
    let world = floor.collision_world();
    let graph = floor.nav_graph();

    let mut reg = EntityRegistry::new();
    let mut warned = AiRuntime::new();
    let player_pos = Vec3::new(20.0, chaser_rest_y(), 20.0);
    let player = spawn_player(&mut reg, player_pos);
    let incumbent = player_pos + Vec3::new(TEST_ATTACK_RANGE * 1.25, 0.0, 0.0);
    let mut brain = brain_with(tuning(), TEST_ALERT_STATE);
    brain.acquired_target = Some(player);
    brain.combat_slot = Some(incumbent);
    brain.combat_slot_hold_ticks = 3;
    let enemy = spawn_enemy(&mut reg, player_pos, brain, 50.0);

    run_ai_tick_with_navigation(&mut reg, &mut warned, STEER_DT, Some(&graph), Some(&world));

    let brain = reg.get_component::<BrainComponent>(enemy).unwrap();
    let slot = brain.combat_slot.expect("retained combat slot");
    assert!(
        distance_xz(slot, incumbent) <= 1.0e-4,
        "same-target incumbent should be retained inside the switch margin"
    );
    assert_eq!(
        brain.combat_slot_hold_ticks, 2,
        "retained incumbents should spend one hold tick"
    );
}

#[test]
fn ai_combat_positioning_clears_stale_slot_when_selector_surfaces_are_absent() {
    let mut reg = EntityRegistry::new();
    let mut warned = AiRuntime::new();
    let player_pos = Vec3::new(8.0, chaser_rest_y(), 8.0);
    let player = spawn_player(&mut reg, player_pos);
    let mut brain = brain_with(tuning(), TEST_ALERT_STATE);
    brain.acquired_target = Some(player);
    brain.combat_slot = Some(player_pos + Vec3::new(TEST_ATTACK_RANGE, 0.0, 0.0));
    brain.combat_slot_hold_ticks = COMBAT_SLOT_HOLD_TICKS;
    let enemy = spawn_enemy(&mut reg, Vec3::new(8.0, chaser_rest_y(), 12.0), brain, 50.0);

    run_ai_tick_with_navigation(&mut reg, &mut warned, STEER_DT, None, None);

    let brain = reg.get_component::<BrainComponent>(enemy).unwrap();
    assert_eq!(
        brain.combat_slot, None,
        "no-nav/no-collision fallback must not preserve stale tactical slots"
    );
    assert_eq!(brain.combat_slot_hold_ticks, 0);
}

#[test]
fn ai_combat_positioning_invalidates_stale_incumbent_on_target_switch() {
    let floor = OpenFloor::new();
    let world = floor.collision_world();
    let graph = floor.nav_graph();

    let mut reg = EntityRegistry::new();
    let mut warned = AiRuntime::new();
    let old_player_pos = Vec3::new(40.0, chaser_rest_y(), 8.0);
    let new_player_pos = Vec3::new(8.0, chaser_rest_y(), 8.0);
    let old_player = spawn_player(&mut reg, old_player_pos);
    let new_player = spawn_player(&mut reg, new_player_pos);
    let enemy_pos = Vec3::new(8.0, chaser_rest_y(), 12.0);
    let stale_slot = new_player_pos + Vec3::new(TEST_ATTACK_RANGE + 0.9, 0.0, 0.0);
    let mut brain = brain_with(tuning(), TEST_ALERT_STATE);
    brain.acquired_target = Some(old_player);
    brain.think_stride_counter =
        think_stride_for_distance(distance_xz(old_player_pos, enemy_pos)) - 1;
    brain.combat_slot = Some(stale_slot);
    brain.combat_slot_hold_ticks = COMBAT_SLOT_HOLD_TICKS;
    let enemy = spawn_enemy(&mut reg, enemy_pos, brain, 50.0);

    run_ai_tick_with_navigation(&mut reg, &mut warned, STEER_DT, Some(&graph), Some(&world));

    let brain = reg.get_component::<BrainComponent>(enemy).unwrap();
    assert_eq!(brain.acquired_target, Some(new_player));
    let slot = brain.combat_slot.expect("new-target combat slot");
    assert!(
        distance_xz(slot, stale_slot) > 1.0e-4,
        "slot from the old target must not be retained as a new-target incumbent"
    );
    assert!(
        distance_xz(slot, new_player_pos) <= TEST_ATTACK_RANGE * 1.25 + 1.0e-4,
        "replacement slot should be generated around the newly selected target"
    );
    assert_eq!(
        brain.combat_slot_hold_ticks, COMBAT_SLOT_HOLD_TICKS,
        "newly selected replacement slots should reset the hold countdown"
    );
}

#[test]
fn ai_combat_positioning_ignores_inactive_stale_slot_claims() {
    let floor = OpenFloor::new();
    let world = floor.collision_world();
    let graph = floor.nav_graph();

    let mut reg = EntityRegistry::new();
    let mut warned = AiRuntime::new();
    let player_pos = Vec3::new(20.0, chaser_rest_y(), 20.0);
    let player = spawn_player(&mut reg, player_pos);
    let expected_slot = player_pos - Vec3::new(TEST_ATTACK_RANGE, 0.0, 0.0);

    let mut inactive = brain_with(tuning(), TEST_IDLE_STATE);
    inactive.acquired_target = Some(player);
    inactive.combat_slot = Some(expected_slot);
    inactive.combat_slot_hold_ticks = COMBAT_SLOT_HOLD_TICKS;
    spawn_enemy(
        &mut reg,
        Vec3::new(40.0, chaser_rest_y(), 40.0),
        inactive,
        50.0,
    );
    let active = spawn_chaser(&mut reg, 14.0, 20.0);

    run_ai_tick_with_navigation(&mut reg, &mut warned, STEER_DT, Some(&graph), Some(&world));

    let active_slot = enemy_combat_slot(&reg, active).expect("active combat slot");
    assert!(
        distance_xz(active_slot, expected_slot) <= 1.0e-4,
        "inactive stale slot claims should not block active claimants"
    );
}

#[test]
fn integrated_chase_loop_keeps_all_chasers_moving_past_replan_budget() {
    // Regression: set_destination wiped the path and forced a replan every tick,
    // so chasers beyond REPLAN_BUDGET_PER_TICK froze and a stationary target
    // replanned each tick. This drives the real loop (FSM tick + steering tick)
    // and proves (a) every chaser keeps moving and (b) the path is preserved /
    // replans stay bounded for a near-stationary player.
    let floor = OpenFloor::new();
    let world = floor.collision_world();
    let graph = floor.nav_graph();

    let mut reg = EntityRegistry::new();
    let mut warned = AiRuntime::new();

    // A stationary player within the direct graph's 18-unit detection range of
    // the entire cluster, but far enough away that the chasers remain in the
    // pursuit phase for this short run.
    let player_pos = Vec3::new(20.0, 0.0, 14.0);
    let player = spawn_player(&mut reg, player_pos);

    // More chasers than the per-tick replan budget, clustered near the far end.
    let chaser_count = agent_steering::REPLAN_BUDGET_PER_TICK + 3;
    let mut chasers: Vec<EntityId> = Vec::new();
    for i in 0..chaser_count {
        let id = spawn_chaser(&mut reg, 14.0 + i as f32 * 0.6, 30.0);
        chasers.push(id);
    }

    // Record each chaser's start position to prove forward progress later.
    let start_pos: Vec<Vec3> = chasers
        .iter()
        .map(|&id| agent_steering::path_state(&reg, id).unwrap().position)
        .collect();

    // Run the integrated no-navigation fallback loop for many ticks: FSM tick
    // (issues raw-target set_destination because no selector inputs are
    // supplied) then the steering tick.
    let mut total_replans = 0u32;
    let mut path_present_ticks = 0u32;
    let ticks = 200;
    for tick_index in 0..ticks {
        run_ai_tick(&mut reg, &mut warned, STEER_DT);
        let result = agent_steering::tick(&mut reg, &world, Some(&graph), STEER_GRAVITY, STEER_DT);
        total_replans += result.replans;

        // After the first few ticks every chaser should hold a live path toward
        // the stationary player (the path is preserved, not wiped each tick).
        if tick_index >= 5 {
            let all_have_path = chasers.iter().all(|&id| {
                agent_steering::path_state(&reg, id)
                    .map(|s| s.has_path)
                    .unwrap_or(false)
            });
            if all_have_path {
                path_present_ticks += 1;
            }
        }
    }

    // (a) No chaser froze: every one moved measurably toward the player. A frozen
    // agent (path wiped, goal_velocity == ZERO) would not advance at all.
    for (&id, &start) in chasers.iter().zip(start_pos.iter()) {
        let state = agent_steering::path_state(&reg, id).unwrap();
        let moved = distance_xz(start, state.position);
        assert!(
            moved > 0.5,
            "chaser {id} should have advanced toward the player, moved only {moved} \
             (start {start:?}, end {:?}) — frozen by a wiped path?",
            state.position
        );
        // It moved toward, not away from, the (stationary) player.
        assert!(
            distance_xz(state.position, player_pos) < distance_xz(start, player_pos),
            "chaser {id} should be closer to the player than at start",
        );
    }

    // (b) A stationary target does not force a replan every tick. Without the fix,
    // every chaser would replan up to the budget EVERY tick — ~budget * ticks
    // total. With the fix, after the initial plan each chaser only replans on the
    // staleness window (REPLAN_STALENESS_TICKS), so the total is far lower.
    let unbounded = agent_steering::REPLAN_BUDGET_PER_TICK * ticks;
    let staleness_bound = chaser_count * (ticks / agent_steering::REPLAN_STALENESS_TICKS + 2);
    assert!(
        total_replans <= staleness_bound,
        "stationary target replanned too often: {total_replans} replans over {ticks} ticks \
         (staleness bound {staleness_bound}, per-tick-budget unbounded would be {unbounded})",
    );

    // And the preserved path held across the run for the stationary target.
    assert!(
        path_present_ticks > 0,
        "chasers should hold a live path across ticks toward a stationary player",
    );

    // Sanity: the player took damage or stayed put — either way it is still the
    // chase target and the loop ran without panicking.
    let _ = player_hp(&reg, player);
}

/// Set the player pawn's XZ position (keeps Y), so a test can walk the target a
/// fixed step each tick.
fn move_player_to(reg: &mut EntityRegistry, pawn: EntityId, x: f32, z: f32) {
    let mut t = *reg.get_component::<Transform>(pawn).unwrap();
    t.position = Vec3::new(x, t.position.y, z);
    reg.set_component(pawn, t).unwrap();
}

#[test]
fn integrated_chase_loop_closes_distance_for_all_chasers_when_player_moves() {
    // Regression (the bug the stationary-player test missed): without selector
    // inputs the FSM re-issues the raw player position to `set_destination` EVERY
    // fallback chase tick. The old `set_destination` wiped each chaser's path on
    // every call; the per-tick replan budget then only replanned
    // REPLAN_BUDGET_PER_TICK of them, so the OVERFLOW chasers ended every tick
    // with an empty path → goal_velocity ZERO → permanent freeze. The fix
    // preserves the path and lets a budget-loss chaser keep following its
    // stale-but-valid route. This test spawns MORE chasers than the budget and a
    // player that moves ~0.12 u/tick (a real per-tick step), and asserts EVERY
    // chaser — overflow included — keeps moving (the load-bearing `moved > 1.0`
    // check). It FAILS pre-fix: overflow chasers freeze (~0.27 u).
    let floor = OpenFloor::new();
    let world = floor.collision_world();
    let graph = floor.nav_graph();

    let mut reg = EntityRegistry::new();
    let mut warned = AiRuntime::new();

    // Player starts far down the floor; it will walk TOWARD the chaser cluster so
    // both sides converge regardless of relative speed — what we assert is that
    // no chaser is frozen, not a fleeing-race outcome.
    let player_start = Vec3::new(20.0, 0.0, 8.0);
    let player = spawn_player(&mut reg, player_start);

    // More chasers than the per-tick replan budget, clustered at the far end.
    let chaser_count = agent_steering::REPLAN_BUDGET_PER_TICK + 3;
    let mut chasers: Vec<EntityId> = Vec::new();
    for i in 0..chaser_count {
        let id = spawn_chaser(&mut reg, 14.0 + i as f32 * 0.6, 30.0);
        chasers.push(id);
    }

    let start_pos: Vec<Vec3> = chasers
        .iter()
        .map(|&id| agent_steering::path_state(&reg, id).unwrap().position)
        .collect();
    let start_dist_to_player: Vec<f32> = start_pos
        .iter()
        .map(|&p| distance_xz(p, player_start))
        .collect();

    // Per-tick player step: ~0.12 u/tick — a real per-tick move (the old
    // path-wiping set_destination cleared on any change), yet small enough that
    // the cluster stays inside detection range.
    const PLAYER_STEP_PER_TICK: f32 = 0.12;
    let ticks = 200u32;

    for _ in 0..ticks {
        // Walk the player in +Z toward the cluster, clamped to the floor bounds so
        // it stays on the navmesh (the chasers' destination must stay routable).
        let p = reg.get_component::<Transform>(player).unwrap().position;
        let next_z = (p.z + PLAYER_STEP_PER_TICK).min(floor.extent - 2.0);
        move_player_to(&mut reg, player, p.x, next_z);

        run_ai_tick(&mut reg, &mut warned, STEER_DT);
        agent_steering::tick(&mut reg, &world, Some(&graph), STEER_GRAVITY, STEER_DT);
    }

    let player_end = reg.get_component::<Transform>(player).unwrap().position;

    // EVERY chaser — including the overflow ones beyond the budget — must have
    // moved a real amount (well above the gravity/separation settle noise floor).
    // The `moved > 1.0` check is the load-bearing freeze guard: a frozen overflow
    // chaser (path wiped, goal_velocity ZERO) advances essentially zero (~0.27 u
    // of settle). The distance-closed check is a secondary sanity assert (with the
    // player advancing toward the cluster it is weaker than `moved`, but it pins
    // that the chasers track the live target rather than wandering).
    for (idx, &id) in chasers.iter().enumerate() {
        let state = agent_steering::path_state(&reg, id).unwrap();
        let moved = distance_xz(start_pos[idx], state.position);
        assert!(
            moved > 1.0,
            "chaser {id} (index {idx}) barely moved ({moved} u) — frozen by a wiped \
             path? start {:?}, end {:?}",
            start_pos[idx],
            state.position
        );
        let end_dist = distance_xz(state.position, player_end);
        assert!(
            end_dist + 1.0 < start_dist_to_player[idx],
            "chaser {id} (index {idx}) did not close distance to the moving player: \
             start dist {}, end dist {end_dist}",
            start_dist_to_player[idx],
        );
    }
}

// ---------------------------------------------------------------------------
// Facing: the enemy orients believably. Nothing else writes the enemy's
// `Transform` rotation, so the AI tick owns yaw — face velocity when moving,
// face the player when stopped-but-engaged, leave Idle/Death facing untouched,
// and never write a NaN yaw from a zero-length direction.
// ---------------------------------------------------------------------------

/// Assert two normalized XZ directions point the same way (dot ≈ 1).
fn assert_faces(actual: Vec3, expected: Vec3, ctx: &str) {
    let dot = actual.normalize().dot(expected.normalize());
    assert!(
        dot > 0.999,
        "{ctx}: expected facing {expected:?}, got {actual:?} (dot {dot})"
    );
}

fn run_ticks_until_facing_converges(
    reg: &mut EntityRegistry,
    warned: &mut AiRuntime,
    enemy: EntityId,
    expected: Vec3,
    ctx: &str,
) {
    for _ in 0..16 {
        run_ai_tick(reg, warned, 0.016);
        if enemy_forward_xz(reg, enemy)
            .normalize()
            .dot(expected.normalize())
            > 0.999
        {
            return;
        }
    }
    assert_faces(enemy_forward_xz(reg, enemy), expected, ctx);
}

#[test]
fn facing_turn_rate_is_at_least_steering_turn_rate() {
    const {
        assert!(
            FACING_TURN_RATE >= agent_steering::MAX_TURN_RATE,
            "enemy facing must keep up with path steering"
        );
    }
}

#[test]
fn stopped_engaged_enemy_faces_the_player() {
    // An enemy in attack range (so it reaches `attack`) with near-zero velocity
    // must face the player, not its spawn heading. Player off to +X.
    let mut reg = EntityRegistry::new();
    let mut warned = AiRuntime::new();

    let player = spawn_player(&mut reg, Vec3::new(1.5, 0.0, 0.0));
    let enemy = spawn_enemy(
        &mut reg,
        Vec3::ZERO,
        brain_with(tuning(), TEST_IDLE_STATE),
        50.0,
    );
    // Stopped (arrived/swinging): zero velocity.
    set_agent_velocity(&mut reg, enemy, Vec3::ZERO);

    run_ai_tick(&mut reg, &mut warned, 0.016);
    assert_eq!(enemy_state_name(&reg, enemy), TEST_ATTACK_STATE);

    // Player is at +X from the enemy → the enemy faces +X.
    let to_player = reg.get_component::<Transform>(player).unwrap().position - Vec3::ZERO;
    run_ticks_until_facing_converges(
        &mut reg,
        &mut warned,
        enemy,
        to_player,
        "stopped engaged enemy faces the player",
    );
    assert_faces(
        enemy_forward_xz(&reg, enemy),
        to_player,
        "stopped engaged enemy faces the player",
    );
}

#[test]
fn stopped_engaged_enemy_front_meets_player_not_its_back() {
    // Regression for the 180°-backward facing bug: the facing rotation must point
    // the model's VISUAL FRONT (`+Z` in model space) at the player, not its back.
    // The earlier helper measured the camera-forward axis (`-Z`), so a model that
    // was actually facing AWAY still "passed". This test pins the model-forward
    // axis directly and explicitly rejects the back-facing case, so a regression
    // to the old `-Z` (camera-forward) convention fails here.
    let mut reg = EntityRegistry::new();
    let mut warned = AiRuntime::new();

    // Player off to +X within attack range so the enemy reaches `attack`.
    spawn_player(&mut reg, Vec3::new(1.5, 0.0, 0.0));
    let enemy = spawn_enemy(
        &mut reg,
        Vec3::ZERO,
        brain_with(tuning(), TEST_IDLE_STATE),
        50.0,
    );
    set_agent_velocity(&mut reg, enemy, Vec3::ZERO);

    run_ai_tick(&mut reg, &mut warned, 0.016);
    assert_eq!(enemy_state_name(&reg, enemy), TEST_ATTACK_STATE);

    // The model's authored front (`+Z` rotated by the stored quaternion) points at
    // the player (+X): dot ≈ +1.
    let to_player = Vec3::new(1.0, 0.0, 0.0);
    run_ticks_until_facing_converges(
        &mut reg,
        &mut warned,
        enemy,
        to_player,
        "stopped engaged enemy front meets the player",
    );
    let front = enemy_forward_xz(&reg, enemy);
    let dot = front.dot(to_player);
    assert!(
        dot > 0.999,
        "the model's FRONT must meet the player, dot {dot} (front {front:?})",
    );
    // And it is NOT facing away (the precise failure mode of the old bug): the
    // back would give dot ≈ -1.
    assert!(
        dot > 0.0,
        "the enemy must not show the player its BACK (dot {dot} ⇒ ~180° error)",
    );
}

#[test]
fn moving_enemy_faces_its_velocity_direction() {
    // A moving enemy (XZ speed above the epsilon) faces where it is going — its
    // velocity direction — even if that differs from the bee-line to the player.
    let mut reg = EntityRegistry::new();
    let mut warned = AiRuntime::new();

    // Player inside detection (so the enemy is engaged/alert) along +Z.
    spawn_player(&mut reg, Vec3::new(0.0, 0.0, 10.0));
    let enemy = spawn_enemy(
        &mut reg,
        Vec3::ZERO,
        brain_with(tuning(), TEST_ALERT_STATE),
        50.0,
    );
    // Velocity points toward +X (routing around an obstacle), NOT toward the
    // player bee-line — the facing must follow the velocity.
    let vel = Vec3::new(4.0, 0.0, 0.0);
    set_agent_velocity(&mut reg, enemy, vel);

    run_ai_tick(&mut reg, &mut warned, 0.016);
    run_ticks_until_facing_converges(
        &mut reg,
        &mut warned,
        enemy,
        vel,
        "moving enemy faces its velocity, not the player bee-line",
    );
    assert_faces(
        enemy_forward_xz(&reg, enemy),
        vel,
        "moving enemy faces its velocity, not the player bee-line",
    );
}

#[test]
fn moving_alert_enemy_facing_is_rate_limited_per_tick() {
    let mut reg = EntityRegistry::new();
    let mut warned = AiRuntime::new();

    spawn_player(&mut reg, Vec3::new(10.0, 0.0, 0.0));
    let enemy = spawn_enemy(
        &mut reg,
        Vec3::ZERO,
        brain_with(tuning(), TEST_ALERT_STATE),
        50.0,
    );
    set_agent_velocity(&mut reg, enemy, Vec3::new(4.0, 0.0, 0.0));

    let before = enemy_yaw(&reg, enemy);
    run_ai_tick(&mut reg, &mut warned, 0.016);

    let after = enemy_yaw(&reg, enemy);
    let target = std::f32::consts::FRAC_PI_2;
    assert!(
        yaw_distance(before, after) <= FACING_TURN_RATE * 0.016 + EPS,
        "one tick must not rotate farther than the facing-rate budget"
    );
    assert!(
        yaw_distance(after, target) > 0.1,
        "one tick should slew toward the velocity heading, not snap to it"
    );
}

#[test]
fn idle_enemy_facing_is_left_unchanged() {
    // An idle enemy (no target) must not have its facing written: the spawn
    // rotation is preserved.
    let mut reg = EntityRegistry::new();
    let mut warned = AiRuntime::new();

    // No player in detection range → stays idle.
    spawn_player(&mut reg, Vec3::new(100.0, 0.0, 0.0));
    let enemy = spawn_enemy(
        &mut reg,
        Vec3::ZERO,
        brain_with(tuning(), TEST_IDLE_STATE),
        50.0,
    );
    // Give it a distinctive non-identity spawn rotation, and a velocity that
    // WOULD turn it if idle facing were (incorrectly) written.
    let spawn_rot = glam::Quat::from_rotation_y(1.2);
    let mut t = *reg.get_component::<Transform>(enemy).unwrap();
    t.rotation = spawn_rot;
    reg.set_component(enemy, t).unwrap();
    set_agent_velocity(&mut reg, enemy, Vec3::new(3.0, 0.0, 0.0));

    run_ai_tick(&mut reg, &mut warned, 0.016);
    assert_eq!(enemy_state_name(&reg, enemy), TEST_IDLE_STATE);
    let rot_after = reg.get_component::<Transform>(enemy).unwrap().rotation;
    assert!(
        rot_after.angle_between(spawn_rot) < 1e-5,
        "an idle enemy's facing must be left unchanged (was {spawn_rot:?}, now {rot_after:?})",
    );
}

#[test]
fn death_enemy_facing_is_left_unchanged() {
    let mut reg = EntityRegistry::new();
    let mut warned = AiRuntime::new();

    spawn_player(&mut reg, Vec3::new(1.5, 0.0, 0.0));
    let enemy = spawn_enemy(
        &mut reg,
        Vec3::ZERO,
        brain_with(tuning(), TEST_DEATH_STATE),
        0.0,
    );
    let spawn_rot = glam::Quat::from_rotation_y(1.2);
    let mut transform = *reg.get_component::<Transform>(enemy).unwrap();
    transform.rotation = spawn_rot;
    reg.set_component(enemy, transform).unwrap();
    set_agent_velocity(&mut reg, enemy, Vec3::new(3.0, 0.0, 0.0));

    run_ai_tick(&mut reg, &mut warned, 0.016);
    assert_eq!(enemy_state_name(&reg, enemy), TEST_DEATH_STATE);
    let rot_after = reg.get_component::<Transform>(enemy).unwrap().rotation;
    assert!(
        rot_after.angle_between(spawn_rot) < 1e-5,
        "a freeze-state enemy's facing must be left unchanged (was {spawn_rot:?}, now {rot_after:?})",
    );
}

#[test]
fn stopped_engaged_enemy_on_top_of_player_writes_no_nan_facing() {
    // Degenerate: a stopped engaged enemy at the SAME XZ as the player → the
    // to-player direction is zero-length. The facing guard must skip the write,
    // leaving the prior rotation finite (no NaN quaternion).
    let mut reg = EntityRegistry::new();
    let mut warned = AiRuntime::new();

    // Player co-located (within attack range, distance 0) → enemy reaches attack.
    spawn_player(&mut reg, Vec3::ZERO);
    let enemy = spawn_enemy(
        &mut reg,
        Vec3::ZERO,
        brain_with(tuning(), TEST_IDLE_STATE),
        50.0,
    );
    set_enemy_yaw(&mut reg, enemy, 1.2);
    let rot_before = reg.get_component::<Transform>(enemy).unwrap().rotation;
    set_agent_velocity(&mut reg, enemy, Vec3::ZERO);

    run_ai_tick(&mut reg, &mut warned, 0.016);
    assert_eq!(enemy_state_name(&reg, enemy), TEST_ATTACK_STATE);
    let rot = reg.get_component::<Transform>(enemy).unwrap().rotation;
    assert!(
        rot.x.is_finite() && rot.y.is_finite() && rot.z.is_finite() && rot.w.is_finite(),
        "zero-length facing direction must not write a NaN rotation (got {rot:?})",
    );
    assert!(
        rot.angle_between(rot_before) < 1e-5,
        "zero-length facing direction must leave the existing rotation unchanged",
    );
}

// ---------------------------------------------------------------------------
// Attack replay: the one-shot attack clip re-fires each in-state swing. The
// entry tick into `attack` plays the clip via the `state_changed` switch; every
// later in-state swing restarts the clip from frame 0. Damage cadence is
// unchanged (cooldown-gated, not frame-synced).
// ---------------------------------------------------------------------------

#[test]
fn repeated_in_attack_swing_restarts_the_attack_clip() {
    // Drive the enemy into `attack`, let the cooldown elapse, and confirm the
    // SECOND swing (an in-state swing, not the entry tick) restarts the attack
    // clip — observed as the entry stamp going pending again after a resolve had
    // filled it. The entry tick must NOT double-restart.
    let mut reg = EntityRegistry::new();
    let mut warned = AiRuntime::new();

    let pawn = spawn_player(&mut reg, Vec3::new(1.0, 0.0, 0.0));
    let enemy = spawn_enemy(
        &mut reg,
        Vec3::ZERO,
        brain_with(tuning(), TEST_IDLE_STATE),
        50.0,
    );

    let dt = 0.1; // 100ms/tick; cooldown 1000ms → 10 ticks between swings.

    // Tick 1: idle→attack (state change), first swing via the `state_changed`
    // switch. The switch leaves the new `attack` entry stamp pending.
    let events = run_ai_tick(&mut reg, &mut warned, dt);
    assert_eq!(events, vec![ENEMY_ATTACK_EVENT], "first swing lands");
    assert_eq!(enemy_state_name(&reg, enemy), TEST_ATTACK_STATE);
    assert_eq!(enemy_animation(&reg, enemy), "attack");
    assert!(
        enemy_anim_entered_at(&reg, enemy).is_none(),
        "entry switch leaves the attack clip stamp pending (frame 0)",
    );

    // Resolve the animation stamps (the per-frame resolve pass) so the attack
    // clip's entry stamp is filled — steady state, clip playing.
    postretro_entities::components::mesh::resolve_pending_animation_stamps(&mut reg, 5.0);
    assert_eq!(
        enemy_anim_entered_at(&reg, enemy),
        Some(5.0),
        "resolve pass fills the attack clip's entry stamp",
    );

    // Ticks 2..=10: still in `attack`, cooldown not elapsed → NO swing, so NO
    // restart. The resolved stamp must remain (no double/spurious restart).
    for _ in 0..9 {
        let events = run_ai_tick(&mut reg, &mut warned, dt);
        assert!(events.is_empty(), "no swing during cooldown");
    }
    assert_eq!(
        enemy_anim_entered_at(&reg, enemy),
        Some(5.0),
        "no in-state restart while the cooldown gates the swing",
    );

    // Tick 11: cooldown elapsed → the second (in-state) swing fires AND restarts
    // the attack clip. The enemy stays in `attack` (no state change), so this is
    // the restart path, not the entry switch.
    let events = run_ai_tick(&mut reg, &mut warned, dt);
    assert_eq!(events, vec![ENEMY_ATTACK_EVENT], "second swing lands");
    assert_eq!(
        enemy_state_name(&reg, enemy),
        TEST_ATTACK_STATE,
        "still in attack — this is an in-state swing, not a re-entry",
    );
    assert!(
        enemy_anim_entered_at(&reg, enemy).is_none(),
        "the in-state swing restarts the attack clip (stamp re-stamped pending)",
    );

    // Damage cadence is unchanged: two hits across the two swings, 8 each.
    assert_eq!(
        player_hp(&reg, pawn),
        84.0,
        "exactly two cooldown-gated hits — restart did not change damage timing",
    );
}

#[test]
fn attack_entry_tick_does_not_double_restart_the_clip() {
    // The entry tick into `attack` plays the clip via the `state_changed` switch
    // ONLY — the restart path is guarded off on that tick. Observed: after the
    // entry tick the fade bookkeeping reflects a single switch (no
    // `previous_state` from a redundant restart-over-switch), and a subsequent
    // resolve cleanly fills one entry stamp.
    let mut reg = EntityRegistry::new();
    let mut warned = AiRuntime::new();

    // Player inside attack range so idle→attack happens on tick 1 WITH a swing.
    spawn_player(&mut reg, Vec3::new(1.0, 0.0, 0.0));
    let enemy = spawn_enemy(
        &mut reg,
        Vec3::ZERO,
        brain_with(tuning(), TEST_IDLE_STATE),
        50.0,
    );

    let events = run_ai_tick(&mut reg, &mut warned, 0.1);
    assert_eq!(events, vec![ENEMY_ATTACK_EVENT]);
    assert_eq!(enemy_state_name(&reg, enemy), TEST_ATTACK_STATE);

    let anim = reg
        .get_component::<MeshComponent>(enemy)
        .unwrap()
        .animation
        .as_ref()
        .unwrap()
        .clone();
    // idle→attack is a hard cut here (test clips use crossfade 0), so the switch
    // records no `previous_state`. The key invariant: the entry tick produced one
    // clean pending entry stamp and no half-applied restart state on top of it.
    assert_eq!(anim.current_state, "attack");
    assert!(
        anim.entered_at.is_none(),
        "entry switch leaves a single pending stamp (frame 0)",
    );
    assert_eq!(
        anim.previous_state, None,
        "the entry tick switches once; the restart path is guarded off",
    );
}

// ---------------------------------------------------------------------------
// End-to-end pursuit around a corner: the live-play freeze regression.
//
// Observed in play: an enemy chasing the player froze mid-pursuit with
// `state:alert speed:0.00 arrived:false blocked:true has_path:false` after the
// player rounded a corner (and again when the player moved farther away while
// still inside its graph's authored pursuit range). Root cause: navmesh erosion
// leaves a wall-margin band
// that capsules legitimately occupy (an agent pushed wall-ward by full-speed
// corner rounding; a player hugging a corner to peek), and `find_path` returned
// `None` for any endpoint in that band — the steering tick then latched
// `blocked`, wiped the path, and held position forever. This test drives the
// REAL loop (FSM tick + steering tick, combat slots active) on a corner arena
// whose navmesh carries the eroded margin, with the enemy STARTING inside the
// band and the player peeking back into it, and asserts the pursuit lifecycle
// end to end.
// ---------------------------------------------------------------------------

/// Corner arena: a 12x12 floor with a solid box in the +X/-Z corner
/// (x in [5,12], z in [0,5]), so pursuit from the south-west must round the
/// box's corner. The navmesh (cell 0.5) leaves a half-unit ERODED MARGIN along
/// the two exposed box faces — exactly what the capsule-radius erosion bake
/// produces — so positions hugging those walls are off every region.
struct CornerArena;

impl CornerArena {
    const EXTENT: f32 = 12.0;
    const WALL_X: f32 = 5.0; // box -X face (z in [0, 5])
    const WALL_Z: f32 = 5.0; // box +Z face (x in [5, 12])
    const HEIGHT: f32 = 3.0;

    fn collision_world() -> CollisionWorld {
        let mut points: Vec<Point<f32>> = vec![
            Point::new(0.0, 0.0, 0.0),
            Point::new(Self::EXTENT, 0.0, 0.0),
            Point::new(Self::EXTENT, 0.0, Self::EXTENT),
            Point::new(0.0, 0.0, Self::EXTENT),
        ];
        let mut tris: Vec<[u32; 3]> = vec![[0, 1, 2], [0, 2, 3]];

        let mut push_wall = |x0: f32, z0: f32, x1: f32, z1: f32| {
            let base = points.len() as u32;
            points.push(Point::new(x0, 0.0, z0));
            points.push(Point::new(x1, 0.0, z1));
            points.push(Point::new(x1, Self::HEIGHT, z1));
            points.push(Point::new(x0, Self::HEIGHT, z0));
            tris.push([base, base + 1, base + 2]);
            tris.push([base, base + 2, base + 3]);
            tris.push([base, base + 2, base + 1]);
            tris.push([base, base + 3, base + 2]);
        };
        push_wall(Self::WALL_X, 0.0, Self::WALL_X, Self::WALL_Z); // box -X face
        push_wall(Self::WALL_X, Self::WALL_Z, Self::EXTENT, Self::WALL_Z); // box +Z face

        CollisionWorld {
            mesh: TriMesh::new(points, tris),
            isometry: Isometry::identity(),
        }
    }

    /// Regions stop half a unit short of the box faces (cell 0.5):
    ///   region 0 (west lane):  x [0, 4.5],  z [0, 5.5]
    ///   region 1 (north half): x [0, 12],   z [5.5, 12]
    /// joined along z = 5.5, x [0, 4.5].
    fn nav_graph() -> NavGraph {
        NavGraph::from_section(&NavMeshSection {
            version: NAVMESH_VERSION,
            origin: [0.0, 0.0, 0.0],
            cell_size: 0.5,
            dim_x: 24,
            dim_z: 24,
            agent_radius: 0.35,
            agent_height: 1.8,
            step_height: 0.4,
            max_slope_deg: 45.0,
            regions: vec![
                NavRegion {
                    x0: 0,
                    z0: 0,
                    x1: 9,
                    z1: 11,
                    floor_y_min: 0.0,
                    floor_y_max: 0.25,
                },
                NavRegion {
                    x0: 0,
                    z0: 11,
                    x1: 24,
                    z1: 24,
                    floor_y_min: 0.0,
                    floor_y_max: 0.25,
                },
            ],
            portals: vec![NavPortal {
                region_a: 0,
                region_b: 1,
                left: [0.0, 0.0, 5.5],
                right: [4.5, 0.0, 5.5],
            }],
        })
    }
}

#[test]
fn e2e_pursuit_around_corner_never_freezes_and_closes_on_the_target() {
    let world = CornerArena::collision_world();
    let graph = CornerArena::nav_graph();
    let mut reg = EntityRegistry::new();
    let mut warned = AiRuntime::new();

    let ry = chaser_rest_y();
    // The player: acquired in the open, rounds the corner through the west
    // lane, then peeks back around it — coming to rest HUGGING the box's +Z
    // face, inside the eroded margin (off every region) yet physically on
    // walkable floor, farther from the enemy's start but inside the graph's
    // authored aggro range.
    let player_waypoints = [
        Vec3::new(1.0, ry, 1.0),
        Vec3::new(1.5, ry, 6.5),
        Vec3::new(7.5, ry, 5.3),
    ];
    let player = spawn_player(&mut reg, player_waypoints[0]);
    set_hp(&mut reg, player, 1.0e6); // swings may land; the target must stay live

    // The enemy starts INSIDE the eroded margin beside the box's -X face —
    // exactly where full-speed corner rounding / collide-and-slide leaves a
    // pursuing capsule — in idle, so acquisition is exercised too.
    let enemy_start = Vec3::new(4.6, ry, 2.0);
    let enemy = spawn_enemy(
        &mut reg,
        enemy_start,
        brain_with(tuning(), TEST_IDLE_STATE),
        50.0,
    );

    // Fixture preconditions: both freeze-inducing positions sit OFF every
    // region (in the eroded band) — the exact inputs that latched the
    // permanent blocked state pre-fix.
    assert_eq!(graph.region_at(enemy_start), None);
    assert_eq!(graph.region_at(player_waypoints[2]), None);

    let player_speed = 2.0_f32;
    let player_at = |elapsed: f32| -> Vec3 {
        let mut budget = elapsed * player_speed;
        for seg in player_waypoints.windows(2) {
            let len = distance_xz(seg[0], seg[1]);
            if budget <= len || len <= 1e-6 {
                let t = if len <= 1e-6 { 0.0 } else { budget / len };
                return seg[0].lerp(seg[1], t);
            }
            budget -= len;
        }
        *player_waypoints.last().unwrap()
    };

    let attack_range = TEST_ATTACK_RANGE;
    let aggro_range = TEST_AGGRO_RANGE;
    let mut acquired = false;
    let mut planned = false;
    let mut min_distance = f32::INFINITY;

    for tick_index in 0..1200 {
        let p = player_at(tick_index as f32 * STEER_DT);
        move_player_to(&mut reg, player, p.x, p.z);

        // The REAL per-tick loop: FSM (with combat-slot selector inputs, as the
        // sim runs it) then steering.
        run_ai_tick_with_navigation(&mut reg, &mut warned, STEER_DT, Some(&graph), Some(&world));
        agent_steering::tick(&mut reg, &world, Some(&graph), STEER_GRAVITY, STEER_DT);

        let state = agent_steering::path_state(&reg, enemy).unwrap();
        let brain_state = enemy_state_name(&reg, enemy);
        let dist = distance_xz(state.position, p);
        min_distance = min_distance.min(dist);
        assert!(
            dist <= aggro_range,
            "fixture must keep the target inside the authored aggro range (tick {tick_index}, dist {dist})"
        );

        if brain_state != TEST_IDLE_STATE {
            acquired = true;
        }
        if acquired {
            // INV6: within its authored aggro range, a live target never de-aggros.
            assert_ne!(
                brain_state, TEST_IDLE_STATE,
                "enemy dropped aggro inside its authored range on tick {tick_index}"
            );
            // INV1: the frozen state — blocked, no path, not moving — is not a
            // legal resting state on ANY tick while the target is live and
            // inside its graph's authored pursuit range. This is the exact HUD
            // signature from live play.
            let speed_xz =
                (state.velocity.x * state.velocity.x + state.velocity.z * state.velocity.z).sqrt();
            let frozen = state.blocked && !state.has_path && speed_xz < 1.0e-3;
            assert!(
                !frozen,
                "enemy froze (blocked, no path, speed 0) on tick {tick_index}: pos={:?}, target={p:?}",
                state.position,
            );
        }
        if state.has_path {
            planned = true;
        }
    }

    assert!(acquired, "enemy must acquire the target (Idle -> Alert)");
    assert!(planned, "enemy must plan a route during the pursuit");
    assert!(
        min_distance <= attack_range + 0.75,
        "enemy never closed toward attack range around the corner: \
         closest approach {min_distance:.3} (attack range {attack_range})"
    );
}

// ---------------------------------------------------------------------------
// Acceptance: focused authored behavior-graph contracts
//
// The fixtures below exercise author-controlled policies that intentionally
// differ from the shared pursuit graph: interrupts, commitment windows, and
// entry events.
// ---------------------------------------------------------------------------

fn brain_input(name: &str) -> IrNode {
    IrNode::Input {
        name: name.to_string(),
        owner: None,
    }
}

fn target_within(distance: f32) -> IrNode {
    IrNode::Le {
        a: Box::new(brain_input(BRAIN_TARGET_DISTANCE_INPUT)),
        b: Box::new(IrNode::Const {
            value: IrValue::Number(distance),
        }),
    }
}

fn target_beyond(distance: f32) -> IrNode {
    IrNode::Gt {
        a: Box::new(brain_input(BRAIN_TARGET_DISTANCE_INPUT)),
        b: Box::new(IrNode::Const {
            value: IrValue::Number(distance),
        }),
    }
}

fn anchor_within(distance: f32) -> IrNode {
    IrNode::Le {
        a: Box::new(brain_input(BRAIN_DISTANCE_FROM_ANCHOR_INPUT)),
        b: Box::new(IrNode::Const {
            value: IrValue::Number(distance),
        }),
    }
}

fn anchor_beyond(distance: f32) -> IrNode {
    IrNode::Gt {
        a: Box::new(brain_input(BRAIN_DISTANCE_FROM_ANCHOR_INPUT)),
        b: Box::new(IrNode::Const {
            value: IrValue::Number(distance),
        }),
    }
}

fn when_acquisition_due(inner: IrNode) -> IrNode {
    IrNode::Select {
        cond: Box::new(brain_input(BRAIN_ACQUISITION_DUE_INPUT)),
        a: Box::new(inner),
        b: Box::new(IrNode::Const {
            value: IrValue::Bool(false),
        }),
    }
}

fn target_lost() -> IrNode {
    IrNode::Select {
        cond: Box::new(brain_input(BRAIN_HAS_TARGET_INPUT)),
        a: Box::new(IrNode::Const {
            value: IrValue::Bool(false),
        }),
        b: Box::new(IrNode::Const {
            value: IrValue::Bool(true),
        }),
    }
}

fn target_is_not_hostile() -> IrNode {
    IrNode::Select {
        cond: Box::new(brain_input(BRAIN_TARGET_HOSTILE_INPUT)),
        a: Box::new(IrNode::Const {
            value: IrValue::Bool(false),
        }),
        b: Box::new(IrNode::Const {
            value: IrValue::Bool(true),
        }),
    }
}

fn candidate_is_alive_within(distance: f32) -> IrNode {
    IrNode::Select {
        cond: Box::new(brain_input(CANDIDATE_DIED_INPUT)),
        a: Box::new(IrNode::Const {
            value: IrValue::Bool(false),
        }),
        b: Box::new(IrNode::Le {
            a: Box::new(brain_input(CANDIDATE_DISTANCE_INPUT)),
            b: Box::new(IrNode::Const {
                value: IrValue::Number(distance),
            }),
        }),
    }
}

fn authored_state(
    animation: &str,
    motion: MotionVerb,
    action: Option<ActionVerb>,
    transitions: Vec<TransitionDescriptor>,
) -> BehaviorStateDescriptor {
    BehaviorStateDescriptor {
        animation: animation.to_string(),
        motion,
        action,
        transitions,
        on_enter: None,
    }
}

fn edge(to: &str, when: IrNode) -> TransitionDescriptor {
    TransitionDescriptor {
        to: to.to_string(),
        when,
    }
}

/// A three-state pursuit graph over the shared `enemy_mesh` animation names:
/// `rest` (stand still) → `charge` (travel) → `strike` (contact damage), with
/// `strike` announcing its entry.
fn pursuit_graph() -> BehaviorGraphDescriptor {
    let mut strike = authored_state(
        "attack",
        MotionVerb::ChaseTarget,
        Some(ActionVerb::Attack),
        vec![edge("charge", target_beyond(2.0))],
    );
    strike.on_enter = Some("gruntSwings".to_string());
    BehaviorGraphDescriptor {
        initial: "rest".to_string(),
        states: BTreeMap::from([
            (
                "rest".to_string(),
                authored_state(
                    "idle",
                    MotionVerb::Hold,
                    None,
                    vec![edge("charge", target_within(16.0))],
                ),
            ),
            (
                "charge".to_string(),
                authored_state(
                    "locomotion",
                    MotionVerb::ChaseTarget,
                    None,
                    vec![edge("strike", target_within(2.0))],
                ),
            ),
            ("strike".to_string(), strike),
        ]),
        interrupts: Vec::new(),
        candidate_filter: None,
        patrol: None,
        attack: Some(AttackParams {
            damage: 8.0,
            range: 2.0,
            cooldown_ms: 1000.0,
        }),
        engagement_radius: None,
        move_speed: 3.5,
    }
}

fn authored_brain(graph: &BehaviorGraphDescriptor, state: &str) -> BrainComponent {
    let mut brain = BrainComponent::from_graph(graph);
    brain.state_index =
        graph_state_index(graph, state).expect("the authored graph declares the state");
    brain
}

#[test]
fn graph_candidate_filter_excludes_a_dead_pawn_and_acquires_a_live_one() {
    let mut graph = pursuit_graph();
    graph.candidate_filter = Some(IrNode::Select {
        cond: Box::new(IrNode::Input {
            name: "@candidate.died".into(),
            owner: None,
        }),
        a: Box::new(IrNode::Const {
            value: IrValue::Bool(false),
        }),
        b: Box::new(IrNode::Const {
            value: IrValue::Bool(true),
        }),
    });
    let mut registry = EntityRegistry::new();
    let dead = spawn_player(&mut registry, Vec3::new(2.0, 0.0, 0.0));
    let live = spawn_player(&mut registry, Vec3::new(4.0, 0.0, 0.0));
    let mut dead_health = registry
        .get_component::<HealthComponent>(dead)
        .unwrap()
        .clone();
    dead_health.death_handled = true;
    registry.set_component(dead, dead_health).unwrap();
    let enemy = spawn_enemy(
        &mut registry,
        Vec3::ZERO,
        authored_brain(&graph, "rest"),
        50.0,
    );
    let mut runtime = AiRuntime::new();

    run_ai_tick(&mut registry, &mut runtime, 0.016);

    assert_eq!(enemy_acquired_target(&registry, enemy), Some(live));
    assert_eq!(enemy_state_name(&registry, enemy), "charge");
}

#[test]
fn death_interrupt_releases_a_latched_target_and_filter_reacquires_a_live_pawn() {
    let mut graph = pursuit_graph();
    graph.interrupts = vec![edge(
        "rest",
        IrNode::Input {
            name: BRAIN_TARGET_DIED_INPUT.into(),
            owner: None,
        },
    )];
    graph.candidate_filter = Some(IrNode::Select {
        cond: Box::new(IrNode::Input {
            name: "@candidate.died".into(),
            owner: None,
        }),
        a: Box::new(IrNode::Const {
            value: IrValue::Bool(false),
        }),
        b: Box::new(IrNode::Const {
            value: IrValue::Bool(true),
        }),
    });
    let mut registry = EntityRegistry::new();
    let dying = spawn_player(&mut registry, Vec3::new(2.0, 0.0, 0.0));
    let enemy = spawn_enemy(
        &mut registry,
        Vec3::ZERO,
        authored_brain(&graph, "rest"),
        50.0,
    );
    let mut runtime = AiRuntime::new();

    run_ai_tick(&mut registry, &mut runtime, 0.016);
    assert_eq!(enemy_acquired_target(&registry, enemy), Some(dying));

    // The death sweep is a separate system. Latch it between AI ticks exactly
    // as the production sweep does, then offer another live pawn.
    let mut health = registry
        .get_component::<HealthComponent>(dying)
        .unwrap()
        .clone();
    health.current = 0.0;
    health.death_handled = true;
    registry.set_component(dying, health).unwrap();
    let live = spawn_player(&mut registry, Vec3::new(3.0, 0.0, 0.0));

    run_ai_tick(&mut registry, &mut runtime, 0.016);
    assert_eq!(enemy_state_name(&registry, enemy), "rest");
    assert_eq!(enemy_acquired_target(&registry, enemy), None);

    run_ai_tick(&mut registry, &mut runtime, 0.016);
    assert_eq!(enemy_acquired_target(&registry, enemy), Some(live));
    assert_eq!(enemy_state_name(&registry, enemy), "charge");
}

#[test]
fn candidate_filter_does_not_reprice_retained_target_think_stride() {
    let graph = BehaviorGraphDescriptor {
        initial: "alpha".to_string(),
        states: BTreeMap::from([
            (
                "alpha".to_string(),
                authored_state(
                    "locomotion",
                    MotionVerb::ChaseTarget,
                    None,
                    vec![edge(
                        "beta",
                        IrNode::Input {
                            name: BRAIN_ACQUISITION_DUE_INPUT.into(),
                            owner: None,
                        },
                    )],
                ),
            ),
            (
                "beta".to_string(),
                authored_state(
                    "locomotion",
                    MotionVerb::ChaseTarget,
                    None,
                    vec![edge(
                        "alpha",
                        IrNode::Input {
                            name: BRAIN_ACQUISITION_DUE_INPUT.into(),
                            owner: None,
                        },
                    )],
                ),
            ),
        ]),
        interrupts: Vec::new(),
        candidate_filter: Some(IrNode::Const {
            value: IrValue::Bool(false),
        }),
        patrol: None,
        attack: None,
        engagement_radius: None,
        move_speed: 3.5,
    };
    let mut registry = EntityRegistry::new();
    let retained_position = Vec3::new(35.0, 0.0, 0.0);
    let retained = spawn_player(&mut registry, retained_position);
    let mut brain = authored_brain(&graph, "alpha");
    brain.acquired_target = Some(retained);
    brain.think_stride_counter = 0;
    let enemy = spawn_enemy(&mut registry, Vec3::ZERO, brain, 50.0);
    let stride = think_stride_for_distance(distance_xz(retained_position, Vec3::ZERO));
    assert!(stride > 1, "fixture must exercise a strided distance band");
    let mut runtime = AiRuntime::new();
    let mut prior_state = enemy_state_name(&registry, enemy);
    let mut flips = 0;

    for tick in 1..=stride * 2 {
        run_ai_tick(&mut registry, &mut runtime, 0.016);
        let state = enemy_state_name(&registry, enemy);
        if state != prior_state {
            flips += 1;
            assert_eq!(
                tick % stride,
                0,
                "the acquisitionDue edge may fire only at the raw-distance stride"
            );
            prior_state = state;
        }
    }

    assert_eq!(
        flips, 2,
        "a filter that rejects offered candidates must not make a far retained target due every tick"
    );
    assert_eq!(enemy_acquired_target(&registry, enemy), Some(retained));
}

// Regression: a retained target must price the stride even when a nearer pawn
// has entered the offered set. The due tick still runs normal hysteresis and
// may switch then; only the off-stride ticks are committed to retention.
#[test]
fn retained_target_prices_stride_off_stride_holds_and_due_tick_allows_hysteresis() {
    let graph = pursuit_graph();
    let mut registry = EntityRegistry::new();
    let retained_position = Vec3::new(35.0, 0.0, 0.0);
    let retained = spawn_player(&mut registry, retained_position);
    let nearer = spawn_player(&mut registry, Vec3::new(1.0, 0.0, 0.0));
    let mut brain = authored_brain(&graph, "charge");
    brain.acquired_target = Some(retained);
    brain.think_stride_counter = 0;
    let stride = think_stride_for_distance(distance_xz(retained_position, Vec3::ZERO));
    assert!(stride > 1, "the retained target must use a strided band");
    let enemy = spawn_enemy(&mut registry, Vec3::ZERO, brain, 50.0);
    let mut runtime = AiRuntime::new();

    for tick in 1..stride {
        run_ai_tick(&mut registry, &mut runtime, 0.016);
        assert_eq!(
            enemy_acquired_target(&registry, enemy),
            Some(retained),
            "tick {tick} is off-stride and must not retarget to the nearer offer",
        );
    }

    run_ai_tick(&mut registry, &mut runtime, 0.016);
    assert_eq!(
        enemy_acquired_target(&registry, enemy),
        Some(nearer),
        "the retained target's due tick must run hysteresis and permit the meaningful switch",
    );
}

// Regression: without a retained target, the nearest raw offered distance
// prices acquisition even when a graph filter rejects that offer. The selected
// eligible candidate alone supplies BrainFacts and therefore decides guards.
#[test]
fn raw_nearest_offer_prices_stride_while_guards_read_the_farther_eligible_target() {
    let graph = BehaviorGraphDescriptor {
        initial: "rest".to_string(),
        states: BTreeMap::from([
            (
                "rest".to_string(),
                authored_state(
                    "idle",
                    MotionVerb::Hold,
                    None,
                    vec![
                        edge("selected_far", target_beyond(30.0)),
                        edge("selected_near", brain_input(BRAIN_HAS_TARGET_INPUT)),
                    ],
                ),
            ),
            (
                "selected_far".to_string(),
                authored_state("locomotion", MotionVerb::ChaseTarget, None, Vec::new()),
            ),
            (
                "selected_near".to_string(),
                authored_state("locomotion", MotionVerb::ChaseTarget, None, Vec::new()),
            ),
        ]),
        interrupts: Vec::new(),
        candidate_filter: Some(IrNode::Ge {
            a: Box::new(brain_input(CANDIDATE_DISTANCE_INPUT)),
            b: Box::new(IrNode::Const {
                value: IrValue::Number(30.0),
            }),
        }),
        patrol: None,
        attack: None,
        engagement_radius: None,
        move_speed: TEST_MOVE_SPEED,
    };
    let mut registry = EntityRegistry::new();
    let raw_near_position = Vec3::new(20.0, 0.0, 0.0);
    let rejected = spawn_player(&mut registry, raw_near_position);
    let eligible = spawn_player(&mut registry, Vec3::new(35.0, 0.0, 0.0));
    let raw_stride = think_stride_for_distance(distance_xz(raw_near_position, Vec3::ZERO));
    let eligible_stride = think_stride_for_distance(35.0);
    assert!(raw_stride > 1 && eligible_stride > raw_stride);

    let mut brain = authored_brain(&graph, "rest");
    brain.think_stride_counter = raw_stride - 1;
    assert_ne!(
        brain.think_stride_counter.wrapping_add(1) % eligible_stride,
        0,
        "the farther eligible target must not price this tick",
    );
    let enemy = spawn_enemy(&mut registry, Vec3::ZERO, brain, 50.0);
    let mut runtime = AiRuntime::new();

    run_ai_tick(&mut registry, &mut runtime, 0.016);

    assert_eq!(
        enemy_state_name(&registry, enemy),
        "selected_far",
        "the target-distance guard must observe the farther eligible target",
    );
    assert_eq!(enemy_acquired_target(&registry, enemy), Some(eligible));
    assert_ne!(enemy_acquired_target(&registry, enemy), Some(rejected));
}

// Regression: the raw nearest candidate used to price an off-stride tick bypassed
// candidate eligibility and became the brain's acquired target.
#[test]
fn off_stride_idle_brain_does_not_acquire_a_candidate_rejected_by_its_filter() {
    let mut graph = target_probe_graph();
    graph.candidate_filter = Some(IrNode::Const {
        value: IrValue::Bool(false),
    });
    let mut registry = EntityRegistry::new();
    let candidate_position = Vec3::new(35.0, 0.0, 0.0);
    spawn_player(&mut registry, candidate_position);
    let mut brain = authored_brain(&graph, "rest");
    brain.think_stride_counter = 0;
    let next_counter = brain.think_stride_counter.wrapping_add(1);
    let enemy = spawn_enemy(&mut registry, Vec3::ZERO, brain, 50.0);
    let stride = think_stride_for_distance(distance_xz(candidate_position, Vec3::ZERO));
    assert!(stride > 1, "fixture must exercise a strided distance band");
    assert_ne!(
        next_counter % stride,
        0,
        "the first tick must be outside the acquisition stride"
    );
    let mut runtime = AiRuntime::new();

    run_ai_tick(&mut registry, &mut runtime, 0.016);

    assert_eq!(
        enemy_state_name(&registry, enemy),
        "rest",
        "the hasTarget engagement guard must see no eligible target"
    );
    assert_eq!(
        enemy_acquired_target(&registry, enemy),
        None,
        "raw nearest distance may price the stride but must not acquire the rejected pawn"
    );
}

#[test]
fn target_died_latch_becomes_visible_after_a_same_ai_tick_kill_and_sweep() {
    let observer_graph = BehaviorGraphDescriptor {
        initial: "rest".to_string(),
        states: BTreeMap::from([
            (
                "rest".to_string(),
                authored_state("idle", MotionVerb::Hold, None, Vec::new()),
            ),
            (
                "watch".to_string(),
                authored_state("locomotion", MotionVerb::ChaseTarget, None, Vec::new()),
            ),
        ]),
        interrupts: vec![edge(
            "rest",
            IrNode::Input {
                name: BRAIN_TARGET_DIED_INPUT.into(),
                owner: None,
            },
        )],
        candidate_filter: Some(IrNode::Select {
            cond: Box::new(IrNode::Input {
                name: CANDIDATE_DIED_INPUT.into(),
                owner: None,
            }),
            a: Box::new(IrNode::Const {
                value: IrValue::Bool(false),
            }),
            b: Box::new(IrNode::Const {
                value: IrValue::Bool(true),
            }),
        }),
        patrol: None,
        attack: None,
        engagement_radius: None,
        move_speed: 3.5,
    };
    let mut attacker_graph = pursuit_graph();
    attacker_graph.attack = Some(AttackParams {
        damage: 100.0,
        range: 2.0,
        cooldown_ms: 1000.0,
    });

    let mut registry = EntityRegistry::new();
    let pawn = spawn_player(&mut registry, Vec3::new(1.0, 0.0, 0.0));
    let mut observer_brain = authored_brain(&observer_graph, "watch");
    observer_brain.acquired_target = Some(pawn);
    let observer = spawn_enemy(&mut registry, Vec3::ZERO, observer_brain, 50.0);
    let mut attacker_brain = authored_brain(&attacker_graph, "strike");
    attacker_brain.acquired_target = Some(pawn);
    spawn_enemy(&mut registry, Vec3::ZERO, attacker_brain, 50.0);
    let mut runtime = AiRuntime::new();

    run_ai_tick(&mut registry, &mut runtime, 0.016);

    let health = registry.get_component::<HealthComponent>(pawn).unwrap();
    assert_eq!(health.current, 0.0, "the other enemy killed the pawn");
    assert!(
        !health.death_handled,
        "damage in the AI apply pass does not preempt the later death sweep"
    );
    assert_eq!(
        enemy_state_name(&registry, observer),
        "watch",
        "all brains read targetDied=false during the compute pass before damage lands"
    );

    crate::scripting_systems::health::sweep_deaths(&mut registry);
    assert!(
        registry
            .get_component::<HealthComponent>(pawn)
            .unwrap()
            .death_handled,
        "the death sweep commits the latch between AI ticks"
    );

    run_ai_tick(&mut registry, &mut runtime, 0.016);
    assert_eq!(enemy_state_name(&registry, observer), "rest");
    assert_eq!(enemy_acquired_target(&registry, observer), None);
}

fn enemy_time_in_state(reg: &EntityRegistry, enemy: EntityId) -> f32 {
    reg.get_component::<BrainComponent>(enemy)
        .unwrap()
        .time_in_state_ms
}

#[test]
fn an_authored_graph_walks_its_states_and_raises_the_entered_state_on_enter() {
    let mut reg = EntityRegistry::new();
    let mut runtime = AiRuntime::new();
    let graph = pursuit_graph();

    let pawn = spawn_player(&mut reg, Vec3::new(1.0, 0.0, 0.0));
    let enemy = spawn_enemy(&mut reg, Vec3::ZERO, authored_brain(&graph, "rest"), 50.0);

    // `rest` declares one edge, so reaching contact takes two ticks.
    let first = run_ai_tick(&mut reg, &mut runtime, 0.016);
    assert_eq!(enemy_state_name(&reg, enemy), "charge");
    assert!(first.is_empty(), "`charge` announces nothing");
    assert_eq!(enemy_animation(&reg, enemy), "idle", "stopped travel rests");

    let second = run_ai_tick(&mut reg, &mut runtime, 0.016);
    assert_eq!(enemy_state_name(&reg, enemy), "strike");
    assert_eq!(
        second,
        vec![
            Cow::Owned("gruntSwings".to_string()),
            Cow::Borrowed(ENEMY_ATTACK_EVENT),
        ],
        "the entry event precedes the action the entered state takes"
    );
    assert_eq!(player_hp(&reg, pawn), 92.0);
    assert_eq!(
        enemy_animation(&reg, enemy),
        "attack",
        "an acting state keeps its own animation at a standstill"
    );

    // Entering is what raises `on_enter`: staying does not re-raise it.
    let third = run_ai_tick(&mut reg, &mut runtime, 0.016);
    assert_eq!(enemy_state_name(&reg, enemy), "strike");
    assert!(
        third.is_empty(),
        "no re-entry, no event, no in-cooldown swing"
    );
}

#[test]
fn a_guard_fires_mid_attack_cooldown_and_mid_one_shot_clip() {
    // Interruptible by default: nothing an enemy is doing — a one-shot attack
    // clip, an armed cooldown — defers guard evaluation to a later tick.
    let mut reg = EntityRegistry::new();
    let mut runtime = AiRuntime::new();
    let graph = pursuit_graph();

    let pawn = spawn_player(&mut reg, Vec3::new(1.0, 0.0, 0.0));
    let enemy = spawn_enemy(&mut reg, Vec3::ZERO, authored_brain(&graph, "strike"), 50.0);

    run_ai_tick(&mut reg, &mut runtime, 0.016);
    assert_eq!(enemy_state_name(&reg, enemy), "strike");
    assert!(
        reg.get_component::<BrainComponent>(enemy)
            .unwrap()
            .attack_cooldown_remaining_ms
            > 900.0,
        "the swing armed a full cooldown"
    );
    // Stage the one-shot swing clip as the live animation: the brain was seeded
    // straight into `strike`, so no entry switch requested it.
    set_enemy_animation(&mut reg, enemy, "attack");

    // The pawn steps out of contact range with ~984ms of cooldown left and the
    // one-shot clip still on screen.
    let mut transform = *reg.get_component::<Transform>(pawn).unwrap();
    transform.position = Vec3::new(6.0, 0.0, 0.0);
    reg.set_component(pawn, transform).unwrap();

    run_ai_tick(&mut reg, &mut runtime, 0.016);

    assert_eq!(
        enemy_state_name(&reg, enemy),
        "charge",
        "the exit guard fires on the tick it becomes true, not after the cooldown"
    );
}

#[test]
fn a_time_in_state_guard_exits_on_the_first_tick_the_window_elapses() {
    const WINDOW_MS: f32 = 500.0;
    const TICK_DT: f32 = 0.016;

    let graph = BehaviorGraphDescriptor {
        initial: "rest".to_string(),
        states: BTreeMap::from([
            (
                "rest".to_string(),
                authored_state(
                    "idle",
                    MotionVerb::Hold,
                    None,
                    vec![edge("commit", target_within(16.0))],
                ),
            ),
            (
                "commit".to_string(),
                authored_state(
                    "attack",
                    MotionVerb::Freeze,
                    None,
                    vec![edge(
                        "rest",
                        IrNode::Ge {
                            a: Box::new(brain_input(BRAIN_TIME_IN_STATE_MS_INPUT)),
                            b: Box::new(IrNode::Const {
                                value: IrValue::Number(WINDOW_MS),
                            }),
                        },
                    )],
                ),
            ),
        ]),
        interrupts: Vec::new(),
        candidate_filter: None,
        patrol: None,
        attack: None,
        engagement_radius: None,
        move_speed: 3.5,
    };

    let mut reg = EntityRegistry::new();
    let mut runtime = AiRuntime::new();
    spawn_player(&mut reg, Vec3::new(5.0, 0.0, 0.0));
    let enemy = spawn_enemy(&mut reg, Vec3::ZERO, authored_brain(&graph, "rest"), 50.0);

    run_ai_tick(&mut reg, &mut runtime, TICK_DT);
    assert_eq!(enemy_state_name(&reg, enemy), "commit");
    assert_eq!(
        enemy_time_in_state(&reg, enemy),
        0.0,
        "entering a state restarts its clock"
    );

    // The window is a whole number of ticks plus a remainder, so the exit lands
    // on the first tick whose accrued time reaches it — never before.
    let ticks_to_elapse = (WINDOW_MS / (TICK_DT * 1000.0)).ceil() as u32;
    for tick in 1..ticks_to_elapse {
        run_ai_tick(&mut reg, &mut runtime, TICK_DT);
        assert_eq!(
            enemy_state_name(&reg, enemy),
            "commit",
            "the commitment window still holds at tick {tick}"
        );
    }
    run_ai_tick(&mut reg, &mut runtime, TICK_DT);
    assert_eq!(
        enemy_state_name(&reg, enemy),
        "rest",
        "the exit fires on the first tick the window elapses"
    );
}

/// A graph whose `rest` state has a state-local edge that is true whenever the
/// listed interrupts are, so precedence is observable in one tick.
fn interrupt_graph(interrupts: Vec<TransitionDescriptor>) -> BehaviorGraphDescriptor {
    BehaviorGraphDescriptor {
        initial: "rest".to_string(),
        states: BTreeMap::from([
            (
                "rest".to_string(),
                authored_state(
                    "idle",
                    MotionVerb::Hold,
                    None,
                    vec![edge("charge", target_within(16.0))],
                ),
            ),
            (
                "charge".to_string(),
                authored_state("locomotion", MotionVerb::ChaseTarget, None, Vec::new()),
            ),
            (
                "flee".to_string(),
                authored_state("death", MotionVerb::Hold, None, Vec::new()),
            ),
            (
                "panic".to_string(),
                authored_state("attack", MotionVerb::Freeze, None, Vec::new()),
            ),
        ]),
        interrupts,
        candidate_filter: None,
        patrol: None,
        attack: None,
        engagement_radius: None,
        move_speed: 3.5,
    }
}

#[test]
fn an_interrupt_wins_over_a_simultaneously_true_state_local_edge_in_declaration_order() {
    let graph = interrupt_graph(vec![
        edge("flee", target_within(16.0)),
        edge("panic", target_within(16.0)),
    ]);
    let mut reg = EntityRegistry::new();
    let mut runtime = AiRuntime::new();
    spawn_player(&mut reg, Vec3::new(5.0, 0.0, 0.0));
    let enemy = spawn_enemy(&mut reg, Vec3::ZERO, authored_brain(&graph, "rest"), 50.0);

    run_ai_tick(&mut reg, &mut runtime, 0.016);

    assert_eq!(
        enemy_state_name(&reg, enemy),
        "flee",
        "interrupts precede the current state's own edges, and the first \
         declared interrupt wins"
    );
}

#[test]
fn an_interrupt_fires_from_any_state() {
    // The same interrupt set, entered from a state that declares no edges at
    // all: any-state means any state.
    let graph = interrupt_graph(vec![edge("panic", target_within(16.0))]);
    let mut reg = EntityRegistry::new();
    let mut runtime = AiRuntime::new();
    spawn_player(&mut reg, Vec3::new(5.0, 0.0, 0.0));
    let enemy = spawn_enemy(&mut reg, Vec3::ZERO, authored_brain(&graph, "charge"), 50.0);

    run_ai_tick(&mut reg, &mut runtime, 0.016);

    assert_eq!(enemy_state_name(&reg, enemy), "panic");
}

#[test]
fn a_self_targeting_interrupt_is_skipped_rather_than_re_entering_its_own_state() {
    let graph = interrupt_graph(vec![edge("rest", target_within(16.0))]);
    let mut reg = EntityRegistry::new();
    let mut runtime = AiRuntime::new();
    spawn_player(&mut reg, Vec3::new(5.0, 0.0, 0.0));
    let enemy = spawn_enemy(&mut reg, Vec3::ZERO, authored_brain(&graph, "rest"), 50.0);

    run_ai_tick(&mut reg, &mut runtime, 0.016);

    assert_eq!(
        enemy_state_name(&reg, enemy),
        "charge",
        "an interrupt back into the current state is skipped, so the \
         state-local edge behind it still decides the tick"
    );
}

#[test]
fn a_closed_aggro_gate_forces_an_authored_graph_to_its_initial_state() {
    let mut reg = EntityRegistry::new();
    let mut runtime = AiRuntime::new();
    let graph = pursuit_graph();

    spawn_player(&mut reg, Vec3::new(5.0, 0.0, 0.0));
    let enemy = spawn_enemy(&mut reg, Vec3::ZERO, authored_brain(&graph, "rest"), 50.0);

    run_ai_tick(&mut reg, &mut runtime, 0.016);
    assert_eq!(enemy_state_name(&reg, enemy), "charge");
    assert!(
        agent_steering::path_state(&reg, enemy)
            .expect("agent present")
            .has_destination
    );

    set_enemy_aggro_armed(&mut reg, enemy, false);
    run_ai_tick(&mut reg, &mut runtime, 0.016);

    assert_eq!(
        enemy_state_name(&reg, enemy),
        graph.initial,
        "a closed gate stands the brain down to its authored initial state"
    );
    assert_eq!(enemy_acquired_target(&reg, enemy), None);
    assert!(
        !agent_steering::path_state(&reg, enemy)
            .expect("agent present")
            .has_destination,
        "standing down clears steering"
    );

    set_enemy_aggro_armed(&mut reg, enemy, true);
    run_ai_tick(&mut reg, &mut runtime, 0.016);

    assert_eq!(
        enemy_state_name(&reg, enemy),
        "charge",
        "re-arming resumes ordinary evaluation"
    );
    assert!(
        agent_steering::path_state(&reg, enemy)
            .expect("agent present")
            .has_destination
    );
}

#[test]
fn an_authored_state_with_an_undeclared_animation_keeps_the_prior_one() {
    let mut graph = pursuit_graph();
    graph.states.get_mut("charge").unwrap().animation = "sprint".to_string();

    let mut reg = EntityRegistry::new();
    let mut runtime = AiRuntime::new();
    spawn_player(&mut reg, Vec3::new(5.0, 0.0, 0.0));
    let enemy = spawn_enemy(&mut reg, Vec3::ZERO, authored_brain(&graph, "rest"), 50.0);
    set_enemy_animation(&mut reg, enemy, "death");
    // Moving, so the travel animation is requested rather than the rest
    // substitution — the unresolvable name is what the tick tries to switch to.
    set_agent_velocity(&mut reg, enemy, Vec3::new(3.0, 0.0, 0.0));

    run_ai_tick(&mut reg, &mut runtime, 0.016);

    assert_eq!(enemy_state_name(&reg, enemy), "charge");
    assert_eq!(
        enemy_animation(&reg, enemy),
        "death",
        "an undeclared animation name keeps the prior animation"
    );
    assert!(runtime.warned.contains("anim:sprint"));

    run_ai_tick(&mut reg, &mut runtime, 0.016);
    assert_eq!(
        runtime
            .warned
            .iter()
            .filter(|key| *key == "anim:sprint")
            .count(),
        1,
        "warned once per distinct name"
    );
}

#[test]
fn standing_down_clears_steering_even_when_the_initial_state_chases() {
    // The engine floor clears steering outright when it forces the resting
    // state: with no target to pursue, the resting state's motion verb has
    // nothing to act on.
    let mut graph = pursuit_graph();
    graph.initial = "charge".to_string();

    let mut reg = EntityRegistry::new();
    let mut runtime = AiRuntime::new();
    spawn_player(&mut reg, Vec3::new(5.0, 0.0, 0.0));
    let enemy = spawn_enemy(&mut reg, Vec3::ZERO, authored_brain(&graph, "charge"), 50.0);

    run_ai_tick(&mut reg, &mut runtime, 0.016);
    assert!(
        agent_steering::path_state(&reg, enemy)
            .expect("agent present")
            .has_destination
    );

    set_enemy_aggro_armed(&mut reg, enemy, false);
    run_ai_tick(&mut reg, &mut runtime, 0.016);

    assert_eq!(enemy_state_name(&reg, enemy), "charge");
    assert!(
        !agent_steering::path_state(&reg, enemy)
            .expect("agent present")
            .has_destination,
        "a closed gate clears steering regardless of the resting state's verb"
    );
}

/// A chase-then-petrify graph: `charge` pursues, and a per-entity `halt` field
/// drops it into a `freeze` state that declares no exits. `stop` takes no action
/// and does not chase, so entering it also releases the enemy's combat slot.
fn petrifying_graph() -> BehaviorGraphDescriptor {
    BehaviorGraphDescriptor {
        initial: "charge".to_string(),
        states: BTreeMap::from([
            (
                "charge".to_string(),
                authored_state(
                    "locomotion",
                    MotionVerb::ChaseTarget,
                    None,
                    vec![edge(
                        "stop",
                        IrNode::Ge {
                            a: Box::new(brain_input("@state.halt")),
                            b: Box::new(IrNode::Const {
                                value: IrValue::Number(1.0),
                            }),
                        },
                    )],
                ),
            ),
            (
                "stop".to_string(),
                authored_state("death", MotionVerb::Freeze, None, Vec::new()),
            ),
        ]),
        interrupts: Vec::new(),
        candidate_filter: None,
        patrol: None,
        attack: None,
        engagement_radius: None,
        move_speed: 3.5,
    }
}

#[test]
fn entering_a_freeze_state_stops_path_following_and_releases_the_combat_slot() {
    // `freeze` touches steering exactly once, on ENTRY. Without that clear, a
    // `set_destination` that deliberately preserves the existing path would keep
    // walking the agent into ground it has just surrendered to the slot solver.
    let floor = OpenFloor::new();
    let world = floor.collision_world();
    let graph = floor.nav_graph();

    let mut reg = EntityRegistry::new();
    let mut runtime = AiRuntime::new();
    let behavior = petrifying_graph();
    let player_pos = Vec3::new(20.0, chaser_rest_y(), 20.0);
    spawn_player(&mut reg, player_pos);
    let enemy = spawn_enemy(
        &mut reg,
        Vec3::new(14.0, chaser_rest_y(), 20.0),
        authored_brain(&behavior, "charge"),
        50.0,
    );

    // Actively chasing: a destination is issued and a combat slot is claimed.
    run_ai_tick_with_navigation(&mut reg, &mut runtime, STEER_DT, Some(&graph), Some(&world));
    assert_eq!(enemy_state_name(&reg, enemy), "charge");
    assert!(
        agent_steering::path_state(&reg, enemy)
            .expect("agent present")
            .has_destination,
        "the chasing state steers before the freeze",
    );
    assert!(
        enemy_combat_slot(&reg, enemy).is_some(),
        "an engaged chaser claims a slot around its target",
    );

    // The transition into the freeze state.
    reg.entity_state_mut(enemy)
        .expect("spawn seeds entity state")
        .set("halt", 1.0);
    run_ai_tick_with_navigation(&mut reg, &mut runtime, STEER_DT, Some(&graph), Some(&world));

    assert_eq!(enemy_state_name(&reg, enemy), "stop");
    assert!(
        !agent_steering::path_state(&reg, enemy)
            .expect("agent present")
            .has_destination,
        "entering a freeze state stops path-following",
    );
    assert_eq!(
        enemy_combat_slot(&reg, enemy),
        None,
        "a frozen enemy is no longer engaged, so its slot is surrendered",
    );
    assert_eq!(
        enemy_acquired_target(&reg, enemy),
        None,
        "and it no longer holds the pawn it was chasing",
    );

    // ...and touches nothing thereafter. A destination written by something else
    // (a scripted mover, a death slide) must survive: the clear is on entry, not
    // per tick.
    agent_steering::set_destination(&mut reg, enemy, Vec3::new(24.0, chaser_rest_y(), 20.0));
    run_ai_tick_with_navigation(&mut reg, &mut runtime, STEER_DT, Some(&graph), Some(&world));

    assert_eq!(enemy_state_name(&reg, enemy), "stop");
    assert!(
        agent_steering::path_state(&reg, enemy)
            .expect("agent present")
            .has_destination,
        "a settled freeze state leaves steering alone — the clear fires on entry only",
    );
}

// ---------------------------------------------------------------------------
// Acceptance: the shipped direct-graph reference enemy
//
// `sdk/behaviors/reference/entities.{ts,luau}` ships the reference enemy as an
// explicit `components.behavior` graph. These restate its graph in Rust and
// pin the direct authoring against the shipped Luau source.
// ---------------------------------------------------------------------------

/// The shipped authored graph, restated in Rust: the same states, the same
/// declaration order, and the same guards `sdk/behaviors/reference/entities.ts`
/// builds through `runtime.select` / `runtime.le` / `runtime.gt` over `brain.*`.
///
/// Detection is conjoined with `@brain.acquisitionDue` (the IR has no `and`
/// opcode, so conjunction is `select(cond, inner, false)`). The ordered
/// interrupts stand down into the active untargeted patrol, first for target
/// loss and then for a retained target that turns friendly. Alert and attack
/// enter the authored anchor-distance retreat, whose only local exit resumes
/// the ping-pong patrol.
///
/// This is the source-shape oracle, so a drifted transcription would assert
/// nothing —
/// [`the_reference_oracle_matches_the_shipped_authored_graph`] pins it against
/// the shipped Luau source so the drift cannot happen silently.
fn reference_behavior_graph() -> BehaviorGraphDescriptor {
    const DETECTION_RANGE: f32 = 16.0;
    const ATTACK_RANGE: f32 = 2.0;
    const LEASH_RANGE: f32 = 20.0;
    const RETURN_ARRIVAL_EPSILON: f32 = 1.0;
    BehaviorGraphDescriptor {
        initial: "patrol".to_string(),
        states: BTreeMap::from([
            (
                "patrol".to_string(),
                authored_state(
                    "walk",
                    MotionVerb::Patrol,
                    None,
                    vec![edge(
                        "alert",
                        when_acquisition_due(target_within(DETECTION_RANGE)),
                    )],
                ),
            ),
            (
                "alert".to_string(),
                authored_state(
                    "walk",
                    MotionVerb::ChaseTarget,
                    None,
                    vec![
                        edge("attack", target_within(ATTACK_RANGE)),
                        edge("retreat", anchor_beyond(LEASH_RANGE)),
                    ],
                ),
            ),
            (
                "attack".to_string(),
                authored_state(
                    "attack",
                    MotionVerb::ChaseTarget,
                    Some(ActionVerb::Attack),
                    vec![
                        edge("retreat", anchor_beyond(LEASH_RANGE)),
                        edge("alert", target_beyond(ATTACK_RANGE)),
                    ],
                ),
            ),
            (
                "retreat".to_string(),
                authored_state(
                    "walk",
                    MotionVerb::MoveToAnchor,
                    None,
                    vec![edge("patrol", anchor_within(RETURN_ARRIVAL_EPSILON))],
                ),
            ),
        ]),
        // This order is author-visible policy: target loss must precede the
        // untargeted-or-friendly polarity of targetHostile, and both must land
        // on the active untargeted resting state.
        interrupts: vec![
            edge("patrol", target_lost()),
            edge("patrol", target_is_not_hostile()),
        ],
        candidate_filter: None,
        patrol: Some(PatrolDescriptor {
            points: vec![[0.0, 0.0], [6.0, 0.0], [6.0, 6.0]],
            mode: PatrolMode::PingPong,
        }),
        attack: Some(AttackParams {
            damage: 8.0,
            range: ATTACK_RANGE,
            cooldown_ms: 1200.0,
        }),
        engagement_radius: Some(ATTACK_RANGE),
        move_speed: 3.0,
    }
}

/// The reference enemy's `components.behavior` block as the SHIPPED
/// `sdk/behaviors/reference/entities.luau` actually authors it.
///
/// The module is evaluated verbatim against the same SDK helper modules the
/// engine's Luau prelude installs (`sdk/lib/runtime.luau` for the IR builders,
/// `sdk/lib/brain.luau` for the `@brain.*` leaves), with `defineEntity` stubbed
/// to the identity it is for a plain descriptor. Reading the guards out of the
/// real source is the whole point: a second hand-written copy would prove only
/// that someone edited both.
fn shipped_reference_behavior_graph() -> BehaviorGraphDescriptor {
    use mlua::LuaSerdeExt as _;

    const RUNTIME_LUAU_SRC: &str = include_str!("../../../../../sdk/lib/runtime.luau");
    const BRAIN_LUAU_SRC: &str = include_str!("../../../../../sdk/lib/brain.luau");
    const ENTITIES_LUAU_SRC: &str =
        include_str!("../../../../../sdk/behaviors/reference/entities.luau");

    let lua = mlua::Lua::new();
    let runtime: mlua::Table = lua
        .load(RUNTIME_LUAU_SRC)
        .set_name("sdk/lib/runtime.luau")
        .eval()
        .expect("runtime.luau evaluates");
    let brain_sdk: mlua::Table = lua
        .load(BRAIN_LUAU_SRC)
        .set_name("sdk/lib/brain.luau")
        .eval()
        .expect("brain.luau evaluates");
    let brain: mlua::Table = brain_sdk.get("brain").expect("brain.luau exports `brain`");
    let candidate: mlua::Table = brain_sdk
        .get("candidate")
        .expect("brain.luau exports `candidate`");
    let define_entity = lua
        .create_function(|_, descriptor: mlua::Table| Ok(descriptor))
        .expect("stub defineEntity");
    let globals = lua.globals();
    globals.set("runtime", runtime).unwrap();
    globals.set("brain", brain).unwrap();
    globals.set("candidate", candidate).unwrap();
    globals.set("defineEntity", define_entity).unwrap();

    let module: mlua::Table = lua
        .load(ENTITIES_LUAU_SRC)
        .set_name("sdk/behaviors/reference/entities.luau")
        .eval()
        .expect("the shipped reference entities module evaluates");
    let entity: mlua::Table = module
        .get("referenceEnemyEntity")
        .expect("the module exports `referenceEnemyEntity`");
    let components: mlua::Table = entity
        .get("components")
        .expect("the archetype has components");
    let behavior: mlua::Value = components
        .get("behavior")
        .expect("the reference enemy is authored as a behavior graph");
    lua.from_value(behavior)
        .expect("the shipped behavior block deserializes into the engine descriptor")
}

/// The oracle below is a Rust hand-transcription of a graph that ships as
/// TypeScript and Luau. This is what stops it becoming fiction: retune a range,
/// reorder an edge, or drop the stand-down interrupt in the shipped source and
/// this fails instead of a stale hand transcription silently remaining green.
///
/// It reads the Luau authoring; that the TypeScript twin says the same thing is
/// `the_shipped_reference_enemy_graph_is_identical_in_both_authorings`
/// (`scripting-core`, where the twin-parser machinery lives). Together the two
/// close the loop: TS ≡ Luau ≡ this oracle.
#[test]
fn the_reference_oracle_matches_the_shipped_authored_graph() {
    assert_eq!(
        reference_behavior_graph(),
        shipped_reference_behavior_graph(),
        "the Rust oracle has drifted from `sdk/behaviors/reference/entities.luau`",
    );
}

/// One tick of observable brain output: the state it settled in, the damage it
/// had dealt by then, the animation it is requesting, and whether it is steering
/// anywhere.
#[derive(Debug, Clone, PartialEq)]
struct BrainTrace {
    state: String,
    player_hp: f32,
    animation: String,
    has_destination: bool,
    acquired: bool,
}

/// Where the player stands on `tick` — out of detection, inside detection,
/// in contact, backing off, and finally past its authored stand-down range.
fn reference_player_x(tick: u32) -> f32 {
    match tick {
        0..=9 => 30.0,
        10..=39 => 10.0,
        40..=139 => 1.5,
        140..=169 => 6.0,
        _ => 80.0,
    }
}

/// Run the scripted approach against one brain and record its per-tick output.
fn trace_reference_fixture(brain: BrainComponent) -> Vec<BrainTrace> {
    const TICKS: u32 = 200;
    let mut reg = EntityRegistry::new();
    let mut runtime = AiRuntime::new();
    let pawn = spawn_player(&mut reg, Vec3::new(reference_player_x(0), 0.0, 0.0));
    let enemy = spawn_enemy(&mut reg, Vec3::ZERO, brain, 50.0);

    (0..TICKS)
        .map(|tick| {
            let mut transform = *reg.get_component::<Transform>(pawn).unwrap();
            transform.position = Vec3::new(reference_player_x(tick), 0.0, 0.0);
            reg.set_component(pawn, transform).unwrap();

            run_ai_tick(&mut reg, &mut runtime, 0.016);

            BrainTrace {
                state: enemy_state_name(&reg, enemy),
                player_hp: player_hp(&reg, pawn),
                animation: enemy_animation(&reg, enemy),
                has_destination: agent_steering::path_state(&reg, enemy)
                    .expect("agent present")
                    .has_destination,
                acquired: enemy_acquired_target(&reg, enemy).is_some(),
            }
        })
        .collect()
}

#[test]
fn the_direct_reference_graph_exercises_the_shipped_patrol_and_pursuit_shape() {
    let authored = trace_reference_fixture(BrainComponent::from_graph(&reference_behavior_graph()));

    // The fixture has to actually exercise the loop, or "identical" is vacuous.
    let states: Vec<&str> = authored
        .iter()
        .map(|row| row.state.as_str())
        .collect::<std::collections::BTreeSet<_>>()
        .into_iter()
        .collect();
    assert_eq!(
        states,
        vec!["alert", "attack", "patrol"],
        "the approach must visit active rest, pursuit, and contact"
    );
    let damage_ticks: Vec<usize> = authored
        .windows(2)
        .enumerate()
        .filter(|(_, pair)| pair[1].player_hp < pair[0].player_hp)
        .map(|(index, _)| index + 1)
        .collect();
    assert!(
        damage_ticks.len() >= 2,
        "the fixture must span more than one attack cooldown: {damage_ticks:?}"
    );
    assert!(
        authored.iter().any(|row| row.animation == "attack")
            && authored.iter().any(|row| row.animation == "walk"),
        "the fixture must switch animation at least once"
    );
}

/// Target loss must stand the graph down into its active patrol in one tick
/// rather than walking `attack` through an unrelated local exit first.
#[test]
fn reference_graph_stands_down_from_attack_in_one_tick_when_the_target_is_lost() {
    let graph = reference_behavior_graph();
    let mut reg = EntityRegistry::new();
    let mut runtime = AiRuntime::new();
    // No player pawn is spawned at all — the disconnect / level-transition /
    // last-co-op-pawn-leaves case.
    let enemy = spawn_enemy(&mut reg, Vec3::ZERO, authored_brain(&graph, "attack"), 50.0);
    assert_eq!(enemy_state_name(&reg, enemy), "attack");

    run_ai_tick(&mut reg, &mut runtime, 0.016);

    assert_eq!(
        enemy_state_name(&reg, enemy),
        "patrol",
        "losing the target must stand the brain down in ONE tick, not walk it \
         back through the pursuit state on the distance sentinel",
    );
    assert_eq!(
        enemy_animation(&reg, enemy),
        graph.states["patrol"].animation,
        "the stand-down lands on the graph's active rest animation",
    );
    assert_eq!(
        enemy_destination(&reg, enemy),
        Some(Vec3::new(6.0, 0.0, 0.0)),
        "the active patrol stand-down resumes its persisted route instead of parking",
    );
}

#[test]
fn reference_graph_enters_retreat_from_chase_and_attack_beyond_its_authored_leash() {
    for state in ["alert", "attack"] {
        let graph = reference_behavior_graph();
        let mut reg = EntityRegistry::new();
        let mut runtime = AiRuntime::new();
        let target = spawn_player(&mut reg, Vec3::new(25.0, 0.0, 0.0));
        let mut brain = authored_brain(&graph, state);
        brain.acquired_target = Some(target);
        brain.think_stride_counter = 0;
        let enemy = spawn_enemy(&mut reg, Vec3::ZERO, brain, 50.0);
        set_enemy_position(&mut reg, enemy, Vec3::new(21.0, 0.0, 0.0));

        run_ai_tick(&mut reg, &mut runtime, 0.016);

        assert_eq!(
            enemy_state_name(&reg, enemy),
            "retreat",
            "the authored distanceFromAnchor leash must route `{state}` to retreat in one tick",
        );
        assert_eq!(enemy_acquired_target(&reg, enemy), None);
        assert_eq!(
            enemy_destination(&reg, enemy),
            Some(Vec3::ZERO),
            "retreat from `{state}` begins steering toward the spawn anchor",
        );
    }
}

// ---------------------------------------------------------------------------
// Acceptance: impact policy writes `@state.*`, an authored interrupt reads it
// ---------------------------------------------------------------------------

/// The stagger shape: `pursuit_graph` plus a `flinch` state and the any-state
/// interrupt that enters it when a per-entity `staggered` field is set.
fn staggerable_graph() -> BehaviorGraphDescriptor {
    let mut graph = pursuit_graph();
    graph.states.insert(
        "flinch".to_string(),
        authored_state("death", MotionVerb::Hold, None, Vec::new()),
    );
    graph.interrupts = vec![edge(
        "flinch",
        IrNode::Ge {
            a: Box::new(brain_input("@state.staggered")),
            b: Box::new(IrNode::Const {
                value: IrValue::Number(1.0),
            }),
        },
    )];
    graph
}

/// An impact policy that sets `@state.staggered` on any `staggerable` entity
/// the damage chokepoint reports a hit on — the authored half of the stagger
/// shape, exactly as a mod would declare it.
fn stagger_impact_policy() -> ImpactEventDescriptor {
    ImpactEventDescriptor {
        id: "reference_stagger".to_string(),
        is_override: false,
        levels: Vec::new(),
        filter_tag: Some("staggerable".to_string()),
        policy: vec![serde_json::json!({
            "primitive": "setState",
            "target": "@impact.target",
            "args": {
                "name": "staggered",
                "value": { "op": "const", "value": 1.0 },
            },
        })],
    }
}

#[test]
fn an_impact_policy_write_fires_an_authored_state_interrupt_on_the_next_tick() {
    let graph = staggerable_graph();
    let mut reg = EntityRegistry::new();
    let mut runtime = AiRuntime::new();
    spawn_player(&mut reg, Vec3::new(5.0, 0.0, 0.0));
    let enemy = spawn_enemy(&mut reg, Vec3::ZERO, authored_brain(&graph, "rest"), 50.0);
    reg.set_tags(enemy, vec!["staggerable".to_string()])
        .expect("enemy is live");

    let mut policies = ImpactPolicyRuntime::new(ScriptCtx::new());
    policies.replace_global_events(vec![stagger_impact_policy()]);

    // Baseline: nothing has written the field, so the interrupt stays false and
    // the state-local edge decides the tick.
    run_ai_tick(&mut reg, &mut runtime, 0.016);
    assert_eq!(enemy_state_name(&reg, enemy), "charge");

    // A weapon lands on the enemy. The chokepoint publishes the dispatch and
    // the policy writes `@state.staggered` inside the same tick.
    apply_damage_with_context(
        &mut reg,
        enemy,
        &DamagePayload { amount: 5.0 },
        DamageContext::new("test.weapon", DamageProducer::InTick),
    );
    policies.evaluate_pending_in_registry(&mut reg);
    assert_eq!(
        reg.get_component::<EntityStateComponent>(enemy)
            .expect("spawn seeds entity state")
            .get("staggered"),
        1.0,
        "the impact policy wrote the per-entity field the guard reads"
    );

    // Next AI tick: the authored interrupt reads that field and wins over the
    // current state's own edges.
    run_ai_tick(&mut reg, &mut runtime, 0.016);
    assert_eq!(
        enemy_state_name(&reg, enemy),
        "flinch",
        "an authored `@state` interrupt fires on the first AI tick after the write"
    );
}

// ---------------------------------------------------------------------------
// Acceptance: guards run on every armed tick, with or without a target
//
// The aggro gate is the ONE thing upstream of guard evaluation. Having no pawn
// to chase is not: the brain evaluates its whole guard set against the no-target
// facts (`@brain.hasTarget` false, `@brain.targetDistance` at its sentinel).
// These drive the real tick — evaluating the scope directly would not prove the
// tick ever reaches it.
// ---------------------------------------------------------------------------

#[test]
fn an_armed_enemy_with_no_target_still_fires_its_interrupts() {
    // The monster-closet shape: a sealed enemy nobody is standing in front of
    // takes a hit and must still flinch. Before guards ran without a target this
    // was unreachable — the aggro gate is open, there is simply nobody to chase.
    let graph = staggerable_graph();
    let mut reg = EntityRegistry::new();
    let mut runtime = AiRuntime::new();
    // Deliberately no player pawn: nothing for the engine floor to acquire.
    let enemy = spawn_enemy(&mut reg, Vec3::ZERO, authored_brain(&graph, "rest"), 50.0);

    run_ai_tick(&mut reg, &mut runtime, 0.016);
    assert_eq!(
        enemy_state_name(&reg, enemy),
        "rest",
        "no guard is true yet, so the brain stays where it is"
    );

    reg.entity_state_mut(enemy)
        .expect("spawn seeds entity state")
        .set("staggered", 1.0);
    run_ai_tick(&mut reg, &mut runtime, 0.016);

    assert_eq!(
        enemy_state_name(&reg, enemy),
        "flinch",
        "an interrupt reaches a brain that has no target to chase"
    );
}

/// `pursuit_graph` with `rest` re-armed to expose what a guard SEES when there
/// is no target: one edge reads `@brain.hasTarget` directly, the other is a bare
/// range guard that has to read false through the no-target sentinel.
fn target_probe_graph() -> BehaviorGraphDescriptor {
    let mut graph = pursuit_graph();
    graph.states.get_mut("rest").unwrap().transitions = vec![
        edge("charge", brain_input(BRAIN_HAS_TARGET_INPUT)),
        edge("strike", target_within(16.0)),
    ];
    graph
}

#[test]
fn with_no_target_a_has_target_guard_and_a_bare_range_guard_both_read_false() {
    let graph = target_probe_graph();
    let mut reg = EntityRegistry::new();
    let mut runtime = AiRuntime::new();
    let enemy = spawn_enemy(&mut reg, Vec3::ZERO, authored_brain(&graph, "rest"), 50.0);

    run_ai_tick(&mut reg, &mut runtime, 0.016);

    assert_eq!(
        enemy_state_name(&reg, enemy),
        "rest",
        "`hasTarget` is false and the sentinel distance fails a bare range \
         guard, so neither edge fires"
    );

    // The same two guards, now with a pawn to read: `hasTarget` is declared
    // first, so it is the one that wins.
    spawn_player(&mut reg, Vec3::new(5.0, 0.0, 0.0));
    run_ai_tick(&mut reg, &mut runtime, 0.016);

    assert_eq!(enemy_state_name(&reg, enemy), "charge");
}

#[test]
fn a_closed_aggro_gate_evaluates_no_guards_at_all() {
    // An interrupt that cannot be false: if the gate consulted the guards for
    // any reason, this brain would land in `panic`. It must land in `initial`.
    let graph = interrupt_graph(vec![edge(
        "panic",
        IrNode::Const {
            value: IrValue::Bool(true),
        },
    )]);
    let mut reg = EntityRegistry::new();
    let mut runtime = AiRuntime::new();
    spawn_player(&mut reg, Vec3::new(5.0, 0.0, 0.0));
    let mut brain = authored_brain(&graph, "charge");
    brain.aggro_armed = false;
    let enemy = spawn_enemy(&mut reg, Vec3::ZERO, brain, 50.0);
    // A stale destination from before the gate closed, so the stand-down's
    // clear is observable.
    agent_steering::set_destination(&mut reg, enemy, Vec3::new(5.0, 0.0, 0.0));

    run_ai_tick(&mut reg, &mut runtime, 0.016);

    assert_eq!(
        enemy_state_name(&reg, enemy),
        graph.initial,
        "a closed gate forces `initial` without consulting a single guard"
    );
    assert_eq!(enemy_acquired_target(&reg, enemy), None);
    assert!(
        !agent_steering::path_state(&reg, enemy)
            .expect("agent present")
            .has_destination,
        "standing down clears steering"
    );

    set_enemy_aggro_armed(&mut reg, enemy, true);
    run_ai_tick(&mut reg, &mut runtime, 0.016);

    assert_eq!(
        enemy_state_name(&reg, enemy),
        "panic",
        "re-arming lets the same interrupt through immediately"
    );
}

#[test]
fn a_brain_seated_outside_its_graph_recovers_to_the_initial_state() {
    // A graph swapped under a persisted `state_index` leaves the brain pointing
    // at no declared state. The tick is TOTAL: it re-seats rather than wedging,
    // and warns once for the enemy.
    let graph = pursuit_graph();
    let mut reg = EntityRegistry::new();
    let mut runtime = AiRuntime::new();
    let mut brain = authored_brain(&graph, "rest");
    brain.state_index = 99;
    let enemy = spawn_enemy(&mut reg, Vec3::ZERO, brain, 50.0);

    run_ai_tick(&mut reg, &mut runtime, 0.016);

    assert_eq!(
        enemy_state_name(&reg, enemy),
        graph.initial,
        "an unaddressable state index re-seats to the graph's `initial`"
    );
    assert!(
        runtime.reseat_warned.contains(&enemy),
        "the re-seat reports through the entity-keyed warn latch: {:?}",
        runtime.reseat_warned
    );
    assert!(
        runtime.warned.is_empty(),
        "the re-seat is entity-keyed, so it must not add a per-spawn string to \
         the content latch: {:?}",
        runtime.warned
    );

    // Re-seated, not merely parked: the recovered state's own edges decide the
    // next tick.
    spawn_player(&mut reg, Vec3::new(5.0, 0.0, 0.0));
    run_ai_tick(&mut reg, &mut runtime, 0.016);
    assert_eq!(enemy_state_name(&reg, enemy), "charge");
}

// ---------------------------------------------------------------------------
// Acceptance: `attack.range` gates damage, and engagement is not motion-shaped
// ---------------------------------------------------------------------------

/// A stand-and-swing graph: one state, `hold` motion plus the `attack` action
/// and no exits at all. Nothing but the engine's `attack.range` gate can stop it
/// dealing damage, and nothing but the ENGAGED test can make it hold a target or
/// turn toward one.
fn standing_attack_graph() -> BehaviorGraphDescriptor {
    BehaviorGraphDescriptor {
        initial: "strike".to_string(),
        states: BTreeMap::from([(
            "strike".to_string(),
            authored_state(
                "attack",
                MotionVerb::Hold,
                Some(ActionVerb::Attack),
                Vec::new(),
            ),
        )]),
        interrupts: Vec::new(),
        candidate_filter: None,
        patrol: None,
        attack: Some(AttackParams {
            damage: 8.0,
            range: 2.0,
            cooldown_ms: 1000.0,
        }),
        engagement_radius: None,
        move_speed: 3.5,
    }
}

#[test]
fn an_attack_state_deals_no_damage_outside_attack_range() {
    let graph = standing_attack_graph();
    let mut reg = EntityRegistry::new();
    let mut runtime = AiRuntime::new();
    let pawn = spawn_player(&mut reg, Vec3::new(5.0, 0.0, 0.0));
    let enemy = spawn_enemy(&mut reg, Vec3::ZERO, authored_brain(&graph, "strike"), 50.0);

    let events = run_ai_tick(&mut reg, &mut runtime, 0.016);
    assert!(
        events.is_empty(),
        "an out-of-range swing raises no attack event"
    );
    assert_eq!(
        player_hp(&reg, pawn),
        100.0,
        "`attack.range` gates the damage, not just the authored transitions"
    );
    assert_eq!(
        enemy_state_name(&reg, enemy),
        "strike",
        "the state is unchanged: only the action was gated"
    );

    // The same state, the same cooldown, the pawn now inside the range.
    let mut transform = *reg.get_component::<Transform>(pawn).unwrap();
    transform.position = Vec3::new(1.0, 0.0, 0.0);
    reg.set_component(pawn, transform).unwrap();

    let events = run_ai_tick(&mut reg, &mut runtime, 0.016);
    assert_eq!(events, vec![ENEMY_ATTACK_EVENT]);
    assert_eq!(player_hp(&reg, pawn), 92.0);
}

#[test]
fn a_standing_attack_state_retains_its_target_and_faces_it() {
    // Engagement is not motion-shaped: a state that holds its ground and swings
    // keeps its acquired pawn and turns toward it, exactly as a chasing state
    // does. Otherwise it swings at whatever its spawn heading happened to be.
    let graph = standing_attack_graph();
    let mut reg = EntityRegistry::new();
    let mut runtime = AiRuntime::new();
    let pawn = spawn_player(&mut reg, Vec3::new(1.5, 0.0, 0.0));
    let enemy = spawn_enemy(&mut reg, Vec3::ZERO, authored_brain(&graph, "strike"), 50.0);
    // Facing away, and stopped — `hold` never gives the agent a destination.
    set_enemy_yaw(&mut reg, enemy, std::f32::consts::PI);
    set_agent_velocity(&mut reg, enemy, Vec3::ZERO);

    run_ai_tick(&mut reg, &mut runtime, 0.016);
    assert_eq!(
        enemy_acquired_target(&reg, enemy),
        Some(pawn),
        "a swinging state is engaged, so it retains its pawn across ticks"
    );

    run_ticks_until_facing_converges(
        &mut reg,
        &mut runtime,
        enemy,
        Vec3::X,
        "a standing attacker turns to face what it is hitting",
    );
    assert_faces(
        enemy_forward_xz(&reg, enemy),
        Vec3::X,
        "a standing attacker turns to face what it is hitting",
    );
    assert!(
        !agent_steering::path_state(&reg, enemy)
            .expect("agent present")
            .has_destination,
        "engagement turns and retains; it does not steer — that stays the \
         motion verb's job"
    );
}

// ---------------------------------------------------------------------------
// Position-goal motion verbs: anchor return and authored patrol routes.
// ---------------------------------------------------------------------------

fn position_goal_graph(
    motion: MotionVerb,
    patrol: Option<PatrolDescriptor>,
) -> BehaviorGraphDescriptor {
    BehaviorGraphDescriptor {
        initial: "position".to_string(),
        states: BTreeMap::from([(
            "position".to_string(),
            authored_state("locomotion", motion, None, Vec::new()),
        )]),
        interrupts: Vec::new(),
        candidate_filter: None,
        patrol,
        attack: None,
        engagement_radius: None,
        move_speed: TEST_MOVE_SPEED,
    }
}

fn patrol_graph(points: Vec<[f32; 2]>, mode: PatrolMode) -> BehaviorGraphDescriptor {
    position_goal_graph(MotionVerb::Patrol, Some(PatrolDescriptor { points, mode }))
}

/// The reference enemy's authored policy in a compact test fixture: patrol is
/// the active untargeted state, engagement may cross the authored leash, and a
/// position-goal retreat returns to the persistent route.
fn retreat_patrol_graph() -> BehaviorGraphDescriptor {
    const DETECTION: f32 = 16.0;
    const ATTACK_RANGE: f32 = 2.0;
    const LEASH: f32 = 4.0;
    const ARRIVAL: f32 = 1.0;

    BehaviorGraphDescriptor {
        initial: "patrol".to_string(),
        states: BTreeMap::from([
            (
                "patrol".to_string(),
                authored_state(
                    "locomotion",
                    MotionVerb::Patrol,
                    None,
                    vec![edge(
                        "alert",
                        when_acquisition_due(target_within(DETECTION)),
                    )],
                ),
            ),
            (
                "alert".to_string(),
                authored_state(
                    "locomotion",
                    MotionVerb::ChaseTarget,
                    None,
                    vec![
                        edge("attack", target_within(ATTACK_RANGE)),
                        edge("retreat", anchor_beyond(LEASH)),
                    ],
                ),
            ),
            (
                "attack".to_string(),
                authored_state(
                    "attack",
                    MotionVerb::ChaseTarget,
                    Some(ActionVerb::Attack),
                    vec![
                        edge("retreat", anchor_beyond(LEASH)),
                        edge("alert", target_beyond(ATTACK_RANGE)),
                    ],
                ),
            ),
            (
                "retreat".to_string(),
                authored_state(
                    "locomotion",
                    MotionVerb::MoveToAnchor,
                    None,
                    vec![edge("patrol", anchor_within(ARRIVAL))],
                ),
            ),
        ]),
        // The lost-target row deliberately precedes the friendly-flip row.
        // Both must target patrol, the active untargeted state, to avoid a
        // patrol/rest oscillation after the target disappears.
        interrupts: vec![
            edge("patrol", target_lost()),
            edge("patrol", target_is_not_hostile()),
        ],
        candidate_filter: None,
        patrol: Some(PatrolDescriptor {
            points: vec![[0.0, 0.0], [3.0, 0.0]],
            mode: PatrolMode::PingPong,
        }),
        attack: Some(AttackParams {
            damage: TEST_ATTACK_DAMAGE,
            range: ATTACK_RANGE,
            cooldown_ms: TEST_ATTACK_COOLDOWN_MS,
        }),
        engagement_radius: Some(ATTACK_RANGE),
        move_speed: TEST_MOVE_SPEED,
    }
}

fn enemy_destination(registry: &EntityRegistry, enemy: EntityId) -> Option<Vec3> {
    registry
        .get_component::<AgentComponent>(enemy)
        .expect("enemy has an agent")
        .destination
}

#[test]
fn position_goal_modes_are_locomotion_states_when_they_take_no_action() {
    for (motion, patrol) in [
        (MotionVerb::MoveToAnchor, None),
        (
            MotionVerb::Patrol,
            Some(PatrolDescriptor {
                points: vec![[0.0, 0.0]],
                mode: PatrolMode::Loop,
            }),
        ),
    ] {
        let mut graph = position_goal_graph(motion, patrol);
        graph.initial = "idle".to_string();
        graph.states.insert(
            "idle".to_string(),
            authored_state("idle", MotionVerb::Hold, None, Vec::new()),
        );
        let position_index = graph_state_index(&graph, "position").unwrap();
        assert_eq!(
            animation_for_state(&graph, position_index, false),
            Some("idle"),
            "{motion:?} yields its travel clip to the rest pose when stopped"
        );
        assert_eq!(
            animation_for_state(&graph, position_index, true),
            Some("locomotion")
        );
        assert_eq!(locomotion_animation(&graph), Some("locomotion"));
    }
}

#[test]
fn move_to_anchor_clears_on_arrival_and_reissues_after_a_later_push() {
    let graph = position_goal_graph(MotionVerb::MoveToAnchor, None);
    let mut registry = EntityRegistry::new();
    let mut runtime = AiRuntime::new();
    let enemy = spawn_enemy(
        &mut registry,
        Vec3::ZERO,
        authored_brain(&graph, "position"),
        50.0,
    );
    let pawn = spawn_player(&mut registry, Vec3::new(1.0, 0.0, 0.0));

    set_enemy_position(&mut registry, enemy, Vec3::new(2.0, 0.0, 0.0));
    run_ai_tick(&mut registry, &mut runtime, 0.016);
    assert_eq!(enemy_destination(&registry, enemy), Some(Vec3::ZERO));
    let brain = registry.get_component::<BrainComponent>(enemy).unwrap();
    assert_eq!(
        brain.acquired_target, None,
        "position goals are non-engaged"
    );
    assert_eq!(
        brain.combat_slot, None,
        "position goals claim no combat slot"
    );
    assert_ne!(brain.acquired_target, Some(pawn));

    set_enemy_position(
        &mut registry,
        enemy,
        Vec3::new(POSITION_GOAL_ARRIVAL_EPSILON, 0.0, 0.0),
    );
    run_ai_tick(&mut registry, &mut runtime, 0.016);
    assert_eq!(
        enemy_destination(&registry, enemy),
        None,
        "arrival clears rather than latches a stale destination"
    );
    run_ai_tick(&mut registry, &mut runtime, 0.016);
    assert_eq!(
        enemy_destination(&registry, enemy),
        None,
        "still arrived stays clear"
    );

    set_enemy_position(&mut registry, enemy, Vec3::new(1.0, 0.0, 0.0));
    run_ai_tick(&mut registry, &mut runtime, 0.016);
    assert_eq!(
        enemy_destination(&registry, enemy),
        Some(Vec3::ZERO),
        "a later external push past epsilon reissues the anchor goal"
    );
}

#[test]
fn patrol_loop_and_ping_pong_advance_cursor_through_anchor_relative_points() {
    let mut registry = EntityRegistry::new();
    let mut runtime = AiRuntime::new();
    let loop_graph = patrol_graph(vec![[0.0, 0.0], [2.0, 0.0]], PatrolMode::Loop);
    let loop_enemy = spawn_enemy(
        &mut registry,
        Vec3::ZERO,
        authored_brain(&loop_graph, "position"),
        50.0,
    );

    run_ai_tick(&mut registry, &mut runtime, 0.016);
    assert_eq!(
        registry
            .get_component::<BrainComponent>(loop_enemy)
            .unwrap()
            .patrol_cursor,
        1
    );
    assert_eq!(
        enemy_destination(&registry, loop_enemy),
        Some(Vec3::new(2.0, 0.0, 0.0))
    );
    set_enemy_position(&mut registry, loop_enemy, Vec3::new(2.0, 0.0, 0.0));
    run_ai_tick(&mut registry, &mut runtime, 0.016);
    assert_eq!(
        registry
            .get_component::<BrainComponent>(loop_enemy)
            .unwrap()
            .patrol_cursor,
        0,
        "loop wraps after the final point"
    );
    assert_eq!(enemy_destination(&registry, loop_enemy), Some(Vec3::ZERO));

    let ping_graph = patrol_graph(
        vec![[0.0, 0.0], [1.0, 0.0], [2.0, 0.0]],
        PatrolMode::PingPong,
    );
    let ping_enemy = spawn_enemy(
        &mut registry,
        Vec3::ZERO,
        authored_brain(&ping_graph, "position"),
        50.0,
    );
    for (position, cursor, direction, destination) in [
        (Vec3::ZERO, 1, 1, Vec3::new(1.0, 0.0, 0.0)),
        (Vec3::new(1.0, 0.0, 0.0), 2, 1, Vec3::new(2.0, 0.0, 0.0)),
        (Vec3::new(2.0, 0.0, 0.0), 1, -1, Vec3::new(1.0, 0.0, 0.0)),
        (Vec3::new(1.0, 0.0, 0.0), 0, -1, Vec3::ZERO),
        (Vec3::ZERO, 1, 1, Vec3::new(1.0, 0.0, 0.0)),
    ] {
        set_enemy_position(&mut registry, ping_enemy, position);
        run_ai_tick(&mut registry, &mut runtime, 0.016);
        let brain = registry
            .get_component::<BrainComponent>(ping_enemy)
            .unwrap();
        assert_eq!(brain.patrol_cursor, cursor);
        assert_eq!(brain.patrol_direction, direction);
        assert_eq!(enemy_destination(&registry, ping_enemy), Some(destination));
    }
}

#[test]
fn patrol_single_point_persists_cursor_and_clamps_a_stale_saved_cursor() {
    let graph = patrol_graph(vec![[0.0, 0.0]], PatrolMode::PingPong);
    let mut registry = EntityRegistry::new();
    let mut runtime = AiRuntime::new();
    let mut brain = authored_brain(&graph, "position");
    brain.patrol_cursor = 99;
    let enemy = spawn_enemy(&mut registry, Vec3::ZERO, brain, 50.0);
    agent_steering::set_destination(&mut registry, enemy, Vec3::new(5.0, 0.0, 0.0));

    run_ai_tick(&mut registry, &mut runtime, 0.016);
    let brain = registry.get_component::<BrainComponent>(enemy).unwrap();
    assert_eq!(
        brain.patrol_cursor, 0,
        "a stale cursor is normalized before indexing"
    );
    assert_eq!(
        brain.patrol_direction, 1,
        "a fresh ping-pong route starts forward"
    );
    assert_eq!(
        enemy_destination(&registry, enemy),
        None,
        "an arrived one-point patrol clears a stale destination and stands still"
    );

    let mut graph = patrol_graph(vec![[0.0, 0.0], [3.0, 0.0]], PatrolMode::Loop);
    graph.states.insert(
        "rest".to_string(),
        authored_state("idle", MotionVerb::Hold, None, Vec::new()),
    );
    let enemy = spawn_enemy(
        &mut registry,
        Vec3::ZERO,
        authored_brain(&graph, "position"),
        50.0,
    );
    run_ai_tick(&mut registry, &mut runtime, 0.016);
    assert_eq!(
        registry
            .get_component::<BrainComponent>(enemy)
            .unwrap()
            .patrol_cursor,
        1
    );
    let mut brain = registry
        .get_component::<BrainComponent>(enemy)
        .unwrap()
        .clone();
    brain.state_index = graph_state_index(&graph, "rest").unwrap();
    registry.set_component(enemy, brain).unwrap();
    run_ai_tick(&mut registry, &mut runtime, 0.016);
    let mut brain = registry
        .get_component::<BrainComponent>(enemy)
        .unwrap()
        .clone();
    brain.state_index = graph_state_index(&graph, "position").unwrap();
    registry.set_component(enemy, brain).unwrap();
    run_ai_tick(&mut registry, &mut runtime, 0.016);
    assert_eq!(
        registry
            .get_component::<BrainComponent>(enemy)
            .unwrap()
            .patrol_cursor,
        1,
        "leaving and re-entering patrol preserves its route phase"
    );
    assert_eq!(
        enemy_destination(&registry, enemy),
        Some(Vec3::new(3.0, 0.0, 0.0))
    );
}

#[test]
fn position_goal_facing_uses_travel_only_and_never_falls_through_to_a_target() {
    let graph = position_goal_graph(MotionVerb::MoveToAnchor, None);
    let mut registry = EntityRegistry::new();
    let mut runtime = AiRuntime::new();
    spawn_player(&mut registry, Vec3::new(0.0, 0.0, 2.0));
    let enemy = spawn_enemy(
        &mut registry,
        Vec3::ZERO,
        authored_brain(&graph, "position"),
        50.0,
    );
    set_enemy_yaw(&mut registry, enemy, std::f32::consts::PI);
    set_agent_velocity(&mut registry, enemy, Vec3::ZERO);

    run_ai_tick(&mut registry, &mut runtime, 0.016);
    assert!(
        yaw_distance(enemy_yaw(&registry, enemy), std::f32::consts::PI) < EPS,
        "an arrived position goal must not turn toward the nearest scanned pawn"
    );

    set_enemy_position(&mut registry, enemy, Vec3::new(2.0, 0.0, 0.0));
    set_agent_velocity(&mut registry, enemy, Vec3::NEG_X);
    run_ticks_until_facing_converges(
        &mut registry,
        &mut runtime,
        enemy,
        Vec3::NEG_X,
        "a travelling position goal faces its travel velocity",
    );
    assert_eq!(
        enemy_acquired_target(&registry, enemy),
        None,
        "position-goal facing does not make the state engaged"
    );
}

#[test]
fn patrol_stand_down_interrupt_is_skipped_while_the_route_cursor_advances() {
    let mut graph = patrol_graph(vec![[0.0, 0.0], [3.0, 0.0]], PatrolMode::PingPong);
    graph.interrupts = vec![edge("position", target_lost())];
    let mut registry = EntityRegistry::new();
    let mut runtime = AiRuntime::new();
    let enemy = spawn_enemy(
        &mut registry,
        Vec3::ZERO,
        authored_brain(&graph, "position"),
        50.0,
    );

    run_ai_tick(&mut registry, &mut runtime, 0.016);

    let brain = registry
        .get_component::<BrainComponent>(enemy)
        .expect("enemy keeps its brain");
    assert_eq!(brain.state_name(), Some("position"));
    assert_eq!(brain.patrol_cursor, 1);
    assert_eq!(
        enemy_destination(&registry, enemy),
        Some(Vec3::new(3.0, 0.0, 0.0))
    );
}

#[test]
fn retreat_returns_to_patrol_at_the_anchor_and_resumes_its_route() {
    let graph = retreat_patrol_graph();
    let mut registry = EntityRegistry::new();
    let mut runtime = AiRuntime::new();
    spawn_player(&mut registry, Vec3::new(1.0, 0.0, 0.0));
    let enemy = spawn_enemy(
        &mut registry,
        Vec3::ZERO,
        authored_brain(&graph, "alert"),
        50.0,
    );

    // The target drew the engaged enemy farther than its graph-authored leash.
    // Alert chooses retreat and the position goal points directly at home.
    set_enemy_position(&mut registry, enemy, Vec3::new(5.0, 0.0, 0.0));
    run_ai_tick(&mut registry, &mut runtime, 0.016);
    assert_eq!(enemy_state_name(&registry, enemy), "retreat");
    assert_eq!(enemy_destination(&registry, enemy), Some(Vec3::ZERO));
    assert_eq!(enemy_acquired_target(&registry, enemy), None);

    // The authored guard uses 1 m, safely above the engine's 0.5 m resolver
    // epsilon. Arrival switches to patrol, whose cursor continues with point 1.
    set_enemy_position(&mut registry, enemy, Vec3::new(0.5, 0.0, 0.0));
    run_ai_tick(&mut registry, &mut runtime, 0.016);
    assert_eq!(enemy_state_name(&registry, enemy), "patrol");
    assert_eq!(enemy_acquired_target(&registry, enemy), None);
    assert_eq!(
        enemy_destination(&registry, enemy),
        Some(Vec3::new(3.0, 0.0, 0.0))
    );
}

fn retreat_patrol_tick_trace() -> (String, usize, i8, Option<Vec3>) {
    let graph = retreat_patrol_graph();
    let mut registry = EntityRegistry::new();
    let mut runtime = AiRuntime::new();
    spawn_player(&mut registry, Vec3::new(1.0, 0.0, 0.0));
    let enemy = spawn_enemy(
        &mut registry,
        Vec3::ZERO,
        authored_brain(&graph, "alert"),
        50.0,
    );

    set_enemy_position(&mut registry, enemy, Vec3::new(5.0, 0.0, 0.0));
    run_ai_tick(&mut registry, &mut runtime, 0.016);
    set_enemy_position(&mut registry, enemy, Vec3::new(0.5, 0.0, 0.0));
    run_ai_tick(&mut registry, &mut runtime, 0.016);

    let brain = registry
        .get_component::<BrainComponent>(enemy)
        .expect("enemy keeps its brain");
    (
        brain.state_name().expect("declared state").to_string(),
        brain.patrol_cursor,
        brain.patrol_direction,
        enemy_destination(&registry, enemy),
    )
}

#[test]
fn retreat_and_patrol_ticks_are_deterministic_for_identical_host_inputs() {
    assert_eq!(retreat_patrol_tick_trace(), retreat_patrol_tick_trace());
}

#[test]
fn retreat_with_no_non_due_target_stands_down_to_patrol_instead_of_stranding() {
    let graph = retreat_patrol_graph();
    let mut registry = EntityRegistry::new();
    let mut runtime = AiRuntime::new();
    spawn_player(&mut registry, Vec3::new(30.0, 0.0, 0.0));
    let mut brain = authored_brain(&graph, "retreat");
    // The 30 m nearest offer is in a strided band. Counter zero makes the next
    // tick non-due, so a non-engaged retreat reads its no-target facts even
    // while the pawn remains in the registry.
    brain.think_stride_counter = 0;
    let enemy = spawn_enemy(&mut registry, Vec3::ZERO, brain, 50.0);
    set_enemy_position(&mut registry, enemy, Vec3::new(5.0, 0.0, 0.0));

    run_ai_tick(&mut registry, &mut runtime, 0.016);

    assert_eq!(enemy_state_name(&registry, enemy), "patrol");
    assert_eq!(enemy_acquired_target(&registry, enemy), None);
    assert_eq!(enemy_destination(&registry, enemy), Some(Vec3::ZERO));
}

#[test]
fn an_arrival_guard_below_the_engine_epsilon_leaves_a_position_goal_wedged() {
    let mut graph = position_goal_graph(MotionVerb::MoveToAnchor, None);
    graph.states.get_mut("position").unwrap().transitions = vec![edge(
        "done",
        anchor_within(POSITION_GOAL_ARRIVAL_EPSILON * 0.5),
    )];
    graph.states.insert(
        "done".to_string(),
        authored_state("idle", MotionVerb::Hold, None, Vec::new()),
    );
    let mut registry = EntityRegistry::new();
    let mut runtime = AiRuntime::new();
    let enemy = spawn_enemy(
        &mut registry,
        Vec3::ZERO,
        authored_brain(&graph, "position"),
        50.0,
    );
    set_enemy_position(
        &mut registry,
        enemy,
        Vec3::new(POSITION_GOAL_ARRIVAL_EPSILON, 0.0, 0.0),
    );

    run_ai_tick(&mut registry, &mut runtime, 0.016);

    assert_eq!(enemy_state_name(&registry, enemy), "position");
    assert_eq!(enemy_destination(&registry, enemy), None);
}
