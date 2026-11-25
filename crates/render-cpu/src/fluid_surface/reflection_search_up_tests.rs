// Tests: water's search (up-only, `reflection_search`) and the search about
// a bound face's axis (`reflection_search_about`, the glossy overlay's) agree
// bit for bit about +Y over real captured tiles and random rays, so an
// up-facing glossy floor reads what water reads; and a wall's and a ceiling's
// lookups stay in their own hemisphere.
// See: context/lib/rendering_pipeline.md §7.3a (Reflection, Glossy world faces)

use glam::{Vec2, Vec3};
use proptest::prelude::*;

use super::fixtures::{Scene, SceneBox, TILE, capture_tile, pillar_room, pool_room, room_b};
use super::*;
use crate::fluid_surface::reflection::{
    reflection_above_horizon, reflection_box_hit, reflection_lookup, reflection_tile_uv,
};
use crate::fluid_surface::reflection_hemisphere::{
    FLUID_REFLECTION_UP, REFLECTION_UP_FACING_MIN_COS, reflection_above_hemisphere,
    reflection_hemisphere_axis, reflection_level, reflection_lookup_about,
};
use crate::fluid_surface::reflection_search_about::{
    reflection_search_about, reflection_shadow_tap_about,
};

// ---- Up-facing parity -------------------------------------------------------

fn finite_unit() -> impl Strategy<Value = Vec3> {
    (-1.0f32..1.0, -1.0f32..1.0, -1.0f32..1.0)
        .prop_map(|(x, y, z)| Vec3::new(x, y, z))
        .prop_filter("a direction", |v| v.length() > 1.0e-3)
        .prop_map(Vec3::normalize)
}

/// The two searches' every output, bit for bit up to a zero's sign.
fn assert_same_search(about: FluidReflectionSearch, water: FluidReflectionSearch, case: &str) {
    assert_eq!(about.status, water.status, "{case}");
    assert_eq!(about.weight, water.weight, "{case}");
    assert_eq!(about.point.to_array(), water.point.to_array(), "{case}");
    assert_eq!(about.lookup.to_array(), water.lookup.to_array(), "{case}");
    assert_eq!(
        reflection_tile_uv(about.lookup, TILE[0], TILE[1]).to_array(),
        reflection_tile_uv(water.lookup, TILE[0], TILE[1]).to_array(),
        "{case}"
    );
}

/// The scenes the parity runs over: the test map's pool room (crate), room B
/// (pillar) and a pillar room with its pillar near the capture point, each
/// captured as the compiler captures.
fn parity_scenes() -> Vec<&'static Scene> {
    static NEAR: std::sync::OnceLock<Scene> = std::sync::OnceLock::new();
    vec![pool_room(), room_b(), NEAR.get_or_init(|| pillar_room(0.3))]
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(4096))]

    // About +Y, the search about an axis is water's search: from any origin
    // (at the water, above or below it, inside the box or past it) along any
    // ray, floored above the horizon or not, both return the same hit point,
    // lookup, tile UV, status and weight, bit for bit.
    #[test]
    fn the_search_about_up_is_waters_search_bit_for_bit(
        scene in 0usize..3,
        at in (0.0f32..1.0, -0.6f32..0.6, 0.0f32..1.0),
        overshoot in (-0.5f32..0.5, -0.5f32..0.5),
        ray in finite_unit(),
        floored in any::<bool>(),
    ) {
        let scene = parity_scenes()[scene];
        let size = scene.box_max - scene.box_min;
        let origin = Vec3::new(
            scene.box_min.x + size.x * at.0 + overshoot.0,
            scene.capture.y - 0.002 + at.1,
            scene.box_min.z + size.z * at.2 + overshoot.1,
        );
        let ray = if floored { reflection_above_horizon(ray) } else { ray };
        let distance = |uv: Vec2| scene.distance(uv);
        let about = reflection_search_about(
            origin,
            ray,
            scene.box_min,
            scene.box_max,
            scene.capture,
            FLUID_REFLECTION_UP,
            TILE,
            distance,
        );
        let water = reflection_search(
            origin,
            ray,
            scene.box_min,
            scene.box_max,
            scene.capture,
            TILE,
            distance,
        );
        assert_same_search(about, water, &format!("{origin} along {ray}"));
    }

    // The hemisphere helpers about +Y are water's horizon clamps exactly.
    #[test]
    fn the_hemisphere_helpers_about_up_are_waters_horizon_clamps(
        v in (-5.0f32..5.0, -5.0f32..5.0, -5.0f32..5.0),
        capture in (-5.0f32..5.0, -5.0f32..5.0, -5.0f32..5.0),
        ray in finite_unit(),
    ) {
        let (v, capture) = (Vec3::from(v), Vec3::from(capture));
        assert_eq!(
            reflection_level(v, FLUID_REFLECTION_UP).to_array(),
            [v.x, v.y.max(0.0), v.z]
        );
        assert_eq!(
            reflection_above_hemisphere(ray, FLUID_REFLECTION_UP).to_array(),
            reflection_above_horizon(ray).to_array()
        );
        for hit in [v, capture] {
            assert_eq!(
                reflection_lookup_about(hit, capture, ray, FLUID_REFLECTION_UP).to_array(),
                reflection_lookup(hit, capture, ray).to_array()
            );
        }
        let distance = |uv: Vec2| 0.25 + 0.5 * uv.x * uv.y;
        for turn in [0.05f32, -0.05, 0.025] {
            assert_eq!(
                reflection_shadow_tap_about(capture, v, turn, FLUID_REFLECTION_UP, TILE, &distance),
                reflection_shadow_tap(capture, v, turn, TILE, &distance)
            );
        }
    }
}

