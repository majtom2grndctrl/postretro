//! Lightmap residency fixtures: a held-read block source, a corridor level, a rig.
//! See: context/lib/testing_guide.md

use std::collections::BTreeSet;
use std::ops::Range;
use std::sync::{Arc, Mutex};
use std::thread::ThreadId;
use std::time::{Duration, Instant};

use postretro_level_format::cell_residency_set::{CellResidencySetSection, ResidencyEntry};
use postretro_level_format::lightmap::{LIGHTMAP_POOL_LAYER_EDGE, LightmapBlockPayload};
use postretro_level_loader::{
    LightmapBlockFileRanges, LightmapDrainBatch, LightmapDrainOutcome, LightmapPoolReport,
    PrlLoadError,
};
use postretro_render_cpu::lightmap_pool::LightmapPoolModel;
use postretro_visibility::{VisibilityPath, VisibleCells};

use super::controller::LightmapResidencyController;
use super::route::{LightmapCompletion, LightmapReadResult};
use super::source::{BlockSummary, LightmapBlockSource};
use crate::streaming::cell_demand::LEAD_UNITS_PER_METRE;
use crate::streaming::cluster_hints::ClusterHints;
use crate::streaming::read_gate_test_fixture::ReadGate;
use crate::streaming::request::{ReadRequest, StreamResource};
use crate::streaming::shared_drain::SharedDrain;

pub(crate) const M: u32 = LEAD_UNITS_PER_METRE;
pub(crate) const MIB: u64 = 1024 * 1024;
/// Far enough apart that no two test ranges coalesce into one read.
pub(crate) const FAR: u64 = 64 * MIB;
/// Where the id-42 halves start: after every id-22 half.
pub(crate) const SHADOWMASK_BASE: u64 = 1 << 40;
pub(crate) const PORTAL: VisibilityPath = VisibilityPath::PrlPortal { walk_reach: 0 };

pub(crate) fn pattern(range: &Range<u64>) -> Vec<u8> {
    range.clone().map(|offset| (offset % 251) as u8).collect()
}

/// One block's shape and absolute file ranges.
#[derive(Debug, Clone)]
pub(crate) struct BlockSpec {
    pub(crate) cell: u32,
    pub(crate) width: u16,
    pub(crate) height: u16,
    pub(crate) lightmap: Range<u64>,
    pub(crate) shadowmask: Option<Range<u64>>,
}

impl BlockSpec {
    /// Block `block` of the standard layout: its id-22 half at
    /// `(2 * block + 1) * FAR`, its id-42 half after every id-22 half.
    pub(crate) fn standard(block: u32, cell: u32, half_bytes: u64, shadowmask: bool) -> Self {
        let start = (2 * u64::from(block) + 1) * FAR;
        let shadow_start = SHADOWMASK_BASE + u64::from(block) * FAR;
        Self {
            cell,
            width: 64,
            height: 64,
            lightmap: start..start + half_bytes,
            shadowmask: shadowmask.then(|| shadow_start..shadow_start + half_bytes),
        }
    }
}

/// Every read the test sources performed, in order, across resources.
#[derive(Debug, Default)]
pub(crate) struct ReadLog {
    pub(crate) reads: Mutex<Vec<(StreamResource, Range<u64>, ThreadId)>>,
    pub(crate) gate: ReadGate,
}

impl ReadLog {
    pub(crate) fn record(&self, resource: StreamResource, span: &Range<u64>) {
        self.reads
            .lock()
            .unwrap()
            .push((resource, span.clone(), std::thread::current().id()));
    }

    pub(crate) fn spans(&self) -> Vec<(StreamResource, Range<u64>)> {
        self.reads
            .lock()
            .unwrap()
            .iter()
            .map(|(resource, span, _)| (*resource, span.clone()))
            .collect()
    }

    pub(crate) fn count(&self) -> usize {
        self.reads.lock().unwrap().len()
    }

    pub(crate) fn wait_for_reads(&self, count: usize) {
        wait_until("reads", || self.count() >= count);
    }

    /// Reads whose span starts at `offset`.
    pub(crate) fn reads_at(&self, offset: u64) -> usize {
        self.reads
            .lock()
            .unwrap()
            .iter()
            .filter(|(_, span, _)| span.start == offset)
            .count()
    }
}

