//! Renderer-owned residency mirrors for cluster-streamed SH data.
//!
//! The loader and the application deliberately do not receive any of these
//! addresses.  They exchange only `ShDrainBatch`/`ShDrainOutcome`; this module
//! turns a checked chunk into renderer-local pool ranges and keeps the sampled
//! indirection one drain behind the compose indirection.

use std::collections::{BTreeMap, BTreeSet};

use postretro_level_format::SectionId;
#[cfg(test)]
use postretro_level_format::cluster_directory::{ClusterDirectorySection, ClusterResourceDomain};
use postretro_level_format::cluster_sh_payloads::DecodedClusterShPayload;
use postretro_level_loader::{PreparedShCluster, ShDrainBatch, ShDrainOutcome, ShStreamManifest};
use postretro_render_cpu::frame_uniforms::LightTermMask;

use super::Renderer;
use super::animated_direct_sh_compose::AnimatedDirectShDebugOverride;
use super::direct_sh_compose::DirectShDebugOverride;
use super::level_resource_set::LevelSetId;
use super::renderer_types::{
    MAX_ANIMATED_BAKED_LIGHTS, PromotedBakedLightState, animated_baked_promotion_weight,
};

mod allocator;
#[cfg(test)]
mod cluster_demand_tests;
mod compose_plan;
#[cfg(test)]
mod compose_plan_oracle;
mod compose_staleness;
#[cfg(test)]
mod contributing_rows_tests;
mod dense;
mod diagnostics;
mod direct_compose;
mod frame;
mod gpu;
mod install;
mod install_journal;
#[cfg(test)]
mod install_tests;
mod lifecycle;
mod patches;
mod payload;
mod pipelines;
mod pool_admission;
mod pool_demand;
mod pool_size;
#[cfg(feature = "dev-tools")]
mod probe_diagnostics;
#[cfg(feature = "dev-tools")]
mod probe_readback;
mod row_bitset;
mod row_refs;
mod rows;
mod sample_region_index;
mod sample_regions;
mod setup;
#[cfg(test)]
pub(in crate::render) use setup::{residency_plan_summary, whole_map_bound_for_test};
mod sparse_install;
mod storage_refs;
#[cfg(test)]
mod storage_refs_tests;
mod stored_nodes;
#[cfg(test)]
mod tests;

use allocator::{FirstFitRanges, PoolRange, SparsePool};
use compose_plan::{ComposeFramePlan, RowMembership, StreamedComposePlanner};
use diagnostics::{InstallCpuCounters, PoolGrowthCounters};
pub use diagnostics::{ShMissBuckets, ShStreamingLiveDiagnostics};
use direct_compose::DirectSparseRowUpload;
pub(crate) use gpu::StagedUploads;
use gpu::StreamingGpuPools;
#[cfg(test)]
pub(in crate::render) use gpu::write_nine_cluster_prl;
use gpu::{AtlasShape, buffer_with_zeroes, checked_cell_count, u32_bytes};
use install::InstallGpu;
use install_journal::InstallJournal;
use patches::{ChunkPatches, SlotRun};
use payload::{ParsedSparseRow, SparseInstallPlan, parse_sparse_rows};
pub(crate) use pipelines::StreamingShPipelines;
use pool_admission::PoolAdmission;
use pool_size::plan_initial_pool;
#[cfg(feature = "dev-tools")]
pub(in crate::render) use probe_diagnostics::ProbeResidencyClass;
use row_bitset::{RowBitSet, RowQueue};
use row_refs::{RowRefTable, RowSet, decrement_row_ref, increment_row_ref, row_counts};
use sample_region_index::SampleRegionIndex;
use storage_refs::StorageRefs;
use stored_nodes::{StoredNode, StoredNodeLayout, rewrite_slot};

const PROBE_PATCH_BLOCK: u32 = 0;
const ISOLATED_ATLAS_BLOCK: u32 = 1;
const SPARSE_ROWS_BLOCK: u32 = 3;
const PROBE_INDIRECTION_VALID_BIT: u32 = 0x4;
const PROBE_INDIRECTION_LEVEL_MASK: u32 = 0x3;
const PROBE_INDIRECTION_SCALE_SHIFT: u32 = 3;
const PROBE_INDIRECTION_SCALE_MASK: u32 = 0x18;
const PROBE_INDIRECTION_SLOT_SHIFT: u32 = 5;

