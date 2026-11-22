// Integrand-weighted visibility for area-light shadow samples (ratio and one-sample estimators).
// See: context/lib/build_pipeline.md §Compiler pipeline (Area lights)

/// A projected solid angle at or below this has no light to share out.
const PROJECTED_SOLID_ANGLE_FLOOR: f64 = 1.0e-15;

/// Heitz–Hill–McGuire ratio estimator over solid-angle samples: Σ w·V / Σ w.
/// Exactly 0 when Σ w is 0 (no division), exactly 1 when every weighted sample
/// is visible and exactly 0 when none is. Negative or non-finite weights count
/// as 0, so the result always lies in [0, 1].
pub(crate) fn weighted_visibility(samples: impl IntoIterator<Item = (f64, bool)>) -> f64 {
    let mut visible = 0.0;
    let mut total = 0.0;
    for (weight, clear) in samples {
        let weight = if weight.is_finite() {
            weight.max(0.0)
        } else {
            0.0
        };
        total += weight;
        if clear {
            // Same additions in the same order as `total`, so all-visible
            // divides a sum by itself.
            visible += weight;
        }
    }
    if total > 0.0 {
        (visible / total).min(1.0)
    } else {
        0.0
    }
}

/// One-sample visibility for the animated bounce arm: w·V·Ω / ∫w dω, with Ω the
/// faced polygons' solid angle and ∫w dω the clipped polygon's projected solid
/// angle (its irradiance per unit radiance). Unbiased when the sample is
/// uniform in solid angle; 0 when the denominator is ~0.
pub(crate) fn one_sample_visibility(
    weight: f64,
    visible: bool,
    solid_angle: f64,
    projected_solid_angle: f64,
) -> f64 {
    if !visible || projected_solid_angle <= PROJECTED_SOLID_ANGLE_FLOOR {
        return 0.0;
    }
    let estimate = weight.max(0.0) * solid_angle / projected_solid_angle;
    if estimate.is_finite() { estimate } else { 0.0 }
}