pub(crate) fn wait_until(what: &str, mut done: impl FnMut() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(10);
    while !done() {
        assert!(Instant::now() < deadline, "timed out waiting for {what}");
        std::thread::yield_now();
    }
}

/// An in-memory level: blocks at chosen file ranges, each byte a function of
/// its offset. Reads record into a shared log and block while held.
#[derive(Debug)]
pub(crate) struct TestBlockSource {
    blocks: Vec<BlockSpec>,
    pub(crate) log: Arc<ReadLog>,
}

impl TestBlockSource {
    pub(crate) fn new(blocks: Vec<BlockSpec>) -> Arc<Self> {
        Self::with_log(blocks, Arc::new(ReadLog::default()))
    }

    pub(crate) fn with_log(blocks: Vec<BlockSpec>, log: Arc<ReadLog>) -> Arc<Self> {
        Arc::new(Self { blocks, log })
    }

    pub(crate) fn spec(&self, block: u32) -> &BlockSpec {
        &self.blocks[block as usize]
    }

    /// The completion the issuer would deliver for `request`.
    pub(crate) fn completion(&self, request: ReadRequest) -> LightmapCompletion {
        let lightmap = pattern(&request.ranges.get(0));
        let shadowmask = (request.ranges.len() > 1).then(|| pattern(&request.ranges.get(1)));
        LightmapCompletion {
            request,
            result: LightmapReadResult::Read {
                lightmap,
                shadowmask,
            },
        }
    }
}

impl LightmapBlockSource for TestBlockSource {
    fn block_count(&self) -> u32 {
        self.blocks.len() as u32
    }

    fn block_summary(&self, block: u32) -> Option<BlockSummary> {
        self.blocks.get(block as usize).map(|spec| BlockSummary {
            cell_id: spec.cell,
            width: spec.width,
            height: spec.height,
        })
    }

    /// The BC edge: the alignment of a level with a direction scale of 1,
    /// 2 or 4.
    fn block_alignment(&self) -> u32 {
        4
    }

    fn block_file_ranges(&self, block: u32) -> Result<LightmapBlockFileRanges, PrlLoadError> {
        let spec = &self.blocks[block as usize];
        Ok(LightmapBlockFileRanges {
            lightmap: spec.lightmap.clone(),
            shadowmask: spec.shadowmask.clone(),
        })
    }

    fn read_file_span(&self, range: Range<u64>) -> Result<Vec<u8>, PrlLoadError> {
        self.log.record(StreamResource::LightmapBlock, &range);
        self.log.gate.wait_while_held(range.start);
        Ok(pattern(&range))
    }

    /// Splits each half at its midpoint: irradiance then direction, group A
    /// then group B.
    fn payload_from_pair_bytes(
        &self,
        block: u32,
        mut lightmap: Vec<u8>,
        shadowmask: Option<Vec<u8>>,
    ) -> Result<LightmapBlockPayload, PrlLoadError> {
        let spec = &self.blocks[block as usize];
        let len = |range: &Range<u64>| (range.end - range.start) as usize;
        let mismatch = || PrlLoadError::SectionValidation {
            section: "test source",
            message: format!("block {block} pair bytes do not match its ranges"),
        };
        if lightmap.len() != len(&spec.lightmap)
            || shadowmask.as_ref().map(Vec::len) != spec.shadowmask.as_ref().map(len)
        {
            return Err(mismatch());
        }
        let direction = lightmap.split_off(lightmap.len() / 2);
        Ok(LightmapBlockPayload {
            irradiance: lightmap,
            direction,
            shadowmask: shadowmask.map(|mut group_a| {
                let group_b = group_a.split_off(group_a.len() / 2);
                [group_a, group_b]
            }),
        })
    }

    fn content_tag(&self) -> [u8; 32] {
        [0x5a; 32]
    }
}

/// Builds id 51 from `(camera, cell, lead metres)` rows; each camera cell's
/// range is sorted by (lead, cell) as the format requires.
pub(crate) fn residency_set(
    cell_count: u32,
    rows: &[(u32, u32, u32)],
    max_lead_metres: u32,
) -> CellResidencySetSection {
    let mut offsets = vec![0];
    let mut entries = Vec::new();
    for camera in 0..cell_count {
        let mut range: Vec<ResidencyEntry> = rows
            .iter()
            .filter(|row| row.0 == camera)
            .map(|&(_, cell_id, lead)| ResidencyEntry {
                lead: lead * M,
                cell_id,
            })
            .collect();
        range.sort();
        entries.extend(range);
        offsets.push(entries.len() as u32);
    }
    CellResidencySetSection {
        max_lead: max_lead_metres * M,
        offsets,
        entries,
    }
}

