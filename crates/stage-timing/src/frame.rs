// One crate's per-frame stage values and the RAII scope that records into them.
// See: context/lib/rendering_pipeline.md §12

use std::cell::Cell;
use std::marker::PhantomData;
use std::time::Instant;

use crate::{MAX_STAGES_PER_SET, StageKind, StageSet, TimingGate};

/// Per-frame values for one [`StageSet`].
///
/// Interior mutability lets nested scopes over the same frame coexist: a
/// parent's guard and its substage's guard both borrow the frame shared.
/// Values are fixed storage, so recording never allocates. With the gate off
/// every operation is a no-op and the frame stays empty.
pub struct StageFrame<S: StageSet> {
    gate: TimingGate,
    values: [Cell<u64>; MAX_STAGES_PER_SET],
    ran: Cell<u32>,
    _set: PhantomData<S>,
}

impl<S: StageSet> StageFrame<S> {
    pub fn new(gate: TimingGate) -> Self {
        debug_assert!(
            S::ALL.len() <= MAX_STAGES_PER_SET,
            "stage set exceeds MAX_STAGES_PER_SET"
        );
        Self {
            gate,
            values: Default::default(),
            ran: Cell::new(0),
            _set: PhantomData,
        }
    }

    pub fn gate(&self) -> TimingGate {
        self.gate
    }

    /// Times `stage` until the guard drops. Re-entering a stage in the same
    /// frame adds to its total. Under the `tracy` feature the scope is also a
    /// Tracy zone, whatever the gate says.
    pub fn scope(&self, stage: S) -> StageScope<'_, S> {
        StageScope {
            frame: self,
            stage,
            start: self.gate.is_enabled().then(Instant::now),
            #[cfg(feature = "tracy")]
            _zone: tracy_zone(stage.label()),
        }
    }

    /// Adds a duration measured elsewhere (for example, returned by a callee
    /// that timed a call this crate cannot wrap).
    pub fn add_nanos(&self, stage: S, nanos: u64) {
        debug_assert_eq!(stage.kind(), StageKind::Time);
        self.add(stage, nanos);
    }

    /// Adds to a counter stage and marks it as having run this frame, so a
    /// zero count is present rather than absent.
    pub fn add_count(&self, stage: S, count: u64) {
        debug_assert_eq!(stage.kind(), StageKind::Count);
        self.add(stage, count);
    }

    /// Raises a marker stage for this frame.
    pub fn mark(&self, stage: S) {
        debug_assert_eq!(stage.kind(), StageKind::Marker);
        self.add(stage, 1);
    }

    /// The stage's value this frame, or `None` if it did not run.
    pub fn value(&self, stage: S) -> Option<u64> {
        let index = stage.index();
        (self.ran.get() & (1 << index) != 0).then(|| self.values[index].get())
    }

    /// Folds another frame of the same set into this one: stages that ran in
    /// either frame run here, with summed values. Used to sum per-tick frames.
    pub fn absorb(&self, other: &StageFrame<S>) {
        if !self.gate.is_enabled() {
            return;
        }
        for &stage in S::ALL {
            if let Some(value) = other.value(stage) {
                self.add(stage, value);
            }
        }
    }

    pub fn clear(&self) {
        self.ran.set(0);
        for value in &self.values {
            value.set(0);
        }
    }

    fn add(&self, stage: S, amount: u64) {
        if !self.gate.is_enabled() {
            return;
        }
        let index = stage.index();
        let value = &self.values[index];
        value.set(value.get().saturating_add(amount));
        self.ran.set(self.ran.get() | (1 << index));
    }
}

impl<S: StageSet> Default for StageFrame<S> {
    fn default() -> Self {
        Self::new(TimingGate::OFF)
    }
}

impl<S: StageSet> Clone for StageFrame<S> {
    fn clone(&self) -> Self {
        Self {
            gate: self.gate,
            values: self.values.clone(),
            ran: self.ran.clone(),
            _set: PhantomData,
        }
    }
}

