// Tests: up-facing probes search exactly as before the hemisphere
// generalization. A frozen copy of the search as it stood (every tap and the
// lookup clamped to y ≥ 0) is compared with `reflection_search_about` about
// +Y over real captured tiles and random rays, bit for bit; and a wall's and a
// ceiling's lookups stay in their own hemisphere.
// See: context/lib/rendering_pipeline.md §7.3a (Reflection, Glossy world faces)

use glam::{Vec2, Vec3};
use proptest::prelude::*;

use super::fixtures::{Scene, SceneBox, TILE, capture_tile, pillar_room, pool_room, room_b};
use super::*;
use crate::fluid_surface::reflection::{
    FLUID_REFLECTION_DIRECTION_EPSILON, reflection_above_horizon, reflection_box_hit,
    reflection_lookup, reflection_tile_uv,
};
use crate::fluid_surface::reflection_hemisphere::{
    FLUID_REFLECTION_UP, REFLECTION_UP_FACING_MIN_COS, reflection_above_hemisphere,
    reflection_hemisphere_axis, reflection_level, reflection_lookup_about,
};

// ---- The search as it stood before the hemisphere generalization ----------

fn frozen_lookup(hit: Vec3, capture_point: Vec3, ray: Vec3) -> Vec3 {
    let toward = hit - capture_point;
    let level = Vec3::new(toward.x, toward.y.max(0.0), toward.z);
    if level.dot(level) < FLUID_REFLECTION_DIRECTION_EPSILON {
        Vec3::new(ray.x, ray.y.max(0.0), ray.z)
    } else {
        level
    }
}

fn frozen_reflection_search_h(
    q: Vec3,
    capture_point: Vec3,
    tile: [u32; 2],
    distance: &impl Fn(Vec2) -> f32,
) -> f32 {
    let toward = q - capture_point;
    let level = Vec3::new(toward.x, toward.y.max(0.0), toward.z);
    let l1 = level.abs().element_sum();
    if l1 < FLUID_REFLECTION_SEARCH_EPSILON {
        return -1.0;
    }
    l1 * distance(reflection_tile_uv(level, tile[0], tile[1])) - 1.0
}