/// Eight cells in a row; cell c owns block c except cell 7, which has no
/// charts and an empty baked range. Leads are in metres; the default L is
/// 16 m and the baked maximum 32 m.
pub(crate) fn corridor_set() -> CellResidencySetSection {
    residency_set(
        8,
        &[
            (0, 0, 0),
            (0, 1, 0),
            (0, 2, 8),
            (0, 3, 20),
            (0, 4, 30),
            (1, 1, 0),
            (1, 0, 0),
            (1, 2, 0),
            (1, 3, 10),
            (1, 4, 24),
            (2, 2, 0),
            (2, 1, 0),
            (2, 3, 0),
            (2, 0, 12),
            (2, 4, 12),
            (2, 5, 28),
            (3, 3, 0),
            (3, 2, 0),
            (3, 4, 0),
            (3, 1, 14),
            (3, 5, 14),
            (4, 4, 0),
            (4, 3, 0),
            (4, 5, 0),
            (4, 2, 18),
            (4, 6, 20),
            (5, 5, 0),
            (5, 4, 0),
            (5, 6, 0),
            (5, 3, 16),
            (6, 6, 0),
            (6, 5, 0),
        ],
        32,
    )
}

/// Blocks 0..=6 for cells 0..=6, each pair `half_bytes` per half.
pub(crate) fn corridor_blocks(half_bytes: u64, shadowmask: bool) -> Vec<BlockSpec> {
    (0..7)
        .map(|block| BlockSpec::standard(block, block, half_bytes, shadowmask))
        .collect()
}

/// Cell c belongs to cluster c / 2. `pinned` clusters are pinned; each
/// `(cluster, priority)` sets an authored prefetch priority.
pub(crate) fn corridor_hints(pinned: &[u32], priorities: &[(u32, u8)]) -> ClusterHints {
    let mut priority = vec![0; 4];
    for &(cluster, value) in priorities {
        priority[cluster as usize] = value;
    }
    ClusterHints {
        cell_to_cluster: (0..8).map(|cell| cell / 2).collect(),
        pinned: pinned.iter().copied().collect::<BTreeSet<_>>(),
        priority,
    }
}

/// A controller driven on the test thread: requests are captured instead
/// of submitted, and completions and outcomes are fed back by the test.
pub(crate) struct Rig {
    pub(crate) source: Arc<TestBlockSource>,
    pub(crate) set: CellResidencySetSection,
    /// The level-scope stage owning lead L.
    pub(crate) stage: crate::streaming::cell_demand::CellDemand,
    pub(crate) controller: LightmapResidencyController,
    pub(crate) requests: Vec<ReadRequest>,
    pub(crate) drain: SharedDrain,
}

impl Rig {
    pub(crate) fn corridor(hints: Option<&ClusterHints>) -> Self {
        Self::new(corridor_blocks(64, true), corridor_set(), hints)
    }

    pub(crate) fn new(
        blocks: Vec<BlockSpec>,
        set: CellResidencySetSection,
        hints: Option<&ClusterHints>,
    ) -> Self {
        let source = TestBlockSource::new(blocks);
        let controller = LightmapResidencyController::new(
            Arc::clone(&source) as Arc<dyn LightmapBlockSource>,
            &set,
            hints,
        )
        .unwrap();
        Self {
            source,
            stage: crate::streaming::cell_demand::CellDemand::new(set.max_lead),
            set,
            controller,
            requests: Vec::new(),
            drain: SharedDrain::default(),
        }
    }

    /// One frame: demand, a lightmap-only drain, then requests.
    pub(crate) fn frame(
        &mut self,
        camera_cell: u32,
        path: VisibilityPath,
        visible_cells: &VisibleCells,
    ) -> Option<LightmapDrainBatch> {
        self.frame_as(camera_cell, path, visible_cells, false)
    }

