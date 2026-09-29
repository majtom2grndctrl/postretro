//! Lightmap residency controller: block targets, read requests, completions.
//! See: context/lib/rendering_pipeline.md §4 (Lightmap cell-block residency)

use std::collections::BTreeMap;
use std::sync::Arc;

use postretro_level_format::cell_residency_set::CellResidencySetSection;
use postretro_level_loader::{
    LightmapBlockClass, LightmapPoolReport, LightmapTarget, PreparedLightmapBlock,
};

use super::LightmapResidencyError;
use super::block_map::LevelBlockMap;
use super::demand::{BlockDemand, BlockTarget, DemandFrame};
use super::levers::LightmapLevers;
use super::source::LightmapBlockSource;
use crate::sh_streaming::generation::{GenerationClock, ProcessGenerationClock};
use crate::streaming::cluster_hints::ClusterHints;
use crate::streaming::drain_budget::MAX_INSTALL_DECODED_BYTES_PER_DRAIN;
use crate::streaming::request::StreamResource;
use crate::streaming::target_bitset::TargetBitset;

#[path = "drain.rs"]
mod drain;
#[path = "preload.rs"]
mod preload;
#[path = "reads.rs"]
mod reads;

pub(crate) use preload::LightmapPreloadReads;
#[cfg(test)]
#[path = "preload_tests.rs"]
mod preload_tests;
#[cfg(test)]
#[path = "controller_tests.rs"]
mod tests;
#[cfg(test)]
#[path = "controller_threaded_tests.rs"]
mod threaded_tests;

/// A permit covers one block pair from request until install, refusal, or
/// discard. Sizes the issuer queue and the completion queue.
pub(crate) const MAX_LIGHTMAP_PERMITS: usize = 32;
/// Pair bytes in hand (in flight, ready, or in a drain) before new requests
/// wait: four drains' worth. The first request is always allowed, so one
/// oversized pair still streams.
pub(crate) const MAX_IN_HAND_PAIR_BYTES: u64 = 4 * MAX_INSTALL_DECODED_BYTES_PER_DRAIN;
/// Failures after which a block is never requested again this generation:
/// the first, then one retry after it leaves demand and returns (SH's
/// failed-request policy), so corrupt data is not re-read every frame.
const MAX_BLOCK_FAILURES: u8 = 2;

/// Where a block is between demand and the renderer's pool.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum BlockPhase {
    Absent,
    /// Submitted to the issuer; one completion will arrive.
    InFlight,
    /// Both halves read, waiting for the shared drain budget.
    Ready,
    /// Handed to the renderer in the outstanding batch.
    InDrain,
    /// The renderer installed it; sampleable until evicted.
    Installed,
    /// A band pair the renderer refused over the cap. Not requested again
    /// until the pool reports more band headroom than it had at refusal.
    Refused,
    /// The read, the payload split, or the renderer's install failed. Not
    /// retried until the block leaves demand and returns, and then only once.
    Failed,
}

#[derive(Debug, Clone, Copy)]
struct BlockSlot {
    phase: BlockPhase,
    target: Option<BlockTarget>,
    /// The target the renderer last accepted.
    sent: Option<LightmapTarget>,
    /// Queued in `pending_delta`.
    pending_delta: bool,
    /// Band texels charged against headroom while the pair is in hand.
    band_texels: u64,
    /// Failures this generation; the first warns.
    failures: u8,
    refused_headroom: u64,
}

