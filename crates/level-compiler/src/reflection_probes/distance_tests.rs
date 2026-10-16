// Distance tile and capture-visibility tests: the stored inverse L1 hit distance per texel, the miss substrate, and the hidden-water warning.
// See: context/lib/resource_management.md §5 · rendering_pipeline.md §7.3a

use glam::Vec3;
use log::Level as LogLevel;
use postretro_level_format::environment_volumes::EnvironmentVolumesSection;
use postretro_level_format::lightmap::f32_to_f16_bits;
use postretro_level_format::octahedral::{
    irradiance_interior_texel_direction, irradiance_tile_source_texel, irradiance_tile_texel_coord,
};
use postretro_level_format::reflection_probes::{
    REFLECTION_PROBE_TILE_BORDER, REFLECTION_PROBE_TILE_DIMENSION, ReflectionProbesSection,
};
use postretro_test_log_capture::LogCapture;

use super::capture::{
    F16_MIN_POSITIVE, SUPERSAMPLE_OFFSETS, TILE_INTERIOR, distance_bits, encode_distance_tile,
    inverse_l1_distance, octahedral_direction,
};
use super::capture_visibility::{CAPTURE_SHADOW_MARGIN, DistanceTile};
use super::reflection_probes_test_fixtures::{
    GeometryBuilder, fluid_volume, rect, up_rect, volumes,
};
use super::tests::{CEILING, bake, lit_floor_room, lit_floor_room_with, pool};
use crate::bc6h::f16_bits_to_f32;
use crate::map_data::MapReflectionVolume;

/// The capture point of [`pool`]'s probe.
const CAPTURE: Vec3 = Vec3::new(0.0, 0.502, 0.0);

