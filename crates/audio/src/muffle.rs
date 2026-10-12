// World muffle: a low-pass effect on the SFX (world-sound) bus, ramping between
// amounts on the audio thread. Driven by the camera eye being in a fluid.
// See: context/lib/audio.md §1 (Mixer bus tree)

use std::f32::consts::PI;
use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};

use kira::Frame;
use kira::effect::{Effect, EffectBuilder};
use kira::info::Info;

/// Seconds for the amount to travel the whole 0..1 range. The caller already
/// ramps the amount per frame; this ramp smooths those per-frame steps and
/// turns a reversed change back from where it is, never stepping the output.
pub(crate) const MUFFLE_RAMP_SECONDS: f64 = 0.05;

/// Cutoff at the lightest muffle: above hearing, so the onset is inaudible.
const OPEN_CUTOFF_HZ: f32 = 20_000.0;

/// Cutoff at amount 1: underwater-dull, voices still intelligible as thuds.
pub(crate) const MAX_MUFFLE_CUTOFF_HZ: f32 = 300.0;

/// Below this amount the filtered signal crossfades in from dry, so amount 0
/// is exactly dry and engaging from a resting filter never steps the output.
const WET_FADE_AMOUNT: f32 = 0.05;

/// The cutoff stays below Nyquist whatever the device rate.
const MAX_CUTOFF_FRACTION_OF_RATE: f32 = 0.45;

/// Butterworth damping (k = 1/Q): a flat passband with no resonant bump.
const DAMPING: f32 = std::f32::consts::SQRT_2;

/// The low-pass cutoff for `amount`, log-linear from [`OPEN_CUTOFF_HZ`] at 0
/// to [`MAX_MUFFLE_CUTOFF_HZ`] at 1, so equal amount steps sound like equal
/// steps of dullness.
pub(crate) fn muffle_cutoff_hz(amount: f32) -> f32 {
    OPEN_CUTOFF_HZ * (MAX_MUFFLE_CUTOFF_HZ / OPEN_CUTOFF_HZ).powf(amount.clamp(0.0, 1.0))
}

/// Game-thread control for the muffle. Cheap to clone; the audio thread reads
/// the target once per processed block. Holds `f32` bits; the default, all
/// zero bits, is amount 0.
#[derive(Clone, Debug, Default)]
pub(crate) struct WorldMuffleHandle {
    target: Arc<AtomicU32>,
}

impl WorldMuffleHandle {
    /// Set the target amount, clamped to `[0, 1]`; non-finite reads as 0.
    pub(crate) fn set(&self, amount: f32) {
        let amount = if amount.is_finite() {
            amount.clamp(0.0, 1.0)
        } else {
            0.0
        };
        self.target.store(amount.to_bits(), Ordering::Relaxed);
    }
}

/// Builds the effect sharing `handle`'s target. Added to the SFX bus track at
/// build, so it filters that bus's sounds and every positional voice's child
/// track, and never touches Music or UI.
pub(crate) struct WorldMuffleBuilder {
    handle: WorldMuffleHandle,
}

impl WorldMuffleBuilder {
    pub(crate) fn new(handle: WorldMuffleHandle) -> Self {
        Self { handle }
    }
}

impl EffectBuilder for WorldMuffleBuilder {
    type Handle = ();

    fn build(self) -> (Box<dyn Effect>, Self::Handle) {
        (Box::new(WorldMuffle::new(self.handle.target)), ())
    }
}

/// Topology-preserving state-variable filter coefficients (Simper), stable
/// while the cutoff moves sample by sample.
#[derive(Clone, Copy, Debug, Default)]
struct Coefficients {
    a1: f32,
    a2: f32,
    a3: f32,
}

impl Coefficients {
    fn low_pass(cutoff_hz: f32, dt: f32) -> Self {
        let cutoff = cutoff_hz.min(MAX_CUTOFF_FRACTION_OF_RATE / dt);
        let g = (PI * cutoff * dt).tan();
        let a1 = 1.0 / (1.0 + g * (g + DAMPING));
        let a2 = g * a1;
        Self { a1, a2, a3: g * a2 }
    }
}

