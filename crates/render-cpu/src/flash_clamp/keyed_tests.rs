// Keyed clamp over the per-value clamp: strobes, red tints, ramps, fresh starts
// and key lifetimes, on the opacity and redness rules a ghost's look clamp shares.
// See: context/lib/rendering_pipeline.md §7.8 (Photosensitivity limiter), §7.9

use super::super::flash_clamp_test_fixtures::worst_transitions;
use super::super::value::{
    FLASH_LUMINANCE_THRESHOLD, INTENSITY_RATE_PER_SECOND, RED_SATURATED, SUPPRESSION_DEADBAND,
    ValueClamp, luminance, redness,
};
use super::*;
use crate::flash_limiter::HITCH_CEILING_SECONDS;

/// A strength and its color per key, each on its own flash window.
impl KeyClamp for ValueClamp {
    type Value = (f32, [f32; 3]);

    fn fresh((strength, rgb): Self::Value) -> Self {
        ValueClamp::fresh(strength, rgb)
    }

    fn limit(&mut self, (strength, rgb): Self::Value, frame: &LimiterFrameUniform) -> Self::Value {
        ValueClamp::limit(self, strength, rgb, frame)
    }

    fn absent(&mut self, frame: &LimiterFrameUniform) {
        ValueClamp::absent(self, frame);
    }

    fn is_at_rest(&self) -> bool {
        ValueClamp::is_at_rest(self)
    }
}

type KeyedValueClamp<K> = KeyedClamp<K, ValueClamp>;

/// A generation-checked id: slot and generation.
type Key = (u32, u32);

const GHOST: Key = (7, 1);
const WHITE: [f32; 3] = [1.0, 1.0, 1.0];
const RED: [f32; 3] = [1.0, 0.0, 0.0];

fn frame(dt: f32, enabled: bool) -> LimiterFrameUniform {
    pack_limiter_frame(
        LimiterFrameInput {
            elapsed_seconds: dt,
        },
        enabled,
        false,
    )
}

/// 0 ↔ 1 at `hz`, starting low.
fn square(hz: f32, t: f32) -> f32 {
    if (t * hz).fract() >= 0.5 { 1.0 } else { 0.0 }
}

/// The membership ramp in front of the clamp: linear at the limiter's rate
/// cap on hitch-clamped presented-frame time, reversing from the presented
/// value.
fn ramp(presented: f32, target: f32, dt: f32) -> f32 {
    let step = INTENSITY_RATE_PER_SECOND * dt.min(HITCH_CEILING_SECONDS);
    presented + (target - presented).clamp(-step, step)
}

/// One frame of a single-key container: `None` presents nothing.
fn present(
    clamp: &mut KeyedValueClamp<Key>,
    value: Option<(f32, [f32; 3])>,
    frame: &LimiterFrameUniform,
) -> (f32, [f32; 3]) {
    clamp.begin_frame(frame);
    let presented = match value {
        Some(value) => clamp.limit(GHOST, value),
        None => (0.0, [0.0; 3]),
    };
    clamp.end_frame();
    presented
}

/// Present a white value's strengths through a fresh container at `fps`, the
/// limiter on. `None` frames present nothing.
fn present_strengths(values: &[Option<f32>], fps: f32) -> Vec<f32> {
    let mut clamp = KeyedValueClamp::default();
    let frame = frame(1.0 / fps, true);
    values
        .iter()
        .map(|value| present(&mut clamp, value.map(|s| (s, WHITE)), &frame).0)
        .collect()
}

/// A 10 Hz target toggle behind the membership ramp, for `seconds` at `fps`.
fn ramped_10hz_toggle(fps: f32, seconds: f32) -> Vec<f32> {
    let dt = 1.0 / fps;
    let mut membership = 0.0;
    (0..(seconds * fps).round() as usize)
        .map(|n| {
            membership = ramp(membership, square(10.0, n as f32 * dt), dt);
            membership
        })
        .collect()
}

/// Frames whose raw value rises past the threshold while the presented value
/// holds at or under the suppression deadband: an over-budget onset resting at
/// its pre-flash level.
fn held_onsets(raw: &[f32], presented: &[f32]) -> usize {
    raw.iter()
        .zip(presented)
        .filter(|&(&r, &p)| r >= FLASH_LUMINANCE_THRESHOLD && p <= SUPPRESSION_DEADBAND)
        .count()
}