/// Decoded distance texel `(x, y)` of `layer`'s tile, border included.
fn distance_texel(section: &ReflectionProbesSection, layer: usize, x: u32, y: u32) -> f32 {
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

// --- Capture-visibility warning ----------------------------------------------

/// An L-shaped pool at 0.5 m in [`lit_floor_room`]: a narrow west leg
/// (x −2.8..−2, z −2.8..0.6) turning into a wide north leg (z 0.6..2.8),
/// 2.72 m² and 12.32 m².
fn l_pool() -> EnvironmentVolumesSection {
    volumes(vec![fluid_volume(
        5,
        &[
            up_rect(-2.8, -2.8, -2.0, 0.6, 0.5),
            up_rect(-2.8, 0.6, 2.8, 2.8, 0.5),
        ],
    )])
}

/// One authored box over the whole L, its capture point moved into the
/// south end of the west leg, (−2.4, −2.4): the faces' area centroid
/// (−0.434, 1.194) plus the offset.
fn l_pool_box() -> MapReflectionVolume {
    MapReflectionVolume {
        source_entity: 6,
        source_label: "entity 6 'reflection_volume'".to_string(),
        min: [-2.9, 0.4, -2.9],
        max: [2.9, 2.9, 2.9],
        capture_offset: Some([-1.966, -3.594]),
    }
}

/// A full-height wall from the east wall to x −2, between the legs.
fn dividing_wall(builder: &mut GeometryBuilder) {
    let (lo, hi) = (Vec3::new(-2.0, 0.0, -0.25), Vec3::new(3.0, 3.0, 0.25));
    for (normal, axis, value) in [
        (Vec3::NEG_X, 0, lo.x),
        (Vec3::NEG_Z, 2, lo.z),
        (Vec3::Z, 2, hi.z),
    ] {
        builder.push_face(normal, &rect(lo, hi, axis, value), 1, lo, 0, [[0.0; 2]; 4]);
    }
}

// Acceptance A2: the dividing wall hides most of the north leg from a
// capture point in the west leg (about a third of the water seen), so the
// probe warns, naming itself and its source entity. The same L without the
// wall, and the plain pool, warn nothing.
#[test]
fn a_capture_point_that_sees_under_half_its_water_warns() {
    let warning = "probe 0 of entity 6: its capture point";
    let mut room = lit_floor_room_with([255, 255, 255, 255], dividing_wall);
    room.extras.authored = vec![l_pool_box()];
    let logs = LogCapture::start();
    let section = bake(&room, &l_pool()).unwrap();
    assert_eq!(section.probes.len(), 1);
    let point = Vec3::from(section.probes[0].capture_point);
    assert!(
        (point - Vec3::new(-2.4, 0.502, -2.4)).length() < 1e-3,
        "{point}"
    );
    logs.assert_logged_once(LogLevel::Warn, warning);
    let message = logs
        .records()
        .into_iter()
        .find(|r| r.message.contains(warning))
        .unwrap()
        .message;
    let percent: f64 = message
        .split(" sees ")
        .nth(1)
        .and_then(|rest| rest.split('%').next())
        .and_then(|n| n.parse().ok())
        .expect("the warning names the fraction seen");
    assert!((20.0..45.0).contains(&percent), "{message}");

    logs.clear();
    let mut open = lit_floor_room([255, 255, 255, 255]);
    open.extras.authored = vec![l_pool_box()];
    assert!(bake(&open, &l_pool()).is_some());
    assert!(bake(&lit_floor_room([255, 255, 255, 255]), &pool()).is_some());
    logs.assert_not_logged(LogLevel::Warn, "of the water it binds");
}

// --- Miss substrate ----------------------------------------------------------

// Acceptance A7: a finite hit at any in-level distance stores a non-zero
// distance, a texel only one supersample of which hit at 40 km included; a
// miss stores 0, so a later sky term can fill exactly what the capture missed.
#[test]
fn a_finite_hit_at_any_distance_stores_non_zero_and_a_miss_stores_zero() {
    assert_eq!(distance_bits(0.0), 0);
    let one_far_hit = inverse_l1_distance(Some(40_000.0), Vec3::ONE.normalize())
        / SUPERSAMPLE_OFFSETS.len() as f32;
    for w in [1.0e-12, 1.0e-9, F16_MIN_POSITIVE, one_far_hit, 3.0e-5, 0.5] {
        let bits = distance_bits(w);
        assert_ne!(bits, 0, "{w}");
        assert!(f16_bits_to_f32(bits) > 0.0, "{w}");
    }
    // Through the tile encoder: a texel holding a vanishing mean keeps it,
    // and its missed neighbour stays 0.
    let mut rows = vec![vec![0.0f32; TILE_INTERIOR as usize]; TILE_INTERIOR as usize];
    rows[10][20] = 1.0e-10;
    let rows: Vec<&[f32]> = rows.iter().map(Vec::as_slice).collect();
    let bytes = encode_distance_tile(&rows);
    let texel = |x: u32, y: u32| {
        let at = ((y * REFLECTION_PROBE_TILE_DIMENSION + x) * 2) as usize;
        f16_bits_to_f32(u16::from_le_bytes([bytes[at], bytes[at + 1]]))
    };
    let border = REFLECTION_PROBE_TILE_BORDER;
    assert!(texel(20 + border, 10 + border) > 0.0);
    assert_eq!(texel(21 + border, 10 + border), 0.0);
}

// --- Sight in the shader's units ---------------------------------------------

/// A distance tile storing `w(direction)` at every texel, border included.
fn tile_of(w: impl Fn(Vec3) -> f32) -> DistanceTile {
    let dimension = REFLECTION_PROBE_TILE_DIMENSION;
    let border = REFLECTION_PROBE_TILE_BORDER;
    let mut bytes = Vec::new();
    for y in 0..dimension {
        for x in 0..dimension {
            let [ix, iy] = irradiance_tile_source_texel(x, y, dimension, border);
            let d = Vec3::from(irradiance_interior_texel_direction(
                ix, iy, dimension, border,
            ));
            bytes.extend_from_slice(&f32_to_f16_bits(w(d)).to_le_bytes());
        }
    }
    DistanceTile::from_bytes(dimension, border, &bytes)
}

/// A point at L1 distance `l1` from the origin, `degrees` above +X.
fn at(l1: f32, degrees: f32) -> Vec3 {
    let (sin, cos) = degrees.to_radians().sin_cos();
    let d = Vec3::new(cos, sin, 0.0);
    d * (l1 / d.abs().element_sum())
}

// The capture-visibility warning's depth margin is the fluid shader's
// capture-shadow κ, so its seen water is the water that draws a probe term.
#[test]
fn capture_shadow_margin_matches_the_fluid_shaders() {
    use postretro_render_cpu::fluid_surface::reflection_search::FLUID_REFLECTION_SHADOW_MARGIN;
    assert_eq!(CAPTURE_SHADOW_MARGIN, FLUID_REFLECTION_SHADOW_MARGIN);
}

// Sight means "the shader draws a probe term": h < κ, not h < 0. Water a
// little behind the stored surface, inside the margin, draws a faded term
// and is seen; water past the margin draws none.
#[test]
fn sight_holds_inside_the_capture_shadow_margin_and_fails_past_it() {
    // A surface at L1 2 in every direction: h = l1 / 2 − 1.
    let shell = tile_of(|_| 0.5);
    let seen = |l1: f32| shell.sees(Vec3::ZERO, at(l1, 20.0));
    assert!(seen(1.9), "in front of the surface");
    assert!(seen(2.0 * (1.0 + 0.5 * CAPTURE_SHADOW_MARGIN)), "h = κ/2");
    assert!(!seen(2.0 * (1.0 + 2.0 * CAPTURE_SHADOW_MARGIN)), "h = 2κ");
    // A capture that stored no surface sees everything.
    let sky = tile_of(|_| 0.0);
    assert!(sky.sees(Vec3::ZERO, at(40.0, 10.0)));
}
