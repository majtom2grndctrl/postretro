// Tests: the reflection distance search. Acceptance row 17 (box projection
// and distance search), the crate and pillar fixtures, and the search as a
// function of the fragment or cell alone.
// See: context/lib/testing_guide.md · content/dev/maps/reflection-probes-test.map

use std::cell::RefCell;

use glam::{Vec2, Vec3, Vec4};
use postretro_level_format::octahedral::irradiance_tile_texel_coord;
use postretro_level_format::reflection_probes::{
    REFLECTION_PROBE_LIGHTS_PER_PROBE, ReflectionProbeLightShare, ReflectionProbeRecord,
};

use super::fixtures::{
    Scene, TILE, UNIT, canal, capture_tile, empty_room, engine, engine_dir, pillar_room, pool_room,
    pool_room_probes, room_b,
};
use super::*;
use crate::fluid_surface::reflection::{
    FLUID_REFLECTION_RAY_MIN_Y, FluidReflectionFragment, FluidReflectionLights,
    reflection_above_horizon, reflection_box_hit, reflection_fragment, reflection_fresnel,
    reflection_lookup, reflection_mirror,
};
use crate::fluid_surface::reflection_style::{
    FLUID_REFLECTION_STYLE_PIXEL, FluidTexelAxes, reflection_point,
};
use crate::frame_uniforms::LightTermMask;

/// The search over `scene`'s probe for a ray from `origin`.
fn search(scene: &Scene, origin: Vec3, ray: Vec3) -> FluidReflectionSearch {
    reflection_search(
        origin,
        ray,
        scene.box_min,
        scene.box_max,
        scene.capture,
        TILE,
        |uv| scene.distance(uv),
    )
}

/// Plain box projection's lookup: toward the box hit.
fn box_lookup(scene: &Scene, origin: Vec3, ray: Vec3) -> Vec3 {
    let hit = reflection_box_hit(origin, ray, scene.box_min, scene.box_max);
    reflection_lookup(hit, scene.capture, ray)
}

/// The continuous tile texel coordinate `direction` reads.
fn texel(direction: Vec3) -> Vec2 {
    Vec2::from(irradiance_tile_texel_coord(
        direction.normalize().to_array(),
        TILE[0],
        TILE[1],
    ))
}

/// Tile texels between two lookup directions, the larger axis.
fn texels_apart(a: Vec3, b: Vec3) -> f32 {
    let d = (texel(a) - texel(b)).abs();
    d.x.max(d.y)
}

/// The view ray from `eye` to `fragment` mirrored about the water's up
/// normal: the reflected ray.
fn reflected(eye: Vec3, fragment: Vec3) -> Vec3 {
    reflection_mirror((eye - fragment).normalize(), Vec3::Y)
}

/// The brush the probe term shows: what the capture saw toward the lookup,
/// or nothing when the capture shadow drops the term.
fn shown(scene: &Scene, result: &FluidReflectionSearch) -> Option<&'static str> {
    (result.weight > 0.0)
        .then(|| scene.seen_from_capture(result.lookup))
        .flatten()
}

// --- Acceptance row 17: box projection and distance search -------------------

// In a room with no interior geometry the search's lookup equals plain box
// projection within one capture texel: fragments over the whole pool, eyes
// low and high around the room, every wall, the ceiling and the edges.
#[test]
fn an_empty_room_searches_to_the_box_projection_lookup() {
    let scene = empty_room();
    let mut checked = 0;
    let mut worst = 0.0f32;
    for ix in 0..19 {
        for iz in 0..15 {
            let fragment = Vec3::new(-4.5 + ix as f32 * 0.5, 0.0, -3.5 + iz as f32 * 0.5);
            for ex in 0..5 {
                for ez in 0..5 {
                    for ey in [0.1, 0.6, 1.7, 3.0] {
                        let eye = Vec3::new(-4.8 + ex as f32 * 2.4, ey, -3.8 + ez as f32 * 1.9);
                        if (eye - fragment).length() < 0.3 {
                            continue;
                        }
                        let ray = reflected(eye, fragment);
                        let result = search(scene, fragment, ray);
                        assert_ne!(result.status, FluidReflectionSearchStatus::CaptureShadow);
                        assert_eq!(result.weight, 1.0);
                        let apart = texels_apart(result.lookup, box_lookup(scene, fragment, ray));
                        assert!(
                            apart <= 1.0,
                            "{fragment} seen from {eye}: {apart} texels off ({:?})",
                            result.status
                        );
                        worst = worst.max(apart);
                        checked += 1;
                    }
                }
            }
        }
    }
    assert!(checked > 25_000, "{checked}");
    assert!(worst > 0.0, "the sweep reaches the filtered edges");

    // A box hit exactly on a concave corner edge, reached steeply from 0.9 m
    // away, is the limit: bilinear filtering blends the two walls' inverse
    // distances there and reads the corner a little near.
    let fragment = Vec3::new(-4.5, 0.0, -3.5);
    let ray = reflected(Vec3::new(-4.0, 1.7, -3.0), fragment);
    let apart = texels_apart(
        search(scene, fragment, ray).lookup,
        box_lookup(scene, fragment, ray),
    );
    assert!(apart <= 1.5, "{apart} texels at the corner");
}

// The box hit never moves with the capture point; the lookup is the search's
// point minus the capture point, clamped to the horizon below it.
#[test]
fn the_lookup_is_the_search_point_minus_the_capture_point_clamped_to_the_horizon() {
    let (box_min, box_max) = (Vec3::new(-5.0, -1.0, -4.0), Vec3::new(5.0, 4.0, 4.0));
    let fragment = Vec3::new(0.5, 0.0, 1.0);
    let ray = reflected(Vec3::new(-1.0, 1.7, -3.5), fragment);
    let hit = reflection_box_hit(fragment, ray, box_min, box_max);
    for capture in [Vec3::new(0.0, 0.002, 0.0), Vec3::new(-2.0, 0.002, 1.5)] {
        let result = reflection_search(fragment, ray, box_min, box_max, capture, TILE, |_| 0.0);
        assert_eq!(result.point, hit, "the box hit is the capture point's");
        assert_eq!(result.lookup, hit - capture);
    }

    // Every search over the crate scene: lookup = level(point − C).
    let scene = pool_room();
    for x in [170.0, 250.0, 300.0, 344.0] {
        for degrees in [1.0f32, 10.0, 30.0, 60.0] {
            let fragment = engine(x, 224.0, 0.0);
            let theta = degrees.to_radians();
            let ray = engine_dir(theta.cos(), 0.0, theta.sin());
            let result = search(scene, fragment, ray);
            let toward = result.point - scene.capture;
            let level = Vec3::new(toward.x, toward.y.max(0.0), toward.z);
            assert_eq!(result.lookup, level, "{x} at {degrees}°");
            assert!(result.lookup.y >= 0.0);
        }
    }
}

