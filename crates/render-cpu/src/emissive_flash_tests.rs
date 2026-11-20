// Emissive volumes through the flash limiter: the clamp input each volume
// feeds, the presented glow mapped back from the clamp, and the container's
// lifecycle (fresh starts, toggles, failed acquires, level loads).
// See: context/lib/rendering_pipeline.md §7.8 (Photosensitivity limiter)

use super::*;
use crate::flash_clamp::INTENSITY_RATE_PER_SECOND;
use crate::flash_limiter::pack_limiter_frame;
use postretro_level_format::emissive::EmissiveVolumeRecord;

const DT: f32 = 1.0 / 60.0;
const WHITE: [f32; 3] = [1.0; 3];
/// The clamp's saturated-red chromaticity (`flash_clamp` `RED_SATURATED`).
const RED_SATURATED: f32 = 0.7;
/// The example's `flicker` curve (`sdk/lib/entities/emissive_volumes.ts`).
const FLICKER_PATTERN: [f32; 8] = [0.95, 0.40, 1.00, 0.72, 0.15, 0.88, 0.30, 0.65];

fn elapsed(dt: f32) -> LimiterFrameInput {
    LimiterFrameInput {
        elapsed_seconds: dt,
    }
}

fn on(dt: f32) -> LimiterFrameUniform {
    pack_limiter_frame(elapsed(dt), true, false)
}

fn off(dt: f32) -> LimiterFrameUniform {
    pack_limiter_frame(elapsed(dt), false, false)
}

/// Redness from chromaticity, as the clamp reads it.
fn redness(rgb: [f32; 3]) -> f32 {
    let total = rgb[0] + rgb[1] + rgb[2];
    if total < 0.003 {
        return 0.0;
    }
    ((rgb[0] / total - 1.0 / 3.0) * 1.5).clamp(0.0, 1.0)
}

/// One volume over one member texture: the texture's baked strength and mean
/// glow colour, and the volume's authored range.
#[derive(Clone, Copy)]
struct Volume {
    strength: f32,
    glow: [f32; 3],
    min: f32,
    max: f32,
}

impl Volume {
    fn white(strength: f32, max: f32) -> Self {
        Self {
            strength,
            glow: WHITE,
            min: 0.0,
            max,
        }
    }

    fn source(self) -> EmissiveFlashSource {
        EmissiveFlashSource::new(self.strength, self.glow)
    }

    fn glow_at(self, brightness: f32, color: [f32; 3]) -> VolumeGlow {
        VolumeGlow::scripted(self.min, self.max, brightness, color)
    }

    /// The member texture's mean glow as it shows under `factor`.
    fn surface(self, factor: [f32; 3]) -> [f32; 3] {
        std::array::from_fn(|c| self.strength * self.glow[c] * factor[c])
    }

    /// What the surface shows in the clamp's units: its luminance, capped
    /// at 1, since anything brighter reads white.
    fn shown(self, factor: [f32; 3]) -> f32 {
        luminance(self.surface(factor)).min(1.0)
    }
}

/// One volume's container, stepped a frame at a time.
struct Harness {
    volume: Volume,
    clamp: EmissiveFlashClamp,
}

impl Harness {
    fn new(volume: Volume) -> Self {
        let mut clamp = EmissiveFlashClamp::default();
        clamp.install(&[volume.source()]);
        Self { volume, clamp }
    }

    fn step_frame(
        &mut self,
        frame: Option<&LimiterFrameUniform>,
        brightness: f32,
        color: [f32; 3],
    ) -> [f32; 3] {
        let mut out = [[0.0; 3]];
        self.clamp
            .present(frame, &[self.volume.glow_at(brightness, color)], &mut out);
        self.clamp.finish_presentation(true);
        out[0]
    }

    fn step(&mut self, brightness: f32, color: [f32; 3]) -> [f32; 3] {
        self.step_frame(Some(&on(DT)), brightness, color)
    }
}

/// A clamp value: strength and colour.
type ClampValue = (f32, [f32; 3]);

/// One volume's clamp stepped directly, as the container steps it, so a test
/// sees the clamp's output and the presented multiplier and colour the packed
/// factor is built from.
struct Direct {
    source: EmissiveFlashSource,
    clamp: Option<ValueClamp>,
}

