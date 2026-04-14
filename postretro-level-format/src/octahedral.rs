// Octahedral normal encoding: unit vector ↔ u16x2.
// See: context/lib/rendering_pipeline.md §6

/// Encode a unit-length direction vector to octahedral `[u16; 2]`.
///
/// Maps the unit sphere to a `[-1, 1]^2` octahedron unfolding, then quantizes
/// to `[0, 65535]`. The bitangent sign (for tangent vectors) is not stored here;
/// callers pack it separately.
pub fn encode(x: f32, y: f32, z: f32) -> [u16; 2] {
    // Project onto octahedron: divide by L1 norm
    let inv_l1 = 1.0 / (x.abs() + y.abs() + z.abs());
    let mut ox = x * inv_l1;
    let mut oy = y * inv_l1;

    // Reflect the lower hemisphere into the [−1,1]^2 square
    if z < 0.0 {
        let new_ox = (1.0 - oy.abs()) * sign_not_zero(ox);
        let new_oy = (1.0 - ox.abs()) * sign_not_zero(oy);
        ox = new_ox;
        oy = new_oy;
    }

    // Map [-1, 1] → [0, 65535]
    let u = ((ox * 0.5 + 0.5) * 65535.0).round() as u16;
    let v = ((oy * 0.5 + 0.5) * 65535.0).round() as u16;
    [u, v]
}

/// Decode an octahedral `[u16; 2]` back to a unit-length direction vector `[f32; 3]`.
pub fn decode(encoded: [u16; 2]) -> [f32; 3] {
    // Map [0, 65535] → [-1, 1]
    let ox = encoded[0] as f32 / 65535.0 * 2.0 - 1.0;
    let oy = encoded[1] as f32 / 65535.0 * 2.0 - 1.0;

    // Reconstruct z from the octahedron constraint |x| + |y| + |z| = 1
    let z = 1.0 - ox.abs() - oy.abs();
    let (x, y) = if z < 0.0 {
        // Lower hemisphere: undo the reflection
        let x = (1.0 - oy.abs()) * sign_not_zero(ox);
        let y = (1.0 - ox.abs()) * sign_not_zero(oy);
        (x, y)
    } else {
        (ox, oy)
    };

    // Normalize to correct for quantization error
    let len = (x * x + y * y + z * z).sqrt();
    [x / len, y / len, z / len]
}

/// Returns 1.0 for non-negative values, -1.0 for negative. Never returns zero.
fn sign_not_zero(v: f32) -> f32 {
    if v >= 0.0 {
        1.0
    } else {
        -1.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Maximum allowed angular error in radians between input and decoded output.
    const MAX_ANGULAR_ERROR: f32 = 0.001;

    fn angular_error(a: [f32; 3], b: [f32; 3]) -> f32 {
        let dot = (a[0] * b[0] + a[1] * b[1] + a[2] * b[2]).clamp(-1.0, 1.0);
        dot.acos()
    }

    fn normalize(v: [f32; 3]) -> [f32; 3] {
        let len = (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt();
        [v[0] / len, v[1] / len, v[2] / len]
    }

    fn assert_round_trip(dir: [f32; 3]) {
        let dir = normalize(dir);
        let encoded = encode(dir[0], dir[1], dir[2]);
        let decoded = decode(encoded);
        let err = angular_error(dir, decoded);
        assert!(
            err < MAX_ANGULAR_ERROR,
            "angular error {err:.6} rad exceeds threshold for direction {:?} \
             (encoded: {:?}, decoded: {:?})",
            dir,
            encoded,
            decoded,
        );
    }

    #[test]
    fn round_trips_positive_axes() {
        assert_round_trip([1.0, 0.0, 0.0]);
        assert_round_trip([0.0, 1.0, 0.0]);
        assert_round_trip([0.0, 0.0, 1.0]);
    }

    #[test]
    fn round_trips_negative_axes() {
        assert_round_trip([-1.0, 0.0, 0.0]);
        assert_round_trip([0.0, -1.0, 0.0]);
        assert_round_trip([0.0, 0.0, -1.0]);
    }

    #[test]
    fn round_trips_diagonals() {
        let d = 1.0 / 3.0_f32.sqrt();
        assert_round_trip([d, d, d]);
        assert_round_trip([-d, -d, -d]);
        assert_round_trip([d, -d, d]);
        assert_round_trip([-d, d, -d]);
    }

    #[test]
    fn round_trips_near_pole_vectors() {
        // Near +Z pole
        assert_round_trip([0.001, 0.001, 1.0]);
        assert_round_trip([-0.001, 0.001, 1.0]);
        // Near -Z pole
        assert_round_trip([0.001, 0.001, -1.0]);
        assert_round_trip([-0.001, -0.001, -1.0]);
        // Near +Y pole
        assert_round_trip([0.001, 1.0, 0.001]);
        // Near -X pole
        assert_round_trip([-1.0, 0.001, 0.001]);
    }

    #[test]
    fn round_trips_random_samples() {
        // Deterministic pseudo-random directions covering the sphere
        // Using a simple LCG for reproducibility
        let mut seed: u32 = 0xDEAD_BEEF;
        for _ in 0..1000 {
            seed = seed.wrapping_mul(1103515245).wrapping_add(12345);
            let x = (seed as f32 / u32::MAX as f32) * 2.0 - 1.0;
            seed = seed.wrapping_mul(1103515245).wrapping_add(12345);
            let y = (seed as f32 / u32::MAX as f32) * 2.0 - 1.0;
            seed = seed.wrapping_mul(1103515245).wrapping_add(12345);
            let z = (seed as f32 / u32::MAX as f32) * 2.0 - 1.0;

            let len = (x * x + y * y + z * z).sqrt();
            if len < 0.001 {
                continue; // Skip near-zero vectors
            }
            assert_round_trip([x, y, z]);
        }
    }

    #[test]
    fn encoded_values_use_expected_range() {
        // Verify axis vectors map to expected octahedral coordinates
        let dirs_and_expected: &[([f32; 3], [u16; 2])] = &[
            // +X: oct (1, 0) -> u16 (65535, 32768)
            ([1.0, 0.0, 0.0], [65535, 32768]),
            // -X: oct (-1, 0) -> u16 (0, 32768)
            ([-1.0, 0.0, 0.0], [0, 32768]),
            // +Y: oct (0, 1) -> u16 (32768, 65535)
            ([0.0, 1.0, 0.0], [32768, 65535]),
            // -Y: oct (0, -1) -> u16 (32768, 0)
            ([0.0, -1.0, 0.0], [32768, 0]),
            // +Z: oct (0, 0) -> u16 (32768, 32768)
            ([0.0, 0.0, 1.0], [32768, 32768]),
        ];
        for (dir, expected) in dirs_and_expected {
            let enc = encode(dir[0], dir[1], dir[2]);
            assert_eq!(enc, *expected, "direction {:?}", dir);
        }
    }

    #[test]
    fn decode_produces_unit_vectors() {
        let test_cases: &[[u16; 2]] = &[
            [0, 0],
            [65535, 65535],
            [32768, 32768],
            [0, 65535],
            [65535, 0],
            [16384, 49152],
        ];
        for &enc in test_cases {
            let dec = decode(enc);
            let len = (dec[0] * dec[0] + dec[1] * dec[1] + dec[2] * dec[2]).sqrt();
            assert!(
                (len - 1.0).abs() < 1e-5,
                "decoded vector {:?} from {:?} has length {}, expected 1.0",
                dec,
                enc,
                len,
            );
        }
    }
}
