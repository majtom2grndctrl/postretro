// Reflection-probe tile capture: trace a full-sphere octahedral tile from a capture point.
// See: context/lib/resource_management.md §5.2 · build_pipeline.md §PRL section IDs (id 54)

use glam::Vec3;
use postretro_level_format::octahedral::irradiance_tile_source_texel;
use postretro_level_format::reflection_probes::{
    REFLECTION_PROBE_TILE_BORDER, REFLECTION_PROBE_TILE_DIMENSION,
};

use super::shading::CaptureScene;
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

/// One interior row of a tile: the mean radiance of each texel's supersamples.
pub(crate) fn capture_row(
    scene: &CaptureScene<'_>,
    capture_point: Vec3,
    row: u32,
) -> anyhow::Result<Vec<Vec3>> {
    let angle = sample_angle();
    let interior = TILE_INTERIOR as f32;
    let mut out = Vec::with_capacity(TILE_INTERIOR as usize);
    for column in 0..TILE_INTERIOR {
        let mut sum = Vec3::ZERO;
        for [dx, dy] in SUPERSAMPLE_OFFSETS {
            let u = (column as f32 + 0.5 + dx) / interior;
            let v = (row as f32 + 0.5 + dy) / interior;
            let direction = octahedral_direction(u, v);
            sum += scene.radiance(capture_point, direction, angle)?;
        }
        out.push(sum / SUPERSAMPLE_OFFSETS.len() as f32);
    }
    Ok(out)
}

/// Assemble interior rows into the bordered tile and encode it BC6H. Border
/// texels copy their interior source by the SH atlas's wrap rule.
pub(crate) fn encode_tile(rows: &[Vec<Vec3>]) -> Vec<u8> {
    let dimension = REFLECTION_PROBE_TILE_DIMENSION;
    encode_bc6h_rgb_from_f32_rgba(&bordered_rgba(rows), dimension, dimension)
}

/// The tile as row-major RGBA f32, border included.
fn bordered_rgba(rows: &[Vec<Vec3>]) -> Vec<f32> {
    let dimension = REFLECTION_PROBE_TILE_DIMENSION;
    let mut rgba = Vec::with_capacity((dimension * dimension * 4) as usize);
    for y in 0..dimension {
        for x in 0..dimension {
            let [ix, iy] =
                irradiance_tile_source_texel(x, y, dimension, REFLECTION_PROBE_TILE_BORDER);
            let texel = rows[iy as usize][ix as usize];
            rgba.extend_from_slice(&[texel.x, texel.y, texel.z, 1.0]);
        }
    }
    rgba
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
    }
}
