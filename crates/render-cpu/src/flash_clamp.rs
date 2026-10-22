// Channel clamp: the photosensitivity limiter on the screen channels.
// `screen.flash`, `screen.vignette` and the engine's fluid view tint are each a
// per-value clamp (`value.rs`), limited as they pack into the resolve's effect
// uniform. `keyed.rs` holds many per-value clamps for draw-side sources.
// See: context/lib/rendering_pipeline.md §7.8 (Photosensitivity limiter)

mod keyed;
mod value;

pub use keyed::KeyedValueClamp;
pub use value::{INTENSITY_RATE_PER_SECOND, ValueClamp};

use crate::flash_limiter::{LimiterFrameInput, LimiterFrameUniform, pack_limiter_frame};
use crate::screen_effects::EffectUniform;

/// Limit one packed `[r, g, b, strength]` channel in place, starting fresh
/// first on the frame that turns the clamp on.
fn apply_channel(channel: &mut ValueClamp, rgba: &mut [f32; 4], frame: &LimiterFrameUniform) {
    let rgb = [rgba[0], rgba[1], rgba[2]];
    if frame.init {
        channel.restart(rgba[3], rgb);
    }
    let (strength, rgb) = channel.limit(rgba[3], rgb, frame);
    *rgba = [rgb[0], rgb[1], rgb[2], strength];
}

/// The channel clamp's state across frames. Off, it passes the uniform through
/// and keeps no history. The frame that turns it on (`init`) starts fresh
/// against its own values — an empty window, nothing to rate-cap from — so
/// that frame packs unchanged and nothing from before the off counts. Each
/// channel keeps its own flash window.
#[derive(Clone, Copy, Debug, Default)]
pub struct ChannelClamp {
    flash: ValueClamp,
    vignette: ValueClamp,
    fluid_tint: ValueClamp,
    /// Whether the previous frame ran with the clamp on. The first enabled
    /// frame after an off one, or of a new clamp, starts fresh.
    history_live: bool,
}

impl ChannelClamp {
    /// This frame's limiter input: the enable flag, presented-frame time, and
    /// whether history starts fresh. Called once per resolve frame, before
    /// [`Self::apply`].
    pub fn begin_frame(&mut self, input: LimiterFrameInput, enabled: bool) -> LimiterFrameUniform {
        let fresh = enabled && !self.history_live;
        self.history_live = enabled;
        pack_limiter_frame(input, enabled, fresh)
    }

    /// Limit `uniform`'s flash, vignette and fluid tint in place for one
    /// presented frame, each on its own channel.
    pub fn apply(&mut self, uniform: &mut EffectUniform, frame: &LimiterFrameUniform) {
        if !frame.enabled {
            return;
        }
        apply_channel(&mut self.flash, &mut uniform.flash, frame);
        apply_channel(&mut self.vignette, &mut uniform.vignette, frame);
        apply_channel(&mut self.fluid_tint, &mut uniform.fluid_tint, frame);
    }
}

#[cfg(test)]
#[path = "flash_clamp/flash_clamp_test_fixtures.rs"]
mod flash_clamp_test_fixtures;

#[cfg(test)]
mod tests {
    use super::flash_clamp_test_fixtures::worst_transitions;
    use super::value::{FLASH_LUMINANCE_THRESHOLD, RED_SATURATED, redness};
    use super::*;
    use crate::flash_limiter::{FLASH_LIMITER_SLOT, HITCH_CEILING_SECONDS, flash_limiter_enabled};
    use crate::screen_effects::pack_effect_uniform;
    use postretro_entities::SlotValue;
    use std::collections::HashMap;

    fn elapsed(dt: f32) -> LimiterFrameInput {
        LimiterFrameInput {
            elapsed_seconds: dt,
        }
    }

    fn frame(dt: f32, enabled: bool) -> LimiterFrameUniform {
        pack_limiter_frame(elapsed(dt), enabled, false)
    }

    fn flash(rgb: [f32; 3], a: f32) -> EffectUniform {
        EffectUniform {
            flash: [rgb[0], rgb[1], rgb[2], a],
            ..EffectUniform::default()
        }
    }

