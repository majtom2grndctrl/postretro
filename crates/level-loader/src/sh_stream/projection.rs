//! Retained SH metadata projections and their cross-source validation.
//! See: context/lib/build_pipeline.md §PRL Compilation.

use postretro_level_format::SectionId;
use postretro_level_format::cluster_sh_payloads::{
    ClusterShPayloadsBaseMetadata, ClusterShPayloadsSourceMetadata,
};
use postretro_level_format::direct_sh_volume::DIRECT_SH_VOLUME_VERSION;
use postretro_level_format::sh_volume::{OctahedralShProbe, SH_VOLUME_VERSION};

use super::{PrlLoadError, stream_error};
/// The validated metadata floor retained by a streaming session for id 34.
/// It intentionally excludes `compact_atlas`.
#[derive(Debug, Clone, PartialEq)]
pub struct ShStreamBaseMetadata {
    pub grid_origin: [f32; 3],
    pub cell_size: [f32; 3],
    pub grid_dimensions: [u32; 3],
    pub probe_stride: u32,
    pub tile_dimension: u32,
    pub tile_border: u32,
    pub atlas_dimensions: [u32; 2],
    pub layer_count: u32,
    pub tiles_per_layer: u32,
    pub atlas_tiles_per_row: u32,
    pub irradiance_format: u32,
    pub probes: Vec<OctahedralShProbe>,
    pub animation_descriptors: Vec<postretro_level_format::sh_volume::AnimationDescriptor>,
    pub slot_for_map_light: Vec<u32>,
}

impl ShStreamBaseMetadata {
    pub(super) fn codec_metadata(&self) -> ClusterShPayloadsBaseMetadata<'_> {
        ClusterShPayloadsBaseMetadata {
            grid_dimensions: self.grid_dimensions,
            tile_dimension: self.tile_dimension,
            tile_border: self.tile_border,
            irradiance_format: self.irradiance_format,
            probes: &self.probes,
        }
    }
}

/// Header-only projection of id 35. The atlas remains in the PRL until a
/// selected cluster is decoded from id 50.
#[derive(Debug, Clone, PartialEq)]
pub struct ShStreamDirectMetadata {
    pub grid_origin: [f32; 3],
    pub cell_size: [f32; 3],
    pub grid_dimensions: [u32; 3],
    pub tile_dimension: u32,
    pub tile_border: u32,
    pub atlas_dimensions: [u32; 2],
    pub layer_count: u32,
    pub tiles_per_layer: u32,
    pub atlas_tiles_per_row: u32,
    pub irradiance_format: u32,
}

/// Shared metadata retained for one streamed sparse source. Payload f16 tiles
/// are deliberately omitted; they live in id-50 chunks.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ShStreamSparseMetadata {
    pub section_id: u32,
    pub internal_version: u32,
    pub affinity_dims: [u32; 3],
    pub tile_dimension: u32,
    pub tile_border: u32,
    pub valid_probe_masks: Vec<u64>,
    pub cell_levels: Vec<u8>,
    pub affinity_offsets: Vec<u32>,
    pub affinity_lights: Vec<u32>,
    /// Ids 27 and 45 retain this always-resident descriptor mapping. Id 41
    /// has no map and stores an empty vector.
    pub animation_descriptor_indices: Vec<u32>,
}

/// The id-50 streamed-source projections. IDs 47/48 intentionally are not
/// represented here: they remain whole-resident companions in this slice.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct ShStreamSourceMetadata {
    pub indirect_delta: Option<ShStreamSparseMetadata>,
    pub direct: Option<ShStreamDirectMetadata>,
    pub direct_delta: Option<ShStreamSparseMetadata>,
    pub animated_direct_delta: Option<ShStreamSparseMetadata>,
}

impl ShStreamSourceMetadata {
    pub(super) fn codec_sources<'a>(
        &'a self,
        base: &'a ShStreamBaseMetadata,
    ) -> Vec<ClusterShPayloadsSourceMetadata<'a>> {
        let mut sources = Vec::with_capacity(5);
        if let Some(delta) = &self.indirect_delta {
            sources.push(delta.codec_source());
        }
        sources.push(ClusterShPayloadsSourceMetadata::Dense {
            section_id: SectionId::OctahedralShVolume as u32,
            internal_version: SH_VOLUME_VERSION,
            irradiance_format: base.irradiance_format,
        });
        if let Some(direct) = &self.direct {
            sources.push(ClusterShPayloadsSourceMetadata::Dense {
                section_id: SectionId::DirectShVolume as u32,
                internal_version: DIRECT_SH_VOLUME_VERSION,
                irradiance_format: direct.irradiance_format,
            });
        }
        if let Some(delta) = &self.direct_delta {
            sources.push(delta.codec_source());
        }
        if let Some(delta) = &self.animated_direct_delta {
            sources.push(delta.codec_source());
        }
        sources
    }
}

