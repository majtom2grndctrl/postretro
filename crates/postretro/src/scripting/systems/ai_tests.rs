// Unit tests for the engine-owned enemy FSM tick. The pure transition core is
// driven directly (no registry); the integration tests build a minimal registry
// with a player pawn (PlayerMovement + Transform + Health), an enemy (Brain +
// Transform + Agent + Mesh), and assert observable outcomes — destination via
// `path_state`, HP deltas via the chokepoint, the selected animation name, and
// the stride gating.

use std::collections::HashSet;

use glam::Vec3;
use parry3d::math::{Isometry, Point};
use parry3d::shape::TriMesh;
use postretro_level_format::navmesh::{NAVMESH_VERSION, NavMeshSection, NavRegion};

use super::*;
use crate::agent_steering;
use crate::collision::CollisionWorld;
use crate::nav::NavGraph;
use crate::scripting::components::agent::AgentComponent;
use crate::scripting::components::brain::{AiStateMap, AiTuning, BrainComponent, LogicalState};
use crate::scripting::components::health::HealthComponent;
use crate::scripting::components::mesh::{
    AnimationState, InterruptPolicy, MeshAnimation, MeshComponent,
};
use crate::scripting::components::player_movement::PlayerMovementComponent;
use crate::scripting::data_descriptors::{
    AirParams, CapsuleParams, FallParams, ForgivenessParams, GroundParams,
    PlayerMovementDescriptor, SpeedParams,
};
use crate::scripting::registry::{EntityId, EntityRegistry, Transform};

// ---------------------------------------------------------------------------
// Fixtures
// ---------------------------------------------------------------------------

/// Resolved tuning with legible ranges: detect at 18, attack at 2, leash at 26,
/// 8 damage on a 1000ms cooldown. Animation names mirror the four logical
/// states: idle→idle, alert→locomotion, attack→attack, death→death.
fn tuning() -> AiTuning {
    AiTuning {
        detection_range: 18.0,
        attack_range: 2.0,
        leash_range: 26.0,
        attack_damage: 8.0,
        attack_cooldown_ms: 1000.0,
        move_speed: 3.5,
        death_despawn_ms: 1500.0,
        states: AiStateMap {
            idle: "idle".into(),
            alert: "locomotion".into(),
            attack: "attack".into(),
            death: "death".into(),
        },
    }
}

fn brain_with(tuning: AiTuning, state: LogicalState) -> BrainComponent {
    BrainComponent {
        state,
        attack_cooldown_remaining_ms: 0.0,
        think_stride_counter: 0,
        death_despawn_remaining_ms: None,
        tuning,
    }
}

/// A usable (clip-resolved) animation state so `switch_animation_state` accepts
/// switches in the integration tests.
fn usable_state(clip: &str, idx: usize) -> AnimationState {
    AnimationState {
        clip: clip.into(),
        looping: true,
        crossfade_ms: 0.0,
        interrupt: InterruptPolicy::Smooth,
        clip_index: Some(idx),
    }
}

/// A four-state mesh declaring the tuning's animation names, all resolved.
fn enemy_mesh() -> MeshComponent {
    let mut states = std::collections::HashMap::new();
    states.insert("idle".to_string(), usable_state("idle_clip", 0));
    states.insert("locomotion".to_string(), usable_state("walk_clip", 1));
    states.insert("attack".to_string(), usable_state("attack_clip", 2));
    states.insert("death".to_string(), usable_state("death_clip", 3));
    MeshComponent {
        model: "grunt".into(),
        animation: Some(MeshAnimation::new(states, "idle".into())),
        origin_offset: Vec3::ZERO,
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
            zone_multipliers: std::collections::HashMap::new(),
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
    reg.set_component(id, brain).unwrap();
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
            zone_multipliers: std::collections::HashMap::new(),
        },
    )
    .unwrap();
    id
}

fn player_hp(reg: &EntityRegistry, pawn: EntityId) -> f32 {
    reg.get_component::<HealthComponent>(pawn).unwrap().current
}

fn enemy_state(reg: &EntityRegistry, enemy: EntityId) -> LogicalState {
    reg.get_component::<BrainComponent>(enemy).unwrap().state
}

