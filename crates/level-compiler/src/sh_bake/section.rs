// SH volume section assembly: compact valid-probe atlas, BC6H encode, stats, f16 decode.
// See: context/lib/build_pipeline.md §Compiler pipeline

use postretro_level_format::lightmap::{IRRADIANCE_FORMAT_BC6H, IRRADIANCE_FORMAT_RGBA16F};
use postretro_level_format::octahedral::{
    IrradianceAtlasArrayLayout, MAX_SH_ATLAS_DIMENSION, MAX_SH_ATLAS_LAYERS,
    irradiance_array_tile_location, irradiance_atlas_array_layout,
};
use postretro_level_format::sh_volume::{OctahedralAtlasTexel, OctahedralShVolumeSection};
use thiserror::Error;

#[derive(Debug, Error)]
pub(crate) enum ShAtlasBakeError {
    #[error(
        "SH atlas layer overflow: packing probe grid {grid_dimensions:?} needs more than \
         {max_layers} array layers at max per-layer dimension {max_dim}; raise `probe_spacing` \
         or split the map"
    )]
    LayerOverflow {
        grid_dimensions: [u32; 3],
        max_dim: u32,
        max_layers: u32,
    },
}

pub fn log_stats(section: &OctahedralShVolumeSection) {
    let dims = section.grid_dimensions;
    let total = section.total_probes();
    let valid = section.probes.iter().filter(|p| p.validity == 1).count();
    let invalid = total - valid;

    // Coarse depth-moment aggregate over valid probes: mean and max E[d].
    // Decoded from the stored f16 bits — diagnostic only, so f16 precision is
    // fine and avoids threading the pre-rounding f32 moments through the bake.
    let mut sum_mean_d = 0.0f64;
    let mut max_mean_d = 0.0f32;
    for probe in section.probes.iter().filter(|p| p.validity == 1) {
        let mean_d = f16_bits_to_f32(probe.mean_distance);
        sum_mean_d += mean_d as f64;
        max_mean_d = max_mean_d.max(mean_d);
    }
    let avg_mean_d = if valid > 0 {
        (sum_mean_d / valid as f64) as f32
    } else {
        0.0
    };

    log::info!(
        "OctahedralShVolume: grid {}x{}x{} = {total} probes ({valid} valid, \
         {invalid} invalid), tile {} (border {}), atlas {}x{}x{} ({} tile(s)/row, {} tile(s)/layer), cell {}m, \
         depth E[d] mean {avg_mean_d:.2}m / max {max_mean_d:.2}m, \
         {} animated light(s)",
        dims[0],
        dims[1],
        dims[2],
        section.tile_dimension,
        section.tile_border,
        section.atlas_dimensions[0],
        section.atlas_dimensions[1],
        section.layer_count,
        section.atlas_tiles_per_row,
        section.tiles_per_layer,
        section.cell_size[0],
        section.animation_descriptors.len(),
    );
}

/// Convert a dense, layer-major tile atlas from a bake into the lossless
/// valid-probe-order intermediate. The source's tile locations retain the dense
/// grid layout, while the destination assigns slots only to metadata-valid probes
/// in x-fastest order.
///
/// The intermediate deliberately remains `Rgba16Float` here. The pipeline's
/// downstream density pack emits the v11 node-aware stored set after hierarchy
/// classification and grouped and monolithic assembly converge, so the group cache
/// never stores an output-format-specific value.
pub(crate) fn compact_section_from_dense_atlas(
    mut section: OctahedralShVolumeSection,
    dense_atlas: &[OctahedralAtlasTexel],
) -> OctahedralShVolumeSection {
    let dense_texel_count = section.layer_count as usize
        * section.atlas_dimensions[0] as usize
        * section.atlas_dimensions[1] as usize;
    debug_assert_eq!(dense_atlas.len(), dense_texel_count);

    let valid_probe_count = section
        .probes
        .iter()
        .filter(|probe| probe.validity != 0)
        .count();
    let compact_layout = sh_atlas_array_layout(
        [
            u32::try_from(valid_probe_count).expect("SH compact valid-probe count must fit in u32"),
            1,
            1,
        ],
        section.tile_dimension,
    )
    .unwrap_or_else(|e| panic!("compact SH atlas layout failed: {e}"));

    let compact_texel_count = compact_layout.layer_count as usize
        * compact_layout.atlas_width as usize
        * compact_layout.atlas_height as usize;
    let mut compact_atlas = vec![OctahedralAtlasTexel::default(); compact_texel_count];
    let dense_width = section.atlas_dimensions[0] as usize;
    let dense_layer_texels = dense_width * section.atlas_dimensions[1] as usize;
    let compact_width = compact_layout.atlas_width as usize;
    let compact_layer_texels = compact_width * compact_layout.atlas_height as usize;
    let tile_dimension = section.tile_dimension as usize;
    let mut compact_slot = 0usize;

    for (probe_index, probe) in section.probes.iter().enumerate() {
        if probe.validity == 0 {
            continue;
        }

        let [dense_layer, dense_tile_x, dense_tile_y] = irradiance_array_tile_location(
            probe_index,
            section.tiles_per_layer,
            section.atlas_tiles_per_row,
        );
        let [compact_layer, compact_tile_x, compact_tile_y] = irradiance_array_tile_location(
            compact_slot,
            compact_layout.tiles_per_layer,
            compact_layout.atlas_tiles_per_row,
        );
        let dense_origin = [
            dense_tile_x as usize * tile_dimension,
            dense_tile_y as usize * tile_dimension,
        ];
        let compact_origin = [
            compact_tile_x as usize * tile_dimension,
            compact_tile_y as usize * tile_dimension,
        ];
        let dense_layer_offset = dense_layer as usize * dense_layer_texels;
        let compact_layer_offset = compact_layer as usize * compact_layer_texels;

        for tile_y in 0..tile_dimension {
            let dense_row = dense_layer_offset + (dense_origin[1] + tile_y) * dense_width;
            let compact_row = compact_layer_offset + (compact_origin[1] + tile_y) * compact_width;
            let source = &dense_atlas
                [dense_row + dense_origin[0]..dense_row + dense_origin[0] + tile_dimension];
            compact_atlas
                [compact_row + compact_origin[0]..compact_row + compact_origin[0] + tile_dimension]
                .copy_from_slice(source);
        }
        compact_slot += 1;
    }
    debug_assert_eq!(compact_slot, valid_probe_count);

    section.atlas_dimensions = [compact_layout.atlas_width, compact_layout.atlas_height];
    section.atlas_tiles_per_row = compact_layout.atlas_tiles_per_row;
    section.tiles_per_layer = compact_layout.tiles_per_layer;
    section.layer_count = compact_layout.layer_count;
    section.irradiance_format = IRRADIANCE_FORMAT_RGBA16F;
    section.compact_atlas = atlas_texels_to_bytes(&compact_atlas);
    section
}

