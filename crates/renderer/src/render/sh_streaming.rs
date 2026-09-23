//! Renderer-owned residency mirrors for cluster-streamed SH data.
//!
//! The loader and the application deliberately do not receive any of these
//! addresses.  They exchange only `ShDrainBatch`/`ShDrainOutcome`; this module
//! turns a checked chunk into renderer-local pool ranges and keeps the sampled
//! indirection one drain behind the compose indirection.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;

use postretro_level_format::SectionId;
use postretro_level_format::cluster_directory::{
    ClusterDirectorySection, ClusterRangeRole, ClusterResourceDomain,
};
use postretro_level_format::cluster_sh_payloads::DecodedClusterShPayload;
use postretro_level_format::delta_sh_volumes::delta_probe_f16_stride;
use postretro_level_format::sh_reconstruct::{Level, stored_delta_tiles};
use postretro_level_loader::{PreparedShCluster, ShDrainBatch, ShDrainOutcome, ShStreamManifest};
use postretro_render_cpu::frame_uniforms::LightTermMask;

use super::animated_direct_sh_compose::AnimatedDirectShDebugOverride;
use super::direct_sh_compose::DirectShDebugOverride;
use super::renderer_types::PromotedBakedLightState;
use super::{Renderer, sh_compose_dispatch::should_dispatch};

mod allocator;
mod dense;
mod direct_compose;
mod floor;
mod frame;
mod gpu;
mod install;
mod lifecycle;
mod ownership;
mod patches;
mod payload;
mod rows;
mod setup;
mod sparse_install;
#[cfg(test)]
mod tests;

use allocator::{FirstFitRanges, PoolRange, SparsePool};
use direct_compose::DirectSparseRowUpload;
use floor::plan_initial_pool_floor;
use gpu::StreamingGpuPools;
use gpu::{AtlasShape, buffer_with_zeroes, checked_cell_count, sparse_compose_capacity, u32_bytes};
use ownership::{StoredNode, StoredNodeLayout, derive_dense_node_layout, rewrite_slot};
use payload::{ParsedSparseRow, SparseInstallPlan, parse_sparse_rows};

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

/// Physical entry/f16 backing needed to install every owned row in one
/// canonical cluster. Values include the allocator's missing-row sentinel
/// and f16 word alignment, but never reserve all legacy CSR payloads.
type SparseCapacityFloors = BTreeMap<u32, (u32, u32)>;

/// A plain-data read of renderer ownership.  This intentionally reports pool
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
    /// Initial non-evictable streamed pool floor, distinct from later growth.
    pub effective_floor_bytes: u64,
    /// Minimum physical capacity for the coupled id-34/id-35 dense pool.
    pub dense_group_minimum_bytes: Option<u64>,
    /// Minimum physical capacity for the id-27 sparse pool, when present.
    pub indirect_delta_minimum_bytes: Option<u64>,
    /// Minimum physical capacity for the id-41 sparse pool, when present.
    pub direct_delta_minimum_bytes: Option<u64>,
    /// Minimum physical capacity for the id-45 sparse pool, when present.
    pub animated_direct_delta_minimum_bytes: Option<u64>,
    /// Bytes addressed by currently live pool allocations only.
    pub logical_occupancy_bytes: u64,
    /// Bytes kept alive by the single retiring generation, if any.
    pub retiring_capacity_bytes: u64,
    /// Temporary active-plus-retiring peak during a replacement transaction.
    pub replacement_peak_bytes: u64,
    /// Whole-resident billboard scatter ids 47/48; deliberately separate
    /// from streamable SH-pool savings.
    pub whole_resident_scatter_bytes: u64,
}

/// A malformed renderer handoff is never repaired by inventing an address.
/// The loader has already validated the wire bytes; these errors therefore
/// catch a violated lifetime/identity contract at the renderer boundary.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ShResidencyDrainError {
    ContentTagMismatch,
    GenerationResetRequired {
        expected: u64,
        received: u64,
    },
    StaleGeneration {
        current: u64,
        received: u64,
    },
    InvalidTargetBitset,
    TargetOutOfRange(u32),
    DuplicateTargetDelta(u32),
    MalformedChunk {
        cluster_id: u32,
        reason: &'static str,
    },
    MissingDenseOwner {
        cluster_id: u32,
        dense_index: u32,
    },
    SlotOverflow,
    GpuCapacity {
        reason: &'static str,
    },
    /// A valid replacement is waiting only for this family’s previous GPU
    /// generation to drain. Unlike adapter limits, this is safe to retry
    /// while retaining the loader-owned ready permit.
    GpuRetirementPressure {
        family: &'static str,
    },
    InvalidTargetResetLifecycle,
}

