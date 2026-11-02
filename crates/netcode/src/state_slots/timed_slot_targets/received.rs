// The target client's received-assist display stream: a line of known slope
// from the newest `player.assistReceivedProgress` sample.
// See: context/lib/networking.md §What gates, and what replicates instead
//
// Unlike `Ramp`, the slope is known before any two samples agree: one
// increment of the host's received required ticks per server tick, the rule
// the assist step counts by. So the first nonzero sample of an assist may step
// up from 0 and the display rises continuously from it (O17), where `Ramp`
// would show the first samples raw.

use postretro_entities::SlotValue;
use postretro_sim::sim::assist::{PROGRESS_UNIT, increment};

use crate::host::SNAPSHOT_TICK_INTERVAL;

pub(crate) const RECEIVED_NAME: &str = "player.assistReceivedProgress";

/// How far past its anchor the line extrapolates, in server ticks: two
/// snapshot intervals, so one lost snapshot keeps the bar moving and an
/// outage holds it.
const LEAD_CAP_TICKS: f64 = 2.0 * SNAPSHOT_TICK_INTERVAL as f64;

#[derive(Clone, Copy, Debug)]
struct Anchor {
    tick: f64,
    value: f64,
}

#[derive(Default, Debug)]
pub(super) struct ReceivedProgressStream {
    /// The newest sample and its server tick.
    anchor: Option<Anchor>,
    /// Progress per server tick; 0 while samples show raw.
    slope: f64,
    /// Received required ticks of 0 or 1: each sample shows raw.
    raw: bool,
    displayed: Option<f64>,
    /// The sample timeline at the last advance.
    timeline: Option<f64>,
}

impl ReceivedProgressStream {
    /// Take one committed sample with the received required ticks the same
    /// snapshot carried. The first matching row applies.
    pub(super) fn accept(&mut self, tick: u32, sample: f32, required_ticks: u32) {
        if !sample.is_finite() {
            return;
        }
        let value = f64::from(sample).clamp(0.0, 1.0);
        let previous = self.anchor.map(|anchor| anchor.value);
        self.anchor = Some(Anchor {
            tick: f64::from(tick),
            value,
        });
        // Row 1: nothing to divide by; show the sample raw.
        if required_ticks <= 1 {
            self.raw = true;
            self.slope = 0.0;
            self.displayed = Some(value);
            return;
        }
        self.raw = false;
        self.slope = increment(required_ticks) as f64 / PROGRESS_UNIT as f64;
        match (previous, self.displayed) {
            // Row 2: the stream's first sample, or an onset after a 0.
            (None, _) | (_, None) => self.displayed = Some(value),
            (Some(previous), _) if previous <= 0.0 && value > 0.0 => self.displayed = Some(value),
            // Row 3: a step down shows at once.
            (Some(previous), _) if value < previous => self.displayed = Some(value),
            // Rows 4 and 5: anchor only; the advance rises toward the line or
            // holds until it overtakes the display.
            _ => {}
        }
    }

    /// The line at timeline time `timeline`: flat at 0 for an anchor of 0,
    /// otherwise the anchor plus the slope, capped in time and at 1.
    fn line(&self, timeline: f64) -> Option<f64> {
        let anchor = self.anchor?;
        if anchor.value <= 0.0 {
            return Some(0.0);
        }
        let tick = timeline.min(anchor.tick + LEAD_CAP_TICKS);
        Some((anchor.value + self.slope * (tick - anchor.tick)).min(1.0))
    }

    /// Row 6, once per client frame on the shared sample timeline: rise
    /// toward a line above the display by at most twice the slope times the
    /// advance; hold under a line below it. With no timeline the display holds.
    pub(super) fn advance(&mut self, timeline: Option<f64>) {
        let Some(timeline) = timeline else {
            self.timeline = None;
            return;
        };
        let advance = self
            .timeline
            .map_or(0.0, |previous| (timeline - previous).max(0.0));
        self.timeline = Some(timeline);
        if self.raw {
            return;
        }
        let (Some(line), Some(displayed)) = (self.line(timeline), self.displayed) else {
            return;
        };
        if line > displayed {
            self.displayed = Some((displayed + 2.0 * self.slope * advance).min(line));
        }
    }

    pub(super) fn apply(&self, values: &mut std::collections::HashMap<String, SlotValue>) {
        if let Some(displayed) = self.displayed
            && let Some(value) = values.get_mut(RECEIVED_NAME)
        {
            *value = SlotValue::Number(displayed as f32);
        }
    }

    #[cfg(test)]
    pub(super) fn is_reset(&self) -> bool {
        self.anchor.is_none() && self.displayed.is_none() && self.timeline.is_none()
    }

    #[cfg(test)]
    pub(super) fn displayed(&self) -> Option<f64> {
        self.displayed
    }
}

#[cfg(test)]
#[path = "received_tests.rs"]
mod tests;
