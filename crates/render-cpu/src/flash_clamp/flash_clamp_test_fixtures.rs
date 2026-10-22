// An independent WCAG flash counter the flash-clamp tests judge presented
// series against, shared by the screen-channel, keyed-clamp and ghost-look tests.
// See: context/lib/testing_guide.md §4 · context/lib/rendering_pipeline.md §7.8

/// Count transitions of `series` (one value per frame at `dt`) in any one
/// second, WCAG-style, from the last extremum. A reversal ends an
/// excursion only once it reaches the 0.1 threshold itself.
pub(super) fn worst_transitions(series: &[f32], dt: f32) -> usize {
    most_in_any_second(&transition_times(series, dt))
}

/// Count transitions of every series in any one second together: what one
/// source presenting all of them at once shows.
pub(super) fn worst_transitions_across(series: &[&[f32]], dt: f32) -> usize {
    let mut times: Vec<f32> = series
        .iter()
        .flat_map(|series| transition_times(series, dt))
        .collect();
    times.sort_by(f32::total_cmp);
    most_in_any_second(&times)
}

/// When each transition of `series` is counted, in order.
fn transition_times(series: &[f32], dt: f32) -> Vec<f32> {
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
    times
}

/// The most of `times` (ascending) inside any one second.
fn most_in_any_second(times: &[f32]) -> usize {
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
