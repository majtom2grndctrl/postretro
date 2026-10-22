// Ghost look clamp: opacity, brightness and darkening judged at the worst
// background on one window, one change counted once across frames, dithered
// coverage, hue kept over budget, black tints held as grey, tints judged as
// drawn, vanished ghosts, frames that never present, and pass-through when off.
// See: context/lib/rendering_pipeline.md §7.8 (Photosensitivity limiter), §7.9

use super::super::flash_clamp_test_fixtures::{
    worst_brightness_transitions, worst_transitions, worst_transitions_across,
    worst_transitions_over_any_background,
};
use super::super::ghost_tint::{drawn_brightness, on_grid, presented_tint, tint_at_luminance};
use super::super::value::{FLASH_DARK_LIMIT, INTENSITY_RATE_PER_SECOND, luminance};
use super::*;
use crate::flash_limiter::{HITCH_CEILING_SECONDS, LimiterFrameInput, pack_limiter_frame};
use crate::ghost_instance::drawn_tint;

/// A ghost identity: link and generation-checked entity.
type Key = (u16, u32);

const GHOST: Key = (0, 7);
const WHITE: [f32; 3] = [1.0; 3];
const BLACK: [f32; 3] = [0.0; 3];

fn frame(dt: f32, enabled: bool) -> LimiterFrameUniform {
    pack_limiter_frame(
        LimiterFrameInput {
            elapsed_seconds: dt,
        },
        enabled,
        false,
    )
}

fn look(opacity: f32, tint: [f32; 3]) -> GhostInstanceLook {
    GhostInstanceLook {
        opacity,
        saturation: 0.5,
        tint,
    }
}

/// `a` for the first half of each period at `hz`, then `b`.
fn alternate(hz: f32, t: f32, a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    if (t * hz).fract() < 0.5 { a } else { b }
}

/// Present `tints` at steady opacity 1 through one ghost, one per frame at `fps`.
fn present_tints(tints: &[[f32; 3]], fps: f32, limiter: Option<bool>) -> Vec<GhostInstanceLook> {
    let mut clamp = GhostLookClamp::default();
    let frame = limiter.map(|enabled| frame(1.0 / fps, enabled));
    tints
        .iter()
        .map(|&tint| {
            clamp.begin_frame(frame.as_ref());
            let presented = clamp.present(GHOST, look(1.0, tint));
            clamp.end_frame();
            presented
        })
        .collect()
}

fn luminances(looks: &[GhostInstanceLook]) -> Vec<f32> {
    looks.iter().map(|look| luminance(look.tint)).collect()
}

/// Each look's tint luminance as drawn: on the 8-bit tint the shader decodes.
fn drawn_luminances(looks: &[GhostInstanceLook]) -> Vec<f32> {
    looks
        .iter()
        .map(|look| luminance(drawn_tint(look.tint)))
        .collect()
}

fn opacities(looks: &[GhostInstanceLook]) -> Vec<f32> {
    looks.iter().map(|look| look.opacity).collect()
}

/// Each look's drawn brightness: what it draws over black.
fn brightnesses(looks: &[GhostInstanceLook]) -> Vec<f32> {
    looks
        .iter()
        .map(|look| drawn_brightness(look.opacity, look.tint))
        .collect()
}

// AC 24a (provisional amendment): with the limiter on and opacity steady, a tint
// alternating black and white at 10 Hz presents at most three flashes in any
// second over any background. Opacity and saturation are untouched throughout.
// Over budget it holds where the first refused onset left it. At 60 Hz that is
// the brightness onset, so it rests at its pre-flash luminance (black, before
// the first white onset). At 240 Hz the darkening onset (white toward black
// over a white wall) is refused first and it rests near white instead, with no
// flash over any background.
#[test]
fn a_10hz_white_black_tint_strobe_presents_at_most_three_flashes_and_rests_at_pre_flash_luminance()
{
    for fps in [60.0f32, 144.0, 240.0] {
        let dt = 1.0 / fps;
        let tints: Vec<_> = (0..(3.0 * fps) as usize)
            .map(|n| alternate(10.0, n as f32 * dt, BLACK, WHITE))
            .collect();
        let raw = luminances(&look_series(&tints));
        assert!(worst_transitions(&raw, dt) >= 19, "{fps} Hz: raw strobe");

        let presented = present_tints(&tints, fps, Some(true));
        let series = luminances(&presented);
        let worst = worst_transitions(&series, dt);
        assert!((1..=6).contains(&worst), "{fps} Hz: {worst} transitions");
        let over_any = worst_transitions_over_any_background(
            &opacities(&presented),
            &drawn_luminances(&presented),
            dt,
        );
        assert!(over_any <= 6, "{fps} Hz: {over_any} transitions");
        assert!(presented.iter().all(|look| look.opacity == 1.0));
        assert!(presented.iter().all(|look| look.saturation == 0.5));
        if fps > 60.0 {
            continue;
        }

        // The last second asks for white half the time; over budget the tint
        // holds black on most of those frames.
        let tail = (2.0 * fps) as usize;
        let asked_white = tints[tail..].iter().filter(|&&t| t == WHITE).count();
        let held_dark = tints[tail..]
            .iter()
            .zip(&series[tail..])
            .filter(|&(&tint, &shown)| tint == WHITE && shown <= 0.05)
            .count();
        assert!(
            held_dark * 2 > asked_white,
            "{fps} Hz: {held_dark} of {asked_white} white frames held dark"
        );
    }
}

