//! Lightmap residency controller: block targets, read requests, completions.
//! See: context/lib/rendering_pipeline.md §4 (Lightmap cell-block residency)

use std::collections::BTreeMap;
use std::sync::Arc;

use postretro_level_format::cell_residency_set::CellResidencySetSection;
use postretro_level_loader::{
    LightmapBlockClass, LightmapPoolReport, LightmapTarget, PreparedLightmapBlock,
};

use super::LightmapResidencyError;
use super::block_map::{BlockFacts, LevelBlockMap};
use super::demand::{BlockDemand, BlockTarget, DemandFrame};
use super::levers::LightmapLevers;
use super::source::LightmapBlockSource;
use crate::sh_streaming::generation::{GenerationClock, ProcessGenerationClock};
use crate::streaming::cluster_hints::ClusterHints;
use crate::streaming::drain_budget::MAX_INSTALL_DECODED_BYTES_PER_DRAIN;
use crate::streaming::request::{ReadTier, StreamResource};
use crate::streaming::target_bitset::TargetBitset;

#[path = "drain.rs"]
mod drain;
#[path = "preload.rs"]
mod preload;
#[path = "reads.rs"]
mod reads;

pub(crate) use preload::LightmapPreloadReads;
use reads::PairCharge;
#[cfg(test)]
#[path = "multi_block_tests.rs"]
mod multi_block_tests;
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
/// discard.
pub(crate) const MAX_LIGHTMAP_PERMITS: usize = 32;
/// Issuer queue and completion queue slots for lightmap requests: a read per
/// permit, and as many again for tier raises. A raise the issuer accepts
/// while its pair is still pending joins that read. It is read, and
/// completed, a second time only when its pair finished in the one physical
/// read in progress as the raise was accepted. That duplicate reads at the
/// mandatory tier, before every optional original, so duplicates never wait
/// behind the band reads that could crowd them; and raises come only from
/// band reads in flight, at most [`MAX_BAND_PERMITS`] at once.
pub(crate) const LIGHTMAP_QUEUE_CAPACITY: usize = 2 * MAX_LIGHTMAP_PERMITS;
/// Pair bytes in hand (in flight, ready, or in a drain) before new requests
/// wait: four drains' worth. The first request is always allowed, so one
/// oversized pair still streams.
pub(crate) const MAX_IN_HAND_PAIR_BYTES: u64 = 4 * MAX_INSTALL_DECODED_BYTES_PER_DRAIN;
/// Band (optional) reads may hold at most half the permits and half the
/// in-hand bytes. The other half is a reserve only mandatory and visible
/// reads use, so prefetch can never keep them from submitting. The first
/// band pair is always allowed, so one oversized band pair still streams.
pub(crate) const MAX_BAND_PERMITS: usize = MAX_LIGHTMAP_PERMITS / 2;
pub(crate) const MAX_BAND_IN_HAND_PAIR_BYTES: u64 = MAX_IN_HAND_PAIR_BYTES / 2;
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
    /// until the pool reports at least one more slot of band headroom than
    /// it had at refusal.
    Refused,
    /// The read, the payload split, or the renderer's install failed. Not
    /// retried until the block leaves demand and returns, and then only once.
    Failed,
}

impl BlockPhase {
    /// Whether the pair is resident or on its way: read, being read, or in
    /// the renderer's hands. Only then is a held block raised to at least
    /// visible.
    fn holds_pair(self) -> bool {
        matches!(
            self,
            Self::InFlight | Self::Ready | Self::InDrain | Self::Installed
        )
    }
}

#[derive(Debug, Clone, Copy)]
struct BlockSlot {
    phase: BlockPhase,
    target: Option<BlockTarget>,
    /// The target the renderer last accepted.
    sent: Option<LightmapTarget>,
    /// Queued in `pending_delta`.
    pending_delta: bool,
    /// What the pair is charged against while it is in hand.
    charge: PairCharge,
    /// The tier the in-flight read was last submitted at.
    read_tier: ReadTier,
    /// Queued in `promotions`.
    promotion_queued: bool,
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
    /// In-flight band reads resubmitted at the mandatory tier after their
    /// block became mandatory or visible.
    pub(crate) tier_raises: u64,
    /// Batches the renderer failed and rolled back.
    pub(crate) aborted_drains: u64,
    /// Visible misses, in two buckets of block-frames drawn on any frame
    /// that draws cells, counted after the frame's drain. A block may land in
    /// both. Outside the baked set: drawn while not mandatory at lead L,
    /// resident or not; this is the check on the baked set's dilation.
    pub(crate) drawn_outside_baked_set: u64,
    /// Drawn while not installed, whatever its class: the stream lagged, or
    /// a non-portal frame drew a block it may not read.
    pub(crate) drawn_not_resident: u64,
    pub(crate) last_frame_drawn_outside_baked_set: u32,
    pub(crate) last_frame_drawn_not_resident: u32,
}