const INDIRECT_DELTA_ID: u32 = SectionId::DeltaShVolumes as u32;
const INDIRECT_BASE_ID: u32 = SectionId::OctahedralShVolume as u32;
const DIRECT_BASE_ID: u32 = SectionId::DirectShVolume as u32;
const DIRECT_DELTA_ID: u32 = SectionId::DirectShDeltaVolumes as u32;
const ANIMATED_DIRECT_DELTA_ID: u32 = SectionId::AnimatedDirectShDeltaVolumes as u32;

pub use postretro_render_cpu::sh_residency::ShResidencyDrainError;
use postretro_render_cpu::sh_residency::{
    ClusterStorage, DenseSpans, NodeMap, ShPoolBound, ShResidencyPlan, SparseCapacities,
    SparseComposeLayout, SparseComposeLayouts,
};
#[cfg(test)]
use postretro_render_cpu::sh_residency::{plan_sh_residency, plan_sh_residency_parts};

/// One streamed SH compose pass's current-frame work and remaining lag.
///
/// These values are gauges, not lifetime counters: the renderer replaces them
/// on every planned frame so diagnostics describe the work consumers can
/// correlate with that frame's timing.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct ShComposePassDiagnostics {
    pub rows_composed: u64,
    /// Composed rows whose pass input carries a CSR entry: the pass's own
    /// section, or for Pass B also any row Pass A rewrote that frame,
    /// whatever made Pass A plan it. On a settled frame without control
    /// changes this equals `rows_composed`.
    pub entry_rows_composed: u64,
    pub dispatches: u64,
    pub lagged_rows_composed: u64,
    pub resident_rows_still_lagging: u64,
}

/// A plain-data read of renderer residency.  This intentionally reports pool
/// capacity and logical occupancy separately: one is physical GPU backing,
/// while the other is only the live subset within it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct ShResidencySnapshot {
    pub generation: u64,
    pub target_clusters: usize,
    pub installed_clusters: usize,
    pub sampleable_clusters: usize,
    pub dense_slot_capacity: u32,
    pub dense_live_slots: u32,
    pub indirect_sparse_entry_capacity: u32,
    pub direct_sparse_entry_capacity: u32,
    pub animated_direct_sparse_entry_capacity: u32,
    pub dirty_affinity_rows: usize,
    /// Bytes of persistent streamed metadata (both indirection mirrors,
    /// compose CSR metadata, and the fixed sample-side grid uniform).
    pub fixed_metadata_bytes: u64,
    /// Bytes physically allocated by the active streamed generation.
    pub active_capacity_bytes: u64,
    /// Bytes addressed by currently live pool allocations only.
    pub logical_occupancy_bytes: u64,
    /// Bytes kept alive by the single retiring generation, if any.
    pub retiring_capacity_bytes: u64,
    /// Temporary active-plus-retiring peak during a replacement transaction.
    pub replacement_peak_bytes: u64,
    /// Whole-resident billboard scatter ids 47/48; deliberately separate
    /// from streamable SH-pool savings.
    pub whole_resident_scatter_bytes: u64,
    /// Cumulative CPU microseconds spent installing ready clusters, summed
    /// over drains that carried ready work.
    pub install_cpu_total_micros: u64,
    /// Largest single-drain install time, in microseconds.
    pub install_cpu_max_drain_micros: u64,
    /// Install time of the most recent drain that carried ready work, in
    /// microseconds.
    pub install_cpu_last_drain_micros: u64,
    /// Largest install time of a drain that grew no pool, in microseconds.
    pub install_cpu_max_steady_drain_micros: u64,
    /// Streamed pool families whose physical capacity grew, cumulative.
    pub pool_growth_events: u64,
    /// Active physical capacity those growths added, cumulative.
    pub pool_growth_bytes: u64,
    /// Cumulative CPU microseconds spent inside pool growth transactions;
    /// part of `install_cpu_total_micros`.
    pub pool_growth_cpu_micros: u64,
    /// Current-frame streamed indirect compose work.
    pub indirect_compose: ShComposePassDiagnostics,
    /// Current-frame streamed static-direct (promotion) compose work.
    pub static_direct_compose: ShComposePassDiagnostics,
    /// Current-frame streamed animated-direct compose work.
    pub animated_direct_compose: ShComposePassDiagnostics,
    /// Current-frame CPU time spent planning all streamed compose passes.
    pub compose_planning_cpu_micros: u64,
}

