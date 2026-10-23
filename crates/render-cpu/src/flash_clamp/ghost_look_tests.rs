// Ghost look clamp: three flashes a second over any background, regressions,
// strobes, ordinary looks, red, dither, vanishing, appearing, and off.
// See: context/lib/rendering_pipeline.md §7.8 (Photosensitivity limiter), §7.9

use super::super::flash_clamp_test_fixtures::{
    transitions_over, worst_transitions, worst_transitions_over_any_background,
    worst_transitions_over_any_background_at,
};
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
/// so neither corner sees a flash.
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

// Regression: each corner the clamp judged saw a slow drift while the mix of
// two corners swung, so a mid-grey background drew 10–31 transitions a second
// that presented unchanged. Opacity climbs while the tint alternates bright and
// dim; the worst is tuned between two edge samples (λ = 17/32).
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
        let opacity = alternate(10.0, n as f32 * dt, 0.0, 1.0);
        clamp.begin_frame(Some(&frame));
        let opacity_strobe = clamp.present(GHOST, look(opacity, WHITE));
        let steady = clamp.present(other, look(0.5, ECHO));
        clamp.end_frame();
        assert_eq!(opacity_strobe.tint, WHITE, "frame {n}");
        assert_eq!(steady, look(0.5, ECHO), "frame {n}");
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

// A ghost's state is fixed-size and cheap to snapshot each frame.
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
        LookValueClamp::fresh(AskedLook {
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