impl Direct {
    fn new(source: EmissiveFlashSource) -> Self {
        Self {
            source,
            clamp: None,
        }
    }

    /// One limited frame at [`DT`]; the first starts the clamp fresh, as a
    /// freshly installed container does.
    fn step(&mut self, glow: VolumeGlow) -> (ClampValue, ClampValue, VolumeGlow) {
        let input = self.source.input(glow);
        let clamp = self
            .clamp
            .get_or_insert_with(|| ValueClamp::fresh(input.0, input.1));
        let output = clamp.limit(input.0, input.1, &on(DT));
        (input, output, self.source.presented(glow, input, output))
    }
}

/// Count transitions of `series` (one value per frame at `dt`) in any one
/// second, from the last extremum: a swing of `threshold` with its darker
/// state under `dark_limit` counts, and a reversal ends an excursion only
/// once it reaches the threshold. Independent of the clamp it judges.
fn worst_swings(series: &[f32], dt: f32, threshold: f32, dark_limit: f32) -> usize {
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
        } else if -beyond >= threshold {
            ext = peak;
            peak = v;
            dir = -dir;
            counted = false;
        }
        if !counted && (v - ext).abs() >= threshold && v.min(ext) < dark_limit {
            counted = true;
            counted_at.push(n as f32 * dt);
        }
    }
    (0..counted_at.len())
        .map(|i| {
            counted_at[i..]
                .iter()
                .take_while(|&&t| t < counted_at[i] + 1.0)
                .count()
        })
        .max()
        .unwrap_or(0)
}

/// WCAG general flashes: a 0.1 swing with the darker state under 0.8.
fn worst_transitions(series: &[f32]) -> usize {
    worst_swings(series, DT, 0.1, 0.8)
}

/// Red flashes: a swing of 0.2 in presented redness × strength.
fn worst_red_transitions(series: &[f32]) -> usize {
    worst_swings(series, DT, 0.2, f32::INFINITY)
}

/// A square wave alternating `a` and `b` at `hz` cycles a second.
fn square(n: usize, hz: f32, a: f32, b: f32) -> f32 {
    if (n as f32 * DT * hz).fract() < 0.5 {
        a
    } else {
        b
    }
}

/// The example's `flicker({ min, max, rate })` sampled at frame `n`.
fn flicker(n: usize, lo: f32, hi: f32, rate: f32) -> f32 {
    let phase = (n as f32 * DT * rate).fract();
    let pos = phase * (FLICKER_PATTERN.len() - 1) as f32;
    let at = (pos as usize).min(FLICKER_PATTERN.len() - 1);
    let next = (at + 1).min(FLICKER_PATTERN.len() - 1);
    let t = FLICKER_PATTERN[at] + (FLICKER_PATTERN[next] - FLICKER_PATTERN[at]) * (pos - at as f32);
    lo + t * (hi - lo)
}

/// Two seconds of `brightness(n)` on `volume` in white, shown unlimited and
/// limited.
fn strobe(volume: Volume, brightness: impl Fn(usize) -> f32) -> (Vec<f32>, Vec<f32>) {
    let mut harness = Harness::new(volume);
    (0..120)
        .map(|n| {
            let b = brightness(n);
            let raw = volume.shown(volume.glow_at(b, WHITE).factor());
            (raw, volume.shown(harness.step(b, WHITE)))
        })
        .unzip()
}

/// The strobe is a flash hazard unlimited, and limited presents at most 3
/// flashes in any second, changing no faster than the rate cap.
fn assert_limited(raw: &[f32], limited: &[f32]) {
    assert!(worst_transitions(raw) > 6, "the strobe flashes: {raw:?}");
    let worst = worst_transitions(limited);
    assert!(worst <= 6, "{worst} transitions in one second: {limited:?}");
    for pair in limited.windows(2) {
        let step = (pair[1] - pair[0]).abs();
        assert!(
            step <= INTENSITY_RATE_PER_SECOND * DT + 1e-5,
            "step {step}: {limited:?}"
        );
    }
}

fn close(a: [f32; 3], b: [f32; 3], eps: f32) {
    for (x, y) in a.into_iter().zip(b) {
        assert!((x - y).abs() <= eps, "{a:?} vs {b:?}");
    }
}

