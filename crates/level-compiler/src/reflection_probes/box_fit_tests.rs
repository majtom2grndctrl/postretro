// Reflection-probe box fit tests: rooms, pits, lips, doorways, beams and tray ceilings against `fit_box`.
// See: context/lib/resource_management.md §5.1

use glam::{DVec3, Vec3};

use super::box_fit::{FIT_AZIMUTHS, FIT_RING_COUNT, WorldBounds, fit_box, fit_rays};
use super::fit_frame::FitFrame;
use super::hit::SceneTracer;
use super::reflection_probes_test_fixtures::{GeometryBuilder, box_room, rect};
use crate::bvh_build::build_bvh;

fn fit(geometry: &crate::geometry::GeometryResult, origin: DVec3) -> (DVec3, DVec3) {
    let (bvh, primitives, _) = build_bvh(geometry).unwrap();
    let tracer = SceneTracer::new(&bvh, &primitives, geometry);
    let bounds =
        WorldBounds::from_positions(geometry.geometry.vertices.iter().map(|v| &v.position))
            .unwrap();
    fit_box(&tracer, origin, &bounds, &fit_rays(), FitFrame::UP)
}

fn quad(builder: &mut GeometryBuilder, normal: Vec3, corners: [Vec3; 4]) {
    builder.push_face(normal, &corners, 0, Vec3::ZERO, 0, [[0.0; 2]; 4]);
}

fn assert_box(actual: (DVec3, DVec3), min: [f64; 3], max: [f64; 3]) {
    let close = |a: DVec3, b: [f64; 3]| (a - DVec3::from(b)).abs().max_element() < 1e-3;
    assert!(close(actual.0, min) && close(actual.1, max), "{actual:?}");
}

/// [`fit`] in the frame of a face facing `normal`.
fn fit_facing(
    geometry: &crate::geometry::GeometryResult,
    origin: DVec3,
    normal: DVec3,
) -> (DVec3, DVec3) {
    let (bvh, primitives, _) = build_bvh(geometry).unwrap();
    let tracer = SceneTracer::new(&bvh, &primitives, geometry);
    let bounds =
        WorldBounds::from_positions(geometry.geometry.vertices.iter().map(|v| &v.position))
            .unwrap();
    fit_box(
        &tracer,
        origin,
        &bounds,
        &fit_rays(),
        FitFrame::nearest(normal),
    )
}

// Row 5's boxes: a wall's or a ceiling's fit runs along its own normal, so
// its box is the room in front of it, floored at the fit origin's plane.
#[test]
fn fit_box_from_a_wall_or_a_ceiling_reaches_the_room_in_front_of_it() {
    let room = box_room([-4.0, -1.0, -3.0], [5.0, 3.0, 2.0]);
    let cases = [
        (
            DVec3::new(-3.998, 1.0, -0.5),
            DVec3::X,
            [-3.998, -1.0, -3.0],
            [5.0, 3.0, 2.0],
        ),
        (
            DVec3::new(4.998, 1.0, -0.5),
            DVec3::NEG_X,
            [-4.0, -1.0, -3.0],
            [4.998, 3.0, 2.0],
        ),
        (
            DVec3::new(1.0, 1.0, -2.998),
            DVec3::Z,
            [-4.0, -1.0, -2.998],
            [5.0, 3.0, 2.0],
        ),
        (
            DVec3::new(1.0, 2.998, -0.5),
            DVec3::NEG_Y,
            [-4.0, -1.0, -3.0],
            [5.0, 2.998, 2.0],
        ),
    ];
    for (origin, normal, min, max) in cases {
        assert_box(fit_facing(&room, origin, normal), min, max);
    }
}

#[test]
fn fit_box_reaches_the_walls_and_ceiling_of_a_plain_room() {
    let room = box_room([-4.0, -1.0, -3.0], [5.0, 3.0, 2.0]);
    let fitted = fit(&room, DVec3::new(1.0, 0.002, -0.5));
    assert_box(fitted, [-4.0, 0.002, -3.0], [5.0, 3.0, 2.0]);
}