impl From<crate::render::uploads::UploadError> for ShResidencyDrainError {
    fn from(error: crate::render::uploads::UploadError) -> Self {
        use crate::render::uploads::UploadError;
        match error {
            UploadError::Alignment => Self::GpuCapacity {
                reason: "streamed buffer upload is not word aligned",
            },
            UploadError::Bounds => Self::SlotOverflow,
            UploadError::TexturePayload => Self::MalformedChunk {
                cluster_id: 0,
                reason: "streamed texture upload rows disagree with their payload",
            },
        }
    }
}

#[derive(Debug)]
struct InstalledCluster {
    patches: Vec<InstalledProbe>,
    /// Each of these holds one `StorageRefs` reference, as does every patch's
    /// word and every sparse row below.
    referenced_nodes: Vec<StoredNode>,
    sparse_rows: Vec<(u32, u32)>,
    required_indirect_epoch: u64,
    required_direct_epoch: u64,
}

#[derive(Debug, Clone, Copy)]
struct InstalledProbe {
    dense: u32,
    mean_distance: u16,
    mean_sq_distance: u16,
}

/// Renderer-local stream state.  It is constructed from loader metadata only;
/// decoded chunk bodies move through `drain` and are never retained after the
/// addresses and upload plan have been derived.
pub(super) struct ShResidencyState {
    content_tag: [u8; 32],
    base_metadata: postretro_level_loader::ShStreamBaseMetadata,
    source_metadata: postretro_level_loader::ShStreamSourceMetadata,
    cluster_count: u32,
    grid_dimensions: [u32; 3],
    generation: u64,
    targets: BTreeSet<u32>,
    dense_node: Vec<Option<StoredNode>>,
    dense_node_local_slot: Vec<Option<u32>>,
    node_layouts: NodeMap<StoredNodeLayout>,
    /// Read only by dev-tools probe classification and the plan summary.
    #[cfg_attr(
        not(any(test, feature = "dev-tools", feature = "test-seams")),
        allow(dead_code)
    )]
    dense_spans: DenseSpans,
    /// What each cluster names, so growth can price targets not yet read.
    cluster_storage: Vec<ClusterStorage>,
    node_slots: BTreeMap<StoredNode, PoolRange>,
    storage_refs: StorageRefs,
    dense_slots: FirstFitRanges,
    sparse_pools: BTreeMap<u32, SparsePool>,
    compose_words: Vec<u32>,
    sampled_words: Vec<u32>,
    installed: BTreeMap<u32, InstalledCluster>,
    sampleable: BTreeSet<u32>,
    pending_promotion: BTreeSet<u32>,
    dirty_rows: BTreeSet<(u32, u32)>,
    indirect_dirty_rows: BTreeSet<u32>,
    indirect_resident_rows: RowBitSet,
    indirect_base_row_refs: BTreeMap<u32, u32>,
    indirect_delta_row_refs: BTreeMap<u32, u32>,
    direct_promotion_dirty_rows: BTreeSet<u32>,
    direct_animated_dirty_rows: BTreeSet<u32>,
    direct_promotion_resident_rows: RowBitSet,
    direct_animated_resident_rows: RowBitSet,
    direct_base_row_refs: BTreeMap<u32, u32>,
    direct_promotion_row_refs: BTreeMap<u32, u32>,
    direct_animated_row_refs: BTreeMap<u32, u32>,
    direct_required: bool,
    direct_compose_required: bool,
    animated_direct_compose_required: bool,
    direct_animation_descriptor_indices: Vec<u32>,
    compose_planner: StreamedComposePlanner,
    compose_frame_plan: Option<ComposeFramePlan>,
    /// Load-time writer-row and per-cell row caches for the sampled-row gate.
    sample_region_index: SampleRegionIndex,
    /// This frame's dynamic (mover and forward-mesh) sample regions.
    compose_input_regions: Vec<crate::render::ShSampleRegion>,
    compose_region_rows: Vec<u32>,
    compose_residency_rows: Vec<u32>,
    /// Rows whose row-ref presence changed since the last compose plan. The
    /// planner re-observes only these, so membership stays O(residency delta).
    /// Drains that run without a compose plan (non-rendering frames, a failed
    /// prepare) keep adding to it, so each row is queued at most once: the
    /// list never exceeds the planner row capacity.
    compose_membership_touched: RowQueue,
    compose_membership_changes: Vec<(u32, RowMembership)>,
    compose_animated_weights: Vec<f32>,
    indirect_compose_diagnostics: ShComposePassDiagnostics,
    static_direct_compose_diagnostics: ShComposePassDiagnostics,
    animated_direct_compose_diagnostics: ShComposePassDiagnostics,
    compose_planning_cpu_micros: u64,
    generation_has_reset: bool,
    indirect_compose_epoch: u64,
    direct_compose_epoch: u64,
    install_cpu: InstallCpuCounters,
    /// The stored bound and whole-map figure that size the pool generation.
    pool_bound: ShPoolBound,
    whole_dense_slots: u32,
    /// A refusal at the whole-map cap warns once per loaded level.
    whole_map_refusal_warned: bool,
    /// The plan's sparse compose layouts, shared with the GPU pools so pool
    /// growth rebuilds carriers without re-deriving them.
    sparse_compose: std::sync::Arc<SparseComposeLayouts>,
    gpu: Option<StreamingGpuPools>,
    #[cfg(feature = "dev-tools")]
    probe_readback: probe_readback::StreamedProbeReadback,
}

