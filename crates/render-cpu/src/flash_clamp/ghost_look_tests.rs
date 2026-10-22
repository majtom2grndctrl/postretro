// Ghost look clamp: drawn brightness on the same flash window as opacity, one
// change counted once, hue kept over budget, black tints held as grey, tints
// judged as drawn, vanished ghosts, frames that never present, and
// pass-through with the limiter off.
// See: context/lib/rendering_pipeline.md §7.8 (Photosensitivity limiter), §7.9

use super::super::flash_clamp_test_fixtures::{
    worst_brightness_transitions, worst_transitions, worst_transitions_across,
};
use super::super::value::INTENSITY_RATE_PER_SECOND;
use super::*;
use crate::flash_limiter::{HITCH_CEILING_SECONDS, LimiterFrameInput, pack_limiter_frame};

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
// second and, over budget, rests at its pre-flash luminance (black, before the
// first white onset). Opacity and saturation are untouched throughout.
#[test]
fn a_10hz_white_black_tint_strobe_presents_at_most_three_flashes_and_rests_at_pre_flash_luminance()
{
    for fps in [60.0f32, 240.0] {
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
        assert!(presented.iter().all(|look| look.opacity == 1.0));
        assert!(presented.iter().all(|look| look.saturation == 0.5));

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

// Regression: a vanished ghost counted as its tint going black, so quick
// re-entries rate-capped the tint up from black and, once its window filled,
// held the ghost near black. Only its opacity falls while it is gone.
#[test]
fn a_ghost_reentering_at_10hz_never_presents_a_dark_tint() {
    let fps = 60.0;
    let dt = 1.0 / fps;
    let step = frame(dt, true);
    let tint = [0.6, 0.8, 1.0];
    let mut clamp = GhostLookClamp::default();
    let mut presented = 0;
    for n in 0..180 {
        // Armed for the first half of each 10 Hz period; disarmed, the ghost
        // leaves the member list and presents nothing.
        let armed = (n as f32 * dt * 10.0).fract() < 0.5;
        clamp.begin_frame(Some(&step));
        if armed {
            let shown = clamp.present(GHOST, look(0.45, tint));
            assert_eq!(shown.tint, tint, "frame {n}");
            presented += 1;
        }
        clamp.end_frame();
    }
    assert!(presented > 0);

    // The held tint goes with the opacity history: a ghost back after its
    // window emptied starts fresh against its own tint.
    for _ in 0..90 {
        clamp.begin_frame(Some(&step));
        clamp.end_frame();
    }
    clamp.begin_frame(Some(&step));
    let back = clamp.present(GHOST, look(0.45, BLACK));
    clamp.end_frame();
    assert_eq!(back.tint, BLACK);
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
    let step = frame(1.0 / fps, true);
    let mut clamp = GhostLookClamp::default();
    (0..frames)
        .map(|n| {
            clamp.begin_frame(Some(&step));
            let shown = clamp.present(GHOST, asked(n));
            clamp.end_frame();
            shown
        })
        .collect()
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
// window drains; admitted, it would have left the return no room.
#[test]
fn a_brightness_onset_waits_for_the_return_an_opacity_onset_still_owes() {
    let fps = 60.0;
    let dt = 1.0 / fps;
    // Two quick opacity pulses (3 frames up, 3 down), a third rise held to
    // frame 32, then opacity falls to half. The tint turns white on frame 16,
    // after the third rise was counted, and stays white.
    let asked = |n: usize| {
        let opacity = match n {
            0 => 0.0,
            1..=12 if (n - 1) % 6 >= 3 => 0.0,
            1..=32 => 1.0,
            _ => 0.5,
        };
        look(opacity, if n >= 16 { WHITE } else { BLACK })
    };
    let shown = present_looks(150, fps, asked);
    let (opacity, brightness) = (opacities(&shown), brightnesses(&shown));

    // The ghost draws dark while the opacity onset owes its return, and while
    // that return plays out at the rate cap.
    let returned = 33 + (0.5 / (INTENSITY_RATE_PER_SECOND * dt)).ceil() as usize;
    assert_eq!(opacity[returned], 0.5, "the opacity return passes");
    assert!(
        brightness[16..=returned].iter().all(|&drawn| drawn == 0.0),
        "{:?}",
        &brightness[16..=returned]
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
            let worst =
                worst_brightness_transitions(&opacities(&shown), &drawn_luminances(&shown), dt);
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

// A white ghost's opacity strobe moves its opacity and drawn brightness
// together, one change counted once: three flashes a second present, not one
// and a half, and the tint never darkens.
#[test]
fn a_white_ghosts_opacity_strobe_counts_each_change_once() {
    for fps in [60.0f32, 240.0] {
        let dt = 1.0 / fps;
        let shown = present_looks((3.0 * fps) as usize, fps, |n| opacity_strobe(n, dt, WHITE));
        let opacity = opacities(&shown);
        assert_eq!(worst_transitions(&opacity, dt), 6, "{fps} fps");
        assert_eq!(
            worst_brightness_transitions(&opacity, &drawn_luminances(&shown), dt),
            6,
            "{fps} fps"
        );
        assert!(shown.iter().all(|shown| shown.tint == WHITE), "{fps} fps");
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

// Whatever a ghost asks, its flash window never holds more than eight
// transitions: a push past that is a debug assertion. Random looks, hitches,
// vanished frames and frames that never present, at several refresh rates.
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
    for fps in [30.0f32, 60.0, 144.0, 240.0] {
        let mut clamp = GhostLookClamp::<Key>::default();
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
