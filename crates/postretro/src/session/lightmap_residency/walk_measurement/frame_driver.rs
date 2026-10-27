//! Owns one measured frame of streamed lightmap residency: drains, the pool
//! mirror, and why each drawn block is missing.
//! See: context/lib/rendering_pipeline.md §4

use std::ops::Range;
use std::time::Instant;

use postretro_level_format::cell_residency_set::CellResidencySetSection;
use postretro_stage_timing::StageFrame;
use postretro_visibility::{TimingGate, VisibilityPath, VisibleCells};

use super::super::LightmapStreamingSession;
use super::completed_reads::CompletionGate;
use super::modes::ReadsMode;
use super::pool_mirror::{PoolMirror, RetireClock};
use crate::cpu_timing::StreamingStage;
use crate::lightmap_streaming::controller::LightmapResidencyCounters;
use crate::session::level_streaming::{LevelStreaming, StreamingFrame};
use crate::session::sh_residency::ShStreamingSession;

/// Drains per frame in the completed-reads mode: drain 0 admits earlier reads
/// and issues this frame's, and drain 1 admits the ones drain 0 issued (O17).
/// Reads held back by the permit cap or the in-hand byte cap are issued in
/// drain 1 and land next frame, or are dropped if their block left demand.
/// More drains would hide the per-drain install budget's own misses, which
/// both builds share.
pub(super) const COMPLETED_READS_DRAINS: u32 = 2;

impl ReadsMode {
    pub(super) fn retire_clock(self) -> RetireClock {
        match self {
            Self::RealTime => RetireClock::Drains,
            Self::CompletedReads => RetireClock::Frames,
        }
    }

    fn drains_per_frame(self) -> u32 {
        match self {
            Self::RealTime => 1,
            Self::CompletedReads => COMPLETED_READS_DRAINS,
        }
    }
}

/// One frame's visibility, as the harness hands it to streaming.
#[derive(Debug, Clone, Copy)]
pub(super) struct FrameInput<'a> {
    /// Every cell the frame draws: the main view's, plus the planar mirror's
    /// reach in planar mode, as the play frame streams them.
    pub(super) visible_cells: &'a VisibleCells,
    /// The cells SH visible targeting reads (the play frame's planar gate).
    /// SH does not stream here; carried so the frame matches play's.
    pub(super) sh_visible_cells: &'a VisibleCells,
    pub(super) camera_cell: u32,
    pub(super) path: VisibilityPath,
    pub(super) seconds: f64,
}

/// What one frame did, counted after its last drain.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub(super) struct FrameOutcome {
    /// The controller's gauges: drawn outside the baked set, and drawn not
    /// installed (on a non-portal frame, of the drawn blocks it holds).
    pub(super) outside: u32,
    pub(super) not_resident: u32,
    /// Drawn blocks, every one the frame's cells draw on any path, still not
    /// resident whose latest drain this frame carried and deferred them:
    /// deferred behind a retiring generation (O29).
    pub(super) deferred: u32,
    /// Every other drawn block still not resident, demanded or not.
    pub(super) left_behind: u32,
    /// A pool growth began this frame.
    pub(super) grew: bool,
    /// A grown-out generation was still retiring as the frame began.
    pub(super) retiring_at_start: bool,
    pub(super) drains: u32,
    /// Delivered read bytes not yet admitted, before the first drain.
    pub(super) delivered: u64,
    pub(super) prepare_nanos: u64,
    pub(super) apply_nanos: u64,
    pub(super) finish_nanos: u64,
    /// The `lightmap_residency` CPU stage, every drain included.
    pub(super) residency_nanos: u64,
}

/// The level-scope streaming owner, the lightmap session inside it, and the
/// pool mirror standing in for the renderer's drain.
pub(super) struct FrameDriver {
    level: LevelStreaming,
    mirror: PoolMirror,
    reads: ReadsMode,
    gate: Option<CompletionGate>,
    /// Each cell's blocks: what a drawn cell draws.
    cell_blocks: Vec<Range<u32>>,
    no_sh: Option<ShStreamingSession>,
}

impl FrameDriver {
    /// `level` holds its cell-demand stage and lightmap session; `mirror`'s
    /// clock should be `reads.retire_clock()`.
    pub(super) fn new(
        mut level: LevelStreaming,
        mirror: PoolMirror,
        reads: ReadsMode,
        cell_blocks: Vec<Range<u32>>,
    ) -> Self {
        let gate = (reads == ReadsMode::CompletedReads)
            .then(|| CompletionGate::install(level.lightmap_mut().expect("installed")));
        Self {
            level,
            mirror,
            reads,
            gate,
            cell_blocks,
            no_sh: None,
        }
    }