    /// One Settling frame: capture-view demand, so every drawn block is
    /// requested on any path.
    pub(crate) fn settling_frame(
        &mut self,
        camera_cell: u32,
        path: VisibilityPath,
        visible_cells: &VisibleCells,
    ) -> Option<LightmapDrainBatch> {
        self.frame_as(camera_cell, path, visible_cells, true)
    }

    fn frame_as(
        &mut self,
        camera_cell: u32,
        path: VisibilityPath,
        visible_cells: &VisibleCells,
        settling: bool,
    ) -> Option<LightmapDrainBatch> {
        let Self {
            set,
            stage,
            controller,
            requests,
            drain,
            ..
        } = self;
        let frame = stage.frame(set, camera_cell, path, visible_cells);
        if settling {
            controller.update_capture_view(frame);
        } else {
            controller.update(frame);
        }
        drain.begin();
        controller.offer_ready(drain).unwrap();
        drain.admit().unwrap();
        let batch = controller.finish_drain(drain).unwrap();
        controller
            .take_requests(&mut |request| {
                requests.push(request);
                Ok(())
            })
            .unwrap();
        batch
    }

    pub(crate) fn portal(&mut self, camera_cell: u32, drawn: &[u32]) -> Option<LightmapDrainBatch> {
        self.frame(camera_cell, PORTAL, &VisibleCells::Culled(drawn.to_vec()))
    }

    /// Delivers every captured request's read.
    pub(crate) fn complete_all(&mut self) {
        for request in std::mem::take(&mut self.requests) {
            let completion = self.source.completion(request);
            self.controller.admit_completion(completion).unwrap();
        }
    }

    pub(crate) fn requested_blocks(&self) -> Vec<u32> {
        self.requests.iter().map(|request| request.key).collect()
    }

    /// The renderer installs every pair and reports `pool`.
    pub(crate) fn install_all(&mut self, batch: &LightmapDrainBatch, pool: LightmapPoolReport) {
        self.controller
            .apply_outcome(LightmapDrainOutcome {
                installed: batch.ready.iter().map(|prepared| prepared.block).collect(),
                pool,
                ..LightmapDrainOutcome::default()
            })
            .unwrap();
    }

    /// Frames at `camera_cell` until every targeted block is installed.
    pub(crate) fn settle(&mut self, camera_cell: u32, drawn: &[u32], pool: LightmapPoolReport) {
        for _ in 0..16 {
            let batch = self
                .portal(camera_cell, drawn)
                .expect("no outcome outstanding");
            self.install_all(&batch, pool);
            self.complete_all();
        }
        let batch = self.portal(camera_cell, drawn).unwrap();
        self.install_all(&batch, pool);
    }
}

/// The renderer's placement policy over `source`'s blocks, standing in for
/// the GPU pool: the same wgpu-free model the renderer drains through.
pub(crate) fn pool_model(source: &TestBlockSource) -> LightmapPoolModel {
    let extents = (0..source.block_count())
        .map(|block| {
            let spec = source.spec(block);
            (u32::from(spec.width), u32::from(spec.height))
        })
        .collect();
    LightmapPoolModel::new(extents, 4, LIGHTMAP_POOL_LAYER_EDGE, 15).expect("test blocks fit")
}

/// One renderer drain through `model`: its plan becomes the outcome, the
/// deferred payloads handed back owned.
pub(crate) fn model_drain(
    model: &mut LightmapPoolModel,
    batch: LightmapDrainBatch,
) -> LightmapDrainOutcome {
    let plan = model.plan_batch(&batch);
    let (installed, refused, failed, deferred_blocks, evicted, pool) = (
        plan.installed.clone(),
        plan.refused.clone(),
        plan.failed.clone(),
        plan.deferred.clone(),
        plan.evicted.iter().map(|eviction| eviction.block).collect(),
        plan.report,
    );
    LightmapDrainOutcome {
        installed,
        refused,
        failed,
        deferred: batch
            .ready
            .into_iter()
            .filter(|prepared| deferred_blocks.contains(&prepared.block))
            .collect(),
        evicted,
        pool,
    }
}

/// Band headroom for `blocks` 64×64 blocks.
pub(crate) fn headroom(blocks: u64) -> LightmapPoolReport {
    LightmapPoolReport {
        layers: 1,
        band_headroom_texels: blocks * 64 * 64,
        ..LightmapPoolReport::default()
    }
}