fn look_series(tints: &[[f32; 3]]) -> Vec<GhostInstanceLook> {
    tints.iter().map(|&tint| look(1.0, tint)).collect()
}

// AC 24a: the same strobe starting from white is held to the same budget; a
// black tint held above its luminance presents as a grey, the only colour with
// no hue to lose.
#[test]
fn a_white_first_tint_strobe_is_limited_and_holds_black_frames_as_grey() {
    let fps = 60.0;
    let dt = 1.0 / fps;
    let tints: Vec<_> = (0..180)
        .map(|n| alternate(10.0, n as f32 * dt, WHITE, BLACK))
        .collect();
    let presented = present_tints(&tints, fps, Some(true));
    let worst = worst_transitions(&luminances(&presented), dt);
    assert!(
        worst <= 6,
        "{worst} transitions: {:?}",
        luminances(&presented)
    );
    let over_any =
        worst_transitions_over_any_background(&opacities(&presented), &luminances(&presented), dt);
    assert!(over_any <= 6, "{over_any} transitions");
    for (tint, shown) in tints.iter().zip(&presented) {
        if *tint == BLACK && luminance(shown.tint) > 0.0 {
            let [r, g, b] = shown.tint;
            assert!(r == g && g == b, "a held black tint is grey: {shown:?}");
        }
    }
}

// AC 24a: over budget the tint keeps its hue: every presented colour is the
// asked colour scaled, within the 8-bit step a held tint draws on, never
// shifted toward another hue.
#[test]
fn a_held_tint_luminance_keeps_the_tints_hue() {
    let fps = 60.0;
    let dt = 1.0 / fps;
    let dim = [0.06, 0.08, 0.1];
    let bright = [0.6, 0.8, 1.0];
    let tints: Vec<_> = (0..180)
        .map(|n| alternate(10.0, n as f32 * dt, dim, bright))
        .collect();
    let presented = present_tints(&tints, fps, Some(true));
    let step = 1.0 / 255.0 + 1e-6;
    let mut held = 0;
    for (tint, shown) in tints.iter().zip(&presented) {
        // Some scale of the asked colour lies within a step of every channel.
        let lowest = (0..3)
            .map(|channel| (shown.tint[channel] - step) / tint[channel])
            .fold(f32::MIN, f32::max);
        let highest = (0..3)
            .map(|channel| (shown.tint[channel] + step) / tint[channel])
            .fold(f32::MAX, f32::min);
        assert!(lowest <= highest, "{shown:?} is not a scale of {tint:?}");
        if (shown.tint[2] / tint[2] - 1.0).abs() > 0.05 {
            held += 1;
        }
    }
    assert!(held > 0, "the strobe goes over budget");
}

// AC 24a: a single tint change passes: it reaches its value within the rate
// cap's 250 ms and stays there.
#[test]
fn a_single_tint_change_reaches_its_value_within_the_rate_cap() {
    let fps = 60.0;
    let blue = [0.2, 0.4, 1.0];
    for (from, to) in [(WHITE, BLACK), (BLACK, WHITE), (WHITE, blue)] {
        let tints: Vec<_> = (0..90).map(|n| if n < 30 { from } else { to }).collect();
        let presented = present_tints(&tints, fps, Some(true));
        // 250 ms after the change, at 60 Hz: 15 frames.
        for shown in &presented[45..] {
            for channel in 0..3 {
                assert!(
                    (shown.tint[channel] - to[channel]).abs() < 1e-6,
                    "{from:?} -> {to:?}: {shown:?}"
                );
            }
        }
    }
}

// The limiter turned off by the player, and a capture (no limiter input), pass
// every look through unchanged.
#[test]
fn limiter_off_and_capture_pass_the_tint_strobe_through() {
    let fps = 60.0;
    let dt = 1.0 / fps;
    let tints: Vec<_> = (0..120)
        .map(|n| alternate(10.0, n as f32 * dt, BLACK, WHITE))
        .collect();
    for limiter in [Some(false), None] {
        let presented = present_tints(&tints, fps, limiter);
        for (tint, shown) in tints.iter().zip(&presented) {
            assert_eq!(*shown, look(1.0, *tint), "{limiter:?}");
        }
    }
}