// AC 23: a value strobing 0 ↔ 1 at 10 Hz (`setColorFilter` opacity at
// `periodMs` 0) presents at most three flashes a second and rests dark.
#[test]
fn a_10hz_value_strobe_presents_at_most_three_flashes_and_rests_at_its_pre_flash_level() {
    for fps in [60.0f32, 240.0] {
        let dt = 1.0 / fps;
        let raw: Vec<f32> = (0..(3.0 * fps) as usize)
            .map(|n| square(10.0, n as f32 * dt))
            .collect();
        assert!(worst_transitions(&raw, dt) >= 19, "{fps} Hz: raw strobe");

        let presented = present_strengths(&raw.iter().copied().map(Some).collect::<Vec<_>>(), fps);
        let worst = worst_transitions(&presented, dt);
        assert!((4..=6).contains(&worst), "{fps} Hz: {worst} transitions");
        // Over budget it rests at its pre-flash level: most frames that ask
        // for full strength present none.
        let on = raw.iter().filter(|&&r| r == 1.0).count();
        assert!(
            held_onsets(&raw, &presented) * 2 > on,
            "{fps} Hz: {} of {on} onset frames held",
            held_onsets(&raw, &presented)
        );
    }
}

// AC 23: arm/disarm, a category toggle or a member crossing the volume
// boundary at 10 Hz toggles the membership target; the ramp in front of the
// clamp turns it into a 0 ↔ 0.2 triangle the clamp still limits.
#[test]
fn a_10hz_membership_toggle_behind_the_ramp_presents_at_most_three_flashes() {
    for fps in [60.0f32, 240.0] {
        let dt = 1.0 / fps;
        let raw = ramped_10hz_toggle(fps, 3.0);
        assert!(worst_transitions(&raw, dt) >= 19, "{fps} Hz: raw ramp");

        let presented = present_strengths(&raw.iter().copied().map(Some).collect::<Vec<_>>(), fps);
        let worst = worst_transitions(&presented, dt);
        assert!((4..=6).contains(&worst), "{fps} Hz: {worst} transitions");
        let rising = raw
            .iter()
            .filter(|&&r| r >= FLASH_LUMINANCE_THRESHOLD)
            .count();
        assert!(
            held_onsets(&raw, &presented) * 2 > rising,
            "{fps} Hz: {} of {rising} onset frames held",
            held_onsets(&raw, &presented)
        );
    }
}

// A ghost removed once its membership reaches 0 vanishes between pulses; its
// key keeps its history, so the strobe does not start a fresh window each pulse.
#[test]
fn a_value_vanishing_between_10hz_pulses_keeps_its_history() {
    for fps in [60.0f32, 240.0] {
        let dt = 1.0 / fps;
        let values: Vec<Option<f32>> = ramped_10hz_toggle(fps, 3.0)
            .into_iter()
            .map(|m| (m > 1e-6).then_some(m))
            .collect();
        let presented = present_strengths(&values, fps);
        let worst = worst_transitions(&presented, dt);
        assert!((4..=6).contains(&worst), "{fps} Hz: {worst} transitions");
    }
}

// AC 24: a saturated-red tint strobing at 10 Hz on a ghost at full opacity
// desaturates over budget, keeping luminance and strength.
#[test]
fn a_red_tint_strobe_desaturates_over_budget_keeping_luminance() {
    for fps in [60.0f32, 240.0] {
        let dt = 1.0 / fps;
        let mut clamp = KeyedValueClamp::default();
        let frame = frame(dt, true);
        let mut desaturated = 0;
        for n in 0..(3.0 * fps) as usize {
            let tint = if square(10.0, n as f32 * dt) == 1.0 {
                RED
            } else {
                WHITE
            };
            let (strength, rgb) = present(&mut clamp, Some((1.0, tint)), &frame);
            assert_eq!(strength, 1.0, "{fps} Hz frame {n}: opacity is untouched");
            assert!(
                (luminance(rgb) - luminance(tint)).abs() < 1e-5,
                "{fps} Hz frame {n}: {rgb:?} for {tint:?}"
            );
            if tint == RED && redness(rgb) < RED_SATURATED {
                desaturated += 1;
            }
        }
        assert!(
            desaturated > 0,
            "{fps} Hz: an over-budget red onset desaturates"
        );
    }
}

// AC 24: one red tint change, under budget, keeps its hue.
#[test]
fn a_single_red_tint_change_keeps_its_hue() {
    let mut clamp = KeyedValueClamp::default();
    let frame = frame(1.0 / 60.0, true);
    for n in 0..120 {
        let tint = if n >= 30 { RED } else { WHITE };
        let presented = present(&mut clamp, Some((1.0, tint)), &frame);
        assert_eq!(presented, (1.0, tint), "frame {n}");
    }
}