// The clamp input: the brightest member texture by strength × glow
// luminance; faces with no texture and out-of-table indices count nothing.
#[test]
fn a_volume_is_judged_by_its_brightest_member_texture() {
    let texture = |strength: f32, glow_color: [f32; 3]| EmissiveTexture {
        strength,
        glow_color,
    };
    let textures = [
        texture(4.0, [0.1; 3]), // 0.4
        texture(1.0, [0.8; 3]), // 0.8
        texture(2.0, [0.6; 3]), // 1.2: the brightest
        EmissiveTexture::DARK,
    ];
    let best =
        EmissiveFlashSource::brightest(&textures, [Some(0), None, Some(2), Some(9), Some(1)]);
    assert_eq!(best, EmissiveFlashSource::new(2.0, [0.6; 3]));
    assert!((best.peak - 1.2).abs() < 1e-5);

    // A strong texture with a dim glow loses to a weak one with a bright
    // glow: the clamp reads what shows, not the strength alone.
    let bright = EmissiveFlashSource::brightest(&textures, [Some(0), Some(1)]);
    assert_eq!(bright, EmissiveFlashSource::new(1.0, [0.8; 3]));

    let section = EmissiveSection {
        textures: textures.to_vec(),
        volumes: [vec![0, 1], vec![], vec![2]]
            .into_iter()
            .map(|faces| EmissiveVolumeRecord {
                min: 0.0,
                max: 1.0,
                color: WHITE,
                origin: [0.0; 3],
                faces,
                tags: Vec::new(),
            })
            .collect(),
    };
    // Face 0 shows texture 1, face 1 has no texture, face 2 texture 3.
    let face_texture = |face: u32| [Some(1), None, Some(3)][face as usize];
    assert_eq!(
        EmissiveFlashSource::for_volumes(&section, face_texture),
        [
            EmissiveFlashSource::new(1.0, [0.8; 3]),
            EmissiveFlashSource::DARK,
            EmissiveFlashSource::DARK,
        ]
    );
}

// Pin P56: a volume a later volume emptied has no member face. It feeds the
// clamp 0, never NaN or −inf, and presents its scripted glow.
#[test]
fn an_emptied_volume_feeds_zero_and_presents_its_scripted_glow() {
    let empty = EmissiveFlashSource::brightest(&[], std::iter::empty());
    assert_eq!(empty, EmissiveFlashSource::DARK);
    let mut clamp = EmissiveFlashClamp::default();
    clamp.install(&[empty]);
    for n in 0..120 {
        let glow = VolumeGlow::scripted(0.0, 4.0, square(n, 6.0, 0.0, 1.0), WHITE);
        let input = empty.input(glow);
        assert_eq!(input, (0.0, [0.0; 3]));
        let mut out = [[f32::NAN; 3]];
        clamp.present(Some(&on(DT)), &[glow], &mut out);
        clamp.finish_presentation(true);
        assert_eq!(out[0], glow.factor());
    }
    // A texture that does not glow, or values the bake never writes, read
    // dark too.
    for (strength, glow) in [
        (0.0, WHITE),
        (1.0, [0.0; 3]),
        (f32::NAN, WHITE),
        (-1.0, WHITE),
    ] {
        assert_eq!(
            EmissiveFlashSource::new(strength, glow),
            EmissiveFlashSource::DARK
        );
    }
}

// Pin P14: the example's rate-6 flicker (0.3 to 1, max 1, strength 1,
// white) presents at most 3 flashes in any second, at no more than 4.0/s.
#[test]
fn the_examples_flicker_is_limited_to_three_flashes_a_second() {
    let (raw, limited) = strobe(Volume::white(1.0, 1.0), |n| flicker(n, 0.3, 1.0, 6.0));
    assert_limited(&raw, &limited);
}

// Pin P34: static writes alternating 0 and 1 six times a second meet the
// same bound.
#[test]
fn static_writes_alternating_off_and_on_are_limited() {
    let (raw, limited) = strobe(Volume::white(1.0, 1.0), |n| square(n, 6.0, 0.0, 1.0));
    assert_limited(&raw, &limited);
}