/// A pool sunk 1.5 m into a 12 m room's floor: the water sees the pit's
/// walls only at the horizon, and the room's walls above them.
#[test]
fn fit_box_of_a_sunken_pool_reaches_the_room_not_the_pit() {
    let (lo, hi) = (Vec3::new(-6.0, -1.5, -6.0), Vec3::new(6.0, 4.0, 6.0));
    let pit = 2.0;
    let mut b = GeometryBuilder::default();
    // Room shell above the floor line.
    quad(&mut b, Vec3::NEG_Y, rect(lo, hi, 1, hi.y));
    for (normal, axis, value) in [
        (Vec3::X, 0, lo.x),
        (Vec3::NEG_X, 0, hi.x),
        (Vec3::Z, 2, lo.z),
        (Vec3::NEG_Z, 2, hi.z),
    ] {
        quad(
            &mut b,
            normal,
            rect(Vec3::new(lo.x, 0.0, lo.z), hi, axis, value),
        );
    }
    // Floor around the pit, as four strips.
    let floor = |b: &mut GeometryBuilder, x0: f32, z0: f32, x1: f32, z1: f32| {
        let r = rect(Vec3::new(x0, 0.0, z0), Vec3::new(x1, 0.0, z1), 1, 0.0);
        quad(b, Vec3::Y, r);
    };
    floor(&mut b, lo.x, lo.z, hi.x, -pit);
    floor(&mut b, lo.x, pit, hi.x, hi.z);
    floor(&mut b, lo.x, -pit, -pit, pit);
    floor(&mut b, pit, -pit, hi.x, pit);
    // The pit: walls facing its centre, and its floor.
    let (plo, phi) = (Vec3::new(-pit, lo.y, -pit), Vec3::new(pit, 0.0, pit));
    quad(&mut b, Vec3::Y, rect(plo, phi, 1, lo.y));
    for (normal, axis, value) in [
        (Vec3::X, 0, -pit),
        (Vec3::NEG_X, 0, pit),
        (Vec3::Z, 2, -pit),
        (Vec3::NEG_Z, 2, pit),
    ] {
        quad(&mut b, normal, rect(plo, phi, axis, value));
    }
    let geometry = b.finish(vec!["t".to_string()]);

    let water = -0.4;
    let fitted = fit(&geometry, DVec3::new(0.3, water + 0.002, -0.2));
    assert_box(fitted, [-6.0, water + 0.002, -6.0], [6.0, 4.0, 6.0]);
}

/// The pit of a 4 m pool sunk into a 12 m room, under a 0.2 m coping lip
/// overhanging the water 0.5 m on all four sides. Every column meets the
/// pit wall under the lip, then the lip's underside, so no column meets
/// the room wall before a ceiling hit; the room wall seen over the lip is
/// farther out than the pit wall, and the box reaches it.
#[test]
fn fit_box_of_a_sunken_pool_under_an_overhanging_lip_reaches_the_room() {
    let mut b = GeometryBuilder::default();
    open_floor_room(&mut b, Vec3::new(-6.0, 0.0, -6.0), Vec3::new(6.0, 6.0, 6.0));
    pit_under_a_lip(&mut b, 2.0, 1.5, 0.2, -1.5);
    let geometry = b.finish(vec!["t".to_string()]);

    let water = -0.6;
    let fitted = fit(&geometry, DVec3::new(0.3, water + 0.002, -0.2));
    assert_box(fitted, [-6.0, water + 0.002, -6.0], [6.0, 6.0, 6.0]);
}

/// A pool's pit, `pit` m from its centre on each side and down to
/// `floor`, under a coping lip whose underside lies on the floor line
/// (y = 0) and whose inner edge, `lip` m from the centre, stands `coping`
/// m tall. Rays rising from the water never reach the room's floor, the
/// lip's top or its outer edge, so none is built.
fn pit_under_a_lip(b: &mut GeometryBuilder, pit: f32, lip: f32, coping: f32, floor: f32) {
    facing_in(b, Vec3::new(-pit, floor, -pit), Vec3::new(pit, 0.0, pit));
    let underside = |b: &mut GeometryBuilder, x0: f32, z0: f32, x1: f32, z1: f32| {
        let r = rect(Vec3::new(x0, 0.0, z0), Vec3::new(x1, 0.0, z1), 1, 0.0);
        quad(b, Vec3::NEG_Y, r);
    };
    underside(b, -pit, -pit, pit, -lip);
    underside(b, -pit, lip, pit, pit);
    underside(b, -pit, -lip, -lip, lip);
    underside(b, lip, -lip, pit, lip);
    facing_in(b, Vec3::new(-lip, 0.0, -lip), Vec3::new(lip, coping, lip));
}

/// The four walls of `lo`..`hi`, facing its centre.
fn facing_in(b: &mut GeometryBuilder, lo: Vec3, hi: Vec3) {
    for (normal, axis, value) in [
        (Vec3::X, 0, lo.x),
        (Vec3::NEG_X, 0, hi.x),
        (Vec3::Z, 2, lo.z),
        (Vec3::NEG_Z, 2, hi.z),
    ] {
        quad(b, normal, rect(lo, hi, axis, value));
    }
}

