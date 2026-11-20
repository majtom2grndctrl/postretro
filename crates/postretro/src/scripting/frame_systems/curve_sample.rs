// Linear sampling of the scalar curves frame bridges animate on the script
// clock (fog volume channels, emissive volume brightness).
// See: context/lib/scripting.md §10.2

/// Linear interpolation across an N-sample curve, where samples are
/// uniformly spaced over `[0.0, 1.0]` and `t` is the phase in that range.
/// Deliberately simpler than the shader's Catmull-Rom path: each curve drives
/// one scalar per frame, and the visual difference at sample boundaries is
/// imperceptible at the cadence scripts animate these values at.
pub(crate) fn sample_linear_curve(curve: &[f32], t: f32) -> f32 {
    match curve.len() {
        // Installs reject empty curves, so no caller passes one. Return 0
        // rather than panicking from a hot path.
        0 => 0.0,
        1 => curve[0],
        n => {
            let pos = t * (n - 1) as f32;
            let lo_idx = (pos as usize).min(n - 1);
            let hi_idx = (lo_idx + 1).min(n - 1);
            let frac = pos - lo_idx as f32;
            let lo = curve[lo_idx];
            let hi = curve[hi_idx];
            lo + (hi - lo) * frac
        }
    }
}

#[cfg(test)]
mod tests {
    use super::sample_linear_curve;

    // Samples sit evenly over [0, 1]; between two the value interpolates
    // linearly; one sample is constant; an empty curve reads 0.
    #[test]
    fn samples_interpolate_linearly_over_the_unit_phase() {
        let curve = [0.0, 1.0, 0.5];
        assert_eq!(sample_linear_curve(&curve, 0.0), 0.0);
        assert_eq!(sample_linear_curve(&curve, 0.5), 1.0);
        assert_eq!(sample_linear_curve(&curve, 1.0), 0.5);
        assert!((sample_linear_curve(&curve, 0.25) - 0.5).abs() < 1e-6);
        assert!((sample_linear_curve(&curve, 0.75) - 0.75).abs() < 1e-6);
        assert_eq!(sample_linear_curve(&[0.4], 0.7), 0.4);
        assert_eq!(sample_linear_curve(&[], 0.3), 0.0);
    }
}
