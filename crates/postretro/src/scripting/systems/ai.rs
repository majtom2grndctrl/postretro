// Engine-owned enemy FSM tick: the system half of the brain (the per-instance
// DATA + spawn-time state-map validation live in `components/brain.rs`). Each
// think tick selects a target pawn, evaluates the closed transition set
// (idle/alert/attack/death), drives the steering API toward a combat slot or
// raw target fallback while chasing, applies damage on the attack cooldown, and
// requests the mapped animation state. The transition CORE is a pure function
// over (target position, agent position, tuning, current state) so it is
// unit-testable without `App` or a GPU; the tick wrapper layers the registry
// reads/writes, the zero-HP death check, damage, and animation switching on top.
//
// Architectural decision (M10): an engine-owned Rust FSM with a closed
// transition set; tuning is declarative; there is no live VM at tick. Scripts
// declare thresholds and the logical→animation map; Rust executes.
//
// See: context/lib/entity_model.md §2 (engine components), §5 (fixed-tick game
//      logic), §7 (collision)
//      context/lib/scripting.md §1 (scripts declare, Rust executes),
//      §10.5 (the contextual damage chokepoint)
//      crates/postretro/src/scripting/components/brain.rs (BrainComponent /
//      LogicalState / AiTuning — the FSM data this tick drives)
//      crates/postretro/src/agent_steering.rs (set_destination /
//      clear_destination / path_state — the steering surface this tick drives)

use std::collections::{HashMap, HashSet};

use glam::{Quat, Vec3};

use crate::agent_steering;
use crate::collision::CollisionWorld;
use crate::combat_positioning::{
    CombatAgentSnapshot, CombatCandidate, CombatQuery, PATH_LENGTH_SCORE_WEIGHT,
    select_combat_positions_batch,
};
use crate::nav::{NavGraph, distance_xz};
use postretro_entities::components::brain::{AiStateMap, AiTuning, BrainComponent, LogicalState};
use postretro_entities::components::health::{
    DamageContext, HealthComponent, apply_damage_with_context,
};
use postretro_entities::components::mesh::{
    SwitchResult, restart_animation_clip, switch_animation_state,
};
use postretro_entities::components::player_movement::PlayerMovementComponent;
use postretro_entities::{ComponentKind, ComponentValue, EntityId, EntityRegistry, Transform};
use postretro_foundation::DamagePayload;

/// Event name fired once per enemy attack that lands this tick. Mirrors the
/// weapon-fire event precedent (`"activate"`/`"impact"`): the tick returns the
/// names it raised and the app drains them through `fire_named_event` after the
/// tick loop settles.
pub(crate) const ENEMY_ATTACK_EVENT: &str = "enemyAttack";
const ENEMY_ATTACK_SOURCE_ID: &str = "enemy.attack";

/// Think-stride bands. Target acquisition is time-sliced by player distance:
/// near enemies re-evaluate every tick, mid enemies every few ticks, distant
/// enemies rarely. The cheap retained-target leash check, the
/// attack-in-range/cooldown check, and the zero-HP death check are NOT strided —
/// they run every tick regardless, so a strided acquisition gap can never
/// suppress an in-stride attack, death, or leash escape.
///
/// Distances are XZ ground distances (the navmesh plane); the bands are coarse
/// by design — stride is a cost knob, not a gameplay contract.
const STRIDE_NEAR_DISTANCE: f32 = 12.0;
const STRIDE_MID_DISTANCE: f32 = 30.0;
/// Stride divisor for each band: `1` = every tick, `n` = once every `n` ticks.
const STRIDE_NEAR: u32 = 1;
const STRIDE_MID: u32 = 4;
const STRIDE_FAR: u32 = 12;

/// Target switching hysteresis in world units on the XZ plane. A retained target
/// stays sticky unless another pawn is MORE than this much closer, preventing
/// co-op target churn when players are only slightly offset from one another.
const TARGET_SWITCH_HYSTERESIS_DISTANCE: f32 = 1.0;
const COMBAT_SLOT_HOLD_TICKS: u32 = 8;

fn is_meaningfully_closer(candidate_distance: f32, retained_distance: f32) -> bool {
    candidate_distance + TARGET_SWITCH_HYSTERESIS_DISTANCE < retained_distance
}

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