impl fmt::Display for ShResidencyDrainError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::ContentTagMismatch => {
                write!(f, "SH drain content tag does not match renderer session")
            }
            Self::GenerationResetRequired { expected, received } => write!(
                f,
                "SH drain generation {received} replaced renderer generation {expected} without a target reset"
            ),
            Self::StaleGeneration { current, received } => write!(
                f,
                "SH drain generation {received} is older than active renderer generation {current}"
            ),
            Self::InvalidTargetBitset => write!(f, "SH drain target reset bitset is malformed"),
            Self::TargetOutOfRange(id) => write!(f, "SH drain names out-of-range cluster {id}"),
            Self::DuplicateTargetDelta(id) => write!(
                f,
                "SH drain names cluster {id} in conflicting target deltas"
            ),
            Self::MalformedChunk { cluster_id, reason } => {
                write!(
                    f,
                    "SH cluster {cluster_id} has malformed renderer payload: {reason}"
                )
            }
            Self::MissingDenseOwner {
                cluster_id,
                dense_index,
            } => write!(
                f,
                "SH cluster {cluster_id} patches dense probe {dense_index} without a canonical owner"
            ),
            Self::SlotOverflow => write!(
                f,
                "SH streamed pool address exceeds probe indirection capacity"
            ),
            Self::GpuCapacity { reason } => {
                write!(f, "SH streamed GPU pool capacity error: {reason}")
            }
            Self::GpuRetirementPressure { family } => write!(
                f,
                "SH streamed {family} pool growth waits for its retiring generation"
            ),
            Self::InvalidTargetResetLifecycle => write!(
                f,
                "SH drain must carry exactly one target reset when a generation begins"
            ),
        }
    }
}

impl std::error::Error for ShResidencyDrainError {}

impl ShResidencyDrainError {
    /// Only a submitted replacement that is still fenced is retryable. An
    /// adapter limit, malformed pool shape, or arithmetic overflow is a real
    /// renderer failure and must return the ready permit to the controller.
    const fn is_retryable_retirement_pressure(&self) -> bool {
        matches!(self, Self::GpuRetirementPressure { .. })
    }
}

#[derive(Debug)]
struct InstalledCluster {
    patches: Vec<InstalledProbe>,
    owned_nodes: Vec<StoredNode>,
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

fn increment_row_ref(refs: &mut BTreeMap<u32, u32>, row: u32) -> Result<(), ShResidencyDrainError> {
    let count = refs.entry(row).or_insert(0);
    *count = count
        .checked_add(1)
        .ok_or(ShResidencyDrainError::SlotOverflow)?;
    Ok(())
}

fn decrement_row_ref(refs: &mut BTreeMap<u32, u32>, row: u32) -> Result<(), ShResidencyDrainError> {
    let count = refs
        .get_mut(&row)
        .ok_or(ShResidencyDrainError::SlotOverflow)?;
    *count = count
        .checked_sub(1)
        .ok_or(ShResidencyDrainError::SlotOverflow)?;
    if *count == 0 {
        refs.remove(&row);
    }
    Ok(())
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
    dense_owner: Vec<Option<u32>>,
    dense_node: Vec<Option<StoredNode>>,
    dense_node_local_slot: Vec<Option<u32>>,
    node_layouts: BTreeMap<StoredNode, StoredNodeLayout>,
    node_owner: BTreeMap<StoredNode, u32>,
    nodes_by_owner: BTreeMap<u32, Vec<StoredNode>>,
    node_slots: BTreeMap<StoredNode, PoolRange>,
    owner_dependencies: Vec<BTreeSet<u32>>,
    sparse_row_owner: BTreeMap<(u32, u32), u32>,
    sparse_capacity_floors: SparseCapacityFloors,
    dense_slots: FirstFitRanges,
    sparse_pools: BTreeMap<u32, SparsePool>,
    compose_words: Vec<u32>,
    sampled_words: Vec<u32>,
    installed: BTreeMap<u32, InstalledCluster>,
    sampleable: BTreeSet<u32>,
    pending_promotion: BTreeSet<u32>,
    dirty_rows: BTreeSet<(u32, u32)>,
    indirect_dirty_rows: BTreeSet<u32>,
    indirect_resident_rows: BTreeSet<u32>,
    indirect_base_row_refs: BTreeMap<u32, u32>,
    indirect_delta_row_refs: BTreeMap<u32, u32>,
    direct_promotion_dirty_rows: BTreeSet<u32>,
    direct_animated_dirty_rows: BTreeSet<u32>,
    direct_promotion_resident_rows: BTreeSet<u32>,
    direct_animated_resident_rows: BTreeSet<u32>,
    direct_base_row_refs: BTreeMap<u32, u32>,
    direct_promotion_row_refs: BTreeMap<u32, u32>,
    direct_animated_row_refs: BTreeMap<u32, u32>,
    direct_required: bool,
    direct_compose_required: bool,
    direct_animation_descriptor_indices: Vec<u32>,
    indirect_was_active: bool,
    last_indirect_mask: LightTermMask,
    direct_was_active: bool,
    last_direct_mask: LightTermMask,
    generation_has_reset: bool,
    indirect_compose_epoch: u64,
    direct_compose_epoch: u64,
    gpu: Option<StreamingGpuPools>,
}

impl ShResidencyState {
    /// Compose the streamed direct atlas after promotion assignment. Pass A
    /// covers id-35 + id-41 and Pass B covers id-35 + id-41 + id-45. A
    /// successful two-pass encode advances the direct epoch; promotion happens
    /// at the following drain, never while the frame can still observe writes.
    #[allow(clippy::too_many_arguments)]