// Pin P40: the clamp reads peak strength, not brightness. At max 20, a
// 0.025 ↔ 0.05 strobe shows 0.5 ↔ 1 and is limited.
#[test]
fn a_small_brightness_swing_on_a_bright_volume_is_limited() {
    let (raw, limited) = strobe(Volume::white(1.0, 20.0), |n| square(n, 6.0, 0.025, 0.05));
    assert_limited(&raw, &limited);
}

// Pin P48: strength 4, max 1, brightness 0.15 ↔ 0.2 swings the peak 0.6 ↔
// 0.8 and is limited.
#[test]
fn a_small_brightness_swing_on_a_strong_texture_is_limited() {
    let (raw, limited) = strobe(Volume::white(4.0, 1.0), |n| square(n, 6.0, 0.15, 0.2));
    assert_limited(&raw, &limited);
}

// Pin P58: a migrated panel (strength 4, mean glow luminance 0.43) under a
// white max-1 volume flickering brightness 0.3 to 1 shows 0.52 ↔ 1 and is
// limited. A clamp assuming white texels would read 1 ↔ 1 and pass it. (The
// example's flicker curve dips to 0.3 only between 60 Hz samples, so the
// swing is driven directly.)
#[test]
fn a_migrated_panels_flicker_is_judged_by_its_glow_colour() {
    let panel = Volume {
        strength: 4.0,
        glow: [0.43; 3],
        min: 0.0,
        max: 1.0,
    };
    assert!((panel.source().peak - 4.0 * 0.43).abs() < 1e-5);
    let (raw, limited) = strobe(panel, |n| square(n, 6.0, 0.3, 1.0));
    assert_limited(&raw, &limited);
}

// Pin P37: a strobe whose shown glow is at or above 1 both ways reads white
// both ways: nothing is limited, and max 20's full value presents.
#[test]
fn a_strobe_white_both_ways_presents_unlimited() {
    let volume = Volume::white(1.0, 20.0);
    let mut harness = Harness::new(volume);
    for n in 0..120 {
        let b = square(n, 6.0, 0.2, 1.0);
        assert_eq!(harness.step(b, WHITE), volume.glow_at(b, WHITE).factor());
    }
}

// Pin P41: at max 20, a write from 0.025 to 1 rises on screen no faster
// than the rate cap, then presents 20. Scaling the factor by output ÷ input
// would present ~11 on the first frame: a one-frame step to white.
#[test]
fn a_step_past_white_rises_at_the_rate_cap_then_presents_its_full_value() {
    let volume = Volume::white(1.0, 20.0);
    let mut harness = Harness::new(volume);
    for _ in 0..90 {
        assert_eq!(harness.step(0.025, WHITE), [0.5; 3]);
    }
    let mut previous = 0.5;
    let mut frames = Vec::new();
    for _ in 0..30 {
        let factor = harness.step(1.0, WHITE);
        let shown = volume.shown(factor);
        assert!(
            shown - previous <= INTENSITY_RATE_PER_SECOND * DT + 1e-5,
            "{previous} → {shown}"
        );
        previous = shown;
        frames.push(factor[0]);
    }
    assert!(
        (frames[0] - (0.5 + INTENSITY_RATE_PER_SECOND * DT)).abs() < 1e-5,
        "first frame presents {}",
        frames[0]
    );
    // Once the shown peak reaches 1, the scripted 20 presents, and every
    // frame before it stayed at or under white.
    let reached = frames.iter().position(|&f| f == 20.0).expect("presents 20");
    assert!(frames[..reached].iter().all(|&f| f <= 1.0), "{frames:?}");
    assert!(frames[reached..].iter().all(|&f| f == 20.0), "{frames:?}");
}

/// P59's red `_e` sign: strength 4, saturated-red mean glow, peak ≈ 0.949.
const RED_SIGN: Volume = Volume {
    strength: 4.0,
    glow: [0.9, 0.06, 0.04],
    min: 0.0,
    max: 1.0,
};