// With the stored surface beyond the box hit (walls past a box fitted
// smaller than the room), nothing stored lies inside the box: the lookup is
// the box hit's.
#[test]
fn a_stored_distance_beyond_the_box_hit_keeps_the_box_hit() {
    let room = empty_room();
    let (box_min, box_max) = (Vec3::new(-3.0, -1.0, -2.5), Vec3::new(3.0, 2.5, 2.5));
    for (fragment, eye) in [
        (Vec3::new(1.0, 0.0, 0.5), Vec3::new(-2.0, 1.5, -2.0)),
        (Vec3::new(-2.0, 0.0, -1.0), Vec3::new(2.5, 0.3, 2.0)),
        (Vec3::new(0.0, 0.0, 2.0), Vec3::new(0.0, 2.0, -2.0)),
    ] {
        let ray = reflected(eye, fragment);
        let result = reflection_search(fragment, ray, box_min, box_max, room.capture, TILE, |uv| {
            room.distance(uv)
        });
        assert_eq!(result.status, FluidReflectionSearchStatus::BeyondBox);
        assert_eq!(
            result.point,
            reflection_box_hit(fragment, ray, box_min, box_max)
        );
        assert_eq!(
            result.lookup,
            reflection_lookup(result.point, room.capture, ray)
        );
    }
}

// A capture miss (stored 0) is never a root and never ends the search: with
// nothing stored within 10° of the horizon, the fragment's own tap misses,
// yet the box hit, behind a wall 2 m short of the box, closes the bracket
// and false position finds that wall.
#[test]
fn a_capture_miss_never_ends_the_search_early() {
    let room = empty_room();
    let walls = room.brushes.clone();
    let near = Scene {
        brushes: walls,
        capture: room.capture,
        box_min: room.box_min,
        box_max: room.box_max + Vec3::new(2.0, 0.0, 0.0),
        tile: capture_tile(|d| {
            if d.y < 10.0f32.to_radians().sin() {
                0.0
            } else {
                room.inverse_l1(d)
            }
        }),
    };
    let fragment = Vec3::new(-1.0, 0.0, 0.0);
    let theta = 20.0f32.to_radians();
    let ray = Vec3::new(theta.cos(), theta.sin(), 0.0);
    assert_eq!(
        reflection_search_h(fragment, near.capture, TILE, &|uv| near.distance(uv)),
        -1.0,
        "the fragment's own tap is a capture miss"
    );
    let result = search(&near, fragment, ray);
    assert_eq!(result.status, FluidReflectionSearchStatus::Hit);
    assert!(
        (result.point.x - 5.0).abs() < 0.1,
        "the stored wall at x = 5, not the box at 7: {}",
        result.point
    );
}

// A capture miss partway along the ray never ends the search either: with
// nothing stored in a band of azimuth east-south-east of the capture point,
// the first sphere step lands in the band and stepping stops, yet the box
// hit, far past the east wall and outside the band, still closes the
// bracket, and false position settles between the step and the box hit.
#[test]
fn a_capture_miss_on_a_sphere_step_never_ends_the_search_early() {
    let room = empty_room();
    let in_band = |d: Vec3| d.x > 0.0 && (-44.0..=-24.0).contains(&d.z.atan2(d.x).to_degrees());
    let banded = Scene {
        brushes: room.brushes.clone(),
        capture: room.capture,
        box_min: room.box_min,
        box_max: room.box_max + Vec3::new(20.0, 0.0, 0.0),
        tile: capture_tile(|d| if in_band(d) { 0.0 } else { room.inverse_l1(d) }),
    };
    let h = |q: Vec3| reflection_search_h(q, banded.capture, TILE, &|uv| banded.distance(uv));
    let fragment = Vec3::new(-4.5, 0.0, -3.5);
    let theta = 6.0f32.to_radians();
    let ray = Vec3::new(theta.cos(), theta.sin(), 0.0);
    // The first sphere step, as the search takes it: to the ray point as far
    // from the capture point as the surface the fragment's own tap stores.
    let h_origin = h(fragment);
    assert!(h_origin > -1.0 && h_origin < 0.0, "{h_origin}");
    let from_capture = fragment - banded.capture;
    let level = Vec3::new(from_capture.x, from_capture.y.max(0.0), from_capture.z);
    let radius = level.length() / (h_origin + 1.0);
    let b = from_capture.dot(ray);
    let t_step = -b + (b * b - (from_capture.dot(from_capture) - radius * radius)).sqrt();
    assert!(
        in_band(fragment + ray * t_step - banded.capture),
        "the fixture's step lies in the band"
    );
    assert_eq!(h(fragment + ray * t_step), -1.0, "the first step is a miss");
    let box_hit = reflection_box_hit(fragment, ray, banded.box_min, banded.box_max);
    let t_box = (box_hit - fragment).dot(ray);
    assert!(t_step < t_box);
    assert!(
        h(box_hit) >= 0.0,
        "the box hit lies behind the stored east wall"
    );

    let result = search(&banded, fragment, ray);
    assert_eq!(result.status, FluidReflectionSearchStatus::Hit);
    let t_hit = (result.point - fragment).dot(ray);
    assert!(
        t_hit > t_step && t_hit <= t_box,
        "settles in the bracket: {t_step} < {t_hit} ≤ {t_box}"
    );
}

// --- The search starts inside the box -----------------------------------------

/// The probe record of `scene`'s probe, at layer 0 with no lights.
fn probe_record(scene: &Scene) -> ReflectionProbeRecord {
    ReflectionProbeRecord {
        source_entity: 0,
        box_min: scene.box_min.to_array(),
        box_max: scene.box_max.to_array(),
        capture_point: scene.capture.to_array(),
        layer: 0,
        lights: [ReflectionProbeLightShare::UNUSED; REFLECTION_PROBE_LIGHTS_PER_PROBE],
    }
}