/// Minimum XZ speed (units/sec) the agent must exceed for "moving" behavior:
/// above it the enemy orients to its velocity and `Alert` selects its walk
/// animation; at or below it the enemy is treated as stopped and uses player
/// facing/idle animation. A shared epsilon keeps facing and locomotion animation
/// in agreement.
const MOVE_SPEED_EPSILON: f32 = 0.05;

/// Maximum enemy-facing yaw rotation, in radians/sec. Higher than path steering
/// so visual facing catches up quickly without snapping.
pub(crate) const FACING_TURN_RATE: f32 = crate::agent_steering::MAX_TURN_RATE * 2.0;

#[derive(Debug, Clone, Copy, PartialEq)]
struct LocomotionIntent {
    moving: bool,
    speed_xz_sq: f32,
}

impl LocomotionIntent {
    const STOPPED: Self = Self {
        moving: false,
        speed_xz_sq: 0.0,
    };

    fn from_velocity(velocity: Vec3) -> Self {
        let speed_xz_sq = velocity.x * velocity.x + velocity.z * velocity.z;
        Self {
            moving: speed_xz_sq > MOVE_SPEED_EPSILON * MOVE_SPEED_EPSILON,
            speed_xz_sq,
        }
    }
}

fn animation_for_locomotion(
    state: LogicalState,
    intent: LocomotionIntent,
    states: &AiStateMap,
) -> &str {
    if state == LogicalState::Alert && !intent.moving {
        states.animation_for(LogicalState::Idle)
    } else {
        states.animation_for(state)
    }
}

fn should_switch_animation(state_changed: bool, moving: bool, latch: bool) -> bool {
    state_changed || moving != latch
}

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

fn yaw_from_rotation(rotation: Quat) -> f32 {
    let heading = rotation * MESH_FORWARD;
    heading.x.atan2(heading.z)
}

/// Advance `current` yaw toward `target` by at most `max_delta` radians along the
/// shortest arc. Returns `target` exactly when it is within the per-tick budget,
/// preserving exact arrival instead of orbiting around the goal.
pub(crate) fn slew_yaw(current: f32, target: f32, max_delta: f32) -> f32 {
    let delta = (target - current + std::f32::consts::PI).rem_euclid(std::f32::consts::TAU)
        - std::f32::consts::PI;
    let max_delta = max_delta.max(0.0);
    if delta.abs() <= max_delta {
        target
    } else {
        current + delta.signum() * max_delta
    }
}

/// What the FSM wants the steering layer to do this tick. Decoupled from the
/// steering API itself so the pure transition function carries no registry
/// dependency — the tick wrapper translates the intent into
/// `set_destination`/`clear_destination` calls.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SteeringIntent {
    /// Chase: the wrapper prefers a combat slot around the selected target and
    /// falls back to the target position. Emitted in `Alert` and `Attack`.
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

#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct TargetPawn {
    pub(crate) entity: EntityId,
    pub(crate) position: Vec3,
}

#[derive(Debug, Clone, Copy, PartialEq)]
struct TargetCandidate {
    target: TargetPawn,
    distance: f32,
}

fn target_candidate(
    registry: &EntityRegistry,
    entity: EntityId,
    from: Vec3,
    visible: Option<&dyn Fn(EntityId) -> bool>,
) -> Option<TargetCandidate> {
    if visible.is_some_and(|is_visible| !is_visible(entity)) {
        return None;
    }
    registry
        .get_component::<PlayerMovementComponent>(entity)
        .ok()?;
    let position = registry.get_component::<Transform>(entity).ok()?.position;
    Some(TargetCandidate {
        target: TargetPawn { entity, position },
        distance: distance_xz(position, from),
    })
}

fn nearest_target_candidate(
    registry: &EntityRegistry,
    from: Vec3,
    visible: Option<&dyn Fn(EntityId) -> bool>,
    exclude: Option<EntityId>,
) -> Option<TargetCandidate> {
    registry
        .iter_with_kind(ComponentKind::PlayerMovement)
        .filter_map(|(entity, _)| {
            if exclude == Some(entity) {
                return None;
            }
            target_candidate(registry, entity, from, visible)
        })
        .min_by(|a, b| a.distance.total_cmp(&b.distance))
}

