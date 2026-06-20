// Engine-owned enemy FSM tick: the system half of the brain (the per-instance
// DATA + spawn-time state-map validation live in `components/brain.rs`). Each
// think tick locates the player pawn, evaluates the closed transition set
// (idle/alert/attack/death), drives the steering API toward the player while
// chasing, applies damage on the attack cooldown, and requests the mapped
// animation state. The transition CORE is a pure function over (player position,
// agent position, tuning, current state) so it is unit-testable without `App` or
// a GPU; the tick wrapper layers the registry reads/writes, the zero-HP death
// check, damage, and animation switching on top.
//
// Architectural decision (M10): an engine-owned Rust FSM with a closed
// transition set; tuning is declarative; there is no live VM at tick. Scripts
// declare thresholds and the logical→animation map; Rust executes.
//
// See: context/lib/entity_model.md §2 (engine components), §5 (fixed-tick game
//      logic), §7 (collision)
//      context/lib/scripting.md §1 (scripts declare, Rust executes),
//      §10.5 (the damage chokepoint — all damage routes through `apply_damage`)
//      crates/postretro/src/scripting/components/brain.rs (BrainComponent /
//      LogicalState / AiTuning — the FSM data this tick drives)
//      crates/postretro/src/agent_steering.rs (set_destination /
//      clear_destination / path_state — the steering surface this tick drives)

use std::collections::HashSet;

use glam::{Quat, Vec3};

use crate::agent_steering;
use crate::nav::distance_xz;
use crate::scripting::components::brain::{AiTuning, BrainComponent, LogicalState};
use crate::scripting::components::health::{HealthComponent, apply_damage, pawn_with_health};
use crate::scripting::components::mesh::{
    SwitchResult, restart_animation_clip, switch_animation_state,
};
use crate::scripting::registry::{
    ComponentKind, ComponentValue, EntityId, EntityRegistry, Transform,
};
use crate::weapon::DamagePayload;

/// Event name fired once per enemy attack that lands this tick. Mirrors the
/// weapon-fire event precedent (`"activate"`/`"impact"`): the tick returns the
/// names it raised and the app drains them through `fire_named_event` after the
/// tick loop settles.
pub(crate) const ENEMY_ATTACK_EVENT: &str = "enemyAttack";

/// Think-stride bands. Target acquisition (detection/leash) is time-sliced by
/// player distance: near enemies re-evaluate every tick, mid enemies every few
/// ticks, distant enemies rarely. The attack-in-range/cooldown check and the
/// zero-HP death check are NOT strided — they run every tick regardless, so a
/// strided acquisition gap can never suppress an in-stride attack or death.
///
/// Distances are XZ ground distances (the navmesh plane); the bands are coarse
/// by design — stride is a cost knob, not a gameplay contract.
const STRIDE_NEAR_DISTANCE: f32 = 12.0;
const STRIDE_MID_DISTANCE: f32 = 30.0;
/// Stride divisor for each band: `1` = every tick, `n` = once every `n` ticks.
const STRIDE_NEAR: u32 = 1;
const STRIDE_MID: u32 = 4;
const STRIDE_FAR: u32 = 12;

/// The think stride (in ticks) for an enemy at `distance` (XZ) from the player:
/// `1` near, larger as the player recedes. Pure helper so the stride policy is
/// testable in isolation.
pub(crate) fn think_stride_for_distance(distance: f32) -> u32 {
    if distance <= STRIDE_NEAR_DISTANCE {
        STRIDE_NEAR
    } else if distance <= STRIDE_MID_DISTANCE {
        STRIDE_MID
    } else {
        STRIDE_FAR
    }
}

/// Minimum XZ speed (units/sec) the agent must exceed for "moving" facing: above
/// it the enemy orients to its velocity (where it is going), at or below it the
/// enemy is treated as stopped and orients to the player instead. A small epsilon
/// so a near-stationary agent (arrived/blocked/swinging) faces the player rather
/// than jittering toward steering noise.
const FACING_MOVE_SPEED_EPSILON: f32 = 0.05;