    fn run_flash_strobe(hz: f32, fps: f32, seconds: f32, rgb: [f32; 3]) -> Vec<EffectUniform> {
        let mut clamp = ChannelClamp::default();
        let dt = 1.0 / fps;
        (0..(seconds * fps) as usize)
            .map(|n| {
                let on = ((n as f32 * dt) * hz).fract() >= 0.5;
                let mut uniform = flash(rgb, if on { 1.0 } else { 0.0 });
                clamp.apply(&mut uniform, &frame(dt, true));
                uniform
            })
            .collect()
    }

    #[test]
    fn a_four_flash_screen_flash_strobe_packs_at_most_three_flashes() {
        let fps = 60.0;
        let packed = run_flash_strobe(4.0, fps, 2.0, [1.0, 1.0, 1.0]);
        let alphas: Vec<f32> = packed.iter().map(|u| u.flash[3]).collect();
        let worst = worst_transitions(&alphas, 1.0 / fps);
        assert!(
            (4..=6).contains(&worst),
            "{worst} transitions in one second"
        );
        // Limited, it rests at its pre-flash level.
        assert!(alphas.windows(8).any(|w| w.iter().all(|&a| a < 0.02)));
    }

    #[test]
    fn a_flash_strobe_that_dips_slightly_on_the_way_packs_at_most_three_flashes() {
        // 0 ↔ 0.19 at 6 Hz in 0.065 steps (under the rate cap) with a 0.001
        // dip between them. Restarting the excursion on each dip, no step
        // reached the threshold and twelve transitions a second packed.
        let cycle = [
            0.0, 0.065, 0.064, 0.129, 0.128, 0.193, 0.128, 0.129, 0.064, 0.065,
        ];
        let dt = 1.0 / 60.0;
        let mut clamp = ChannelClamp::default();
        let alphas: Vec<f32> = (0..120)
            .map(|n| {
                let mut uniform = flash([1.0; 3], cycle[n % cycle.len()]);
                clamp.apply(&mut uniform, &frame(dt, true));
                uniform.flash[3]
            })
            .collect();
        let worst = worst_transitions(&alphas, dt);
        assert!(worst <= 6, "{worst} transitions in one second");
    }

    #[test]
    fn a_red_screen_flash_strobe_over_budget_packs_desaturated() {
        let packed = run_flash_strobe(5.0, 60.0, 2.0, [1.0, 0.0, 0.0]);
        let desaturated = packed
            .iter()
            .filter(|u| u.flash[3] > 0.05)
            .any(|u| redness([u.flash[0], u.flash[1], u.flash[2]]) < RED_SATURATED);
        assert!(desaturated, "an over-budget red onset is desaturated");
    }

    #[test]
    fn a_single_saturated_red_flash_under_budget_keeps_its_hue() {
        // Dark, then one full-strength red flash for half a second and its
        // return: two onsets (strength and red) and their returns fit the
        // budget, so nothing is held or desaturated.
        let dt = 1.0 / 60.0;
        let red = [1.0, 0.0, 0.0];
        let mut clamp = ChannelClamp::default();
        let packed: Vec<EffectUniform> = (0..90)
            .map(|n| {
                let alpha = if (6..36).contains(&n) { 1.0 } else { 0.0 };
                let mut uniform = flash(red, alpha);
                clamp.apply(&mut uniform, &frame(dt, true));
                uniform
            })
            .collect();
        let alphas: Vec<f32> = packed.iter().map(|u| u.flash[3]).collect();
        // Rate-capped to 4.0/s, it still reaches full strength and returns.
        assert!(alphas.iter().any(|&a| a >= 1.0), "{alphas:?}");
        assert_eq!(alphas.last(), Some(&0.0));
        assert_eq!(worst_transitions(&alphas, dt), 2);
        for u in &packed {
            assert_eq!(
                [u.flash[0], u.flash[1], u.flash[2]],
                red,
                "desaturated at strength {}",
                u.flash[3]
            );
        }
    }