// Water outside an authored box (a face straddling the box, its centroid
// inside): the search starts from the fragment clamped onto the box face it
// lies past, and a ray leaving through that face leaves the box through
// another, so the reflection resolves along the ray to the wall the capture
// saw past the box and follows the view, rather than collapsing onto the
// horizon at the fragment's own azimuth.
#[test]
fn water_outside_an_authored_box_resolves_along_its_ray() {
    let room = empty_room();
    let (box_min, box_max) = (Vec3::new(-3.0, -1.0, -2.5), Vec3::new(3.0, 2.5, 2.5));
    let fragment = Vec3::new(3.5, 0.0, 0.0);
    let start = Vec3::new(3.0, 0.0, 0.0);
    let mut heights = Vec::new();
    for eye in [Vec3::new(-2.0, 1.5, 0.0), Vec3::new(-2.0, 3.0, 0.0)] {
        let ray = reflected(eye, fragment);
        assert!(
            ray.x > 0.0,
            "the ray leaves through the face the fragment lies past"
        );
        let result = reflection_search(fragment, ray, box_min, box_max, room.capture, TILE, |uv| {
            room.distance(uv)
        });
        assert_eq!(result.status, FluidReflectionSearchStatus::Hit, "eye {eye}");
        assert_eq!(result.weight, 1.0, "eye {eye}");
        assert!(
            (result.point.x - 5.0).abs() < 0.05,
            "the east wall past the box: {}",
            result.point
        );
        let along = (result.point - start).normalize();
        assert!(
            along.abs_diff_eq(ray, 1.0e-3),
            "on the ray from the clamped start: {along} vs {ray}"
        );
        let lookup = result.lookup;
        let elevation = lookup.y.atan2(lookup.x.hypot(lookup.z)).to_degrees();
        assert!(
            elevation > 4.0,
            "eye {eye}: {elevation:.1}° above the horizon"
        );
        heights.push(result.point.y);
    }
    assert!(
        heights[1] > heights[0] + 0.3,
        "the reflection follows the view: {heights:?}"
    );
}

// A pixel-style cell straddling a wall at the water's rim: its centre lies
// inside the wall, behind the surface the capture stored there, where the
// depth margin alone would fade the term to about 0.8. The search starts
// from the centre clamped onto the box (a derived box's wall), on the stored
// surface: the cell keeps its full term, and its search reads the wall's
// foot where the cell meets it.
#[test]
fn a_pixel_cell_centred_inside_a_wall_at_the_rim_is_not_dimmed() {
    let scene = canal();
    let probe = probe_record(scene);
    let eye = Vec3::new(-1.5, 1.6, 0.0);
    // The rim's last 3 mm of water before the +z wall, in a cell (16 texels
    // a metre, offset a twentieth of a texel) whose centre lies 2.8 cm in.
    let world = Vec3::new(0.0, 0.0, 1.998);
    let mut fragment = smooth_fragment(eye, world);
    fragment.style = FLUID_REFLECTION_STYLE_PIXEL;
    fragment.texel_uv = Vec2::new(world.x * 16.0, world.z * 16.0 + 0.05);
    let point = reflection_point(
        fragment.style,
        fragment.texel_uv,
        world,
        Vec3::Y,
        fragment.axes,
        fragment.time,
        fragment.texels_per_pixel,
    );
    assert!(
        point.position.z > 2.02,
        "inside the wall: {}",
        point.position
    );
    let h_centre = reflection_search_h(point.position, scene.capture, TILE, &|uv| {
        scene.distance(uv)
    });
    assert!(
        h_centre > 0.0 && h_centre < FLUID_REFLECTION_SHADOW_MARGIN,
        "the centre lies behind the stored wall, within the depth margin: {h_centre}"
    );

    let view = (eye - point.position).normalize();
    let ray = reflection_above_horizon(reflection_mirror(view, point.normal));
    let result = search(scene, point.position, ray);
    assert_eq!(result.weight, 1.0, "{:?}", result.status);
    let start = point.position.clamp(scene.box_min, scene.box_max);
    let foot = Vec3::new(start.x, 0.0, start.z) - Vec3::new(scene.capture.x, 0.0, scene.capture.z);
    let apart = texels_apart(result.lookup, foot);
    assert!(apart <= 1.0, "the wall's foot: {apart} texels off");

    // The whole fragment reference: over a zero-alpha surface its coverage
    // is the cell's undimmed Fresnel weight.
    let out = reflection_fragment(
        Vec4::new(0.1, 0.2, 0.3, 0.0),
        &fragment,
        &[probe],
        TILE,
        &FluidReflectionLights::NONE,
        |_, _| Vec3::ONE,
        |_, uv| scene.distance(uv),
    );
    let fresnel = reflection_fresnel(view.dot(Vec3::Y), 1.0);
    assert!((out.w - fresnel).abs() < 1.0e-6, "{} vs {fresnel}", out.w);
}

// A grazing pixel-style view: a cell whose displayed offset tilts its normal
// away from the eye mirrors the ray below the water. The fragment reference
// floors it above the horizon before the search: its distance taps are
// those of the search along the floored ray, not along the mirrored one.
#[test]
fn a_pixel_cell_tilted_past_a_grazing_view_searches_above_the_horizon() {
    let scene = empty_room();
    let probe = probe_record(scene);
    let camera = Vec3::new(-4.5, 0.25, 0.0);
    let (fragment, point, mirrored) = (0..64)
        .flat_map(|x| (-16..16).map(move |z| Vec2::new(x as f32 + 0.5, z as f32 + 0.5)))
        .find_map(|texel| {
            let mut fragment = smooth_fragment(camera, cell_point(texel));
            fragment.style = FLUID_REFLECTION_STYLE_PIXEL;
            fragment.time = 3.3;
            let point = reflection_point(
                fragment.style,
                fragment.texel_uv,
                fragment.world_position,
                Vec3::Y,
                fragment.axes,
                fragment.time,
                fragment.texels_per_pixel,
            );
            let mirrored = reflection_mirror((camera - point.position).normalize(), point.normal);
            (mirrored.y < 0.0).then_some((fragment, point, mirrored))
        })
        .expect("some cell's tilt mirrors the grazing view below the water");
    let floored = reflection_above_horizon(mirrored);
    assert!(floored.y >= 0.99 * FLUID_REFLECTION_RAY_MIN_Y && floored.y < 0.01);
    assert!((floored.length() - 1.0).abs() < 1.0e-6);
    let level = |v: Vec3| Vec3::new(v.x, 0.0, v.z).normalize();
    assert!(
        level(floored).abs_diff_eq(level(mirrored), 1.0e-6),
        "azimuth kept"
    );

    let taps_along = |ray: Vec3| {
        let taps = RefCell::new(Vec::new());
        reflection_search(
            point.position,
            ray,
            scene.box_min,
            scene.box_max,
            scene.capture,
            TILE,
            |uv| {
                taps.borrow_mut().push(uv);
                scene.distance(uv)
            },
        );
        taps.into_inner()
    };
    let taps = RefCell::new(Vec::new());
    reflection_fragment(
        Vec4::new(0.1, 0.2, 0.3, 0.6),
        &fragment,
        &[probe],
        TILE,
        &FluidReflectionLights::NONE,
        |_, _| Vec3::ONE,
        |_, uv| {
            taps.borrow_mut().push(uv);
            scene.distance(uv)
        },
    );
    let taps = taps.into_inner();
    assert_eq!(taps, taps_along(floored));
    assert_ne!(taps, taps_along(mirrored));
}