// An opacity strobe at a steady tint spends the window but changes no tint, and
// a second ghost's history is its own.
#[test]
fn an_opacity_strobe_leaves_a_steady_tint_alone_and_each_ghost_keeps_its_own_history() {
    let fps = 60.0;
    let dt = 1.0 / fps;
    let frame = frame(dt, true);
    let other: Key = (1, 7);
    let mut clamp = GhostLookClamp::default();
    for n in 0..180 {
        let on = alternate(10.0, n as f32 * dt, BLACK, WHITE) == WHITE;
        clamp.begin_frame(Some(&frame));
        let opacity_strobe = clamp.present(GHOST, look(if on { 1.0 } else { 0.0 }, WHITE));
        let steady = clamp.present(other, look(0.5, [0.6, 0.8, 1.0]));
        clamp.end_frame();
        assert_eq!(opacity_strobe.tint, WHITE, "frame {n}");
        assert_eq!(steady, look(0.5, [0.6, 0.8, 1.0]), "frame {n}");
    }
}

#[test]
fn tint_at_luminance_scales_and_greys_black() {
    let tint = [0.5, 0.25, 0.1];
    let half = tint_at_luminance(tint, luminance(tint) * 0.5);
    for channel in 0..3 {
        assert!((half[channel] - tint[channel] * 0.5).abs() < 1e-6);
    }
    let grey = tint_at_luminance(BLACK, 0.3);
    assert!(grey[0] == grey[1] && grey[1] == grey[2] && (grey[0] - 0.3).abs() < 1e-6);
    assert_eq!(tint_at_luminance(tint, luminance(tint)), tint);
}

/// Each component's share of the spread from the darkest component to the
/// brightest: what scaling and mixing toward white both keep.
fn hue_shares(rgb: [f32; 3]) -> [f32; 3] {
    let max = rgb[0].max(rgb[1]).max(rgb[2]);
    let min = rgb[0].min(rgb[1]).min(rgb[2]);
    rgb.map(|channel| (channel - min) / (max - min))
}

// Regression: scaling a saturated tint clipped its components at 1 short of the
// held luminance, which the clamp still recorded as presented.
#[test]
fn tint_at_luminance_draws_exactly_the_held_luminance_for_a_saturated_hue() {
    for tint in [
        [0.2, 0.4, 1.0],
        [1.0, 0.4, 0.3],
        [1.0, 0.0, 0.0],
        [0.05, 0.1, 0.4],
    ] {
        for held in [0.1, 0.45, 0.6, 0.93, 1.0] {
            let drawn = tint_at_luminance(tint, held);
            assert!(
                (luminance(drawn) - held).abs() < 1e-6,
                "{tint:?} held at {held}: {drawn:?}"
            );
            assert!(
                drawn.iter().all(|channel| (0.0..=1.0).contains(channel)),
                "{tint:?} held at {held}: {drawn:?}"
            );
            let spread =
                drawn[0].max(drawn[1]).max(drawn[2]) - drawn[0].min(drawn[1]).min(drawn[2]);
            if spread > 1e-2 {
                let (asked, shown) = (hue_shares(tint), hue_shares(drawn));
                for channel in 0..3 {
                    assert!(
                        (asked[channel] - shown[channel]).abs() < 1e-3,
                        "{tint:?} held at {held} changes hue: {drawn:?}"
                    );
                }
            }
        }
    }
}

// Regression: a white/blue strobe drew blue at its own luminance while the
// channel held a brighter one, so on screen it escaped both the rate cap and
// the flash window. Judged here on the luminance actually drawn: a held or
// slowed tint draws on the 8-bit grid, within a step of what the clamp holds.
#[test]
fn a_10hz_white_blue_tint_strobe_holds_the_rate_cap_and_flash_window_on_the_drawn_luminance() {
    let fps = 60.0;
    let dt = 1.0 / fps;
    let blue = [0.2, 0.4, 1.0];
    let tints: Vec<_> = (0..(3.0 * fps) as usize)
        .map(|n| alternate(10.0, n as f32 * dt, WHITE, blue))
        .collect();
    let raw = luminances(&look_series(&tints));
    assert!(worst_transitions(&raw, dt) > 6, "raw strobe");

    let drawn = drawn_luminances(&present_tints(&tints, fps, Some(true)));
    let cap = INTENSITY_RATE_PER_SECOND * dt;
    let grid = 2.0 / 255.0;
    for (n, pair) in drawn.windows(2).enumerate() {
        assert!(
            (pair[1] - pair[0]).abs() <= cap + grid,
            "frame {}: drawn luminance {} -> {}",
            n + 1,
            pair[0],
            pair[1]
        );
    }
    let worst = worst_transitions(&drawn, dt);
    assert!(worst <= 6, "{worst} transitions: {drawn:?}");
}

/// Tints the dev map's ghost styles use: `echo`'s blue and the colour filter's
/// red, neither near white.
const STYLE_TINTS: [[f32; 3]; 2] = [[0.6, 0.8, 1.0], [1.0, 0.4, 0.3]];

