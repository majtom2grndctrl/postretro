// Reflection-probe colour mips: each probe's capture prefiltered per mip by the lobe the exponent → mip function maps to it, bordered and BC6H-encoded.
// See: context/lib/rendering_pipeline.md §7.3a (Glossy world faces) · build_pipeline.md §PRL section IDs (id 54)

use glam::{DVec3, Vec3};
use postretro_level_format::reflection_probes::{
    REFLECTION_PROBE_TILE_BORDER, REFLECTION_PROBE_TILE_DIMENSION,
    reflection_probe_colour_mip_dimension, reflection_probe_full_colour_mip_count,
    reflection_probe_mip_exponent,
};

use super::capture::{TILE_INTERIOR, bordered_rgba_at, encode_tile, octahedral_direction};
use super::placement::PlacedProbe;
use crate::bc6h::encode_bc6h_rgb_from_f32_rgba;

/// Kernel weight, relative to its peak, below which a source texel adds
/// nothing: the lobe is cut at the angle where it falls to this, and blocks
/// lying wholly past that angle are skipped. The cut tail changes no mip by
/// more than this share of its value.
pub(super) const PREFILTER_CUTOFF: f32 = 1.0e-4;
/// Source texels per block edge for the cull.
const PREFILTER_BLOCK: u32 = 6;
/// Intervals of each mip's kernel table over cos φ, from the cutoff to 1.
/// Linear interpolation in it stays within about 2e-5 of the lobe's peak at
/// mip 1, the narrowest.
const PREFILTER_TABLE_STEPS: usize = 4096;
/// Bumped when the prefilter's output for the same capture changes; keyed
/// into the stage cache for sections that carry mips.
pub(super) const PREFILTER_VERSION: u32 = 1;

/// Colour mips the section carries: the full chain to one BC6H block when
/// any probe binds a world face, else mip 0 alone. Water reads mip 0 only,
/// so a section binding only water keeps exactly the bytes it had.
pub(super) fn colour_mip_count(probes: &[PlacedProbe]) -> u32 {
    if probes.iter().any(|probe| !probe.world_faces.is_empty()) {
        reflection_probe_full_colour_mip_count(REFLECTION_PROBE_TILE_DIMENSION)
    } else {
        1
    }
}

/// What the stage key reads of the prefilter: its version, cutoff and
/// block, and each mip's lobe exponent.
pub(super) fn key_bytes(mip_count: u32) -> Vec<u8> {
    let mut out = Vec::new();
    for value in [
        PREFILTER_VERSION,
        PREFILTER_BLOCK,
        PREFILTER_TABLE_STEPS as u32,
        mip_count,
    ] {
        out.extend_from_slice(&value.to_le_bytes());
    }
    for value in
        std::iter::once(PREFILTER_CUTOFF).chain((1..mip_count).map(reflection_probe_mip_exponent))
    {
        out.extend_from_slice(&value.to_le_bytes());
    }
    out
}

/// One probe's colour chain as id 54 stores it: mip 0 the capture exactly as
/// [`encode_tile`] writes it, then each later mip the capture prefiltered
/// ([`prefilter_mip`]) and wrapped in its own 1-texel border by the SH
/// atlas's rule, each BC6H. `rows` are the mip-0 interior rows.
pub(super) fn encode_colour_chain(rows: &[&[Vec3]], mip_count: u32) -> Vec<u8> {
    let mut out = encode_tile(rows);
    for (mip, rgba) in (1..).zip(prefiltered_mips(rows, mip_count)) {
        let dimension = mip_dimension(mip);
        out.extend(encode_bc6h_rgb_from_f32_rgba(&rgba, dimension, dimension));
    }
    out
}

/// Mips 1..`mip_count` before encoding: each [`prefilter_mip`]'s interior in
/// its own bordered tile, row-major RGBA f32.
pub(super) fn prefiltered_mips(rows: &[&[Vec3]], mip_count: u32) -> Vec<Vec<f32>> {
    if mip_count <= 1 {
        return Vec::new();
    }
    let source = PrefilterSource::new(rows);
    (1..mip_count)
        .map(|mip| {
            bordered_rgba_at(
                &rows_of(&prefilter_mip(&source, mip), mip),
                mip_dimension(mip),
            )
        })
        .collect()
}

