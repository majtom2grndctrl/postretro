// The target client's received-assist display stream: a line of known slope
// from the newest `player.assistReceivedProgress` sample.
// See: context/lib/networking.md §What gates, and what replicates instead
//
// Unlike `Ramp`, the slope is known before any two samples agree: one
// increment of the host's received required ticks per server tick, the rule
// the assist step counts by. So the first nonzero sample of an assist may step
// up from 0 and the display rises continuously from it, where `Ramp`
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
    /// Progress per server tick; 0 while the stream does not extrapolate.
    slope: f64,
    /// Received required ticks of 0 or 1: no line to extrapolate (see
    /// `accept_without_slope`).
    raw: bool,
    displayed: Option<f64>,
    /// The sample timeline at the last advance.
    timeline: Option<f64>,
}

impl ReceivedProgressStream {
    /// Take one committed sample with the received required ticks the same
    /// snapshot carried. The first matching row of the received-stream table
    /// applies.
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
        if required_ticks <= 1 {
            self.accept_without_slope(previous, value);
            return;
        }
        self.raw = false;
        self.slope = increment(required_ticks) as f64 / PROGRESS_UNIT as f64;
        match (previous, self.displayed) {
            // The stream's first sample, or an onset after a 0: show it at once.
            (None, _) | (_, None) => self.displayed = Some(value),
            (Some(previous), _) if previous <= 0.0 && value > 0.0 => self.displayed = Some(value),
            // A sample below its predecessor shows at once.
            (Some(previous), _) if value < previous => self.displayed = Some(value),
            // At or above its predecessor: anchor only; the advance rises
            // toward the line, or holds until it overtakes the display.
            _ => {}
        }
    }

    /// Received required ticks of 0 or 1 give no usable slope: 0 means the
    /// helper is not advancing, and 1 fills the bar in a single server tick,
    /// so twice the slope bounds no rise worth extrapolating. The display
    /// never extrapolates, and it still never steps down without a lower
    /// sample: an onset or a sample below its predecessor shows at once; any
    /// other sample holds the display, or lifts it at once to a higher
    /// sample, since a rise with no rate to pace it would otherwise leave the
    /// bar below a host value it has received.
    fn accept_without_slope(&mut self, previous: Option<f64>, value: f64) {
        self.raw = true;
        self.slope = 0.0;
        let shown = match (previous, self.displayed) {
            (None, _) | (_, None) => value,
            (Some(previous), _) if previous <= 0.0 && value > 0.0 => value,
            (Some(previous), _) if value < previous => value,
            (_, Some(displayed)) => displayed.max(value),
        };
        self.displayed = Some(shown);
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

    /// The timeline advance, once per client frame on the shared sample
    /// timeline: rise
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
