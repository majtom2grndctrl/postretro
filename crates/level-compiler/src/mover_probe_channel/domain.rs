// The affected-probe rule: a posed probe's bounce counts as changed only past
// its own id-34 tile's BC6H round-trip error and the perceptual floor.
// See: context/lib/build_pipeline.md §Compiler pipeline step 12 (Mover probe channel)

use postretro_level_format::indirect_display::{DEFAULT_INDIRECT_SCALE, soft_knee_tonemap};
use postretro_level_format::octahedral::irradiance_array_tile_location;
use postretro_level_format::sh_volume::{OctahedralAtlasTexel, OctahedralShVolumeSection};

use crate::bc6h::{decode_bc6h_block, encode_bc6h_rgb_from_f32_rgba, f16_bits_to_f32};

/// BC6H encodes 4×4 blocks; a tile pads to the next multiple of 4.
const BLOCK: usize = 4;

/// The byte offset of texel `(x, y)` of compact slot `slot`'s tile in the
/// RGBA16F intermediate the SH base bake hands on.
fn texel_offset(section: &OctahedralShVolumeSection, slot: usize, x: usize, y: usize) -> usize {
    let [layer, tile_x, tile_y] =
        irradiance_array_tile_location(slot, section.tiles_per_layer, section.atlas_tiles_per_row);
    let dim = section.tile_dimension as usize;
    let width = section.atlas_dimensions[0] as usize;
    let layer_texels = width * section.atlas_dimensions[1] as usize;
    let texel = layer as usize * layer_texels
        + (tile_y as usize * dim + y) * width
        + tile_x as usize * dim
        + x;
    texel * size_of::<OctahedralAtlasTexel>()
}

/// Compact slot `slot`'s whole tile, border included, row-major.
pub(crate) fn compact_tile(
    section: &OctahedralShVolumeSection,
    slot: usize,
) -> Vec<OctahedralAtlasTexel> {
    let dim = section.tile_dimension as usize;
    let mut tile = Vec::with_capacity(dim * dim);
    for y in 0..dim {
        for x in 0..dim {
            let at = texel_offset(section, slot, x, y);
            let bytes = &section.compact_atlas[at..at + 8];
            tile.push(OctahedralAtlasTexel {
                rgba: std::array::from_fn(|c| u16::from_le_bytes([bytes[2 * c], bytes[2 * c + 1]])),
            });
        }
    }
    tile
}

/// Write `tile`'s RGB over compact slot `slot`, keeping the stored alpha:
/// bounce tiles carry none.
pub(crate) fn write_compact_rgb(
    section: &mut OctahedralShVolumeSection,
    slot: usize,
    tile: &[OctahedralAtlasTexel],
) {
    let dim = section.tile_dimension as usize;
    for y in 0..dim {
        for x in 0..dim {
            let at = texel_offset(section, slot, x, y);
            for (c, bits) in tile[y * dim + x].rgba[..3].iter().enumerate() {
                section.compact_atlas[at + 2 * c..at + 2 * c + 2]
                    .copy_from_slice(&bits.to_le_bytes());
            }
        }
    }
}

/// The largest absolute difference between two tiles over every texel and
/// RGB channel, as the f16 values they store.
pub(crate) fn max_abs_difference(a: &[OctahedralAtlasTexel], b: &[OctahedralAtlasTexel]) -> f32 {
    a.iter()
        .zip(b)
        .flat_map(|(a, b)| {
            (0..3).map(move |c| (f16_bits_to_f32(a.rgba[c]) - f16_bits_to_f32(b.rgba[c])).abs())
        })
        .fold(0.0, f32::max)
}

/// The perceptual floor (owner, Stop 2, 2026-11-08): a probe's bounce is
/// affected only when its change also exceeds this many display levels
/// ([`display_levels`]). The largest floor that keeps every `mover-closet`
/// probe the closed door darkens, whose smallest change is 2.30 levels.
pub(crate) const BOUNCE_FLOOR_LEVELS: f32 = 2.0;

/// The receiver the perceptual measure lights: mid-grey.
const DISPLAY_ALBEDO: f32 = 0.5;

/// Linear to sRGB-encoded, 0..=1, as an `Rgba8UnormSrgb` target writes it.
fn srgb_encode(linear: f32) -> f32 {
    let x = linear.clamp(0.0, 1.0);
    if x <= 0.003_130_8 {
        12.92 * x
    } else {
        1.055 * x.powf(1.0 / 2.4) - 0.055
    }
}

/// One texel as the display shows it: a mid-grey receiver lit by its RGB
/// alone through the renderer's indirect scale and soft-knee tonemap, in
/// 8-bit sRGB levels.
fn display_rgb(texel: &OctahedralAtlasTexel) -> [f32; 3] {
    let scale = DISPLAY_ALBEDO * DEFAULT_INDIRECT_SCALE;
    let lit = [0, 1, 2].map(|c| scale * f16_bits_to_f32(texel.rgba[c]));
    soft_knee_tonemap(lit).map(|channel| 255.0 * srgb_encode(channel))
}

/// How far apart two irradiance tiles read on a display: the largest change,
/// over texels and RGB channels, in 8-bit sRGB levels of a mid-grey receiver
/// lit by each tile's texel alone through the renderer's indirect scale and
/// tonemap (`indirect_display`). Indirect only, so it never hides a change
/// behind direct light; local and deterministic like the round-trip bar.
pub(crate) fn display_levels(a: &[OctahedralAtlasTexel], b: &[OctahedralAtlasTexel]) -> f32 {
    a.iter()
        .zip(b)
        .flat_map(|(a, b)| {
            let (a, b) = (display_rgb(a), display_rgb(b));
            (0..3).map(move |c| (a[c] - b[c]).abs())
        })
        .fold(0.0, f32::max)
}

