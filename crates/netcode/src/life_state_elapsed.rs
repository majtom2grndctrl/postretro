//! Reset evidence from accepted authoritative owner samples, before replay.
//!
//! The predicted component can move backwards after a command trim. Only an
//! actual decrease between committed owner samples witnesses an elapsed reset.

#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct OwnerLifeStateElapsedWitness {
    pub elapsed_ms: Option<f32>,
    /// Cumulative within this client's level lifetime. Later samples never hide
    /// a decrease that occurred earlier in the same rendered frame.
    pub reset_serial: u64,
}

impl OwnerLifeStateElapsedWitness {
    pub(crate) fn observe_committed(&mut self, elapsed_ms: f32) {
        debug_assert!(elapsed_ms.is_finite());
        if self
            .elapsed_ms
            .is_some_and(|previous| elapsed_ms < previous)
        {
            self.reset_serial = self.reset_serial.wrapping_add(1);
        }
        self.elapsed_ms = Some(elapsed_ms);
    }
}