/// The reference enemy mesh's VISUAL forward axis in model space. The skinned
/// glTF characters (`content/dev/models/reference_enemy_kaykit_knight`) are
/// authored facing `+Z` in model space — the KayKit/glTF/Blender convention, and
/// confirmed by this rig: the knee/toe IK control bones sit in front of the body
/// at `+Z` (`kneeIK` ≈ `+0.576`, `control-toe-roll` ≈ `+0.246`). The renderer
/// applies `Transform.rotation` straight to the model matrix with no import-time
/// axis flip (`mesh_render.rs`, `Mat4::from_scale_rotation_translation`), so a
/// rotation that aims the model's `+Z` at the target makes its FACE meet the
/// target.
///
/// Note this is the OPPOSITE of the engine's camera/view forward, which is `-Z`
/// (`camera.rs`: `forward(yaw) = (-sin yaw, 0, -cos yaw)`). Facing code orients a
/// rendered MESH, so it must aim the mesh's authored front (`+Z`), not the view
/// forward — aiming the view forward at the target would leave the model's back
/// to it (a clean 180° error).
const MESH_FORWARD: Vec3 = Vec3::Z;

/// A yaw-only rotation that aims the model's visual forward ([`MESH_FORWARD`],
/// `+Z`) at a horizontal direction. `Quat::from_rotation_y(yaw) * (+Z)` is
/// `(sin yaw, 0, cos yaw)`; solving `that == dir_xz` gives `yaw = atan2(dx, dz)`,
/// so the rotation turns the model's authored FRONT to face `dir`.
///
/// Returns `None` for a direction with negligible XZ length (the squared XZ
/// magnitude is at or below `MIN_XZ_LEN_SQ`), so a zero-length steering/aim vector
/// never produces a NaN yaw — the caller then leaves the existing facing
/// untouched. The Y component is ignored: facing is yaw-only, keeping the model
/// upright.
fn yaw_rotation_toward(dir: Vec3) -> Option<Quat> {
    // Squared XZ length guard: below this the direction is too short to derive a
    // stable heading (and `atan2(0, 0)` would be meaningless), so report "no
    // facing change".
    const MIN_XZ_LEN_SQ: f32 = 1e-8;
    if dir.x * dir.x + dir.z * dir.z <= MIN_XZ_LEN_SQ {
        return None;
    }
    // Aim MESH_FORWARD at `dir` in the XZ plane: the yaw that rotates the model's
    // authored forward heading onto the target heading. `Quat::from_rotation_y`
    // measures yaw from `+Z` (its heading is `atan2(x, z)`), so subtract the
    // model-forward's own heading — for `MESH_FORWARD == +Z` this term is `0`,
    // leaving `atan2(dir.x, dir.z)`. Keeping the term keeps `MESH_FORWARD` the
    // single source of truth: re-authoring the mesh-forward axis updates the result
    // without touching this math.
    let yaw = dir.x.atan2(dir.z) - MESH_FORWARD.x.atan2(MESH_FORWARD.z);
    Some(Quat::from_rotation_y(yaw))
}

/// What the FSM wants the steering layer to do this tick. Decoupled from the
/// steering API itself so the pure transition function carries no registry
/// dependency — the tick wrapper translates the intent into
/// `set_destination`/`clear_destination` calls.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SteeringIntent {
    /// Chase the player: the wrapper sets the agent destination to the player's
    /// position. Emitted in `Alert` and `Attack`.
    Chase,
    /// Stand down: the wrapper clears the agent destination. Emitted in `Idle`.
    Clear,
    /// Hold the current steering state (no set/clear). Emitted in `Death` so a
    /// dying enemy neither chases nor re-issues a clear every tick.
    Hold,
}

/// One transition evaluation's result: the next logical state plus what the
/// steering layer should do. Pure output of [`evaluate_transition`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct TransitionResult {
    pub(crate) next_state: LogicalState,
    pub(crate) steering: SteeringIntent,
}