/// A tile's own round-trip error: BC6H-encoded alone and decoded, the
/// largest absolute error over its texels and RGB channels. The tile pads
/// to whole blocks by repeating its last row and column, so the padding
/// widens no block's endpoints. Independent of atlas placement and
/// neighbours, so local and deterministic. Zero under
/// `--uncompressed-irradiance`, where id 34 stores the f16 tile itself.
pub(crate) fn round_trip_error(
    tile: &[OctahedralAtlasTexel],
    dim: usize,
    uncompressed: bool,
) -> f32 {
    if uncompressed {
        return 0.0;
    }
    let padded = dim.div_ceil(BLOCK) * BLOCK;
    let mut rgba = vec![0.0f32; padded * padded * 4];
    for y in 0..padded {
        for x in 0..padded {
            let texel = &tile[y.min(dim - 1) * dim + x.min(dim - 1)];
            for c in 0..3 {
                rgba[(y * padded + x) * 4 + c] = f16_bits_to_f32(texel.rgba[c]);
            }
            rgba[(y * padded + x) * 4 + 3] = 1.0;
        }
    }
    let blocks = encode_bc6h_rgb_from_f32_rgba(&rgba, padded as u32, padded as u32);
    let per_row = padded / BLOCK;
    let mut error = 0.0f32;
    for (index, block) in blocks.as_chunks::<16>().0.iter().enumerate() {
        let Ok(decoded) = decode_bc6h_block(block) else {
            return f32::INFINITY;
        };
        let (bx, by) = (index % per_row * BLOCK, index / per_row * BLOCK);
        for (local, rgb) in decoded.iter().enumerate() {
            let (x, y) = (bx + local % BLOCK, by + local / BLOCK);
            if x >= dim || y >= dim {
                continue;
            }
            for (c, value) in rgb.iter().enumerate() {
                let stored = f16_bits_to_f32(tile[y * dim + x].rgba[c]);
                error = error.max((value - stored).abs());
            }
        }
    }
    error
}

#[cfg(test)]
mod tests {
    use super::*;
    use postretro_level_format::lightmap::f32_to_f16_bits;

    fn tile(f: impl Fn(usize) -> f32) -> Vec<OctahedralAtlasTexel> {
        (0..36)
            .map(|i| OctahedralAtlasTexel {
                rgba: [
                    f32_to_f16_bits(f(i)),
                    f32_to_f16_bits(f(i) * 0.5),
                    f32_to_f16_bits(f(i) * 0.25),
                    f32_to_f16_bits(1.0),
                ],
            })
            .collect()
    }

    #[test]
    fn a_flat_tile_round_trips_near_exactly_and_uncompressed_is_exact() {
        let flat = tile(|_| 0.0);
        assert_eq!(round_trip_error(&flat, 6, false), 0.0);
        // A smooth tile, as SH irradiance is: small but nonzero error.
        let ramp = tile(|i| 1.0 + i as f32 * 0.01);
        let error = round_trip_error(&ramp, 6, false);
        assert!(error > 0.0 && error < 0.05, "ramp round-trip error {error}");
        assert_eq!(round_trip_error(&ramp, 6, true), 0.0);
    }

    #[test]
    fn display_levels_are_zero_for_equal_tiles_and_larger_for_darker_ones() {
        let a = tile(|_| 0.1);
        assert_eq!(display_levels(&a, &a), 0.0);
        // The same absolute change reads more levels on a darker tile, as
        // sRGB is steeper near black.
        let dark = display_levels(&tile(|_| 0.02), &tile(|_| 0.03));
        let bright = display_levels(&tile(|_| 1.0), &tile(|_| 1.01));
        assert!(dark > bright && bright > 0.0, "dark {dark} bright {bright}");
    }

    // The measure reads the renderer's own display constants: its indirect
    // weight from the shared constant, and the shader's knee literal pinned
    // to the shared knee.
    #[test]
    fn the_display_measure_reads_the_renderers_constants() {
        let types = include_str!("../../../renderer/src/render/renderer_types.rs");
        let definition = types
            .split("pub const DEFAULT_INDIRECT_SCALE: f32 =")
            .nth(1)
            .and_then(|rest| rest.split(';').next())
            .expect("renderer_types.rs defines DEFAULT_INDIRECT_SCALE");
        assert_eq!(
            definition.trim(),
            "postretro_level_format::indirect_display::DEFAULT_INDIRECT_SCALE",
            "the renderer's indirect weight is the shared constant"
        );
        let shader = include_str!("../../../renderer/src/shaders/screen_effects.wgsl");
        let knee: f32 = shader
            .lines()
            .find_map(|line| line.trim().strip_prefix("let knee_start = "))
            .and_then(|rhs| rhs.trim_end_matches(';').trim().parse().ok())
            .expect("screen_effects.wgsl's soft knee names its start");
        assert_eq!(
            knee,
            postretro_level_format::indirect_display::TONEMAP_KNEE_START
        );
    }

    #[test]
    fn max_abs_difference_reads_every_rgb_channel() {
        let a = tile(|_| 1.0);
        let mut b = a.clone();
        b[35].rgba[2] = f32_to_f16_bits(0.75);
        assert_eq!(max_abs_difference(&a, &b), 0.5);
        assert_eq!(max_abs_difference(&a, &a), 0.0);
    }
}