// AC 25: a single rise reaches full strength within 250 ms, stepped directly
// or behind the membership ramp, which passes the clamp unchanged.
#[test]
fn a_single_rise_reaches_full_strength_within_250_ms() {
    for fps in [60.0f32, 240.0] {
        let dt = 1.0 / fps;
        let frames = (0.5 * fps) as usize;
        let full_by = (0.25 * fps).round() as usize;
        let step: Vec<Option<f32>> = (0..frames).map(|n| Some((n > 0) as u8 as f32)).collect();
        let stepped = present_strengths(&step, fps);
        assert!(
            stepped[full_by] >= 1.0 - 1e-5,
            "{fps} Hz: {} at 250 ms",
            stepped[full_by]
        );

        let mut membership = 0.0;
        let ramped: Vec<f32> = (0..frames)
            .map(|n| {
                membership = ramp(membership, (n > 0) as u8 as f32, dt);
                membership
            })
            .collect();
        let presented =
            present_strengths(&ramped.iter().copied().map(Some).collect::<Vec<_>>(), fps);
        assert_eq!(presented, ramped, "{fps} Hz: the ramp passes unchanged");
        assert!(presented[full_by] >= 1.0 - 1e-5, "{fps} Hz");
    }
}

// AC 25: the example's 1500 ms fade presents exactly as with the limiter off.
#[test]
fn a_1500ms_linear_fade_presents_unchanged() {
    for fps in [60.0f32, 240.0] {
        let dt = 1.0 / fps;
        let fade = |t: f32| (t / 1.5).min(1.0);
        // Up over 1.5 s, held, then back down over 1.5 s.
        let raw: Vec<f32> = (0..(4.0 * fps) as usize)
            .map(|n| {
                let t = n as f32 * dt;
                if t < 2.0 {
                    fade(t)
                } else {
                    1.0 - fade(t - 2.0)
                }
            })
            .collect();
        let presented = present_strengths(&raw.iter().copied().map(Some).collect::<Vec<_>>(), fps);
        assert_eq!(presented, raw, "{fps} Hz");
    }
}

// AC 25 (P21): a new key starts fresh against its own value, beside a key
// whose budget is spent, so a new ghost's first frame packs unchanged.
#[test]
fn a_new_key_packs_its_first_frame_unchanged() {
    let dt = 1.0 / 60.0;
    let frame = frame(dt, true);
    let mut clamp = KeyedValueClamp::default();
    let strobing: Key = (1, 1);
    // A second of 10 Hz strobe spends the strobing key's budget.
    for n in 0..60 {
        clamp.begin_frame(&frame);
        clamp.limit(strobing, (square(10.0, n as f32 * dt), WHITE));
        clamp.end_frame();
    }
    // A new ghost at membership 0 ramps in beside it; a new key arriving at
    // any value packs it unchanged on its first frame.
    let mut membership = 0.0;
    for n in 60..90 {
        clamp.begin_frame(&frame);
        clamp.limit(strobing, (square(10.0, n as f32 * dt), WHITE));
        let first_frame = n == 60;
        membership = if first_frame {
            0.0
        } else {
            ramp(membership, 1.0, dt)
        };
        assert_eq!(
            clamp.limit(GHOST, (membership, WHITE)),
            (membership, WHITE),
            "frame {n}"
        );
        if first_frame {
            assert_eq!(clamp.limit((9, 1), (0.7, RED)), (0.7, RED));
        }
        clamp.end_frame();
    }
}

// AC 25 (P31): turning the limiter on while values present dips none, and a
// new container's first frame (a new renderer) packs them unchanged.
#[test]
fn turning_the_limiter_on_while_values_present_dips_none() {
    let dt = 1.0 / 60.0;
    let values: [(Key, f32, [f32; 3]); 3] = [
        ((1, 1), 1.0, WHITE),
        ((2, 1), 0.6, RED),
        ((3, 4), 0.3, WHITE),
    ];
    let check = |clamp: &mut KeyedValueClamp<Key>, enabled: bool, what: &str| {
        clamp.begin_frame(&frame(dt, enabled));
        for (key, strength, rgb) in values {
            assert_eq!(clamp.limit(key, (strength, rgb)), (strength, rgb), "{what}");
        }
        clamp.end_frame();
    };
    let mut fresh = KeyedValueClamp::default();
    for _ in 0..30 {
        check(&mut fresh, true, "a new container");
    }
    let mut toggled = KeyedValueClamp::default();
    for _ in 0..10 {
        check(&mut toggled, false, "off");
    }
    for _ in 0..30 {
        check(&mut toggled, true, "turned on");
    }
}

/// A clamp that presents only whether its key appeared or started fresh.
#[derive(Clone, Debug)]
struct Started {
    appeared: bool,
}

impl KeyClamp for Started {
    type Value = bool;

    fn fresh(_: bool) -> Self {
        Self { appeared: false }
    }

    fn appearing(_: bool) -> Self {
        Self { appeared: true }
    }