// --- Crate fixture (test-map pool room) ---------------------------------------

/// Highest elevation (0.1° steps) of a ray due east from map `(x, 224)` on
/// the water whose reflection shows the crate: truly, or by `lookup`.
fn crate_top(scene: &Scene, x: f32, lookup: Option<fn(&Scene, Vec3, Vec3) -> Vec3>) -> f32 {
    let fragment = engine(x, 224.0, 0.0);
    let mut top = 0.0;
    for k in 1..700 {
        let theta = (k as f32 * 0.1).to_radians();
        let ray = engine_dir(theta.cos(), 0.0, theta.sin());
        let shows = match lookup {
            None => scene.trace(fragment, ray).map(|(_, name)| name),
            Some(f) => scene.seen_from_capture(f(scene, fragment, ray)),
        };
        if shows == Some("crate") {
            top = k as f32 * 0.1;
        }
    }
    top
}

fn searched_lookup(scene: &Scene, fragment: Vec3, ray: Vec3) -> Vec3 {
    let result = search(scene, fragment, ray);
    assert_eq!(result.weight, 1.0, "the pool is in the capture's view");
    result.lookup
}

/// Depression of the reflected crate's top edge on the line y = 224, seen
/// from the west coping (map x 152) at `eye_m` above the water: the nearest
/// fragment (0.5-unit steps) whose reflection shows the crate.
fn crate_edge_from_coping(scene: &Scene, eye_m: f32, searched: bool) -> f32 {
    let eye = engine(152.0, 224.0, eye_m / UNIT);
    let nearest = (322..704).map(|x| x as f32 * 0.5).find(|&x| {
        let fragment = engine(x, 224.0, 0.0);
        let ray = reflected(eye, fragment);
        let shows = if searched {
            shown(scene, &search(scene, fragment, ray))
        } else {
            scene.trace(fragment, ray).map(|(_, name)| name)
        };
        shows == Some("crate")
    });
    let x = nearest.expect("the crate reflects in the pool");
    (eye_m / UNIT).atan2(x - 152.0).to_degrees()
}

// Crate fixture: from the west coping at 0.5, 1.6 and 3 m the reflected
// crate's top edge lies within 0.6° (under half a capture texel) of the true
// mirror's, where box projection shows no crate at all from 3 m.
#[test]
fn the_crate_top_edge_from_the_west_coping_matches_the_mirror() {
    let scene = pool_room();
    for eye_m in [0.5, 1.6, 3.0] {
        let truth = crate_edge_from_coping(scene, eye_m, false);
        let searched = crate_edge_from_coping(scene, eye_m, true);
        assert!(
            (searched - truth).abs() <= 0.6,
            "eye {eye_m} m: top edge {searched:.2}° vs the mirror's {truth:.2}°"
        );
    }
    let eye = engine(152.0, 224.0, 3.0 / UNIT);
    assert!(
        (322..704).all(|x| {
            let fragment = engine(x as f32 * 0.5, 224.0, 0.0);
            let ray = reflected(eye, fragment);
            scene.seen_from_capture(box_lookup(scene, fragment, ray)) != Some("crate")
        }),
        "box projection shows no crate from 3 m"
    );
}

// Crate fixture: at east-rim fragments x 300–336 the reflected crate's top
// elevation is within 2° of the true mirror's; box projection is 4.5–13.5°
// low there.
#[test]
fn the_crate_top_elevation_at_the_east_rim_matches_the_mirror() {
    let scene = pool_room();
    for x in [300.0, 320.0, 336.0] {
        let truth = crate_top(scene, x, None);
        let searched = crate_top(scene, x, Some(searched_lookup));
        let boxed = crate_top(scene, x, Some(box_lookup));
        assert!(
            (searched - truth).abs() <= 2.0,
            "x {x}: {searched:.1}° vs the mirror's {truth:.1}°"
        );
        assert!(
            truth - boxed >= 4.0,
            "x {x}: box {boxed:.1}° vs {truth:.1}°"
        );
    }
}

// --- Pillar fixture (test-map room B) -------------------------------------------

/// Water fragments of room B's basin on an 8-unit grid, the pillar's
/// footprint left out, that `eye` sees: (fragment, reflected ray).
fn basin_fragments(scene: &Scene, eye: Vec3) -> Vec<(Vec3, Vec3)> {
    let mut out = Vec::new();
    for x in (928..1440).step_by(8) {
        for y in (112..400).step_by(8) {
            let (x, y) = (x as f32 + 4.0, y as f32 + 4.0);
            if (1168.0..=1200.0).contains(&x) && (240.0..=272.0).contains(&y) {
                continue;
            }
            let fragment = engine(x, y, -16.0);
            let to_eye = eye - fragment;
            if scene
                .trace(fragment, to_eye.normalize())
                .is_some_and(|(t, _)| t < to_eye.length() - UNIT)
            {
                continue;
            }
            out.push((fragment, reflected(eye, fragment)));
        }
    }
    out
}

// Pillar fixture: water behind the pillar as seen from the capture point
// draws no probe term; the fragment renders exactly as without reflection.
#[test]
fn a_fragment_behind_the_pillar_draws_no_probe_term() {
    let scene = room_b();
    let eye = engine(960.0, 100.0, 63.0);
    for fragment in [engine(1230.0, 256.0, -16.0), engine(1300.0, 262.0, -16.0)] {
        let result = search(scene, fragment, reflected(eye, fragment));
        assert_eq!(result.status, FluidReflectionSearchStatus::CaptureShadow);
        assert_eq!(result.weight, 0.0);

        let probe = ReflectionProbeRecord {
            source_entity: 0,
            box_min: scene.box_min.to_array(),
            box_max: scene.box_max.to_array(),
            capture_point: scene.capture.to_array(),
            layer: 0,
            lights: [ReflectionProbeLightShare::UNUSED; REFLECTION_PROBE_LIGHTS_PER_PROBE],
        };
        let surface = Vec4::new(0.1, 0.2, 0.3, 0.6);
        let out = reflection_fragment(
            surface,
            &smooth_fragment(eye, fragment),
            &[probe],
            TILE,
            &FluidReflectionLights::NONE,
            |_, _| Vec3::new(4.0, 4.0, 4.0),
            |_, uv| scene.distance(uv),
        );
        assert!(out.abs_diff_eq(surface, 1.0e-6), "{out} vs {surface}");
    }
}