/// Two 6 m rooms joined by a 2 m doorway in the wall at x = 0: a pool in
/// the west room fits the west room, not both.
#[test]
fn fit_box_stops_at_a_doorway_wall() {
    let (lo, hi) = (Vec3::new(-6.0, 0.0, -3.0), Vec3::new(6.0, 3.0, 3.0));
    let mut b = GeometryBuilder::default();
    quad(&mut b, Vec3::Y, rect(lo, hi, 1, lo.y));
    quad(&mut b, Vec3::NEG_Y, rect(lo, hi, 1, hi.y));
    for (normal, axis, value) in [
        (Vec3::X, 0, lo.x),
        (Vec3::NEG_X, 0, hi.x),
        (Vec3::Z, 2, lo.z),
        (Vec3::NEG_Z, 2, hi.z),
    ] {
        quad(&mut b, normal, rect(lo, hi, axis, value));
    }
    // The dividing wall, both faces, around a doorway z −1..1, y 0..2.
    for normal in [Vec3::NEG_X, Vec3::X] {
        let pieces = [
            (Vec3::new(0.0, 0.0, -3.0), Vec3::new(0.0, 3.0, -1.0)),
            (Vec3::new(0.0, 0.0, 1.0), Vec3::new(0.0, 3.0, 3.0)),
            (Vec3::new(0.0, 2.0, -1.0), Vec3::new(0.0, 3.0, 1.0)),
        ];
        for (plo, phi) in pieces {
            quad(&mut b, normal, rect(plo, phi, 0, 0.0));
        }
    }
    let geometry = b.finish(vec!["t".to_string()]);

    // Off the doorway's axis, as a pool beside a doorway would be.
    let fitted = fit(&geometry, DVec3::new(-3.0, 0.502, 0.5));
    assert_box(fitted, [-6.0, 0.502, -3.0], [0.0, 3.0, 3.0]);
}

/// A 6 m room opening into a 9 m one through a doorway 4 m wide and 2 m
/// tall in the 6 m wall at x = 0. Most columns facing +X see through the
/// doorway to the far room's back wall, then its ceiling, then end on the
/// lintel; only two beside the doorway meet the wall below any ceiling.
/// That makes its plane a wall, so the door columns take the lintel, and
/// the box stops at the doorway wall rather than the far room's back wall.
#[test]
fn fit_box_stops_at_a_wide_doorway_into_a_deeper_room() {
    let (lo, hi) = (Vec3::new(-6.0, 0.0, -3.0), Vec3::new(9.0, 3.0, 3.0));
    let mut b = GeometryBuilder::default();
    quad(&mut b, Vec3::Y, rect(lo, hi, 1, lo.y));
    open_floor_room(&mut b, lo, hi);
    // The dividing wall, both faces, around a doorway z −2..2, y 0..2.
    for normal in [Vec3::NEG_X, Vec3::X] {
        let pieces = [
            (Vec3::new(0.0, 0.0, -3.0), Vec3::new(0.0, 3.0, -2.0)),
            (Vec3::new(0.0, 0.0, 2.0), Vec3::new(0.0, 3.0, 3.0)),
            (Vec3::new(0.0, 2.0, -2.0), Vec3::new(0.0, 3.0, 2.0)),
        ];
        for (plo, phi) in pieces {
            quad(&mut b, normal, rect(plo, phi, 0, 0.0));
        }
    }
    let geometry = b.finish(vec!["t".to_string()]);

    let fitted = fit(&geometry, DVec3::new(-3.0, 0.502, 0.5));
    assert_box(fitted, [-6.0, 0.502, -3.0], [0.0, 3.0, 3.0]);
}