impl ShResidencyState {
    /// Compose the streamed direct atlas after promotion assignment. Pass A
    /// covers id-35 + id-41 and Pass B covers id-35 + id-41 + id-45. A
    /// successful two-pass encode advances the direct epoch; promotion happens
    /// at the following drain, never while the frame can still observe writes.
    fn grid_dimensions(&self) -> [u32; 3] {
        self.grid_dimensions
    }

    /// The manifest base lattice, origin and cell size: what this level's
    /// streamed grid uniform carries, so what the SH sampler reads.
    pub(in crate::render) fn base_lattice(&self) -> ([f32; 3], [f32; 3]) {
        (self.base_metadata.grid_origin, self.base_metadata.cell_size)
    }

    fn logical_occupancy_bytes(&self) -> u64 {
        let Some(gpu) = self.gpu.as_ref() else {
            return 0;
        };
        let dense_slots = self
            .node_slots
            .values()
            .try_fold(0u64, |total, range| total.checked_add(u64::from(range.len)))
            .expect("validated streamed dense ranges must fit the logical ledger");
        let dense_bytes = dense_slots
            .checked_mul(
                gpu.logical_bytes_per_dense_slot()
                    .expect("validated streamed dense source bytes must fit the logical ledger"),
            )
            .expect("validated streamed dense bytes must fit the logical ledger");
        let sparse_f16 = self
            .sparse_pools
            .values()
            .try_fold(0u64, |total, pool| {
                total.checked_add(u64::from(pool.logical_occupancy().1))
            })
            .expect("validated streamed sparse tile bytes must fit the logical ledger");
        let sparse_entries = self
            .sparse_pools
            .values()
            .try_fold(0u64, |total, pool| {
                total.checked_add(u64::from(pool.logical_occupancy().0))
            })
            .expect("validated streamed sparse entry bytes must fit the logical ledger");
        dense_bytes
            .checked_add(
                sparse_f16
                    .checked_mul(2)
                    .expect("validated streamed sparse tile bytes must fit the logical ledger"),
            )
            .and_then(|bytes| {
                sparse_entries
                    .checked_mul(16)
                    .and_then(|entries| bytes.checked_add(entries))
            })
            .expect("validated streamed logical occupancy must fit u64")
    }
}

impl Renderer {
    /// Drain the loader-owned batch at the single pre-compose boundary.  The
    /// application/controller remains responsible for applying the returned
    /// permit and ready-byte outcome; it never observes renderer pool slots.
    pub fn drain_sh_residency(
        &mut self,
        batch: ShDrainBatch,
    ) -> Result<ShDrainOutcome, ShResidencyDrainError> {
        let Self {
            device,
            queue,
            full,
            ..
        } = self;
        let full = full
            .as_mut()
            .expect("renderer full-init must complete before full-ready paths run");
        let Some((state, level)) = full.level.as_mut().and_then(|level| {
            let super::LevelResourceSet {
                sh_streaming,
                sh_volume_resources,
                promoted_static_weight_buffer,
                ..
            } = level;
            sh_streaming.as_mut().map(|state| {
                (
                    state,
                    (sh_volume_resources, &*promoted_static_weight_buffer),
                )
            })
        }) else {
            return Ok(ShDrainOutcome {
                dropped: batch
                    .ready
                    .into_iter()
                    .map(|ready| ready.chunk.cluster_id)
                    .collect(),
                ..ShDrainOutcome::default()
            });
        };
        state.drain(device, queue, level.0, level.1, batch)
    }

