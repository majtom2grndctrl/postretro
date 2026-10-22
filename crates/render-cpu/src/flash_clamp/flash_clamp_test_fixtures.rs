// An independent WCAG flash counter the flash-clamp tests judge presented
// series against, shared by the screen-channel and keyed-clamp tests.
// See: context/lib/testing_guide.md §4 · context/lib/rendering_pipeline.md §7.8

/// Count transitions of `series` (one value per frame at `dt`) in any one
/// second, WCAG-style, from the last extremum. A reversal ends an
/// excursion only once it reaches the 0.1 threshold itself.
pub(super) fn worst_transitions(series: &[f32], dt: f32) -> usize {
    let mut times = Vec::new();
    let (mut ext, mut peak, mut dir, mut counted) = (series[0], series[0], 0.0f32, false);
    for (n, &v) in series.iter().enumerate().skip(1) {
        let beyond = (v - peak) * dir;
        if dir == 0.0 {
            if v != peak {
                dir = (v - peak).signum();
                peak = v;
            }
        } else if beyond > 0.0 {
            peak = v;
        } else if -beyond >= 0.1 {
            ext = peak;
            peak = v;
            dir = -dir;
            counted = false;
        }
        if !counted && (v - ext).abs() >= 0.1 && v.min(ext) < 0.8 {
            counted = true;
            times.push(n as f32 * dt);
        }
    }
    (0..times.len())
        .map(|i| {
            times[i..]
                .iter()
                .take_while(|&&t| t < times[i] + 1.0)
                .count()
        })
        .max()
        .unwrap_or(0)
}
