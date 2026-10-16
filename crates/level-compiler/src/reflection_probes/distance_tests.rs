// Distance tile tests: the stored inverse L1 hit distance per texel.
// See: context/lib/resource_management.md §5 · rendering_pipeline.md §7.3a

use glam::Vec3;
use postretro_level_format::octahedral::{
    irradiance_tile_source_texel, irradiance_tile_texel_coord,
};
use postretro_level_format::reflection_probes::ReflectionProbesSection;

use super::capture::{SUPERSAMPLE_OFFSETS, TILE_INTERIOR, octahedral_direction};
use super::tests::{CEILING, bake, lit_floor_room, pool};
use crate::bc6h::f16_bits_to_f32;

/// The capture point of [`pool`]'s probe.
const CAPTURE: Vec3 = Vec3::new(0.0, 0.502, 0.0);

/// Decoded distance texel `(x, y)` of `layer`'s tile, border included.
pub(super) fn distance_texel(
    section: &ReflectionProbesSection,
    layer: usize,
    x: u32,
    y: u32,
) -> f32 {
    let at = ((y * section.tile_dimension + x) * 2) as usize;
    let tile = section.distance_tile(layer);
    f16_bits_to_f32(u16::from_le_bytes([tile[at], tile[at + 1]]))
}

/// `1 / |hit − CAPTURE|₁` along unit `direction` in [`lit_floor_room`]'s
/// 6 × 3 × 6 m box, or 0 when the ray leaves through the top: `open_top`
/// drops the ceiling, and the walls end at 3 m.
fn room_inverse_l1(direction: Vec3, open_top: bool) -> f32 {
    let (lo, hi) = (Vec3::new(-3.0, 0.0, -3.0), Vec3::new(3.0, 3.0, 3.0));
    let mut t = f32::INFINITY;
    for axis in 0..3 {
        let d = direction[axis];
        if d == 0.0 || (open_top && axis == 1 && d > 0.0) {
            continue;
        }
        let bound = if d > 0.0 { hi[axis] } else { lo[axis] };
        t = t.min((bound - CAPTURE[axis]) / d);
    }
    // With the top open, a wall hit above the walls' 3 m top is a miss.
    if !t.is_finite() || (open_top && (CAPTURE + direction * t).y > hi.y) {
        return 0.0;
    }
    1.0 / (t * direction.abs().element_sum())
}

/// The capture's analytic value for interior texel `(x, y)`: the mean over
/// its supersamples.
fn analytic_texel(x: u32, y: u32, open_top: bool) -> f32 {
    let interior = TILE_INTERIOR as f32;
    SUPERSAMPLE_OFFSETS
        .iter()
        .map(|[dx, dy]| {
            let d = octahedral_direction(
                (x as f32 + 0.5 + dx) / interior,
                (y as f32 + 0.5 + dy) / interior,
            );
            room_inverse_l1(d, open_top)
        })
        .sum::<f32>()
        / SUPERSAMPLE_OFFSETS.len() as f32
}

// Acceptance A1: in a box room every texel reads the mean of
// 1/|wall − capture point|₁ over its supersamples within f16 rounding, and
// the texel toward each wall reads that wall's distance.
#[test]
fn distance_tile_reads_the_inverse_l1_distance_to_each_wall_of_a_box_room() {
    let section = bake(&lit_floor_room([255, 255, 255, 255]), &pool()).unwrap();
    for y in 0..TILE_INTERIOR {
        for x in 0..TILE_INTERIOR {
            let stored = distance_texel(&section, 0, x + 1, y + 1);
            let expected = analytic_texel(x, y, false);
            // f16 keeps 11 significant bits: half an ulp is 2⁻¹¹ relative.
            assert!(
                (stored - expected).abs() <= expected * 1.0e-3,
                "texel ({x}, {y}): stored {stored}, expected {expected}"
            );
        }
    }
    // Toward each wall, at the texel nearest its axis: the wall's distance
    // from (0, 0.502, 0), within the texel's angular width.
    for (axis, distance) in [
        (Vec3::X, 3.0),
        (Vec3::NEG_X, 3.0),
        (Vec3::Z, 3.0),
        (Vec3::NEG_Z, 3.0),
        (Vec3::Y, 3.0 - 0.502),
        (Vec3::NEG_Y, 0.502),
    ] {
        let [u, v] = irradiance_tile_texel_coord(
            (axis + Vec3::splat(1.0e-3)).normalize().to_array(),
            section.tile_dimension,
            section.tile_border,
        );
        let stored = distance_texel(&section, 0, u as u32, v as u32);
        let expected = 1.0 / distance;
        assert!(
            (stored - expected).abs() <= expected * 0.05,
            "toward {axis}: stored {stored}, expected about {expected}"
        );
    }
}

// Acceptance A1: a ray that hits nothing stores 0. The ceiling is wound away
// from the room, so the capture culls it as the world pass culls back faces
// and every ray leaving through the top hits nothing.
#[test]
fn distance_tile_stores_zero_where_the_capture_hits_nothing() {
    let mut room = lit_floor_room([255, 255, 255, 255]);
    let ceiling = CEILING * 6;
    for triangle in room.geometry.geometry.indices[ceiling..ceiling + 6].chunks_mut(3) {
        triangle.swap(1, 2);
    }
    let section = bake(&room, &pool()).unwrap();
    let mut misses = 0;
    for y in 0..TILE_INTERIOR {
        for x in 0..TILE_INTERIOR {
            if analytic_texel(x, y, true) == 0.0 {
                assert_eq!(distance_texel(&section, 0, x + 1, y + 1), 0.0, "({x}, {y})");
                misses += 1;
            }
        }
    }
    assert!(misses > 100, "only {misses} texels see the open top");
    // Straight up is open sky; straight down still reads the floor.
    let [u, v] = irradiance_tile_texel_coord([1.0e-3, 1.0, 1.0e-3], 128, 1);
    assert_eq!(distance_texel(&section, 0, u as u32, v as u32), 0.0);
    let [u, v] = irradiance_tile_texel_coord([1.0e-3, -1.0, 1.0e-3], 128, 1);
    assert!(distance_texel(&section, 0, u as u32, v as u32) > 1.9);
}

// Acceptance A1, row 16: border texels follow the colour tile's rule, each a
// bit-exact copy of its wrapped interior source.
#[test]
fn distance_tile_borders_copy_their_wrapped_interior_sources() {
    let section = bake(&lit_floor_room([255, 255, 255, 255]), &pool()).unwrap();
    let dimension = section.tile_dimension;
    let tile = section.distance_tile(0);
    let bits = |x: u32, y: u32| {
        let at = ((y * dimension + x) * 2) as usize;
        [tile[at], tile[at + 1]]
    };
    let mut borders = 0;
    for y in 0..dimension {
        for x in 0..dimension {
            if x != 0 && y != 0 && x != dimension - 1 && y != dimension - 1 {
                continue;
            }
            let [ix, iy] = irradiance_tile_source_texel(x, y, dimension, section.tile_border);
            assert_eq!(bits(x, y), bits(ix + 1, iy + 1), "border ({x}, {y})");
            borders += 1;
        }
    }
    assert_eq!(borders, 4 * dimension - 4);
}
