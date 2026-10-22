// Ghost look clamp: the tint-luminance channel beside the opacity channel, hue kept
// over budget, black tints held as grey, and pass-through with the limiter off.
// See: context/lib/rendering_pipeline.md §7.8 (Photosensitivity limiter), §7.9

use super::super::flash_clamp_test_fixtures::worst_transitions;
use super::*;
use crate::flash_limiter::{LimiterFrameInput, pack_limiter_frame};

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
// asked colour scaled, never shifted toward another hue.
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
    let mut held = 0;
    for (tint, shown) in tints.iter().zip(&presented) {
        let scale = shown.tint[2] / tint[2];
        for channel in 0..3 {
            assert!(
                (shown.tint[channel] - tint[channel] * scale).abs() < 1e-5,
                "{shown:?} is not a scale of {tint:?}"
            );
        }
        if (scale - 1.0).abs() > 0.05 {
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

// Keys include the channel: an opacity strobe at a steady tint leaves the tint
// alone, a tint strobe at steady opacity leaves opacity alone, and a second
// ghost's history is its own.
#[test]
fn opacity_and_luminance_channels_and_ghosts_keep_separate_histories() {
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
fn tint_at_luminance_scales_clamps_and_greys_black() {
    let tint = [0.5, 0.25, 0.1];
    let half = tint_at_luminance(tint, luminance(tint) * 0.5);
    for channel in 0..3 {
        assert!((half[channel] - tint[channel] * 0.5).abs() < 1e-6);
    }
    let raised = tint_at_luminance([1.0, 0.0, 0.0], 0.9);
    assert_eq!(raised, [1.0, 0.0, 0.0], "components clamp at 1");
    let grey = tint_at_luminance(BLACK, 0.3);
    assert!(grey[0] == grey[1] && grey[1] == grey[2] && (grey[0] - 0.3).abs() < 1e-6);
    assert_eq!(tint_at_luminance(tint, luminance(tint)), tint);
}