/// Re-encode the packed, lossless base atlas for its at-rest representation.
/// The stored geometry stays unchanged; only the BC6H input gains a
/// zero-filled 4×4 fringe that is never addressed by the runtime tile layout.
pub(crate) fn encode_sh_volume_section_bc6h(
    section: &OctahedralShVolumeSection,
    uncompressed_irradiance: bool,
) -> OctahedralShVolumeSection {
    if uncompressed_irradiance {
        return section.clone();
    }
    debug_assert_eq!(
        section.irradiance_format, IRRADIANCE_FORMAT_RGBA16F,
        "SH BC6H encode expects the compact RGBA16F section from the bake",
    );

    let width = section.atlas_dimensions[0];
    let height = section.atlas_dimensions[1];
    let padded_width = width.div_ceil(4) * 4;
    let padded_height = height.div_ceil(4) * 4;
    let source_layer_stride = width as usize * height as usize * 8;
    let encoded_layer_len = (padded_width / 4) as usize * (padded_height / 4) as usize * 16;
    let mut compact_atlas = Vec::with_capacity(section.layer_count as usize * encoded_layer_len);

    for layer in 0..section.layer_count as usize {
        // BC6H consumes f32 RGBA input but encodes RGB only. The valid-probe
        // metadata, not alpha, defines stored payload membership in v11.
        let mut rgba_f32 = vec![0.0f32; padded_width as usize * padded_height as usize * 4];
        let source_layer = layer * source_layer_stride;
        for y in 0..height {
            for x in 0..width {
                let src = source_layer + ((y * width + x) * 8) as usize;
                let dst = ((y * padded_width + x) * 4) as usize;
                for channel in 0..4 {
                    let bits = u16::from_le_bytes([
                        section.compact_atlas[src + channel * 2],
                        section.compact_atlas[src + channel * 2 + 1],
                    ]);
                    rgba_f32[dst + channel] = f16_bits_to_f32(bits);
                }
            }
        }
        compact_atlas.extend_from_slice(&crate::bc6h::encode_bc6h_rgb_from_f32_rgba(
            &rgba_f32,
            padded_width,
            padded_height,
        ));
    }

    OctahedralShVolumeSection {
        irradiance_format: IRRADIANCE_FORMAT_BC6H,
        compact_atlas,
        ..section.clone()
    }
}

fn atlas_texels_to_bytes(atlas_texels: &[OctahedralAtlasTexel]) -> Vec<u8> {
    let mut bytes = Vec::with_capacity(atlas_texels.len() * 8);
    for texel in atlas_texels {
        for channel in texel.rgba {
            bytes.extend_from_slice(&channel.to_le_bytes());
        }
    }
    bytes
}

pub(crate) fn sh_atlas_array_layout(
    grid_dimensions: [u32; 3],
    tile_dimension: u32,
) -> Result<IrradianceAtlasArrayLayout, ShAtlasBakeError> {
    irradiance_atlas_array_layout(grid_dimensions, tile_dimension, MAX_SH_ATLAS_DIMENSION).ok_or(
        ShAtlasBakeError::LayerOverflow {
            grid_dimensions,
            max_dim: MAX_SH_ATLAS_DIMENSION,
            max_layers: MAX_SH_ATLAS_LAYERS,
        },
    )
}

/// Decode IEEE 754 binary16 bits → f32. The inverse of
/// `lightmap::f32_to_f16_bits` for the depth moments; covers finite non-negative
/// magnitudes (the diagnostic stats aggregate and the `sh_group` warm-SH gate,
/// which decode depth moments / octahedral-tile irradiance — both finite
/// non-negative).
pub(crate) fn f16_bits_to_f32(bits: u16) -> f32 {
    let sign = (bits >> 15) & 0x1;
    let exp = (bits >> 10) & 0x1f;
    let mant = bits & 0x3ff;
    let value = if exp == 0 {
        // Subnormal: no implicit leading 1.
        (mant as f32) * 2.0f32.powi(-24)
    } else if exp == 0x1f {
        if mant == 0 { f32::INFINITY } else { f32::NAN }
    } else {
        let m = 1.0 + (mant as f32) / 1024.0;
        m * 2.0f32.powi(exp as i32 - 15)
    };
    if sign == 1 { -value } else { value }
}
