// Deterministic pellet-cone sampling and per-shell RNG seeding for weapon resolution.
// See: context/plans/in-progress/E16--shotgun-pellet-spread/index.md (Task 2)

use glam::Vec3;

use crate::trigger_pools::SplitMix64;

/// Sample a unit direction uniformly over the solid angle of a cone around
/// `axis`. `u1` and `u2` are independent uniforms in `[0, 1)`.
///
/// A zero axis falls back to `Vec3::Y`. At zero spread, a finite, non-zero
/// axis is returned byte-for-byte unchanged so legacy straight-through casts
/// retain their exact direction.
pub(crate) fn sample_cone_direction(axis: Vec3, half_angle_rad: f32, u1: f32, u2: f32) -> Vec3 {
    let half_angle_rad = half_angle_rad.max(0.0);
    if half_angle_rad <= f32::EPSILON && axis.is_finite() && axis != Vec3::ZERO {
        return axis;
    }

    let axis = axis.normalize_or_zero();
    let axis = if axis == Vec3::ZERO { Vec3::Y } else { axis };
    if half_angle_rad <= f32::EPSILON {
        return axis;
    }

    // `1 - u · (1 - cos α)` is the inverse CDF for uniform solid angle.
    let cos_theta = 1.0 - u1 * (1.0 - half_angle_rad.cos());
    let sin_theta = (1.0 - cos_theta * cos_theta).max(0.0).sqrt();
    let azimuth = u2 * std::f32::consts::TAU;

    // Pick a helper that cannot be nearly parallel to the axis, then build an
    // orthonormal frame in which the cone direction is expressed.
    let helper = if axis.x.abs() < 0.9 { Vec3::X } else { Vec3::Y };
    let tangent = axis.cross(helper).normalize_or_zero();
    let bitangent = axis.cross(tangent);
    let direction = tangent * (sin_theta * azimuth.cos())
        + bitangent * (sin_theta * azimuth.sin())
        + axis * cos_theta;

    direction.normalize_or_zero()
}

/// Deterministic pellet-direction stream for one resolved shell.
#[derive(Debug, Clone)]
pub(crate) struct PelletRng {
    mixer: SplitMix64,
}

impl PelletRng {
    pub(crate) fn new(seed: u64) -> Self {
        Self {
            mixer: SplitMix64::new(seed),
        }
    }

    /// Return the next uniform in `[0, 1)` from the high 24 mixer bits.
    pub(crate) fn next_f32(&mut self) -> f32 {
        (self.mixer.next_u64() >> 40) as f32 / (1 << 24) as f32
    }
}

/// Derive a replay- and spawn-order-stable seed for one weapon shell.
///
/// The source name is folded byte-by-byte through SplitMix64 instead of a
/// process-randomized hasher. Callers pass `"weapon.unknown"` when neither
/// descriptor provenance nor an authored credit source supplies a name.
pub(crate) fn pellet_rng_seed(shell_counter: u32, salt_name: &str, slot: usize) -> u64 {
    let mut seed = mix_seed(0, u64::from(shell_counter));
    for byte in salt_name.bytes() {
        seed = mix_seed(seed, u64::from(byte));
    }
    seed = mix_seed(seed, salt_name.len() as u64);
    mix_seed(seed, slot as u64)
}

fn mix_seed(seed: u64, value: u64) -> u64 {
    SplitMix64::new(seed ^ value).next_u64()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn assert_vec3_bits_eq(actual: Vec3, expected: Vec3) {
        assert_eq!(actual.x.to_bits(), expected.x.to_bits());
        assert_eq!(actual.y.to_bits(), expected.y.to_bits());
        assert_eq!(actual.z.to_bits(), expected.z.to_bits());
    }

    #[test]
    fn sample_cone_direction_preserves_finite_axis_exactly_at_zero_spread() {
        let axis = Vec3::new(0.0, 1.0, -0.0);

        assert_vec3_bits_eq(sample_cone_direction(axis, 0.0, 0.4, 0.7), axis);
        assert_vec3_bits_eq(sample_cone_direction(axis, -1.0, 0.4, 0.7), axis);
    }

    #[test]
    fn sample_cone_direction_returns_unit_length() {
        let direction = sample_cone_direction(Vec3::new(2.0, -3.0, 4.0), 0.6, 0.25, 0.75);

        assert!((direction.length() - 1.0).abs() < 1e-5);
    }

    #[test]
    fn sample_cone_direction_falls_back_to_up_for_zero_axis() {
        let direction = sample_cone_direction(Vec3::ZERO, 0.6, 0.25, 0.75);

        assert!((direction.length() - 1.0).abs() < 1e-5);
        assert!(direction.dot(Vec3::Y) >= 0.6_f32.cos() - 1e-5);
        assert_eq!(sample_cone_direction(Vec3::ZERO, 0.0, 0.25, 0.75), Vec3::Y);
    }

    #[test]
    fn pellet_rng_seed_is_deterministic_and_distinguishes_shells_and_salts() {
        let seed = pellet_rng_seed(7, "weapon.shotgun", 2);
        let mut first = PelletRng::new(seed);
        let mut second = PelletRng::new(seed);
        let first_sequence: Vec<f32> = (0..8).map(|_| first.next_f32()).collect();
        let second_sequence: Vec<f32> = (0..8).map(|_| second.next_f32()).collect();

        assert_eq!(first_sequence, second_sequence);
        assert_ne!(
            pellet_rng_seed(8, "weapon.shotgun", 2),
            pellet_rng_seed(7, "weapon.shotgun", 2)
        );
        assert_ne!(
            pellet_rng_seed(7, "weapon.rifle", 2),
            pellet_rng_seed(7, "weapon.shotgun", 2)
        );
        assert_ne!(
            pellet_rng_seed(7, "weapon.shotgun", 3),
            pellet_rng_seed(7, "weapon.shotgun", 2)
        );
    }

    #[test]
    fn sample_cone_direction_distributes_uniformly_within_cone() {
        let axis = Vec3::Y;
        let half_angle = std::f32::consts::FRAC_PI_4;
        let mut rng = PelletRng::new(pellet_rng_seed(0, "weapon.shotgun", 0));
        let mut sum = Vec3::ZERO;

        for _ in 0..1_000 {
            let direction = sample_cone_direction(axis, half_angle, rng.next_f32(), rng.next_f32());
            assert!(direction.dot(axis) >= half_angle.cos() - 1e-5);
            sum += direction;
        }

        let mean = sum / 1_000.0;
        let expected_mean_length = (1.0 + half_angle.cos()) * 0.5;
        assert!((mean.length() - expected_mean_length).abs() < 0.04);
        assert!(mean.normalize().dot(axis) > 0.99);
    }
}