/// The brain's death-despawn countdown — `None` until the brain enters `Death`,
/// reset back to `None` when it recovers.
fn enemy_despawn_remaining(reg: &EntityRegistry, enemy: EntityId) -> Option<f32> {
    reg.get_component::<BrainComponent>(enemy)
        .unwrap()
        .death_despawn_remaining_ms
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

/// Force the enemy agent's live velocity (what `path_state` reports), so a facing
/// test can stage a "moving" agent without running the steering tick.
fn set_agent_velocity(reg: &mut EntityRegistry, enemy: EntityId, velocity: Vec3) {
    let mut agent = reg.get_component::<AgentComponent>(enemy).unwrap().clone();
    agent.velocity = velocity;
    reg.set_component(enemy, agent).unwrap();
}

// ---------------------------------------------------------------------------
// Pure transition core
// ---------------------------------------------------------------------------

#[test]
fn idle_transitions_to_alert_when_player_enters_detection_range() {
    let t = tuning();
    // Player 10 units away (inside detection 18, outside attack 2): alert+chase.
    let result = evaluate_transition(
        Vec3::new(10.0, 0.0, 0.0),
        Vec3::ZERO,
        &t,
        LogicalState::Idle,
        true,
    );
    assert_eq!(result.next_state, LogicalState::Alert);
    assert_eq!(result.steering, SteeringIntent::Chase);
}

#[test]
fn idle_stays_idle_and_clears_when_player_outside_detection_range() {
    let t = tuning();
    let result = evaluate_transition(
        Vec3::new(50.0, 0.0, 0.0),
        Vec3::ZERO,
        &t,
        LogicalState::Idle,
        true,
    );
    assert_eq!(result.next_state, LogicalState::Idle);
    assert_eq!(result.steering, SteeringIntent::Clear);
}

#[test]
fn alert_transitions_to_idle_when_player_leaves_leash_range() {
    let t = tuning();
    // Player 30 units away (outside leash 26): drop target → idle + clear.
    let result = evaluate_transition(
        Vec3::new(30.0, 0.0, 0.0),
        Vec3::ZERO,
        &t,
        LogicalState::Alert,
        true,
    );
    assert_eq!(result.next_state, LogicalState::Idle);
    assert_eq!(result.steering, SteeringIntent::Clear);
}

#[test]
fn alert_transitions_to_attack_within_attack_range() {
    let t = tuning();
    let result = evaluate_transition(
        Vec3::new(1.0, 0.0, 0.0),
        Vec3::ZERO,
        &t,
        LogicalState::Alert,
        true,
    );
    assert_eq!(result.next_state, LogicalState::Attack);
    assert_eq!(result.steering, SteeringIntent::Chase);
}

#[test]
fn attack_falls_back_to_alert_when_leaving_attack_range() {
    let t = tuning();
    let result = evaluate_transition(
        Vec3::new(5.0, 0.0, 0.0),
        Vec3::ZERO,
        &t,
        LogicalState::Attack,
        true,
    );
    assert_eq!(result.next_state, LogicalState::Alert);
    assert_eq!(result.steering, SteeringIntent::Chase);
}

#[test]
fn alert_keeps_chasing_when_acquisition_gated_off_and_still_engaged() {
    // Inside leash but acquisition NOT evaluated this tick: must not drop the
    // target — it keeps chasing.
    let t = tuning();
    let result = evaluate_transition(
        Vec3::new(10.0, 0.0, 0.0),
        Vec3::ZERO,
        &t,
        LogicalState::Alert,
        false,
    );
    assert_eq!(result.next_state, LogicalState::Alert);
    assert_eq!(result.steering, SteeringIntent::Chase);
}

#[test]
fn attack_range_entry_is_evaluated_even_when_acquisition_gated_off() {
    // The strided-gap-must-not-suppress-attack contract at the pure level:
    // acquisition off, but the player is inside attack range — still attacks.
    let t = tuning();
    let result = evaluate_transition(
        Vec3::new(1.0, 0.0, 0.0),
        Vec3::ZERO,
        &t,
        LogicalState::Alert,
        false,
    );
    assert_eq!(result.next_state, LogicalState::Attack);
}

// ---------------------------------------------------------------------------
// Acceptance: detection sets destination, leash clears it (via path_state)
// ---------------------------------------------------------------------------

#[test]
fn detection_sets_agent_destination_and_leash_clears_it() {
    let mut reg = EntityRegistry::new();
    let mut warned = HashSet::new();

    // A short leash (8) so "beyond leash" still falls in the near stride band
    // (<= 12) — the leash drop is then evaluated every tick, isolating this test
    // from the think-stride gating (covered by its own test).
    let mut t = tuning();
    t.detection_range = 18.0;
    t.leash_range = 8.0;
    let enemy = spawn_enemy(
        &mut reg,
        Vec3::ZERO,
        brain_with(t, LogicalState::Idle),
        50.0,
    );

    // Player crosses into detection range (5 units away): the tick must set the
    // agent destination to the player. Assert via the path_state read.
    let pawn = spawn_player(&mut reg, Vec3::new(5.0, 0.0, 0.0));
    run_ai_tick(&mut reg, &mut warned, 0.016);
    assert_eq!(enemy_state(&reg, enemy), LogicalState::Alert);
    assert!(
        agent_steering::path_state(&reg, enemy)
            .expect("agent present")
            .has_destination,
        "detection must set a destination",
    );

    // Player leaves leash range (10 units > leash 8, still near band): the tick
    // must clear the destination.
    let mut t = *reg.get_component::<Transform>(pawn).unwrap();
    t.position = Vec3::new(10.0, 0.0, 0.0);
    reg.set_component(pawn, t).unwrap();
    run_ai_tick(&mut reg, &mut warned, 0.016);
    assert_eq!(enemy_state(&reg, enemy), LogicalState::Idle);
    assert!(
        !agent_steering::path_state(&reg, enemy)
            .expect("agent present")
            .has_destination,
        "leaving leash must clear the destination",
    );
}

// ---------------------------------------------------------------------------
// Acceptance: damage exactly once per cooldown via the chokepoint
// ---------------------------------------------------------------------------

#[test]
fn attack_applies_configured_damage_once_per_cooldown() {
    let mut reg = EntityRegistry::new();
    let mut warned = HashSet::new();

    // Player inside attack range (1 unit). Enemy idle → detection puts it in
    // attack this tick (already in attack range), cooldown ready → one hit.
    let pawn = spawn_player(&mut reg, Vec3::new(1.0, 0.0, 0.0));
    let _enemy = spawn_enemy(
        &mut reg,
        Vec3::ZERO,
        brain_with(tuning(), LogicalState::Idle),
        50.0,
    );

    // dt = 0.1s = 100ms; cooldown is 1000ms → ~10 ticks between hits.
    let dt = 0.1;

    // Tick 1: attacks once (8 damage), arms cooldown to 1000ms.
    let events = run_ai_tick(&mut reg, &mut warned, dt);
    assert_eq!(events, vec![ENEMY_ATTACK_EVENT]);
    assert_eq!(player_hp(&reg, pawn), 92.0, "one hit lands");

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
}

#[test]
fn attack_does_not_damage_remote_health_when_marked_local_pawn_lacks_health() {
    let mut reg = EntityRegistry::new();
    let mut warned = HashSet::new();

    let remote = spawn_player(&mut reg, Vec3::new(100.0, 0.0, 0.0));
    let local = spawn_player_without_health(&mut reg, Vec3::new(1.0, 0.0, 0.0));
    reg.mark_local_player_pawn(local).unwrap();
    spawn_enemy(
        &mut reg,
        Vec3::ZERO,
        brain_with(tuning(), LogicalState::Attack),
        50.0,
    );

    let events = run_ai_tick(&mut reg, &mut warned, 1.0 / 60.0);

    assert!(
        events.is_empty(),
        "enemy should target the marked pawn's position but not attack a different pawn's health"
    );
    assert_eq!(
        player_hp(&reg, remote),
        100.0,
        "remote pawn health must not be used as fallback damage target"
    );
}

#[test]
fn no_damage_when_player_below_attack_range() {
    let mut reg = EntityRegistry::new();
    let mut warned = HashSet::new();

    // Player at 10 units: inside detection, outside attack range → no damage.
    let pawn = spawn_player(&mut reg, Vec3::new(10.0, 0.0, 0.0));
    spawn_enemy(
        &mut reg,
        Vec3::ZERO,
        brain_with(tuning(), LogicalState::Idle),
        50.0,
    );

    let events = run_ai_tick(&mut reg, &mut warned, 0.1);
    assert!(events.is_empty(), "no attack event out of range");
    assert_eq!(player_hp(&reg, pawn), 100.0, "no damage out of range");
}

#[test]
fn no_attack_or_event_when_player_already_dead() {
    let mut reg = EntityRegistry::new();
    let mut warned = HashSet::new();

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
            zone_multipliers: std::collections::HashMap::new(),
        },
    )
    .unwrap();
    spawn_enemy(
        &mut reg,
        Vec3::ZERO,
        brain_with(tuning(), LogicalState::Idle),
        50.0,
    );

    for _ in 0..5 {
        let events = run_ai_tick(&mut reg, &mut warned, 0.1);
        assert!(events.is_empty(), "no attack event against a dead player");
        assert_eq!(player_hp(&reg, pawn), 0.0, "a dead player takes no damage");
    }
}

