// Per-ghost look clamp: three flashes a second over any background,
// regressions, targeted attacks, strobes, ordinary looks, red, dither,
// appearing, and drops at rest.
// See: context/lib/rendering_pipeline.md §7.8 (Photosensitivity limiter), §7.9

use std::ops::Range;

use super::super::flash_clamp_test_fixtures::{
    transition_frames_over, transitions_over, worst_transitions,
    worst_transitions_over_any_background, worst_transitions_over_any_background_at,
};
use super::super::ghost_look_clamp::GhostLookClamp;
use super::super::ghost_tint::{drawn_luminance, tint_at_luminance};
use super::super::value::{INTENSITY_RATE_PER_SECOND, RED_SATURATED, luminance, redness};
use super::*;
use crate::flash_limiter::{HITCH_CEILING_SECONDS, LimiterFrameInput, pack_limiter_frame};
use crate::ghost_instance::drawn_tint;

/// A ghost identity: link and generation-checked entity.
type Key = (u16, u32);

const GHOST: Key = (0, 7);
const WHITE: [f32; 3] = [1.0; 3];
const BLACK: [f32; 3] = [0.0; 3];
/// The dev map's `echo` style tint.
const ECHO: [f32; 3] = [0.6, 0.8, 1.0];
/// The colour filter's red-orange.
const EMBER: [f32; 3] = [1.0, 0.4, 0.3];
const BLUE: [f32; 3] = [0.2, 0.4, 1.0];
const RATES: [f32; 3] = [60.0, 144.0, 240.0];

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

fn grey(level: f32) -> [f32; 3] {
    [level; 3]
}