    #[test]
    fn off_passes_through_and_the_enabling_frame_starts_fresh() {
        let dt = 1.0 / 60.0;
        let mut clamp = ChannelClamp::default();
        let pack = |clamp: &mut ChannelClamp, alpha: f32, enabled: bool| {
            let frame = clamp.begin_frame(elapsed(dt), enabled);
            let mut uniform = flash([1.0; 3], alpha);
            clamp.apply(&mut uniform, &frame);
            (uniform, frame)
        };
        // On: dark for half a second, then a 7.5 Hz strobe spends the whole
        // budget in the half second before the off.
        for n in 0..60 {
            let alpha = if n >= 30 && ((n - 30) / 4) % 2 == 0 {
                1.0
            } else {
                0.0
            };
            let (_, frame) = pack(&mut clamp, alpha, true);
            assert_eq!(
                frame.init,
                n == 0,
                "only a new clamp's first frame starts fresh"
            );
        }
        // Off for half a second, mid-strobe, ending on a dark frame.
        for n in 60..90 {
            let alpha = ((n + 1) % 2) as f32;
            let (off, _) = pack(&mut clamp, alpha, false);
            assert_eq!(off, flash([1.0; 3], alpha), "off packs content unchanged");
        }
        // Turned on on a frame that jumps 0 → 1: that frame starts fresh and
        // packs unchanged, not rate-capped.
        let (enabling, frame) = pack(&mut clamp, 1.0, true);
        assert!(frame.init);
        assert_eq!(enabling.flash[3], 1.0);
        // Nothing from before the off counts: the fall that follows and the
        // next onset are both admitted, though the six transitions before the
        // off were all under a second old when it turned back on.
        let alphas: Vec<f32> = (1..=12)
            .map(|k| {
                let alpha = if (1..6).contains(&k) { 0.0 } else { 1.0 };
                let (packed, frame) = pack(&mut clamp, alpha, true);
                assert!(!frame.init, "history is live again");
                packed.flash[3]
            })
            .collect();
        let trough = alphas[..5].iter().copied().fold(f32::INFINITY, f32::min);
        let peak = alphas[5..].iter().copied().fold(0.0, f32::max);
        assert!(trough <= 0.9, "the fall is admitted: {alphas:?}");
        assert!(
            peak >= trough + 0.2,
            "the next onset is admitted: {alphas:?}"
        );
    }

    #[test]
    fn the_rate_cap_slows_a_full_strength_flash_and_clamps_a_hitch() {
        let mut clamp = ChannelClamp::default();
        let mut uniform = flash([1.0; 3], 1.0);
        clamp.apply(&mut uniform, &frame(2.0, true));
        assert!(
            uniform.flash[3] <= 4.0 / 30.0 + 1e-6,
            "{}",
            uniform.flash[3]
        );
    }

    /// A 5 Hz strobe of `shape` (strength over time) packed at `fps` for
    /// `seconds`, as flash alphas.
    fn packed_strobe(shape: fn(f32) -> f32, fps: f32, seconds: f32) -> Vec<f32> {
        let dt = 1.0 / fps;
        let mut clamp = ChannelClamp::default();
        (0..(seconds * fps).round() as usize)
            .map(|n| {
                let mut uniform = flash([1.0; 3], shape(n as f32 * dt));
                clamp.apply(&mut uniform, &frame(dt, true));
                uniform.flash[3]
            })
            .collect()
    }

    fn square_5hz(t: f32) -> f32 {
        if (t * 5.0).fract() < 0.5 { 1.0 } else { 0.0 }
    }

    fn sine_5hz(t: f32) -> f32 {
        0.5 - 0.5 * (std::f32::consts::TAU * 5.0 * t).cos()
    }

    #[test]
    fn a_5hz_strobe_packs_the_same_three_flashes_at_30_and_240_hz() {
        for (name, shape) in [("square", square_5hz as fn(f32) -> f32), ("sine", sine_5hz)] {
            // Unlimited, the strobe is ten transitions a second.
            let raw: Vec<f32> = (0..240).map(|n| shape(n as f32 / 240.0)).collect();
            assert!(worst_transitions(&raw, 1.0 / 240.0) >= 9, "{name}");

            let counts = [30.0, 240.0].map(|fps| {
                let alphas = packed_strobe(shape, fps, 3.0);
                worst_transitions(&alphas, 1.0 / fps)
            });
            assert!(
                (4..=6).contains(&counts[0]),
                "{name}: {} transitions in one second at 30 Hz",
                counts[0]
            );
            assert_eq!(counts[0], counts[1], "{name}: 30 Hz vs 240 Hz");
        }
    }