// ---------------------------------------------------------------------------
// Acceptance: each logical state selects the mapped animation name
// ---------------------------------------------------------------------------

#[test]
fn each_logical_state_switches_to_mapped_animation() {
    let mut reg = EntityRegistry::new();
    let mut warned = HashSet::new();

    let enemy = spawn_enemy(
        &mut reg,
        Vec3::ZERO,
        brain_with(tuning(), LogicalState::Idle),
        50.0,
    );
    let pawn = spawn_player(&mut reg, Vec3::new(10.0, 0.0, 0.0));

    // idle starts as the mesh default; entering ALERT selects "locomotion".
    run_ai_tick(&mut reg, &mut warned, 0.016);
    assert_eq!(enemy_state(&reg, enemy), LogicalState::Alert);
    assert_eq!(enemy_animation(&reg, enemy), "locomotion");

    // Move the player into attack range → ATTACK selects "attack".
    let mut t = *reg.get_component::<Transform>(pawn).unwrap();
    t.position = Vec3::new(1.0, 0.0, 0.0);
    reg.set_component(pawn, t).unwrap();
    run_ai_tick(&mut reg, &mut warned, 0.016);
    assert_eq!(enemy_state(&reg, enemy), LogicalState::Attack);
    assert_eq!(enemy_animation(&reg, enemy), "attack");

    // Player leaves to beyond leash. From ATTACK the FSM steps back through
    // ALERT (leaving attack range) and then to IDLE (leaving leash on the next
    // think tick). First tick: ATTACK → ALERT ("locomotion").
    let mut t = *reg.get_component::<Transform>(pawn).unwrap();
    t.position = Vec3::new(30.0, 0.0, 0.0);
    reg.set_component(pawn, t).unwrap();
    run_ai_tick(&mut reg, &mut warned, 0.016);
    assert_eq!(enemy_state(&reg, enemy), LogicalState::Alert);
    assert_eq!(enemy_animation(&reg, enemy), "locomotion");

    // Second tick: ALERT → IDLE (player outside leash, acquisition evaluated)
    // selects "idle".
    run_ai_tick(&mut reg, &mut warned, 0.016);
    assert_eq!(enemy_state(&reg, enemy), LogicalState::Idle);
    assert_eq!(enemy_animation(&reg, enemy), "idle");

    // Zero HP → DEATH selects "death" (every tick, regardless of range).
    let mut h = reg.get_component::<HealthComponent>(enemy).unwrap().clone();
    h.current = 0.0;
    reg.set_component(enemy, h).unwrap();
    run_ai_tick(&mut reg, &mut warned, 0.016);
    assert_eq!(enemy_state(&reg, enemy), LogicalState::Death);
    assert_eq!(enemy_animation(&reg, enemy), "death");
}