// Pin P59: a red `_e` sign under a white volume, strobing 0 ↔ 1 past the
// budget: the clamp's colour is the red glow, so the red-flash rule holds the
// onset. The sign's weak green and blue cannot rise past the white volume's
// brightest channel, so the held sign presents less red than scripted (though
// more than the held level) and dims below the clamp's strength instead of
// keeping its luminance.
#[test]
fn a_red_sign_under_a_white_volume_meets_the_red_flash_rule() {
    let sign = RED_SIGN;
    assert!(redness(sign.glow) >= RED_SATURATED);
    let mut harness = Harness::new(sign);
    let mut direct = Direct::new(sign.source());
    let (mut held, mut less_red, mut dimmed) = (0, 0, 0);
    let mut red = Vec::new();
    let mut raw_red = Vec::new();
    for n in 0..120 {
        let b = square(n, 6.0, 0.0, 1.0);
        let factor = harness.step(b, WHITE);
        let (input, output, presented) = direct.step(sign.glow_at(b, WHITE));
        assert_eq!(factor, packed(presented));
        let surface = sign.surface(factor);
        let shown = sign.shown(factor);
        red.push(shown * redness(surface));
        raw_red.push(sign.shown(sign.glow_at(b, WHITE).factor()) * redness(sign.glow));
        if output.1 == input.1 {
            continue;
        }
        // A held red onset.
        held += 1;
        let presented_red = redness(surface);
        assert!(
            presented_red <= redness(sign.glow) + 1e-6,
            "frame {n}: redness {presented_red} above the scripted sign's"
        );
        assert!(
            presented_red >= redness(output.1) - 1e-4,
            "frame {n}: redness {presented_red} below the held {}",
            redness(output.1)
        );
        assert!(
            luminance(surface) <= output.0 + 1e-6,
            "frame {n}: luminance {} above the clamp's strength {}",
            luminance(surface),
            output.0
        );
        if presented_red < redness(sign.glow) - 0.01 {
            less_red += 1;
        }
        if luminance(surface) < output.0 - 0.01 {
            dimmed += 1;
        }
    }
    assert!(held > 0, "the strobe holds a red onset");
    assert!(
        less_red > 0,
        "a held red onset presents less red than scripted"
    );
    assert!(dimmed > 0, "a held red onset on a red glow dims");
    assert!(worst_red_transitions(&raw_red) > 6);
    assert!(worst_red_transitions(&red) <= 6, "{red:?}");
}

// Regression: a held red onset on a red-glow volume boosted the presented
// green and blue far past the scripted colour (only the value ceiling stopped
// them), so a white member texture of the same volume showed about twice what
// the clamp let through and stepped past the rate cap.
//
// Invariant: no member face shows more than it would under a white volume at
// the presented multiplier: every packed channel is at most multiplier ×
// the scripted colour's brightest channel.
#[test]
fn a_held_red_never_shows_another_member_brighter_than_a_white_volume_would() {
    // Strength 0.9 white `_e`: peak 0.9, under the sign's ≈ 0.949, so the
    // sign judges the volume and the white member only rides its factor.
    let white = Volume {
        strength: 0.9,
        glow: WHITE,
        min: 0.0,
        max: 1.0,
    };
    let texture = |volume: Volume| EmissiveTexture {
        strength: volume.strength,
        glow_color: volume.glow,
    };
    let source =
        EmissiveFlashSource::brightest(&[texture(RED_SIGN), texture(white)], [Some(0), Some(1)]);
    assert_eq!(source, RED_SIGN.source());

    // White, and a grey whose brightest channel caps below 1.
    for color in [WHITE, [0.5; 3]] {
        let mut clamp = EmissiveFlashClamp::default();
        clamp.install(&[source]);
        let mut direct = Direct::new(source);
        let mut held = 0;
        let mut previous: Option<f32> = None;
        for n in 0..120 {
            let glow = RED_SIGN.glow_at(square(n, 6.0, 0.0, 1.0), color);
            let mut out = [[f32::NAN; 3]];
            clamp.present(Some(&on(DT)), &[glow], &mut out);
            clamp.finish_presentation(true);
            let factor = out[0];
            let (input, output, presented) = direct.step(glow);
            assert_eq!(factor, packed(presented), "frame {n}");
            if output.1 != input.1 {
                held += 1;
            }

            let brightest = color.into_iter().fold(0.0, f32::max);
            for channel in factor {
                assert!(
                    channel <= presented.multiplier * brightest,
                    "{color:?} frame {n}: {factor:?} past {} × {brightest}",
                    presented.multiplier
                );
            }
            // What the white member shows, in the judged clamp's units.
            let shown = white.shown(factor);
            assert!(
                shown <= white.strength * presented.multiplier * brightest + 1e-6,
                "{color:?} frame {n}: the white member shows {shown} under {factor:?}"
            );
            if let Some(previous) = previous {
                assert!(
                    (shown - previous).abs() <= INTENSITY_RATE_PER_SECOND * DT + 1e-5,
                    "{color:?} frame {n}: the white member steps {previous} → {shown}"
                );
            }
            previous = Some(shown);
        }
        assert!(held > 0, "{color:?}: the strobe holds a red onset");
    }
}