// Regression: a vanished ghost counted as its tint going black, so quick
// re-entries rate-capped the tint up from black and, once its window filled,
// held the ghost near black. Only its opacity falls while it is gone. Later,
// its brightness crossed the threshold frames after its opacity at 144 Hz and
// above, missed the opacity's slot, and was held: the tint darkened instead.
#[test]
fn a_ghost_reentering_at_10hz_never_presents_a_dark_tint() {
    for fps in [60.0f32, 144.0, 240.0] {
        for tint in STYLE_TINTS {
            let dt = 1.0 / fps;
            let step = frame(dt, true);
            let mut clamp = GhostLookClamp::default();
            let mut presented = 0;
            for n in 0..(3.0 * fps) as usize {
                // Armed for the first half of each 10 Hz period; disarmed, the
                // ghost leaves the member list and presents nothing.
                let armed = (n as f32 * dt * 10.0).fract() < 0.5;
                clamp.begin_frame(Some(&step));
                if armed {
                    let shown = clamp.present(GHOST, look(0.45, tint));
                    assert_eq!(shown.tint, tint, "{fps} fps {tint:?} frame {n}");
                    presented += 1;
                }
                clamp.end_frame();
            }
            assert!(presented > 0);

            // The held tint goes with the opacity history: a ghost back after
            // its window emptied starts fresh against its own tint.
            for _ in 0..(1.5 * fps) as usize {
                clamp.begin_frame(Some(&step));
                clamp.end_frame();
            }
            clamp.begin_frame(Some(&step));
            let back = clamp.present(GHOST, look(0.45, BLACK));
            clamp.end_frame();
            assert_eq!(back.tint, BLACK);
        }
    }
}

// Regression: a frame whose surface acquire failed still spent the ghost
// clamp's presented time, so the first visible frame after a run of them
// stepped several rate caps at once.
#[test]
fn a_frame_that_never_presents_leaves_the_clamp_as_if_it_never_ran_and_merges_its_time() {
    let dt = 1.0 / 60.0;
    let step = frame(dt, true);
    let start = look(0.0, BLACK);
    let target = look(1.0, WHITE);
    let mut skipping = GhostLookClamp::default();
    let mut reference = GhostLookClamp::default();
    for clamp in [&mut skipping, &mut reference] {
        clamp.begin_frame(Some(&step));
        clamp.present(GHOST, start);
        clamp.end_frame();
        clamp.finish_presentation(true);
    }
    // Five frames that never present, then one that does. The reference never
    // saw the five: its next frame carries their time.
    let mut merged = dt;
    for _ in 0..5 {
        skipping.begin_frame(Some(&step));
        skipping.present(GHOST, target);
        skipping.end_frame();
        skipping.finish_presentation(false);
        merged += dt;
    }
    for n in 0..30 {
        let reference_frame = if n == 0 { frame(merged, true) } else { step };
        skipping.begin_frame(Some(&step));
        let shown = skipping.present(GHOST, target);
        skipping.end_frame();
        skipping.finish_presentation(true);
        reference.begin_frame(Some(&reference_frame));
        let expected = reference.present(GHOST, target);
        reference.end_frame();
        reference.finish_presentation(true);

        assert!((shown.opacity - expected.opacity).abs() < 1e-6, "frame {n}");
        for channel in 0..3 {
            assert!(
                (shown.tint[channel] - expected.tint[channel]).abs() < 1e-6,
                "frame {n}: {shown:?} vs {expected:?}"
            );
        }
        if n == 0 {
            // Six frames' time, hitch-clamped: one hitch ceiling of rate cap.
            let cap = INTENSITY_RATE_PER_SECOND * HITCH_CEILING_SECONDS;
            assert!((shown.opacity - cap).abs() < 1e-5, "{shown:?}");
        }
    }
}

/// Presented looks per frame, one ghost at `fps` with the limiter on, from
/// `asked(n)`.
fn present_looks(
    frames: usize,
    fps: f32,
    asked: impl Fn(usize) -> GhostInstanceLook,
) -> Vec<GhostInstanceLook> {
    present_looks_drawn(frames, fps, GhostTransparency::Blend, asked)
}

/// 0 ↔ 1 opacity at 10 Hz, starting dark, with a steady tint.
fn opacity_strobe(n: usize, dt: f32, tint: [f32; 3]) -> GhostInstanceLook {
    let on = alternate(10.0, n as f32 * dt, BLACK, WHITE) == WHITE;
    look(if on { 1.0 } else { 0.0 }, tint)
}

// One ghost's opacity, tint redness and drawn brightness share one flash
// window: a tint strobe at full opacity, then an opacity strobe of the ghost
// tinted black, present at most three flashes in any second in total, though
// each alone would fit three of its own.
#[test]
fn a_tint_strobe_then_an_opacity_strobe_present_at_most_three_flashes_in_total() {
    for fps in [60.0f32, 240.0] {
        let dt = 1.0 / fps;
        // For the first half second the tint strobes black and white at 5 Hz;
        // then the ghost turns black and its opacity strobes.
        let asked = |n: usize| {
            let t = n as f32 * dt;
            let on = (t * 5.0).fract() >= 0.5;
            if t < 0.5 {
                look(1.0, if on { WHITE } else { BLACK })
            } else {
                look(if on { 0.0 } else { 1.0 }, BLACK)
            }
        };
        let frames = (3.0 * fps) as usize;
        let raw: Vec<_> = (0..frames).map(&asked).collect();
        let (raw_opacity, raw_brightness) = (opacities(&raw), brightnesses(&raw));
        assert!(
            worst_transitions_across(&[raw_opacity.as_slice(), raw_brightness.as_slice()], dt) > 6,
            "{fps} Hz: raw strobe"
        );

        let shown = present_looks(frames, fps, asked);
        let (opacity, brightness) = (opacities(&shown), brightnesses(&shown));
        let worst = worst_transitions_across(&[opacity.as_slice(), brightness.as_slice()], dt);
        assert!(worst <= 6, "{fps} Hz: {worst} transitions in total");
        assert!(
            worst_transitions(&opacity, dt) > 0 && worst_transitions(&brightness, dt) > 0,
            "{fps} Hz: both parts present some flash"
        );
    }
}

