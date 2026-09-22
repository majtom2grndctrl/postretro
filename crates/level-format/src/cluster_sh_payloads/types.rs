//! Id-50 public constants, wire types, input projections, and errors.
//! See: context/plans/in-progress/sh-probe-streaming--cluster-residency/index.md

use super::*;

pub const CLUSTER_SH_PAYLOADS_VERSION: u32 = 1;
pub const CLUSTER_SH_PAYLOADS_CONTAINER_VERSION: u16 = 1;
pub const CLUSTER_SH_PAYLOADS_HEADER_SIZE: usize = 72;
pub const CLUSTER_SH_PAYLOADS_SOURCE_RECORD_SIZE: usize = 16;
pub const CLUSTER_SH_PAYLOADS_INDEX_RECORD_SIZE: usize = 80;
pub const CLUSTER_SH_CHUNK_HEADER_SIZE: usize = 16;
pub const CLUSTER_SH_BLOCK_RECORD_SIZE: usize = 32;

pub const CLUSTER_SH_LOGICAL_TILE_DIMENSION: u32 = DEFAULT_IRRADIANCE_TILE_DIMENSION;
pub const CLUSTER_SH_LOGICAL_TILE_BORDER: u32 = DEFAULT_IRRADIANCE_TILE_BORDER;
pub const CLUSTER_SH_PHYSICAL_TILE_STRIDE: u32 = 8;

/// The format-owned layout of an independently streamable atlas.  The
/// compiler uses this same helper in its cap-agreement assertion; no caller
/// may derive a different physical-cell layout for id 50.
pub fn cluster_sh_isolated_atlas_array_layout(
    slot_count: u32,
) -> Option<IrradianceAtlasArrayLayout> {
    irradiance_atlas_array_layout(
        [slot_count, 1, 1],
        CLUSTER_SH_PHYSICAL_TILE_STRIDE,
        MAX_SH_ATLAS_DIMENSION,
    )
}

pub(super) const SOURCE_KIND_DENSE_BASE_ATLAS: u32 = 0;
pub(super) const SOURCE_KIND_SPARSE_AFFINITY: u32 = 1;
pub(super) const BLOCK_KIND_PROBE_PATCHES: u32 = 0;
pub(super) const BLOCK_KIND_ISOLATED_ATLAS: u32 = 1;
pub(super) const BLOCK_KIND_SPARSE_ROWS: u32 = 3;

pub(super) const PROBE_INDIRECTION_LEVEL_MASK: u32 = 0x0000_0003;
pub(super) const PROBE_INDIRECTION_VALID_BIT: u32 = 0x0000_0004;
pub(super) const PROBE_INDIRECTION_SCALE_SHIFT: u32 = 3;
pub(super) const PROBE_INDIRECTION_SLOT_SHIFT: u32 = 5;
pub(super) const PROBE_INDIRECTION_MAX_SLOT: u32 = u32::MAX >> PROBE_INDIRECTION_SLOT_SHIFT;