fn target_distance(target: TargetPawn, from: Vec3) -> f32 {
    distance_xz(target.position, from)
}

fn acquisition_due(brain: &BrainComponent, distance: Option<f32>) -> bool {
    distance
        .map(|distance| {
            let stride = think_stride_for_distance(distance);
            stride <= 1 || brain.think_stride_counter.wrapping_add(1) % stride == 0
        })
        .unwrap_or(true)
}

fn selected_target_alive(registry: &EntityRegistry, target: EntityId) -> bool {
    registry
        .get_component::<HealthComponent>(target)
        .map(|health| health.current > 0.0 && health.current.is_finite())
        .unwrap_or(false)
}

/// Select the player pawn this enemy should pursue.
///
/// This is the AI targeting extension point: v1 ranks all
/// [`ComponentKind::PlayerMovement`] pawns by nearest XZ distance from `from`.
/// The optional predicate is the future visibility/relevance seam intended for
/// `context/research/cell-visibility-substrate.md` (and exact LOS work) without
/// re-threading the FSM. If `retained_target` is still a valid, relevant player
/// pawn, it is preferred unless another pawn is meaningfully closer by
/// [`TARGET_SWITCH_HYSTERESIS_DISTANCE`]. When `retained_outside_leash` is true,
/// the retained pawn is no longer relevant for this acquisition tick and is
/// excluded; the caller still owns any leash/range rules for replacements. This
/// targeting path intentionally does not consult the registry's local-player
/// marker, which is client-side convenience state.
pub(crate) fn select_target(
    registry: &EntityRegistry,
    from: Vec3,
    retained_target: Option<EntityId>,
    retained_outside_leash: bool,
    visible: Option<&dyn Fn(EntityId) -> bool>,
) -> Option<TargetPawn> {
    let retained = retained_target
        .filter(|_| !retained_outside_leash)
        .and_then(|entity| target_candidate(registry, entity, from, visible));
    let nearest = nearest_target_candidate(
        registry,
        from,
        visible,
        retained_target.filter(|_| retained_outside_leash),
    );

    match (retained, nearest) {
        (Some(retained), Some(nearest))
            if nearest.target.entity != retained.target.entity
                && is_meaningfully_closer(nearest.distance, retained.distance) =>
        {
            Some(nearest.target)
        }
        (Some(retained), _) => Some(retained.target),
        (None, Some(nearest)) => Some(nearest.target),
        (None, None) => None,
    }
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
    target: Option<TargetPawn>,
    brain: BrainComponent,
    steering: SteeringIntent,
    combat_slot: Option<Vec3>,
    /// `true` when the logical state changed this tick; the apply pass uses this
    /// with locomotion intent changes to decide whether to switch animation.
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
/// path to its selected destination.
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
///    damage to the selected target pawn through the chokepoint and raise the
///    attack event.
/// 5. On a state CHANGE or locomotion stop/resume, request the selected
///    animation state.
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
#[cfg_attr(not(test), allow(dead_code))]
pub(crate) fn run_ai_tick(
    registry: &mut EntityRegistry,
    warned: &mut HashSet<String>,
    tick_dt: f32,
) -> Vec<&'static str> {
    run_ai_tick_with_navigation(registry, warned, tick_dt, None, None)
}