#[test]
fn unmapped_animation_warns_once_and_keeps_prior_state() {
    // The enemy's tuning maps alert→"locomotion" but the mesh does NOT declare
    // it: the switch fails, the prior animation is kept, and the warn latch
    // records the name exactly once.
    let mut reg = EntityRegistry::new();
    let mut warned = HashSet::new();

    let mut t = tuning();
    t.states.alert = "missing_clip".into();
    let enemy = spawn_enemy(
        &mut reg,
        Vec3::ZERO,
        brain_with(t, LogicalState::Idle),
        50.0,
    );
    spawn_player(&mut reg, Vec3::new(10.0, 0.0, 0.0));

    run_ai_tick(&mut reg, &mut warned, 0.016);
    assert_eq!(
        enemy_state(&reg, enemy),
        LogicalState::Alert,
        "logical state still advances"
    );
    assert_eq!(
        enemy_animation(&reg, enemy),
        "idle",
        "failed switch keeps the prior animation",
    );
    assert!(
        warned.contains("anim:missing_clip"),
        "warn latch records the namespaced animation name",
    );
    assert_eq!(warned.len(), 1, "exactly one distinct name warned");
}

// ---------------------------------------------------------------------------
// Acceptance: stride gating does not suppress in-stride attack or death
// ---------------------------------------------------------------------------

#[test]
fn near_enemy_evaluates_detection_every_tick() {
    // A near enemy (within STRIDE_NEAR_DISTANCE) uses stride 1: detection is
    // evaluated on the very first tick after the player appears.
    let mut reg = EntityRegistry::new();
    let mut warned = HashSet::new();

    let enemy = spawn_enemy(
        &mut reg,
        Vec3::ZERO,
        brain_with(tuning(), LogicalState::Idle),
        50.0,
    );
    // 5 units: near band, inside detection.
    spawn_player(&mut reg, Vec3::new(5.0, 0.0, 0.0));

    run_ai_tick(&mut reg, &mut warned, 0.016);
    assert_eq!(
        enemy_state(&reg, enemy),
        LogicalState::Alert,
        "near enemy acquires on the first tick",
    );
}

