// Reflection-probe tile capture: trace a full-sphere octahedral colour and distance tile from a capture point.
// See: context/lib/resource_management.md §5.2 · build_pipeline.md §PRL section IDs (id 54)

use glam::Vec3;
use postretro_level_format::lightmap::f32_to_f16_bits;
use postretro_level_format::octahedral::irradiance_tile_source_texel;
use postretro_level_format::reflection_probes::{
    REFLECTION_PROBE_TILE_BORDER, REFLECTION_PROBE_TILE_DIMENSION,
};

use super::shading::{CaptureScene, ShadeScratch};
use super::share::{ShareSums, sample_solid_angle};
use crate::bc6h::encode_bc6h_rgb_from_f32_rgba;

/// Interior texels per tile edge: the tile minus its wrap border.
pub(crate) const TILE_INTERIOR: u32 =
    REFLECTION_PROBE_TILE_DIMENSION - 2 * REFLECTION_PROBE_TILE_BORDER;

/// Rotated-grid supersample offsets within an interior texel, in texels from
/// its centre, on the octahedral square. Four rays per texel resolve the
/// near-horizontal edges water reflects most (a wall's base, a pool's coping)
/// better than an axis-aligned 2×2 at the same cost; the pattern is fixed, so
/// the capture is deterministic.
pub(crate) const SUPERSAMPLE_OFFSETS: [[f32; 2]; 4] = [
    [-0.125, -0.375],
    [0.375, -0.125],
    [0.125, 0.375],
    [-0.375, 0.125],
];

/// Angular width one supersample stands for: the sphere's 4π sr over every
/// interior sample, as a square's side. It sizes each ray's texture footprint.
pub(crate) fn sample_angle() -> f32 {
    let samples = (TILE_INTERIOR * TILE_INTERIOR) as f32 * SUPERSAMPLE_OFFSETS.len() as f32;
    (4.0 * std::f32::consts::PI / samples).sqrt()
}

/// Largest finite f16. A stored inverse distance clamps to it, so a hit
/// within 1.5e-5 m of the capture point never encodes as infinity, which the
/// section decoder refuses.
const F16_MAX: f32 = 65504.0;
/// Smallest positive f16 (2⁻²⁴, subnormal). A texel whose mean inverse
/// distance is positive stores at least this, so no finite hit, however far
/// or however few of its supersamples hit, rounds to the 0 a miss stores: a
/// later sky term fills exactly the directions the capture saw nothing.
pub(crate) const F16_MIN_POSITIVE: f32 = 5.960_464_5e-8;

/// One interior row of a tile: each texel's mean captured radiance and mean
/// inverse L1 distance over its supersamples, and the row's light-share sums.
#[derive(Debug, Clone, Default)]
pub(crate) struct RowCapture {
    pub texels: Vec<Vec3>,
    /// Per texel, the mean over its supersamples of `1 / |hit − capture
    /// point|₁`, a miss counting 0.
    pub distances: Vec<f32>,
    pub shares: ShareSums,
}

/// One supersample's stored distance quantity: `1 / |hit − capture point|₁`
/// for a hit `distance` along unit `direction` (the L1 length of `t·d` is
/// `t·|d|₁`), 0 for a miss. Inverse L1 is affine across an octahedral octant
/// on a plane, so the mean over symmetric supersamples and the runtime's
/// bilinear filter both reproduce a wall exactly.
pub(crate) fn inverse_l1_distance(distance: Option<f32>, direction: Vec3) -> f32 {
    match distance {
        Some(t) => (1.0 / (t * direction.abs().element_sum())).min(F16_MAX),
        None => 0.0,
    }
}