    fn grid_dimensions(&self) -> [u32; 3] {
        self.grid_dimensions
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

    fn missing_owner(&self, cluster_id: u32) -> bool {
        self.owner_dependencies
            .get(cluster_id as usize)
            .is_some_and(|owners| owners.iter().any(|owner| !self.sampleable.contains(owner)))
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
        let Some(state) = full.sh_streaming.as_mut() else {
            return Ok(ShDrainOutcome {
                dropped: batch
                    .ready
                    .into_iter()
                    .map(|ready| ready.chunk.cluster_id)
                    .collect(),
                ..ShDrainOutcome::default()
            });
        };
        state.drain(
            device,
            queue,
            &mut full.sh_volume_resources,
            &full.uniform_bind_group_layout,
            &full.promoted_static_weight_buffer,
            batch,
        )
    }

    pub fn sh_residency_snapshot(&self) -> Option<ShResidencySnapshot> {
        self.full()
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

/// Derive an initial sparse backing floor from id-49 ownership, not from the
/// complete legacy CSR body.  Each owner total represents the exact worst
/// case for one ready cluster (multiple rows included), so a normal cluster
/// cannot stall after its first row merely because its second row needs a
/// separate allocation.  Later residency pressure grows the pool
/// append-preservingly; this floor is deliberately bounded and never turns
/// the streamed path into a whole-resident copy.
fn sparse_capacity_floors(
    sources: &postretro_level_loader::ShStreamSourceMetadata,
    sparse_row_owner: &BTreeMap<(u32, u32), u32>,
) -> Result<SparseCapacityFloors, ShResidencyDrainError> {
    let mut by_owner = BTreeMap::<(u32, u32), (u32, u32)>::new();
    for (&(section_id, row), &owner) in sparse_row_owner {
        let source = match section_id {
            INDIRECT_DELTA_ID => sources.indirect_delta.as_ref(),
            DIRECT_DELTA_ID => sources.direct_delta.as_ref(),
            ANIMATED_DIRECT_DELTA_ID => sources.animated_direct_delta.as_ref(),
            _ => None,
        }
        .ok_or(ShResidencyDrainError::MalformedChunk {
            cluster_id: owner,
            reason: "id-49 names a sparse source omitted by stream metadata",
        })?;
        let index = usize::try_from(row).map_err(|_| ShResidencyDrainError::SlotOverflow)?;
        let start =
            *source
                .affinity_offsets
                .get(index)
                .ok_or(ShResidencyDrainError::MalformedChunk {
                    cluster_id: owner,
                    reason: "id-49 sparse row lacks a CSR start",
                })?;
        let end = *source.affinity_offsets.get(index + 1).ok_or(
            ShResidencyDrainError::MalformedChunk {
                cluster_id: owner,
                reason: "id-49 sparse row lacks a CSR end",
            },
        )?;
        let level = Level::from_u8(*source.cell_levels.get(index).ok_or(
            ShResidencyDrainError::MalformedChunk {
                cluster_id: owner,
                reason: "id-49 sparse row lacks its reconstruction level",
            },
        )?)
        .ok_or(ShResidencyDrainError::MalformedChunk {
            cluster_id: owner,
            reason: "id-49 sparse row has an invalid reconstruction level",
        })?;
        let mask =
            *source
                .valid_probe_masks
                .get(index)
                .ok_or(ShResidencyDrainError::MalformedChunk {
                    cluster_id: owner,
                    reason: "id-49 sparse row lacks its valid-probe mask",
                })?;
        let entries = end
            .checked_sub(start)
            .ok_or(ShResidencyDrainError::MalformedChunk {
                cluster_id: owner,
                reason: "streamed sparse CSR offsets are not monotonic",
            })?;
        let per_entry = u32::try_from(
            stored_delta_tiles(level, mask)
                .checked_mul(delta_probe_f16_stride(source.tile_dimension))
                .ok_or(ShResidencyDrainError::SlotOverflow)?,
        )
        .map_err(|_| ShResidencyDrainError::SlotOverflow)?;
        let tiles = entries
            .checked_mul(per_entry)
            .ok_or(ShResidencyDrainError::SlotOverflow)?;
        let total = by_owner.entry((section_id, owner)).or_insert((0, 0));
        total.0 = total
            .0
            .checked_add(entries)
            .ok_or(ShResidencyDrainError::SlotOverflow)?;
        total.1 = total
            .1
            .checked_add(tiles)
            .ok_or(ShResidencyDrainError::SlotOverflow)?;
    }

    let mut floors = SparseCapacityFloors::new();
    for ((section_id, _), (entries, tiles)) in by_owner {
        let floor = floors.entry(section_id).or_insert((1, 2));
        floor.0 = floor.0.max(
            entries
                .checked_add(1)
                .ok_or(ShResidencyDrainError::SlotOverflow)?,
        );
        let aligned_tiles = tiles
            .checked_add(tiles & 1)
            .and_then(|count| count.checked_add(2))
            .ok_or(ShResidencyDrainError::SlotOverflow)?;
        floor.1 = floor.1.max(aligned_tiles);
    }

    Ok(floors)
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