    fn limit(&mut self, _: bool, _: &LimiterFrameUniform) -> bool {
        self.appeared
    }

    fn absent(&mut self, _: &LimiterFrameUniform) {}

    fn is_at_rest(&self) -> bool {
        false
    }
}

// The frame that turns the limiter on, and a frame that starts history, start
// every key presenting on it fresh; a key first presented on a later frame
// appeared, so a clamp may count its appearance.
#[test]
fn keys_presenting_as_history_starts_start_fresh_and_later_keys_appear() {
    let dt = 1.0 / 60.0;
    let starts = pack_limiter_frame(
        LimiterFrameInput {
            elapsed_seconds: dt,
        },
        true,
        true,
    );
    let mut clamp = KeyedClamp::<Key, Started>::default();
    clamp.begin_frame(&frame(dt, false));
    assert!(!clamp.limit(GHOST, false), "off passes through");
    clamp.end_frame();
    clamp.begin_frame(&frame(dt, true));
    assert!(!clamp.limit(GHOST, false), "turned on");
    clamp.end_frame();
    clamp.begin_frame(&frame(dt, true));
    assert!(!clamp.limit(GHOST, false), "its history goes on");
    assert!(clamp.limit((8, 1), false), "a key appearing later");
    clamp.end_frame();
    clamp.begin_frame(&starts);
    assert!(!clamp.limit(GHOST, false), "history starts again");
    assert!(!clamp.limit((8, 1), false), "for every key");
    clamp.end_frame();
}

// AC 26: off, a 10 Hz strobe passes unclamped and nothing is kept, so turning
// on mid-strobe starts fresh.
#[test]
fn the_limiter_off_passes_a_10hz_strobe_unclamped_and_keeps_no_history() {
    let dt = 1.0 / 60.0;
    let mut clamp = KeyedValueClamp::default();
    // On first, so there is history to discard.
    for n in 0..30 {
        present(
            &mut clamp,
            Some((square(10.0, n as f32 * dt), WHITE)),
            &frame(dt, true),
        );
    }
    for n in 30..90 {
        let value = square(10.0, n as f32 * dt);
        let presented = present(&mut clamp, Some((value, RED)), &frame(dt, false));
        assert_eq!(presented, (value, RED), "frame {n}");
        assert!(clamp.entries.is_empty(), "frame {n}: off keeps no history");
    }
    // Turned on on a full-strength frame: it packs unchanged.
    let enabling = present(&mut clamp, Some((1.0, RED)), &frame(dt, true));
    assert_eq!(enabling, (1.0, RED));
}

// AC 26 (P30): a hitch frame's step is at most the rate cap over the hitch
// ceiling, though the hitch spans far more time.
#[test]
fn a_hitch_frame_steps_at_most_the_hitch_ceiling() {
    let dt = 1.0 / 60.0;
    let mut clamp = KeyedValueClamp::default();
    for _ in 0..10 {
        present(&mut clamp, Some((0.0, WHITE)), &frame(dt, true));
    }
    let (step, _) = present(&mut clamp, Some((1.0, WHITE)), &frame(0.5, true));
    assert!(
        step > 0.0 && step <= INTENSITY_RATE_PER_SECOND * HITCH_CEILING_SECONDS + 1e-6,
        "the hitch frame steps {step}"
    );
}

// P19: a slot re-tenanted on one frame is a different key. The new tenant
// starts its own history; the old one's ends once its window empties.
#[test]
fn a_re_tenanted_key_starts_fresh_and_the_old_history_ends() {
    let dt = 1.0 / 60.0;
    let frame = frame(dt, true);
    let (old, new): (Key, Key) = ((4, 1), (4, 2));
    let mut clamp = KeyedValueClamp::default();
    // The old tenant spends its budget on a 10 Hz strobe and ends dark.
    for n in 0..60 {
        clamp.begin_frame(&frame);
        clamp.limit(old, (square(10.0, n as f32 * dt), WHITE));
        clamp.end_frame();
    }
    // Despawn and respawn on one frame: the new tenant rises from 0 at the
    // rate cap, its onset admitted though the old key's budget is spent.
    let mut membership = 0.0;
    let mut rose = Vec::new();
    for _ in 0..90 {
        clamp.begin_frame(&frame);
        membership = ramp(membership, 1.0, dt);
        rose.push(clamp.limit(new, (membership, WHITE)).0);
        clamp.end_frame();
    }
    assert_eq!(rose[0], INTENSITY_RATE_PER_SECOND * dt, "{rose:?}");
    assert_eq!(rose.last(), Some(&1.0));
    assert_eq!(clamp.entries.len(), 1, "the old tenant's history ended");
    assert!(clamp.entries.contains_key(&new));
}