impl<S: StageSet> PartialEq for StageFrame<S> {
    fn eq(&self, other: &Self) -> bool {
        self.gate == other.gate
            && S::ALL
                .iter()
                .all(|&stage| self.value(stage) == other.value(stage))
    }
}

impl<S: StageSet> std::fmt::Debug for StageFrame<S> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let mut map = f.debug_map();
        for &stage in S::ALL {
            if let Some(value) = self.value(stage) {
                map.entry(&stage.label(), &value);
            }
        }
        map.finish()
    }
}

/// Times one stage from creation to drop.
pub struct StageScope<'a, S: StageSet> {
    frame: &'a StageFrame<S>,
    stage: S,
    start: Option<Instant>,
    #[cfg(feature = "tracy")]
    _zone: Option<tracy_client::Span>,
}

/// Labels are chosen at runtime, so zones use Tracy's allocated source
/// locations rather than its static-literal macro. Allocation here is outside
/// the built-in timer's allocation contract, which assumes Tracy off.
#[cfg(feature = "tracy")]
fn tracy_zone(label: &'static str) -> Option<tracy_client::Span> {
    tracy_client::Client::running()
        .map(|client| client.span_alloc(Some(label), "", file!(), line!(), 0))
}

impl<S: StageSet> Drop for StageScope<'_, S> {
    fn drop(&mut self) {
        if let Some(start) = self.start {
            let nanos = u64::try_from(start.elapsed().as_nanos()).unwrap_or(u64::MAX);
            self.frame.add(self.stage, nanos);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_sets::Demo;

    #[test]
    fn stage_entered_twice_in_one_frame_reports_the_sum() {
        let frame = StageFrame::<Demo>::new(TimingGate::ON);
        frame.add_nanos(Demo::Outer, 300);
        frame.add_nanos(Demo::Outer, 450);
        assert_eq!(frame.value(Demo::Outer), Some(750));

        let timed = StageFrame::<Demo>::new(TimingGate::ON);
        drop(timed.scope(Demo::Inner));
        let first = timed.value(Demo::Inner).expect("first entry recorded");
        drop(timed.scope(Demo::Inner));
        assert!(timed.value(Demo::Inner).expect("second entry recorded") >= first);
    }

    #[test]
    fn gate_off_records_nothing() {
        let frame = StageFrame::<Demo>::new(TimingGate::OFF);
        drop(frame.scope(Demo::Outer));
        frame.add_count(Demo::Hits, 3);
        frame.mark(Demo::Tripped);
        assert!(Demo::ALL.iter().all(|&stage| frame.value(stage).is_none()));
    }

    #[test]
    fn zero_count_is_present_not_absent() {
        let frame = StageFrame::<Demo>::new(TimingGate::ON);
        frame.add_count(Demo::Hits, 0);
        assert_eq!(frame.value(Demo::Hits), Some(0));
        assert_eq!(frame.value(Demo::Tripped), None);
    }

    #[test]
    fn nested_scopes_keep_substage_inside_parent() {
        let frame = StageFrame::<Demo>::new(TimingGate::ON);
        {
            let _outer = frame.scope(Demo::Outer);
            let _inner = frame.scope(Demo::Inner);
            std::hint::black_box(0);
        }
        let outer = frame.value(Demo::Outer).expect("outer ran");
        let inner = frame.value(Demo::Inner).expect("inner ran");
        assert!(inner <= outer, "inner {inner} ns exceeds outer {outer} ns");
    }

    #[test]
    fn absorb_sums_stages_across_frames() {
        let total = StageFrame::<Demo>::new(TimingGate::ON);
        let tick = StageFrame::<Demo>::new(TimingGate::ON);
        tick.add_nanos(Demo::Outer, 10);
        total.absorb(&tick);
        tick.clear();
        tick.add_nanos(Demo::Outer, 5);
        tick.add_count(Demo::Hits, 2);
        total.absorb(&tick);
        assert_eq!(total.value(Demo::Outer), Some(15));
        assert_eq!(total.value(Demo::Hits), Some(2));
        assert_eq!(total.value(Demo::Inner), None);
    }
}