/// One channel's filter memory.
#[derive(Clone, Copy, Debug, Default)]
struct Channel {
    ic1: f32,
    ic2: f32,
}

impl Channel {
    fn low_pass(&mut self, input: f32, c: Coefficients) -> f32 {
        let v3 = input - self.ic2;
        let v1 = c.a1 * self.ic1 + c.a2 * v3;
        let v2 = self.ic2 + c.a2 * self.ic1 + c.a3 * v3;
        self.ic1 = 2.0 * v1 - self.ic1;
        self.ic2 = 2.0 * v2 - self.ic2;
        v2
    }
}

pub(crate) struct WorldMuffle {
    target: Arc<AtomicU32>,
    /// The amount being applied, moving toward the target at the ramp rate.
    amount: f32,
    /// Coefficients for `coefficients_for` = (amount, dt); recomputed only
    /// while the amount moves or the rate changes.
    coefficients: Coefficients,
    coefficients_for: Option<(f32, f32)>,
    channels: [Channel; 2],
}

impl WorldMuffle {
    fn new(target: Arc<AtomicU32>) -> Self {
        Self {
            target,
            amount: 0.0,
            coefficients: Coefficients::default(),
            coefficients_for: None,
            channels: [Channel::default(); 2],
        }
    }

    fn coefficients(&mut self, dt: f32) -> Coefficients {
        if self.coefficients_for != Some((self.amount, dt)) {
            self.coefficients = Coefficients::low_pass(muffle_cutoff_hz(self.amount), dt);
            self.coefficients_for = Some((self.amount, dt));
        }
        self.coefficients
    }
}