// Regression: an opacity onset that takes the window's last room owes its
// return that room, so a brightness onset after it holds dark until the
// window drains; admitted, it would have left the return no room. The
// brightness onset comes while the opacity falls back, so it can't join the
// opacity's onset as part of one change.
#[test]
fn a_brightness_onset_waits_for_the_return_an_opacity_onset_still_owes() {
    let fps = 60.0;
    let dt = 1.0 / fps;
    // Two quick opacity pulses (3 frames up, 3 down), a third rise held to
    // frame 32, then opacity falls to half. The tint turns white on frame 34,
    // as the opacity falls, and stays white.
    let asked = |n: usize| {
        let opacity = match n {
            0 => 0.0,
            1..=12 if (n - 1) % 6 >= 3 => 0.0,
            1..=32 => 1.0,
            _ => 0.5,
        };
        look(opacity, if n >= 34 { WHITE } else { BLACK })
    };
    let shown = present_looks(150, fps, asked);
    let (opacity, brightness) = (opacities(&shown), brightnesses(&shown));

    // The ghost draws dark while the window has no room for the brightness
    // onset and its return beside what it holds.
    let returned = 33 + (0.5 / (INTENSITY_RATE_PER_SECOND * dt)).ceil() as usize;
    assert_eq!(opacity[returned], 0.5, "the opacity return passes");
    assert!(
        brightness[34..=returned].iter().all(|&drawn| drawn == 0.0),
        "{:?}",
        &brightness[34..=returned]
    );
    let worst = worst_transitions_across(&[opacity.as_slice(), brightness.as_slice()], dt);
    assert!(worst <= 6, "{worst} transitions in total");
    // Once the window has room, the ghost draws its asked white at half
    // opacity.
    assert_eq!(shown.last(), Some(&look(0.5, WHITE)), "{brightness:?}");
}

// Regression: opacity and tint were each judged alone, so a ghost alternating
// opacity 1 and white with opacity 0.81 and a 0.81 grey counted nothing (each
// part stays above the 0.8 dark limit) while drawing 1 ↔ 0.66 over black.
// Judged on its drawn brightness it presents at most three flashes and, over
// budget, rests at the darker brightness. At 10 Hz the brightness rate cap
// alone keeps the darker state at 0.8, so it presents no flash at all.
#[test]
fn an_opacity_and_tint_strobe_is_limited_on_its_drawn_brightness() {
    let grey = [0.81; 3];
    for hz in [10.0f32, 5.0] {
        for fps in [60.0f32, 240.0] {
            let dt = 1.0 / fps;
            let asked = |n: usize| {
                if (n as f32 * dt * hz).fract() < 0.5 {
                    look(1.0, WHITE)
                } else {
                    look(0.81, grey)
                }
            };
            let frames = (3.0 * fps) as usize;
            let raw: Vec<_> = (0..frames).map(&asked).collect();
            assert_eq!(
                worst_transitions(&opacities(&raw), dt),
                0,
                "{hz} Hz at {fps} fps: opacity alone is no flash"
            );
            assert_eq!(
                worst_transitions(&drawn_luminances(&raw), dt),
                0,
                "{hz} Hz at {fps} fps: the tint alone is no flash"
            );
            assert!(
                worst_brightness_transitions(&opacities(&raw), &drawn_luminances(&raw), dt) > 6,
                "{hz} Hz at {fps} fps: the raw strobe draws more than three flashes"
            );

            let shown = present_looks(frames, fps, asked);
            let brightness = brightnesses(&shown);
            let worst = worst_transitions_over_any_background(
                &opacities(&shown),
                &drawn_luminances(&shown),
                dt,
            );
            assert!(worst <= 6, "{hz} Hz at {fps} fps: {worst} transitions");
            if hz == 5.0 {
                // The last second asks for the bright state half the time;
                // over budget the ghost rests at the darker brightness.
                let tail = (2.0 * fps) as usize;
                let asked_bright: Vec<usize> = (tail..frames)
                    .filter(|&n| asked(n).opacity == 1.0)
                    .collect();
                let held_dark = asked_bright
                    .iter()
                    .filter(|&&n| brightness[n] <= 0.66)
                    .count();
                assert!(
                    held_dark * 3 > asked_bright.len(),
                    "{fps} fps: {held_dark} of {} bright frames held dark",
                    asked_bright.len()
                );
            }
        }
    }
}

