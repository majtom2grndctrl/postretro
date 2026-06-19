// Unit tests for the engine-owned enemy FSM tick. The pure transition core is
// driven directly (no registry); the integration tests build a minimal registry
// with a player pawn (PlayerMovement + Transform + Health), an enemy (Brain +
// Transform + Agent + Mesh), and assert observable outcomes — destination via
// `path_state`, HP deltas via the chokepoint, the selected animation name, and
// the stride gating.

use std::collections::HashSet;

use glam::Vec3;

use super::*;
use crate::agent_steering;
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

fn enemy_animation(reg: &EntityRegistry, enemy: EntityId) -> String {
    reg.get_component::<MeshComponent>(enemy)
        .unwrap()
        .animation
        .as_ref()
        .unwrap()
        .current_state
        .clone()
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