pub(crate) fn run_ai_tick_with_navigation(
    registry: &mut EntityRegistry,
    warned: &mut HashSet<String>,
    tick_dt: f32,
    nav_graph: Option<&NavGraph>,
    collision_world: Option<&CollisionWorld>,
) -> Vec<&'static str> {
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
    for snap in &snapshots {
        let mut brain = snap.brain.clone();
        let prior_state = brain.state;
        let (target, evaluate_acquisition) = if brain.aggro_armed {
            let retained_target = matches!(brain.state, LogicalState::Alert | LogicalState::Attack)
                .then_some(brain.acquired_target)
                .flatten();
            let retained = retained_target
                .and_then(|entity| target_candidate(registry, entity, snap.position, None));
            let nearest = retained
                .is_none()
                .then(|| select_target(registry, snap.position, None, false, None))
                .flatten();
            let current_target = retained.map(|candidate| candidate.target).or(nearest);
            let current_distance =
                current_target.map(|target| target_distance(target, snap.position));
            let evaluate_acquisition = acquisition_due(&brain, current_distance);

            let retained_outside_leash =
                retained.is_some_and(|retained| retained.distance > brain.tuning.leash_range);
            let target = if retained_outside_leash {
                // Leash escape for the already-retained target is cheap because the
                // retained pawn has already been read. Clear immediately instead of
                // continuing to chase stale destinations between acquisition ticks.
                // Replacement search still stays acquisition-strided.
                if evaluate_acquisition {
                    select_target(
                        registry,
                        snap.position,
                        retained.map(|retained| retained.target.entity),
                        true,
                        None,
                    )
                    .filter(|target| {
                        target_distance(*target, snap.position) <= brain.tuning.leash_range
                    })
                } else {
                    None
                }
            } else if evaluate_acquisition {
                if let Some(retained) = retained {
                    select_target(
                        registry,
                        snap.position,
                        Some(retained.target.entity),
                        false,
                        None,
                    )
                } else {
                    select_target(registry, snap.position, retained_target, false, None)
                }
            } else {
                current_target
            };
            (target, evaluate_acquisition)
        } else {
            (None, false)
        };

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
            brain.acquired_target = None;
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

            if !brain.aggro_armed {
                // The aggro gate's v1 disengage policy is hold. This deliberately
                // runs after the death/recovery handling above: sealed enemies
                // remain damageable and still enter Death/despawn. A closed brain
                // neither consults target selection nor evaluates FSM transitions;
                // clearing its destination sends the agent through steering's
                // destination-less idle-settle path, which has no separation push.
                brain.state = LogicalState::Idle;
                brain.acquired_target = None;
                steering = SteeringIntent::Clear;
            } else if let Some(target) = target {
                // The think stride is derived from the CURRENT player distance;
                // the gate fires when the per-enemy counter aligns with the
                // band's divisor. Acquisition (detection/leash) is evaluated only
                // on a think tick; attack-range edges + the cooldown check are
                // not.
                let result = evaluate_transition(
                    target.position,
                    snap.position,
                    &brain.tuning,
                    brain.state,
                    evaluate_acquisition,
                );
                brain.state = result.next_state;
                steering = result.steering;
                if matches!(brain.state, LogicalState::Alert | LogicalState::Attack)
                    && steering == SteeringIntent::Chase
                {
                    brain.acquired_target = Some(target.entity);
                } else {
                    brain.acquired_target = None;
                }

                // (4) Attack: in `Attack` with the cooldown elapsed AND the
                // SELECTED target still alive, apply the configured damage once
                // and arm the cooldown. Checked every tick. Gating on the
                // selected target's Health stops attack/event spam against an
                // already-dead but still-present pawn and prevents damaging a
                // different co-op pawn than the one this enemy chose.
                if brain.state == LogicalState::Attack
                    && brain.attack_cooldown_remaining_ms <= 0.0
                    && selected_target_alive(registry, target.entity)
                {
                    attacked = true;
                    brain.attack_cooldown_remaining_ms = brain.tuning.attack_cooldown_ms;
                }
            } else {
                // No player to target: idle and clear any stale steering.
                brain.state = LogicalState::Idle;
                brain.acquired_target = None;
                steering = SteeringIntent::Clear;
            }
        }

        outcomes.push(EnemyOutcome {
            id: snap.id,
            target,
            state_changed: brain.state != prior_state,
            attacked,
            despawn,
            steering,
            combat_slot: None,
            brain,
        });
    }

    resolve_combat_slots(&snapshots, &mut outcomes, nav_graph, collision_world);

    // Pass 3 (apply): write back brains, drive steering, apply damage, switch
    // animation. Mutable borrow only; no iterator held. Death despawns are NOT
    // applied here — they are collected and run in a final two-pass step below
    // so the registry is never written mid-iteration (entity_model.md §3, the
    // `sweep_deaths`/particle-sim precedent).
    let mut events: Vec<&'static str> = Vec::new();
    let mut to_despawn: Vec<EntityId> = Vec::new();
    for mut outcome in outcomes {
        if outcome.despawn {
            to_despawn.push(outcome.id);
        }
        // Persist the brain (state + timers + stride counter).
        let _ = registry.set_component(outcome.id, outcome.brain.clone());

        let path_state = agent_steering::path_state(registry, outcome.id);
        let locomotion_intent = if outcome.brain.aggro_armed {
            path_state
                .as_ref()
                .map(|path| LocomotionIntent::from_velocity(path.velocity))
                .unwrap_or(LocomotionIntent::STOPPED)
        } else {
            LocomotionIntent::STOPPED
        };

        // Steering: chase sets the destination to a selected combat slot when
        // one is available, otherwise to the raw target position. Clear stands
        // down; hold leaves the agent untouched.
        // `set_destination`/`clear_destination` no-op when the enemy carries no
        // agent component.
        match outcome.steering {
            SteeringIntent::Chase => {
                if let Some(target) = outcome.target {
                    let destination = outcome.combat_slot.unwrap_or(target.position);
                    agent_steering::set_destination(registry, outcome.id, destination);
                    // Diagnostic read of the steering surface: a chasing enemy
                    // whose agent cannot route to its selected destination (a
                    // combat slot, or the raw target fallback when no slot was
                    // assigned) is `blocked`. Surface it once per enemy via the
                    // warn latch so a mis-placed spawn (off the navmesh, or
                    // behind a wall with no portal) is visible without per-tick
                    // spam. The steering tick still holds the agent in place;
                    // this only reports.
                    if let Some(state) = path_state.as_ref() {
                        if state.blocked {
                            let key = format!("blocked:{}", outcome.id.to_raw());
                            if warned.insert(key) {
                                log::warn!(
                                    "[AI] enemy {} is chasing its selected destination but its agent \
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
        // its selected target. Orient it believably each tick it is engaged:
        //   - Moving (XZ speed above the epsilon): face the velocity direction, so
        //     it faces where it is going even when routing around obstacles. The
        //     velocity is read from `path_state` (last tick's resolved velocity) —
        //     a one-tick lag on facing that is imperceptible.
        //   - Stopped but engaged (`Alert`/`Attack` with near-zero XZ speed —
        //     arrived/blocked/swinging): face this enemy's selected target.
        //   - `Idle` (no target) and `Death`: leave facing untouched.
        // Yaw only (model stays upright); a zero-length direction yields `None` and
        // writes nothing (never a NaN yaw).
        if outcome.brain.aggro_armed
            && matches!(
                outcome.brain.state,
                LogicalState::Alert | LogicalState::Attack
            )
        {
            if let Some(path) = path_state.as_ref() {
                let facing =
                    if locomotion_intent.speed_xz_sq > MOVE_SPEED_EPSILON * MOVE_SPEED_EPSILON {
                        // Moving: face the direction of travel.
                        yaw_rotation_toward(path.velocity)
                    } else {
                        // Stopped but engaged: face this enemy's selected target
                        // (if one exists).
                        outcome
                            .target
                            .and_then(|target| yaw_rotation_toward(target.position - path.position))
                    };
                if let Some(target_rotation) = facing {
                    if let Ok(mut transform) =
                        registry.get_component::<Transform>(outcome.id).cloned()
                    {
                        let current_yaw = yaw_from_rotation(transform.rotation);
                        let target_yaw = yaw_from_rotation(target_rotation);
                        let slewed_yaw =
                            slew_yaw(current_yaw, target_yaw, FACING_TURN_RATE * tick_dt);
                        transform.rotation = Quat::from_rotation_y(slewed_yaw);
                        let _ = registry.set_component(outcome.id, transform);
                    }
                }
            }
        }

        // Damage: route the configured amount through the chokepoint to the
        // SELECTED target id, and raise the attack event. The chokepoint no-ops
        // on a non-health / stale target, but attacks are only marked above
        // after confirming this selected pawn currently has live Health.
        if outcome.attacked {
            if let Some(target) = outcome.target {
                apply_damage_with_context(
                    registry,
                    target.entity,
                    &DamagePayload {
                        amount: outcome.brain.tuning.attack_damage,
                    },
                    DamageContext {
                        source_id: ENEMY_ATTACK_SOURCE_ID.to_string(),
                        attacker: Some(outcome.id),
                        weapon: None,
                        zone: None,
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

        // Animation: on a state change or locomotion stop/resume, request the
        // selected animation name for the new logical/locomotion state. A failed
        // switch (`UnknownState`/`NotAnimated`) warns ONCE per distinct name and
        // keeps the prior animation — it never aborts the tick. The locomotion
        // latch is still persisted after failures so unresolved clips do not
        // re-request the same switch every tick.
        if should_switch_animation(
            outcome.state_changed,
            locomotion_intent.moving,
            outcome.brain.locomotion_moving,
        ) {
            let name = animation_for_locomotion(
                outcome.brain.state,
                locomotion_intent,
                &outcome.brain.tuning.states,
            );
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
        outcome.brain.locomotion_moving = locomotion_intent.moving;
        let _ = registry.set_component(outcome.id, outcome.brain.clone());
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

fn resolve_combat_slots(
    snapshots: &[EnemySnapshot],
    outcomes: &mut [EnemyOutcome],
    nav_graph: Option<&NavGraph>,
    collision_world: Option<&CollisionWorld>,
) {
    for outcome in outcomes.iter_mut() {
        outcome.combat_slot = None;
        if outcome.steering != SteeringIntent::Chase || outcome.target.is_none() {
            clear_combat_slot(outcome);
        }
    }

    let (Some(nav_graph), Some(collision_world)) = (nav_graph, collision_world) else {
        for outcome in outcomes.iter_mut() {
            clear_combat_slot(outcome);
        }
        return;
    };

    if !outcomes
        .iter()
        .any(|outcome| outcome.steering == SteeringIntent::Chase && outcome.target.is_some())
    {
        return;
    }

    let other_agents: Vec<CombatAgentSnapshot> = snapshots
        .iter()
        .map(|snap| CombatAgentSnapshot {
            claimant_id: snap.id.to_raw(),
            position: snap.position,
        })
        .collect();

    let mut queries = Vec::new();
    for (snap, outcome) in snapshots.iter().zip(outcomes.iter()) {
        if outcome.steering != SteeringIntent::Chase {
            continue;
        }
        let Some(target) = outcome.target else {
            continue;
        };
        let retained_slot = retained_combat_slot(snap, outcome);
        queries.push(CombatQuery {
            claimant_id: outcome.id.to_raw(),
            agent_pos: snap.position,
            engagement_radius: outcome.brain.tuning.attack_range,
            target_pos: target.position,
            combat_slot: retained_slot,
            scan_challengers: retained_slot.is_none(),
            other_agents: &other_agents,
            nav_graph,
            collision_world,
            path_length_score_weight: PATH_LENGTH_SCORE_WEIGHT,
        });
    }

    let assignments: HashMap<u32, Option<CombatCandidate>> =
        select_combat_positions_batch(&queries)
            .into_iter()
            .map(|assignment| (assignment.claimant_id, assignment.candidate))
            .collect();

    for outcome in outcomes.iter_mut() {
        if outcome.steering != SteeringIntent::Chase || outcome.target.is_none() {
            clear_combat_slot(outcome);
            continue;
        }

        match assignments.get(&outcome.id.to_raw()).copied().flatten() {
            Some(candidate) => {
                outcome.combat_slot = Some(candidate.position);
                outcome.brain.combat_slot = Some(candidate.position);
                outcome.brain.combat_slot_hold_ticks = if candidate.is_incumbent {
                    outcome.brain.combat_slot_hold_ticks.saturating_sub(1)
                } else {
                    COMBAT_SLOT_HOLD_TICKS
                };
            }
            None => {
                clear_combat_slot(outcome);
            }
        }
    }
}

fn clear_combat_slot(outcome: &mut EnemyOutcome) {
    outcome.combat_slot = None;
    outcome.brain.combat_slot = None;
    outcome.brain.combat_slot_hold_ticks = 0;
}

fn retained_combat_slot(snap: &EnemySnapshot, outcome: &EnemyOutcome) -> Option<Vec3> {
    let target = outcome.target?;
    (outcome.steering == SteeringIntent::Chase
        && snap.brain.acquired_target == Some(target.entity)
        && snap.brain.combat_slot_hold_ticks > 0)
        .then_some(snap.brain.combat_slot)
        .flatten()
}

#[cfg(test)]
#[path = "ai_tests.rs"]
mod tests;