fn smooth_fragment(camera: Vec3, world_position: Vec3) -> FluidReflectionFragment {
    FluidReflectionFragment {
        probe: 0,
        camera,
        world_position,
        outward: Vec3::Y,
        light_term_mask: LightTermMask::ALL.bits(),
        reflectivity: 1.0,
        style: crate::fluid_surface::reflection_style::FLUID_REFLECTION_STYLE_SMOOTH,
        texel_uv: Vec2::new(world_position.x, world_position.z) * 16.0,
        axes: FluidTexelAxes {
            u: Vec3::X / 16.0,
            v: Vec3::Z / 16.0,
        },
        time: 0.0,
        texels_per_pixel: 0.5,
    }
}

/// One capture texel at the pillar's range from the capture point (about
/// 1.4° at 130 units), in map units.
const PILLAR_TEXEL_UNITS: f32 = 3.2;

// Pillar fixture: a fragment whose reflected ray meets the pillar's
// capture-facing (west) face samples it within one tile texel of the true
// hit, so the reflection attaches at the base: the fragments in front of the
// face along the capture point's own sight line, the hit and that line
// crossing the face at least a texel inside its edges. (A ray that meets the
// face off that line is the thin-object case: the first sphere step jumps
// to the far wall stored in the fragment's own direction.)
#[test]
fn a_ray_meeting_the_pillars_capture_facing_face_samples_it_within_one_texel() {
    let scene = room_b();
    let face = engine(1168.0, 0.0, 0.0).z;
    let inside =
        |map_y: f32| (240.0 + PILLAR_TEXEL_UNITS..=272.0 - PILLAR_TEXEL_UNITS).contains(&map_y);
    let mut checked = 0;
    for eye in [
        engine(960.0, 100.0, 63.0),
        engine(960.0, 100.0, 118.0),
        engine(1100.0, 90.0, 63.0),
    ] {
        for (fragment, ray) in basin_fragments(scene, eye) {
            let Some((t, "pillar")) = scene.trace(fragment, ray) else {
                continue;
            };
            let hit = fragment + ray * t;
            let toward = fragment - scene.capture;
            let sight = scene.capture + toward * ((face - scene.capture.z) / toward.z);
            if (hit.z - face).abs() > 1.0e-4 || !inside(-hit.x / UNIT) || !inside(-sight.x / UNIT) {
                continue;
            }
            let result = search(scene, fragment, ray);
            let apart = texels_apart(result.lookup, hit - scene.capture);
            assert!(
                apart <= 1.0,
                "{fragment}: {apart} texels from the true hit {hit} ({:?})",
                result.status
            );
            assert!(
                (result.point.y - hit.y).abs() < 2.0 * UNIT,
                "attached within 2 units of the true height"
            );
            checked += 1;
        }
    }
    assert!(checked >= 10, "only {checked} fragments");
}

// Pillar fixture: from the west walkway at 1.6 m no more than 10% of the
// visible water shows a false pillar (box projection paints it on about a
// fifth, much of that over the capture shadow behind it).
#[test]
fn from_the_west_walkway_few_fragments_show_a_false_pillar() {
    let scene = room_b();
    let eye = engine(960.0, 100.0, 63.0);
    let fragments = basin_fragments(scene, eye);
    let (mut searched_false, mut box_false) = (0, 0);
    for &(fragment, ray) in &fragments {
        let truth = scene.trace(fragment, ray).map(|(_, name)| name) == Some("pillar");
        let result = search(scene, fragment, ray);
        if !truth && shown(scene, &result) == Some("pillar") {
            searched_false += 1;
        }
        if !truth && scene.seen_from_capture(box_lookup(scene, fragment, ray)) == Some("pillar") {
            box_false += 1;
        }
    }
    let n = fragments.len();
    assert!(searched_false * 10 <= n, "{searched_false} of {n}");
    assert!(
        box_false * 10 > n,
        "the fixture separates: box {box_false} of {n}"
    );
}

// --- Two probes over one room ----------------------------------------------------

/// Points on the pool room's four walls on a 32-unit grid, 8 to 248 units up.
fn pool_room_wall_points() -> Vec<Vec3> {
    let mut points = Vec::new();
    for along in (16..512).step_by(32) {
        for up in (8..256).step_by(16) {
            let (a, z) = (along as f32, up as f32);
            points.extend([
                engine(0.0, a, z),
                engine(512.0, a, z),
                engine(a, 0.0, z),
                engine(a, 512.0, z),
            ]);
        }
    }
    points
}

/// Where the eye sees the water mirror `wall`: the fragment on the water on
/// the line from `eye` to `wall` mirrored below the water plane, when it lies
/// on the pool and the eye sees it.
fn mirror_fragment(scene: &Scene, eye: Vec3, wall: Vec3) -> Option<Vec3> {
    let fragment = eye + (Vec3::new(wall.x, -wall.y, wall.z) - eye) * (eye.y / (eye.y + wall.y));
    let (map_x, map_y) = (-fragment.z / UNIT, -fragment.x / UNIT);
    let on_pool = (160.5..351.5).contains(&map_x) && (160.5..351.5).contains(&map_y);
    let to_eye = eye - fragment;
    let seen = scene
        .trace(fragment, to_eye.normalize())
        .is_none_or(|(t, _)| t >= to_eye.length() - UNIT);
    (on_pool && seen).then_some(fragment)
}

// Acceptance A8: two probes sharing the pool room's box at different capture
// points, both seeing a wall point W. For every fragment of the pool whose
// true mirror ray hits W (wall points on a grid, eyes around the room at 0.5,
// 1.6 and 3 m), at least 99% of each probe's searches resolve to within one
// capture texel of W in that probe's own tile, and none beyond five, so two
// faces hard-bound to the two probes meet there with no geometry jump. The
// misses are rays passing behind an occluder's silhouette as seen from that
// probe's capture point (A4's thin-object class): the bench and stump put
// 2.75 and 3.92 texels of error on 5 of 3072 fragments per probe.
const CROSS_PROBE_MISS_SHARE: f32 = 0.01;
const CROSS_PROBE_WORST_TEXELS: f32 = 5.0;