impl ShStreamSparseMetadata {
    pub(super) fn codec_source(&self) -> ClusterShPayloadsSourceMetadata<'_> {
        ClusterShPayloadsSourceMetadata::Sparse {
            section_id: self.section_id,
            internal_version: self.internal_version,
            affinity_dimensions: self.affinity_dims,
            tile_dimension: self.tile_dimension,
            tile_border: self.tile_border,
            valid_probe_masks: &self.valid_probe_masks,
            cell_levels: &self.cell_levels,
            affinity_offsets: &self.affinity_offsets,
            affinity_lights: &self.affinity_lights,
        }
    }
}
pub(super) fn validate_projected_sources(
    base: &ShStreamBaseMetadata,
    sources: &ShStreamSourceMetadata,
) -> Result<(), PrlLoadError> {
    if let Some(direct) = &sources.direct {
        if direct.grid_origin != base.grid_origin
            || direct.cell_size != base.cell_size
            || direct.grid_dimensions != base.grid_dimensions
            || direct.tile_dimension != base.tile_dimension
            || direct.tile_border != base.tile_border
            || direct.atlas_dimensions != base.atlas_dimensions
            || direct.layer_count != base.layer_count
            || direct.tiles_per_layer != base.tiles_per_layer
            || direct.atlas_tiles_per_row != base.atlas_tiles_per_row
            || direct.irradiance_format != base.irradiance_format
        {
            return Err(stream_error(
                "id-35 metadata does not exactly match id-34 streamed metadata",
            ));
        }
    }
    for sparse in [
        sources.indirect_delta.as_ref(),
        sources.direct_delta.as_ref(),
        sources.animated_direct_delta.as_ref(),
    ]
    .into_iter()
    .flatten()
    {
        let expected_dims = base.grid_dimensions.map(|dimension| dimension.div_ceil(4));
        if sparse.affinity_dims != expected_dims
            || sparse.tile_dimension != base.tile_dimension
            || sparse.tile_border != base.tile_border
        {
            return Err(stream_error(format!(
                "streamed sparse section {} metadata does not match id-34",
                sparse.section_id
            )));
        }
        for (cell, &mask) in sparse.valid_probe_masks.iter().enumerate() {
            if mask != valid_probe_mask_for_affinity_cell(base, sparse.affinity_dims, cell)? {
                return Err(stream_error(format!(
                    "streamed sparse section {} valid probe mask {cell} disagrees with id-34",
                    sparse.section_id
                )));
            }
        }
        if sparse.animation_descriptor_indices.iter().any(|&index| {
            index != postretro_level_format::sh_volume::ANIMATED_SLOT_NONE
                && usize::try_from(index)
                    .map_or(true, |index| index >= base.animation_descriptors.len())
        }) {
            return Err(stream_error(format!(
                "streamed sparse section {} names an out-of-range animation descriptor",
                sparse.section_id
            )));
        }
    }
    Ok(())
}

fn valid_probe_mask_for_affinity_cell(
    base: &ShStreamBaseMetadata,
    affinity_dims: [u32; 3],
    cell_index: usize,
) -> Result<u64, PrlLoadError> {
    let cell_index =
        u32::try_from(cell_index).map_err(|_| stream_error("affinity cell index exceeds u32"))?;
    let xy = affinity_dims[0]
        .checked_mul(affinity_dims[1])
        .ok_or_else(|| stream_error("affinity dimensions overflow"))?;
    if xy == 0 || affinity_dims.contains(&0) {
        return Err(stream_error("affinity dimensions contain zero"));
    }
    let cell_x = cell_index % affinity_dims[0];
    let cell_y = (cell_index / affinity_dims[0]) % affinity_dims[1];
    let cell_z = cell_index / xy;
    let mut mask = 0u64;
    for local_z in 0..4u32 {
        for local_y in 0..4u32 {
            for local_x in 0..4u32 {
                let probe_x = cell_x * 4 + local_x;
                let probe_y = cell_y * 4 + local_y;
                let probe_z = cell_z * 4 + local_z;
                if probe_x >= base.grid_dimensions[0]
                    || probe_y >= base.grid_dimensions[1]
                    || probe_z >= base.grid_dimensions[2]
                {
                    continue;
                }
                let probe = usize::try_from(probe_x)
                    .ok()
                    .and_then(|x| {
                        usize::try_from(probe_y).ok().and_then(|y| {
                            usize::try_from(probe_z).ok().and_then(|z| {
                                usize::try_from(base.grid_dimensions[0])
                                    .ok()
                                    .and_then(|width| {
                                        usize::try_from(base.grid_dimensions[1]).ok().and_then(
                                            |height| {
                                                x.checked_add(y.checked_mul(width)?).and_then(
                                                    |offset| {
                                                        offset.checked_add(z.checked_mul(
                                                            width.checked_mul(height)?,
                                                        )?)
                                                    },
                                                )
                                            },
                                        )
                                    })
                            })
                        })
                    })
                    .ok_or_else(|| stream_error("probe index overflows usize"))?;
                if base.probes[probe].validity != 0 {
                    let local = local_x + local_y * 4 + local_z * 16;
                    mask |= 1u64 << local;
                }
            }
        }
    }
    Ok(mask)
}
