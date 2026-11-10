// Enemy visual facing: the facing policy, its direction, and yaw helpers.
// See: context/lib/entity_model.md §7c (enemy brain component)

use glam::{Quat, Vec3};
use postretro_foundation::FacingPolicy;

use super::LocomotionIntent;
use super::engine_floor::SteeringIntent;

/// Maximum enemy-facing yaw rotation, in radians/sec. Higher than path steering
/// so visual facing catches up quickly without snapping.
pub(super) const FACING_TURN_RATE: f32 = crate::agent_steering::MAX_TURN_RATE * 2.0;

/// Maximum yaw residual allowed for an attack to connect. The fire gate compares
/// this against the heading after this tick's facing slew, so reaching the
/// boundary on the current tick permits the attack without a one-tick delay.
pub(super) const ATTACK_FACING_TOLERANCE_RADIANS: f32 = std::f32::consts::PI / 12.0;
const ATTACK_FACING_COMPARISON_EPSILON: f32 = 2.0 * f32::EPSILON;

/// The reference enemy mesh's visual forward axis in model space. The skinned
/// glTF characters (`content/dev/models/mutant` and the rifle squad) are
/// authored facing `+Z`; the renderer applies `Transform.rotation` directly to
/// the model matrix, so aiming this axis at a target makes the mesh face it.
///
/// This is the opposite of the engine camera/view forward (`-Z`). Facing code
/// orients a rendered mesh, so it must aim the mesh's authored front (`+Z`).
const MESH_FORWARD: Vec3 = Vec3::Z;

/// The facing policy each verb implies when the activity names none. `None`
/// writes no heading. A position goal faces travel only. An engaged path faces
/// travel while steering moves and the target once it stops; a committed aim
/// always faces the target.
pub(super) fn verb_facing_policy(
    steering: SteeringIntent,
    engaged: bool,
    committed_aim: bool,
    steering_moving: bool,
) -> Option<FacingPolicy> {
    match steering {
        SteeringIntent::MoveTo(_) => Some(FacingPolicy::Travel),
        _ if committed_aim => Some(FacingPolicy::Target),
        _ if engaged && steering_moving => Some(FacingPolicy::Travel),
        _ if engaged => Some(FacingPolicy::Target),
        _ => None,
    }
}

/// Resolve the one direction this tick's facing slews toward, for compute's
/// post-slew attack gate and apply's transform write alike.
///
/// `steering_velocity` excludes knockback. While launched (`impact_yaw` is
/// `Some`), travel facing holds the impact yaw; target facing keeps re-aiming
/// at the normal slew rate.
pub(super) fn facing_direction(
    policy: Option<FacingPolicy>,
    steering_velocity: Option<Vec3>,
    aim_direction: Option<Vec3>,
    impact_yaw: Option<f32>,
) -> Option<Vec3> {
    match (policy?, impact_yaw) {
        (FacingPolicy::Target, _) => aim_direction,
        (FacingPolicy::Travel, Some(yaw)) => Some(Quat::from_rotation_y(yaw) * MESH_FORWARD),
        (FacingPolicy::Travel, None) => {
            steering_velocity.filter(|velocity| LocomotionIntent::from_velocity(*velocity).moving)
        }
    }
}

/// A yaw-only rotation that aims the mesh's visual forward at a horizontal
/// direction. Returns `None` for negligible or NaN XZ headings so callers leave
/// the existing facing untouched.
pub(super) fn yaw_rotation_toward(dir: Vec3) -> Option<Quat> {
    const MIN_XZ_LEN_SQ: f32 = 1e-8;
    let len_xz_sq = dir.x * dir.x + dir.z * dir.z;
    if len_xz_sq.is_nan() || len_xz_sq <= MIN_XZ_LEN_SQ {
        return None;
    }

    let yaw = dir.x.atan2(dir.z) - MESH_FORWARD.x.atan2(MESH_FORWARD.z);
    Some(Quat::from_rotation_y(yaw))
}

pub(super) fn yaw_from_rotation(rotation: Quat) -> f32 {
    let heading = rotation * MESH_FORWARD;
    heading.x.atan2(heading.z)
}

/// Advance `current` yaw toward `target` by at most `max_delta` radians along
/// the shortest arc. Non-finite targets retain the current yaw; corrupt current
/// yaws reseat at a finite target rather than becoming absorbing.
pub(super) fn slew_yaw(current: f32, target: f32, max_delta: f32) -> f32 {
    if !target.is_finite() {
        return current;
    }
    if !current.is_finite() {
        return target;
    }
    let delta = (target - current + std::f32::consts::PI).rem_euclid(std::f32::consts::TAU)
        - std::f32::consts::PI;
    let max_delta = max_delta.max(0.0);
    if delta.abs() <= max_delta {
        target
    } else {
        current + delta.signum() * max_delta
    }
}

/// Resolve the yaw produced by this tick's ordinary facing slew toward
/// `direction`. The AI compute pass uses this same calculation for its fire
/// gate before the apply pass writes the resulting rotation.
pub(super) fn slewed_yaw_toward(rotation: Quat, direction: Vec3, max_delta: f32) -> Option<f32> {
    let target_yaw = yaw_from_rotation(yaw_rotation_toward(direction)?);
    Some(slew_yaw(yaw_from_rotation(rotation), target_yaw, max_delta))
}

/// Whether `yaw` aims within the attack tolerance of `direction`. A non-finite
/// heading fails closed so a malformed transform cannot land damage. A finite
/// vertical eye-to-target segment has no yaw to test, so it preserves melee
/// contact's established LOS-only behavior.
pub(super) fn yaw_within_attack_tolerance(yaw: f32, direction: Vec3) -> bool {
    if !yaw.is_finite() || !direction.is_finite() {
        return false;
    }
    const MIN_XZ_LEN_SQ: f32 = 1e-8;
    let len_xz_sq = direction.x * direction.x + direction.z * direction.z;
    if len_xz_sq <= MIN_XZ_LEN_SQ {
        return true;
    }
    let Some(target_rotation) = yaw_rotation_toward(direction) else {
        return false;
    };
    let target_yaw = yaw_from_rotation(target_rotation);
    let residual = (target_yaw - yaw + std::f32::consts::PI).rem_euclid(std::f32::consts::TAU)
        - std::f32::consts::PI;
    // `slew_yaw` can land mathematically exactly at the tolerance boundary
    // through several f32 operations. Keep that inclusive contract stable
    // across their final rounding step.
    residual.abs() <= ATTACK_FACING_TOLERANCE_RADIANS + ATTACK_FACING_COMPARISON_EPSILON
}