/// The PURE FSM core: given the player position, the agent position, the resolved
/// tuning, the current logical state, and whether THIS tick re-evaluates target
/// acquisition (the think-stride gate), return the next state and the steering
/// intent. No registry, no `App`, no time — the unit tests drive it directly.
///
/// Closed transition set:
/// - `idle` → `alert` when the player enters `detection_range` (acquisition).
/// - `alert` → `idle` when the player leaves `leash_range` (acquisition).
/// - `alert` → `attack` when the player is within `attack_range`.
/// - `attack` → `alert` when the player leaves `attack_range`.
/// - `death` is terminal here (zero-HP death is layered by the caller, never by
///   this function — it has no HP input).
///
/// `evaluate_acquisition` gates ONLY the detection (`idle`→`alert`) and leash
/// (`alert`→`idle`) edges — the strided target-acquisition. The attack-range
/// edges (`alert`↔`attack`) are evaluated EVERY call regardless, so a strided
/// acquisition gap never suppresses an in-range attack transition. When
/// acquisition is gated off and the agent is already engaged, the agent keeps
/// chasing (steering stays `Chase`) — it does not drop the target mid-stride.
pub(crate) fn evaluate_transition(
    player_pos: Vec3,
    agent_pos: Vec3,
    tuning: &AiTuning,
    current: LogicalState,
    evaluate_acquisition: bool,
) -> TransitionResult {
    let distance = distance_xz(player_pos, agent_pos);
    match current {
        LogicalState::Idle => {
            // Detection is acquisition-gated: only re-checked on a think tick.
            if evaluate_acquisition && distance <= tuning.detection_range {
                // Newly alerted: if already inside attack range, go straight to
                // attack; otherwise chase.
                let next_state = if distance <= tuning.attack_range {
                    LogicalState::Attack
                } else {
                    LogicalState::Alert
                };
                return TransitionResult {
                    next_state,
                    steering: SteeringIntent::Chase,
                };
            }
            TransitionResult {
                next_state: LogicalState::Idle,
                steering: SteeringIntent::Clear,
            }
        }
        LogicalState::Alert => {
            // Attack-range entry is evaluated every tick (not acquisition-gated).
            if distance <= tuning.attack_range {
                return TransitionResult {
                    next_state: LogicalState::Attack,
                    steering: SteeringIntent::Chase,
                };
            }
            // Leash is acquisition-gated: only drop the target on a think tick.
            if evaluate_acquisition && distance > tuning.leash_range {
                return TransitionResult {
                    next_state: LogicalState::Idle,
                    steering: SteeringIntent::Clear,
                };
            }
            // Still engaged: keep chasing.
            TransitionResult {
                next_state: LogicalState::Alert,
                steering: SteeringIntent::Chase,
            }
        }
        LogicalState::Attack => {
            // Leaving attack range drops back to alert; evaluated every tick.
            if distance > tuning.attack_range {
                return TransitionResult {
                    next_state: LogicalState::Alert,
                    steering: SteeringIntent::Chase,
                };
            }
            TransitionResult {
                next_state: LogicalState::Attack,
                steering: SteeringIntent::Chase,
            }
        }
        // Terminal: the caller owns the zero-HP transition into death; once here
        // the FSM holds (despawn is owned by `run_ai_tick`'s death-countdown pass).
        LogicalState::Death => TransitionResult {
            next_state: LogicalState::Death,
            steering: SteeringIntent::Hold,
        },
    }
}

/// Locate the local player pawn's POSITION via its `Transform`. The Phase 0 sim
/// command drives one local pawn, so the registry marker wins; older
/// fixtures/maps with no marker fall back to the first entity carrying
/// `PlayerMovement`. This is the FSM's TARGETING input and is a DISTINCT id from
/// the damage target (`pawn_with_health`): the pawn is targeted by position but
/// damaged through the health chokepoint. `None` when there is no resolved pawn
/// or it carries no `Transform`.
fn player_position(registry: &EntityRegistry) -> Option<Vec3> {
    if let Some(pawn) = registry.local_player_pawn() {
        if matches!(
            registry.has_component_kind(pawn, ComponentKind::PlayerMovement),
            Ok(true)
        ) {
            return registry
                .get_component::<Transform>(pawn)
                .ok()
                .map(|t| t.position);
        }
    }

    let (pawn, _) = registry
        .iter_with_kind(ComponentKind::PlayerMovement)
        .next()?;
    registry
        .get_component::<Transform>(pawn)
        .ok()
        .map(|t| t.position)
}