/// Cumulative since controller creation, except the two `last_frame_*`
/// gauges.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct LightmapResidencyCounters {
    pub(crate) baked_recomputes: u64,
    pub(crate) residency_lookups: u64,
    /// `target_set` plus `target_remove` entries emitted (reset excluded).
    pub(crate) target_deltas: u64,
    pub(crate) drains: u64,
    pub(crate) reads_requested: u64,
    pub(crate) installs: u64,
    pub(crate) refusals: u64,
    /// Pairs the renderer handed back unplaced: counted transient misses.
    pub(crate) deferrals: u64,
    pub(crate) evictions: u64,
    pub(crate) cancelled_reads: u64,
    pub(crate) failed_reads: u64,
    /// Pairs the renderer could not install (payload did not fill its
    /// block): a contract violation, handled as a failed read.
    pub(crate) failed_installs: u64,
    /// Reads that completed after their block left demand; dropped.
    pub(crate) departed_reads: u64,
    /// Completions from another generation or content; dropped.
    pub(crate) stale_completions: u64,
    /// Completions for a block with no read in flight; dropped.
    pub(crate) duplicate_completions: u64,
    /// Block-frames drawn on a portal walk while not mandatory at lead L
    /// (demanded visible): "drawn but outside the baked set".
    pub(crate) drawn_outside_baked_set: u64,
    /// Block-frames drawn on a portal walk while not installed.
    pub(crate) drawn_not_resident: u64,
    pub(crate) last_frame_drawn_outside_baked_set: u32,
    pub(crate) last_frame_drawn_not_resident: u32,
}

/// Session-local lightmap block policy. It owns demand, reads, and the
/// shared-budget offer; the renderer owns placement and reports what
/// happened. No renderer or wgpu types: the renderer sees only the loader's
/// `LightmapDrainBatch` and returns a `LightmapDrainOutcome`.
pub(crate) struct LightmapResidencyController {
    source: Arc<dyn LightmapBlockSource>,
    map: LevelBlockMap,
    demand: BlockDemand,
    levers: LightmapLevers,
    /// Published to the issuer's lightmap route for pre-read cancellation.
    targets: Arc<TargetBitset>,
    generation: u64,
    content_tag: [u8; 32],
    slots: Vec<BlockSlot>,
    ready: BTreeMap<u32, PreparedLightmapBlock>,
    in_drain: Vec<u32>,
    pending_delta: Vec<u32>,
    /// Targeted blocks in request order; rebuilt only when targets change.
    request_order: Vec<u32>,
    request_order_stale: bool,
    /// Something may now be requestable: a target, permit, or headroom change.
    requests_due: bool,
    /// This frame's visibility path allows new reads.
    may_request: bool,
    needs_target_reset: bool,
    drain_outstanding: bool,
    permits_in_use: usize,
    in_hand_bytes: u64,
    band_committed_texels: u64,
    pool: LightmapPoolReport,
    refused_blocks: usize,
    counters: LightmapResidencyCounters,
    /// Reused by outcome validation.
    outcome_scratch: Vec<u32>,
}

impl std::fmt::Debug for LightmapResidencyController {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LightmapResidencyController")
            .field("generation", &self.generation)
            .field("blocks", &self.slots.len())
            .field("permits_in_use", &self.permits_in_use)
            .finish_non_exhaustive()
    }
}

impl LightmapResidencyController {
    /// `residency_set` and `cluster_hints` come from the same level as
    /// `source`. The set is read here for its shape and maximum lead, and
    /// borrowed again each frame through [`DemandFrame`].
    pub(crate) fn new(
        source: Arc<dyn LightmapBlockSource>,
        residency_set: &CellResidencySetSection,
        cluster_hints: Option<&ClusterHints>,
    ) -> Result<Self, LightmapResidencyError> {
        Self::with_clock(
            source,
            residency_set,
            cluster_hints,
            &ProcessGenerationClock,
        )
    }