/// Gauges of what is resident and what the current camera cell's mandatory
/// set (lead L plus the pins) needs, in upload bytes per section: id 22
/// (irradiance plus direction) and id 42 (both shadowmask groups). Kept
/// incrementally on install, eviction and retarget, never recomputed.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct LightmapResidencyBytes {
    pub(crate) resident_blocks: u32,
    pub(crate) resident_lightmap_bytes: u64,
    pub(crate) resident_shadowmask_bytes: u64,
    pub(crate) mandatory_blocks: u32,
    pub(crate) mandatory_lightmap_bytes: u64,
    pub(crate) mandatory_shadowmask_bytes: u64,
}

impl LightmapResidencyBytes {
    fn add_resident(&mut self, facts: &BlockFacts) {
        self.resident_blocks += 1;
        self.resident_lightmap_bytes += facts.lightmap_bytes;
        self.resident_shadowmask_bytes += facts.shadowmask_bytes;
    }

    fn remove_resident(&mut self, facts: &BlockFacts) {
        self.resident_blocks -= 1;
        self.resident_lightmap_bytes -= facts.lightmap_bytes;
        self.resident_shadowmask_bytes -= facts.shadowmask_bytes;
    }

    fn add_mandatory(&mut self, facts: &BlockFacts) {
        self.mandatory_blocks += 1;
        self.mandatory_lightmap_bytes += facts.lightmap_bytes;
        self.mandatory_shadowmask_bytes += facts.shadowmask_bytes;
    }

    fn remove_mandatory(&mut self, facts: &BlockFacts) {
        self.mandatory_blocks -= 1;
        self.mandatory_lightmap_bytes -= facts.lightmap_bytes;
        self.mandatory_shadowmask_bytes -= facts.shadowmask_bytes;
    }
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
    /// This frame is not a portal walk: it may read only the camera cell's
    /// baked set and the pins.
    camera_set_only: bool,
    needs_target_reset: bool,
    drain_outstanding: bool,
    permits_in_use: usize,
    in_hand_bytes: u64,
    /// Band pairs' share of the permits and in-hand bytes.
    band_permits: usize,
    band_in_hand_bytes: u64,
    /// Slot texels of the band pairs in hand.
    band_committed_texels: u64,
    /// Slot texels of the mandatory and visible pairs in hand. The renderer
    /// places them before any band pair, so they come off band headroom.
    never_refused_texels: u64,
    /// In-flight band reads whose block became mandatory or visible; their
    /// requests are resubmitted at the mandatory tier.
    promotions: Vec<u32>,
    pool: LightmapPoolReport,
    refused_blocks: usize,
    counters: LightmapResidencyCounters,
    residency: LightmapResidencyBytes,
    /// The latest demand update drew cells whose blocks have not yet been
    /// counted for visible misses.
    misses_due: bool,
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
                    charge: PairCharge::None,
                    read_tier: ReadTier::Optional,
                    promotion_queued: false,
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
            camera_set_only: false,
            needs_target_reset: true,
            drain_outstanding: false,
            permits_in_use: 0,
            in_hand_bytes: 0,
            band_permits: 0,
            band_in_hand_bytes: 0,
            band_committed_texels: 0,
            never_refused_texels: 0,
            promotions: Vec::new(),
            pool: LightmapPoolReport::default(),
            refused_blocks: 0,
            counters: LightmapResidencyCounters::default(),
            residency: LightmapResidencyBytes::default(),
            misses_due: false,
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

    pub(crate) fn levers(&self) -> LightmapLevers {
        self.levers
    }

    /// The dev-tools sliders and capture's cap override write here. A lead
    /// change takes effect at the next [`Self::update`]; the cap rides the
    /// next drain batch.
    #[cfg(any(test, feature = "capture", feature = "dev-tools"))]
    pub(crate) fn levers_mut(&mut self) -> &mut LightmapLevers {
        &mut self.levers
    }