// Regression: a tint strobe at a steady opacity below 1 drew 0.9 ↔ 0.73 over
// black while the tint's own luminance stayed above the dark limit, so
// nothing counted.
#[test]
fn a_tint_strobe_at_a_steady_opacity_below_one_is_limited() {
    let grey = [0.81; 3];
    for fps in [60.0f32, 240.0] {
        let dt = 1.0 / fps;
        let asked = |n: usize| look(0.9, alternate(10.0, n as f32 * dt, WHITE, grey));
        let frames = (3.0 * fps) as usize;
        let raw: Vec<_> = (0..frames).map(&asked).collect();
        assert_eq!(worst_transitions(&drawn_luminances(&raw), dt), 0);
        assert!(
            worst_brightness_transitions(&opacities(&raw), &drawn_luminances(&raw), dt) > 6,
            "{fps} fps: raw strobe"
        );

        let shown = present_looks(frames, fps, asked);
        let worst = worst_brightness_transitions(&opacities(&shown), &drawn_luminances(&shown), dt);
        assert!(worst <= 6, "{fps} fps: {worst} transitions");
        assert!(shown.iter().all(|shown| shown.opacity == 0.9));
    }
}

// A tint strobe on an invisible ghost (opacity 0) changes nothing on screen
// and spends none of its budget: an opacity strobe after it presents exactly
// as after a steady dark second, with its full three flashes.
#[test]
fn a_tint_strobe_on_an_invisible_ghost_spends_no_budget() {
    for fps in [60.0f32, 240.0] {
        let dt = 1.0 / fps;
        let second = fps as usize;
        let tint_strobe = |n: usize| alternate(10.0, n as f32 * dt, BLACK, WHITE);
        let after_tint_strobe = present_looks(3 * second, fps, |n| {
            if n < second {
                look(0.0, tint_strobe(n))
            } else {
                opacity_strobe(n, dt, WHITE)
            }
        });
        let after_dark = present_looks(3 * second, fps, |n| {
            if n < second {
                look(0.0, BLACK)
            } else {
                opacity_strobe(n, dt, WHITE)
            }
        });
        // An invisible ghost presents its asked tint: nothing draws to limit.
        for (n, shown) in after_tint_strobe[..second].iter().enumerate() {
            assert_eq!(shown.tint, tint_strobe(n), "{fps} fps frame {n}");
        }
        assert_eq!(
            after_tint_strobe[second..],
            after_dark[second..],
            "{fps} fps"
        );
        let strobe = &after_tint_strobe[second..];
        assert_eq!(
            worst_brightness_transitions(&opacities(strobe), &drawn_luminances(strobe), dt),
            6,
            "{fps} fps: the opacity strobe's full three flashes"
        );
    }
}

// Regression: an opacity strobe moves opacity, drawn brightness and darkening
// together, but unless the tint is near white they cross the threshold on
// different frames, so one change counted two or three times: two flashes a
// second presented, not three, and a brightness onset held over budget
// darkened the tint, down to black. Each change now counts once, three flashes
// present, and the tint never changes.
#[test]
fn an_opacity_strobe_counts_each_change_once_and_never_darkens_the_tint() {
    let strobes = [(10.0f32, 1.0f32), (5.0, 0.45)];
    for fps in [60.0f32, 144.0, 240.0] {
        for tint in [STYLE_TINTS[0], STYLE_TINTS[1], WHITE] {
            for (hz, high) in strobes {
                let dt = 1.0 / fps;
                let shown = present_looks((3.0 * fps) as usize, fps, |n| {
                    let on = (n as f32 * dt * hz).fract() >= 0.5;
                    look(if on { high } else { 0.0 }, tint)
                });
                let what = format!("{hz} Hz to {high} at {fps} fps, {tint:?}");
                let opacity = opacities(&shown);
                assert_eq!(worst_transitions(&opacity, dt), 6, "{what}");
                assert_eq!(
                    worst_brightness_transitions(&opacity, &drawn_luminances(&shown), dt),
                    6,
                    "{what}"
                );
                assert!(shown.iter().all(|shown| shown.tint == tint), "{what}");
            }
        }
    }
}

// A black-tinted ghost draws nothing over black, but strobing its opacity over
// a bright wall darkens the wall: its opacity alone still limits it.
#[test]
fn a_black_ghosts_opacity_strobe_is_limited_on_its_opacity() {
    for fps in [60.0f32, 240.0] {
        let dt = 1.0 / fps;
        let asked = |n: usize| opacity_strobe(n, dt, BLACK);
        let frames = (3.0 * fps) as usize;
        let raw: Vec<_> = (0..frames).map(&asked).collect();
        assert!(
            worst_transitions(&opacities(&raw), dt) > 6,
            "{fps} fps: raw"
        );

        let shown = present_looks(frames, fps, asked);
        let worst = worst_transitions(&opacities(&shown), dt);
        assert!((1..=6).contains(&worst), "{fps} fps: {worst} transitions");
        assert!(brightnesses(&shown).iter().all(|&drawn| drawn == 0.0));
    }
}