#[test]
fn distant_enemy_strides_detection_but_attack_and_death_still_fire() {
    // A distant enemy (far band, stride 12) does NOT re-acquire detection every
    // tick. But the attack-in-range/cooldown and zero-HP death checks run every
    // tick regardless: even mid-stride-gap they must fire.

    // 1) Stride-gated detection: a far enemy in IDLE with the player far (but
    // inside detection) does NOT flip to alert on the first (non-think) tick.
    {
        let mut reg = EntityRegistry::new();
        let mut warned = HashSet::new();
        // Detection range wide enough to include a far-band player.
        let mut t = tuning();
        t.detection_range = 40.0;
        let enemy = spawn_enemy(
            &mut reg,
            Vec3::ZERO,
            brain_with(t, LogicalState::Idle),
            50.0,
        );
        // 35 units: far band (> STRIDE_MID_DISTANCE 30), inside detection 40.
        spawn_player(&mut reg, Vec3::new(35.0, 0.0, 0.0));

        // think_stride_counter starts at 0 → becomes 1 after the first tick;
        // 1 % 12 != 0 so acquisition is gated OFF this tick → stays idle.
        run_ai_tick(&mut reg, &mut warned, 0.016);
        assert_eq!(
            enemy_state(&reg, enemy),
            LogicalState::Idle,
            "far enemy's detection is strided: no acquire on a non-think tick",
        );
    }

    // 2) In-stride DEATH still fires: a far enemy at zero HP transitions to
    // death on a non-think tick (death is not strided).
    {
        let mut reg = EntityRegistry::new();
        let mut warned = HashSet::new();
        let enemy = spawn_enemy(
            &mut reg,
            Vec3::ZERO,
            brain_with(tuning(), LogicalState::Alert),
            0.0,
        );
        spawn_player(&mut reg, Vec3::new(35.0, 0.0, 0.0));
        run_ai_tick(&mut reg, &mut warned, 0.016);
        assert_eq!(
            enemy_state(&reg, enemy),
            LogicalState::Death,
            "zero-HP death fires every tick regardless of stride",
        );
    }

    // 3) In-stride ATTACK still fires: a far-positioned enemy ALREADY in attack
    // state with the player within attack range damages on a non-think tick
    // (attack-range + cooldown are not strided). Here the enemy sits at origin
    // and the player is at attack range, but we force the far stride by starting
    // the counter such that acquisition is gated; the attack check ignores that.
    {
        let mut reg = EntityRegistry::new();
        let mut warned = HashSet::new();
        let pawn = spawn_player(&mut reg, Vec3::new(1.0, 0.0, 0.0));
        // Enemy in Attack already, cooldown ready, player in attack range.
        let mut brain = brain_with(tuning(), LogicalState::Attack);
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
    let mut warned = HashSet::new();

    let enemy = spawn_enemy(
        &mut reg,
        Vec3::ZERO,
        brain_with(tuning(), LogicalState::Alert),
        50.0,
    );
    // Pre-seed a destination so we can observe it being cleared.
    agent_steering::set_destination(&mut reg, enemy, Vec3::new(5.0, 0.0, 0.0));

    let events = run_ai_tick(&mut reg, &mut warned, 0.016);
    assert!(events.is_empty());
    assert_eq!(enemy_state(&reg, enemy), LogicalState::Idle);
    assert!(
        !agent_steering::path_state(&reg, enemy)
            .expect("agent present")
            .has_destination,
        "no pawn clears any stale destination",
    );
}

// ---------------------------------------------------------------------------
// Acceptance: brain death despawn — the AI tick owns the despawn, timer is
// authoritative, and despawn happens after `death_despawn_ms`.
// ---------------------------------------------------------------------------

#[test]
fn dead_enemy_is_despawned_after_death_despawn_ms_timer_authoritative() {
    // A killed enemy enters Death, plays its death clip, and is despawned by the
    // AI tick only AFTER `death_despawn_ms` elapses: alive just before the timer,
    // gone just after. `death_despawn_ms = 1500` (from `tuning()`), `dt = 0.5s`
    // (500ms): seed on tick 1, 1500→1000 on tick 2, →500 on tick 3, →0 on tick 4
    // (despawn). The entity survives ticks 1–3 and is gone after tick 4.
    let mut reg = EntityRegistry::new();
    let mut warned = HashSet::new();
    let enemy = spawn_enemy(
        &mut reg,
        Vec3::ZERO,
        brain_with(tuning(), LogicalState::Alert),
        0.0, // dead at spawn
    );
    // No player needed; the zero-HP death check runs regardless.

    let dt = 0.5; // 500ms per tick

    // Tick 1: enters Death, seeds the countdown to 1500ms, plays death anim.
    run_ai_tick(&mut reg, &mut warned, dt);
    assert!(reg.exists(enemy), "alive on the Death-entry (seed) tick");
    assert_eq!(enemy_state(&reg, enemy), LogicalState::Death);
    assert_eq!(
        enemy_animation(&reg, enemy),
        "death",
        "the death animation is requested on entering Death",
    );

    // Ticks 2 and 3: countdown 1500→1000→500. Still alive.
    run_ai_tick(&mut reg, &mut warned, dt);
    assert!(
        reg.exists(enemy),
        "alive while the timer counts down (1000ms left)"
    );
    run_ai_tick(&mut reg, &mut warned, dt);
    assert!(
        reg.exists(enemy),
        "alive just before the timer (500ms left)"
    );

    // Tick 4: countdown 500→0 → despawn.
    run_ai_tick(&mut reg, &mut warned, dt);
    assert!(
        !reg.exists(enemy),
        "despawned by the AI tick after death_despawn_ms elapsed",
    );
}

#[test]
fn dead_enemy_despawns_on_timer_even_with_unresolved_death_clip() {
    // The TIMER is authoritative: an enemy whose death animation name is NOT
    // declared on its mesh (an unresolved death clip — `switch_animation_state`
    // returns UnknownState and plays nothing) is STILL despawned after
    // `death_despawn_ms`. Here the mesh is stateless (no animation block), so
    // every state name is unresolved.
    let mut reg = EntityRegistry::new();
    let mut warned = HashSet::new();
    let enemy = spawn_enemy(
        &mut reg,
        Vec3::ZERO,
        brain_with(tuning(), LogicalState::Alert),
        0.0,
    );
    // Replace the four-state mesh with a stateless one: the death clip can never
    // resolve, so animation playback is a no-op — but the timer still fires.
    reg.set_component(enemy, MeshComponent::stateless("grunt".into()))
        .unwrap();

    let dt = 0.5;
    // 1500ms / 500ms = seed + 3 decrements → despawn on the 4th tick.
    run_ai_tick(&mut reg, &mut warned, dt); // seed
    run_ai_tick(&mut reg, &mut warned, dt); // 1000
    run_ai_tick(&mut reg, &mut warned, dt); // 500
    assert!(reg.exists(enemy), "alive until the timer elapses");
    run_ai_tick(&mut reg, &mut warned, dt); // 0 → despawn
    assert!(
        !reg.exists(enemy),
        "the despawn timer is authoritative even with an unresolved death clip",
    );
}

#[test]
fn zero_death_despawn_ms_still_gives_one_death_tick_before_despawn() {
    // A zero (or negative) configured death-despawn delay is clamped to >= 0 so
    // the enemy still gets ONE Death tick (death animation requested) before the
    // despawn pass removes it: seeded to 0 on the entry tick (no despawn), then
    // despawned on the next tick when the decrement keeps it at 0.
    let mut reg = EntityRegistry::new();
    let mut warned = HashSet::new();
    let mut t = tuning();
    t.death_despawn_ms = 0.0;
    let enemy = spawn_enemy(
        &mut reg,
        Vec3::ZERO,
        brain_with(t, LogicalState::Alert),
        0.0,
    );

    // Tick 1: enters Death, seeds countdown to 0 — but the SEED tick never
    // despawns, so the entity survives this tick and plays the death anim.
    run_ai_tick(&mut reg, &mut warned, 0.016);
    assert!(
        reg.exists(enemy),
        "the entity gets one Death tick before despawn"
    );
    assert_eq!(enemy_state(&reg, enemy), LogicalState::Death);

    // Tick 2: the countdown is already 0; the decrement keeps it at 0 → despawn.
    run_ai_tick(&mut reg, &mut warned, 0.016);
    assert!(
        !reg.exists(enemy),
        "despawned on the tick after the single Death tick"
    );
}

// ---------------------------------------------------------------------------
// Acceptance: an enemy that recovers HP before the despawn timer elapses leaves
// the terminal `Death` state and re-engages (forward-looking heal/revive
// robustness — no heal path exists in the engine today). The recovery runs in
// the not-dead path before the player-presence split, so it fires WITH a player
// (re-acquiring) and WITHOUT one (resolving to Idle); the control confirms an
// un-healed enemy still despawns on the timer.
// ---------------------------------------------------------------------------

#[test]
fn healed_enemy_recovers_from_death_and_reacquires_player() {
    let mut reg = EntityRegistry::new();
    let mut warned = HashSet::new();

    // Player inside detection (10 units) but outside attack range (2): a
    // recovered enemy re-acquires to Alert, not Attack.
    let pawn = spawn_player(&mut reg, Vec3::new(10.0, 0.0, 0.0));
    let enemy = spawn_enemy(
        &mut reg,
        Vec3::ZERO,
        brain_with(tuning(), LogicalState::Alert),
        0.0, // dead at spawn
    );

    // Tick 1: zero HP → Death, despawn countdown seeded (death_despawn_ms 1500).
    run_ai_tick(&mut reg, &mut warned, 0.016);
    assert_eq!(enemy_state(&reg, enemy), LogicalState::Death);
    assert_eq!(
        enemy_despawn_remaining(&reg, enemy),
        Some(1500.0),
        "entering Death seeds the despawn countdown",
    );

    // Heal the enemy back above zero BEFORE the timer elapses.
    set_hp(&mut reg, enemy, 30.0);

    // Tick 2: recovers from Death and, with the player in detection range,
    // re-acquires to Alert this same tick (recovery resets to Idle, then the
    // normal transition runs).
    run_ai_tick(&mut reg, &mut warned, 0.016);
    assert_ne!(
        enemy_state(&reg, enemy),
        LogicalState::Death,
        "a healed enemy must leave the terminal Death state",
    );
    assert_eq!(
        enemy_state(&reg, enemy),
        LogicalState::Alert,
        "with a player in detection range the recovered enemy re-acquires",
    );
    assert_eq!(
        enemy_despawn_remaining(&reg, enemy),
        None,
        "recovery clears the despawn countdown",
    );
    assert!(reg.exists(enemy), "the recovered enemy is not despawned");

    // Sanity: the recovered enemy is the live chase target again.
    let _ = player_hp(&reg, pawn);
}

#[test]
fn healed_enemy_recovers_from_death_with_no_player() {
    // Recovery must run even with no player to target: the not-dead path resets
    // Death → Idle before the player-presence split, and the no-player `else`
    // branch then resolves the enemy to Idle (not stuck in Death).
    let mut reg = EntityRegistry::new();
    let mut warned = HashSet::new();

    let enemy = spawn_enemy(
        &mut reg,
        Vec3::ZERO,
        brain_with(tuning(), LogicalState::Alert),
        0.0, // dead at spawn
    );

    // Tick 1 (no player): zero HP → Death, countdown seeded.
    run_ai_tick(&mut reg, &mut warned, 0.016);
    assert_eq!(enemy_state(&reg, enemy), LogicalState::Death);
    assert_eq!(enemy_despawn_remaining(&reg, enemy), Some(1500.0));

    // Heal before the timer elapses.
    set_hp(&mut reg, enemy, 30.0);

    // Tick 2 (still no player): recovers to Idle, countdown cleared, alive.
    run_ai_tick(&mut reg, &mut warned, 0.016);
    assert_eq!(
        enemy_state(&reg, enemy),
        LogicalState::Idle,
        "with no player the recovered enemy resolves to Idle",
    );
    assert_eq!(
        enemy_despawn_remaining(&reg, enemy),
        None,
        "recovery clears the despawn countdown even with no player",
    );
    assert!(reg.exists(enemy), "the recovered enemy is not despawned");
}

#[test]
fn unhealed_dead_enemy_still_despawns_on_timer() {
    // Control: an enemy left at zero HP (never healed) still despawns when the
    // death-despawn timer elapses — the recovery path does not affect the
    // existing despawn behavior. dt = 0.5s, death_despawn_ms = 1500: seed on
    // tick 1, 1500→1000→500→0 (despawn) on tick 4.
    let mut reg = EntityRegistry::new();
    let mut warned = HashSet::new();
    let enemy = spawn_enemy(
        &mut reg,
        Vec3::ZERO,
        brain_with(tuning(), LogicalState::Alert),
        0.0,
    );

    let dt = 0.5;
    for _ in 0..3 {
        run_ai_tick(&mut reg, &mut warned, dt);
        assert!(
            reg.exists(enemy),
            "un-healed enemy alive while the timer runs"
        );
        assert_eq!(enemy_state(&reg, enemy), LogicalState::Death);
    }
    run_ai_tick(&mut reg, &mut warned, dt);
    assert!(
        !reg.exists(enemy),
        "an un-healed enemy still despawns on the timer as before",
    );
}

// ---------------------------------------------------------------------------
// Regression: the integrated FSM-steering loop (run_ai_tick + agent_steering
// ::tick, mirroring main.rs's run_agent_tick) must not freeze chasers beyond
// the replan budget, and a stationary target must not force a replan per tick.
//
// Bug: `set_destination` wiped the path on every call. The FSM re-issues the
// player's position every chase tick, so with more than REPLAN_BUDGET_PER_TICK
// chasers, the overflow chasers ended each tick with an empty path → goal_velocity
// ZERO → permanent freeze. Fix: `set_destination` only records the target; the
// path is (re)built solely inside `tick`'s budget-gated replan block, so a
// budget-deferred agent keeps its stale-but-valid path and keeps moving.
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

/// Spawn a grounded enemy already in `Alert` (chasing) at world `(x, _, z)`. The
/// agent capsule matches the navmesh's baked agent so it routes cleanly.
fn spawn_chaser(reg: &mut EntityRegistry, x: f32, z: f32) -> EntityId {
    let pos = Vec3::new(x, chaser_rest_y(), z);
    spawn_enemy(reg, pos, brain_with(tuning(), LogicalState::Alert), 50.0)
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
    let mut warned = HashSet::new();

    // A (near-)stationary player at one end of the floor, inside detection range
    // of the cluster so all chasers stay in Alert and pursue.
    let player = spawn_player(&mut reg, Vec3::new(20.0, 0.0, 8.0));

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

    // Run the integrated loop for many ticks, mirroring main.rs's run_agent_tick:
    // FSM tick (issues set_destination to the player) then the steering tick.
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
        let player_xz = Vec3::new(20.0, 0.0, 8.0);
        assert!(
            distance_xz(state.position, player_xz) < distance_xz(start, player_xz),
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
    // Regression (the bug the stationary-player test missed): the FSM re-issues
    // the player's position to `set_destination` EVERY chase tick. The old
    // `set_destination` wiped each chaser's path on every call; the per-tick
    // replan budget then only replanned REPLAN_BUDGET_PER_TICK of them, so the
    // OVERFLOW chasers ended every tick with an empty path → goal_velocity ZERO →
    // permanent freeze. The fix preserves the path and lets a budget-loss chaser
    // keep following its stale-but-valid route. This test spawns MORE chasers than
    // the budget and a player that moves ~0.12 u/tick (a real per-tick step), and
    // asserts EVERY chaser — overflow included — keeps moving (the load-bearing
    // `moved > 1.0` check). It FAILS pre-fix: overflow chasers freeze (~0.27 u).
    let floor = OpenFloor::new();
    let world = floor.collision_world();
    let graph = floor.nav_graph();

    let mut reg = EntityRegistry::new();
    let mut warned = HashSet::new();

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
// face the player when stopped-but-engaged, leave Idle facing untouched, and
// never write a NaN yaw from a zero-length direction.
// ---------------------------------------------------------------------------

/// Assert two normalized XZ directions point the same way (dot ≈ 1).
fn assert_faces(actual: Vec3, expected: Vec3, ctx: &str) {
    let dot = actual.normalize().dot(expected.normalize());
    assert!(
        dot > 0.999,
        "{ctx}: expected facing {expected:?}, got {actual:?} (dot {dot})"
    );
}

#[test]
fn stopped_engaged_enemy_faces_the_player() {
    // An enemy in attack range (so it reaches `Attack`) with near-zero velocity
    // must face the player, not its spawn heading. Player off to +X.
    let mut reg = EntityRegistry::new();
    let mut warned = HashSet::new();

    let player = spawn_player(&mut reg, Vec3::new(1.5, 0.0, 0.0));
    let enemy = spawn_enemy(
        &mut reg,
        Vec3::ZERO,
        brain_with(tuning(), LogicalState::Idle),
        50.0,
    );
    // Stopped (arrived/swinging): zero velocity.
    set_agent_velocity(&mut reg, enemy, Vec3::ZERO);

    run_ai_tick(&mut reg, &mut warned, 0.016);
    assert_eq!(enemy_state(&reg, enemy), LogicalState::Attack);

    // Player is at +X from the enemy → the enemy faces +X.
    let to_player = reg.get_component::<Transform>(player).unwrap().position - Vec3::ZERO;
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
    let mut warned = HashSet::new();

    // Player off to +X within attack range so the enemy reaches `Attack`.
    spawn_player(&mut reg, Vec3::new(1.5, 0.0, 0.0));
    let enemy = spawn_enemy(
        &mut reg,
        Vec3::ZERO,
        brain_with(tuning(), LogicalState::Idle),
        50.0,
    );
    set_agent_velocity(&mut reg, enemy, Vec3::ZERO);

    run_ai_tick(&mut reg, &mut warned, 0.016);
    assert_eq!(enemy_state(&reg, enemy), LogicalState::Attack);

    // The model's authored front (`+Z` rotated by the stored quaternion) points at
    // the player (+X): dot ≈ +1.
    let to_player = Vec3::new(1.0, 0.0, 0.0);
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
    let mut warned = HashSet::new();

    // Player inside detection (so the enemy is engaged/Alert) along +X.
    spawn_player(&mut reg, Vec3::new(10.0, 0.0, 0.0));
    let enemy = spawn_enemy(
        &mut reg,
        Vec3::ZERO,
        brain_with(tuning(), LogicalState::Alert),
        50.0,
    );
    // Velocity points toward +Z (routing around an obstacle), NOT toward the
    // player at +X — the facing must follow the velocity.
    let vel = Vec3::new(0.0, 0.0, 4.0);
    set_agent_velocity(&mut reg, enemy, vel);

    run_ai_tick(&mut reg, &mut warned, 0.016);
    assert_faces(
        enemy_forward_xz(&reg, enemy),
        vel,
        "moving enemy faces its velocity, not the player bee-line",
    );
}

#[test]
fn idle_enemy_facing_is_left_unchanged() {
    // An Idle enemy (no target) must not have its facing written: the spawn
    // rotation is preserved.
    let mut reg = EntityRegistry::new();
    let mut warned = HashSet::new();

    // No player in detection range → stays Idle.
    spawn_player(&mut reg, Vec3::new(100.0, 0.0, 0.0));
    let enemy = spawn_enemy(
        &mut reg,
        Vec3::ZERO,
        brain_with(tuning(), LogicalState::Idle),
        50.0,
    );
    // Give it a distinctive non-identity spawn rotation, and a velocity that
    // WOULD turn it if Idle facing were (incorrectly) written.
    let spawn_rot = glam::Quat::from_rotation_y(1.2);
    let mut t = *reg.get_component::<Transform>(enemy).unwrap();
    t.rotation = spawn_rot;
    reg.set_component(enemy, t).unwrap();
    set_agent_velocity(&mut reg, enemy, Vec3::new(3.0, 0.0, 0.0));

    run_ai_tick(&mut reg, &mut warned, 0.016);
    assert_eq!(enemy_state(&reg, enemy), LogicalState::Idle);
    let rot_after = reg.get_component::<Transform>(enemy).unwrap().rotation;
    assert!(
        rot_after.angle_between(spawn_rot) < 1e-5,
        "an Idle enemy's facing must be left unchanged (was {spawn_rot:?}, now {rot_after:?})",
    );
}

#[test]
fn stopped_engaged_enemy_on_top_of_player_writes_no_nan_facing() {
    // Degenerate: a stopped engaged enemy at the SAME XZ as the player → the
    // to-player direction is zero-length. The facing guard must skip the write,
    // leaving the prior rotation finite (no NaN quaternion).
    let mut reg = EntityRegistry::new();
    let mut warned = HashSet::new();

    // Player co-located (within attack range, distance 0) → enemy reaches Attack.
    spawn_player(&mut reg, Vec3::ZERO);
    let enemy = spawn_enemy(
        &mut reg,
        Vec3::ZERO,
        brain_with(tuning(), LogicalState::Idle),
        50.0,
    );
    set_agent_velocity(&mut reg, enemy, Vec3::ZERO);

    run_ai_tick(&mut reg, &mut warned, 0.016);
    assert_eq!(enemy_state(&reg, enemy), LogicalState::Attack);
    let rot = reg.get_component::<Transform>(enemy).unwrap().rotation;
    assert!(
        rot.x.is_finite() && rot.y.is_finite() && rot.z.is_finite() && rot.w.is_finite(),
        "zero-length facing direction must not write a NaN rotation (got {rot:?})",
    );
}

// ---------------------------------------------------------------------------
// Attack replay: the one-shot attack clip re-fires each in-state swing. The
// entry tick into `Attack` plays the clip via the `state_changed` switch; every
// later in-state swing restarts the clip from frame 0. Damage cadence is
// unchanged (cooldown-gated, not frame-synced).
// ---------------------------------------------------------------------------

#[test]
fn repeated_in_attack_swing_restarts_the_attack_clip() {
    // Drive the enemy into `Attack`, let the cooldown elapse, and confirm the
    // SECOND swing (an in-state swing, not the entry tick) restarts the attack
    // clip — observed as the entry stamp going pending again after a resolve had
    // filled it. The entry tick must NOT double-restart.
    let mut reg = EntityRegistry::new();
    let mut warned = HashSet::new();

    let pawn = spawn_player(&mut reg, Vec3::new(1.0, 0.0, 0.0));
    let enemy = spawn_enemy(
        &mut reg,
        Vec3::ZERO,
        brain_with(tuning(), LogicalState::Idle),
        50.0,
    );

    let dt = 0.1; // 100ms/tick; cooldown 1000ms → 10 ticks between swings.

    // Tick 1: Idle→Attack (state change), first swing via the `state_changed`
    // switch. The switch leaves the new `attack` entry stamp pending.
    let events = run_ai_tick(&mut reg, &mut warned, dt);
    assert_eq!(events, vec![ENEMY_ATTACK_EVENT], "first swing lands");
    assert_eq!(enemy_state(&reg, enemy), LogicalState::Attack);
    assert_eq!(enemy_animation(&reg, enemy), "attack");
    assert!(
        enemy_anim_entered_at(&reg, enemy).is_none(),
        "entry switch leaves the attack clip stamp pending (frame 0)",
    );

    // Resolve the animation stamps (the per-frame resolve pass) so the attack
    // clip's entry stamp is filled — steady state, clip playing.
    crate::scripting::components::mesh::resolve_pending_animation_stamps(&mut reg, 5.0);
    assert_eq!(
        enemy_anim_entered_at(&reg, enemy),
        Some(5.0),
        "resolve pass fills the attack clip's entry stamp",
    );

    // Ticks 2..=10: still in `Attack`, cooldown not elapsed → NO swing, so NO
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
    // the attack clip. The enemy stays in `Attack` (no state change), so this is
    // the restart path, not the entry switch.
    let events = run_ai_tick(&mut reg, &mut warned, dt);
    assert_eq!(events, vec![ENEMY_ATTACK_EVENT], "second swing lands");
    assert_eq!(
        enemy_state(&reg, enemy),
        LogicalState::Attack,
        "still in Attack — this is an in-state swing, not a re-entry",
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
    // The entry tick into `Attack` plays the clip via the `state_changed` switch
    // ONLY — the restart path is guarded off on that tick. Observed: after the
    // entry tick the fade bookkeeping reflects a single switch (no
    // `previous_state` from a redundant restart-over-switch), and a subsequent
    // resolve cleanly fills one entry stamp.
    let mut reg = EntityRegistry::new();
    let mut warned = HashSet::new();

    // Player inside attack range so Idle→Attack happens on tick 1 WITH a swing.
    spawn_player(&mut reg, Vec3::new(1.0, 0.0, 0.0));
    let enemy = spawn_enemy(
        &mut reg,
        Vec3::ZERO,
        brain_with(tuning(), LogicalState::Idle),
        50.0,
    );

    let events = run_ai_tick(&mut reg, &mut warned, 0.1);
    assert_eq!(events, vec![ENEMY_ATTACK_EVENT]);
    assert_eq!(enemy_state(&reg, enemy), LogicalState::Attack);

    let anim = reg
        .get_component::<MeshComponent>(enemy)
        .unwrap()
        .animation
        .as_ref()
        .unwrap()
        .clone();
    // Idle→Attack is a hard cut here (test clips use crossfade 0), so the switch
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
