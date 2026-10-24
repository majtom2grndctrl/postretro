// One bounded agreed-slope stream on the shared sample timeline.
// See: context/lib/networking.md §What gates, and what replicates instead

use super::{HOLD_CAP_TICKS, SLOPE_TOLERANCE};
use crate::weapon::SlotSample;

// More than half a second of one-tick stamps; ordinary transit needs only a few.
const PENDING_LIMIT: usize = 32;

#[derive(Clone, Copy, Default)]
struct Sample {
    tick: f64,
    value: f64,
}

#[derive(Clone, Copy)]
struct Line {
    sample: Sample,
    slope: f64,
}

#[derive(Default)]
pub(super) struct Ramp {
    slot: Option<usize>,
    newest: Option<Sample>,
    interval_slope: Option<f64>,
    agreeing: bool,
    bound: Option<f64>,
    upward: bool,
    target: Option<f64>,
    fed: Option<Line>,
    pending: [Option<Line>; PENDING_LIMIT],
    pending_len: usize,
    #[cfg(test)]
    samples: usize,
}

impl Ramp {
    pub(super) fn target(&self) -> Option<f32> {
        self.target.map(|v| v as f32)
    }

    pub(super) fn show_raw(&mut self) {
        self.target = self.newest.map(|s| s.value);
        self.fed = None;
        self.pending.fill(None);
        self.pending_len = 0;
    }

    pub(super) fn accept(
        &mut self,
        tick: u32,
        raw: Option<SlotSample<Option<f32>>>,
        bound: Option<f32>,
        upward: bool,
        timeline: Option<f64>,
    ) {
        let Some(raw) = raw else {
            return;
        };
        if self.slot != Some(raw.slot) || raw.value.is_none() {
            *self = Self::default();
            self.slot = Some(raw.slot);
        }
        let Some(value) = raw.value.filter(|v| v.is_finite()) else {
            return;
        };
        #[cfg(test)]
        {
            self.samples += 1;
        }
        let sample = Sample {
            tick: f64::from(tick),
            value: f64::from(value),
        };
        self.bound = bound.map(f64::from);
        self.upward = upward;
        let Some(previous) = self.newest else {
            self.newest = Some(sample);
            self.target = Some(sample.value);
            return;
        };
        if sample.tick <= previous.tick {
            return;
        }
        let slope = (sample.value - previous.value) / (sample.tick - previous.tick);
        let previous_slope = self.interval_slope;
        let agrees = previous_slope.is_some_and(|old| {
            self.in_direction(old)
                && self.in_direction(slope)
                && (slope - old).abs() <= SLOPE_TOLERANCE * slope.abs().min(old.abs())
        });
        let onset = !self.agreeing;
        let outage = timeline.is_some_and(|t| t > previous.tick + HOLD_CAP_TICKS);
        self.newest = Some(sample);
        self.interval_slope = Some(slope);
        self.agreeing = agrees;
        if !agrees || self.bound.is_none() || self.at_bound(sample.value) || timeline.is_none() {
            self.show_raw();
            return;
        }
        let timeline = timeline.expect("checked above");
        if onset || outage || self.fed.is_none() {
            self.pending.fill(None);
            self.pending_len = 0;
            // The newest sample may be ahead of playback. Keep the raw target
            // already shown as a monotonic floor while the older line catches up.
            self.fed = Some(Line {
                sample: previous,
                slope: previous_slope.expect("agreement requires two intervals"),
            });
        }
        let line = Line { sample, slope };
        if sample.tick <= timeline {
            self.pending.fill(None);
            self.pending_len = 0;
            self.fed = Some(line);
        } else {
            // Bound storage even across a wildly wrong clock estimate. Preserve
            // the earliest waiting lines and coalesce overflow into the newest.
            let index = self.pending_len.min(PENDING_LIMIT - 1);
            self.pending[index] = Some(line);
            self.pending_len = (self.pending_len + 1).min(PENDING_LIMIT);
        }
        if outage {
            self.target = None;
        }
        self.advance(timeline);
    }

    fn in_direction(&self, slope: f64) -> bool {
        if self.upward {
            slope > 0.0
        } else {
            slope < 0.0
        }
    }
    fn at_bound(&self, value: f64) -> bool {
        self.bound.is_some_and(|bound| {
            if self.upward {
                value >= bound
            } else {
                value <= bound
            }
        })
    }

    pub(super) fn advance(&mut self, timeline: f64) {
        if !self.agreeing || self.bound.is_none() {
            return;
        }
        // Before the first sync echo we showed raw samples. Establish the line
        // on that first initialized frame, whose startup step is exempt from
        // the normal frame bound, rather than jumping at the next sample.
        if self.fed.is_none() {
            self.fed = self
                .newest
                .zip(self.interval_slope)
                .map(|(sample, slope)| Line { sample, slope });
        }
        let mut consumed = 0;
        for line in self.pending[..self.pending_len].iter().flatten() {
            if line.sample.tick > timeline {
                break;
            }
            self.fed = Some(*line);
            consumed += 1;
        }
        if consumed > 0 {
            self.pending.copy_within(consumed..self.pending_len, 0);
            self.pending_len -= consumed;
            self.pending[self.pending_len..].fill(None);
        }
        let Some(line) = self.fed else {
            return;
        };
        let Some(newest) = self.newest else {
            return;
        };
        let tick = timeline.min(newest.tick + HOLD_CAP_TICKS);
        let elapsed = (tick - line.sample.tick).max(0.0);
        let projected = line.sample.value + line.slope * elapsed;
        let bound = self.bound.expect("checked above");
        let projected = if self.upward {
            projected.min(bound)
        } else {
            projected.max(bound)
        };
        let target = match self.target {
            Some(held) if self.upward => projected.max(held),
            Some(held) => projected.min(held),
            None => projected,
        };
        self.target = Some(if self.upward {
            target.min(bound)
        } else {
            target.max(bound)
        });
    }
}

#[cfg(test)]
#[derive(Clone, Copy, Debug)]
pub(crate) struct TimedSlotDebugRamp {
    pub(crate) target: Option<f64>,
    pub(crate) newest_tick: Option<f64>,
    pub(crate) newest_value: Option<f64>,
    pub(crate) host_slot: Option<usize>,
    pub(crate) agreed_slope: Option<f64>,
    pub(crate) fed_tick: Option<f64>,
    pub(crate) cap_expired: bool,
    pub(crate) samples: usize,
    pub(crate) bound: Option<f64>,
}

#[cfg(test)]
impl Ramp {
    pub(super) fn newest_tick(&self) -> Option<f64> {
        self.newest.map(|s| s.tick)
    }
    pub(super) fn debug(&self, timeline: Option<f64>) -> TimedSlotDebugRamp {
        TimedSlotDebugRamp {
            target: self.target,
            newest_tick: self.newest.map(|s| s.tick),
            newest_value: self.newest.map(|s| s.value),
            host_slot: self.slot,
            agreed_slope: self.agreeing.then_some(self.interval_slope).flatten(),
            fed_tick: self.fed.map(|line| line.sample.tick),
            cap_expired: timeline
                .zip(self.newest)
                .is_some_and(|(t, s)| t > s.tick + HOLD_CAP_TICKS),
            samples: self.samples,
            bound: self.bound,
        }
    }
}