// ---- Walls and ceilings -----------------------------------------------------

// A face's hemisphere axis is +Y exactly within water's 1° up-facing test,
// and the face's own normal otherwise: a property of the bound face, never of
// a lookup direction.
#[test]
fn the_hemisphere_axis_is_up_within_a_degree_and_the_face_normal_otherwise() {
    assert_eq!(REFLECTION_UP_FACING_MIN_COS, 1.0f32.to_radians().cos());
    let tilted =
        |degrees: f32| Vec3::new(degrees.to_radians().sin(), degrees.to_radians().cos(), 0.0);
    assert_eq!(reflection_hemisphere_axis(Vec3::Y), FLUID_REFLECTION_UP);
    assert_eq!(reflection_hemisphere_axis(tilted(0.9)), FLUID_REFLECTION_UP);
    assert_eq!(reflection_hemisphere_axis(tilted(1.1)), tilted(1.1));
    for normal in [Vec3::X, Vec3::NEG_Z, Vec3::NEG_Y, tilted(45.0)] {
        assert_eq!(reflection_hemisphere_axis(normal), normal);
    }
}

/// An empty 6 × 3 × 6 m room with a probe 2 mm in front of the face whose
/// outward normal is `axis`, at `point` on it; the face itself is
/// transparent to the capture, as a bound reflector is to its own probe.
fn room_probe(axis: Vec3, point: Vec3) -> Scene {
    let (box_min, box_max) = (Vec3::new(-3.0, 0.0, -3.0), Vec3::new(3.0, 3.0, 3.0));
    let slab = |min: [f32; 3], max: [f32; 3], name, outside: Vec3| {
        (
            SceneBox {
                min: Vec3::from(min),
                max: Vec3::from(max),
                name,
            },
            outside,
        )
    };
    let walls = [
        slab([-3.5, -0.5, -3.5], [3.5, 0.0, 3.5], "floor", Vec3::NEG_Y),
        slab([-3.5, 3.0, -3.5], [3.5, 3.5, 3.5], "ceiling", Vec3::Y),
        slab([-3.5, -0.5, -3.5], [-3.0, 3.5, 3.5], "west", Vec3::NEG_X),
        slab([3.0, -0.5, -3.5], [3.5, 3.5, 3.5], "east", Vec3::X),
        slab([-3.5, -0.5, -3.5], [3.5, 3.5, -3.0], "north", Vec3::NEG_Z),
        slab([-3.5, -0.5, 3.0], [3.5, 3.5, 3.5], "south", Vec3::Z),
    ];
    // The face the probe binds faces into the room along `axis`: its brush
    // lies outside along −axis, and the capture sees through it.
    let brushes = walls
        .into_iter()
        .filter(|&(_, outside)| outside != -axis)
        .map(|(brush, _)| brush)
        .collect();
    let mut scene = Scene {
        brushes,
        capture: point + axis * 0.002,
        box_min,
        box_max,
        tile: Vec::new(),
    };
    scene.tile = capture_tile(|d| scene.inverse_l1(d));
    scene
}

