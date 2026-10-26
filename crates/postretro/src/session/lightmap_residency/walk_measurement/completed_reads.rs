//! Completed-reads mode: every lightmap read in flight lands before a drain.
//! See: context/lib/rendering_pipeline.md §4

// The gate sits between the issuer and the session's completion queue. The
// issuer delivers to the queue the gate holds; the gate forwards in delivery
// order to the queue the session admits from, so admission runs exactly as
// in play, only never racing the disk.

use std::sync::mpsc::{Receiver, Sender, channel};
use std::time::Duration;

use super::super::LightmapStreamingSession;
use crate::lightmap_streaming::controller::BlockPhase;
use crate::lightmap_streaming::route::LightmapCompletion;

/// A read this slow means the issuer stalled; fail rather than hang.
const READ_TIMEOUT: Duration = Duration::from_secs(120);

pub(super) struct CompletionGate {
    upstream: Receiver<LightmapCompletion>,
    forward: Sender<LightmapCompletion>,
    /// Blocks whose completion has not yet arrived; reused.
    owed: Vec<u32>,
}

impl CompletionGate {
    /// Interposes on `session`'s completion queue. Call before the issuer
    /// spawns or at any time after: the route keeps sending to the original
    /// queue either way.
    pub(super) fn install(session: &mut LightmapStreamingSession) -> Self {
        let (forward, receiver) = channel();
        let upstream = std::mem::replace(&mut session.completions, receiver);
        Self {
            upstream,
            forward,
            owed: Vec::new(),
        }
    }

    /// Blocks until every read `session` has in flight is delivered, and
    /// forwards each for the next drain to admit. Call right before each
    /// drain: that drain admits everything forwarded, so the next call
    /// starts from a clean queue. A tier raise's rare second completion for
    /// one read owes nothing; it is forwarded whenever it arrives and the
    /// controller drops it as a duplicate.
    pub(super) fn await_in_flight(&mut self, session: &LightmapStreamingSession) {
        let controller = &session.controller;
        self.owed.clear();
        self.owed.extend(
            (0..controller.block_count() as u32)
                .filter(|&block| controller.phase(block) == BlockPhase::InFlight),
        );
        while !self.owed.is_empty() {
            let completion = self
                .upstream
                .recv_timeout(READ_TIMEOUT)
                .expect("every read in flight completes");
            let key = completion.request.key;
            if let Ok(at) = self.owed.binary_search(&key) {
                self.owed.remove(at);
            }
            self.forward
                .send(completion)
                .expect("the session holds the forward queue");
        }
    }
}