/// The P42 strobe: `setEmissiveColor` alternating red and green six times
/// a second at brightness 1, max 1, strength 1, white glow.
const STATUS_RED: [f32; 3] = [1.0, 0.05, 0.02];
const STATUS_GREEN: [f32; 3] = [0.02, 1.0, 0.05];
const STATUS_BLUE: [f32; 3] = [0.02, 0.05, 1.0];

fn status_color(n: usize) -> [f32; 3] {
    if square(n, 6.0, 1.0, 0.0) == 1.0 {
        STATUS_RED
    } else {
        STATUS_GREEN
    }
}

// Pin P42: colour writes reach the clamp as its colour, and the red-flash
// rule limits a red ↔ green strobe. Brightness is untouched.
#[test]
fn a_red_green_colour_strobe_meets_the_red_flash_rule() {
    let lamp = Volume::white(1.0, 1.0);
    let mut harness = Harness::new(lamp);
    let (raw, limited): (Vec<f32>, Vec<f32>) = (0..120)
        .map(|n| {
            let color = status_color(n);
            let factor = harness.step(1.0, color);
            // Luminance is the scripted colour's, every frame.
            assert!((luminance(factor) - luminance(color)).abs() < 1e-5);
            (redness(color), redness(factor))
        })
        .unzip();
    assert!(worst_red_transitions(&raw) > 6);
    assert!(worst_red_transitions(&limited) <= 6, "{limited:?}");
}

// Pin P43: white ↔ blue at max 20 reads strength 1 both ways and no
// redness, so it is not limited here (E23 owns it).
#[test]
fn a_white_blue_colour_strobe_at_high_strength_passes() {
    let volume = Volume::white(1.0, 20.0);
    let mut harness = Harness::new(volume);
    for n in 0..120 {
        let color = if square(n, 6.0, 1.0, 0.0) == 1.0 {
            WHITE
        } else {
            STATUS_BLUE
        };
        assert_eq!(
            harness.step(1.0, color),
            volume.glow_at(1.0, color).factor()
        );
    }
}

// Pin P52: once the P42 strobe has spent the window, a red onset is held:
// it presents the written red desaturated toward its own luminance, with
// luminance unchanged, since the lamp's white glow is neutral and no channel
// reaches the cap at the written red's brightest channel. A write away from
// red presents that frame.
#[test]
fn a_held_red_presents_desaturated_and_a_write_away_from_red_presents_at_once() {
    let lamp = Volume::white(1.0, 1.0);
    let mut harness = Harness::new(lamp);
    let mut held = None;
    for n in 0..120 {
        let color = status_color(n);
        let factor = harness.step(1.0, color);
        if color == STATUS_RED && redness(factor) < RED_SATURATED {
            held = Some((n, factor));
        }
    }
    let (at, factor) = held.expect("an over-budget red onset is held");
    // Desaturated toward its own luminance, not dimmed or eased: luminance
    // is the written red's.
    assert!(redness(factor) < redness(STATUS_RED));
    assert!((luminance(factor) - luminance(STATUS_RED)).abs() < 1e-5);
    // Replay to the held red, then write blue: it presents on its own frame,
    // since a move away from saturated red is a return.
    let mut harness = Harness::new(lamp);
    for n in 0..at {
        harness.step(1.0, status_color(n));
    }
    assert_eq!(harness.step(1.0, STATUS_RED), factor);
    assert_eq!(harness.step(1.0, STATUS_BLUE), STATUS_BLUE);
}