fn frozen_smoothstep(edge0: f32, edge1: f32, x: f32) -> f32 {
    let t = ((x - edge0) / (edge1 - edge0)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

fn frozen_reflection_shadow_fade_angle(tile: [u32; 2]) -> f32 {
    FLUID_REFLECTION_SHADOW_FADE_TEXELS * FLUID_REFLECTION_HALF_TURN
        / (tile[0] as f32 - 2.0 * tile[1] as f32)
}

fn frozen_reflection_shadow_tap(
    capture_point: Vec3,
    toward: Vec3,
    turn: f32,
    tile: [u32; 2],
    distance: &impl Fn(Vec2) -> f32,
) -> f32 {
    let turn2 = turn * turn;
    let sine = turn * (1.0 - turn2 / 6.0 * (1.0 - turn2 / 20.0));
    let cosine = 1.0 - turn2 / 2.0 * (1.0 - turn2 / 12.0);
    let turned = Vec3::new(
        toward.x * cosine + toward.z * sine,
        toward.y,
        toward.z * cosine - toward.x * sine,
    );
    frozen_reflection_search_h(capture_point + turned, capture_point, tile, distance)
}

#[allow(clippy::too_many_arguments)]
fn frozen_reflection_shadow_edge(
    capture_point: Vec3,
    toward: Vec3,
    angle: f32,
    side: f32,
    h_origin: f32,
    h_far: f32,
    h_opposite: f32,
    tile: [u32; 2],
    distance: &impl Fn(Vec2) -> f32,
) -> f32 {
    let step = (h_far - h_opposite) / (1.0 + h_opposite).max(FLUID_REFLECTION_SEARCH_EPSILON);
    let h_lit = h_origin.min(0.0);
    let gate = frozen_smoothstep(
        FLUID_REFLECTION_SHADOW_GATE_START * angle,
        FLUID_REFLECTION_SHADOW_GATE_FULL * angle,
        step,
    );
    if h_far <= 0.0 || gate <= 0.0 {
        return angle;
    }
    let h_half = frozen_reflection_shadow_tap(capture_point, toward, 0.5 * side, tile, distance);
    let crossing = if h_half > 0.0 {
        0.5 * angle * h_lit / (h_lit - h_half)
    } else {
        0.5 * angle + 0.5 * angle * h_half / (h_half - h_far)
    };
    angle + (crossing - angle) * gate
}

fn frozen_reflection_shadow_fade(
    origin: Vec3,
    capture_point: Vec3,
    tile: [u32; 2],
    h_origin: f32,
    distance: &impl Fn(Vec2) -> f32,
) -> f32 {
    let angle = frozen_reflection_shadow_fade_angle(tile);
    let toward = origin - capture_point;
    let h_plus = frozen_reflection_shadow_tap(capture_point, toward, angle, tile, distance);
    let h_minus = frozen_reflection_shadow_tap(capture_point, toward, -angle, tile, distance);
    let edge = frozen_reflection_shadow_edge(
        capture_point,
        toward,
        angle,
        angle,
        h_origin,
        h_plus,
        h_minus,
        tile,
        distance,
    )
    .min(frozen_reflection_shadow_edge(
        capture_point,
        toward,
        angle,
        -angle,
        h_origin,
        h_minus,
        h_plus,
        tile,
        distance,
    ));
    frozen_smoothstep(0.0, angle, edge)
}

fn frozen_reflection_search(
    origin: Vec3,
    ray: Vec3,
    box_min: Vec3,
    box_max: Vec3,
    capture_point: Vec3,
    tile: [u32; 2],
    distance: impl Fn(Vec2) -> f32,
) -> FluidReflectionSearch {
    let h = |q: Vec3| frozen_reflection_search_h(q, capture_point, tile, &distance);
    let box_hit = reflection_box_hit(origin, ray, box_min, box_max);
    let t_box = (box_hit - origin).dot(ray);
    let h_origin = h(origin);
    let h_box = h(box_hit);
    let ends_at = |point: Vec3, status, weight| FluidReflectionSearch {
        point,
        lookup: frozen_lookup(point, capture_point, ray),
        status,
        weight,
    };
    if h_origin >= FLUID_REFLECTION_SHADOW_MARGIN {
        return ends_at(box_hit, FluidReflectionSearchStatus::CaptureShadow, 0.0);
    }
    let weight = frozen_reflection_shadow_fade(origin, capture_point, tile, h_origin, &distance)
        * (1.0 - frozen_smoothstep(0.0, FLUID_REFLECTION_SHADOW_MARGIN, h_origin));
    if h_origin >= 0.0 {
        return ends_at(box_hit, FluidReflectionSearchStatus::CaptureShadow, weight);
    }

    // Front end of the bracket: the last sample in front of a surface.
    let (mut t_front, mut h_front) = (0.0, h_origin);
    let mut back = None;
    let from_capture = origin - capture_point;
    let b = from_capture.dot(ray);
    for _ in 0..FLUID_REFLECTION_SEARCH_STEPS {
        // The stored surface toward this sample, as a distance from the
        // capture point: |v|₂ / (|v|₁ · w), and |v|₁ · w is h + 1.
        if h_front <= -1.0 {
            break;
        }
        let q = origin + ray * t_front;
        let toward = q - capture_point;
        let level = Vec3::new(toward.x, toward.y.max(0.0), toward.z);
        let radius = level.length() / (h_front + 1.0);
        let disc = b * b - (from_capture.dot(from_capture) - radius * radius);
        if disc < 0.0 {
            break;
        }
        let t_next = -b + disc.sqrt();
        if t_next <= t_front || t_next >= t_box {
            break;
        }
        let h_next = h(origin + ray * t_next);
        if h_next >= 0.0 {
            back = Some((t_next, h_next));
            break;
        }
        (t_front, h_front) = (t_next, h_next);
    }
    let (mut t_back, mut h_back) = match back {
        Some(sample) => sample,
        None if h_box >= 0.0 => (t_box, h_box),
        None => return ends_at(box_hit, FluidReflectionSearchStatus::BeyondBox, weight),
    };

    let mut t_hit = t_back;
    for refinement in 0..FLUID_REFLECTION_SEARCH_REFINEMENTS {
        t_hit = if h_back != h_front {
            t_front - h_front * (t_back - t_front) / (h_back - h_front)
        } else {
            t_back
        };
        if refinement + 1 == FLUID_REFLECTION_SEARCH_REFINEMENTS {
            break;
        }
        let h_hit = h(origin + ray * t_hit);
        if h_hit >= 0.0 {
            (t_back, h_back) = (t_hit, h_hit);
        } else {
            (t_front, h_front) = (t_hit, h_hit);
        }
    }
    ends_at(
        origin + ray * t_hit,
        FluidReflectionSearchStatus::Hit,
        weight,
    )
}

// ---- Up-facing parity -------------------------------------------------------

fn finite_unit() -> impl Strategy<Value = Vec3> {
    (-1.0f32..1.0, -1.0f32..1.0, -1.0f32..1.0)
        .prop_map(|(x, y, z)| Vec3::new(x, y, z))
        .prop_filter("a direction", |v| v.length() > 1.0e-3)
        .prop_map(Vec3::normalize)
}

/// The two searches' every output, bit for bit up to a zero's sign.
fn assert_same_search(new: FluidReflectionSearch, old: FluidReflectionSearch, case: &str) {
    assert_eq!(new.status, old.status, "{case}");
    assert_eq!(new.weight, old.weight, "{case}");
    assert_eq!(new.point.to_array(), old.point.to_array(), "{case}");
    assert_eq!(new.lookup.to_array(), old.lookup.to_array(), "{case}");
    assert_eq!(
        reflection_tile_uv(new.lookup, TILE[0], TILE[1]).to_array(),
        reflection_tile_uv(old.lookup, TILE[0], TILE[1]).to_array(),
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

    // Up-facing probes, water's included, are unchanged by the hemisphere
    // generalization: about +Y, the search from any origin (at the water,
    // above or below it, inside the box or past it) along any ray, floored
    // above the horizon or not, returns what the search before it returned,
    // bit for bit — hit point, lookup, its tile UV, status and weight.
    #[test]
    fn up_facing_searches_match_the_search_before_the_hemisphere_bit_for_bit(
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
        let new = reflection_search_about(
            origin,
            ray,
            scene.box_min,
            scene.box_max,
            scene.capture,
            FLUID_REFLECTION_UP,
            TILE,
            distance,
        );
        let old = frozen_reflection_search(
            origin,
            ray,
            scene.box_min,
            scene.box_max,
            scene.capture,
            TILE,
            distance,
        );
        assert_same_search(new, old, &format!("{origin} along {ray}"));
        // Water's own entry is the same search.
        let water = reflection_search(
            origin,
            ray,
            scene.box_min,
            scene.box_max,
            scene.capture,
            TILE,
            distance,
        );
        assert_same_search(water, old, &format!("water: {origin} along {ray}"));
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
                frozen_lookup(hit, capture, ray).to_array()
            );
            assert_eq!(
                reflection_lookup(hit, capture, ray).to_array(),
                frozen_lookup(hit, capture, ray).to_array()
            );
        }
        let distance = |uv: Vec2| 0.25 + 0.5 * uv.x * uv.y;
        for turn in [0.05f32, -0.05, 0.025] {
            assert_eq!(
                reflection_shadow_tap(capture, v, turn, TILE, &distance),
                frozen_reflection_shadow_tap(capture, v, turn, TILE, &distance)
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
