// The distance search about a bound face's hemisphere axis: the glossy
// overlay's, whose faces bind probes of any facing. GPU-free CPU reference
// for `fluid_reflection_about.wgsl`, which mirrors it and a test pins.
// Water's search, up-facing only, is `reflection_search`; about +Y the two
// agree bit for bit (`reflection_search_up_tests.rs`).
// See: context/lib/rendering_pipeline.md §7.3a (Reflection, Glossy world faces)

use glam::{Vec2, Vec3};

use super::reflection::{reflection_box_hit, reflection_tile_uv};
use super::reflection_hemisphere::{reflection_level, reflection_lookup_about};
use super::reflection_search::{
    FLUID_REFLECTION_SEARCH_EPSILON, FLUID_REFLECTION_SEARCH_REFINEMENTS,
    FLUID_REFLECTION_SEARCH_STEPS, FLUID_REFLECTION_SHADOW_GATE_FULL,
    FLUID_REFLECTION_SHADOW_GATE_START, FLUID_REFLECTION_SHADOW_MARGIN, FluidReflectionSearch,
    FluidReflectionSearchStatus, reflection_shadow_fade_angle, smoothstep,
};

/// `h(Q) = |v|₁ · w(v) − 1` for `v` the offset from the capture point to
/// `q`, clamped to the hemisphere about `axis` as every tile read is (the
/// horizon, for an up-facing probe): negative in front of the surface the
/// capture stored in that direction, at or above 0 behind it. A stored 0 (no
/// surface) gives −1, never a root. `distance` reads the distance tile at a
/// normalized tile UV.
pub fn reflection_search_h_about(
    q: Vec3,
    capture_point: Vec3,
    axis: Vec3,
    tile: [u32; 2],
    distance: &impl Fn(Vec2) -> f32,
) -> f32 {
    let level = reflection_level(q - capture_point, axis);
    let l1 = level.abs().element_sum();
    if l1 < FLUID_REFLECTION_SEARCH_EPSILON {
        return -1.0;
    }
    l1 * distance(reflection_tile_uv(level, tile[0], tile[1])) - 1.0
}

/// h at the origin turned about the capture point's `axis` (its vertical,
/// for an up-facing probe) by `turn` radians, keeping its distance from the
/// capture point. `toward` is the origin minus the capture point. The fluid
/// shader calls no trig: the turn's sine and cosine are their Taylor series
/// to the fifth and fourth order, within 1e-8 of exact at the fade's ≈3°.
/// About +Y the turn is exactly `(x·cos + z·sin, y, z·cos − x·sin)`.
pub fn reflection_shadow_tap_about(
    capture_point: Vec3,
    toward: Vec3,
    turn: f32,
    axis: Vec3,
    tile: [u32; 2],
    distance: &impl Fn(Vec2) -> f32,
) -> f32 {
    let turn2 = turn * turn;
    let sine = turn * (1.0 - turn2 / 6.0 * (1.0 - turn2 / 20.0));
    let cosine = 1.0 - turn2 / 2.0 * (1.0 - turn2 / 12.0);
    let along = axis * toward.dot(axis);
    let across = toward - along;
    let turned = along + across * cosine + axis.cross(across) * sine;
    reflection_search_h_about(capture_point + turned, capture_point, axis, tile, distance)
}

/// One side's shadow edge for the fade, in radians from the origin: at most
/// `angle`, and 0 for an origin behind the stored surface. `side` (± `angle`)
/// turns toward that side, `h_far` is the tap there and `h_opposite` the tap
/// on the other side. Behind its stored surface, the far tap puts an edge on
/// this side, and a tap at half the turn brackets it to a texel, where h,
/// interpolated linearly, crosses 0: the stored distance's bilinear ramp
/// across a silhouette spans one to two texels, so samples a texel apart
/// leave no stretch of the band unread. The gate weighs the edge by how far
/// the far tap's stored surface stands in front of the opposite one's: an
/// occluder against its background, not a wall seen obliquely, which a turn
/// also reaches behind. A side the gate shuts takes no half tap.
#[allow(clippy::too_many_arguments)]
pub fn reflection_shadow_edge_about(
    capture_point: Vec3,
    toward: Vec3,
    angle: f32,
    side: f32,
    h_origin: f32,
    h_far: f32,
    h_opposite: f32,
    axis: Vec3,
    tile: [u32; 2],
    distance: &impl Fn(Vec2) -> f32,
) -> f32 {
    let step = (h_far - h_opposite) / (1.0 + h_opposite).max(FLUID_REFLECTION_SEARCH_EPSILON);
    let h_lit = h_origin.min(0.0);
    let gate = smoothstep(
        FLUID_REFLECTION_SHADOW_GATE_START * angle,
        FLUID_REFLECTION_SHADOW_GATE_FULL * angle,
        step,
    );
    if h_far <= 0.0 || gate <= 0.0 {
        return angle;
    }
    let h_half =
        reflection_shadow_tap_about(capture_point, toward, 0.5 * side, axis, tile, distance);
    let crossing = if h_half > 0.0 {
        0.5 * angle * h_lit / (h_lit - h_half)
    } else {
        0.5 * angle + 0.5 * angle * h_half / (h_half - h_far)
    };
    angle + (crossing - angle) * gate
}

