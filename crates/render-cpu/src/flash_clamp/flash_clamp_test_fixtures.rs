// An independent WCAG flash counter the flash-clamp tests judge presented
// series against, shared by the screen-channel, keyed-clamp and ghost-look tests,
// and a ghost's drawn value over the backgrounds that bound any other.
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

/// Count transitions of a ghost's drawn brightness in any one second: each
/// frame's presented opacity times its drawn tint luminance, judged as one
/// series. Over black that product is what the ghost draws, so a strobe that
/// moves opacity and tint together counts on the product, not on either part.
pub(super) fn worst_brightness_transitions(
    opacity: &[f32],
    tint_luminance: &[f32],
    dt: f32,
) -> usize {
    let brightness: Vec<f32> = opacity
        .iter()
        .zip(tint_luminance)
        .map(|(level, luminance)| level * luminance)
        .collect();
    worst_transitions(&brightness, dt)
}

/// Backgrounds and albedos a ghost's drawn value is counted over: a white ghost
/// over black, a black ghost over white and a white ghost over white (the
/// corners where its change is largest), then a white and a black ghost over
/// mid grey.
const BACKGROUNDS: [(f32, f32); 5] = [(0.0, 1.0), (1.0, 0.0), (1.0, 1.0), (0.5, 1.0), (0.5, 0.0)];

/// Count transitions of what a ghost draws in any one second, over each of
/// [`BACKGROUNDS`], and return the most: each frame's background B and albedo
/// a draw B(1 − o) + a·o·L at presented opacity o and drawn tint luminance L.
pub(super) fn worst_transitions_over_any_background(
    opacity: &[f32],
    tint_luminance: &[f32],
    dt: f32,
) -> usize {
    BACKGROUNDS
        .iter()
        .map(|&(background, albedo)| {
            let drawn: Vec<f32> = opacity
                .iter()
                .zip(tint_luminance)
                .map(|(level, luminance)| background * (1.0 - level) + albedo * level * luminance)
                .collect();
            worst_transitions(&drawn, dt)
        })
        .max()
        .unwrap_or(0)
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