// Regression: under a lip no column met the room wall before a ceiling,
// so the doorway rescue found no wall and the box reached the bay.
/// A 12 m room, 4 m tall, whose +X wall has an opening 7 m wide and 3 m
/// tall into a bay 3 m deep, its ceiling flush with the lintel; in its
/// floor, the lip fixture's pool under a 0.1 m coping, its water 0.2 m
/// below the lip. No column facing +X meets that wall below a ceiling:
/// each meets the pit wall, then the lip's underside. Those beside the
/// opening top out on the wall past nothing but the lip and end there,
/// so it is a wall: the columns through the opening take the lintel, and
/// the box stops at the room rather than the bay's back wall.
#[test]
fn fit_box_under_a_lip_stops_at_a_wall_with_an_opening() {
    let (lo, hi) = (Vec3::new(-6.0, 0.0, -6.0), Vec3::new(6.0, 4.0, 6.0));
    let (width, door, depth) = (7.0, 3.0, 3.0);
    let mut b = GeometryBuilder::default();
    quad(&mut b, Vec3::NEG_Y, rect(lo, hi, 1, hi.y));
    for (normal, axis, value) in [
        (Vec3::X, 0, lo.x),
        (Vec3::Z, 2, lo.z),
        (Vec3::NEG_Z, 2, hi.z),
    ] {
        quad(&mut b, normal, rect(lo, hi, axis, value));
    }
    // The +X wall beside the opening, and the lintel over it.
    let w = width * 0.5;
    for (plo, phi) in [
        (Vec3::new(hi.x, lo.y, lo.z), Vec3::new(hi.x, hi.y, -w)),
        (Vec3::new(hi.x, lo.y, w), Vec3::new(hi.x, hi.y, hi.z)),
        (Vec3::new(hi.x, door, -w), Vec3::new(hi.x, hi.y, w)),
    ] {
        quad(&mut b, Vec3::NEG_X, rect(plo, phi, 0, hi.x));
    }
    // The bay: its back and side walls, and its ceiling.
    let (blo, bhi) = (Vec3::new(hi.x, lo.y, -w), Vec3::new(hi.x + depth, door, w));
    for (normal, axis, value) in [
        (Vec3::NEG_X, 0, bhi.x),
        (Vec3::Z, 2, blo.z),
        (Vec3::NEG_Z, 2, bhi.z),
        (Vec3::NEG_Y, 1, bhi.y),
    ] {
        quad(&mut b, normal, rect(blo, bhi, axis, value));
    }
    pit_under_a_lip(&mut b, 2.0, 1.5, 0.1, -1.5);
    let geometry = b.finish(vec!["t".to_string()]);

    let water = -0.2;
    let fitted = fit(&geometry, DVec3::new(-0.3, water + 0.002, 0.1));
    assert_box(fitted, [-6.0, water + 0.002, -6.0], [6.0, 4.0, 6.0]);
}

/// A 1.6 m wall on the pool's rim, 2 m off, as e24's grain markers stand:
/// the far wall shows in the band above it, and the box reaches it.
#[test]
fn fit_box_reaches_past_a_low_wall_on_the_rim() {
    let mut b = GeometryBuilder::default();
    let (lo, hi) = (Vec3::new(-12.0, 0.0, -6.0), Vec3::new(12.0, 10.0, 6.0));
    quad(&mut b, Vec3::Y, rect(lo, hi, 1, lo.y));
    quad(&mut b, Vec3::NEG_Y, rect(lo, hi, 1, hi.y));
    for (normal, axis, value) in [
        (Vec3::X, 0, lo.x),
        (Vec3::NEG_X, 0, hi.x),
        (Vec3::Z, 2, lo.z),
        (Vec3::NEG_Z, 2, hi.z),
    ] {
        quad(&mut b, normal, rect(lo, hi, axis, value));
    }
    let (plo, phi) = (Vec3::new(2.0, 0.0, -2.5), Vec3::new(2.4, 1.6, 2.5));
    for (normal, axis, value) in [
        (Vec3::NEG_X, 0, plo.x),
        (Vec3::X, 0, phi.x),
        (Vec3::Y, 1, phi.y),
        (Vec3::NEG_Z, 2, plo.z),
        (Vec3::Z, 2, phi.z),
    ] {
        quad(&mut b, normal, rect(plo, phi, axis, value));
    }
    let geometry = b.finish(vec!["t".to_string()]);

    let fitted = fit(&geometry, DVec3::new(0.0, 0.302, 0.0));
    assert_box(fitted, [-12.0, 0.302, -6.0], [12.0, 10.0, 6.0]);
}

/// The walls and ceiling of the room `lo`..`hi`; no floor, since fit rays
/// never point down.
fn open_floor_room(b: &mut GeometryBuilder, lo: Vec3, hi: Vec3) {
    quad(b, Vec3::NEG_Y, rect(lo, hi, 1, hi.y));
    for (normal, axis, value) in [
        (Vec3::X, 0, lo.x),
        (Vec3::NEG_X, 0, hi.x),
        (Vec3::Z, 2, lo.z),
        (Vec3::NEG_Z, 2, hi.z),
    ] {
        quad(b, normal, rect(lo, hi, axis, value));
    }
}