    pub(crate) fn with_clock(
        source: Arc<dyn LightmapBlockSource>,
        residency_set: &CellResidencySetSection,
        cluster_hints: Option<&ClusterHints>,
        clock: &impl GenerationClock,
    ) -> Result<Self, LightmapResidencyError> {
        let generation = clock
            .take_generation()
            .filter(|&generation| generation != 0)
            .ok_or(LightmapResidencyError::GenerationExhausted)?;
        let map = LevelBlockMap::build(
            source.as_ref(),
            residency_set.camera_cell_count(),
            cluster_hints,
        )?;
        let block_count = source.block_count();
        let demand = BlockDemand::new(&map);
        Ok(Self {
            content_tag: source.content_tag(),
            source,
            demand,
            levers: LightmapLevers::new(residency_set.max_lead),
            targets: Arc::new(TargetBitset::new(block_count)),
            generation,
            slots: vec![
                BlockSlot {
                    phase: BlockPhase::Absent,
                    target: None,
                    sent: None,
                    pending_delta: false,
                    band_texels: 0,
                    refused_headroom: 0,
                    failures: 0,
                };
                map.block_count()
            ],
            map,
            ready: BTreeMap::new(),
            in_drain: Vec::new(),
            pending_delta: Vec::new(),
            request_order: Vec::new(),
            request_order_stale: true,
            requests_due: true,
            may_request: false,
            needs_target_reset: true,
            drain_outstanding: false,
            permits_in_use: 0,
            in_hand_bytes: 0,
            band_committed_texels: 0,
            pool: LightmapPoolReport::default(),
            refused_blocks: 0,
            counters: LightmapResidencyCounters::default(),
            outcome_scratch: Vec::new(),
        })
    }

    #[cfg(test)]
    pub(crate) fn generation(&self) -> u64 {
        self.generation
    }

    /// The block target set the issuer's lightmap route checks.
    pub(crate) fn target_bitset(&self) -> &Arc<TargetBitset> {
        &self.targets
    }