// A wall's and a ceiling's lookups keep their own hemisphere, not the
// horizon: a ray from the face toward the floor (from a wall) or the floor
// (from a ceiling) looks up below the capture point, toward what it reflects,
// and no lookup ever points behind the face.
#[test]
fn wall_and_ceiling_lookups_reach_below_the_capture_point_and_never_behind_the_face() {
    let cases = [
        ("east wall", Vec3::NEG_X, Vec3::new(3.0, 2.0, 0.5)),
        ("north wall", Vec3::Z, Vec3::new(-0.5, 2.0, -3.0)),
        ("ceiling", Vec3::NEG_Y, Vec3::new(0.4, 3.0, -0.3)),
    ];
    for (name, axis, centre) in cases {
        let scene = room_probe(axis, centre);
        let mut below = 0;
        for i in 0..64 {
            let t = i as f32 / 63.0;
            // Fragments over the face around its centre, rays fanning down
            // and across from it into the room.
            let (u, v) = if axis.y.abs() > 0.5 {
                (Vec3::X, Vec3::Z)
            } else {
                (axis.cross(Vec3::Y), Vec3::Y)
            };
            let fragment = centre + u * (t - 0.5) + v * ((t * 7.0).fract() - 0.5) * 0.8;
            let ray = (axis * (0.3 + t) + Vec3::NEG_Y * (1.5 - t) + u * (t - 0.4)).normalize();
            let search = reflection_search_about(
                fragment,
                ray,
                scene.box_min,
                scene.box_max,
                scene.capture,
                axis,
                TILE,
                |uv| scene.distance(uv),
            );
            assert!(search.lookup.dot(axis) >= 0.0, "{name}: {}", search.lookup);
            assert_eq!(search.weight, 1.0, "{name}: an empty room shadows nothing");
            // Nothing stands in the room: the lookup is the box projection's,
            // toward the box hit, unclamped.
            let hit = reflection_box_hit(fragment, ray, scene.box_min, scene.box_max);
            let toward = hit - scene.capture;
            assert!(
                (search.lookup.normalize() - toward.normalize()).length() < 2.0e-2,
                "{name}: {} against {}",
                search.lookup,
                toward
            );
            if toward.y < -0.1 {
                below += 1;
                // The up-facing clamp would have levelled it to the horizon.
                assert!(search.lookup.y < -0.05, "{name}: {}", search.lookup);
                assert_eq!(reflection_level(toward, FLUID_REFLECTION_UP).y, 0.0);
            }
        }
        assert!(
            below > 16,
            "{name}: {below} lookups below the capture point"
        );
    }
}

// The shadow fade turns its taps about the face's axis, keeping each tap's
// height along it: about a wall's normal a tap never leaves the wall's
// distance from the capture point along that normal.
#[test]
fn shadow_taps_turn_about_the_faces_axis() {
    let captured = std::cell::RefCell::new(Vec::new());
    let distance = |uv: Vec2| {
        captured.borrow_mut().push(uv);
        0.0
    };
    let capture = Vec3::new(2.998, 1.5, 0.0);
    for axis in [Vec3::NEG_X, Vec3::Z, Vec3::NEG_Y, Vec3::Y] {
        let toward = Vec3::new(-0.7, -0.4, 1.3);
        let h = reflection_shadow_tap_about(capture, toward, 0.05, axis, TILE, &distance);
        assert_eq!(h, -1.0, "a capture miss");
        let uv = captured.borrow_mut().pop().expect("one tap");
        // The tap read toward the turned offset, clamped into the
        // hemisphere: its UV is that direction's.
        let turn2 = 0.05f32 * 0.05;
        let (sine, cosine) = (
            0.05 * (1.0 - turn2 / 6.0 * (1.0 - turn2 / 20.0)),
            1.0 - turn2 / 2.0 * (1.0 - turn2 / 12.0),
        );
        let along = axis * toward.dot(axis);
        let across = toward - along;
        let turned = along + across * cosine + axis.cross(across) * sine;
        assert!((turned.dot(axis) - toward.dot(axis)).abs() < 1.0e-6);
        assert!((turned.length() - toward.length()).abs() < 1.0e-5);
        let expected = reflection_tile_uv(reflection_level(turned, axis), TILE[0], TILE[1]);
        assert!(
            (uv - expected).length() < 1.0e-6,
            "about {axis}: {uv} against {expected}"
        );
    }
}
