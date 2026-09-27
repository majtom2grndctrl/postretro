// Per-frame CPU accounting: open, attribute, exclude or commit a frame.
// See: context/lib/rendering_pipeline.md §12

use std::rc::Rc;
use std::time::Instant;

use postretro_stage_timing::{
    FrameRecord, StageFrame, StageSet, StageWindow, TimingGate, WindowSnapshot,
};

use super::{FrameStage, derived};

/// Where a blocking span came from. Reported as substages of `wait`, which
/// shows where the vsync block lands on a given platform.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum WaitSource {
    /// The surface texture request.
    Acquire,
    /// The present call.
    Present,
}

impl WaitSource {
    fn label(self) -> &'static str {
        match self {
            Self::Acquire => derived::WAIT_ACQUIRE,
            Self::Present => derived::WAIT_PRESENT,
        }
    }
}

/// Owns the in-level frame's stage values and every CPU window.
///
/// A frame counts only when [`commit_frame`](Self::commit_frame) runs for it:
/// frontend frames and early returns simply never commit, and the next
/// [`begin_frame`](Self::begin_frame) drops whatever they staged. With the
/// gate off every method is a no-op and no window exists.
pub(crate) struct CpuFrameTimer {
    gate: TimingGate,
    /// Shared so a scope guard can outlive `&mut App` method calls inside its
    /// stage. Cloned once per frame (a refcount bump, no allocation).
    stages: Rc<StageFrame<FrameStage>>,
    /// Stage sets from other crates, gathered during the frame.
    nested: FrameRecord,
    /// The composed record folded at commit.
    record: FrameRecord,
    window: Option<StageWindow>,
    start: Option<Instant>,
    wait_nanos: u64,
    /// Per-source wait, `None` for a source that did not run this frame.
    wait_sources: [Option<u64>; 2],
    /// Blocking time moved out of each top-level stage into wait.
    blocked: [u64; FrameStage::ALL.len()],
    excluded: bool,
    /// Set once a frame was dropped for an impossible record; see `compose`.
    warned_invalid_frame: bool,
}

impl CpuFrameTimer {
    pub(crate) fn new(gate: TimingGate) -> Self {
        Self {
            gate,
            stages: Rc::new(StageFrame::new(gate)),
            nested: FrameRecord::new(),
            record: FrameRecord::new(),
            window: gate.is_enabled().then(StageWindow::new),
            start: None,
            wait_nanos: 0,
            wait_sources: [None; 2],
            blocked: [0; FrameStage::ALL.len()],
            excluded: false,
            warned_invalid_frame: false,
        }
    }

    pub(crate) fn gate(&self) -> TimingGate {
        self.gate
    }

    /// Opens a frame at handler entry, discarding anything a previous,
    /// uncommitted frame staged.
    pub(crate) fn begin_frame(&mut self, start: Instant) {
        if !self.gate.is_enabled() {
            return;
        }
        self.stages.clear();
        self.nested.clear();
        self.wait_nanos = 0;
        self.wait_sources = [None; 2];
        self.blocked = [0; FrameStage::ALL.len()];
        self.excluded = false;
        self.start = Some(start);
    }

    /// Handle for scoping top-level stages across `&mut self` calls.
    pub(crate) fn stages(&self) -> Rc<StageFrame<FrameStage>> {
        Rc::clone(&self.stages)
    }

    /// Stage frames from other crates land here, placed by label.
    pub(crate) fn nested_mut(&mut self) -> &mut FrameRecord {
        &mut self.nested
    }

    /// Counts a blocking span measured inside `stage` (surface acquire,
    /// present) as wait, and removes it from that stage at commit.
    pub(crate) fn add_wait_within(&mut self, stage: FrameStage, source: WaitSource, nanos: u64) {
        if !self.gate.is_enabled() {
            return;
        }
        self.wait_nanos = self.wait_nanos.saturating_add(nanos);
        let by_source = &mut self.wait_sources[source as usize];
        *by_source = Some(by_source.unwrap_or(0).saturating_add(nanos));
        let blocked = &mut self.blocked[stage.index()];
        *blocked = blocked.saturating_add(nanos);
    }

    /// Keeps the current frame out of every window (no surface this frame,
    /// the level-install frame, a reload-commit frame).
    pub(crate) fn exclude_frame(&mut self) {
        self.excluded = true;
    }

