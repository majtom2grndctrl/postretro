// An independent WCAG flash counter the flash-clamp tests judge presented
// series against, shared by the screen-channel, keyed-clamp and ghost-look
// tests, and what a ghost draws over a dense grid of albedos and backgrounds.
// See: context/lib/testing_guide.md §4 · context/lib/rendering_pipeline.md §7.8

/// Count transitions of `series` (one value per frame at `dt`) in any one
/// second, WCAG-style, from the last extremum. A reversal ends an
/// excursion only once it reaches the 0.1 threshold itself.
pub(super) fn worst_transitions(series: &[f32], dt: f32) -> usize {
    let times: Vec<f32> = (0..series.len()).map(|n| n as f32 * dt).collect();
    most_in_any_second(&transition_times(series, &times))
}

/// Steps of the albedo and background grid a ghost's drawn value is counted
/// over: 0, 1/40, …, 1 on each axis. Denser than the clamp's own edge
/// samples, and off them, so a leak between samples shows.
const GRID_STEPS: usize = 40;

/// Every (albedo, background) of the grid.
fn grid() -> impl Iterator<Item = (f32, f32)> {
    let step = |k: usize| k as f32 / GRID_STEPS as f32;
    (0..=GRID_STEPS).flat_map(move |a| (0..=GRID_STEPS).map(move |b| (step(a), step(b))))
}

/// What a ghost draws, frame by frame, with `albedo` over `background`: at
/// presented (drawn) opacity o and drawn tint luminance L, B(1 − o) + a·o·L.
fn drawn_over(albedo: f32, background: f32, opacity: &[f32], tint_luminance: &[f32]) -> Vec<f32> {
    opacity
        .iter()
        .zip(tint_luminance)
        .map(|(level, luminance)| background * (1.0 - level) + albedo * level * luminance)
        .collect()
}

/// Count transitions of what a ghost draws in any one second with `albedo`
/// over `background`, one frame per `dt`.
pub(super) fn transitions_over(
    albedo: f32,
    background: f32,
    opacity: &[f32],
    tint_luminance: &[f32],
    dt: f32,
) -> usize {
    worst_transitions(&drawn_over(albedo, background, opacity, tint_luminance), dt)
}

/// Count transitions of what a ghost draws in any one second at every albedo
/// and background of the grid, and return the most: the WCAG counter (a swing
/// of 0.1 with its darker state under 0.8) on the drawn value, one frame per
/// `dt`. `opacity` is what draws: the screen door's coverage when dithered.
pub(super) fn worst_transitions_over_any_background(
    opacity: &[f32],
    tint_luminance: &[f32],
    dt: f32,
) -> usize {
    let times: Vec<f32> = (0..opacity.len()).map(|n| n as f32 * dt).collect();
    worst_transitions_over_any_background_at(opacity, tint_luminance, &times)
}

/// [`worst_transitions_over_any_background`] for frames presented at `times`
/// (seconds, ascending), so hitches and skipped frames count as they elapsed.
pub(super) fn worst_transitions_over_any_background_at(
    opacity: &[f32],
    tint_luminance: &[f32],
    times: &[f32],
) -> usize {
    grid()
        .map(|(albedo, background)| {
            let drawn = drawn_over(albedo, background, opacity, tint_luminance);
            most_in_any_second(&transition_times(&drawn, times))
        })
        .max()
        .unwrap_or(0)
}

/// When each transition of `series` is counted, frame `n` presenting at
/// `times[n]`.
fn transition_times(series: &[f32], times: &[f32]) -> Vec<f32> {
    let mut counted_at = Vec::new();
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
            counted_at.push(times[n]);
        }
    }
    counted_at
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