    #[test]
    fn a_strobe_alternating_every_frame_changes_no_faster_than_the_rate_cap() {
        for fps in [30.0, 60.0, 144.0, 240.0] {
            let dt = 1.0 / fps;
            let mut clamp = ChannelClamp::default();
            let mut previous = 0.0;
            for n in 0..(2.0 * fps) as usize {
                let mut uniform = flash([1.0; 3], (n % 2) as f32);
                clamp.apply(&mut uniform, &frame(dt, true));
                let step = (uniform.flash[3] - previous).abs();
                assert!(
                    step <= INTENSITY_RATE_PER_SECOND * dt + 1e-6,
                    "{fps} Hz frame {n}: step {step}"
                );
                previous = uniform.flash[3];
            }
        }
    }

    #[test]
    fn a_hitch_ages_out_earlier_transitions_and_caps_its_own_step() {
        let dt = 1.0 / 60.0;
        // Dark for half a second, then a 7.5 Hz strobe spends the whole
        // budget in the next half second. It settles dark for 0.1 s, then a
        // frame that jumps to full strength arrives after `gap` seconds and a
        // 5 Hz strobe runs for another second.
        let run = |gap: f32| {
            let mut clamp = ChannelClamp::default();
            let mut pack = |alpha: f32, dt: f32| {
                let mut uniform = flash([1.0; 3], alpha);
                clamp.apply(&mut uniform, &frame(dt, true));
                uniform.flash[3]
            };
            for n in 0..60 {
                let on = n >= 30 && ((n - 30) / 4) % 2 == 0;
                pack(if on { 1.0 } else { 0.0 }, dt);
            }
            let mut after = Vec::new();
            for _ in 0..6 {
                after.push(pack(0.0, dt));
            }
            after.push(pack(1.0, gap));
            for n in 1..60 {
                after.push(pack(square_5hz(n as f32 * dt), dt));
            }
            after
        };

        let hitched = run(2.0);
        let settled = hitched[5];
        assert_eq!(settled, 0.0, "the strobe settled dark before the hitch");
        let step = hitched[6] - settled;
        assert!(
            step > 0.0 && step <= INTENSITY_RATE_PER_SECOND * HITCH_CEILING_SECONDS + 1e-6,
            "the hitch frame steps {step}"
        );
        // A fresh budget: the onset that starts at the hitch is admitted, and
        // the second after it shows the full three flashes. The same frames at
        // a steady cadence hold that onset, because the window still holds the
        // strobe before it.
        let onset_peak = |alphas: &[f32]| alphas[6..12].iter().copied().fold(0.0, f32::max);
        assert!(onset_peak(&hitched) >= 0.3, "{hitched:?}");
        let steady = run(dt);
        assert!(
            onset_peak(&steady) < FLASH_LUMINANCE_THRESHOLD,
            "{steady:?}"
        );
        let fresh = worst_transitions(&hitched[5..], dt);
        assert!(
            (4..=6).contains(&fresh),
            "{fresh} transitions after the hitch"
        );
    }

    #[test]
    fn a_vignette_strobe_is_limited_on_its_own_channel() {
        let fps = 60.0;
        let dt = 1.0 / fps;
        let four_hz = |t: f32| if (t * 4.0).fract() < 0.5 { 1.0 } else { 0.0 };
        // The vignette strobes at 4 Hz; with `flash_too`, a 4 Hz flash strobe
        // spends the flash channel's budget out of phase with it.
        let run = |flash_too: bool| {
            let mut clamp = ChannelClamp::default();
            (0..(2.0 * fps) as usize)
                .map(|n| {
                    let t = n as f32 * dt;
                    let flash_a = if flash_too { four_hz(t + 0.125) } else { 0.0 };
                    let mut uniform = EffectUniform {
                        vignette: [0.0, 0.0, 0.0, four_hz(t)],
                        ..flash([1.0; 3], flash_a)
                    };
                    clamp.apply(&mut uniform, &frame(dt, true));
                    uniform
                })
                .collect::<Vec<_>>()
        };
        let alone = run(false);
        let strengths: Vec<f32> = alone.iter().map(|u| u.vignette[3]).collect();
        let worst = worst_transitions(&strengths, dt);
        assert!((4..=6).contains(&worst), "{worst} vignette transitions");
        // Its own window: a flash strobe beside it changes none of it.
        let beside = run(true);
        let flashes: Vec<f32> = beside.iter().map(|u| u.flash[3]).collect();
        assert!((4..=6).contains(&worst_transitions(&flashes, dt)));
        for (a, b) in alone.iter().zip(&beside) {
            assert_eq!(a.vignette, b.vignette);
        }
    }

