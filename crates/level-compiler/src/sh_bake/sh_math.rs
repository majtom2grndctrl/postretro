// L2 SH projection, cosine-lobe convolution, and octahedral irradiance tile packing.
// See: context/lib/rendering_pipeline.md §4

use glam::Vec3;
use postretro_level_format::lightmap::f32_to_f16_bits;
use postretro_level_format::octahedral::{
    IrradianceAtlasArrayLayout, irradiance_array_tile_location,
    irradiance_interior_texel_direction, irradiance_tile_source_texel,
};
use postretro_level_format::sh_volume::OctahedralAtlasTexel;

use super::BakedProbe;

/// Real-valued L2 SH basis — Ramamoorthi-Hanrahan convention, Condon-Shortley phase omitted.
pub(super) fn sh_basis_l2(dir: Vec3) -> [f32; 9] {
    let x = dir.x;
    let y = dir.y;
    let z = dir.z;
    let mut b = [0f32; 9];
    b[0] = 0.282_094_8; // 0.5 * sqrt(1/pi)
    b[1] = -0.488_602_5 * y;
    b[2] = 0.488_602_5 * z;
    b[3] = -0.488_602_5 * x;
    b[4] = 1.092_548_4 * x * y;
    b[5] = -1.092_548_4 * y * z;
    b[6] = 0.315_391_6 * (3.0 * z * z - 1.0);
    b[7] = -1.092_548_4 * x * z;
    b[8] = 0.546_274_2 * (x * x - y * y);
    b
}

pub(super) fn accumulate_sh_rgb(acc: &mut [f32; 27], dir: Vec3, value: Vec3, weight: f32) {
    let basis = sh_basis_l2(dir);
    for (band, b) in basis.iter().enumerate() {
        let base = band * 3;
        acc[base] += *b * value.x * weight;
        acc[base + 1] += *b * value.y * weight;
        acc[base + 2] += *b * value.z * weight;
    }
}

pub(super) fn evaluate_sh_rgb(coefficients: &[f32; 27], dir: Vec3) -> Vec3 {
    let basis = sh_basis_l2(dir);
    let mut out = Vec3::ZERO;
    for (band, b) in basis.iter().enumerate() {
        let base = band * 3;
        out.x += coefficients[base] * *b;
        out.y += coefficients[base + 1] * *b;
        out.z += coefficients[base + 2] * *b;
    }
    out.max(Vec3::ZERO)
}

pub(super) fn pack_octahedral_irradiance_atlas(
    probes: &[BakedProbe],
    grid_dimensions: [u32; 3],
    tile_dimension: u32,
    border: u32,
    atlas_layout: IrradianceAtlasArrayLayout,
) -> Vec<OctahedralAtlasTexel> {
    let total = (grid_dimensions[0] as usize)
        * (grid_dimensions[1] as usize)
        * (grid_dimensions[2] as usize);
    debug_assert_eq!(probes.len(), total);
    let atlas_width = atlas_layout.atlas_width as usize;
    let layer_texel_count = atlas_width * atlas_layout.atlas_height as usize;
    let atlas_texel_count = atlas_layout.layer_count as usize * layer_texel_count;
    let mut atlas = vec![OctahedralAtlasTexel::default(); atlas_texel_count];
    if total == 0 {
        return atlas;
    }

    for (probe_index, probe) in probes.iter().enumerate() {
        let [layer, tile_x, tile_y] = irradiance_array_tile_location(
            probe_index,
            atlas_layout.tiles_per_layer,
            atlas_layout.atlas_tiles_per_row,
        );
        let origin = [tile_x * tile_dimension, tile_y * tile_dimension];
        let layer_offset = layer as usize * layer_texel_count;
        let tile = pack_octahedral_irradiance_tile(
            &probe.coefficients,
            probe.metadata.validity != 0,
            tile_dimension,
            border,
        );

        for tile_y in 0..tile_dimension {
            for tile_x in 0..tile_dimension {
                let texel = tile[(tile_y * tile_dimension + tile_x) as usize];
                let atlas_x = origin[0] + tile_x;
                let atlas_y = origin[1] + tile_y;
                let atlas_off = layer_offset + atlas_y as usize * atlas_width + atlas_x as usize;
                atlas[atlas_off] = texel;
            }
        }
    }
    atlas
}

pub(crate) fn pack_octahedral_irradiance_tile(
    coefficients: &[f32; 27],
    valid: bool,
    tile_dimension: u32,
    border: u32,
) -> Vec<OctahedralAtlasTexel> {
    let interior = tile_dimension - 2 * border;
    let mut interior_texels = vec![OctahedralAtlasTexel::default(); (interior * interior) as usize];

    if valid {
        let valid_alpha = f32_to_f16_bits(1.0);
        for iy in 0..interior {
            for ix in 0..interior {
                let dir = Vec3::from(irradiance_interior_texel_direction(
                    ix,
                    iy,
                    tile_dimension,
                    border,
                ));
                let irradiance = evaluate_sh_rgb(coefficients, dir);
                let off = (iy * interior + ix) as usize;
                interior_texels[off] = OctahedralAtlasTexel {
                    rgba: [
                        f32_to_f16_bits(irradiance.x),
                        f32_to_f16_bits(irradiance.y),
                        f32_to_f16_bits(irradiance.z),
                        valid_alpha,
                    ],
                };
            }
        }
    }

    let mut tile =
        vec![OctahedralAtlasTexel::default(); (tile_dimension * tile_dimension) as usize];
    for tile_y in 0..tile_dimension {
        for tile_x in 0..tile_dimension {
            let [src_x, src_y] =
                irradiance_tile_source_texel(tile_x, tile_y, tile_dimension, border);
            tile[(tile_y * tile_dimension + tile_x) as usize] =
                interior_texels[(src_y * interior + src_x) as usize];
        }
    }
    tile
}

// Ramamoorthi & Hanrahan 2001, eq. 11 — zonal-harmonic cosine-lobe factors.
// After convolution the coefficients reconstruct irradiance directly; the runtime shader
// needs no per-fragment A_l multiply.
const COSINE_LOBE_L0: f32 = std::f32::consts::PI;
const COSINE_LOBE_L1: f32 = 2.0 * std::f32::consts::PI / 3.0;
const COSINE_LOBE_L2: f32 = std::f32::consts::PI * 0.25;

pub(super) fn apply_cosine_lobe_rgb(acc: &mut [f32; 27]) {
    for band in 0..9 {
        let factor = cosine_lobe_factor(band);
        let base = band * 3;
        acc[base] *= factor;
        acc[base + 1] *= factor;
        acc[base + 2] *= factor;
    }
}

pub(super) fn cosine_lobe_factor(band: usize) -> f32 {
    match band {
        0 => COSINE_LOBE_L0,
        1..=3 => COSINE_LOBE_L1,
        4..=8 => COSINE_LOBE_L2,
        _ => 0.0,
    }
}
