// Lightmap section encode: BC6H/RGBA16F irradiance, reduced Rg8 direction, layer-major assembly.
// See: context/lib/build_pipeline.md §PRL section IDs

use glam::Vec3;
use postretro_level_format::lightmap::{
    DIRECTION_FORMAT_OCT_RG8, IRRADIANCE_FORMAT_BC6H, IRRADIANCE_FORMAT_RGBA16F, LightmapMode,
    LightmapSection, encode_direction_oct, f32_to_f16_bits,
};

use super::{CompositedAtlas, MIN_ATLAS_DIMENSION};
use crate::bc6h;

impl CompositedAtlas {
    /// Encode this atlas into a [`LightmapSection`] via the shared BC6H (or
    /// uncompressed-debug) irradiance path. The single section-22 encoder, so a
    /// composited atlas and a monolithically-baked one emit byte-identical
    /// sections when their buffers are equal.
    pub fn encode_section(
        &self,
        texel_density: f32,
        uncompressed_irradiance: bool,
        direction_texel_scale: u32,
    ) -> LightmapSection {
        let (irr_bytes, irradiance_format) = if uncompressed_irradiance {
            // RGBA16F is already flat layer-major (`w·h·8` per layer concatenated),
            // so the whole buffer encodes in one pass.
            (
                encode_irradiance_rgba16f(&self.irradiance),
                IRRADIANCE_FORMAT_RGBA16F,
            )
        } else {
            // The BC6H encoder is single-image — it asserts its input length is
            // exactly `w·h·4` floats — so it must run once per layer over that
            // layer's slice; the per-layer block blobs concatenate into the
            // layer-major irradiance blob.
            let plane = (self.atlas_width * self.atlas_height) as usize;
            let mut blob = Vec::new();
            for layer in 0..self.layer_count as usize {
                let start = layer * plane * 4;
                let end = start + plane * 4;
                blob.extend_from_slice(&bc6h::encode_bc6h_rgb_from_f32_rgba(
                    &self.irradiance[start..end],
                    self.atlas_width,
                    self.atlas_height,
                ));
            }
            (blob, IRRADIANCE_FORMAT_BC6H)
        };
        // Direction is a lower-frequency signal than irradiance. Reduce it only
        // after the warm/cold byte-identity seam: both paths retain this full
        // resolution CompositedAtlas and arrive here with identical buffers.
        let direction_texel_scale = effective_direction_texel_scale(
            direction_texel_scale,
            self.atlas_width,
            self.atlas_height,
        );
        let (dir_width, dir_height, dir_bytes) = if direction_texel_scale == 1 {
            // Keep the full-resolution composited atlas intact; only the
            // on-wire octahedral encoding changes to its two used channels.
            (
                self.atlas_width,
                self.atlas_height,
                encode_direction_rg8(&self.direction, &self.coverage),
            )
        } else {
            let (direction, coverage) = reduce_direction_atlas(
                &self.direction,
                &self.coverage,
                self.atlas_width,
                self.atlas_height,
                self.layer_count,
                direction_texel_scale,
            );
            (
                self.atlas_width / direction_texel_scale,
                self.atlas_height / direction_texel_scale,
                encode_direction_rg8(&direction, &coverage),
            )
        };
        LightmapSection {
            layer_count: self.layer_count,
            irr_width: self.atlas_width,
            irr_height: self.atlas_height,
            irr_texel_density: texel_density,
            irradiance: irr_bytes,
            irradiance_format,
            dir_width,
            dir_height,
            dir_texel_density: texel_density * direction_texel_scale as f32,
            direction: dir_bytes,
            direction_format: DIRECTION_FORMAT_OCT_RG8,
            mode: LightmapMode::Shadowed,
        }
    }
}

/// Assemble the stable layer-major section payload after callers have encoded
/// each atlas plane. The cold bake and warm incremental compositor share this
/// seam so their final layer ordering and wire-format choices stay identical.
#[allow(clippy::too_many_arguments)]
pub(crate) fn assemble_layered_section(
    atlas_w: u32,
    atlas_h: u32,
    layer_count: u32,
    texel_density: f32,
    uncompressed_irradiance: bool,
    direction_texel_scale: u32,
    irradiance: Vec<u8>,
    direction: Vec<u8>,
) -> LightmapSection {
    let direction_texel_scale =
        effective_direction_texel_scale(direction_texel_scale, atlas_w, atlas_h);
    LightmapSection {
        layer_count,
        irr_width: atlas_w,
        irr_height: atlas_h,
        irr_texel_density: texel_density,
        irradiance,
        irradiance_format: if uncompressed_irradiance {
            IRRADIANCE_FORMAT_RGBA16F
        } else {
            IRRADIANCE_FORMAT_BC6H
        },
        dir_width: atlas_w / direction_texel_scale,
        dir_height: atlas_h / direction_texel_scale,
        dir_texel_density: texel_density * direction_texel_scale as f32,
        direction,
        direction_format: DIRECTION_FORMAT_OCT_RG8,
        mode: LightmapMode::Shadowed,
    }
}