/// Capture one interior row. Each supersample's radiance is the static
/// result plus every animated light's unit term at its reference value
/// (`references`, by roster index), and the same sample, weighted by the
/// solid angle it stands for, feeds the share sums, which keep only the upper
/// hemisphere the water reads; the tile keeps every direction. The same ray's
/// hit distance feeds the distance tile, summed in the fixed supersample order
/// so the bytes never depend on the worker count.
pub(crate) fn capture_row(
    scene: &CaptureScene<'_>,
    capture_point: Vec3,
    row: u32,
    references: &[[f32; 3]],
) -> anyhow::Result<RowCapture> {
    let angle = sample_angle();
    let interior = TILE_INTERIOR as f32;
    let samples = SUPERSAMPLE_OFFSETS.len();
    let mut scratch = ShadeScratch::default();
    let mut out = RowCapture {
        texels: Vec::with_capacity(TILE_INTERIOR as usize),
        distances: Vec::with_capacity(TILE_INTERIOR as usize),
        shares: ShareSums::default(),
    };
    for column in 0..TILE_INTERIOR {
        let mut sum = Vec3::ZERO;
        let mut inverse_distance = 0.0;
        for [dx, dy] in SUPERSAMPLE_OFFSETS {
            let u = (column as f32 + 0.5 + dx) / interior;
            let v = (row as f32 + 0.5 + dy) / interior;
            let direction = octahedral_direction(u, v);
            let (static_radiance, distance) =
                scene.radiance(capture_point, direction, angle, &mut scratch)?;
            inverse_distance += inverse_l1_distance(distance, direction);
            let animated = scratch.animated.lights();
            let capture = animated
                .iter()
                .fold(static_radiance, |acc, &(light, unit)| {
                    acc + unit
                        * Vec3::from(references.get(light as usize).copied().unwrap_or([0.0; 3]))
                });
            out.shares.add(
                direction,
                sample_solid_angle(u, v, TILE_INTERIOR, samples),
                capture,
                animated,
                references,
            );
            sum += capture;
        }
        out.texels.push(sum / samples as f32);
        out.distances.push(inverse_distance / samples as f32);
    }
    Ok(out)
}

/// Assemble interior rows into the bordered tile and encode it BC6H. Border
/// texels copy their interior source by the SH atlas's wrap rule.
pub(crate) fn encode_tile(rows: &[&[Vec3]]) -> Vec<u8> {
    let dimension = REFLECTION_PROBE_TILE_DIMENSION;
    encode_bc6h_rgb_from_f32_rgba(&bordered_rgba(rows), dimension, dimension)
}

/// Assemble interior distance rows into the bordered distance tile as
/// little-endian f16, by the colour tile's wrap rule.
pub(crate) fn encode_distance_tile(rows: &[&[f32]]) -> Vec<u8> {
    bordered(rows)
        .into_iter()
        .flat_map(|w| distance_bits(w).to_le_bytes())
        .collect()
}

/// One texel's stored f16: 0 stays the miss, and any positive mean keeps a
/// non-zero encoding ([`F16_MIN_POSITIVE`]).
pub(crate) fn distance_bits(w: f32) -> u16 {
    if w > 0.0 {
        f32_to_f16_bits(w.max(F16_MIN_POSITIVE))
    } else {
        0
    }
}

/// The tile as row-major RGBA f32, border included.
fn bordered_rgba(rows: &[&[Vec3]]) -> Vec<f32> {
    bordered(rows)
        .into_iter()
        .flat_map(|texel| [texel.x, texel.y, texel.z, 1.0])
        .collect()
}

/// Every tile texel, row-major, border included: each copies its interior
/// source by the SH atlas's wrap rule.
fn bordered<T: Copy>(rows: &[&[T]]) -> Vec<T> {
    let dimension = REFLECTION_PROBE_TILE_DIMENSION;
    let mut tile = Vec::with_capacity((dimension * dimension) as usize);
    for y in 0..dimension {
        for x in 0..dimension {
            let [ix, iy] =
                irradiance_tile_source_texel(x, y, dimension, REFLECTION_PROBE_TILE_BORDER);
            tile.push(rows[iy as usize][ix as usize]);
        }
    }
    tile
}

/// Unit direction at octahedral `(u, v)` in `[0, 1]²`: the inverse of
/// `octahedral::encode_unquantized_uv`, the SH atlas's mapping, evaluated off
/// texel centres so supersamples fall inside their texel.
pub(crate) fn octahedral_direction(u: f32, v: f32) -> Vec3 {
    let (ox, oy) = (u * 2.0 - 1.0, v * 2.0 - 1.0);
    let z = 1.0 - ox.abs() - oy.abs();
    let (x, y) = if z < 0.0 {
        (
            (1.0 - oy.abs()) * sign_not_zero(ox),
            (1.0 - ox.abs()) * sign_not_zero(oy),
        )
    } else {
        (ox, oy)
    };
    Vec3::new(x, y, z).normalize()
}

fn sign_not_zero(v: f32) -> f32 {
    if v >= 0.0 { 1.0 } else { -1.0 }
}

#[cfg(test)]
mod tests {
    use super::*;
    use postretro_level_format::octahedral::{
        encode_unquantized_uv, irradiance_interior_texel_direction,
    };