    /// Drops the partial window (vsync toggle, reload commit). The last closed
    /// window stays visible.
    pub(crate) fn discard_partial(&mut self) {
        if let Some(window) = self.window.as_mut() {
            window.discard_partial();
        }
    }

    /// A level installed or unloaded: the current frame does not count, and
    /// no surface may show a window measured in the previous level.
    pub(crate) fn level_changed(&mut self) {
        self.excluded = true;
        if let Some(window) = self.window.as_mut() {
            window.clear();
        }
    }

    /// Folds the open frame into the window. Returns `true` when it closed one.
    pub(crate) fn commit_frame(&mut self, end: Instant) -> bool {
        let Some(start) = self.start.take() else {
            return false;
        };
        if self.excluded || self.window.is_none() {
            return false;
        }
        let total =
            u64::try_from(end.saturating_duration_since(start).as_nanos()).unwrap_or(u64::MAX);
        if let Err(reason) = self.compose(total) {
            // Unreachable with disjoint top-level scopes and today's label
            // count. Warn once so a future overlap cannot silently shorten
            // windows instead of surfacing.
            debug_assert!(false, "[CpuTiming] frame dropped: {reason}");
            if !self.warned_invalid_frame {
                self.warned_invalid_frame = true;
                log::warn!("[CpuTiming] frame dropped from the window: {reason}");
            }
            return false;
        }
        self.window
            .as_mut()
            .expect("window checked above")
            .fold(&self.record)
    }

    /// Commits the frame and writes the `[CpuTiming]` line when it closed a
    /// window. The redraw path's frame end.
    pub(crate) fn finish_frame(&mut self, end: Instant) -> bool {
        let closed = self.commit_frame(end);
        if closed {
            self.log_last_window();
        }
        closed
    }

    /// Builds the frame's record: derived split first, then top-level stages
    /// in frame order, then nested sets. Fails if the stage values cannot fit
    /// inside the frame total, which would mean overlapping stages.
    fn compose(&mut self, total: u64) -> Result<(), &'static str> {
        if self.nested.overflowed() {
            return Err("more stage labels than a frame record holds");
        }
        let record = &mut self.record;
        record.clear();
        for &stage in FrameStage::ALL {
            let Some(value) = self.stages.value(stage) else {
                continue;
            };
            let value = value
                .checked_sub(self.blocked[stage.index()])
                .ok_or("wait exceeds its enclosing stage")?;
            record.push(postretro_stage_timing::Sample {
                label: stage.label(),
                parent: None,
                kind: stage.kind(),
                value,
                aggregate: false,
            });
        }
        let unattributed = total
            .checked_sub(record.top_level_time())
            .and_then(|rest| rest.checked_sub(self.wait_nanos))
            .ok_or("top-level stages and wait exceed the frame total")?;

        let stages = record.clone();
        record.clear();
        record.push_aggregate_time(derived::TOTAL, total);
        record.push_aggregate_time(derived::WORK, total - self.wait_nanos);
        record.push_time(derived::WAIT, None, self.wait_nanos);
        record.push_time(derived::UNATTRIBUTED, None, unattributed);
        for source in [WaitSource::Acquire, WaitSource::Present] {
            if let Some(nanos) = self.wait_sources[source as usize] {
                record.push_time(source.label(), Some(derived::WAIT), nanos);
            }
        }
        for sample in stages.samples() {
            record.push(*sample);
        }
        for sample in self.nested.samples() {
            record.push(*sample);
        }
        debug_assert!(
            record.substage_overruns().next().is_none(),
            "a substage exceeds its parent"
        );
        Ok(())
    }

    /// The last composed frame, for tests of the frame split.
    #[cfg(test)]
    pub(crate) fn last_record(&self) -> &FrameRecord {
        &self.record
    }

    /// The latest closed window. Reading does not consume it.
    pub(crate) fn last_window(&self) -> Option<&WindowSnapshot> {
        self.window.as_ref()?.last_window()
    }

    /// Frames counted toward the window that has not closed yet.
    #[cfg(test)]
    pub(crate) fn partial_frames(&self) -> u32 {
        self.window.as_ref().map_or(0, StageWindow::partial_frames)
    }

    /// Writes the `[CpuTiming]` line for the latest closed window. Surface
    /// only: allocates the line text.
    pub(crate) fn log_last_window(&self) {
        if let Some(window) = self.last_window() {
            log::info!(
                "[CpuTiming] frames={} {}",
                window.frames,
                window.fields_string()
            );
        }
    }
}
