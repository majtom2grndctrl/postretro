// Player movement component: live state carried on the player entity.
// Materialized at spawn from `PlayerMovementDescriptor`; mutated each tick
// by `crate::movement::tick`.
//
// See: context/lib/entity_model.md §7 (collision/movement)

use glam::Vec3;
use serde::{Deserialize, Serialize};

use crate::scripting::data_descriptors::{
    AirParams, CapsuleParams, FallParams, GroundParams, PlayerMovementDescriptor,
};

/// The player's active movement state. Mutually-exclusive: exactly one state
/// owns the per-tick velocity intent at a time. `tick` dispatches to the
/// active state's intent step, runs the shared collision substrate, then
/// applies any transition the intent returns. Today only `Normal` exists
/// (the walk/run/jump/air-control baseline); later states (dash, crouch,
/// slide, wall-run, vault) plug in behind the same seam.
///
/// See: context/lib/movement.md §4 (state-machine seam).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub(crate) enum MovementState {
    /// Baseline locomotion: gravity, jump/air-jump, ground acceleration,
    /// ground friction, airborne cap. The behavior-unchanged baseline.
    #[default]
    Normal,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub(crate) struct PlayerMovementComponent {
    pub(crate) capsule: CapsuleParams,
    pub(crate) ground: GroundParams,
    pub(crate) air: AirParams,
    pub(crate) fall: FallParams,
    /// Cosine of `ground.max_slope` (degrees → radians → cos), precomputed at
    /// materialization so the per-tick floor check is a single dot-product
    /// compare. A surface counts as walkable when the contact normal's Y
    /// component (the dot with world-up) is at least this value.
    pub(crate) cos_walkable: f32,
    pub(crate) is_grounded: bool,
    pub(crate) velocity: Vec3,
    pub(crate) air_jumps_remaining: u32,
    /// Consecutive ticks the player has spent without floor contact. Used to
    /// gate `landed` event emission so the 1-tick airborne blip introduced by
    /// the step-up probe's vertical lift cannot fire spurious landings during
    /// normal walking. Reset to 0 on any tick with floor contact; incremented
    /// otherwise.
    pub(crate) air_ticks: u32,
    /// Stuck-stop deadzone: when enabled, the slide loop zeroes horizontal
    /// velocity and rolls back XZ position when contradictory wall normals
    /// (≥60° apart horizontally) are seen within the same tick AND net
    /// horizontal displacement is below `stuck_stop_threshold`. This is the
    /// geometric signature of a corner wedge; max-iteration exhaustion alone
    /// does not trigger it. Suppresses orbital jitter in interior corners.
    /// Disable for gameplay scenarios that want looser physics-driven
    /// micro-motion.
    pub(crate) stuck_stop_enabled: bool,
    /// Horizontal-displacement threshold (metres). The deadzone fires only
    /// when contradictory wall normals were seen and net XZ displacement
    /// this tick is below this value. Tuned well below a single tick's
    /// normal displacement at the canonical ground speed (7 m/s × 1/60 s ≈
    /// 0.117 m) yet above floating-point and skin-distance noise (~1e-4 m).
    pub(crate) stuck_stop_threshold: f32,
    /// The active movement state. Drives `tick`'s per-tick velocity-intent
    /// dispatch. Defaults to `Normal`; carried as live runtime state across
    /// descriptor refresh.
    pub(crate) movement_state: MovementState,
}

impl PlayerMovementComponent {
    /// Materialize from a descriptor. The descriptor's `ground.max_slope` is
    /// in degrees; precomputed `cos_walkable` lets the runtime skip the
    /// per-tick degrees→radians→cosine work.
    pub(crate) fn from_descriptor(desc: &PlayerMovementDescriptor) -> Self {
        let cos_walkable = desc.ground.max_slope.to_radians().cos();
        let air_jumps_remaining = desc.air.jumps;
        Self {
            capsule: desc.capsule.clone(),
            ground: desc.ground.clone(),
            air: desc.air.clone(),
            fall: desc.fall.clone(),
            cos_walkable,
            is_grounded: false,
            velocity: Vec3::ZERO,
            air_jumps_remaining,
            air_ticks: 0,
            stuck_stop_enabled: desc.stuck_stop_enabled,
            stuck_stop_threshold: desc.stuck_stop_threshold,
            movement_state: MovementState::Normal,
        }
    }

    /// Landing-refresh point: reset every ability budget on floor contact.
    /// Invoked from `tick` after the collision substrate reports floor
    /// contact (`SubstrateResult::hit_floor`). Today only `air_jumps_remaining`
    /// resets; later budgets (air-dash, Task 4) hook this same single point so
    /// every charge replenishes uniformly on landing.
    pub(crate) fn refresh_on_landing(&mut self) {
        self.air_jumps_remaining = self.air.jumps;
    }
}

// Manual Serialize/Deserialize impls on the descriptor sub-structs are not
// present; derive on this component requires the sub-structs to derive too.