    /// The capture's texel directions are the SH atlas's: at each texel centre
    /// the inverse mapping returns `irradiance_interior_texel_direction`, and
    /// every supersample stays inside its own texel.
    #[test]
    fn capture_directions_follow_the_sh_atlas_mapping() {
        let interior = TILE_INTERIOR as f32;
        for (x, y) in [(0, 0), (5, 77), (62, 63), (63, 62), (125, 125), (100, 3)] {
            let expected = Vec3::from(irradiance_interior_texel_direction(
                x,
                y,
                REFLECTION_PROBE_TILE_DIMENSION,
                REFLECTION_PROBE_TILE_BORDER,
            ));
            let centre =
                octahedral_direction((x as f32 + 0.5) / interior, (y as f32 + 0.5) / interior);
            assert!((centre - expected).length() < 1e-5, "({x}, {y})");
            for [dx, dy] in SUPERSAMPLE_OFFSETS {
                let d = octahedral_direction(
                    (x as f32 + 0.5 + dx) / interior,
                    (y as f32 + 0.5 + dy) / interior,
                );
                let uv = encode_unquantized_uv(d.x, d.y, d.z);
                assert_eq!((uv[0] * interior) as u32, x, "({x}, {y}) sample {dx},{dy}");
                assert_eq!((uv[1] * interior) as u32, y, "({x}, {y}) sample {dx},{dy}");
            }
        }
    }

    #[test]
    fn tile_borders_copy_their_wrapped_interior_sources() {
        let rows: Vec<Vec<Vec3>> = (0..TILE_INTERIOR)
            .map(|y| {
                (0..TILE_INTERIOR)
                    .map(|x| Vec3::new(x as f32, y as f32, 1.0))
                    .collect()
            })
            .collect();
        let rows: Vec<&[Vec3]> = rows.iter().map(Vec::as_slice).collect();
        let rgba = bordered_rgba(&rows);
        let dimension = REFLECTION_PROBE_TILE_DIMENSION;
        let at = |x: u32, y: u32| {
            let i = ((y * dimension + x) * 4) as usize;
            [rgba[i], rgba[i + 1]]
        };
        // Interior texel (ix, iy) sits at (ix + 1, iy + 1).
        assert_eq!(at(1, 1), [0.0, 0.0]);
        assert_eq!(at(126, 126), [125.0, 125.0]);
        // An edge border mirrors its own edge; a corner takes the diagonal.
        assert_eq!(at(0, 10), [0.0, 116.0]);
        assert_eq!(at(127, 10), [125.0, 116.0]);
        assert_eq!(at(10, 0), [116.0, 0.0]);
        assert_eq!(at(0, 0), [125.0, 125.0]);
        assert_eq!(rgba.len(), (dimension * dimension * 4) as usize);
        assert_eq!(
            encode_tile(&rows).len(),
            (dimension / 4 * dimension / 4 * 16) as usize
        );

        // The distance tile borders by the same rule, as little-endian f16.
        let distance_rows: Vec<Vec<f32>> = (0..TILE_INTERIOR)
            .map(|y| (0..TILE_INTERIOR).map(|x| (y * 200 + x) as f32).collect())
            .collect();
        let distance_rows: Vec<&[f32]> = distance_rows.iter().map(Vec::as_slice).collect();
        let bytes = encode_distance_tile(&distance_rows);
        assert_eq!(bytes.len(), (dimension * dimension * 2) as usize);
        let at = |x: u32, y: u32| {
            let i = ((y * dimension + x) * 2) as usize;
            u16::from_le_bytes([bytes[i], bytes[i + 1]])
        };
        let f16 = |value: f32| f32_to_f16_bits(value);
        assert_eq!(at(1, 1), f16(0.0));
        assert_eq!(at(0, 10), f16(116.0 * 200.0));
        assert_eq!(at(0, 3), f16(123.0 * 200.0));
        assert_eq!(at(10, 0), f16(116.0));
        assert_eq!(at(0, 0), at(126, 126));
    }

    #[test]
    fn inverse_l1_distance_is_the_reciprocal_l1_length_of_the_hit_offset() {
        let direction = Vec3::new(0.6, -0.8, 0.0);
        // Hit 2 m along: offset (1.2, -1.6, 0), L1 2.8.
        let w = inverse_l1_distance(Some(2.0), direction);
        assert!((w - 1.0 / 2.8).abs() < 1e-6, "{w}");
        assert_eq!(inverse_l1_distance(None, direction), 0.0);
        // A hit on the capture point stays finite in f16.
        assert_eq!(inverse_l1_distance(Some(0.0), direction), 65504.0);
    }
}
