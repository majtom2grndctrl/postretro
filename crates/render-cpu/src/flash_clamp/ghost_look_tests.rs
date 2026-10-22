// Ghost look clamp: tint luminance on the same flash window as opacity, hue kept
// over budget, black tints held as grey, vanished ghosts, frames that never
// present, and pass-through with the limiter off.
// See: context/lib/rendering_pipeline.md §7.8 (Photosensitivity limiter), §7.9

use super::super::flash_clamp_test_fixtures::{worst_transitions, worst_transitions_across};
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
// the flash window. Judged here on the luminance actually drawn.
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

    let drawn = luminances(&present_tints(&tints, fps, Some(true)));
    let cap = INTENSITY_RATE_PER_SECOND * dt;
    for (n, pair) in drawn.windows(2).enumerate() {
        assert!(
            (pair[1] - pair[0]).abs() <= cap + 1e-5,
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

/// Presented opacity and drawn tint luminance per frame, one ghost at `fps`
/// with the limiter on, from `asked(n)`.
fn present_looks(
    frames: usize,
    fps: f32,
    asked: impl Fn(usize) -> GhostInstanceLook,
) -> (Vec<f32>, Vec<f32>) {
    let step = frame(1.0 / fps, true);
    let mut clamp = GhostLookClamp::default();
    (0..frames)
        .map(|n| {
            clamp.begin_frame(Some(&step));
            let shown = clamp.present(GHOST, asked(n));
            clamp.end_frame();
            (shown.opacity, luminance(shown.tint))
        })
        .unzip()
}

// One ghost's opacity, tint redness and tint luminance share one flash window:
// opacity strobing 0 ↔ 1 at 10 Hz while its tint strobes black ↔ white half a
// period behind presents at most three flashes in any second in total, though
// each alone would fit three of its own.
#[test]
fn a_10hz_opacity_strobe_with_an_offset_tint_strobe_presents_at_most_three_flashes_in_total() {
    for fps in [60.0f32, 240.0] {
        let dt = 1.0 / fps;
        // Both start dark; after the first half period the tint is white
        // exactly while opacity is low.
        let asked = |n: usize| {
            let t = n as f32 * dt;
            let opacity = if (t * 10.0).fract() >= 0.5 { 1.0 } else { 0.0 };
            let tint = if t >= 0.05 && opacity == 0.0 {
                WHITE
            } else {
                BLACK
            };
            look(opacity, tint)
        };
        let frames = (3.0 * fps) as usize;
        let raw_opacity: Vec<f32> = (0..frames).map(|n| asked(n).opacity).collect();
        let raw_luminance: Vec<f32> = (0..frames).map(|n| luminance(asked(n).tint)).collect();
        assert!(
            worst_transitions_across(&[raw_opacity.as_slice(), raw_luminance.as_slice()], dt) > 12,
            "{fps} Hz: raw strobe"
        );

        let (opacity, drawn) = present_looks(frames, fps, asked);
        let worst = worst_transitions_across(&[opacity.as_slice(), drawn.as_slice()], dt);
        assert!(worst <= 6, "{fps} Hz: {worst} transitions in total");
        assert!(
            worst_transitions(&opacity, dt) > 0 && worst_transitions(&drawn, dt) > 0,
            "{fps} Hz: both parts present some flash"
        );
    }
}

// Regression: an opacity onset that takes the window's last room owes its
// return that room, so a tint-luminance onset after it holds dark until the
// window drains; admitted, it would have left the return no room.
#[test]
fn a_tint_onset_waits_for_the_return_an_opacity_onset_still_owes() {
    let fps = 60.0;
    let dt = 1.0 / fps;
    // Two quick opacity pulses (3 frames up, 3 down), a third rise held to
    // frame 32, then opacity falls. The tint turns white on frame 16, after
    // the third rise was counted, and stays white.
    let asked = |n: usize| {
        let opacity = match n {
            0 => 0.0,
            1..=12 if (n - 1) % 6 >= 3 => 0.0,
            1..=32 => 1.0,
            _ => 0.0,
        };
        look(opacity, if n >= 16 { WHITE } else { BLACK })
    };
    let (opacity, drawn) = present_looks(150, fps, asked);

    // The tint holds black while the opacity onset owes its return, and
    // while that return plays out at the rate cap.
    let returned = 33 + (1.0 / (INTENSITY_RATE_PER_SECOND * dt)).ceil() as usize;
    assert_eq!(opacity[returned], 0.0, "the opacity return passes");
    assert!(
        drawn[16..=returned].iter().all(|&shown| shown == 0.0),
        "{:?}",
        &drawn[16..=returned]
    );
    let worst = worst_transitions_across(&[opacity.as_slice(), drawn.as_slice()], dt);
    assert!(worst <= 6, "{worst} transitions in total");
    // Once the window has room, the tint rises to its asked white.
    assert_eq!(drawn.last(), Some(&luminance(WHITE)), "{drawn:?}");
}