    pub(super) fn session(&self) -> &LightmapStreamingSession {
        self.level.lightmap().expect("installed")
    }

    pub(super) fn mirror(&self) -> &PoolMirror {
        &self.mirror
    }

    /// The session and the mirror together, for the settled-entry preload.
    pub(super) fn session_and_mirror(
        &mut self,
    ) -> (&mut LightmapStreamingSession, &mut PoolMirror) {
        (
            self.level.lightmap_mut().expect("installed"),
            &mut self.mirror,
        )
    }

    pub(super) fn frame(
        &mut self,
        input: FrameInput<'_>,
        residency_set: &CellResidencySetSection,
    ) -> FrameOutcome {
        self.mirror.begin_frame();
        let mut out = FrameOutcome {
            retiring_at_start: self.mirror.retiring(),
            ..FrameOutcome::default()
        };
        let growths = self.mirror.stats.growths;
        let cpu = StageFrame::<StreamingStage>::new(TimingGate::ON);
        for drain in 0..self.reads.drains_per_frame() {
            if let Some(gate) = &mut self.gate {
                gate.await_in_flight(self.level.lightmap().expect("installed"));
            }
            if drain == 0 {
                out.delivered = self.session().ledger().in_memory_bytes();
            }
            let part = Instant::now();
            self.level
                .prepare_drains(
                    &mut self.no_sh,
                    Some(residency_set),
                    StreamingFrame {
                        visible_cells: input.visible_cells,
                        sh_visible_cells: input.sh_visible_cells,
                        camera_cell: Some(input.camera_cell as usize),
                        path: input.path,
                        monotonic_seconds: input.seconds,
                        settling: false,
                        cpu: &cpu,
                    },
                )
                .expect("prepare streaming drains");
            out.prepare_nanos += elapsed_nanos(part);
            let session = self.level.lightmap_mut().expect("installed");
            if let Some(batch) = session.take_drain_batch_for_renderer() {
                let outcome = {
                    let _scope = cpu.scope(StreamingStage::LightmapDrain);
                    self.mirror.drain(batch)
                };
                let part = Instant::now();
                let _scope = cpu.scope(StreamingStage::LightmapResidency);
                session.apply_outcome(outcome).expect("apply outcome");
                out.apply_nanos += elapsed_nanos(part);
            }
            out.drains += 1;
        }
        let session = self.level.lightmap_mut().expect("installed");
        let part = Instant::now();
        {
            let _scope = cpu.scope(StreamingStage::LightmapResidency);
            session.finish_frame(None, input.seconds);
        }
        out.finish_nanos = elapsed_nanos(part);
        let counters = session.controller.counters();
        out.outside = counters.last_frame_drawn_outside_baked_set;
        out.not_resident = counters.last_frame_drawn_not_resident;
        out.residency_nanos = cpu.value(StreamingStage::LightmapResidency).unwrap_or(0);
        out.grew = self.mirror.stats.growths > growths;
        let mirror = &self.mirror;
        let mut tally = |blocks: Range<u32>| {
            for block in blocks.filter(|&block| !mirror.model().is_resident(block)) {
                if mirror.deferred_this_frame(block) {
                    out.deferred += 1;
                } else {
                    out.left_behind += 1;
                }
            }
        };
        match input.visible_cells {
            VisibleCells::Culled(cells) => {
                for &cell in cells {
                    tally(self.cell_blocks[cell as usize].clone());
                }
            }
            VisibleCells::DrawAll => tally(0..mirror.model().block_count()),
        }
        out
    }

    pub(super) fn permits_in_use(&self) -> usize {
        self.session().controller.permits_in_use()
    }

    /// Retires the level's I/O and returns the session's final counters and
    /// physical reads, and the mirror.
    pub(super) fn finish(mut self) -> (LightmapResidencyCounters, u64, PoolMirror) {
        let session = self.session();
        let counters = session.controller.counters();
        let physical_reads = session.ledger().physical_reads();
        // The issuer's deliveries fail once the gate's queue is gone, and
        // release their bytes; nothing waits on them.
        self.gate = None;
        self.level.retire(&mut self.no_sh);
        drop(self.level);
        (counters, physical_reads, self.mirror)
    }
}

fn elapsed_nanos(since: Instant) -> u64 {
    u64::try_from(since.elapsed().as_nanos()).unwrap_or(u64::MAX)
}