// Regression: a tint's luminance was judged in float, but the draw packs the
// tint to 8 bits a channel. This tint measures just above the 0.8 dark limit in
// float and just under it as drawn, so a strobe against white went uncounted
// while drawing a flash every pulse.
#[test]
fn a_tint_is_judged_on_the_8_bit_tint_it_draws() {
    let tint = [0.398, 1.0, 0.005843];
    assert!(luminance(tint) >= 0.8, "{}", luminance(tint));
    assert!(luminance(drawn_tint(tint)) < 0.8);
    for fps in [60.0f32, 240.0] {
        let dt = 1.0 / fps;
        let asked = |n: usize| look(1.0, alternate(5.0, n as f32 * dt, WHITE, tint));
        let frames = (3.0 * fps) as usize;
        let raw: Vec<_> = (0..frames).map(&asked).collect();
        assert_eq!(
            worst_transitions(&luminances(&raw), dt),
            0,
            "{fps} fps: float"
        );
        assert!(
            worst_transitions(&drawn_luminances(&raw), dt) > 6,
            "{fps} fps: as drawn"
        );

        let shown = present_looks(frames, fps, asked);
        let worst = worst_brightness_transitions(&opacities(&shown), &drawn_luminances(&shown), dt);
        assert!(worst <= 6, "{fps} fps: {worst} transitions");
    }
}

// Regression: while acquires kept failing right after an install, each
// frame's snapshot cloned the never-allocated empty table, which freed the
// snapshot's own; the live table then allocated again on the next present.
// Both tables now start with room and keep it.
#[test]
fn frames_that_never_present_keep_both_tables_allocated() {
    let step = frame(1.0 / 60.0, true);
    let mut clamp = GhostLookClamp::<Key>::default();
    let room = (clamp.looks.capacity(), clamp.frame_start.capacity());
    assert!(room.0 > 0 && room.1 > 0, "{room:?}");
    for n in 0..30 {
        clamp.begin_frame(Some(&step));
        clamp.present(GHOST, look(0.5, WHITE));
        clamp.end_frame();
        clamp.finish_presentation(false);
        assert_eq!(
            (clamp.looks.capacity(), clamp.frame_start.capacity()),
            room,
            "frame {n}"
        );
    }
}

// Whatever a ghost asks, its flash window never overflows: a push past it is a
// debug assertion. Random looks, hitches, vanished frames and frames that
// never present, at several refresh rates, blended and dithered.
#[test]
fn random_looks_never_overflow_a_ghosts_flash_window() {
    let mut state = 0x9E37_79B9_7F4A_7C15u64;
    let mut next = move || {
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        (state >> 40) as f32 / (1u64 << 24) as f32
    };
    let tints = [WHITE, BLACK, [1.0, 0.0, 0.0], [0.81; 3], [0.6, 0.8, 1.0]];
    for (fps, transparency) in [30.0f32, 60.0, 144.0, 240.0]
        .into_iter()
        .flat_map(|fps| GhostTransparency::ALL.map(|transparency| (fps, transparency)))
    {
        let mut clamp = GhostLookClamp::<Key>::default();
        clamp.set_transparency(transparency);
        let mut asked = look(0.0, WHITE);
        for _ in 0..(20.0 * fps) as usize {
            if next() < 0.3 {
                let levels = [0.0, 0.2, 0.5, 0.81, 1.0, next()];
                asked.opacity = levels[(next() * 6.0) as usize % 6];
            }
            if next() < 0.3 {
                asked.tint = if next() < 0.2 {
                    [next(), next(), next()]
                } else {
                    tints[(next() * 5.0) as usize % 5]
                };
            }
            let dt = if next() < 0.02 { 0.2 } else { 1.0 / fps };
            clamp.begin_frame(Some(&frame(dt, true)));
            if next() >= 0.05 {
                let shown = clamp.present(GHOST, asked);
                assert!((0.0..=1.0).contains(&shown.opacity), "{shown:?}");
                assert!(
                    shown
                        .tint
                        .iter()
                        .all(|channel| (0.0..=1.0).contains(channel)),
                    "{shown:?}"
                );
            }
            clamp.end_frame();
            clamp.finish_presentation(next() >= 0.05);
        }
    }
}

/// Presented looks per frame, one ghost at `fps` with the limiter on, drawn
/// with `transparency`, from `asked(n)`.
fn present_looks_drawn(
    frames: usize,
    fps: f32,
    transparency: GhostTransparency,
    asked: impl Fn(usize) -> GhostInstanceLook,
) -> Vec<GhostInstanceLook> {
    let step = frame(1.0 / fps, true);
    let mut clamp = GhostLookClamp::default();
    clamp.set_transparency(transparency);
    (0..frames)
        .map(|n| {
            clamp.begin_frame(Some(&step));
            let shown = clamp.present(GHOST, asked(n));
            clamp.end_frame();
            shown
        })
        .collect()
}