/// Edge of colour mip `mip` of the engine tile, border included.
pub(super) fn mip_dimension(mip: u32) -> u32 {
    reflection_probe_colour_mip_dimension(REFLECTION_PROBE_TILE_DIMENSION, mip)
}

/// Interior texels per edge of colour mip `mip`.
pub(super) fn mip_interior(mip: u32) -> u32 {
    mip_dimension(mip) - 2 * REFLECTION_PROBE_TILE_BORDER
}

/// `texels` (row-major, `mip`'s interior) as rows.
pub(super) fn rows_of(texels: &[Vec3], mip: u32) -> Vec<&[Vec3]> {
    texels.chunks(mip_interior(mip) as usize).collect()
}

/// The direction at interior texel `(x, y)`'s centre of an `interior`-texel
/// edge: the capture's own mapping, so a mip texel and the mip-0 texel the
/// capture traced agree where their centres meet.
pub(super) fn texel_direction(x: u32, y: u32, interior: u32) -> Vec3 {
    let n = interior as f32;
    octahedral_direction((x as f32 + 0.5) / n, (y as f32 + 0.5) / n)
}

/// The solid angle of interior texel `(x, y)` of an `interior`-texel edge,
/// at its centre: the square's area element over |p|³ for p the texel's
/// point on the octahedron (`share::sample_solid_angle`'s method).
fn texel_solid_angle(x: u32, y: u32, interior: u32) -> f32 {
    let n = interior as f32;
    let (ox, oy) = (
        (x as f32 + 0.5) / n * 2.0 - 1.0,
        (y as f32 + 0.5) / n * 2.0 - 1.0,
    );
    let z = 1.0 - ox.abs() - oy.abs();
    let (px, py) = if z < 0.0 {
        (
            (1.0 - oy.abs()) * ox.signum(),
            (1.0 - ox.abs()) * oy.signum(),
        )
    } else {
        (ox, oy)
    };
    let length = Vec3::new(px, py, z).length();
    (2.0 / n).powi(2) / (length * length * length)
}

/// The mip-0 capture as the prefilter reads it: every interior texel's
/// direction, solid angle and radiance, grouped into square blocks, each
/// with its centre direction and angular radius.
pub(super) struct PrefilterSource {
    blocks: Vec<SourceBlock>,
}

struct SourceBlock {
    centre: Vec3,
    /// The largest angle from `centre` to a member, in radians.
    radius: f32,
    texels: Vec<SourceTexel>,
}

struct SourceTexel {
    direction: Vec3,
    solid_angle: f32,
    radiance: Vec3,
}

impl PrefilterSource {
    /// `rows`: the mip-0 interior rows, [`TILE_INTERIOR`] square.
    pub(super) fn new(rows: &[&[Vec3]]) -> Self {
        let interior = TILE_INTERIOR;
        assert_eq!(rows.len(), interior as usize, "interior rows");
        let per_edge = interior.div_ceil(PREFILTER_BLOCK);
        let mut blocks = Vec::with_capacity((per_edge * per_edge) as usize);
        for by in 0..per_edge {
            for bx in 0..per_edge {
                let xs = bx * PREFILTER_BLOCK..((bx + 1) * PREFILTER_BLOCK).min(interior);
                let ys = by * PREFILTER_BLOCK..((by + 1) * PREFILTER_BLOCK).min(interior);
                let texels: Vec<SourceTexel> = ys
                    .flat_map(|y| xs.clone().map(move |x| (x, y)))
                    .map(|(x, y)| SourceTexel {
                        direction: texel_direction(x, y, interior),
                        solid_angle: texel_solid_angle(x, y, interior),
                        radiance: rows[y as usize][x as usize],
                    })
                    .collect();
                let centre = texels
                    .iter()
                    .map(|texel| texel.direction)
                    .sum::<Vec3>()
                    .normalize();
                let radius = texels
                    .iter()
                    .map(|texel| texel.direction.dot(centre).clamp(-1.0, 1.0).acos())
                    .fold(0.0f32, f32::max);
                blocks.push(SourceBlock {
                    centre,
                    // A hair of slack for rounding in the cull's own dot.
                    radius: radius + 1.0e-3,
                    texels,
                });
            }
        }
        Self { blocks }
    }
}

