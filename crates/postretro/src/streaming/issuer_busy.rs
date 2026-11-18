//! Issuer busy time: how long the read issuer thread works on each physical read.
//! See: context/lib/rendering_pipeline.md §4 (one issuer and one budget)

// Off unless a level owner installs a log (the residency walk measurement
// does); the issuer then times each planned read from its byte charge through
// the read, the split and every delivery it completes. Idle waits for
// requests are never counted. Compiled into every build so the figure means
// the same thing in any build that measures it.

use std::sync::Mutex;
use std::time::Duration;

/// Busy time per physical read, in nanoseconds, in issue order.
#[derive(Debug, Default)]
pub(crate) struct IssuerBusyLog {
    samples: Mutex<Vec<u64>>,
}

impl IssuerBusyLog {
    pub(crate) fn record(&self, busy: Duration) {
        let nanos = u64::try_from(busy.as_nanos()).unwrap_or(u64::MAX);
        // A poisoned log only loses measurement samples.
        if let Ok(mut samples) = self.samples.lock() {
            samples.push(nanos);
        }
    }

    /// Every sample so far.
    #[cfg(test)]
    pub(crate) fn samples(&self) -> Vec<u64> {
        self.samples
            .lock()
            .map(|samples| samples.clone())
            .unwrap_or_default()
    }
}
