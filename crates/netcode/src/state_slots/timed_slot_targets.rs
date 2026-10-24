// Display targets derived from host-stamped weapon timers, independent of the store.
// See: context/lib/networking.md §What gates, and what replicates instead

use crate::weapon::{ReplicatedWeaponProjection, SlotSample};
use postretro_entities::SlotValue;

mod client;
mod ramp;
use ramp::Ramp;

pub(crate) const SLOPE_TOLERANCE: f64 = 0.08;
pub(crate) const HOLD_CAP_TICKS: f64 = 16.0;
pub(crate) const TOP_SLEW_RATE: f64 = 1.25;
const TICKS_PER_SECOND: f64 = 60.0;
pub(crate) const NAMES: [&str; 4] = [
    "player.reloadProgress",
    "player.weaponCooldownMs",
    "player.heat",
    "player.cell",
];

#[derive(Default)]
pub(super) struct TimedSlotTargets {
    timeline: Option<f64>,
    ramps: [Ramp; 4],
}

impl TimedSlotTargets {
    pub(super) fn accept(
        &mut self,
        tick: u32,
        raw: &ReplicatedWeaponProjection,
        excluded: [bool; 4],
    ) {
        let number = |s: Option<SlotSample<f32>>| {
            s.map(|s| SlotSample {
                slot: s.slot,
                value: Some(s.value),
            })
        };
        let samples = [
            number(raw.reload_progress),
            number(raw.cooldown),
            raw.heat,
            raw.cell,
        ];
        for (index, sample) in samples.into_iter().enumerate() {
            if excluded[index] {
                continue;
            }
            let bound = match index {
                0 => Some(1.0),
                1 | 2 => Some(0.0),
                3 => raw
                    .cell_capacity
                    .filter(|capacity| sample.is_some_and(|s| s.slot == capacity.slot))
                    .and_then(|s| s.value),
                _ => unreachable!(),
            };
            self.ramps[index].accept(tick, sample, bound, index == 0 || index == 3, self.timeline);
        }
    }

    pub(super) fn advance(&mut self, estimate: Option<f64>, dt: f64) {
        let Some(estimate) = estimate.filter(|tick| tick.is_finite()) else {
            for ramp in &mut self.ramps {
                ramp.show_raw();
            }
            return;
        };
        let timeline = match self.timeline {
            None => estimate,
            Some(previous) => {
                let elapsed = if dt.is_finite() {
                    dt.max(0.0) * TICKS_PER_SECOND
                } else {
                    0.0
                };
                // Slew from frame time, not the estimator's step at a sync echo.
                previous + (estimate - previous).clamp(0.75 * elapsed, TOP_SLEW_RATE * elapsed)
            }
        };
        self.timeline = Some(timeline);
        for ramp in &mut self.ramps {
            ramp.advance(timeline);
        }
    }

    pub(super) fn apply(&self, values: &mut std::collections::HashMap<String, SlotValue>) {
        for (name, ramp) in NAMES.into_iter().zip(&self.ramps) {
            if let Some(target) = ramp.target()
                && let Some(value) = values.get_mut(name)
            {
                *value = SlotValue::Number(target);
            }
        }
    }
}

#[cfg(test)]
mod tests;

#[cfg(test)]
#[derive(Clone, Copy, Debug)]
pub(crate) struct TimedSlotDebug {
    pub(crate) playback_tick: Option<f64>,
    pub(crate) slots: [ramp::TimedSlotDebugRamp; 4],
}

#[cfg(test)]
impl TimedSlotTargets {
    pub(super) fn is_reset(&self) -> bool {
        self.timeline.is_none() && self.ramps.iter().all(|ramp| ramp.target().is_none())
    }

    pub(super) fn debug(&self) -> TimedSlotDebug {
        TimedSlotDebug {
            playback_tick: self.timeline,
            slots: std::array::from_fn(|i| self.ramps[i].debug(self.timeline)),
        }
    }
}