// Drift guard: the luminance this module feeds and maps back with is the one
// the clamp's red rule holds unchanged while it desaturates.
#[test]
fn the_luminance_weights_match_the_clamps_desaturation() {
    let mut clamp = ValueClamp::default();
    let mut desaturated = 0;
    for n in 0..120 {
        let rgb = if square(n, 6.0, 1.0, 0.0) == 1.0 {
            [0.9, 0.1, 0.05]
        } else {
            [0.1, 0.6, 0.3]
        };
        let (_, out) = clamp.limit(1.0, rgb, &on(DT));
        if out != rgb {
            desaturated += 1;
            assert!(
                (luminance(out) - luminance(rgb)).abs() < 1e-6,
                "{rgb:?} → {out:?}"
            );
        }
    }
    assert!(desaturated > 0);
}

// Pins P9 and P13: while the script clock is frozen, brightness holds and a
// flat fade presents unchanged, counting nothing; the clamp keeps aging on
// presented-frame time, so a strobe's spent window empties during the
// freeze and the next onset is admitted.
#[test]
fn a_frozen_or_flat_glow_counts_nothing_and_the_clamp_keeps_aging() {
    let volume = Volume::white(1.0, 1.0);
    let mut harness = Harness::new(volume);
    // Spend the budget.
    for n in 0..60 {
        harness.step(square(n, 6.0, 0.0, 1.0), WHITE);
    }
    // A flat fade (from == to) or a frozen sample: 1.5 s at 0. Once it has
    // settled from the strobe it presents unchanged.
    for _ in 0..30 {
        harness.step(0.0, WHITE);
    }
    for _ in 0..60 {
        assert_eq!(harness.step(0.0, WHITE), [0.0; 3]);
    }
    // The window aged out on presented time: the onset rises at the rate cap.
    let rise: Vec<f32> = (0..20).map(|_| harness.step(1.0, WHITE)[0]).collect();
    assert_eq!(rise.last(), Some(&1.0), "{rise:?}");
}

// Pin P15: turning the limiter on presents that frame unchanged and starts
// history fresh; turning it off presents scripted glow the same frame.
#[test]
fn toggling_the_limiter_presents_the_toggle_frame_unchanged() {
    let volume = Volume::white(1.0, 1.0);
    let mut harness = Harness::new(volume);
    let b = |n| square(n, 6.0, 0.0, 1.0);
    // On: the strobe spends the budget and a held onset rests low.
    let mut held_at = None;
    for n in 0..60 {
        let factor = harness.step(b(n), WHITE);
        if b(n) == 1.0 && factor[0] < 0.1 {
            held_at = Some(n);
        }
    }
    let held_at = held_at.expect("the limiter holds an onset");
    // Off mid-hold: the scripted value presents that frame.
    assert_eq!(harness.step_frame(Some(&off(DT)), 1.0, WHITE), [1.0; 3]);
    for n in 0..30 {
        assert_eq!(
            harness.step_frame(Some(&off(DT)), b(n), WHITE),
            [b(n); 3],
            "off passes through"
        );
    }
    // On mid-strobe, on a frame that jumps 0 → 1: unchanged, not rate-capped.
    assert_eq!(harness.step(1.0, WHITE), [1.0; 3]);
    // History started fresh: the fall and the next onset are admitted,
    // though the budget was spent before the off.
    let after: Vec<f32> = (0..12)
        .map(|k| harness.step(if (1..6).contains(&k) { 0.0 } else { 1.0 }, WHITE)[0])
        .collect();
    assert!(
        after[..6].iter().any(|&f| f <= 0.9),
        "{after:?} (held at {held_at})"
    );
    assert!(after[6..].iter().any(|&f| f >= 0.5), "{after:?}");
}