#[test]
fn two_probes_over_one_room_resolve_a_wall_point_both_see_within_one_texel() {
    let probes = pool_room_probes();
    let eyes: Vec<Vec3> = [
        (152.0, 224.0),
        (100.0, 300.0),
        (256.0, 140.0),
        (400.0, 400.0),
        (470.0, 150.0),
        (300.0, 450.0),
    ]
    .into_iter()
    .flat_map(|(x, y)| [0.5, 1.6, 3.0].map(|m: f32| engine(x, y, m / UNIT)))
    .collect();
    let mut worst = [0.0f32; 2];
    let mut misses = [0usize; 2];
    let mut over = Vec::new();
    let mut checked = 0;
    for wall in pool_room_wall_points() {
        let both_see = probes.iter().all(|scene| {
            let toward = wall - scene.capture;
            scene
                .trace(scene.capture, toward.normalize())
                .is_some_and(|(t, _)| (t - toward.length()).abs() < 1.0e-3)
        });
        if !both_see {
            continue;
        }
        for &eye in &eyes {
            let Some(fragment) = mirror_fragment(&probes[0], eye, wall) else {
                continue;
            };
            let ray = reflected(eye, fragment);
            let truly = probes[0]
                .trace(fragment, ray)
                .is_some_and(|(t, _)| (fragment + ray * t).abs_diff_eq(wall, 1.0e-3));
            if !truly {
                continue;
            }
            for (i, scene) in probes.iter().enumerate() {
                let result = search(scene, fragment, ray);
                let apart = texels_apart(result.lookup, wall - scene.capture);
                worst[i] = worst[i].max(apart);
                if apart > 1.0 {
                    misses[i] += 1;
                    over.push(format!(
                        "probe {i}: W {wall} from eye {eye} at {fragment}: {apart:.2} texels ({:?})",
                        result.status
                    ));
                }
            }
            checked += 1;
        }
    }
    assert!(checked >= 100, "only {checked} fragments");
    let allowed = CROSS_PROBE_MISS_SHARE * checked as f32;
    assert!(
        misses.iter().all(|&n| n as f32 <= allowed)
            && worst.iter().all(|&w| w <= CROSS_PROBE_WORST_TEXELS),
        "per probe {misses:?} of {checked} fragments' searches land over one texel from W \
         (at most {allowed:.0} allowed); worst per probe {worst:?} texels (at most \
         {CROSS_PROBE_WORST_TEXELS}):\n{}",
        over.join("\n")
    );
}

// --- The search is a function of the fragment or cell alone ----------------------

// Every fragment of one pixel-style cell reads one lookup: the search runs
// from the cell centre about the cell's tilted normal, so each fragment
// takes the same distance taps and samples the colour tile at one UV.
#[test]
fn every_fragment_of_a_pixel_cell_reads_one_search() {
    let scene = pool_room();
    let probe = ReflectionProbeRecord {
        source_entity: 0,
        box_min: scene.box_min.to_array(),
        box_max: scene.box_max.to_array(),
        capture_point: scene.capture.to_array(),
        layer: 0,
        lights: [ReflectionProbeLightShare::UNUSED; REFLECTION_PROBE_LIGHTS_PER_PROBE],
    };
    let camera = engine(152.0, 224.0, 1.6 / UNIT);
    let run = |world: Vec3| {
        let taps = RefCell::new(Vec::new());
        let colour = RefCell::new(None);
        let mut fragment = smooth_fragment(camera, world);
        fragment.style = FLUID_REFLECTION_STYLE_PIXEL;
        reflection_fragment(
            Vec4::new(0.1, 0.2, 0.3, 0.6),
            &fragment,
            &[probe],
            TILE,
            &FluidReflectionLights::NONE,
            |_, uv| {
                *colour.borrow_mut() = Some(uv);
                Vec3::ONE
            },
            |_, uv| {
                taps.borrow_mut().push(uv);
                scene.distance(uv)
            },
        );
        (taps.into_inner(), colour.into_inner().expect("samples"))
    };
    // Cells across the pool, 16 texels a metre.
    for cell in [(-90, -82), (-110, -86), (-128, -84)] {
        let corner = Vec2::new(cell.0 as f32, cell.1 as f32);
        let (first_taps, first_colour) = run(cell_point(corner + Vec2::splat(0.1)));
        assert!(first_taps.len() >= 2);
        for inside in [
            Vec2::new(0.9, 0.2),
            Vec2::new(0.5, 0.5),
            Vec2::new(0.15, 0.85),
        ] {
            let (taps, colour) = run(cell_point(corner + inside));
            assert_eq!(taps, first_taps, "cell {cell:?}");
            assert_eq!(colour, first_colour, "cell {cell:?}");
        }
    }
    // The cell centre is where the search starts.
    let point = reflection_point(
        FLUID_REFLECTION_STYLE_PIXEL,
        Vec2::new(-89.3, -81.6),
        cell_point(Vec2::new(-89.3, -81.6)),
        Vec3::Y,
        smooth_fragment(camera, Vec3::ZERO).axes,
        0.0,
        0.5,
    );
    assert!(
        point
            .position
            .abs_diff_eq(cell_point(Vec2::new(-89.5, -81.5)), 1.0e-5)
    );
}

/// The world point on the pool's water at texel UV `texel` (16 texels a
/// metre, as [`smooth_fragment`] maps it).
fn cell_point(texel: Vec2) -> Vec3 {
    Vec3::new(texel.x / 16.0, 0.0, texel.y / 16.0)
}

// The fragment reference hands the search the style's point and the ray
// reflected there, and nothing else of the fragment.
#[test]
fn the_fragment_reference_searches_from_the_style_point_alone() {
    let source = include_str!("reflection.rs");
    let body: String = source
        .split("pub fn reflection_fragment(")
        .nth(1)
        .expect("reflection_fragment exists")
        .chars()
        .filter(|c| !c.is_whitespace())
        .collect();
    assert!(
        body.contains(
            "letsearch=reflection_search(point.position,ray,Vec3::from(probe.box_min),\
             Vec3::from(probe.box_max),Vec3::from(probe.capture_point),tile,\
             |uv|distance(probe.layer,uv),);"
        ),
        "{body}"
    );
}

// --- Capture shadow and cost --------------------------------------------------

