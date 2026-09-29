// Mono fold: a main-track effect that folds left and right together after
// spatialization, crossfading on toggle. Player accessibility option.
// See: context/lib/audio.md §1 (Mixer bus tree)

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use kira::Frame;
use kira::effect::{Effect, EffectBuilder};
use kira::info::Info;

/// Crossfade length for a toggle. At least 10 ms, so a toggle never steps the
/// output.
pub(crate) const MONO_CROSSFADE_SECONDS: f64 = 0.020;

/// Game-thread control for the fold. Cheap to clone; the audio thread reads the
/// flag once per processed block.
#[derive(Clone, Debug, Default)]
pub(crate) struct MonoFoldHandle {
    on: Arc<AtomicBool>,
}

impl MonoFoldHandle {
    pub(crate) fn set(&self, on: bool) {
        self.on.store(on, Ordering::Relaxed);
    }
}

/// Builds the effect sharing `handle`'s flag. Added to the main track at
/// manager build, so it runs after every sub-track, spatial panning included.
pub(crate) struct MonoFoldBuilder {
    handle: MonoFoldHandle,
}

impl MonoFoldBuilder {
    pub(crate) fn new(handle: MonoFoldHandle) -> Self {
        Self { handle }
    }
}

impl EffectBuilder for MonoFoldBuilder {
    type Handle = ();

    fn build(self) -> (Box<dyn Effect>, Self::Handle) {
        (
            Box::new(MonoFold {
                on: self.handle.on,
                mix: 0.0,
            }),
            (),
        )
    }
}

pub(crate) struct MonoFold {
    on: Arc<AtomicBool>,
    /// 0 = stereo, 1 = fully folded. Moves toward the flag at the crossfade
    /// rate, so a toggle reversed mid-crossfade turns back from where it is.
    mix: f32,
}

impl Effect for MonoFold {
    fn process(&mut self, input: &mut [Frame], dt: f64, _info: &Info) {
        let target = if self.on.load(Ordering::Relaxed) {
            1.0
        } else {
            0.0
        };
        // Settled stereo: leave the block untouched.
        if self.mix == 0.0 && target == 0.0 {
            return;
        }
        let step = (dt / MONO_CROSSFADE_SECONDS) as f32;
        for frame in input.iter_mut() {
            self.mix = if self.mix < target {
                (self.mix + step).min(target)
            } else {
                (self.mix - step).max(target)
            };
            let mono = (frame.left + frame.right) * 0.5;
            frame.left += (mono - frame.left) * self.mix;
            frame.right += (mono - frame.right) * self.mix;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use kira::info::MockInfoBuilder;

    const SAMPLE_RATE: f64 = 8_000.0;

    /// A hard-left tone, one second long.
    fn hard_left(frames: usize) -> Vec<Frame> {
        (0..frames)
            .map(|n| {
                let t = n as f64 / SAMPLE_RATE;
                Frame {
                    left: (t * 440.0 * std::f64::consts::TAU).sin() as f32,
                    right: 0.0,
                }
            })
            .collect()
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

    fn fold() -> (MonoFold, MonoFoldHandle) {
        let handle = MonoFoldHandle::default();
        let effect = MonoFold {
            on: handle.on.clone(),
            mix: 0.0,
        };
        (effect, handle)
    }

    fn process(effect: &mut MonoFold, frames: &mut [Frame]) {
        let info = MockInfoBuilder::new().build();
        effect.process(frames, 1.0 / SAMPLE_RATE, &info);
    }

    #[test]
    fn folded_output_is_equal_left_and_right_and_off_restores_stereo() {
        let (mut effect, handle) = fold();
        handle.set(true);
        let mut block = hard_left(800);
        process(&mut effect, &mut block);
        for frame in &block[400..] {
            assert!((frame.left - frame.right).abs() < 1e-6, "{frame:?}");
        }

        handle.set(false);
        let mut settle = hard_left(800);
        process(&mut effect, &mut settle);
        let mut stereo = hard_left(800);
        let reference = stereo.clone();
        process(&mut effect, &mut stereo);
        assert_eq!(stereo, reference, "settled stereo passes untouched");
    }

    #[test]
    fn a_toggle_reversed_mid_crossfade_turns_back_without_a_step() {
        let frames = 1600;
        let untoggled = hard_left(frames);
        let limit = largest_step(&untoggled);

        let (mut effect, handle) = fold();
        let mut signal = hard_left(frames);
        // On for half the crossfade (80 frames at 8 kHz = 10 ms), then off.
        handle.set(true);
        process(&mut effect, &mut signal[..80]);
        let turned_at = effect.mix;
        assert!((turned_at - 0.5).abs() < 0.02, "halfway: {turned_at}");
        handle.set(false);
        process(&mut effect, &mut signal[80..]);

        assert!(
            largest_step(&signal) <= limit + 1e-6,
            "no toggle step exceeds the untoggled signal's largest step"
        );
        // Stereo returns within the elapsed half of the crossfade.
        let returned = &signal[80 + 81..];
        let reference = &untoggled[80 + 81..];
        assert_eq!(returned, reference);
    }
}