    pub fn sh_residency_snapshot(&self) -> Option<ShResidencySnapshot> {
        self.full()
            .level
            .as_ref()?
            .sh_streaming
            .as_ref()
            .map(ShResidencyState::snapshot)
    }

    /// [`Self::sh_residency_snapshot`] for the built set `id`, active or
    /// resident and unpublished: what a level's commit checks its streaming
    /// sessions against before it publishes the set. `None` for an unknown
    /// set or one that streams no SH.
    pub fn level_set_sh_residency_snapshot(&self, id: LevelSetId) -> Option<ShResidencySnapshot> {
        let full = self.full.as_ref()?;
        full.level
            .iter()
            .chain(&full.resident_levels)
            .find(|level| level.id == id)?
            .sh_streaming
            .as_ref()
            .map(ShResidencyState::snapshot)
    }
}

fn validate_generation_transition(
    current: u64,
    received: u64,
    has_target_reset: bool,
) -> Result<bool, ShResidencyDrainError> {
    if received == 0 {
        return Err(ShResidencyDrainError::GenerationResetRequired {
            expected: current,
            received,
        });
    }
    if received < current {
        return Err(ShResidencyDrainError::StaleGeneration { current, received });
    }
    let new_generation = current != received;
    if new_generation && !has_target_reset {
        return Err(ShResidencyDrainError::GenerationResetRequired {
            expected: current,
            received,
        });
    }
    if !new_generation && has_target_reset {
        return Err(ShResidencyDrainError::InvalidTargetResetLifecycle);
    }
    Ok(new_generation)
}

fn validate_isolated_atlas_block(
    cluster_id: u32,
    bytes: &[u8],
    expected_slots: u32,
) -> Result<(), ShResidencyDrainError> {
    if bytes.len() < 20 {
        return Err(malformed(cluster_id, "isolated atlas header is truncated"));
    }
    let slot_count =
        u32_at(bytes, 4).ok_or(malformed(cluster_id, "isolated atlas header is truncated"))?;
    let width =
        u32_at(bytes, 8).ok_or(malformed(cluster_id, "isolated atlas header is truncated"))?;
    let height =
        u32_at(bytes, 12).ok_or(malformed(cluster_id, "isolated atlas header is truncated"))?;
    let layers =
        u32_at(bytes, 16).ok_or(malformed(cluster_id, "isolated atlas header is truncated"))?;
    if slot_count != expected_slots || width == 0 || height == 0 || layers == 0 {
        return Err(malformed(
            cluster_id,
            "isolated atlas shape disagrees with chunk table",
        ));
    }
    Ok(())
}

fn u32_at(bytes: &[u8], offset: usize) -> Option<u32> {
    bytes
        .get(offset..offset + 4)
        .map(|value| u32::from_le_bytes(value.try_into().expect("four-byte slice")))
}

fn malformed(cluster_id: u32, reason: &'static str) -> ShResidencyDrainError {
    ShResidencyDrainError::MalformedChunk { cluster_id, reason }
}

fn required_compose_epochs(
    indirect_epoch: u64,
    direct_epoch: u64,
    direct_compose_required: bool,
    wrote_dense: bool,
    sparse_sections: impl IntoIterator<Item = u32>,
) -> Result<(u64, u64), ShResidencyDrainError> {
    let sections = sparse_sections.into_iter().collect::<BTreeSet<_>>();
    let requires_indirect = wrote_dense || sections.contains(&INDIRECT_DELTA_ID);
    let requires_direct = direct_compose_required
        && (wrote_dense
            || sections.contains(&DIRECT_DELTA_ID)
            || sections.contains(&ANIMATED_DIRECT_DELTA_ID));
    Ok((
        if requires_indirect {
            indirect_epoch
                .checked_add(1)
                .ok_or(ShResidencyDrainError::SlotOverflow)?
        } else {
            indirect_epoch
        },
        if requires_direct {
            direct_epoch
                .checked_add(1)
                .ok_or(ShResidencyDrainError::SlotOverflow)?
        } else {
            direct_epoch
        },
    ))
}

#[cfg(test)]
pub(crate) use gpu::UploadOrderSh;
