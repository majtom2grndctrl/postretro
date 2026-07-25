// The enemy FSM's pure transition core: state/steering evaluation plus the
// think-stride and target-switch hysteresis helpers it depends on.
// See: context/lib/entity_model.md §2 (engine components)

use glam::Vec3;

use crate::nav::distance_xz;
use postretro_entities::components::brain::{AiTuning, LogicalState};

/// Think-stride bands. Target acquisition is time-sliced by player distance:
/// near enemies re-evaluate every tick, mid enemies every few ticks, distant
/// enemies rarely. The cheap retained-target leash check and the
/// attack-in-range/cooldown check are NOT strided — they run every tick
/// regardless, so a strided acquisition gap can never suppress an in-stride
/// attack or leash escape.
///
/// Distances are XZ ground distances (the navmesh plane); the bands are coarse
/// by design — stride is a cost knob, not a gameplay contract.
pub(super) const STRIDE_NEAR_DISTANCE: f32 = 12.0;
pub(super) const STRIDE_MID_DISTANCE: f32 = 30.0;
/// Stride divisor for each band: `1` = every tick, `n` = once every `n` ticks.
const STRIDE_NEAR: u32 = 1;
const STRIDE_MID: u32 = 4;
const STRIDE_FAR: u32 = 12;

/// Target switching hysteresis in world units on the XZ plane. A retained target
/// stays sticky unless another pawn is MORE than this much closer, preventing
/// co-op target churn when players are only slightly offset from one another.
pub(super) const TARGET_SWITCH_HYSTERESIS_DISTANCE: f32 = 1.0;

pub(super) fn is_meaningfully_closer(candidate_distance: f32, retained_distance: f32) -> bool {
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
/// - `death` is terminal here. It has no HP input and is not HP-reachable.
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
        // Terminal: this state is no longer HP-reachable. An authored deferred
        // despawn owns removal; once another path enters Death, the FSM holds.
        LogicalState::Death => TransitionResult {
            next_state: LogicalState::Death,
            steering: SteeringIntent::Hold,
        },
    }
}
