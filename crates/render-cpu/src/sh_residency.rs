// CPU half of streamed-SH residency: everything the renderer's residency
// state derives from the stream manifest and the runtime cells before any GPU
// work, and the error type both halves report.
// See: context/lib/rendering_pipeline.md §4 (Cluster SH residency)

use std::fmt;

use postretro_level_format::SectionId;

mod dense_spans;
mod node_map;
mod plan;
mod pool_bound;
mod sample_region_tables;
mod sample_regions;
mod sparse_compose;
mod stored_nodes;

pub use dense_spans::{DenseSpan, DenseSpans};
pub use node_map::NodeMap;
pub use plan::{ClusterStorage, ShResidencyPlan, plan_sh_residency, plan_sh_residency_parts};
pub use pool_bound::ShPoolBound;
pub use sample_region_tables::SampleRegionTables;
pub use sample_regions::{
    SAMPLE_REGION_DILATION_CELLS, SampleRegionGrid, ShSampleRegion, affinity_row, brick_rows_in,
    clamp_probe, region_brick_bounds, scaled_writer_row,
};
pub use sparse_compose::{
    SparseCapacities, SparseComposeLayout, SparseComposeLayouts, compose_row_capacity,
    sparse_compose_capacity, sparse_row_demand,
};
pub use stored_nodes::{
    DenseNodeLayout, StoredNode, StoredNodeLayout, affinity_index, dense_index,
    derive_dense_node_layout,
};

const INDIRECT_DELTA_ID: u32 = SectionId::DeltaShVolumes as u32;
const INDIRECT_BASE_ID: u32 = SectionId::OctahedralShVolume as u32;
const DIRECT_DELTA_ID: u32 = SectionId::DirectShDeltaVolumes as u32;
const ANIMATED_DIRECT_DELTA_ID: u32 = SectionId::AnimatedDirectShDeltaVolumes as u32;

/// A malformed renderer handoff is never repaired by inventing an address.
/// The loader has already validated the wire bytes; these errors therefore
/// catch a violated lifetime/identity contract at the renderer boundary.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ShResidencyDrainError {
    InvalidBatch(String),
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
    /// A valid probe no cluster names, so no chunk can ever install it.
    UncoveredDenseProbe {
        dense_index: u32,
    },
    SlotOverflow,
    GpuCapacity {
        reason: &'static str,
    },
    /// An install placed past the pools' physical capacity and was rolled
    /// back. The drain consumes this internal verdict: mandatory work evicts
    /// and grows, and optional work lands in `pool_refused`.
    PoolShortfall,
    /// Only a growth whose required backing exceeds the captured device
    /// limit. The drain consumes this internal verdict into `refused`.
    GpuDeviceLimit {
        reason: &'static str,
    },
    /// A growth past the whole map, which first-fit reaches only when
    /// fragmentation strands free slots. The drain consumes this internal
    /// verdict into `whole_map_refused`.
    PoolAtWholeMap {
        family: &'static str,
    },
    /// Id 49 stores no bound at the lead the runtime sizes its pools from.
    MandatoryBoundMismatch {
        expected_lead: u32,
        stored_leads: Vec<u32>,
    },
    /// Id 49's bound leaves out a streamed sparse family: corrupt data, not a
    /// capacity failure.
    MandatoryBoundMissingFamily {
        section_id: u32,
    },
    /// Id 49's bound names a family this level does not stream: corrupt data,
    /// not a capacity failure.
    MandatoryBoundExtraFamily {
        section_id: u32,
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
            Self::InvalidBatch(reason) => write!(f, "invalid SH drain batch: {reason}"),
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
            Self::UncoveredDenseProbe { dense_index } => {
                write!(f, "SH dense probe {dense_index} is named by no cluster")
            }
            Self::SlotOverflow => write!(
                f,
                "SH streamed pool address exceeds probe indirection capacity"
            ),
            Self::GpuCapacity { reason } => {
                write!(f, "SH streamed GPU pool capacity error: {reason}")
            }
            Self::PoolShortfall => write!(f, "SH streamed pools have no room for this install"),
            Self::GpuDeviceLimit { reason } => {
                write!(f, "SH streamed GPU device limit: {reason}")
            }
            Self::PoolAtWholeMap { family } => {
                write!(f, "SH streamed {family} pool is at its whole-map cap")
            }
            Self::MandatoryBoundMismatch {
                expected_lead,
                stored_leads,
            } => write!(
                f,
                "id-49 stores no SH mandatory bound at lead {expected_lead} \
                 (stored leads {stored_leads:?}); re-bake the level"
            ),
            Self::MandatoryBoundMissingFamily { section_id } => write!(
                f,
                "id-49 SH mandatory bound omits streamed sparse family id-{section_id}; \
                 re-bake the level"
            ),
            Self::MandatoryBoundExtraFamily { section_id } => write!(
                f,
                "id-49 SH mandatory bound names id-{section_id}, which this level does not \
                 stream as a sparse family; re-bake the level"
            ),
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
    /// Only a submitted replacement that is still fenced retains the ready
    /// payload for this drain's retry. Device limits consume it into a refusal;
    /// malformed shape and arithmetic guards remain fatal contract errors.
    pub const fn is_retryable_retirement_pressure(&self) -> bool {
        matches!(self, Self::GpuRetirementPressure { .. })
    }

    /// A growth no retirement wait could serve, so the cluster is refused.
    pub const fn is_growth_refusal(&self) -> bool {
        matches!(
            self,
            Self::GpuDeviceLimit { .. } | Self::PoolAtWholeMap { .. }
        )
    }
}
