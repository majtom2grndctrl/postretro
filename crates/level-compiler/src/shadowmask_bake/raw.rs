//! Per-bake raw-fill accounting follows buffers across consumer workers.
#[cfg(test)]
use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};

#[derive(Clone, Default)]
pub(crate) struct RawFillProbe {
    #[cfg(test)]
    state: Arc<RawFillState>,
}

#[cfg(test)]
#[derive(Default)]
struct RawFillState {
    live: AtomicUsize,
    peak: AtomicUsize,
    allocations: AtomicUsize,
    closed_layers: AtomicUsize,
}

impl RawFillProbe {
    #[cfg(test)]
    pub(crate) fn live(&self) -> usize {
        self.state.live.load(Ordering::SeqCst)
    }
    #[cfg(test)]
    pub(crate) fn peak(&self) -> usize {
        self.state.peak.load(Ordering::SeqCst)
    }
    #[cfg(test)]
    pub(crate) fn closed_layers(&self) -> usize {
        self.state.closed_layers.load(Ordering::SeqCst)
    }
    #[cfg(test)]
    pub(super) fn close_layer(&self) {
        self.state.closed_layers.fetch_add(1, Ordering::SeqCst);
    }
    #[cfg(test)]
    fn allocate(&self, bytes: usize) {
        let live = self.state.live.fetch_add(bytes, Ordering::SeqCst) + bytes;
        self.state.peak.fetch_max(live, Ordering::SeqCst);
        self.state.allocations.fetch_add(1, Ordering::SeqCst);
    }
    #[cfg(test)]
    fn release(&self, bytes: usize) {
        self.state.live.fetch_sub(bytes, Ordering::SeqCst);
    }
}

/// The allocation and its accounting owner travel together; a buffer can be
/// allocated on one worker and released on another without a global counter.
pub(super) struct RawFillBuffer {
    bytes: Vec<u8>,
    #[cfg(test)]
    probe: RawFillProbe,
}

impl RawFillBuffer {
    pub(super) fn new(bytes: Vec<u8>, _probe: &RawFillProbe) -> Self {
        #[cfg(test)]
        _probe.allocate(bytes.len());
        Self {
            bytes,
            #[cfg(test)]
            probe: _probe.clone(),
        }
    }
}

impl std::ops::Deref for RawFillBuffer {
    type Target = [u8];
    fn deref(&self) -> &[u8] {
        &self.bytes
    }
}
impl std::ops::DerefMut for RawFillBuffer {
    fn deref_mut(&mut self) -> &mut [u8] {
        &mut self.bytes
    }
}
#[cfg(test)]
impl Drop for RawFillBuffer {
    fn drop(&mut self) {
        self.probe.release(self.bytes.len());
    }
}