    #[test]
    fn a_fluid_tint_strobe_is_limited_on_its_own_channel_and_passes_when_off() {
        let fps = 60.0;
        let dt = 1.0 / fps;
        let water = |strength: f32| [0.1, 0.3, 0.4, strength];
        // The fluid tint strobes at 5 Hz beside a steady mod flash.
        let run = |enabled: bool| {
            let mut clamp = ChannelClamp::default();
            (0..(2.0 * fps) as usize)
                .map(|n| {
                    let mut uniform = EffectUniform {
                        fluid_tint: water(square_5hz(n as f32 * dt)),
                        ..flash([0.8, 0.2, 0.1], 0.3)
                    };
                    let frame = clamp.begin_frame(elapsed(dt), enabled);
                    clamp.apply(&mut uniform, &frame);
                    uniform
                })
                .collect::<Vec<_>>()
        };
        let limited = run(true);
        let strengths: Vec<f32> = limited.iter().map(|u| u.fluid_tint[3]).collect();
        let worst = worst_transitions(&strengths, dt);
        assert!((4..=6).contains(&worst), "{worst} fluid tint transitions");
        for u in &limited {
            assert_eq!(
                u.flash,
                [0.8, 0.2, 0.1, 0.3],
                "the flash channel is untouched"
            );
        }
        for (n, u) in run(false).iter().enumerate() {
            assert_eq!(
                u.fluid_tint,
                water(square_5hz(n as f32 * dt)),
                "off passes through"
            );
        }
    }

    /// Pack a red flash and vignette strobe through the resolve's own path:
    /// the slots pack, the enable flag is read fail-safe, and the clamp
    /// limits. Returns what was authored and what packed, frame by frame.
    fn pack_slots(flag: Option<SlotValue>) -> Vec<(EffectUniform, EffectUniform)> {
        let dt = 1.0 / 60.0;
        let mut clamp = ChannelClamp::default();
        (0..120)
            .map(|n| {
                let t = n as f32 * dt;
                let mut slots = HashMap::from([
                    (
                        "screen.flash".to_string(),
                        SlotValue::Array(vec![1.0, 0.0, 0.0, square_5hz(t)]),
                    ),
                    (
                        "screen.vignette".to_string(),
                        SlotValue::Array(vec![1.0, 0.0, 0.0, square_5hz(t + 0.1)]),
                    ),
                ]);
                if let Some(flag) = &flag {
                    slots.insert(FLASH_LIMITER_SLOT.to_string(), flag.clone());
                }
                let authored = pack_effect_uniform(&slots, [0.0; 4]);
                let frame = clamp.begin_frame(elapsed(dt), flash_limiter_enabled(&slots));
                let mut packed = authored;
                clamp.apply(&mut packed, &frame);
                (authored, packed)
            })
            .collect()
    }

    #[test]
    fn the_limiter_runs_when_its_flag_is_absent_or_malformed() {
        let dt = 1.0 / 60.0;
        for flag in [
            None,
            Some(SlotValue::Boolean(true)),
            Some(SlotValue::Number(0.0)),
            Some(SlotValue::String("off".into())),
        ] {
            let packed = pack_slots(flag.clone());
            let flashes: Vec<f32> = packed.iter().map(|(_, p)| p.flash[3]).collect();
            let vignettes: Vec<f32> = packed.iter().map(|(_, p)| p.vignette[3]).collect();
            assert!(worst_transitions(&flashes, dt) <= 6, "flash, {flag:?}");
            assert!(worst_transitions(&vignettes, dt) <= 6, "vignette, {flag:?}");
        }
    }

    #[test]
    fn an_explicit_false_packs_flash_and_vignette_as_authored() {
        let dt = 1.0 / 60.0;
        let packed = pack_slots(Some(SlotValue::Boolean(false)));
        for (authored, packed) in &packed {
            assert_eq!(packed, authored);
        }
        // What passed through is a strobe the limiter would have held.
        let flashes: Vec<f32> = packed.iter().map(|(_, p)| p.flash[3]).collect();
        let vignettes: Vec<f32> = packed.iter().map(|(_, p)| p.vignette[3]).collect();
        assert!(worst_transitions(&flashes, dt) > 6);
        assert!(worst_transitions(&vignettes, dt) > 6);
    }
}
