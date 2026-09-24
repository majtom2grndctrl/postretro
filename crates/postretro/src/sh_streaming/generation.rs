//! Process-wide, checked residency-generation allocation.
//! See: context/plans/in-progress/sh-probe-streaming--cluster-residency/index.md

use std::sync::atomic::{AtomicU64, Ordering};

/// Allocates one nonzero generation for a streaming session. `None` means the
/// process has exhausted its checked identity space; callers must reject the
/// next session rather than reuse or wrap an identity.
pub(crate) trait GenerationClock {
    fn take_generation(&self) -> Option<u64>;
}

/// The production clock is process-global so a reload cannot accidentally
/// reuse a completed worker's identity.
#[derive(Debug, Default)]
pub(crate) struct ProcessGenerationClock;

static NEXT_RESIDENCY_GENERATION: AtomicU64 = AtomicU64::new(1);

impl GenerationClock for ProcessGenerationClock {
    fn take_generation(&self) -> Option<u64> {
        NEXT_RESIDENCY_GENERATION
            .fetch_update(Ordering::SeqCst, Ordering::SeqCst, |current| {
                (current != 0).then(|| current.checked_add(1).unwrap_or(0))
            })
            .ok()
    }
}

#[cfg(test)]
pub(super) struct FixedGenerationClock {
    next: std::cell::Cell<u64>,
}

#[cfg(test)]
impl FixedGenerationClock {
    pub(super) const fn new(next: u64) -> Self {
        Self {
            next: std::cell::Cell::new(next),
        }
    }
}

#[cfg(test)]
impl GenerationClock for FixedGenerationClock {
    fn take_generation(&self) -> Option<u64> {
        let generation = self.next.get();
        (generation != 0).then(|| {
            self.next.set(generation.checked_add(1).unwrap_or(0));
            generation
        })
    }
}