/// Encode one already-dilated composited atlas plane.
///
/// The whole-atlas encoder applies these same operations independently to each
/// plane. Keeping this helper plane-only makes the cold path's append order
/// explicit while preserving BC6H blocks, direction reduction, and Rg8 output.
pub(crate) fn encode_atlas_layer(
    atlas: &CompositedAtlas,
    uncompressed_irradiance: bool,
    direction_texel_scale: u32,
) -> (Vec<u8>, Vec<u8>) {
    debug_assert_eq!(atlas.layer_count, 1);

    let irradiance = if uncompressed_irradiance {
        encode_irradiance_rgba16f(&atlas.irradiance)
    } else {
        bc6h::encode_bc6h_rgb_from_f32_rgba(
            &atlas.irradiance,
            atlas.atlas_width,
            atlas.atlas_height,
        )
    };

    let direction = if direction_texel_scale == 1 {
        encode_direction_rg8(&atlas.direction, &atlas.coverage)
    } else {
        let (direction, coverage) = reduce_direction_atlas(
            &atlas.direction,
            &atlas.coverage,
            atlas.atlas_width,
            atlas.atlas_height,
            1,
            direction_texel_scale,
        );
        encode_direction_rg8(&direction, &coverage)
    };

    (irradiance, direction)
}

fn encode_irradiance_rgba16f(data: &[f32]) -> Vec<u8> {
    let texel_count = data.len() / 4;
    let mut out = Vec::with_capacity(texel_count * 8);
    for t in 0..texel_count {
        let r = f32_to_f16_bits(data[t * 4]);
        let g = f32_to_f16_bits(data[t * 4 + 1]);
        let b = f32_to_f16_bits(data[t * 4 + 2]);
        let a = f32_to_f16_bits(data[t * 4 + 3]);
        out.extend_from_slice(&r.to_le_bytes());
        out.extend_from_slice(&g.to_le_bytes());
        out.extend_from_slice(&b.to_le_bytes());
        out.extend_from_slice(&a.to_le_bytes());
    }
    out
}

/// Normalize a programmatic direction scale to a supported power of two.
///
/// The CLI rejects malformed values. This defensive path keeps direct callers
/// from producing a zero-sized direction atlas if they construct a
/// `LightmapConfig` themselves.
fn normalized_direction_texel_scale(scale: u32) -> u32 {
    let bounded = scale.clamp(1, MIN_ATLAS_DIMENSION);
    if bounded.is_power_of_two() {
        bounded
    } else {
        bounded.next_power_of_two()
    }
}

/// Clamp the configured scale to dimensions that this particular atlas can
/// represent. Baked atlas axes are power-of-two and at least 64, but keeping
/// this guard here also makes synthetic/direct callers safe.
pub(crate) fn effective_direction_texel_scale(
    scale: u32,
    atlas_width: u32,
    atlas_height: u32,
) -> u32 {
    let mut effective = normalized_direction_texel_scale(scale);
    while effective > atlas_width
        || effective > atlas_height
        || atlas_width % effective != 0
        || atlas_height % effective != 0
    {
        effective /= 2;
    }
    effective.max(1)
}

/// Reduce full-resolution dominant directions for the static direction atlas.
///
/// Each output layer reads only its matching input plane. Within a block the
/// loop is fixed row-major, coverage is ORed, and covered cancelling vectors
/// resolve to neutral up rather than normalizing zero.
pub(super) fn reduce_direction_atlas(
    direction: &[Vec3],
    coverage: &[bool],
    atlas_width: u32,
    atlas_height: u32,
    layer_count: u32,
    factor: u32,
) -> (Vec<Vec3>, Vec<bool>) {
    debug_assert!(factor.is_power_of_two());
    debug_assert!(factor > 0);
    debug_assert_eq!(atlas_width % factor, 0);
    debug_assert_eq!(atlas_height % factor, 0);

    let reduced_width = atlas_width / factor;
    let reduced_height = atlas_height / factor;
    let source_plane = atlas_width as usize * atlas_height as usize;
    let reduced_plane = reduced_width as usize * reduced_height as usize;
    let mut reduced_direction = Vec::with_capacity(reduced_plane * layer_count as usize);
    let mut reduced_coverage = Vec::with_capacity(reduced_plane * layer_count as usize);

    for layer in 0..layer_count as usize {
        let layer_start = layer * source_plane;
        for y in 0..reduced_height {
            for x in 0..reduced_width {
                let mut sum = Vec3::ZERO;
                let mut covered = false;
                for block_y in 0..factor {
                    for block_x in 0..factor {
                        let source_x = x * factor + block_x;
                        let source_y = y * factor + block_y;
                        let index = layer_start
                            + source_y as usize * atlas_width as usize
                            + source_x as usize;
                        if coverage[index] {
                            covered = true;
                            sum += direction[index];
                        }
                    }
                }
                reduced_coverage.push(covered);
                reduced_direction.push(if covered && sum.length_squared() > 1.0e-8 {
                    sum.normalize()
                } else {
                    Vec3::Y
                });
            }
        }
    }

    (reduced_direction, reduced_coverage)
}

pub(super) fn encode_direction_rg8(direction: &[Vec3], coverage: &[bool]) -> Vec<u8> {
    let mut out = Vec::with_capacity(direction.len() * 2);
    for (i, d) in direction.iter().enumerate() {
        let bytes = if coverage[i] {
            encode_direction_oct([d.x, d.y, d.z])
        } else {
            // Neutral up-direction: stray bilinear samples return a Lambert-valid vector.
            [128u8, 255]
        };
        out.extend_from_slice(&bytes);
    }
    out
}
