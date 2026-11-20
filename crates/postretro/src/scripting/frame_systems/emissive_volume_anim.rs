// Brightness animation clock for emissive volumes: per-volume run start
// times, curve sampling on the script clock, and finite-run settling.
// See: context/lib/scripting.md §10.2a

use postretro_entities::{EmissiveAnimation, EmissiveVolumeComponent};

use crate::scripting_systems::curve_sample::sample_linear_curve;

/// One volume's running animation: when its run started and which run it is.
/// The start is engine state, so it lives here rather than on the
/// script-visible component.
struct AnimSlot {
    start_ms: f64,
    /// The component's `animation_serial` when this run started.
    serial: u32,
    /// The component's `install_touch` as of the last tick (or seed): a
    /// different value means a script installed a curve since.
    touch: u32,
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
    ///
    /// The seed takes the component's install touch as seen: the start it
    /// places is the answer, so no install before it restarts the run.
    pub(super) fn seed(&mut self, index: usize, volume: &EmissiveVolumeComponent, start_ms: f64) {
        let Some(slot) = self.slots.get_mut(index) else {
            return;
        };
        *slot = volume.animation.as_ref().map(|animation| AnimSlot {
            start_ms,
            serial: volume.animation_serial,
            touch: volume.install_touch,
            animation: animation.clone(),
        });
    }

    /// Keep volume `index`'s running start but adopt the component's current
    /// run, curve and serial: a replicated record that names no start
    /// continues the local clock, as fog's does. No animation drops the slot;
    /// no slot leaves the run to start lazily at the next advance.
    ///
    /// Co-op apply writes a replicated curve through `install_animation`,
    /// which touches the component; adopting takes that touch as seen, so
    /// applying a record never restarts a run the local clock has finished.
    pub(super) fn adopt(&mut self, index: usize, volume: &EmissiveVolumeComponent) {
        let Some(slot) = self.slots.get_mut(index) else {
            return;
        };
        match (volume.animation.as_ref(), slot.as_mut()) {
            (None, _) => *slot = None,
            (Some(animation), Some(running)) => {
                running.serial = volume.animation_serial;
                running.touch = volume.install_touch;
                running.animation.clone_from(animation);
            }
            (Some(_), None) => {}
        }
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
    ///
    /// One re-install of the running curve does restart: on a finite run
    /// already finished at `now_ms`. The drain that installed it ran before
    /// this settle, so without the restart the install would be lost; after
    /// a finite run settles, re-install restarts it (P6). The component's
    /// `install_touch` shows the install.
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
                    restart.touch = volume.install_touch;
                    restart.animation.clone_from(animation);
                }
                None => {
                    *slot = Some(AnimSlot {
                        start_ms: now_ms,
                        serial: volume.animation_serial,
                        touch: volume.install_touch,
                        animation: animation.clone(),
                    });
                }
            }
        } else if let Some(running) = slot.as_mut()
            && running.touch != volume.install_touch
        {
            // Re-installed since the last tick: a run still playing keeps
            // its start; one this frame would settle starts over instead.
            if animation.has_finished(running.start_ms, now_ms) {
                running.start_ms = now_ms;
            }
            running.touch = volume.install_touch;
        }
        let start_ms = slot.as_ref().map_or(now_ms, |running| running.start_ms);

        if animation.has_finished(start_ms, now_ms) {
            let last = *animation
                .brightness
                .last()
                .expect("a sampled curve is non-empty");
            volume.brightness = last;
            volume.animation = None;
            *slot = None;
            return;
        }
        // In f64: at long uptimes an f32 elapsed time loses the precision the
        // phase needs.
        let elapsed_periods = (now_ms - start_ms) / f64::from(animation.period_ms);
        let phase = elapsed_periods.rem_euclid(1.0) as f32;
        volume.brightness = sample_linear_curve(&animation.brightness, phase);
    }
}
