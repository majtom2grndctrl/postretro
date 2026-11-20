// Brightness animation clock for emissive volumes: per-volume run start
// times, curve sampling on the script clock, and finite-run settling.
// See: context/lib/scripting.md §10.2a

use postretro_entities::{EmissiveAnimation, EmissiveVolumeComponent};

use crate::scripting_systems::fog_volume_bridge::sample_linear_curve;

/// One volume's running animation: when its run started and which run it is.
/// The start is engine state, so it lives here rather than on the
/// script-visible component.
struct AnimSlot {
    start_ms: f64,
    /// The component's `animation_serial` when this run started.
    serial: u32,
    /// The curve this run samples. A component whose curve differs, however
    /// it got there, restarts.
    animation: EmissiveAnimation,
}

/// Start times per volume, indexed in id-60 record order. Sized at level
/// load; a frame with no new install allocates nothing.
pub(super) struct EmissiveAnimClock {
    slots: Vec<Option<AnimSlot>>,
}

impl EmissiveAnimClock {
    pub(super) fn new() -> Self {
        Self { slots: Vec::new() }
    }

    /// One empty slot per volume.
    pub(super) fn reset(&mut self, volume_count: usize) {
        self.slots.clear();
        self.slots.resize_with(volume_count, || None);
    }

    /// Start volume `index`'s current animation at `start_ms`, whatever run
    /// the slot held. Level install seeds `levelLoad` animations at script
    /// time 0, so the first running frame samples from the curve's start.
    pub(super) fn seed(&mut self, index: usize, volume: &EmissiveVolumeComponent, start_ms: f64) {
        let Some(slot) = self.slots.get_mut(index) else {
            return;
        };
        *slot = volume.animation.as_ref().map(|animation| AnimSlot {
            start_ms,
            serial: volume.animation_serial,
            animation: animation.clone(),
        });
    }

    /// Advance volume `index` to `now_ms` on the freeze-aware script clock:
    /// write its animation's sample into `brightness`, or, once a finite run
    /// has played its count, write the last sample and clear the animation.
    ///
    /// A run restarts from its first sample when the component's
    /// `animation_serial` or curve differs from the slot's, so a brightness
    /// write followed by an identical re-install in one drain restarts
    /// (`research.md` P30), while a re-install of the running curve, which
    /// keeps the serial, keeps its start (P6). A colour write touches
    /// neither, so the run and its samples carry on (P44).
    pub(super) fn advance(
        &mut self,
        index: usize,
        volume: &mut EmissiveVolumeComponent,
        now_ms: f64,
    ) {
        let Some(slot) = self.slots.get_mut(index) else {
            return;
        };
        let Some(animation) = volume.animation.as_ref() else {
            *slot = None;
            return;
        };
        // Install validation rejects these; never sample them.
        if !animation.period_ms.is_finite()
            || animation.period_ms <= 0.0
            || animation.brightness.is_empty()
        {
            return;
        }
        let running = slot.as_ref().is_some_and(|running| {
            running.serial == volume.animation_serial && running.animation == *animation
        });
        if !running {
            match slot {
                Some(restart) => {
                    restart.start_ms = now_ms;
                    restart.serial = volume.animation_serial;
                    restart.animation.clone_from(animation);
                }
                None => {
                    *slot = Some(AnimSlot {
                        start_ms: now_ms,
                        serial: volume.animation_serial,
                        animation: animation.clone(),
                    });
                }
            }
        }
        let start_ms = slot.as_ref().map_or(now_ms, |running| running.start_ms);

        // In f64: at long uptimes an f32 elapsed time loses the precision the
        // period boundary and the phase need.
        let elapsed_periods = (now_ms - start_ms) / f64::from(animation.period_ms);
        if animation
            .play_count
            .is_some_and(|count| elapsed_periods >= f64::from(count))
        {
            let last = *animation
                .brightness
                .last()
                .expect("a sampled curve is non-empty");
            volume.brightness = last;
            volume.animation = None;
            *slot = None;
            return;
        }
        let phase = elapsed_periods.rem_euclid(1.0) as f32;
        volume.brightness = sample_linear_curve(&animation.brightness, phase);
    }
}