/// `a` for the first half of each period at `hz`, then `b`.
fn alternate<T>(hz: f32, t: f32, a: T, b: T) -> T {
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

/// Presented looks per frame, one ghost at `fps` with the limiter on, from
/// `asked(n)`.
fn present_looks(
    frames: usize,
    fps: f32,
    asked: impl Fn(usize) -> GhostInstanceLook,
) -> Vec<GhostInstanceLook> {
    present_looks_drawn(frames, fps, GhostTransparency::Blend, asked)
}

/// Presented looks per frame, one ghost at `fps` with the limiter on, drawn
/// with `transparency`, from `asked(n)`.
fn present_looks_drawn(
    frames: usize,
    fps: f32,
    transparency: GhostTransparency,
    asked: impl Fn(usize) -> GhostInstanceLook,
) -> Vec<GhostInstanceLook> {
    let asked: Vec<Option<GhostInstanceLook>> = (0..frames).map(|n| Some(asked(n))).collect();
    present_sequence(&asked, fps, transparency)
        .into_iter()
        .map(|shown| shown.expect("every frame presents"))
        .collect()
}

/// Presented looks per frame, one ghost at `fps` with the limiter on, drawn
/// with `transparency`. A `None` frame presents nothing: the ghost vanished.
fn present_sequence(
    asked: &[Option<GhostInstanceLook>],
    fps: f32,
    transparency: GhostTransparency,
) -> Vec<Option<GhostInstanceLook>> {
    let step = frame(1.0 / fps, true);
    let mut clamp = GhostLookClamp::default();
    clamp.set_transparency(transparency);
    asked
        .iter()
        .map(|asked| {
            clamp.begin_frame(Some(&step));
            let shown = asked.map(|asked| clamp.present(GHOST, asked));
            clamp.end_frame();
            shown
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
        .map(|look| look.opacity * drawn_luminance(look.tint))
        .collect()
}

/// The most transitions in any second over any background of the grid, for
/// looks drawn with `transparency` (the screen door's coverage, dithered).
fn over_any(looks: &[GhostInstanceLook], dt: f32, transparency: GhostTransparency) -> usize {
    let opacity: Vec<f32> = looks
        .iter()
        .map(|look| drawn_opacity(transparency)(look.opacity))
        .collect();
    worst_transitions_over_any_background(&opacity, &drawn_luminances(looks), dt)
}

/// The longest run of frames presenting one look: its length and the look.
fn longest_held(looks: &[GhostInstanceLook]) -> (usize, GhostInstanceLook) {
    let mut longest = (1, looks[0]);
    let mut run = 1;
    for pair in looks.windows(2) {
        run = if pair[0] == pair[1] { run + 1 } else { 1 };
        if run > longest.0 {
            longest = (run, pair[1]);
        }
    }
    longest
}

/// Looks interpolated at the rate cap through `keys`, each a point in (p,
/// p + q): what a white ghost draws over black and over white. The tint is
/// the grey whose luminance draws them.
fn rate_capped_path(keys: &[(f32, f32)], fps: f32, seconds: f32) -> Vec<GhostInstanceLook> {
    let cap = INTENSITY_RATE_PER_SECOND / fps;
    let mut looks = Vec::new();
    for pair in keys.windows(2) {
        let ((x0, y0), (x1, y1)) = (pair[0], pair[1]);
        let largest = (x1 - x0)
            .abs()
            .max((y1 - y0).abs())
            .max(((y1 - x1) - (y0 - x0)).abs());
        let steps = ((largest / cap) - 1e-6).ceil().max(1.0) as usize;
        for step in 1..=steps {
            let t = step as f32 / steps as f32;
            let (x, y) = (x0 + (x1 - x0) * t, y0 + (y1 - y0) * t);
            let opacity = 1.0 - (y - x);
            let level = if opacity > 0.0 { x / opacity } else { 0.0 };
            looks.push(look(opacity, grey(level.clamp(0.0, 1.0))));
        }
    }
    looks.truncate((seconds * fps) as usize);
    looks
}

/// Keys of a drift in (p, p + q): the drawn value at λ of the way from a
/// white ghost over black to one over white swings by `swing` every step,
/// while over black it dips and over white it rises by only `sub` (< 0.1),
/// so neither over black nor over white sees a flash.
fn drift_keys(swing: f32, lambda: f32, sub: f32) -> Vec<(f32, f32)> {
    let up = (swing - lambda * sub) / (1.0 - lambda);
    let down = (swing - (1.0 - lambda) * sub) / lambda;
    let valid = |(x, y): (f32, f32)| (0.0..=1.0).contains(&x) && x <= y && y <= 1.0;
    let (mut x, mut y) = (0.05, 1.0 - sub);
    let mut keys = vec![(x, y)];
    let mut rising = true;
    for _ in 0..400 {
        let (a, b) = if rising {
            let a = (x + up, y + sub);
            (a, (a.0 - sub, a.1 - down))
        } else {
            let a = (x - up, y - sub);
            (a, (a.0 + sub, a.1 + down))
        };
        let blocked = if rising { b.1 - b.0 < 0.05 } else { a.0 < 0.02 };
        if !valid(a) || !valid(b) || blocked {
            rising = !rising;
            continue;
        }
        keys.push(a);
        keys.push(b);
        (x, y) = b;
    }
    keys
}

/// Assert `asked` strobes over some background and presents at most three
/// flashes a second over every one, at each rate.
fn assert_limited_over_any_background(name: &str, asked: impl Fn(f32) -> Vec<GhostInstanceLook>) {
    for fps in RATES {
        let dt = 1.0 / fps;
        let raw = asked(fps);
        let raw_worst = over_any(&raw, dt, GhostTransparency::Blend);
        assert!(raw_worst > 6, "{name} at {fps} fps: raw {raw_worst}");
        let shown = present_looks(raw.len(), fps, |n| raw[n]);
        let worst = over_any(&shown, dt, GhostTransparency::Blend);
        assert!(worst <= 6, "{name} at {fps} fps: {worst} transitions");
    }
}

// Regression: judged only over black, over white and for a white ghost over
// white, each saw a slow drift while a mix of two swung, so a mid-grey
// background drew 10–31 transitions a second that presented unchanged.
// Opacity climbs while the tint alternates bright and dim; the worst is tuned
// between two edge samples (λ = 17/32).
#[test]
fn a_mid_grey_drift_presents_at_most_three_flashes_over_any_background() {
    for (swing, lambda) in [(0.12, 0.5), (0.105, 17.0 / 32.0), (0.15, 0.5)] {
        assert_limited_over_any_background(&format!("drift {swing} at {lambda}"), |fps| {
            rate_capped_path(&drift_keys(swing, lambda, 0.095), fps, 3.0)
        });
    }
}

// Regression: the dark-state test assumed albedo 1, so a white ghost strobing
// its tint between white and a 0.85 grey counted nothing, though at albedo
// 0.9 over black it draws 0.9 ↔ 0.765.
#[test]
fn a_near_white_tint_strobe_is_limited_over_any_background() {
    let strobe = |fps: f32| -> Vec<GhostInstanceLook> {
        (0..(3.0 * fps) as usize)
            .map(|n| look(1.0, alternate(10.0, n as f32 / fps, WHITE, grey(0.85))))
            .collect()
    };
    let raw = strobe(60.0);
    assert_eq!(
        transitions_over(
            1.0,
            0.0,
            &opacities(&raw),
            &drawn_luminances(&raw),
            1.0 / 60.0
        ),
        0,
        "a white ghost over black sees no flash"
    );
    assert_limited_over_any_background("near-white strobe", strobe);
}

// Regression: two-part and three-part judging each missed what one of these
// draws over some background. Round 2 alternates opacity 1 and white with
// opacity 0.81 and a 0.81 grey: neither opacity nor the tint alone is a flash,
// but over black it draws 1 ↔ 0.66. Sequence A moves opacity and brightness
// oppositely by less than 0.1 each, which a white ghost over white draws as
// 0.9 ↔ 0.72. Sequence C strobes a black ghost's opacity between 1 and 0.81,
// which over white draws 0 ↔ 0.19.
#[test]
fn the_round_2_and_round_3_strobes_present_at_most_three_flashes_over_any_background() {
    let round_2 = [look(1.0, WHITE), look(0.81, grey(0.81))];
    let raw: Vec<_> = (0..180)
        .map(|n| alternate(10.0, n as f32 / 60.0, round_2[0], round_2[1]))
        .collect();
    assert_eq!(worst_transitions(&opacities(&raw), 1.0 / 60.0), 0);
    assert_eq!(worst_transitions(&drawn_luminances(&raw), 1.0 / 60.0), 0);
    for (name, [first, second]) in [
        ("round 2", round_2),
        ("A", [look(0.60, grey(0.8333)), look(0.69, grey(0.594))]),
        ("C", [look(1.0, BLACK), look(0.81, BLACK)]),
    ] {
        assert_limited_over_any_background(name, |fps| {
            (0..(3.0 * fps) as usize)
                .map(|n| alternate(10.0, n as f32 / fps, first, second))
                .collect()
        });
    }
}

// Regression: each part kept its own rate cap, so one change from an opaque
// 0.4 grey to clear white held brightness while opacity fell, drawing
// 0.40 → 0.64 → 0.40 over a 0.4 background, a flash that spent one slot. On
// the straight line it draws no flash anywhere.
#[test]
fn a_single_change_from_opaque_grey_to_clear_white_presents_no_flash() {
    for fps in RATES {
        let dt = 1.0 / fps;
        let frames = 3 + (0.5 * fps) as usize;
        let asked = |n: usize| {
            if n < 3 {
                look(1.0, grey(0.4))
            } else {
                look(0.0, WHITE)
            }
        };
        let shown = present_looks(frames, fps, asked);
        let worst = over_any(&shown, dt, GhostTransparency::Blend);
        assert!(worst <= 1, "{fps} fps: {worst} transitions");
        assert_eq!(shown.last().map(|look| look.opacity), Some(0.0));
    }
}

// With the limiter on and opacity steady, a tint alternating black and white
// at 10 Hz presents at most three flashes in any second over any background
// and rests at a held state. Opacity and saturation are untouched, and a black
// tint held above its luminance presents as a grey, the only colour with no
// hue to lose.
#[test]
fn a_10hz_white_black_tint_strobe_presents_at_most_three_flashes_and_rests_at_a_held_state() {
    for fps in RATES {
        let dt = 1.0 / fps;
        for (first, second) in [(BLACK, WHITE), (WHITE, BLACK)] {
            let tints: Vec<_> = (0..(3.0 * fps) as usize)
                .map(|n| alternate(10.0, n as f32 * dt, first, second))
                .collect();
            let raw: Vec<_> = tints.iter().map(|&tint| look(1.0, tint)).collect();
            assert!(worst_transitions(&luminances(&raw), dt) >= 19, "{fps} Hz");

            let shown = present_tints(&tints, fps, Some(true));
            let worst = over_any(&shown, dt, GhostTransparency::Blend);
            assert!(worst <= 6, "{fps} Hz from {first:?}: {worst} transitions");
            assert!(shown.iter().all(|look| look.opacity == 1.0));
            assert!(shown.iter().all(|look| look.saturation == 0.5));
            let (held, _) = longest_held(&shown);
            assert!(
                held as f32 >= 0.3 * fps,
                "{fps} Hz from {first:?}: held {held} frames at most"
            );
            for (tint, shown) in tints.iter().zip(&shown) {
                if *tint == BLACK && luminance(shown.tint) > 0.0 {
                    let [r, g, b] = shown.tint;
                    assert!(r == g && g == b, "a held black tint is grey: {shown:?}");
                }
            }
        }
    }
}

// A tint strobe is limited without being over-limited: each change counts
// once over every background, so it presents five transitions a second where
// it draws its largest swing (a white ghost over black), not one or two.
#[test]
fn a_tint_strobe_presents_five_transitions_where_it_draws_largest() {
    for fps in RATES {
        let dt = 1.0 / fps;
        for (opacity, a, b) in [
            (1.0, WHITE, BLACK),
            (1.0, BLACK, WHITE),
            (0.5, ECHO, BLACK),
            (1.0, WHITE, BLUE),
        ] {
            let frames = (3.0 * fps) as usize;
            let shown = present_looks(frames, fps, |n| {
                look(opacity, alternate(10.0, n as f32 * dt, a, b))
            });
            let what = format!("{a:?} ↔ {b:?} at {opacity}, {fps} fps");
            let worst = over_any(&shown, dt, GhostTransparency::Blend);
            assert!(
                (5..=6).contains(&worst),
                "{what}: {worst} over any background"
            );
            let over_black =
                transitions_over(1.0, 0.0, &opacities(&shown), &drawn_luminances(&shown), dt);
            assert!(over_black >= 5, "{what}: {over_black} over black");
        }
    }
}

// Regression: an opacity strobe moves what draws at every background, but the
// parts crossed the threshold on different frames, so one change counted two
// or three times, and at high refresh rates the last admitted onset froze
// below the flash threshold: two flashes presented, not three. Each change now
// counts once and completes, three flashes present, the tint never changes,
// and over budget the ghost rests at its pre-flash level.
#[test]
fn an_opacity_strobe_presents_exactly_three_flashes_and_rests_at_its_pre_flash_level() {
    let strobes = [(10.0f32, 1.0f32), (5.0, 0.45)];
    for fps in RATES {
        for tint in [ECHO, EMBER, WHITE] {
            for (hz, high) in strobes {
                let dt = 1.0 / fps;
                let shown = present_looks((3.0 * fps) as usize, fps, |n| {
                    let on = (n as f32 * dt * hz).fract() >= 0.5;
                    look(if on { high } else { 0.0 }, tint)
                });
                let what = format!("{hz} Hz to {high} at {fps} fps, {tint:?}");
                let opacity = opacities(&shown);
                assert_eq!(worst_transitions(&opacity, dt), 6, "{what}");
                assert_eq!(worst_transitions(&brightnesses(&shown), dt), 6, "{what}");
                assert_eq!(over_any(&shown, dt, GhostTransparency::Blend), 6, "{what}");
                assert!(shown.iter().all(|shown| shown.tint == tint), "{what}");
                let (held, rest) = longest_held(&shown);
                assert!(held as f32 >= 0.3 * fps, "{what}: held {held} frames");
                assert_eq!(rest.opacity, 0.0, "{what}: rests at {rest:?}");
            }
        }
    }
}

// A black-tinted ghost draws nothing over black, but strobing its opacity over
// a bright wall darkens the wall: what it draws there limits it.
#[test]
fn a_black_ghosts_opacity_strobe_is_limited_on_its_opacity() {
    for fps in RATES {
        let dt = 1.0 / fps;
        let asked = |n: usize| look(alternate(10.0, n as f32 * dt, 0.0, 1.0), BLACK);
        let frames = (3.0 * fps) as usize;
        let raw: Vec<_> = (0..frames).map(asked).collect();
        assert!(
            worst_transitions(&opacities(&raw), dt) > 6,
            "{fps} fps: raw"
        );

        let shown = present_looks(frames, fps, asked);
        let worst = over_any(&shown, dt, GhostTransparency::Blend);
        assert!((1..=6).contains(&worst), "{fps} fps: {worst} transitions");
        assert!(brightnesses(&shown).iter().all(|&drawn| drawn == 0.0));
    }
}

// One ghost's luminance and redness share one window: a tint strobe, then an
// opacity strobe of the ghost tinted black, present at most three flashes in
// any second over any background in total, and each presents some.
#[test]
fn a_tint_strobe_then_an_opacity_strobe_present_at_most_three_flashes_in_total() {
    for fps in RATES {
        let dt = 1.0 / fps;
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
        let raw: Vec<_> = (0..frames).map(asked).collect();
        assert!(
            over_any(&raw, dt, GhostTransparency::Blend) > 6,
            "{fps} fps: raw"
        );
        let shown = present_looks(frames, fps, asked);
        let worst = over_any(&shown, dt, GhostTransparency::Blend);
        assert!(worst <= 6, "{fps} fps: {worst} transitions in total");
        assert!(
            worst_transitions(&opacities(&shown), dt) > 0
                && worst_transitions(&brightnesses(&shown), dt) > 0,
            "{fps} fps: both strobes present some flash"
        );
    }
}

// Regression: a tint strobe at a steady opacity below one drew 0.9 ↔ 0.73 over
// black while the tint's own luminance stayed above the dark limit, so nothing
// counted.
#[test]
fn a_tint_strobe_at_a_steady_opacity_below_one_is_limited() {
    for fps in RATES {
        let dt = 1.0 / fps;
        let asked = |n: usize| look(0.9, alternate(10.0, n as f32 * dt, WHITE, grey(0.81)));
        let frames = (3.0 * fps) as usize;
        let raw: Vec<_> = (0..frames).map(asked).collect();
        assert_eq!(worst_transitions(&drawn_luminances(&raw), dt), 0);
        assert!(
            over_any(&raw, dt, GhostTransparency::Blend) > 6,
            "{fps} fps: raw"
        );
        let shown = present_looks(frames, fps, asked);
        let worst = over_any(&shown, dt, GhostTransparency::Blend);
        assert!(worst <= 6, "{fps} fps: {worst} transitions");
        assert!(shown.iter().all(|shown| shown.opacity == 0.9));
    }
}

// Regression: a tint's luminance was judged in float, but the draw packs the
// tint to 8 bits a channel. This tint measures just above the 0.8 dark limit in
// float and just under it as drawn.
#[test]
fn a_tint_is_judged_on_the_8_bit_tint_it_draws() {
    let tint = [0.398, 1.0, 0.005843];
    assert!(luminance(tint) >= 0.8, "{}", luminance(tint));
    assert!(luminance(drawn_tint(tint)) < 0.8);
    for fps in RATES {
        let dt = 1.0 / fps;
        let asked = |n: usize| look(1.0, alternate(5.0, n as f32 * dt, WHITE, tint));
        let frames = (3.0 * fps) as usize;
        let raw: Vec<_> = (0..frames).map(asked).collect();
        assert_eq!(
            worst_transitions(&luminances(&raw), dt),
            0,
            "{fps} fps: float"
        );
        assert!(
            worst_transitions(&drawn_luminances(&raw), dt) > 6,
            "{fps} fps: drawn"
        );
        let shown = present_looks(frames, fps, asked);
        let worst = over_any(&shown, dt, GhostTransparency::Blend);
        assert!(worst <= 6, "{fps} fps: {worst} transitions");
    }
}

/// `asked` under the straight-line rate cap from the presented `from`, as
/// (opacity, drawn luminance), and whether the cap bound.
fn rate_capped(from: (f32, f32), asked: (f32, f32), cap: f32) -> ((f32, f32), bool) {
    let lit = asked.0 * asked.1 - from.0 * from.1;
    let covered = (1.0 - asked.0) - (1.0 - from.0);
    let largest = lit.abs().max(covered.abs()).max((lit + covered).abs());
    if largest <= cap {
        return (asked, false);
    }
    let scale = cap / largest;
    let opacity = from.0 + (asked.0 - from.0) * scale;
    let level = if asked.1 == from.1 {
        from.1
    } else {
        ((from.0 * from.1 + lit * scale) / opacity).clamp(0.0, 1.0)
    };
    ((opacity, level), true)
}

// Ordinary looks are never held. Each frame presents the asked look
// under the rate cap from what presented last: exactly as asked whenever the
// cap does not bind. The dev `echo` style stepping 0.45 ↔ 0.6, the example's
// 1500 ms `setColorFilter` fades of opacity and tint together, the 250 ms arm
// and disarm, and a ghost re-entering twice in a second, blended and dithered.
#[test]
fn ordinary_looks_present_as_asked_within_the_rate_cap() {
    let lerp = |a: f32, b: f32, t: f32| a + (b - a) * t.min(1.0);
    let mix = |a: [f32; 3], b: [f32; 3], t: f32| -> [f32; 3] {
        std::array::from_fn(|i| lerp(a[i], b[i], t))
    };
    for fps in RATES {
        let dt = 1.0 / fps;
        let frames = |seconds: f32| (seconds * fps).round() as usize;
        let fade = |from: (f32, [f32; 3]), to: (f32, [f32; 3])| -> Vec<Option<GhostInstanceLook>> {
            let mut looks = vec![Some(look(from.0, from.1)); frames(0.2)];
            looks.extend((0..frames(1.5)).map(|n| {
                let t = n as f32 * dt / 1.5;
                Some(look(lerp(from.0, to.0, t), mix(from.1, to.1, t)))
            }));
            looks.extend(vec![Some(look(to.0, to.1)); frames(0.5)]);
            looks
        };
        let mut arm: Vec<_> = (0..frames(0.5))
            .map(|n| Some(look(lerp(0.0, 0.6, n as f32 * dt / 0.25), ECHO)))
            .collect();
        arm.extend(
            (0..frames(0.5)).map(|n| Some(look(lerp(0.6, 0.0, n as f32 * dt / 0.25), ECHO))),
        );
        let mut re_entry = vec![Some(look(0.6, ECHO)); frames(0.3)];
        for _ in 0..2 {
            re_entry.extend([None; 3]);
            re_entry.extend(vec![Some(look(0.6, ECHO)); frames(0.3)]);
        }
        let mut steps = Vec::new();
        for opacity in [0.45, 0.6, 0.45, 0.6] {
            steps.extend(vec![Some(look(opacity, ECHO)); frames(0.3)]);
        }
        let cases = [
            ("echo steps", steps),
            (
                "fade 0.45 → 1, echo → ember",
                fade((0.45, ECHO), (1.0, EMBER)),
            ),
            (
                "fade 1 → 0.2, white → black",
                fade((1.0, WHITE), (0.2, BLACK)),
            ),
            (
                "fade 0.6 → 0.45, echo → white",
                fade((0.6, ECHO), (0.45, WHITE)),
            ),
            ("arm and disarm", arm),
            ("re-entry twice", re_entry),
        ];
        for transparency in GhostTransparency::ALL {
            for (name, asked) in &cases {
                let shown = present_sequence(asked, fps, transparency);
                let mut last = None;
                for (n, (asked, shown)) in asked.iter().zip(&shown).enumerate() {
                    let what = format!("{name}, {transparency:?} at {fps} fps, frame {n}");
                    let (Some(asked), Some(shown)) = (asked, shown) else {
                        last = last.map(|(_, level)| (0.0, level));
                        continue;
                    };
                    let asked_drawn = (asked.opacity, drawn_luminance(asked.tint));
                    let from = last.unwrap_or(asked_drawn);
                    let (expected, capped) =
                        rate_capped(from, asked_drawn, INTENSITY_RATE_PER_SECOND * dt);
                    if capped {
                        assert!(
                            (shown.opacity - expected.0).abs() < 1e-5,
                            "{what}: {shown:?}"
                        );
                        let level = drawn_luminance(shown.tint);
                        assert!(
                            (level - expected.1).abs() <= 1.0 / 255.0,
                            "{what}: {shown:?}"
                        );
                    } else {
                        assert_eq!(*shown, *asked, "{what}");
                    }
                    last = Some((shown.opacity, drawn_luminance(shown.tint)));
                }
            }
        }
    }
}

// Regression: the screen door keeps the nearest sixteenth of its pixels, which
// the clamp didn't judge: a dithered strobe of 0.656 ↔ 0.719, a step under the
// edge threshold, presents 10/16 ↔ 12/16 coverage, a step of 0.125. Dithered,
// it is judged on that coverage and limited; blended, the same strobe is no
// flash and presents as asked within the rate cap.
#[test]
fn a_dithered_ghost_is_judged_on_the_coverage_its_screen_door_presents() {
    assert_eq!(screen_door_coverage(0.656), 10.0 / 16.0);
    assert_eq!(screen_door_coverage(0.719), 12.0 / 16.0);
    for fps in RATES {
        let dt = 1.0 / fps;
        let frames = (3.0 * fps) as usize;
        let asked = |n: usize| look(alternate(10.0, n as f32 * dt, 0.656, 0.719), WHITE);

        // Blended: no flash, and every frame is the asked strobe rate-capped.
        let blended = present_looks_drawn(frames, fps, GhostTransparency::Blend, asked);
        assert_eq!(
            over_any(&blended, dt, GhostTransparency::Blend),
            0,
            "{fps} fps"
        );
        let cap = INTENSITY_RATE_PER_SECOND * dt;
        let mut ramp = asked(0).opacity;
        for (n, shown) in blended.iter().enumerate() {
            ramp = asked(n).opacity.clamp(ramp - cap, ramp + cap);
            assert!((shown.opacity - ramp).abs() < 1e-6, "{fps} fps frame {n}");
        }

        // Dithered: the coverage the screen door presents is the flash.
        let raw: Vec<_> = (0..frames).map(asked).collect();
        assert!(
            over_any(&raw, dt, GhostTransparency::Dither) > 6,
            "{fps} fps: raw"
        );
        let dithered = present_looks_drawn(frames, fps, GhostTransparency::Dither, asked);
        let worst = over_any(&dithered, dt, GhostTransparency::Dither);
        assert!((1..=6).contains(&worst), "{fps} fps: {worst} transitions");
    }
}

// Regression: a dithered ghost fading out under a tint change froze at 9/16
// coverage for as long as the ask held: its screen-door step widened the
// range the next decision could reach, and a hold fallback blocked the
// return. The fall is the exempt ray, which always presents.
#[test]
fn a_dithered_fade_out_reaches_zero_within_250_ms() {
    for fps in RATES {
        let second = fps as usize;
        let shown = present_looks_drawn(3 * second, fps, GhostTransparency::Dither, |n| {
            if n < second {
                look(0.6, ECHO)
            } else {
                look(0.0, [0.3, 0.4, 0.5])
            }
        });
        let gone = second + (0.25 * fps) as usize;
        for (n, shown) in shown.iter().enumerate().skip(gone) {
            assert_eq!(
                screen_door_coverage(shown.opacity),
                0.0,
                "{fps} fps frame {n}: {shown:?}"
            );
        }
    }
}

// With the limiter on, a red tint strobe desaturates over budget,
// keeping luminance. The grey shares the red's luminance, so only redness
// strobes.
#[test]
fn a_red_tint_strobe_desaturates_over_budget_keeping_luminance() {
    let red = [1.0, 0.0, 0.0];
    let grey_of_red = grey(luminance(red));
    for fps in RATES {
        let dt = 1.0 / fps;
        let asked = |n: usize| look(1.0, alternate(10.0, n as f32 * dt, red, grey_of_red));
        let shown = present_looks((3.0 * fps) as usize, fps, asked);
        let mut desaturated = 0;
        for (n, shown) in shown.iter().enumerate() {
            let asked = asked(n);
            assert_eq!(shown.opacity, 1.0, "{fps} fps frame {n}");
            assert!(
                (drawn_luminance(shown.tint) - drawn_luminance(asked.tint)).abs() <= 2.0 / 255.0,
                "{fps} fps frame {n}: {shown:?} for {asked:?}"
            );
            if asked.tint == red && redness(shown.tint) < RED_SATURATED {
                desaturated += 1;
            }
        }
        assert!(
            desaturated > 0,
            "{fps} fps: an over-budget red onset desaturates"
        );
    }
}

/// Each component's share of the spread from the darkest component to the
/// brightest: what scaling and mixing toward white both keep.
fn hue_shares(rgb: [f32; 3]) -> [f32; 3] {
    let max = rgb[0].max(rgb[1]).max(rgb[2]);
    let min = rgb[0].min(rgb[1]).min(rgb[2]);
    rgb.map(|channel| (channel - min) / (max - min))
}

// One red tint change, under budget, keeps its hue angle. From white,
// its luminance falls at the rate cap, so it ramps through lighter reds, each
// red's own hue mixed toward white, and ends at the asked red.
#[test]
fn a_single_red_tint_change_keeps_its_hue_angle() {
    let red = [1.0, 0.0, 0.0];
    let fps = 60.0;
    let shown = present_looks(90, fps, |n| look(1.0, if n < 30 { WHITE } else { red }));
    for (n, shown) in shown.iter().enumerate().skip(30) {
        let spread = shown.tint[0] - shown.tint[1].min(shown.tint[2]);
        if spread > 1e-3 {
            assert_eq!(
                hue_shares(shown.tint),
                [1.0, 0.0, 0.0],
                "frame {n}: {shown:?}"
            );
        }
    }
    assert_eq!(shown.last().map(|look| look.tint), Some(red));
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

// Regression: scaling a saturated tint clipped its components at 1 short of the
// held luminance, which the clamp still recorded as presented.
#[test]
fn tint_at_luminance_draws_exactly_the_held_luminance_for_a_saturated_hue() {
    for tint in [BLUE, EMBER, [1.0, 0.0, 0.0], [0.05, 0.1, 0.4]] {
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

// Over budget the tint keeps its hue: every presented colour is the
// asked colour scaled, within the 8-bit step a held tint draws on, never
// shifted toward another hue.
#[test]
fn a_held_tint_luminance_keeps_the_tints_hue() {
    let fps = 60.0;
    let dt = 1.0 / fps;
    let dim = [0.06, 0.08, 0.1];
    let tints: Vec<_> = (0..180)
        .map(|n| alternate(10.0, n as f32 * dt, dim, ECHO))
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

// A single tint change passes: it reaches its value within the rate
// cap's 250 ms and stays there.
#[test]
fn a_single_tint_change_reaches_its_value_within_the_rate_cap() {
    let fps = 60.0;
    for (from, to) in [(WHITE, BLACK), (BLACK, WHITE), (WHITE, BLUE)] {
        let tints: Vec<_> = (0..90).map(|n| if n < 30 { from } else { to }).collect();
        let presented = present_tints(&tints, fps, Some(true));
        // 250 ms after the change, at 60 Hz: 15 frames.
        for shown in &presented[45..] {
            assert_eq!(shown.tint, to, "{from:?} -> {to:?}: {shown:?}");
        }
    }
}

// Regression: a white/blue strobe drew blue at its own luminance while the
// clamp held a brighter one, so on screen it escaped both the rate cap and
// the flash window. Judged on the luminance drawn: a held or slowed tint draws
// on the 8-bit grid, within a step of the rate cap.
#[test]
fn a_10hz_white_blue_tint_strobe_holds_the_rate_cap_and_flash_window_on_the_drawn_luminance() {
    for fps in RATES {
        let dt = 1.0 / fps;
        let tints: Vec<_> = (0..(3.0 * fps) as usize)
            .map(|n| alternate(10.0, n as f32 * dt, WHITE, BLUE))
            .collect();
        let raw: Vec<_> = tints.iter().map(|&tint| look(1.0, tint)).collect();
        assert!(
            worst_transitions(&luminances(&raw), dt) > 6,
            "{fps} fps: raw"
        );

        let shown = present_tints(&tints, fps, Some(true));
        let drawn = drawn_luminances(&shown);
        let cap = INTENSITY_RATE_PER_SECOND * dt;
        for (n, pair) in drawn.windows(2).enumerate() {
            assert!(
                (pair[1] - pair[0]).abs() <= cap + 2.0 / 255.0,
                "{fps} fps frame {}: drawn luminance {} -> {}",
                n + 1,
                pair[0],
                pair[1]
            );
        }
        let worst = over_any(&shown, dt, GhostTransparency::Blend);
        assert!(worst <= 6, "{fps} fps: {worst} transitions");
    }
}

// Regression: a vanished ghost counted as its tint going black, so quick
// re-entries rate-capped the tint up from black and held the ghost near black.
// Only its opacity falls while it is gone.
#[test]
fn a_ghost_reentering_at_10hz_never_presents_a_dark_tint() {
    for fps in RATES {
        for tint in [ECHO, EMBER] {
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

            // The held tint goes with the history: a ghost back after its
            // window emptied starts fresh against its own tint.
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

// A tint strobe on an invisible ghost (opacity 0) changes nothing on screen
// and spends none of its budget: an opacity strobe after it presents exactly
// as after a steady dark second, with its full three flashes.
#[test]
fn a_tint_strobe_on_an_invisible_ghost_spends_no_budget() {
    for fps in RATES {
        let dt = 1.0 / fps;
        let second = fps as usize;
        let tint_strobe = |n: usize| alternate(10.0, n as f32 * dt, BLACK, WHITE);
        let opacity_strobe = |n: usize| look(alternate(10.0, n as f32 * dt, 0.0, 1.0), WHITE);
        let after_tint_strobe = present_looks(3 * second, fps, |n| {
            if n < second {
                look(0.0, tint_strobe(n))
            } else {
                opacity_strobe(n)
            }
        });
        let after_dark = present_looks(3 * second, fps, |n| {
            if n < second {
                look(0.0, BLACK)
            } else {
                opacity_strobe(n)
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
            worst_transitions(&brightnesses(strobe), dt),
            6,
            "{fps} fps: the opacity strobe's full three flashes"
        );
    }
}

// A ghost's state is fixed-size and cheap to snapshot each frame. Each frame
// copies every ghost's clamp into the unsettled frame's snapshot, and a frame
// whose look does not fit copies it up to fourteen more times while bisecting,
// so this size is the per-ghost cost of every frame. A kilobyte is the budget
// the clamp is designed to; a field that would pass it needs that cost
// weighed, not the bound raised.
#[test]
fn a_ghosts_look_clamp_holds_about_a_kilobyte() {
    let size = std::mem::size_of::<LookValueClamp>();
    assert!(size <= 1024, "{size} bytes per ghost");
}

// Whatever a ghost asks, it presents at most three flashes in any second over
// any background, and its flash window never overflows (a debug assertion on
// every slot it opens). Random looks, hitches, zero-length and vanished
// frames, and frames that never present, at several refresh rates, blended
// and dithered.
#[test]
fn random_looks_present_at_most_three_flashes_over_any_background() {
    let mut state = 0x9E37_79B9_7F4A_7C15u64;
    let mut next = move || {
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        (state >> 40) as f32 / (1u64 << 24) as f32
    };
    let tints = [WHITE, BLACK, [1.0, 0.0, 0.0], grey(0.81), ECHO, EMBER];
    for (fps, transparency) in [30.0f32, 60.0, 144.0, 240.0]
        .into_iter()
        .flat_map(|fps| GhostTransparency::ALL.map(|transparency| (fps, transparency)))
    {
        let mut clamp = GhostLookClamp::<Key>::default();
        clamp.set_transparency(transparency);
        let mut asked = look(0.0, WHITE);
        let (mut opacity, mut level, mut times) = (Vec::new(), Vec::new(), Vec::new());
        let mut now = 0.0f32;
        for _ in 0..(12.0 * fps) as usize {
            if next() < 0.3 {
                let levels = [0.0, 0.2, 0.5, 0.81, 1.0, next()];
                asked.opacity = levels[(next() * 6.0) as usize % 6];
            }
            if next() < 0.3 {
                asked.tint = if next() < 0.2 {
                    [next(), next(), next()]
                } else {
                    tints[(next() * 6.0) as usize % 6]
                };
            }
            let roll = next();
            let dt = if roll < 0.02 {
                0.2
            } else if roll < 0.03 {
                0.0
            } else {
                1.0 / fps
            };
            now += dt;
            clamp.begin_frame(Some(&frame(dt, true)));
            let shown = (next() >= 0.05).then(|| clamp.present(GHOST, asked));
            clamp.end_frame();
            let presented = next() >= 0.05;
            clamp.finish_presentation(presented);
            if let Some(shown) = shown {
                assert!((0.0..=1.0).contains(&shown.opacity), "{shown:?}");
                assert!(
                    shown
                        .tint
                        .iter()
                        .all(|channel| (0.0..=1.0).contains(channel)),
                    "{shown:?}"
                );
            }
            if presented {
                let drawn = shown.map_or((0.0, 0.0), |shown| {
                    (
                        drawn_opacity(transparency)(shown.opacity),
                        drawn_luminance(shown.tint),
                    )
                });
                opacity.push(drawn.0);
                level.push(drawn.1);
                times.push(now);
            }
        }
        let worst = worst_transitions_over_any_background_at(&opacity, &level, &times);
        assert!(
            worst <= 6,
            "{fps} fps {transparency:?}: {worst} transitions"
        );
    }
}

// Regression: a ghost appearing presented its look uncounted, so the changes
// right after it had the whole budget, and over a background where appearing
// was itself a flash they drew a seventh transition in the second.
#[test]
fn a_ghost_appearing_counts_its_appearance() {
    let dt = 1.0 / 30.0;
    let red = [1.0, 0.0, 0.0];
    let gone: Option<(f32, [f32; 3])> = None;
    let mut steps = vec![
        (gone, dt),
        (Some((1.0, ECHO)), dt),
        (gone, dt),
        (Some((1.0, ECHO)), dt),
        (Some((1.0, ECHO)), dt),
        (Some((1.0, grey(0.81))), dt),
        (Some((0.0, grey(0.81))), dt),
        (Some((0.0, grey(0.81))), dt),
        (Some((0.0, ECHO)), dt),
        (Some((0.0, ECHO)), dt),
        (Some((0.0, ECHO)), dt),
        (Some((0.2, WHITE)), dt),
        (Some((0.685, WHITE)), dt),
        (Some((0.685, WHITE)), dt),
        (Some((0.5, WHITE)), dt),
        (Some((0.5, WHITE)), dt),
        (Some((0.5, grey(0.81))), dt),
        (Some((0.5, grey(0.81))), dt),
        // A hitch.
        (Some((0.5, BLACK)), 0.2),
        (Some((0.435, BLACK)), dt),
        (Some((0.435, BLACK)), dt),
        (Some((0.435, red)), dt),
        (Some((0.435, red)), dt),
        (Some((0.829, red)), dt),
        (gone, dt),
    ];
    steps.extend((0..30).map(|_| (Some((0.829, red)), dt)));

    let mut clamp = GhostLookClamp::default();
    let (mut opacity, mut level, mut times) = (Vec::new(), Vec::new(), Vec::new());
    let mut now = 0.0f32;
    for (asked, dt) in steps {
        now += dt;
        clamp.begin_frame(Some(&frame(dt, true)));
        let shown = asked.map(|(asked, tint)| clamp.present(GHOST, look(asked, tint)));
        clamp.end_frame();
        let drawn = shown.map_or((0.0, 0.0), |shown| {
            (shown.opacity, drawn_luminance(shown.tint))
        });
        opacity.push(drawn.0);
        level.push(drawn.1);
        times.push(now);
    }
    let worst = worst_transitions_over_any_background_at(&opacity, &level, &times);
    assert!(worst <= 6, "{worst} transitions");

    // Appearing visibly counts; appearing at nothing draws no change, so it
    // counts nothing.
    let appearing = |opacity| {
        LookValueClamp::appearing(AskedLook {
            look: look(opacity, WHITE),
            transparency: GhostTransparency::Blend,
        })
    };
    assert!(!appearing(1.0).window.is_empty());
    assert!(appearing(0.0).window.is_empty());
}

// Regression: an edge point counts a swing at less than the flash threshold,
// so over a background between edge samples a later creep could complete the
// flash after that count had aged out, and the strobe after it then had the
// whole budget and drew a seventh transition there.
#[test]
fn a_creep_completing_an_old_swing_counts_against_the_budget() {
    for fps in RATES {
        let dt = 1.0 / fps;
        let second = fps as usize;
        let (held, crept) = (3 * second, 3 * second + second / 10);
        let asked = |n: usize| {
            let opacity = if n < held / 2 {
                0.5
            } else if n < held {
                0.399
            } else if n < crept {
                0.396
            } else {
                alternate(10.0, (n - crept) as f32 * dt, 1.0, 0.0)
            };
            look(opacity, WHITE)
        };
        let shown = present_looks(crept + 2 * second, fps, asked);
        // A white ghost over a background of 0.025, between the first two
        // edge samples: 0.396 completes the flash from 0.5 there.
        let at_point = transitions_over(
            1.0,
            0.025,
            &opacities(&shown),
            &drawn_luminances(&shown),
            dt,
        );
        assert!(at_point <= 6, "{fps} fps: {at_point} transitions");
        let worst = over_any(&shown, dt, GhostTransparency::Blend);
        assert!(worst <= 6, "{fps} fps: {worst} transitions");
    }
}

/// An 8-bit grey.
fn grey_byte(byte: u8) -> [f32; 3] {
    [f32::from(byte) / 255.0; 3]
}

fn rgb_bytes(rgb: [u8; 3]) -> [f32; 3] {
    rgb.map(|channel| f32::from(channel) / 255.0)
}

/// Ask `asked` for `seconds` at `fps`, at least one frame.
fn hold(looks: &mut Vec<GhostInstanceLook>, asked: GhostInstanceLook, seconds: f32, fps: f32) {
    let frames = ((seconds * fps).round() as usize).max(1);
    looks.extend(std::iter::repeat_n(asked, frames));
}

/// What the undrawn-corner and between-samples attacks ask after their
/// crossing look: four tint swings to `bright` and back, the crossing look
/// held until a second after frame `first` (when the slot its first change
/// refreshed last ages out), one more swing, then the exempt fall to nothing. Had the crossing gone
/// uncounted, the point it crossed at draws a seventh transition in the
/// second.
fn strobe_after_crossing(
    looks: &mut Vec<GhostInstanceLook>,
    crossing: GhostInstanceLook,
    bright: [f32; 3],
    first: usize,
    fps: f32,
) {
    let swing = look(crossing.opacity, bright);
    for i in 0..4 {
        hold(looks, if i % 2 == 0 { swing } else { crossing }, 0.1, fps);
    }
    let until = first + fps as usize + 1;
    if looks.len() < until {
        looks.extend(std::iter::repeat_n(crossing, until - looks.len()));
    }
    hold(looks, swing, 0.06, fps);
    hold(looks, look(0.0, bright), 0.5, fps);
}

/// The opacity step of the undrawn-corner attack's judged frame: just under
/// one frame's rate cap at `fps`, and at most 0.06.
fn rate_step(fps: f32) -> f32 {
    let cap = INTENSITY_RATE_PER_SECOND * (1.0 / fps).min(HITCH_CEILING_SECONDS);
    0.06f32.min(0.95 * cap)
}

/// The undrawn-corner attack from opacity `from` at grey `start`: a darker
/// grey `low` just short of a flash at a white ghost over black, then one
/// frame asking the opacity a rate step down while the tint rises to `up`,
/// which keeps what draws there from falling. The corner of that frame's
/// segment at the lower opacity and the held grey is never drawn; asking it
/// next crosses the flash threshold over black.
fn corner_attack_with(fps: f32, from: f32, start: u8, low: u8, up: u8) -> Vec<GhostInstanceLook> {
    let lowered = from - rate_step(fps);
    let mut looks = Vec::new();
    hold(&mut looks, look(from, grey_byte(start)), 2.0, fps);
    hold(&mut looks, look(from, grey_byte(low)), 0.1, fps);
    let first = looks.len();
    hold(&mut looks, look(lowered, grey_byte(up)), 0.3, fps);
    let crossing = look(lowered, grey_byte(low));
    hold(&mut looks, crossing, 0.1, fps);
    strobe_after_crossing(&mut looks, crossing, grey_byte(230), first, fps);
    looks
}

/// [`corner_attack_with`] with its greys found from `start`: the lightest
/// grey short of a flash over black at `from` that is one past it a rate
/// step lower, and the grey keeping that lower opacity from falling.
fn corner_attack(fps: f32, from: f32, start: u8) -> Option<Vec<GhostInstanceLook>> {
    let level = |byte: u8| drawn_luminance(grey_byte(byte));
    let lowered = from - rate_step(fps);
    let lit = from * level(start);
    let low = (0..start).rev().find(|&byte| {
        lit - from * level(byte) < 0.1 && lit - lowered * level(byte) >= 0.1 + 1e-4
    })?;
    let up = (low..=255).find(|&byte| lowered * level(byte) >= from * level(low))?;
    Some(corner_attack_with(fps, from, start, low, up))
}

/// The 8-bit tint near grey whose drawn luminance is nearest `level`.
fn tint_near(level: f32) -> [f32; 3] {
    let centre = (level * 255.0).round() as i32;
    let mut best = (f32::MAX, [0.0; 3]);
    for g in centre - 3..=centre + 3 {
        for r in g - 12..=g + 12 {
            for b in g - 12..=g + 12 {
                if [r, g, b].iter().any(|channel| !(0..=255).contains(channel)) {
                    continue;
                }
                let tint = [r, g, b].map(|channel| channel as f32 / 255.0);
                let error = (drawn_luminance(tint) - level).abs();
                if error < best.0 {
                    best = (error, tint);
                }
            }
        }
    }
    best.1
}

/// The between-samples attack on a white ghost over background B = (k +
/// `lambda`) / 16, between edge samples k/16 and (k + 1)/16, k = `segment`.
/// Two looks each take one sample to its low, 2e-5 under where the crossing
/// look draws it, at greys `dim_by` either side of the crossing's, so their
/// opacities straddle it; the point between draws each above the crossing.
/// The ghost starts 0.1 above the crossing there and under 0.1 above both
/// lows, so the crossing look completes the flash while neither sample
/// passes its low.
fn between_attack(
    fps: f32,
    segment: usize,
    lambda: f32,
    dim_by: u8,
) -> Option<Vec<GhostInstanceLook>> {
    const UNDER: f32 = 2e-5;
    let (opacity, grey) = (0.6f32, 100u8);
    let (below, above) = (segment as f32 / 16.0, (segment + 1) as f32 / 16.0);
    let level = |byte: u8| drawn_luminance(grey_byte(byte));
    // What a white ghost draws over `background`, and the opacity drawing
    // `target` there.
    let draws =
        |opacity: f32, tint: f32, background: f32| background + opacity * (tint - background);
    let opacity_for = |target: f32, tint: f32, background: f32| {
        let reach = tint - background;
        (reach.abs() >= 1e-6).then(|| (target - background) / reach)
    };
    let crossing_level = level(grey);
    let (lighter, darker) = (level(grey + dim_by), level(grey - dim_by));
    let low_below = opacity_for(
        draws(opacity, crossing_level, below) - UNDER,
        lighter,
        below,
    )?;
    let low_above = opacity_for(draws(opacity, crossing_level, above) - UNDER, darker, above)?;
    if !(0.0..1.0).contains(&low_below) || !(0.0..1.0).contains(&low_above) {
        return None;
    }
    let between = |opacity: f32, tint: f32| {
        (1.0 - lambda) * draws(opacity, tint, below) + lambda * draws(opacity, tint, above)
    };
    let crossed = between(opacity, crossing_level);
    let lows = between(low_below, lighter).min(between(low_above, darker));
    if lows <= crossed {
        return None;
    }
    let start = 0.1 + crossed + 0.3 * (lows - crossed);
    let background = below + lambda / 16.0;
    let start_level = (start - background * (1.0 - low_above)) / low_above;
    if !(0.0..1.0).contains(&start_level) {
        return None;
    }
    let mut looks = Vec::new();
    hold(
        &mut looks,
        look(low_above, tint_near(start_level)),
        2.0,
        fps,
    );
    hold(
        &mut looks,
        look(low_above, grey_byte(grey - dim_by)),
        0.05,
        fps,
    );
    let first = looks.len();
    hold(
        &mut looks,
        look(low_below, grey_byte(grey + dim_by)),
        0.3,
        fps,
    );
    let crossing = look(opacity, grey_byte(grey));
    hold(&mut looks, crossing, 0.1, fps);
    strobe_after_crossing(&mut looks, crossing, grey_byte(190), first, fps);
    Some(looks)
}

/// The most transitions over any background of the grid of what `asked`
/// presents through one ghost at `fps`, drawn with `transparency`.
fn presented_worst(
    asked: &[GhostInstanceLook],
    fps: f32,
    transparency: GhostTransparency,
) -> usize {
    let shown = present_looks_drawn(asked.len(), fps, transparency, |n| asked[n]);
    over_any(&shown, 1.0 / fps, transparency)
}

/// Whether what `asked` presents through one ghost at `fps`, blended, draws
/// a transition with `albedo` over `background` on frame `n`: an attack's
/// crossing look presents its flash rather than being held back.
fn presents_a_flash_on(
    asked: &[GhostInstanceLook],
    fps: f32,
    (albedo, background): (f32, f32),
    n: usize,
) -> bool {
    let shown = present_looks(asked.len(), fps, |frame| asked[frame]);
    transition_frames_over(
        albedo,
        background,
        &opacities(&shown),
        &drawn_luminances(&shown),
    )
    .contains(&n)
}

// Regression: on a frame asking opacity down and tint up, the clamp judged
// the corner of its segment at the lower opacity and the held tint, which
// never drew, so the trackers' peaks sat where nothing presented. Asking that
// corner later completed a flash over black that reached no tracker's peak,
// so the slot it belonged to aged out early and the strobe after it drew a
// seventh transition in the second. Only the look that presents is judged.
#[test]
fn a_flash_completed_at_the_corner_a_frame_judged_but_never_drew_is_counted() {
    for (fps, low, up) in [(30.0, 103, 112), (60.0, 103, 112), (240.0, 97, 99)] {
        let asked = corner_attack_with(fps, 0.79, 128, low, up);
        // The crossing look follows two seconds at the start grey, a tenth
        // at the low one and the judged frame's 0.3 s.
        let crossed: usize = [2.0f32, 0.1, 0.3]
            .iter()
            .map(|seconds| (seconds * fps).round() as usize)
            .sum();
        assert!(
            presents_a_flash_on(&asked, fps, (1.0, 0.0), crossed),
            "{fps} fps: the crossing completes a flash over black"
        );
        for transparency in GhostTransparency::ALL {
            let worst = presented_worst(&asked, fps, transparency);
            assert!(
                worst <= 6,
                "{fps} fps {transparency:?}: {worst} transitions"
            );
        }
    }
}

// Regression: over the background 0.025, between the edge samples 0 and
// 1/16, one look took the sample at 1/16 to its low, the next the sample at 0,
// and the crossing look drew the point between 0.1 under where the ghost
// started while each sample sat 2e-5 short of its low. No tracker passed its
// peak there, so the crossing was never counted again and the strobe after it
// drew a seventh transition in the second at (1, 0.025).
#[test]
fn a_flash_completed_between_edge_samples_is_counted() {
    let (opacity, low_below, low_above) = (0.6, 0.576874, 0.62991285);
    for fps in [30.0, 60.0, 144.0] {
        let mut asked = Vec::new();
        hold(
            &mut asked,
            look(low_above, rgb_bytes([126, 138, 148])),
            2.0,
            fps,
        );
        hold(&mut asked, look(low_above, grey_byte(96)), 0.05, fps);
        let first = asked.len();
        hold(&mut asked, look(low_below, grey_byte(104)), 0.3, fps);
        let crossing = look(opacity, grey_byte(100));
        let crossed = asked.len();
        hold(&mut asked, crossing, 0.1, fps);
        strobe_after_crossing(&mut asked, crossing, grey_byte(190), first, fps);
        assert!(
            presents_a_flash_on(&asked, fps, (1.0, 0.025), crossed),
            "{fps} fps: the crossing completes a flash between the samples"
        );
        let worst = presented_worst(&asked, fps, GhostTransparency::Blend);
        assert!(worst <= 6, "{fps} fps: {worst} transitions");
    }
}

// The sample nearer a background between samples may turn back by just over
// the edge threshold, a swing the point between never sees, and then return
// near the low it turned from while the point between reaches a new low and
// completes a flash. Over the background 0.025 here the sample at 0 falls,
// rises 0.069, and sits 3e-4 above its low as the point between crosses; the
// sample at 1/16 sits above an earlier, lower look. Counted as a transition
// of the old swing, the strobe after it stays within six.
#[test]
fn a_flash_completed_past_a_samples_small_turn_back_is_counted() {
    let start = look(0.61, rgb_bytes([160, 169, 177]));
    let low = look(0.56, rgb_bytes([146, 135, 125]));
    let lower_above = look(0.61, rgb_bytes([116, 128, 134]));
    let turned = look(0.61, rgb_bytes([160, 153, 149]));
    let crossing = look(0.58, rgb_bytes([122, 134, 142]));
    for fps in [30.0, 60.0, 144.0] {
        let mut asked = Vec::new();
        hold(&mut asked, start, 2.0, fps);
        hold(&mut asked, low, 0.05, fps);
        let first = asked.len();
        for step in [lower_above, turned, crossing] {
            hold(&mut asked, step, 0.1, fps);
        }
        strobe_after_crossing(&mut asked, crossing, grey_byte(200), first, fps);
        let worst = presented_worst(&asked, fps, GhostTransparency::Blend);
        assert!(worst <= 6, "{fps} fps: {worst} transitions");
    }
}

// A targeted search over the undrawn-corner and between-samples attacks,
// which random looks cannot find: flashes tuned to complete 2e-5 past the
// threshold between edge samples, over each of the first four background
// segments at every grid background inside it, and at the corner a frame
// judges but never draws, from several opacities and greys, at several
// rates, blended and dithered. Every one presents at most three flashes over any background.
#[test]
fn targeted_attacks_on_the_edge_samples_present_at_most_three_flashes_over_any_background() {
    let mut built = 0;
    for fps in [30.0, 60.0, 144.0] {
        for segment in 0..4 {
            let (below, above) = (segment as f32 / 16.0, (segment + 1) as f32 / 16.0);
            let inside = (1..40)
                .map(|step| step as f32 / 40.0)
                .filter(|&background| below < background && background < above);
            for background in inside {
                let lambda = (background - below) * 16.0;
                for dim_by in [2, 4] {
                    let Some(asked) = between_attack(fps, segment, lambda, dim_by) else {
                        continue;
                    };
                    built += 1;
                    let worst = presented_worst(&asked, fps, GhostTransparency::Blend);
                    assert!(
                        worst <= 6,
                        "between samples over {background}, {dim_by} greys, {fps} fps: {worst}"
                    );
                }
            }
        }
    }
    for fps in [30.0, 60.0, 240.0] {
        for from in [0.6, 0.79] {
            for start in [100, 128, 170] {
                let Some(asked) = corner_attack(fps, from, start) else {
                    continue;
                };
                built += 1;
                for transparency in GhostTransparency::ALL {
                    let worst = presented_worst(&asked, fps, transparency);
                    assert!(
                        worst <= 6,
                        "corner from {from} at grey {start}, {fps} fps {transparency:?}: {worst}"
                    );
                }
            }
        }
    }
    assert!(built >= 40, "the search built {built} attacks");
}

// A ghost already presenting when the limiter turns on, or on a frame that
// starts history afresh, starts against its look and spends no slot; one
// appearing on a later frame counts its appearance. Both present as asked.
#[test]
fn a_ghost_presenting_as_history_starts_spends_no_slot_and_a_later_one_counts_its_appearance() {
    let dt = 1.0 / 60.0;
    let other: Key = (1, 7);
    let restart = pack_limiter_frame(
        LimiterFrameInput {
            elapsed_seconds: dt,
        },
        true,
        true,
    );
    // The limiter off, then on; or on, then a frame that starts history.
    for (before, starts) in [
        (frame(dt, false), frame(dt, true)),
        (frame(dt, true), restart),
    ] {
        let mut clamp = GhostLookClamp::default();
        for _ in 0..30 {
            clamp.begin_frame(Some(&before));
            clamp.present(GHOST, look(1.0, WHITE));
            clamp.end_frame();
            clamp.finish_presentation(true);
        }
        clamp.begin_frame(Some(&starts));
        let shown = clamp.present(GHOST, look(1.0, WHITE));
        clamp.end_frame();
        clamp.finish_presentation(true);
        assert_eq!(shown, look(1.0, WHITE));
        let presenting = clamp.look_clamp(&GHOST).expect("limited");
        assert!(presenting.window.is_empty(), "{starts:?}: spends no slot");

        clamp.begin_frame(Some(&frame(dt, true)));
        clamp.present(GHOST, look(1.0, WHITE));
        let appeared = clamp.present(other, look(1.0, WHITE));
        clamp.end_frame();
        assert_eq!(appeared, look(1.0, WHITE));
        let appearing = clamp.look_clamp(&other).expect("limited");
        assert!(!appearing.window.is_empty(), "{starts:?}: appearing counts");
    }
}

/// What `asked` presents through one ghost at `fps` drawn with
/// `transparency`, a `None` frame presenting nothing (the ghost vanished):
/// each frame's presented look, if any, and whether the ghost had no clamp as
/// the frame began, never seen or dropped at rest.
fn present_tracking_drops(
    asked: &[Option<GhostInstanceLook>],
    fps: f32,
    transparency: GhostTransparency,
) -> Vec<(Option<GhostInstanceLook>, bool)> {
    let step = frame(1.0 / fps, true);
    let mut clamp = GhostLookClamp::default();
    clamp.set_transparency(transparency);
    asked
        .iter()
        .map(|asked| {
            let unseen = clamp.look_clamp(&GHOST).is_none();
            clamp.begin_frame(Some(&step));
            let shown = asked.map(|asked| clamp.present(GHOST, asked));
            clamp.end_frame();
            (shown, unseen)
        })
        .collect()
}

/// The most transitions over any background of the grid of what `asked`
/// presents through one ghost at `fps`, drawn with `transparency`, a frame
/// presenting nothing drawing no ghost; and whether the ghost was dropped at
/// rest on the way.
fn presented_worst_with_drops(
    asked: &[Option<GhostInstanceLook>],
    fps: f32,
    transparency: GhostTransparency,
) -> (usize, bool) {
    let presented = present_tracking_drops(asked, fps, transparency);
    let dropped = presented.iter().skip(1).any(|&(_, unseen)| unseen);
    let drawn: Vec<GhostInstanceLook> = presented
        .iter()
        .map(|&(shown, _)| shown.unwrap_or(look(0.0, BLACK)))
        .collect();
    (over_any(&drawn, 1.0 / fps, transparency), dropped)
}

/// Assert `asked`, which drops its ghost at rest on the way, presents at most
/// three flashes a second over every background, blended and dithered.
fn assert_limited_across_a_drop(name: &str, asked: &[Option<GhostInstanceLook>], fps: f32) {
    for transparency in GhostTransparency::ALL {
        let (worst, dropped) = presented_worst_with_drops(asked, fps, transparency);
        assert!(
            dropped,
            "{name} {transparency:?}: the ghost rests and is dropped"
        );
        assert!(worst <= 6, "{name} {transparency:?}: {worst} transitions");
    }
}

// Regression: a ghost dropped at rest came back fresh, so it forgot the swing
// it was in. Drawing 0.099 over a background before it vanished (no
// transition), dropped after resting 1.5 s, it reappeared so faintly that no
// edge point counted, which completed the old swing there, and the opacity
// strobe after it had the whole budget: a seventh transition in the second.
// Back with unknown history, every edge point counts its first move.
#[test]
fn a_ghost_dropped_at_rest_counts_the_swing_its_reappearance_completes() {
    for fps in [30.0f32, 60.0, 144.0] {
        let frames = |seconds: f32| ((seconds * fps).round() as usize).max(1);
        let mut asked = vec![Some(look(0.198, WHITE)); frames(1.0)];
        asked.extend(vec![None; frames(1.5)]);
        asked.extend(vec![Some(look(0.004, BLACK)); frames(0.05)]);
        for i in 0..6 {
            let opacity = if i % 2 == 0 { 0.5 } else { 0.004 };
            asked.extend(vec![Some(look(opacity, WHITE)); frames(1.0 / 12.0)]);
        }
        asked.extend(vec![Some(look(0.004, WHITE)); frames(0.5)]);
        assert_limited_across_a_drop("reappearing faintly", &asked, fps);
    }

    // However faint, an appearance counts.
    let faint = LookValueClamp::appearing(AskedLook {
        look: look(0.004, BLACK),
        transparency: GhostTransparency::Blend,
    });
    assert!(!faint.window.is_empty(), "a faint appearance counts");
}

// Ordinary looks after a drop are never held either. A ghost back after its
// clamp was dropped at rest presents its first frame as asked, as any
// appearance does, then the asked look under the rate cap from what presented
// last, though its appearance and each edge point's first move count: arming
// and disarming, re-entering twice in a second, the echo steps, fades, a white
// or black arm then a tint change, a faint start, arming twice, and re-arming
// after a second rest, blended and dithered.
#[test]
fn ordinary_looks_after_a_drop_present_as_asked_within_the_rate_cap() {
    let lerp = |a: f32, b: f32, t: f32| a + (b - a) * t.min(1.0);
    let mix = |a: [f32; 3], b: [f32; 3], t: f32| -> [f32; 3] {
        std::array::from_fn(|i| lerp(a[i], b[i], t))
    };
    for fps in RATES {
        let dt = 1.0 / fps;
        let frames = |seconds: f32| (seconds * fps).round() as usize;
        let steady = |opacity: f32, tint: [f32; 3], seconds: f32| {
            vec![Some(look(opacity, tint)); frames(seconds)]
        };
        let ramp = |from: f32, to: f32, tint: [f32; 3]| -> Vec<Option<GhostInstanceLook>> {
            (0..frames(0.5))
                .map(|n| Some(look(lerp(from, to, n as f32 * dt / 0.25), tint)))
                .collect()
        };
        let fade = |from: (f32, [f32; 3]), to: (f32, [f32; 3])| -> Vec<Option<GhostInstanceLook>> {
            let mut looks = steady(from.0, from.1, 0.2);
            looks.extend((0..frames(1.5)).map(|n| {
                let t = n as f32 * dt / 1.5;
                Some(look(lerp(from.0, to.0, t), mix(from.1, to.1, t)))
            }));
            looks.extend(steady(to.0, to.1, 0.5));
            looks
        };
        let after_drop = |parts: &[Vec<Option<GhostInstanceLook>>]| {
            let mut looks = steady(0.6, ECHO, 0.3);
            looks.extend(vec![None; frames(1.5)]);
            for part in parts {
                looks.extend(part.iter().copied());
            }
            looks
        };
        let arm = [ramp(0.0, 0.6, ECHO), ramp(0.6, 0.0, ECHO)].concat();
        let mut re_entry = steady(0.6, ECHO, 0.3);
        for _ in 0..2 {
            re_entry.extend([None; 3]);
            re_entry.extend(steady(0.6, ECHO, 0.3));
        }
        let mut steps = Vec::new();
        for opacity in [0.45, 0.6, 0.45, 0.6] {
            steps.extend(steady(opacity, ECHO, 0.3));
        }
        let rest = vec![None; frames(1.5)];
        let cases = [
            ("arm and disarm", after_drop(std::slice::from_ref(&arm))),
            ("re-entry twice", after_drop(&[re_entry])),
            ("echo steps", after_drop(&[steps.clone()])),
            (
                "fade 0.45 → 1, echo → ember",
                after_drop(&[fade((0.45, ECHO), (1.0, EMBER))]),
            ),
            (
                "fade 1 → 0.2, white → black",
                after_drop(&[fade((1.0, WHITE), (0.2, BLACK))]),
            ),
            (
                "white arm, then ember",
                after_drop(&[
                    ramp(0.0, 0.8, WHITE),
                    steady(0.8, WHITE, 0.3),
                    steady(0.8, EMBER, 0.3),
                ]),
            ),
            (
                "black arm, then echo",
                after_drop(&[
                    ramp(0.0, 0.8, BLACK),
                    steady(0.8, BLACK, 0.3),
                    steady(0.8, ECHO, 0.3),
                ]),
            ),
            (
                "faint, then steps",
                after_drop(&[
                    steady(0.03, ECHO, 0.3),
                    steady(0.6, ECHO, 0.6),
                    steady(0.45, ECHO, 0.3),
                ]),
            ),
            ("arm twice", after_drop(&[arm.clone(), arm.clone()])),
            (
                "re-armed after a second rest",
                after_drop(&[arm.clone(), rest, arm, steps]),
            ),
        ];
        for transparency in GhostTransparency::ALL {
            for (name, asked) in &cases {
                let presented = present_tracking_drops(asked, fps, transparency);
                let mut last = None;
                for (n, (asked, &(shown, unseen))) in asked.iter().zip(&presented).enumerate() {
                    let what = format!("{name}, {transparency:?} at {fps} fps, frame {n}");
                    let (Some(asked), Some(shown)) = (asked, shown) else {
                        last = last.map(|(_, level)| (0.0, level));
                        continue;
                    };
                    let asked_drawn = (asked.opacity, drawn_luminance(asked.tint));
                    // A ghost with no clamp appears as asked.
                    let from = if unseen {
                        asked_drawn
                    } else {
                        last.unwrap_or(asked_drawn)
                    };
                    let (expected, capped) =
                        rate_capped(from, asked_drawn, INTENSITY_RATE_PER_SECOND * dt);
                    if capped {
                        assert!(
                            (shown.opacity - expected.0).abs() < 1e-5,
                            "{what}: {shown:?}"
                        );
                        let level = drawn_luminance(shown.tint);
                        assert!(
                            (level - expected.1).abs() <= 1.0 / 255.0,
                            "{what}: {shown:?}"
                        );
                    } else {
                        assert_eq!(shown, *asked, "{what}");
                    }
                    last = Some((shown.opacity, drawn_luminance(shown.tint)));
                }
            }
        }
    }
}

/// The aimed-strobe attack across a drop, at (1, `background`). Before the
/// drop a faint white ghost draws there 0.1 + 3e-4 above where the crossing
/// look will, and vanishing leaves a fall short of a flash. Back black and
/// faint: a first move, a frame back near the rest held `gap` seconds, then
/// a black crossing look at opacity 0.0606, held long enough to reach under
/// the rate cap, completing the old fall while no edge point moves as far as
/// the edge threshold. Then a tint strobe aimed there at opacity `strobe`
/// between greys `bright` and `dim`, `hold` seconds a look: four changes,
/// the dim look held until just past a second after the first move (when
/// that move's slot ages out), one more change, and the exempt fall to
/// nothing. Returns the looks and the crossing look's frames.
fn aimed_strobe_after_a_drop(
    fps: f32,
    background: f32,
    gap: f32,
    (strobe, bright, dim): (f32, f32, f32),
    hold: f32,
) -> (Vec<Option<GhostInstanceLook>>, Range<usize>) {
    let crossing_opacity = 0.0606;
    let crossed_at = background * (1.0 - crossing_opacity);
    let before = (crossed_at + 0.1 + 3e-4 - background) / (1.0 - background);
    let frames = |seconds: f32| ((seconds * fps).round() as usize).max(1);
    let second = fps.round() as usize;
    let mut asked = vec![Some(look(before, WHITE)); second];
    asked.extend(vec![None; (1.5 * fps) as usize]);
    let first = asked.len();
    asked.push(Some(look(0.004, BLACK)));
    asked.extend(vec![Some(look(0.0005, BLACK)); 1 + (gap * fps) as usize]);
    let crossing = asked.len()..asked.len() + frames(0.025);
    asked.extend(vec![Some(look(crossing_opacity, BLACK)); crossing.len()]);
    let (lit, dimmed) = (look(strobe, grey(bright)), look(strobe, grey(dim)));
    for strobed in [lit, dimmed, lit, dimmed] {
        asked.extend(vec![Some(strobed); frames(hold)]);
    }
    if asked.len() < first + second + 2 {
        asked.resize(first + second + 2, Some(dimmed));
    }
    asked.extend(vec![Some(lit); frames(hold)]);
    asked.extend(vec![Some(look(0.0, grey(bright))); frames(0.5)]);
    (asked, crossing)
}

/// Whether what `asked` presents through one ghost at `fps`, blended, a
/// frame presenting nothing drawing no ghost, draws a transition with
/// `albedo` over `background` on one of `frames`: an attack's crossing look
/// presents its flash rather than being held back.
fn presents_a_flash_across_a_drop_on(
    asked: &[Option<GhostInstanceLook>],
    fps: f32,
    (albedo, background): (f32, f32),
    frames: Range<usize>,
) -> bool {
    let drawn: Vec<GhostInstanceLook> = present_sequence(asked, fps, GhostTransparency::Blend)
        .into_iter()
        .map(|shown| shown.unwrap_or(look(0.0, BLACK)))
        .collect();
    transition_frames_over(
        albedo,
        background,
        &opacities(&drawn),
        &drawn_luminances(&drawn),
    )
    .into_iter()
    .any(|frame| frames.contains(&frame))
}

// Regression: back with unknown history, an edge sample's faint first move
// was counted and then its swing back near the rest, after which moving on
// past that first move's peak counted nothing. A hidden fall completing
// there restarted no slot on its frame, and a tint strobe aimed at the
// point, timed to finish after the first move's slot aged out, drew a
// seventh transition in the second at (1, 0.025). Passing the peak again
// carries the first move's count on.
#[test]
fn a_flash_completed_past_a_faint_first_moves_peak_after_a_drop_is_counted() {
    for fps in [30.0, 60.0, 144.0, 240.0] {
        let (asked, crossing) = aimed_strobe_after_a_drop(fps, 0.05, 0.6, (0.3, 0.8, 0.2), 0.08);
        assert!(
            presents_a_flash_across_a_drop_on(&asked, fps, (1.0, 0.05), crossing),
            "{fps} fps: the crossing completes the hidden fall"
        );
        assert_limited_across_a_drop(&format!("aimed strobe, {fps} fps"), &asked, fps);
    }
}

// Back with unknown history, the sample nearer (1, 0.475) counts its faint
// first move and its swing back near the rest, then dips past the rest away
// from that move's peak, under the edge threshold, on the frame the point
// completes the fall the drop hid. That frame passes no peak, so it restarts
// no slot. The tint strobe after it moves such samples past their faint first
// moves' peaks, carrying those counts on and restarting the appearance's
// slot, which holds back a seventh transition in the second there; without
// that carry-on the strobe draws seven.
#[test]
fn a_flash_completed_as_a_sample_dips_past_its_rest_after_a_drop_is_limited() {
    let fps = 60.0;
    let held = |asked: GhostInstanceLook, frames: usize| vec![Some(asked); frames];
    let mut asked = held(look(0.183477, WHITE), 60);
    asked.extend(vec![None; 90]);
    asked.extend(held(look(0.039361, grey(0.787879)), 1));
    asked.extend(held(look(0.066723, grey(0.492562)), 1));
    asked.extend(held(look(0.026117, grey(0.460099)), 51));
    let crossing = asked.len();
    asked.extend(held(look(0.022045, grey(0.160281)), 1));
    for tint in [WHITE, BLACK, WHITE, BLACK, WHITE] {
        asked.extend(held(look(0.5, tint), 5));
    }
    asked.extend(held(look(0.0, WHITE), 30));
    assert!(
        presents_a_flash_across_a_drop_on(&asked, fps, (1.0, 0.475), crossing..crossing + 1),
        "the crossing completes the hidden fall"
    );
    assert_limited_across_a_drop("a dip past the rest", &asked, fps);
}

/// The still-sample attack across a drop: a black tint keeps the edge point
/// (1, 0) still while (1, 1/16) moves. Before the drop a faint white ghost
/// draws 0.0995 above the background 0.025, nearer the still sample. Back
/// black and faint, it falls a little there, rises back for `gap` seconds,
/// then falls on to complete the old fall; then an opacity strobe at `hz` up
/// to `high`.
fn still_sample_after_a_drop(
    fps: f32,
    gap: f32,
    hz: f32,
    high: f32,
) -> Vec<Option<GhostInstanceLook>> {
    let background = 0.025;
    let dt = 1.0 / fps;
    let start = (0.1245 - background) / (1.0 - background);
    let mut asked = vec![Some(look(start, WHITE)); fps as usize];
    asked.extend(vec![None; (1.5 * fps) as usize]);
    asked.push(Some(look(0.01, BLACK)));
    asked.extend(vec![
        Some(look(0.005, BLACK));
        ((gap * fps) as usize).max(1)
    ]);
    asked.extend(vec![Some(look(0.03, BLACK)); 2]);
    asked.extend(
        (0..(1.5 * fps) as usize)
            .map(|n| Some(look(alternate(hz, n as f32 * dt, high, 0.0), WHITE))),
    );
    asked
}

// Targeted attacks across a drop, which random looks cannot find, on the two
// cases the argument across a drop leaves to search. Aimed strobe: a sample's
// faint first move, its swing back near the rest, and an old swing completing
// at a point between samples, then a tint strobe aimed there, at several
// backgrounds, rests and strobes; each crossing presents its flash. Still
// sample: an old swing completing beside a sample the tint keeps still.
// Every one presents at most three flashes over any background, blended and
// dithered.
#[test]
fn targeted_attacks_across_a_drop_present_at_most_three_flashes_over_any_background() {
    let mut built = 0;
    for fps in [30.0, 60.0, 144.0, 240.0] {
        for background in [0.025, 0.05, 0.075] {
            for gap in [0.5, 0.75] {
                for strobe in [(0.3, 0.8, 0.2), (0.5, 0.6, 0.1)] {
                    let (asked, crossing) =
                        aimed_strobe_after_a_drop(fps, background, gap, strobe, 0.08);
                    let name =
                        format!("aimed strobe over {background} {gap} {strobe:?}, {fps} fps");
                    assert!(
                        presents_a_flash_across_a_drop_on(&asked, fps, (1.0, background), crossing),
                        "{name}: the crossing completes the hidden fall"
                    );
                    assert_limited_across_a_drop(&name, &asked, fps);
                    built += 1;
                }
            }
        }
    }
    for fps in [30.0, 60.0] {
        for gap in [0.2, 0.5] {
            for hz in [6.0, 10.0] {
                for high in [0.3, 1.0] {
                    let asked = still_sample_after_a_drop(fps, gap, hz, high);
                    let name = format!("still sample {gap} {hz} Hz {high}, {fps} fps");
                    assert_limited_across_a_drop(&name, &asked, fps);
                    built += 1;
                }
            }
        }
    }
    assert_eq!(built, 64);
}

// Random looks across drops: as in the random test above, with the ghost now
// and then gone long enough to rest and be dropped, back on whatever it asks,
// sometimes faint. Every run drops its ghost, and presents at most three
// flashes in any second over any background.
#[test]
fn random_looks_across_drops_present_at_most_three_flashes_over_any_background() {
    let mut state = 0xD1B5_4A32_D192_ED03u64;
    let mut next = move || {
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        (state >> 40) as f32 / (1u64 << 24) as f32
    };
    let tints = [WHITE, BLACK, [1.0, 0.0, 0.0], grey(0.81), ECHO, EMBER];
    for (fps, transparency) in [30.0f32, 60.0, 144.0, 240.0]
        .into_iter()
        .flat_map(|fps| GhostTransparency::ALL.map(|transparency| (fps, transparency)))
    {
        let mut clamp = GhostLookClamp::<Key>::default();
        clamp.set_transparency(transparency);
        let mut asked = look(0.0, WHITE);
        let (mut opacity, mut level, mut times) = (Vec::new(), Vec::new(), Vec::new());
        let (mut now, mut away, mut dropped) = (0.0f32, 0usize, 0);
        for _ in 0..(10.0 * fps) as usize {
            if next() < 0.3 {
                let levels = [0.0, 0.004, 0.03, 0.2, 0.5, 1.0, next()];
                asked.opacity = levels[(next() * 7.0) as usize % 7];
            }
            if next() < 0.3 {
                asked.tint = if next() < 0.2 {
                    [next(), next(), next()]
                } else {
                    tints[(next() * 6.0) as usize % 6]
                };
            }
            if away == 0 && next() < 0.02 {
                away = ((1.1 + next()) * fps) as usize;
            }
            let gone = away > 0;
            away = away.saturating_sub(1);
            let dt = if next() < 0.02 { 0.2 } else { 1.0 / fps };
            now += dt;
            clamp.begin_frame(Some(&frame(dt, true)));
            let held = clamp.look_clamp(&GHOST).is_some();
            let shown = (!gone).then(|| clamp.present(GHOST, asked));
            clamp.end_frame();
            if held && clamp.look_clamp(&GHOST).is_none() {
                dropped += 1;
            }
            let drawn = shown.map_or((0.0, 0.0), |shown| {
                (
                    drawn_opacity(transparency)(shown.opacity),
                    drawn_luminance(shown.tint),
                )
            });
            opacity.push(drawn.0);
            level.push(drawn.1);
            times.push(now);
        }
        assert!(
            dropped > 0,
            "{fps} fps {transparency:?}: the ghost is never dropped"
        );
        let worst = worst_transitions_over_any_background_at(&opacity, &level, &times);
        assert!(
            worst <= 6,
            "{fps} fps {transparency:?}: {worst} transitions"
        );
    }
}