impl Effect for WorldMuffle {
    fn process(&mut self, input: &mut [Frame], dt: f64, _info: &Info) {
        let target = f32::from_bits(self.target.load(Ordering::Relaxed));
        // Settled dry: leave the block untouched and let the filter rest, so
        // the next engage starts from silence under a zero wet mix.
        if self.amount == 0.0 && target == 0.0 {
            self.channels = [Channel::default(); 2];
            return;
        }
        let step = (dt / MUFFLE_RAMP_SECONDS) as f32;
        let dt = dt as f32;
        for frame in input.iter_mut() {
            self.amount = if self.amount < target {
                (self.amount + step).min(target)
            } else {
                (self.amount - step).max(target)
            };
            let c = self.coefficients(dt);
            let wet = (self.amount / WET_FADE_AMOUNT).min(1.0);
            let left = self.channels[0].low_pass(frame.left, c);
            let right = self.channels[1].low_pass(frame.right, c);
            frame.left += (left - frame.left) * wet;
            frame.right += (right - frame.right) * wet;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use kira::info::MockInfoBuilder;

    const SAMPLE_RATE: f64 = 48_000.0;

    fn tone(hz: f64, frames: usize) -> Vec<Frame> {
        (0..frames)
            .map(|n| {
                let s = ((n as f64 / SAMPLE_RATE) * hz * std::f64::consts::TAU).sin() as f32;
                Frame { left: s, right: s }
            })
            .collect()
    }

    fn rms(frames: &[Frame]) -> f32 {
        let sum: f32 = frames.iter().map(|f| f.left * f.left).sum();
        (sum / frames.len() as f32).sqrt()
    }

    fn largest_step(frames: &[Frame]) -> f32 {
        frames
            .windows(2)
            .map(|w| {
                (w[1].left - w[0].left)
                    .abs()
                    .max((w[1].right - w[0].right).abs())
            })
            .fold(0.0, f32::max)
    }

    fn muffle() -> (WorldMuffle, WorldMuffleHandle) {
        let handle = WorldMuffleHandle::default();
        (WorldMuffle::new(handle.target.clone()), handle)
    }

    fn process(effect: &mut WorldMuffle, frames: &mut [Frame]) {
        let info = MockInfoBuilder::new().build();
        effect.process(frames, 1.0 / SAMPLE_RATE, &info);
    }

    #[test]
    fn muffle_cutoff_falls_log_linearly_from_open_to_its_floor() {
        assert!((muffle_cutoff_hz(0.0) - OPEN_CUTOFF_HZ).abs() < 1e-2);
        assert!((muffle_cutoff_hz(1.0) - MAX_MUFFLE_CUTOFF_HZ).abs() < 1e-2);
        let half = muffle_cutoff_hz(0.5);
        assert!((half - (OPEN_CUTOFF_HZ * MAX_MUFFLE_CUTOFF_HZ).sqrt()).abs() < 1.0);
        assert!(muffle_cutoff_hz(0.8) < half && half < muffle_cutoff_hz(0.2));
    }

    #[test]
    fn muffle_at_zero_passes_untouched_and_at_full_dulls_highs_not_lows() {
        let (mut effect, handle) = muffle();
        let mut dry = tone(3_000.0, 4_800);
        let reference = dry.clone();
        process(&mut effect, &mut dry);
        assert_eq!(dry, reference, "amount 0 leaves the block untouched");

        handle.set(1.0);
        // Settle past the ramp, then measure.
        process(&mut effect, &mut tone(3_000.0, 4_800));
        let mut high = tone(3_000.0, 4_800);
        process(&mut effect, &mut high);
        assert!(
            rms(&high) < 0.02,
            "3 kHz under a 300 Hz cutoff: {}",
            rms(&high)
        );
        let mut low = tone(60.0, 9_600);
        process(&mut effect, &mut low);
        assert!(
            rms(&low[4_800..]) > 0.65,
            "60 Hz passes: {}",
            rms(&low[4_800..])
        );
    }

    #[test]
    fn a_muffle_toggle_reversed_mid_ramp_turns_back_without_a_step() {
        let frames = 9_600;
        let untoggled = tone(200.0, frames);
        let limit = largest_step(&untoggled);
        let step = (1.0 / (SAMPLE_RATE * MUFFLE_RAMP_SECONDS)) as f32;
        let half_ramp = (SAMPLE_RATE * MUFFLE_RAMP_SECONDS / 2.0) as usize;

        let (mut effect, handle) = muffle();
        let mut signal = untoggled.clone();
        let mut amounts = Vec::with_capacity(frames);
        // On for half the ramp, then off, sampling the applied amount frame by
        // frame across the reversal.
        handle.set(1.0);
        for n in 0..frames {
            if n == half_ramp {
                handle.set(0.0);
            }
            process(&mut effect, &mut signal[n..n + 1]);
            amounts.push(effect.amount);
        }
        let turned_at = amounts[half_ramp - 1];
        assert!((turned_at - 0.5).abs() < 0.01, "halfway: {turned_at}");
        for (n, pair) in amounts.windows(2).enumerate() {
            let delta = pair[1] - pair[0];
            assert!(
                delta.abs() <= step + 1e-6,
                "frame {n}: amount jumps {delta}"
            );
            if n + 1 < half_ramp - 1 {
                assert!(delta > 0.0, "frame {n}: still rising");
            } else if n + 1 >= half_ramp && pair[0] > 0.0 {
                assert!(delta < 0.0, "frame {n}: turned back");
            }
        }
        assert_eq!(amounts.last(), Some(&0.0), "returns to dry");
        assert!(
            largest_step(&signal) <= limit * 1.05,
            "no output step beyond the untoggled signal's: {} vs {limit}",
            largest_step(&signal)
        );
        // Dry again, the signal passes untouched.
        let settled = 2 * half_ramp + 1;
        assert_eq!(&signal[settled..], &untoggled[settled..]);
    }

    #[test]
    fn a_non_finite_or_out_of_range_amount_is_clamped() {
        let handle = WorldMuffleHandle::default();
        for (set, expect) in [
            (f32::NAN, 0.0),
            (f32::INFINITY, 0.0),
            (-1.0, 0.0),
            (3.0, 1.0),
        ] {
            handle.set(set);
            assert_eq!(
                f32::from_bits(handle.target.load(Ordering::Relaxed)),
                expect
            );
        }
    }
}