// Behind a stored surface with nothing near it to fade beside, the term
// fades over the depth margin κ: full at the surface, half at κ/2, gone
// from κ on, looked up toward the box hit.
#[test]
fn the_capture_shadow_fades_over_the_depth_margin() {
    let (box_min, box_max) = (Vec3::new(-5.0, -1.0, -4.0), Vec3::new(5.0, 4.0, 4.0));
    let capture = Vec3::new(0.0, 0.002, 0.0);
    let fragment = Vec3::new(2.0, 0.0, 1.0);
    let ray = reflected(Vec3::new(-3.0, 1.5, 0.0), fragment);
    // |fragment − capture|₁ is 3 at the horizon: w = (1 + h) / 3 everywhere.
    for (h, weight) in [
        (0.0, 1.0),
        (FLUID_REFLECTION_SHADOW_MARGIN * 0.5, 0.5),
        (FLUID_REFLECTION_SHADOW_MARGIN, 0.0),
        (0.5, 0.0),
        (4.0, 0.0),
    ] {
        let result = reflection_search(fragment, ray, box_min, box_max, capture, TILE, |_| {
            (1.0 + h) / 3.0
        });
        assert_eq!(result.status, FluidReflectionSearchStatus::CaptureShadow);
        assert!(
            (result.weight - weight).abs() < 1.0e-5,
            "h {h}: {}",
            result.weight
        );
        assert_eq!(
            result.point,
            reflection_box_hit(fragment, ray, box_min, box_max)
        );
    }
}

// The search's cost is bounded: two independent taps, at most K sphere-step
// taps and M − 1 false-position taps, six in all, beside the capture-shadow
// fade's two to four, over every fragment of the basin from every eye. On
// average the fade adds at most two: a fragment in the shadow takes none,
// one well clear of it two, one near its edge three.
#[test]
fn the_search_takes_at_most_six_taps_and_the_fade_two_on_average() {
    let scene = room_b();
    let bound = 2 + FLUID_REFLECTION_SEARCH_STEPS + FLUID_REFLECTION_SEARCH_REFINEMENTS - 1;
    assert_eq!(bound, 6);
    let (mut most_search, mut most_fade, mut fade_taps, mut fragments) = (0, 0, 0, 0);
    let mut near_edge = 0;
    for eye in [
        engine(960.0, 100.0, 63.0),
        engine(1100.0, 90.0, 63.0),
        engine(1330.0, 256.0, 255.0),
    ] {
        for (fragment, ray) in basin_fragments(scene, eye) {
            let taps = std::cell::Cell::new(0u32);
            let counted = |uv: Vec2| {
                taps.set(taps.get() + 1);
                scene.distance(uv)
            };
            let result = reflection_search(
                fragment,
                ray,
                scene.box_min,
                scene.box_max,
                scene.capture,
                TILE,
                counted,
            );
            let total = taps.get();
            // The fade's own taps, counted apart.
            // Past the depth margin the shadow takes the two first taps only.
            let deep = result.status == FluidReflectionSearchStatus::CaptureShadow && total == 2;
            let fade = if deep {
                0
            } else {
                let h_origin =
                    reflection_search_h(fragment, scene.capture, TILE, &|uv| scene.distance(uv));
                taps.set(0);
                reflection_shadow_fade(fragment, scene.capture, TILE, h_origin, &|uv| {
                    taps.set(taps.get() + 1);
                    scene.distance(uv)
                });
                taps.get()
            };
            most_search = most_search.max(total - fade);
            most_fade = most_fade.max(fade);
            fade_taps += fade;
            fragments += 1;
            if fade > 2 {
                near_edge += 1;
            }
        }
    }
    assert!(most_search <= bound, "{most_search} search taps");
    assert_eq!(most_search, bound, "some fragment takes every search step");
    assert!(most_fade <= 4, "{most_fade} fade taps");
    assert!(near_edge > 0, "some fragment brackets an edge");
    let average = fade_taps as f32 / fragments as f32;
    assert!(average <= 2.0, "the fade adds {average} taps on average");
}

// --- The capture-shadow fade -----------------------------------------------------

/// One interior capture texel of angle at the engine's tile.
fn texel_angle() -> f32 {
    std::f32::consts::PI / (TILE[0] - 2 * TILE[1]) as f32
}

/// The water fragment `radius` metres from the capture point at `azimuth`
/// radians from +x toward +z.
fn on_arc(scene: &Scene, radius: f32, azimuth: f32) -> Vec3 {
    let fragment = scene.capture + Vec3::new(azimuth.cos(), 0.0, azimuth.sin()) * radius;
    Vec3::new(fragment.x, 0.0, fragment.z)
}

/// The fade's profile across one side of a pillar's capture shadow: the
/// fragment `radius` metres from the capture point, behind the pillar,
/// swept in azimuth from well clear of the shadow into it in steps of
/// 1/200 texel. Per step: the angle past the shadow's edge in texels
/// (negative in it), and the search's weight.
fn fade_profile(scene: &Scene, radius: f32) -> Vec<(f32, f32)> {
    let eye = Vec3::new(-4.0, 1.6, -3.0);
    let texel = texel_angle();
    let samples: Vec<(f32, f32, FluidReflectionSearchStatus)> = (0..2400)
        .map(|k| {
            let azimuth = (14.0 - k as f32 / 200.0) * texel;
            let fragment = on_arc(scene, radius, azimuth);
            let result = search(scene, fragment, reflected(eye, fragment));
            (azimuth, result.weight, result.status)
        })
        .collect();
    let edge = samples
        .iter()
        .find(|(_, _, status)| *status == FluidReflectionSearchStatus::CaptureShadow)
        .expect("the sweep reaches the shadow")
        .0;
    samples
        .into_iter()
        .map(|(azimuth, weight, _)| ((azimuth - edge) / texel, weight))
        .collect()
}

/// Where the old rule, `1 − smoothstep(0, κ, h)` behind the stored surface
/// with κ = 0.05, faded the same sweep: its width in texels, from the first
/// fragment behind the surface to the first at κ.
fn old_fade_width(scene: &Scene, radius: f32) -> f32 {
    let texel = texel_angle();
    let h_at = |k: u32| {
        let azimuth = (14.0 - k as f32 / 2000.0) * texel;
        let fragment = on_arc(scene, radius, azimuth);
        (
            azimuth,
            reflection_search_h(fragment, scene.capture, TILE, &|uv| scene.distance(uv)),
        )
    };
    let start = (0..24_000).map(h_at).find(|&(_, h)| h >= 0.0).unwrap().0;
    let end = (0..24_000).map(h_at).find(|&(_, h)| h >= 0.05).unwrap().0;
    (start - end) / texel
}