#[derive(Debug, Error, PartialEq, Eq)]
pub enum ClusterShPayloadsError {
    #[error("ClusterShPayloadsVersionMismatch: version {version}, expected {expected}")]
    VersionMismatch { version: u32, expected: u32 },
    #[error("ClusterShPayloadsInvalidData: {0}")]
    InvalidData(String),
    #[error("ClusterShPayloadsSourceMismatch: {0}")]
    SourceMismatch(String),
    #[error("ClusterShPayloadsRangeOutOfBounds: {0}")]
    RangeOutOfBounds(String),
    #[error("ClusterShPayloadsSizeOverflow: {0}")]
    SizeOverflow(&'static str),
    #[error("ClusterShPayloadsHashMismatch: cluster {cluster_id}")]
    HashMismatch { cluster_id: u32 },
    #[error("ClusterShPayloadsAllocationFailed: {0}")]
    AllocationFailed(&'static str),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ClusterShPayloadsHeader {
    pub cluster_count: u32,
    pub source_count: u32,
    pub grid_dimensions: [u32; 3],
    pub affinity_dimensions: [u32; 3],
    pub payload_bytes: u64,
}

impl ClusterShPayloadsHeader {
    /// Return the exact number of prefix bytes a positional reader must fetch
    /// before it can validate the index. This never includes payload bytes.
    pub fn metadata_len(&self) -> Result<usize, ClusterShPayloadsError> {
        let sources = u64::from(self.source_count)
            .checked_mul(CLUSTER_SH_PAYLOADS_SOURCE_RECORD_SIZE as u64)
            .ok_or(ClusterShPayloadsError::SizeOverflow("source table length"))?;
        let index = u64::from(self.cluster_count)
            .checked_mul(CLUSTER_SH_PAYLOADS_INDEX_RECORD_SIZE as u64)
            .ok_or(ClusterShPayloadsError::SizeOverflow("cluster index length"))?;
        let total = (CLUSTER_SH_PAYLOADS_HEADER_SIZE as u64)
            .checked_add(sources)
            .and_then(|value| value.checked_add(index))
            .ok_or(ClusterShPayloadsError::SizeOverflow("metadata length"))?;
        usize::try_from(total)
            .map_err(|_| ClusterShPayloadsError::SizeOverflow("metadata length exceeds usize"))
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u32)]
pub enum ClusterShPayloadsSourceKind {
    DenseBaseAtlas = SOURCE_KIND_DENSE_BASE_ATLAS,
    SparseAffinity = SOURCE_KIND_SPARSE_AFFINITY,
}

impl ClusterShPayloadsSourceKind {
    pub(super) fn parse(value: u32) -> Result<Self, ClusterShPayloadsError> {
        match value {
            SOURCE_KIND_DENSE_BASE_ATLAS => Ok(Self::DenseBaseAtlas),
            SOURCE_KIND_SPARSE_AFFINITY => Ok(Self::SparseAffinity),
            _ => invalid(format!("source kind {value} is unknown")),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ClusterShPayloadsSourceRecord {
    pub section_id: u32,
    pub internal_version: u32,
    pub kind: ClusterShPayloadsSourceKind,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClusterShPayloadsIndexRecord {
    pub payload_offset: u64,
    pub payload_len: u64,
    pub decoded_bytes: u64,
    pub requested_resident_bytes: u64,
    pub stored_tile_count: u32,
    pub dense_patch_count: u32,
    pub affinity_patch_count: u32,
    pub hash: [u8; 32],
}

/// Validated id-50 prefix. It deliberately owns only header/source/index data;
/// callers retain the payload in the PRL file and read selected chunks later.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClusterShPayloadsSection {
    pub header: ClusterShPayloadsHeader,
    pub sources: Vec<ClusterShPayloadsSourceRecord>,
    pub index: Vec<ClusterShPayloadsIndexRecord>,
}

#[derive(Debug, Clone, Copy)]
pub struct ClusterShPayloadsBaseMetadata<'a> {
    pub grid_dimensions: [u32; 3],
    pub tile_dimension: u32,
    pub tile_border: u32,
    pub irradiance_format: u32,
    pub probes: &'a [OctahedralShProbe],
}

impl<'a> From<&'a OctahedralShVolumeSection> for ClusterShPayloadsBaseMetadata<'a> {
    fn from(section: &'a OctahedralShVolumeSection) -> Self {
        Self {
            grid_dimensions: section.grid_dimensions,
            tile_dimension: section.tile_dimension,
            tile_border: section.tile_border,
            irradiance_format: section.irradiance_format,
            probes: &section.probes,
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub enum ClusterShPayloadsSourceMetadata<'a> {
    Dense {
        section_id: u32,
        internal_version: u32,
        irradiance_format: u32,
    },
    Sparse {
        section_id: u32,
        internal_version: u32,
        affinity_dimensions: [u32; 3],
        tile_dimension: u32,
        tile_border: u32,
        valid_probe_masks: &'a [u64],
        cell_levels: &'a [u8],
        affinity_offsets: &'a [u32],
        affinity_lights: &'a [u32],
    },
}

impl ClusterShPayloadsSourceMetadata<'_> {
    pub fn section_id(self) -> u32 {
        match self {
            Self::Dense { section_id, .. } | Self::Sparse { section_id, .. } => section_id,
        }
    }

    pub fn internal_version(self) -> u32 {
        match self {
            Self::Dense {
                internal_version, ..
            }
            | Self::Sparse {
                internal_version, ..
            } => internal_version,
        }
    }

    pub fn kind(self) -> ClusterShPayloadsSourceKind {
        match self {
            Self::Dense { .. } => ClusterShPayloadsSourceKind::DenseBaseAtlas,
            Self::Sparse { .. } => ClusterShPayloadsSourceKind::SparseAffinity,
        }
    }
}

/// The metadata-only inputs shared by compiler publication and loader startup.
/// No field needs an atlas or delta body, so the streaming loader can construct
/// these from its projections without materializing the legacy payloads.
#[derive(Debug, Clone, Copy)]
pub struct ClusterShPayloadsValidationInputs<'a> {
    pub directory: &'a ClusterDirectorySection,
    pub base: ClusterShPayloadsBaseMetadata<'a>,
    /// Exactly the present streaming sources: 27/34/35/41/45. The base id 34
    /// dense source is required; ids 47/48 are never valid here.
    pub sources: &'a [ClusterShPayloadsSourceMetadata<'a>],
}

/// Build the canonical streaming inventory from legacy parsed sections. The
/// compiler can use it now; the later streaming loader constructs equivalent
/// rows from metadata projections.
pub fn source_metadata_from_sections<'a>(
    base: &'a OctahedralShVolumeSection,
    direct: Option<&'a DirectShVolumeSection>,
    delta: Option<&'a DeltaShVolumesSection>,
    direct_delta: Option<&'a DirectShDeltaVolumesSection>,
    animated_direct_delta: Option<&'a AnimatedDirectShDeltaVolumesSection>,
) -> Vec<ClusterShPayloadsSourceMetadata<'a>> {
    let mut sources = Vec::with_capacity(5);
    if let Some(delta) = delta {
        sources.push(sparse_source(
            SectionId::DeltaShVolumes as u32,
            u32::from(DELTA_SH_VOLUMES_VERSION),
            delta.affinity_dims,
            delta.tile_dimension,
            delta.tile_border,
            &delta.valid_probe_masks,
            &delta.cell_levels,
            &delta.affinity_offsets,
            &delta.affinity_lights,
        ));
    }
    sources.push(ClusterShPayloadsSourceMetadata::Dense {
        section_id: SectionId::OctahedralShVolume as u32,
        internal_version: SH_VOLUME_VERSION,
        irradiance_format: base.irradiance_format,
    });
    if let Some(direct) = direct {
        sources.push(ClusterShPayloadsSourceMetadata::Dense {
            section_id: SectionId::DirectShVolume as u32,
            internal_version: DIRECT_SH_VOLUME_VERSION,
            irradiance_format: direct.irradiance_format,
        });
    }
    if let Some(delta) = direct_delta {
        sources.push(sparse_source(
            SectionId::DirectShDeltaVolumes as u32,
            u32::from(DIRECT_SH_DELTA_VOLUMES_VERSION),
            delta.affinity_dims,
            delta.tile_dimension,
            delta.tile_border,
            &delta.valid_probe_masks,
            &delta.cell_levels,
            &delta.affinity_offsets,
            &delta.affinity_lights,
        ));
    }
    if let Some(delta) = animated_direct_delta {
        sources.push(sparse_source(
            SectionId::AnimatedDirectShDeltaVolumes as u32,
            u32::from(ANIMATED_DIRECT_SH_DELTA_VOLUMES_VERSION),
            delta.affinity_dims,
            delta.tile_dimension,
            delta.tile_border,
            &delta.valid_probe_masks,
            &delta.cell_levels,
            &delta.affinity_offsets,
            &delta.affinity_lights,
        ));
    }
    sources
}

fn sparse_source<'a>(
    section_id: u32,
    internal_version: u32,
    affinity_dimensions: [u32; 3],
    tile_dimension: u32,
    tile_border: u32,
    valid_probe_masks: &'a [u64],
    cell_levels: &'a [u8],
    affinity_offsets: &'a [u32],
    affinity_lights: &'a [u32],
) -> ClusterShPayloadsSourceMetadata<'a> {
    ClusterShPayloadsSourceMetadata::Sparse {
        section_id,
        internal_version,
        affinity_dimensions,
        tile_dimension,
        tile_border,
        valid_probe_masks,
        cell_levels,
        affinity_offsets,
        affinity_lights,
    }
}