/// The prefilter's lobe at angle φ from the lookup, for exponent `n`:
/// cos(φ/2)ⁿ = ((1 + cos φ) / 2)^(n/2), the Blinn-Phong lobe over the
/// reflected direction, whose half-width at half maximum is exactly the
/// θ(n) the exponent → mip function reads. For small φ it is the
/// reflected-direction Phong lobe cos^(n/4) φ, the usual Blinn → Phong
/// match, and unlike it never needs a clamp past 90°.
pub(super) fn prefilter_weight(cos_angle: f32, exponent: f32) -> f32 {
    ((1.0 + cos_angle) * 0.5).max(0.0).powf(0.5 * exponent)
}

/// Colour mip `mip`'s interior, row-major: each texel the solid-angle-
/// weighted mean of the mip-0 capture under the lobe of
/// `reflection_probe_mip_exponent(mip)` ([`PrefilterLobe`]), centred on
/// the texel's direction. Texels past the lobe's [`PREFILTER_CUTOFF`] angle
/// add nothing, and blocks lying wholly past it are skipped; the sum runs in
/// a fixed order, so the bytes never depend on the worker count. The
/// distance tile and capture-shadow weight never enter: only colour is
/// blurred.
pub(super) fn prefilter_mip(source: &PrefilterSource, mip: u32) -> Vec<Vec3> {
    let lobe = PrefilterLobe::new(reflection_probe_mip_exponent(mip));
    // Per block, the dot below which no member can reach the cutoff angle.
    let cull: Vec<f32> = source
        .blocks
        .iter()
        .map(|block| {
            let reach = lobe.cutoff + block.radius;
            if reach >= std::f32::consts::PI {
                f32::NEG_INFINITY
            } else {
                reach.cos()
            }
        })
        .collect();
    let interior = mip_interior(mip);
    let mut out = Vec::with_capacity((interior * interior) as usize);
    for y in 0..interior {
        for x in 0..interior {
            let lookup = texel_direction(x, y, interior);
            let mut sum = DVec3::ZERO;
            let mut weight = 0.0f64;
            for (block, &floor) in source.blocks.iter().zip(&cull) {
                if lookup.dot(block.centre) < floor {
                    continue;
                }
                for texel in &block.texels {
                    let w = lobe.weight(lookup.dot(texel.direction));
                    if w > 0.0 {
                        let w = f64::from(w * texel.solid_angle);
                        sum += texel.radiance.as_dvec3() * w;
                        weight += w;
                    }
                }
            }
            out.push(if weight > 0.0 {
                (sum / weight).as_vec3()
            } else {
                Vec3::ZERO
            });
        }
    }
    out
}

/// One mip's lobe, [`prefilter_weight`] cut at [`PREFILTER_CUTOFF`] and
/// tabulated over cos φ so the sum pays no `powf` per texel. The table is
/// built from the same inputs every run, so the bytes stay deterministic.
pub(super) struct PrefilterLobe {
    /// The cut angle in radians: cos(φ/2)ⁿ = ε at φ = 2·acos(ε^(1/n)).
    cutoff: f32,
    /// cos of `cutoff`, the table's first entry.
    floor: f32,
    /// Table steps per unit of cos φ.
    scale: f32,
    table: Vec<f32>,
}

impl PrefilterLobe {
    pub(super) fn new(exponent: f32) -> Self {
        let cutoff = (2.0 * PREFILTER_CUTOFF.powf(1.0 / exponent).acos()).min(std::f32::consts::PI);
        let floor = cutoff.cos();
        let span = 1.0 - floor;
        let table = (0..=PREFILTER_TABLE_STEPS)
            .map(|i| {
                let cos_angle = floor + span * i as f32 / PREFILTER_TABLE_STEPS as f32;
                prefilter_weight(cos_angle.min(1.0), exponent)
            })
            .collect();
        Self {
            cutoff,
            floor,
            scale: PREFILTER_TABLE_STEPS as f32 / span,
            table,
        }
    }

    /// The lobe at `cos_angle`, 0 past the cut.
    pub(super) fn weight(&self, cos_angle: f32) -> f32 {
        if cos_angle < self.floor {
            return 0.0;
        }
        let position = ((cos_angle - self.floor) * self.scale).min(PREFILTER_TABLE_STEPS as f32);
        let index = (position as usize).min(PREFILTER_TABLE_STEPS - 1);
        let fraction = position - index as f32;
        self.table[index] + (self.table[index + 1] - self.table[index]) * fraction
    }
}

#[cfg(test)]
#[path = "prefilter_tests.rs"]
mod tests;
