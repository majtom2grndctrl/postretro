//! Cluster-major id-50 SH streaming codec and shared metadata validator.
//! See: context/lib/build_pipeline.md §PRL section IDs.
//!
//! Wire ownership, semantic validation, worker chunk decoding, and address
//! formulas live in focused children so startup parsing stays visibly separate
//! from any payload access.

use std::collections::{BTreeMap, BTreeSet};

use thiserror::Error;

use crate::{
    SectionId,
    animated_direct_sh_delta_volumes::{
        ANIMATED_DIRECT_SH_DELTA_VOLUMES_VERSION, AnimatedDirectShDeltaVolumesSection,
    },
    cluster_directory::{ClusterDirectorySection, ClusterRangeRole, ClusterResourceDomain},
    delta_sh_volumes::{DELTA_SH_VOLUMES_VERSION, DeltaShVolumesSection, delta_probe_f16_stride},
    direct_sh_delta_volumes::{DIRECT_SH_DELTA_VOLUMES_VERSION, DirectShDeltaVolumesSection},
    direct_sh_volume::{DIRECT_SH_VOLUME_VERSION, DirectShVolumeSection},
    lightmap::{IRRADIANCE_FORMAT_BC6H, IRRADIANCE_FORMAT_RGBA16F},
    octahedral::{
        DEFAULT_IRRADIANCE_TILE_BORDER, DEFAULT_IRRADIANCE_TILE_DIMENSION,
        IrradianceAtlasArrayLayout, MAX_SH_ATLAS_DIMENSION, irradiance_atlas_array_layout,
    },
    sh_reconstruct::{Level, StoredBrickPrefixSum, stored_delta_tiles},
    sh_volume::{
        OctahedralShProbe, OctahedralShVolumeSection, SH_VOLUME_VERSION, validate_probe_metadata,
        validate_storage_levels_against_delta,
    },
};

mod codec;
mod decode;
mod layout;
mod semantic;
mod source_validation;
mod types;
mod wire;

pub use codec::{
    DecodedClusterShBlock, DecodedClusterShPayload, ValidatedClusterShPayloadsSection,
};
pub use types::{
    CLUSTER_SH_BLOCK_RECORD_SIZE, CLUSTER_SH_CHUNK_HEADER_SIZE, CLUSTER_SH_LOGICAL_TILE_BORDER,
    CLUSTER_SH_LOGICAL_TILE_DIMENSION, CLUSTER_SH_PAYLOADS_CONTAINER_VERSION,
    CLUSTER_SH_PAYLOADS_HEADER_SIZE, CLUSTER_SH_PAYLOADS_INDEX_RECORD_SIZE,
    CLUSTER_SH_PAYLOADS_SOURCE_RECORD_SIZE, CLUSTER_SH_PAYLOADS_VERSION,
    CLUSTER_SH_PHYSICAL_TILE_STRIDE, ClusterShPayloadsBaseMetadata, ClusterShPayloadsError,
    ClusterShPayloadsHeader, ClusterShPayloadsIndexRecord, ClusterShPayloadsSection,
    ClusterShPayloadsSourceKind, ClusterShPayloadsSourceMetadata, ClusterShPayloadsSourceRecord,
    ClusterShPayloadsValidationInputs, cluster_sh_isolated_atlas_array_layout,
    source_metadata_from_sections,
};

use decode::*;
use layout::*;
use semantic::*;
use source_validation::*;
use types::{
    BLOCK_KIND_ISOLATED_ATLAS, BLOCK_KIND_PROBE_PATCHES, BLOCK_KIND_SPARSE_ROWS,
    PROBE_INDIRECTION_LEVEL_MASK, PROBE_INDIRECTION_MAX_SLOT, PROBE_INDIRECTION_SCALE_SHIFT,
    PROBE_INDIRECTION_SLOT_SHIFT, PROBE_INDIRECTION_VALID_BIT,
};
use wire::*;

#[cfg(test)]
mod tests;