// Pin P16: every volume is limited every frame on its own window, whatever
// the others do; the container takes no view, so being out of view changes
// nothing.
#[test]
fn every_volume_is_limited_every_frame_on_its_own_window() {
    let volume = Volume::white(1.0, 1.0);
    let mut clamp = EmissiveFlashClamp::default();
    clamp.install(&[volume.source(); 3]);
    let mut series = [Vec::new(), Vec::new(), Vec::new()];
    for n in 0..120 {
        let scripted = [
            volume.glow_at(square(n, 6.0, 0.0, 1.0), WHITE),
            volume.glow_at(0.5, WHITE),
            volume.glow_at(square(n + 3, 5.0, 1.0, 0.0), WHITE),
        ];
        let mut out = [[f32::NAN; 3]; 3];
        clamp.present(Some(&on(DT)), &scripted, &mut out);
        clamp.finish_presentation(true);
        for (shown, factor) in series.iter_mut().zip(out) {
            shown.push(volume.shown(factor));
        }
    }
    assert!(worst_transitions(&series[0]) <= 6);
    assert!(
        series[1].iter().all(|&s| s == 0.5),
        "a steady volume is untouched"
    );
    assert!(worst_transitions(&series[2]) <= 6);
}

// Pin P17: a frame whose surface acquire fails leaves every clamp where it
// began, and its time merges into the next frame: the run matches one where
// that frame never happened and the next took both frames' time.
#[test]
fn a_failed_acquire_leaves_every_clamp_where_the_frame_began() {
    let volume = Volume::white(1.0, 1.0);
    let b = |n| square(n, 6.0, 0.0, 1.0);
    let run = |fail: bool| {
        let mut harness = Harness::new(volume);
        let mut out = Vec::new();
        for n in 0..120 {
            if n == 40 {
                if fail {
                    let mut lost = [[0.0; 3]];
                    harness
                        .clamp
                        .present(Some(&on(DT)), &[volume.glow_at(b(n), WHITE)], &mut lost);
                    harness.clamp.finish_presentation(false);
                }
                continue;
            }
            // The lost frame's time reaches frame 41 by merging; with no
            // lost frame, frame 41 simply took twice as long.
            let dt = if n == 41 && !fail { 2.0 * DT } else { DT };
            out.push(harness.step_frame(Some(&on(dt)), b(n), WHITE));
        }
        out
    };
    let failed = run(true);
    let skipped = run(false);
    for (a, b) in failed.iter().zip(&skipped) {
        close(*a, *b, 1e-6);
    }

    // The enabling frame failing: the next frame still starts fresh.
    let mut harness = Harness::new(volume);
    harness.step_frame(Some(&off(DT)), 0.0, WHITE);
    let mut lost = [[0.0; 3]];
    harness
        .clamp
        .present(Some(&on(DT)), &[volume.glow_at(0.0, WHITE)], &mut lost);
    harness.clamp.finish_presentation(false);
    assert_eq!(harness.step(1.0, WHITE), [1.0; 3], "fresh, not rate-capped");
}

// Pin P18: loading a map starts every clamp fresh; a brightness write does
// not.
#[test]
fn a_level_load_starts_fresh_and_a_brightness_write_does_not() {
    let volume = Volume::white(1.0, 1.0);
    let mut harness = Harness::new(volume);
    for n in 0..60 {
        harness.step(square(n, 6.0, 0.0, 1.0), WHITE);
    }
    for _ in 0..30 {
        harness.step(0.0, WHITE);
    }
    // A write keeps history: the step is rate-capped, not presented at once.
    let mut written = Harness {
        volume,
        clamp: harness.clamp.clone(),
    };
    assert!(written.step(1.0, WHITE)[0] < 1.0);
    // Map B's volume 0, static at brightness 1: fresh, so it presents at once.
    harness.clamp.install(&[volume.source()]);
    assert_eq!(harness.step(1.0, WHITE), [1.0; 3]);
}

// With the limiter off, a step lands in one frame; a frame outside the
// limiter (no input) passes through and leaves history untouched.
#[test]
fn off_or_outside_the_limiter_a_step_lands_in_one_frame() {
    let volume = Volume::white(1.0, 20.0);
    let mut harness = Harness::new(volume);
    harness.step_frame(Some(&off(DT)), 0.0, WHITE);
    assert_eq!(harness.step_frame(Some(&off(DT)), 1.0, WHITE), [20.0; 3]);

    let mut harness = Harness::new(volume);
    harness.step(0.0, WHITE);
    assert_eq!(harness.step_frame(None, 1.0, WHITE), [20.0; 3]);
    // The limited frame after it rate-caps from where history left it.
    let next = harness.step(1.0, WHITE);
    assert!(
        (next[0] - INTENSITY_RATE_PER_SECOND * DT).abs() < 1e-5,
        "{next:?}"
    );
}
