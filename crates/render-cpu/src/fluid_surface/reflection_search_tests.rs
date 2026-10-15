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
    Scene, TILE, UNIT, capture_tile, empty_room, engine, engine_dir, pool_room, room_b,
};
use super::*;
use crate::fluid_surface::reflection::{
    FluidReflectionFragment, FluidReflectionLights, reflection_box_hit, reflection_fragment,
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

// Behind a stored surface the term fades over κ of relative depth: full at
// the surface, half at κ/2, gone from κ on, looked up toward the box hit.
#[test]
fn the_capture_shadow_fades_over_the_pinned_margin() {
    let (box_min, box_max) = (Vec3::new(-5.0, -1.0, -4.0), Vec3::new(5.0, 4.0, 4.0));
    let capture = Vec3::new(0.0, 0.002, 0.0);
    let fragment = Vec3::new(2.0, 0.0, 1.0);
    let ray = reflected(Vec3::new(-3.0, 1.5, 0.0), fragment);
    // |fragment − capture|₁ is 3 at the horizon: w = (1 + h) / 3 everywhere.
    for (h, weight) in [
        (0.0, 1.0),
        (FLUID_REFLECTION_SHADOW_FADE * 0.5, 0.5),
        (FLUID_REFLECTION_SHADOW_FADE, 0.0),
        (0.5, 0.0),
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
// taps and M − 1 false-position taps, six in all, over every fragment of the
// basin from every eye.
#[test]
fn the_search_takes_at_most_six_distance_taps() {
    let scene = room_b();
    let bound = 2 + FLUID_REFLECTION_SEARCH_STEPS + FLUID_REFLECTION_SEARCH_REFINEMENTS - 1;
    assert_eq!(bound, 6);
    let mut most = 0;
    for eye in [
        engine(960.0, 100.0, 63.0),
        engine(1100.0, 90.0, 63.0),
        engine(1330.0, 256.0, 255.0),
    ] {
        for (fragment, ray) in basin_fragments(scene, eye) {
            let taps = std::cell::Cell::new(0u32);
            reflection_search(
                fragment,
                ray,
                scene.box_min,
                scene.box_max,
                scene.capture,
                TILE,
                |uv| {
                    taps.set(taps.get() + 1);
                    scene.distance(uv)
                },
            );
            most = most.max(taps.get());
        }
    }
    assert!(most <= bound, "{most} taps");
    assert_eq!(most, bound, "some fragment takes every step");
}