/// Per-enemy snapshot captured under the immutable iterator borrow so the
/// mutable writes (steering, damage, animation) happen after the walk completes.
struct EnemySnapshot {
    id: EntityId,
    position: Vec3,
    brain: BrainComponent,
}

/// One enemy's resolved outcome after evaluating its brain this tick, applied in
/// a second pass under `&mut registry`.
struct EnemyOutcome {
    id: EntityId,
    brain: BrainComponent,
    steering: SteeringIntent,
    /// `true` when the logical state changed this tick — the apply pass then
    /// requests the brain-mapped animation for the new state.
    state_changed: bool,
    /// `true` when an attack landed this tick (damage applied, event raised).
    attacked: bool,
    /// `true` when this dead enemy's death-despawn countdown has elapsed and the
    /// AI tick should despawn it. Collected in the apply pass and despawned in a
    /// final two-pass step (collect-then-despawn) so the registry is never
    /// written mid-iteration (entity_model.md §3).
    despawn: bool,
}

/// Drive every enemy brain one tick. Returns the event names raised this tick
/// (one [`ENEMY_ATTACK_EVENT`] per enemy that attacked), for the app's post-tick
/// event drain. `tick_dt` is the fixed tick delta in seconds.
///
/// `warned` is the warn-once latch (owned by `App`), keyed and namespaced so a
/// given diagnostic fires once across the whole run, never each tick:
/// `anim:<name>` for an animation state that fails to switch
/// (`UnknownState`/`NotAnimated` — the prior animation is kept and the tick is
/// never aborted) and `blocked:<id>` for a chasing enemy whose agent found no
/// path to the player.
///
/// Ordering inside the tick, PER enemy:
/// 1. Tick the attack cooldown down (every tick).
/// 2. Zero-HP → `Death` (every tick, regardless of stride). Conversely, a brain
///    still in `Death` whose HP was restored above zero recovers to `Idle` (and
///    clears the despawn countdown) before the normal FSM runs, so it re-engages
///    instead of staying a frozen zombie.
/// 3. Otherwise evaluate the transition core, with acquisition gated by the
///    think stride (distance-derived). Attack-range edges + the cooldown check
///    are NOT strided.
/// 4. On an attack (in `Attack` with the cooldown elapsed) apply the configured
///    damage to the player through the chokepoint and raise the attack event.
/// 5. On a state CHANGE, request the mapped animation state.
///
/// Death + despawn: a zero-HP enemy enters `Death` (step 2), which seeds a
/// per-instance death-despawn countdown from `tuning.death_despawn_ms` (clamped
/// `>= 0`) on the entry tick and decrements it by the tick delta thereafter. The
/// TIMER is authoritative — the entity despawns after `death_despawn_ms`
/// regardless of whether the death clip ever resolved (an unresolved death clip
/// yields `UnknownState` and plays nothing). The despawn itself runs in a final
/// two-pass collect-then-despawn step so the registry is never written
/// mid-iteration. The kill was already counted ONCE at the death sweep's
/// authoritative `death_handled` latch (`systems/health.rs`); this tick owns
/// only the despawn, never the kill report.
pub(crate) fn run_ai_tick(
    registry: &mut EntityRegistry,
    warned: &mut HashSet<String>,
    tick_dt: f32,
) -> Vec<&'static str> {
    // The player POSITION (targeting). Absent pawn ⇒ no targets to evaluate;
    // every enemy still ticks its cooldown and resolves death.
    let player_pos = player_position(registry);

    // The DAMAGE TARGET id (distinct from the position pawn): the health
    // chokepoint addresses this id. Resolved once; `None` when the pawn carries
    // no health (damage then no-ops, matching `apply_damage`'s contract).
    // `player_alive` gates the attack so enemies do not keep swinging at — and
    // spamming `enemyAttack` for — an already-dead (HP <= 0) but still-present
    // player; the player-death/respawn flow is owned elsewhere.
    let player_pawn = pawn_with_health(registry);
    let damage_target: Option<EntityId> = player_pawn.as_ref().map(|(id, _)| *id);
    let player_alive = player_pawn.map(|(_, h)| h.current > 0.0).unwrap_or(false);

    let dt_ms = tick_dt.max(0.0) * 1000.0;

    // Pass 1: snapshot every brain-bearing enemy under the immutable borrow.
    let snapshots: Vec<EnemySnapshot> = registry
        .iter_with_kind(ComponentKind::Brain)
        .filter_map(|(id, value)| {
            let ComponentValue::Brain(brain) = value else {
                return None;
            };
            let position = registry.get_component::<Transform>(id).ok()?.position;
            Some(EnemySnapshot {
                id,
                position,
                brain: brain.clone(),
            })
        })
        .collect();

    // Pass 2 (compute): evaluate each brain, producing the outcomes to apply.
    let mut outcomes: Vec<EnemyOutcome> = Vec::with_capacity(snapshots.len());
    for snap in snapshots {
        let mut brain = snap.brain;
        let prior_state = brain.state;

        // (1) Cooldown ticks down every tick.
        brain.attack_cooldown_remaining_ms = (brain.attack_cooldown_remaining_ms - dt_ms).max(0.0);

        // Stride bookkeeping advances every tick so the gate is deterministic.
        brain.think_stride_counter = brain.think_stride_counter.wrapping_add(1);

        // (2) Zero-HP death check runs EVERY tick, regardless of stride and
        // regardless of whether a player exists. A dead enemy short-circuits all
        // targeting/attack logic. Non-finite HP counts as dead too (same predicate
        // as the death sweep), so a corrupt `current` cannot leave an enemy
        // immortal — `NaN <= 0.0` is false on its own.
        let is_dead = registry
            .get_component::<HealthComponent>(snap.id)
            .map(|h| h.current <= 0.0 || !h.current.is_finite())
            .unwrap_or(false);

        let mut attacked = false;
        let mut despawn = false;
        let steering;
        if is_dead {
            brain.state = LogicalState::Death;
            steering = SteeringIntent::Hold;

            // Death despawn countdown. Seeded once on entering Death (the
            // countdown is `None` until now), then decremented by the tick delta
            // each subsequent tick. The TIMER is authoritative — the entity
            // despawns after `death_despawn_ms` whether or not the death clip
            // resolved. A zero/negative configured value is clamped to `0` so the
            // entity still gets THIS one Death tick (death animation requested on
            // the state change below) before the despawn pass takes it: the
            // SEEDING tick never despawns, only a later decrement-to-zero does.
            match brain.death_despawn_remaining_ms {
                None => {
                    brain.death_despawn_remaining_ms = Some(brain.tuning.death_despawn_ms.max(0.0));
                }
                Some(remaining) => {
                    let next = (remaining - dt_ms).max(0.0);
                    brain.death_despawn_remaining_ms = Some(next);
                    if next <= 0.0 {
                        despawn = true;
                    }
                }
            }
        } else {
            // Not dead. Recover from a stale `Death` state BEFORE the normal FSM
            // runs: if HP was restored above zero (and finite) while the brain
            // still reads `Death`, reset it to `Idle` and clear the despawn
            // countdown so the entity re-engages instead of staying a frozen
            // zombie. `evaluate_transition` treats `Death` as terminal and has no
            // HP input, so this HP-driven recovery is the tick's responsibility.
            // Placed before the player-presence split so it runs for BOTH the
            // player and the no-player branch: with a player the normal
            // transition below re-acquires to `Alert` (or `Attack`) this same
            // tick if it is in range; with no player the `else` branch resolves
            // to `Idle`. The kill was already counted once at the death sweep's
            // `death_handled` latch; recovery touches only the FSM state, never
            // the kill accounting (the sweep remains the sole kill authority).
            if brain.state == LogicalState::Death {
                brain.state = LogicalState::Idle;
                brain.death_despawn_remaining_ms = None;
            }

            if let Some(player_pos) = player_pos {
                // The think stride is derived from the CURRENT player distance;
                // the gate fires when the per-enemy counter aligns with the
                // band's divisor. Acquisition (detection/leash) is evaluated only
                // on a think tick; attack-range edges + the cooldown check are
                // not.
                let distance = distance_xz(player_pos, snap.position);
                let stride = think_stride_for_distance(distance);
                let evaluate_acquisition = stride <= 1 || brain.think_stride_counter % stride == 0;

                let result = evaluate_transition(
                    player_pos,
                    snap.position,
                    &brain.tuning,
                    brain.state,
                    evaluate_acquisition,
                );
                brain.state = result.next_state;
                steering = result.steering;

                // (4) Attack: in `Attack` with the cooldown elapsed AND the
                // player still alive, apply the configured damage once and arm
                // the cooldown. Checked every tick. Gating on `player_alive`
                // stops attack/event spam against an already-dead but
                // still-present player.
                if brain.state == LogicalState::Attack
                    && brain.attack_cooldown_remaining_ms <= 0.0
                    && player_alive
                {
                    attacked = true;
                    brain.attack_cooldown_remaining_ms = brain.tuning.attack_cooldown_ms;
                }
            } else {
                // No player to target: idle and clear any stale steering.
                brain.state = LogicalState::Idle;
                steering = SteeringIntent::Clear;
            }
        }

        outcomes.push(EnemyOutcome {
            id: snap.id,
            state_changed: brain.state != prior_state,
            attacked,
            despawn,
            steering,
            brain,
        });
    }

    // Pass 3 (apply): write back brains, drive steering, apply damage, switch
    // animation. Mutable borrow only; no iterator held. Death despawns are NOT
    // applied here — they are collected and run in a final two-pass step below
    // so the registry is never written mid-iteration (entity_model.md §3, the
    // `sweep_deaths`/particle-sim precedent).
    let mut events: Vec<&'static str> = Vec::new();
    let mut to_despawn: Vec<EntityId> = Vec::new();
    for outcome in outcomes {
        if outcome.despawn {
            to_despawn.push(outcome.id);
        }
        // Persist the brain (state + timers + stride counter).
        let _ = registry.set_component(outcome.id, outcome.brain.clone());

        // Steering: chase sets the destination to the player, clear stands down,
        // hold leaves the agent untouched. `set_destination`/`clear_destination`
        // no-op when the enemy carries no agent component.
        match outcome.steering {
            SteeringIntent::Chase => {
                if let Some(player_pos) = player_pos {
                    agent_steering::set_destination(registry, outcome.id, player_pos);
                    // Diagnostic read of the steering surface: a chasing enemy
                    // whose agent cannot route to the player (no nav path) is
                    // `blocked`. Surface it once per enemy via the warn latch so a
                    // mis-placed spawn (off the navmesh, or behind a wall with no
                    // portal) is visible without per-tick spam. The steering tick
                    // still holds the agent in place; this only reports.
                    if let Some(state) = agent_steering::path_state(registry, outcome.id) {
                        if state.blocked {
                            let key = format!("blocked:{}", outcome.id.to_raw());
                            if warned.insert(key) {
                                log::warn!(
                                    "[AI] enemy {} is chasing the player but its agent \
                                     found no path (blocked); holding position. Warned \
                                     once per enemy.",
                                    outcome.id
                                );
                            }
                        }
                    }
                }
            }
            SteeringIntent::Clear => {
                agent_steering::clear_destination(registry, outcome.id);
            }
            SteeringIntent::Hold => {}
        }

        // Facing (yaw-only): nothing else writes the enemy's `Transform` rotation,
        // so without this the model keeps its spawn heading and moonwalks toward
        // the player. Orient it believably each tick it is engaged:
        //   - Moving (XZ speed above the epsilon): face the velocity direction, so
        //     it faces where it is going even when routing around obstacles. The
        //     velocity is read from `path_state` (last tick's resolved velocity) —
        //     a one-tick lag on facing that is imperceptible.
        //   - Stopped but engaged (`Alert`/`Attack` with near-zero XZ speed —
        //     arrived/blocked/swinging): face the player.
        //   - `Idle` (no target) and `Death`: leave facing untouched.
        // Yaw only (model stays upright); a zero-length direction yields `None` and
        // writes nothing (never a NaN yaw).
        if matches!(
            outcome.brain.state,
            LogicalState::Alert | LogicalState::Attack
        ) {
            if let Some(path) = agent_steering::path_state(registry, outcome.id) {
                let vel_xz_sq =
                    path.velocity.x * path.velocity.x + path.velocity.z * path.velocity.z;
                let facing = if vel_xz_sq > FACING_MOVE_SPEED_EPSILON * FACING_MOVE_SPEED_EPSILON {
                    // Moving: face the direction of travel.
                    yaw_rotation_toward(path.velocity)
                } else {
                    // Stopped but engaged: face the player (if one exists).
                    player_pos.and_then(|p| yaw_rotation_toward(p - path.position))
                };
                if let Some(rotation) = facing {
                    if let Ok(mut transform) =
                        registry.get_component::<Transform>(outcome.id).cloned()
                    {
                        transform.rotation = rotation;
                        let _ = registry.set_component(outcome.id, transform);
                    }
                }
            }
        }

        // Damage: route the configured amount through the chokepoint to the
        // DAMAGE-TARGET id (distinct from the position pawn), and raise the
        // attack event. `apply_damage` no-ops on a non-health / stale target.
        if outcome.attacked {
            if let Some(target) = damage_target {
                apply_damage(
                    registry,
                    target,
                    &DamagePayload {
                        amount: outcome.brain.tuning.attack_damage,
                    },
                );
            }
            events.push(ENEMY_ATTACK_EVENT);

            // Replay the attack clip on every IN-STATE swing. The attack clip is
            // one-shot (`loop:false`) and animation is otherwise switched only on
            // `state_changed`, so a repeated cooldown-gated swing while the enemy
            // STAYS in `Attack` would leave the clip clamped on its last frame —
            // the player cannot tell they are being hit. Restarting it from frame 0
            // re-fires the swing visually. This is purely cosmetic: damage stays
            // cooldown-gated above (NOT frame-synced).
            //
            // Guard on `!state_changed`: on the entry tick INTO `Attack` the
            // `state_changed` switch below already plays the clip from zero, so a
            // restart here would double-fire (it would be a harmless re-stamp of a
            // just-stamped pending clip, but skipping it keeps the seam explicit:
            // first swing via the switch, every later in-state swing via restart).
            if !outcome.state_changed {
                let name = outcome
                    .brain
                    .tuning
                    .states
                    .animation_for(outcome.brain.state);
                let _ = restart_animation_clip(registry, outcome.id, name);
            }
        }

        // Animation: on a state change, request the brain-mapped animation name
        // for the new state. A failed switch (`UnknownState`/`NotAnimated`)
        // warns ONCE per distinct name and keeps the prior animation — it never
        // aborts the tick.
        if outcome.state_changed {
            let name = outcome
                .brain
                .tuning
                .states
                .animation_for(outcome.brain.state);
            match switch_animation_state(registry, outcome.id, name) {
                SwitchResult::Switched | SwitchResult::AlreadyInState => {}
                SwitchResult::UnknownState | SwitchResult::NotAnimated => {
                    if warned.insert(format!("anim:{name}")) {
                        log::warn!(
                            "[AI] enemy animation state `{name}` could not be switched \
                             (undeclared/unresolved on the mesh); keeping the prior \
                             animation. Warned once per distinct name."
                        );
                    }
                }
            }
        }
    }

    // Pass 4 (despawn): two-pass collect-then-despawn. The despawn ids were
    // collected under the mutable apply walk above (never despawned mid-walk);
    // here, after that walk completes, each dead enemy whose death-despawn timer
    // elapsed is removed. The kill was already counted at the sweep's
    // authoritative latch, so despawning here never re-reports it.
    for id in to_despawn {
        let _ = registry.despawn(id);
    }

    events
}

#[cfg(test)]
#[path = "ai_tests.rs"]
mod tests;