/// A beam spanning the room above the pool, 1 m deep under a 4 m
/// ceiling: columns toward it meet the ceiling beyond the beam first,
/// then the beam's side, which must not become the room's wall.
#[test]
fn fit_box_ignores_a_ceiling_beam_spanning_the_room() {
    let (lo, hi) = (Vec3::new(-6.0, 0.0, -6.0), Vec3::new(6.0, 4.0, 6.0));
    let mut b = GeometryBuilder::default();
    open_floor_room(&mut b, lo, hi);
    let (blo, bhi) = (Vec3::new(0.0, 3.0, lo.z), Vec3::new(0.4, hi.y, hi.z));
    for (normal, axis, value) in [
        (Vec3::NEG_Y, 1, blo.y),
        (Vec3::NEG_X, 0, blo.x),
        (Vec3::X, 0, bhi.x),
    ] {
        quad(&mut b, normal, rect(blo, bhi, axis, value));
    }
    let geometry = b.finish(vec!["t".to_string()]);

    let fitted = fit(&geometry, DVec3::new(-2.0, 0.302, 0.3));
    assert_box(fitted, [-6.0, 0.302, -6.0], [6.0, 4.0, 6.0]);
}

/// A tray ceiling: a 3 m soffit 2 m deep around the walls, the centre
/// raised to 4 m. Columns meet the soffit, then the raised centre's
/// riser, which must not become the room's wall.
#[test]
fn fit_box_of_a_tray_ceiling_reaches_the_walls() {
    let mut b = GeometryBuilder::default();
    tray_ceiling_room(&mut b);
    let geometry = b.finish(vec!["t".to_string()]);

    // Most sky ends on the raised centre, so the box's top is its 4 m.
    let fitted = fit(&geometry, DVec3::new(0.3, 0.302, -0.2));
    assert_box(fitted, [-6.0, 0.302, -6.0], [6.0, 4.0, 6.0]);
}

/// The walls of a 12 m room with a tray ceiling: a 3 m soffit 2 m deep
/// around the walls, the centre raised to 4 m. No floor.
fn tray_ceiling_room(b: &mut GeometryBuilder) {
    let (lo, hi) = (Vec3::new(-6.0, 0.0, -6.0), Vec3::new(6.0, 3.0, 6.0));
    facing_in(b, lo, hi);
    // The soffit as four strips around the raised centre.
    let inner = 4.0;
    let soffit = |b: &mut GeometryBuilder, x0: f32, z0: f32, x1: f32, z1: f32| {
        let r = rect(Vec3::new(x0, 0.0, z0), Vec3::new(x1, 0.0, z1), 1, hi.y);
        quad(b, Vec3::NEG_Y, r);
    };
    soffit(b, lo.x, lo.z, hi.x, -inner);
    soffit(b, lo.x, inner, hi.x, hi.z);
    soffit(b, lo.x, -inner, -inner, inner);
    soffit(b, inner, -inner, hi.x, inner);
    // The raised centre: its risers facing inward, and its ceiling.
    let (clo, chi) = (
        Vec3::new(-inner, hi.y, -inner),
        Vec3::new(inner, 4.0, inner),
    );
    quad(b, Vec3::NEG_Y, rect(clo, chi, 1, chi.y));
    facing_in(b, clo, chi);
}

/// The lip fixture's pool under the tray fixture's ceiling, off centre
/// toward +X and −Z. The lip hides the +X and −Z walls from the water
/// nearly up to the soffit, so no ring sees them: those columns pass the
/// soffit and top out on the raised centre's riser, and the box stops at
/// it on those sides. Pins the limit `fit_box` names; an authored box is
/// the override.
#[test]
fn fit_box_of_a_pool_under_a_lip_and_a_tray_ceiling_stops_at_the_riser_it_lies_toward() {
    let mut b = GeometryBuilder::default();
    tray_ceiling_room(&mut b);
    pit_under_a_lip(&mut b, 2.0, 1.5, 0.2, -1.5);
    let geometry = b.finish(vec!["t".to_string()]);

    let water = -0.6;
    let fitted = fit(&geometry, DVec3::new(0.3, water + 0.002, -0.2));
    assert_box(fitted, [-6.0, water + 0.002, -4.0], [4.0, 4.0, 6.0]);
}

#[test]
fn fit_rays_cover_the_upper_hemisphere_by_solid_angle() {
    let rays = fit_rays();
    assert_eq!(rays.len(), 1 + FIT_AZIMUTHS * FIT_RING_COUNT);
    assert!(rays.iter().all(|r| r.direction.y >= 0.0 && r.weight > 0.0));
    assert!(
        rays.iter()
            .all(|r| (r.direction.length() - 1.0).abs() < 1e-12)
    );
    // Weights partition the hemisphere: 2π sr over 2π.
    let total: f64 = rays.iter().map(|r| r.weight).sum();
    assert!((total - 1.0).abs() < 1e-9, "{total}");
}