/// The capture-shadow fade for an origin in front of the stored surface or
/// within the depth margin behind it: 1 with no shadow edge within the fade
/// angle about the capture point, falling smoothly to 0 at the edge, where
/// the shadow's 0 takes over (an origin behind the surface sits at the
/// edge). A tap on each side turns the origin about the capture point's
/// `axis` (its vertical, for an up-facing probe) by the angle, keeping its
/// distance; each side then places its edge
/// ([`reflection_shadow_edge_about`]), and the nearer counts. Two taps with
/// no edge near, three with one, four with one each side. A function of the
/// origin, the capture point, the axis and the tile alone.
pub fn reflection_shadow_fade_about(
    origin: Vec3,
    capture_point: Vec3,
    axis: Vec3,
    tile: [u32; 2],
    h_origin: f32,
    distance: &impl Fn(Vec2) -> f32,
) -> f32 {
    let angle = reflection_shadow_fade_angle(tile);
    let toward = origin - capture_point;
    let h_plus = reflection_shadow_tap_about(capture_point, toward, angle, axis, tile, distance);
    let h_minus = reflection_shadow_tap_about(capture_point, toward, -angle, axis, tile, distance);
    let edge = reflection_shadow_edge_about(
        capture_point,
        toward,
        angle,
        angle,
        h_origin,
        h_plus,
        h_minus,
        axis,
        tile,
        distance,
    )
    .min(reflection_shadow_edge_about(
        capture_point,
        toward,
        angle,
        -angle,
        h_origin,
        h_minus,
        h_plus,
        axis,
        tile,
        distance,
    ));
    smoothstep(0.0, angle, edge)
}

/// The distance search along unit `ray` from `origin` (the fragment, or the
/// pixel-style cell's point) against the probe's box and distance tile. A
/// function of those inputs alone, so every fragment of a cell reads one
/// lookup. The search starts at the origin itself, inside the box or past
/// it (water outside an authored box, a fragment below a derived box's
/// floor), keeping its parallax along the ray.
///
/// 1. Two independent taps: at the origin and at the box hit. An origin at
///    or behind the stored surface (h ≥ 0) is in the capture shadow: no
///    term past the depth margin κ. Otherwise two to four more taps,
///    independent of the search's, fade the term beside the shadow
///    ([`reflection_shadow_fade_about`]); within κ the term also fades out by
///    depth, looked up toward the box hit. The compiler's hidden-water
///    warning tests the same raw water point against the same cutoff.
/// 2. Up to K sphere steps from the origin; the first sample behind a
///    surface closes the bracket. A sample in a capture miss, a step that
///    stalls, and a step reaching the box hit stop stepping.
/// 3. Otherwise the box hit closes the bracket when it lies behind a stored
///    surface; if not, nothing stored lies inside the box: the box hit.
/// 4. M false-position estimates on the bracket.
///
/// A wall on the box keeps box projection's answer: h is 0 at the box hit,
/// and false position returns it. An origin past a face the ray leaves
/// through leaves the box through another slab ([`reflection_box_hit`]), so
/// water outside an authored box resolves along its ray rather than at its
/// own azimuth.
///
/// Every tap and the lookup clamp to the hemisphere about `axis`: the
/// horizon for an up-facing probe
/// ([`FLUID_REFLECTION_UP`](super::reflection_hemisphere::FLUID_REFLECTION_UP)), a wall's,
/// ceiling's or slope's face normal otherwise. The axis is the bound face's,
/// never a lookup direction's.
#[allow(clippy::too_many_arguments)]
pub fn reflection_search_about(
    origin: Vec3,
    ray: Vec3,
    box_min: Vec3,
    box_max: Vec3,
    capture_point: Vec3,
    axis: Vec3,
    tile: [u32; 2],
    distance: impl Fn(Vec2) -> f32,
) -> FluidReflectionSearch {
    let h = |q: Vec3| reflection_search_h_about(q, capture_point, axis, tile, &distance);
    let box_hit = reflection_box_hit(origin, ray, box_min, box_max);
    let t_box = (box_hit - origin).dot(ray);
    let h_origin = h(origin);
    let h_box = h(box_hit);
    let ends_at = |point: Vec3, status, weight| FluidReflectionSearch {
        point,
        lookup: reflection_lookup_about(point, capture_point, ray, axis),
        status,
        weight,
    };
    if h_origin >= FLUID_REFLECTION_SHADOW_MARGIN {
        return ends_at(box_hit, FluidReflectionSearchStatus::CaptureShadow, 0.0);
    }
    let weight =
        reflection_shadow_fade_about(origin, capture_point, axis, tile, h_origin, &distance)
            * (1.0 - smoothstep(0.0, FLUID_REFLECTION_SHADOW_MARGIN, h_origin));
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
        let level = reflection_level(q - capture_point, axis);
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