    #[allow(
        dead_code,
        reason = "Task 11 shows the lever values in the Streaming tab"
    )]
    pub(crate) fn levers(&self) -> LightmapLevers {
        self.levers
    }

    /// The dev-tools sliders write here (Task 11). A lead change takes effect
    /// at the next [`Self::update`]; the cap rides the next drain batch.
    #[cfg_attr(
        not(test),
        allow(dead_code, reason = "Task 11 wires the pool-cap and lead sliders")
    )]
    pub(crate) fn levers_mut(&mut self) -> &mut LightmapLevers {
        &mut self.levers
    }

    #[cfg_attr(
        not(test),
        allow(dead_code, reason = "Task 11 logs and captures the residency counters")
    )]
    pub(crate) fn counters(&self) -> LightmapResidencyCounters {
        let demand = self.demand.counters();
        LightmapResidencyCounters {
            baked_recomputes: demand.baked_recomputes,
            residency_lookups: demand.residency_lookups,
            ..self.counters
        }
    }

    /// The pool shape from the renderer's latest outcome.
    #[allow(dead_code, reason = "Task 11 reports pool layers and headroom")]
    pub(crate) fn pool_report(&self) -> LightmapPoolReport {
        self.pool
    }

    #[cfg(test)]
    pub(crate) fn phase(&self, block: u32) -> BlockPhase {
        self.slots[block as usize].phase
    }

    #[cfg(test)]
    pub(crate) fn target(&self, block: u32) -> Option<LightmapTarget> {
        self.slots[block as usize]
            .target
            .map(|target| target.wire(block))
    }

    #[cfg(test)]
    pub(crate) fn permits_in_use(&self) -> usize {
        self.permits_in_use
    }

    #[cfg(test)]
    pub(crate) fn in_hand_bytes(&self) -> u64 {
        self.in_hand_bytes
    }

    /// Capacities of every per-frame buffer, for the steady-frame proof.
    #[cfg(test)]
    pub(crate) fn buffer_capacities(&self) -> Vec<usize> {
        let mut capacities = self.demand.buffer_capacities().to_vec();
        capacities.extend([
            self.in_drain.capacity(),
            self.pending_delta.capacity(),
            self.request_order.capacity(),
            self.outcome_scratch.capacity(),
        ]);
        capacities
    }

    /// Applies one frame's visibility: recomputes baked demand only when the
    /// camera cell or lead changed, updates visible demand on a portal walk,
    /// retargets every changed block, and counts visible misses.
    pub(crate) fn update(&mut self, frame: DemandFrame<'_>) {
        self.may_request = self.demand.update(&self.map, self.levers.lead(), frame);
        self.retarget_dirty();
        if frame.is_portal_walk() {
            self.count_visible_misses();
        }
    }

    /// Demand from `camera_cell`'s baked set within lead L plus the pins, with
    /// no drawn cells: the spawn camera cell at level install, before any
    /// frame has walked its portals.
    pub(crate) fn update_camera_set(
        &mut self,
        residency_set: &CellResidencySetSection,
        camera_cell: u32,
    ) {
        self.demand
            .update_camera_set(&self.map, self.levers.lead(), residency_set, camera_cell);
        self.may_request = true;
        self.retarget_dirty();
    }

    /// Whether the camera cell's mandatory set (every block within lead L of
    /// it, plus the pinned blocks) is installed, as of the latest demand
    /// update. Visible and band blocks do not count. A mandatory block whose
    /// pair failed stays unsettled: it cannot become resident this
    /// generation. This is the lightmap answer a settle chokepoint asks.
    pub(crate) fn settled(&self) -> bool {
        self.demand.demanded_blocks(&self.map).all(|block| {
            let slot = &self.slots[block as usize];
            !slot
                .target
                .is_some_and(|target| target.class == LightmapBlockClass::Mandatory)
                || slot.phase == BlockPhase::Installed
        })
    }

    fn retarget_dirty(&mut self) {
        for index in 0..self.demand.dirty_len() {
            let block = self.demand.dirty_at(index);
            let next = self.demand.target(&self.map, block);
            self.retarget(block, next);
        }
        self.demand.clear_dirty();
    }

    fn count_visible_misses(&mut self) {
        let (mut outside, mut not_resident) = (0u32, 0u32);
        for &block in self.demand.drawn_blocks() {
            let slot = &self.slots[block as usize];
            if slot
                .target
                .is_some_and(|target| target.class == LightmapBlockClass::Visible)
            {
                outside += 1;
            }
            if slot.phase != BlockPhase::Installed {
                not_resident += 1;
            }
        }
        let counters = &mut self.counters;
        counters.drawn_outside_baked_set += u64::from(outside);
        counters.drawn_not_resident += u64::from(not_resident);
        counters.last_frame_drawn_outside_baked_set = outside;
        counters.last_frame_drawn_not_resident = not_resident;
    }

    fn retarget(&mut self, block: u32, next: Option<BlockTarget>) {
        let index = block as usize;
        let previous = self.slots[index].target;
        if previous == next {
            return;
        }
        self.slots[index].target = next;
        if previous.is_some() != next.is_some() {
            self.targets.set(block, next.is_some());
        }
        match (next, self.slots[index].phase) {
            (None, BlockPhase::Refused) => {
                self.refused_blocks -= 1;
                self.slots[index].phase = BlockPhase::Absent;
            }
            (None, BlockPhase::Failed) if self.slots[index].failures < MAX_BLOCK_FAILURES => {
                self.slots[index].phase = BlockPhase::Absent;
            }
            (None, BlockPhase::Ready) => {
                // Left demand before its drain: release its buffers now.
                self.ready.remove(&block);
                self.release_in_hand(block);
                self.slots[index].phase = BlockPhase::Absent;
                self.counters.departed_reads += 1;
            }
            (Some(target), BlockPhase::Refused) if target.class != LightmapBlockClass::Band => {
                self.refused_blocks -= 1;
                self.slots[index].phase = BlockPhase::Absent;
            }
            // In flight: the issuer cancels it, or admission discards it.
            // In a drain or installed: the renderer frees it at the drain
            // that carries the removal.
            _ => {}
        }
        if !self.slots[index].pending_delta {
            self.slots[index].pending_delta = true;
            self.pending_delta.push(block);
        }
        self.request_order_stale = true;
        self.requests_due = true;
    }
}