    pub(crate) fn counters(&self) -> LightmapResidencyCounters {
        let demand = self.demand.counters();
        LightmapResidencyCounters {
            baked_recomputes: demand.baked_recomputes,
            residency_lookups: demand.residency_lookups,
            ..self.counters
        }
    }

    pub(crate) fn residency_bytes(&self) -> LightmapResidencyBytes {
        self.residency
    }

    /// The pool shape from the renderer's latest outcome.
    pub(crate) fn pool_report(&self) -> LightmapPoolReport {
        self.pool
    }

    pub(crate) fn block_count(&self) -> usize {
        self.slots.len()
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
            self.promotions.capacity(),
            self.outcome_scratch.capacity(),
        ]);
        capacities
    }

    /// Applies one frame's visibility: recomputes baked demand only when the
    /// camera cell or lead changed, updates visible demand on a portal walk,
    /// and retargets every changed block. A non-portal frame keeps a drawn
    /// block whose pair is resident or on its way, as visible or, when demand
    /// names it so, mandatory, and reads only the camera cell's baked set.
    /// The frame's drawn blocks are counted for visible misses by
    /// [`Self::count_visible_misses`], after the frame's drain.
    pub(crate) fn update(&mut self, frame: DemandFrame<'_>) {
        self.may_request = self.demand.update(&self.map, self.levers.lead(), frame);
        self.camera_set_only = !frame.is_portal_walk();
        self.retarget_dirty();
        self.misses_due = frame.draws_cells();
    }

    /// Capture's fixed view: the camera cell's baked set plus every drawn
    /// cell's blocks as visible, whatever the visibility path. Capture is an
    /// offline tool that renders the full view synchronously, so it is exempt
    /// from the in-play rule that a non-portal frame reads only the camera
    /// cell's baked set.
    pub(crate) fn update_capture_view(&mut self, frame: DemandFrame<'_>) {
        self.demand
            .update_capture_view(&self.map, self.levers.lead(), frame);
        self.may_request = true;
        self.camera_set_only = false;
        self.retarget_dirty();
        self.misses_due = frame.draws_cells();
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
        self.camera_set_only = true;
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
            slot.target
                .is_none_or(|target| target.class != LightmapBlockClass::Mandatory)
                || slot.phase == BlockPhase::Installed
        })
    }

    fn retarget_dirty(&mut self) {
        for index in 0..self.demand.dirty_len() {
            let block = self.demand.dirty_at(index);
            let mut next = self.demand.target(&self.map, block);
            if self.demand.is_held(block) && self.slots[block as usize].phase.holds_pair() {
                next = Some(BlockTarget::held(next));
            }
            self.retarget(block, next);
        }
        self.demand.clear_dirty();
    }

    /// Counts the latest frame's drawn blocks into the two visible-miss
    /// buckets, once. Call after the frame's drain outcome is applied, so a
    /// block that drain installed is resident for the frame that draws it.
    /// The buckets overlap: outside the baked set counts every drawn block
    /// not mandatory at lead L, resident or not; not resident counts every
    /// drawn block not installed, whatever its class. A frame that draws no
    /// cell, or one already counted, reports zero.
    pub(crate) fn count_visible_misses(&mut self) {
        let (mut outside, mut not_resident) = (0u32, 0u32);
        if std::mem::take(&mut self.misses_due) {
            for &block in self.demand.drawn_blocks() {
                let slot = &self.slots[block as usize];
                if slot
                    .target
                    .is_none_or(|target| target.class != LightmapBlockClass::Mandatory)
                {
                    outside += 1;
                }
                if slot.phase != BlockPhase::Installed {
                    not_resident += 1;
                }
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
        let mandatory = |target: Option<BlockTarget>| {
            target.is_some_and(|t| t.class == LightmapBlockClass::Mandatory)
        };
        match (mandatory(previous), mandatory(next)) {
            (false, true) => self.residency.add_mandatory(self.map.facts(block)),
            (true, false) => self.residency.remove_mandatory(self.map.facts(block)),
            _ => {}
        }
        self.slots[index].target = next;
        if next.is_some_and(|target| target.class != LightmapBlockClass::Band) {
            self.promote_in_hand(block);
        }
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