// Regression: each of opacity, brightness and the tint's luminance stayed
// under the flash threshold or above the dark limit while what the ghost drew
// over a white wall flashed. Sequence A moves opacity and brightness oppositely
// by less than 0.1 each, which a white ghost over white draws as 0.9 ↔ 0.72.
// Sequence C strobes a black ghost's opacity between 1 and 0.81, which over
// white draws 0 ↔ 0.19. With the round-2 strobe of opacity and tint together,
// each presents at most three flashes a second over any background.
#[test]
fn the_worst_case_over_any_background_presents_at_most_three_flashes() {
    let sequences: [(&str, [GhostInstanceLook; 2]); 3] = [
        ("A", [look(0.60, [0.8333; 3]), look(0.69, [0.594; 3])]),
        ("C", [look(1.0, BLACK), look(0.81, BLACK)]),
        ("round 2", [look(1.0, WHITE), look(0.81, [0.81; 3])]),
    ];
    for (name, [first, second]) in sequences {
        for fps in [60.0f32, 144.0, 240.0] {
            let dt = 1.0 / fps;
            let asked = |n: usize| {
                if (n as f32 * dt * 10.0).fract() < 0.5 {
                    first
                } else {
                    second
                }
            };
            let frames = (3.0 * fps) as usize;
            let raw: Vec<_> = (0..frames).map(&asked).collect();
            let raw_worst = worst_transitions_over_any_background(
                &opacities(&raw),
                &drawn_luminances(&raw),
                dt,
            );
            assert!(raw_worst > 6, "{name} at {fps} fps: raw strobe");

            let shown = present_looks(frames, fps, asked);
            let worst = worst_transitions_over_any_background(
                &opacities(&shown),
                &drawn_luminances(&shown),
                dt,
            );
            assert!(worst <= 6, "{name} at {fps} fps: {worst} transitions");
        }
    }
}

// Regression: the screen door keeps the nearest sixteenth of its pixels, which
// the clamp didn't judge: a dithered opacity strobe of 0.6874 ↔ 0.7874 (a step
// just under 0.1) presents 11/16 ↔ 13/16 coverage, a step of 0.125. Dithered,
// it is judged on that coverage and limited; blended, the same strobe is no
// flash and presents as asked, within the rate cap. A strobe of 1 ↔ 0.8124
// presents 1 ↔ 13/16 in either mode: no deeper when dithered.
#[test]
fn a_dithered_ghost_is_judged_on_the_coverage_its_screen_door_presents() {
    assert_eq!(screen_door_coverage(0.8124), 13.0 / 16.0);
    for fps in [60.0f32, 144.0, 240.0] {
        let dt = 1.0 / fps;
        let frames = (3.0 * fps) as usize;
        let asked = |n: usize| {
            let high = (n as f32 * dt * 10.0).fract() >= 0.5;
            look(if high { 0.7874 } else { 0.6874 }, WHITE)
        };

        // Blended: no flash, and every frame is the asked strobe rate-capped.
        let blended = present_looks_drawn(frames, fps, GhostTransparency::Blend, asked);
        assert_eq!(worst_transitions(&opacities(&blended), dt), 0, "{fps} fps");
        let cap = INTENSITY_RATE_PER_SECOND * dt;
        let mut ramp = asked(0).opacity;
        for (n, shown) in blended.iter().enumerate() {
            ramp = asked(n).opacity.clamp(ramp - cap, ramp + cap);
            assert!((shown.opacity - ramp).abs() < 1e-6, "{fps} fps frame {n}");
        }

        // Dithered: the coverage the screen door presents is the flash.
        let raw: Vec<f32> = (0..frames)
            .map(|n| screen_door_coverage(asked(n).opacity))
            .collect();
        assert!(worst_transitions(&raw, dt) > 6, "{fps} fps: raw coverage");
        let dithered = present_looks_drawn(frames, fps, GhostTransparency::Dither, asked);
        let coverage = opacities(&dithered);
        let worst = worst_transitions(&coverage, dt);
        assert!((1..=6).contains(&worst), "{fps} fps: {worst} transitions");
        // Each presented opacity is a coverage the screen door draws exactly.
        for shown in &coverage {
            assert_eq!(screen_door_coverage(*shown), *shown, "{fps} fps");
        }
    }
}

// Regression: a held or slowed rise was placed on the 8-bit grid no brighter
// than the brightness the clamp held, which for a rise from a dark state at or
// just above the 0.8 dark limit could draw just under it: a dark state the
// clamp judged not to be one.
#[test]
fn a_held_rise_never_draws_past_the_dark_limit_it_was_judged_short_of() {
    let tint = [0.6, 0.8, 1.0];
    let held = 0.8012;
    // Rounded down onto the grid, as a held rise used to be, it draws dark.
    let aimed = tint_at_luminance(tint, held);
    let rounded_down = on_grid(aimed, 1.0, f32::NEG_INFINITY, held);
    assert!(drawn_brightness(1.0, rounded_down) < FLASH_DARK_LIMIT);

    let shown = presented_tint(tint, 1.0, 0.9, held);
    let drawn = drawn_brightness(1.0, shown);
    assert!(drawn >= FLASH_DARK_LIMIT, "{shown:?} draws {drawn}");
    assert!(drawn - held < 3.0 / 255.0, "{shown:?} draws {drawn}");
}
