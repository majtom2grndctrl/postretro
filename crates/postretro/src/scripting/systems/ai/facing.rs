use glam::{Quat, Vec3};

/// Maximum enemy-facing yaw rotation, in radians/sec. Higher than path steering
/// so visual facing catches up quickly without snapping.
pub(super) const FACING_TURN_RATE: f32 = crate::agent_steering::MAX_TURN_RATE * 2.0;

/// The reference enemy mesh's visual forward axis in model space. The skinned
/// glTF characters (`content/dev/models/reference_enemy_kaykit_knight`) are
/// authored facing `+Z`; the renderer applies `Transform.rotation` directly to
/// the model matrix, so aiming this axis at a target makes the mesh face it.
///
/// This is the opposite of the engine camera/view forward (`-Z`). Facing code
/// orients a rendered mesh, so it must aim the mesh's authored front (`+Z`).
const MESH_FORWARD: Vec3 = Vec3::Z;

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