// The point of the fade: its width in angle is the pinned number of capture
// texels whether the occluder stands near the capture point or far from it,
// and whether the fragment lies just behind it or far behind. The old κ
// rule faded over a fraction of a texel that changed with the distances.
// (Within about 30% of the background wall's distance the gate opens late
// and the band narrows, to 1.5 texels 4% in front of it.)
#[test]
fn the_capture_shadow_fade_spans_the_pinned_capture_texels_near_and_far() {
    let n = FLUID_REFLECTION_SHADOW_FADE_TEXELS;
    assert_eq!(n, 2.0);
    let mut old_widths = Vec::new();
    for (near, radii) in [(1.2, [2.2, 4.0]), (3.0, [3.8, 4.2])] {
        let scene = pillar_room(near);
        for radius in radii {
            let profile = fade_profile(&scene, radius);
            let start = profile
                .iter()
                .find(|(_, weight)| *weight < 1.0)
                .expect("the sweep starts clear of the band")
                .0;
            assert!(
                (start - n).abs() <= 0.25,
                "occluder {near} m, fragment {radius} m: fades over {start} texels"
            );
            old_widths.push(old_fade_width(&scene, radius));
        }
    }
    assert!(
        old_widths.iter().all(|&width| width < 0.5),
        "the old rule faded over under half a texel: {old_widths:?}"
    );
    let (least, most) = old_widths
        .iter()
        .fold((f32::INFINITY, 0.0f32), |(a, b), &w| (a.min(w), b.max(w)));
    assert!(
        most > 2.0 * least,
        "the old rule's width changed with the distances: {old_widths:?}"
    );
}

// Across the band the weight falls monotonically and continuously from 1 to
// 0: full a whisker past N texels from the edge, none in the shadow (at any
// depth past it), and no step a 1/200-texel move can see.
#[test]
fn the_capture_shadow_fade_falls_monotonically_and_continuously() {
    let n = FLUID_REFLECTION_SHADOW_FADE_TEXELS;
    for (near, radius) in [(1.2, 2.2), (1.2, 4.0), (3.0, 3.8), (3.0, 4.2), (3.0, 4.8)] {
        let scene = pillar_room(near);
        let profile = fade_profile(&scene, radius);
        let mut steepest = 0.0f32;
        for pair in profile.windows(2) {
            let ((_, outer), (past, inner)) = (pair[0], pair[1]);
            assert!(
                inner <= outer + 1.0e-6,
                "occluder {near} m, fragment {radius} m: rises to {inner} at {past} texels"
            );
            steepest = steepest.max(outer - inner);
        }
        assert!(
            steepest <= 0.05,
            "occluder {near} m, fragment {radius} m: steps {steepest} in 1/200 texel"
        );
        for &(past, weight) in &profile {
            if past > n + 0.05 {
                assert_eq!(weight, 1.0, "{past} texels clear of the shadow");
            }
            if past <= 0.0 {
                assert_eq!(weight, 0.0, "{past} texels into the shadow");
            }
        }
        let start = profile.iter().find(|(_, weight)| *weight < 1.0).unwrap().0;
        assert!(
            start >= 1.4,
            "occluder {near} m, fragment {radius} m: {start} texels"
        );
        let last_lit = profile.iter().rev().find(|(past, _)| *past > 0.0).unwrap();
        assert!(
            last_lit.1 <= 0.001,
            "falls to the shadow's 0 at its edge: {last_lit:?}"
        );
    }
}

// A wall seen obliquely from the capture point: turning a fragment at its
// foot by the fade angle reaches behind it, yet the gate keeps the foot's
// full term up to 66° from the wall's normal, 1 cm from the wall, in a
// canal and in the box room.
#[test]
fn a_wall_seen_obliquely_keeps_its_foot_unfaded() {
    let canal = canal();
    let mut checked = 0;
    for k in 0..45 {
        let along = k as f32 * 0.1;
        for side in [-1.0, 1.0] {
            let fragment = Vec3::new(along, 0.0, side * 1.99);
            let incidence = along.atan2(1.99).to_degrees();
            assert!(incidence <= 66.0);
            let ray = reflected(Vec3::new(-3.0, 1.6, 0.0), fragment);
            let result = search(canal, fragment, ray);
            assert_ne!(result.status, FluidReflectionSearchStatus::CaptureShadow);
            assert_eq!(result.weight, 1.0, "{fragment} at {incidence:.1}°");
            assert_eq!(angular_fade(canal, fragment), 1.0);
            checked += 1;
        }
    }
    let room = empty_room();
    for k in 0..80 {
        let along = -3.95 + k as f32 * 0.1;
        for fragment in [
            Vec3::new(4.99, 0.0, along),
            Vec3::new(-4.99, 0.0, along),
            Vec3::new(along, 0.0, 3.99),
            Vec3::new(along, 0.0, -3.99),
        ] {
            // A foot by a concave corner may read the filtered corner a
            // little near and fall within the depth margin; the angular
            // fade still leaves it whole.
            let ray = reflected(Vec3::new(0.0, 1.6, 0.0), fragment);
            assert!(search(room, fragment, ray).weight >= 0.95, "{fragment}");
            assert_eq!(angular_fade(room, fragment), 1.0, "{fragment}");
            checked += 1;
        }
    }
    assert_eq!(checked, 90 + 320);
}

/// The angular part of the fade alone, at `fragment`.
fn angular_fade(scene: &Scene, fragment: Vec3) -> f32 {
    let distance = |uv: Vec2| scene.distance(uv);
    let h_origin = reflection_search_h(fragment, scene.capture, TILE, &distance);
    reflection_shadow_fade(fragment, scene.capture, TILE, h_origin, &distance)
}

// The fade angle is N interior texels of the tile the section names, each
// a half-turn over the interior: it follows the tile's dimension.
#[test]
fn the_fade_angle_follows_the_tile_dimension() {
    let pi = std::f32::consts::PI;
    assert_eq!(FLUID_REFLECTION_HALF_TURN, pi);
    for (tile, interior) in [([128, 1], 126.0), ([64, 1], 62.0), ([256, 2], 252.0)] {
        let angle = reflection_shadow_fade_angle(tile);
        assert!(
            (angle - FLUID_REFLECTION_SHADOW_FADE_TEXELS * pi / interior).abs() < 1.0e-7,
            "{tile:?}: {angle}"
        );
    }
    // ≈1.43° a texel at the engine's 128² tile: the fade is ≈2.86°.
    let degrees = reflection_shadow_fade_angle(TILE).to_degrees();
    assert!((degrees - 2.857).abs() < 0.001, "{degrees}°");
}
