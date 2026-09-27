// Listener orientation math: forward/up primitives to a kira-convention
// quaternion, in plain f32 so no glam or quaternion type crosses the boundary.
// See: context/lib/audio.md §5

/// Build a kira-convention orientation quaternion `[x, y, z, w]` from a forward
/// and up vector, both world-space primitives. kira's unrotated listener faces
/// `-Z` with `+X` right and `+Y` up, so this constructs the rotation taking that
/// reference basis onto the camera basis derived from `forward`/`up`. Done with
/// plain f32 math so no glam/quaternion type crosses the module boundary.
///
/// Degenerate inputs (zero-length forward, or forward parallel to up) fall back
/// to identity rather than producing NaNs; full spatialization is not yet
/// implemented, so the listener stays anchored and oriented best-effort.
pub(crate) fn orientation_from_forward_up(forward: [f32; 3], up: [f32; 3]) -> [f32; 4] {
    // Camera basis: -Z = forward (look), +X = right, +Y = up.
    let f = normalize(forward);
    // `right = forward × up`; `up' = right × forward` re-orthogonalizes.
    let right = normalize(cross(f, up));
    // `normalize` returns its input unchanged when near-zero, so a zero-length
    // forward leaves `f` near-zero and this guard still catches it — the
    // post-normalize length check is valid for both the zero-forward and
    // forward-parallel-to-up (zero cross product) degenerate cases.
    if length(f) < f32::EPSILON || length(right) < f32::EPSILON {
        return [0.0, 0.0, 0.0, 1.0];
    }
    let u = cross(right, f);

    // Columns of the rotation matrix mapping reference axes to the camera basis:
    // +X -> right, +Y -> u, -Z -> f  (so +Z -> -f).
    let m00 = right[0];
    let m10 = right[1];
    let m20 = right[2];
    let m01 = u[0];
    let m11 = u[1];
    let m21 = u[2];
    let m02 = -f[0];
    let m12 = -f[1];
    let m22 = -f[2];

    // Standard matrix-to-quaternion conversion (Shepperd's method, trace case
    // plus the three diagonal-dominant cases for numerical stability).
    let trace = m00 + m11 + m22;
    let (x, y, z, w) = if trace > 0.0 {
        let s = (trace + 1.0).sqrt() * 2.0;
        ((m21 - m12) / s, (m02 - m20) / s, (m10 - m01) / s, 0.25 * s)
    } else if m00 > m11 && m00 > m22 {
        let s = (1.0 + m00 - m11 - m22).sqrt() * 2.0;
        (0.25 * s, (m01 + m10) / s, (m02 + m20) / s, (m21 - m12) / s)
    } else if m11 > m22 {
        let s = (1.0 + m11 - m00 - m22).sqrt() * 2.0;
        ((m01 + m10) / s, 0.25 * s, (m12 + m21) / s, (m02 - m20) / s)
    } else {
        let s = (1.0 + m22 - m00 - m11).sqrt() * 2.0;
        ((m02 + m20) / s, (m12 + m21) / s, 0.25 * s, (m10 - m01) / s)
    };
    [x, y, z, w]
}

fn cross(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [
        a[1] * b[2] - a[2] * b[1],
        a[2] * b[0] - a[0] * b[2],
        a[0] * b[1] - a[1] * b[0],
    ]
}

fn length(v: [f32; 3]) -> f32 {
    (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt()
}

fn normalize(v: [f32; 3]) -> [f32; 3] {
    let len = length(v);
    if len < f32::EPSILON {
        v
    } else {
        [v[0] / len, v[1] / len, v[2] / len]
    }
}
