// Fluid reflection distance search: box projection corrected against the
// probe's stored per-texel distance. GPU-free CPU reference for
// `fluid_surface.wgsl`'s search, which mirrors it and a test pins.
// See: context/lib/rendering_pipeline.md §7.3a (Reflection) · resource_management.md §5

use glam::{Vec2, Vec3};

use super::reflection::{reflection_box_hit, reflection_lookup, reflection_tile_uv};

/// K: sphere steps from the fragment before the box hit closes the bracket.
/// Each step moves to the point of the reflected ray as far from the capture
/// point as the surface the capture stored in the current sample's
/// direction. On the test map's crate three bring the reflected top within
/// half a capture texel from the coping and within 2° at the east rim; two
/// leave it up to 9° low at the rim, and a fourth changes nothing.
pub const FLUID_REFLECTION_SEARCH_STEPS: u32 = 3;
/// M: false-position estimates on the bracket. The last needs no tap, so
/// two cost one tap: worst case 2 + K + M − 1 = 6 distance taps. A third
/// changes neither test-map fixture beyond a fragment or two.
pub const FLUID_REFLECTION_SEARCH_REFINEMENTS: u32 = 2;
/// κ: the capture-shadow fade's width in h, the relative depth behind the
/// stored surface (0.05 is 5% beyond it). Wide enough that the shadow edge,
/// which bilinear filtering already softens over a texel, has no hard step;
/// narrow enough that a lit face never paints far into the wedge behind it.
pub const FLUID_REFLECTION_SHADOW_FADE: f32 = 0.05;
/// An offset from the capture point shorter than this (L1, m) has no
/// direction; the point counts as in front of every surface.
pub const FLUID_REFLECTION_SEARCH_EPSILON: f32 = 1.0e-6;

/// How the search ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FluidReflectionSearchStatus {
    /// A bracket closed and false position found the surface: the lookup is
    /// the hit.
    Hit,
    /// Nothing the capture stored lies along the ray inside the box (a
    /// doorway, a vista past the box, or a capture miss): the lookup is the
    /// box hit, plain box projection.
    BeyondBox,
    /// The fragment itself lies behind a surface the capture point saw: the
    /// capture holds nothing it could reflect, so the term fades out by
    /// `weight`, looked up toward the box hit.
    CaptureShadow,
}

/// The search's result for one fragment (or pixel-style cell).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct FluidReflectionSearch {
    /// Where the ray meets what the probe reflects: the search's hit, or the
    /// box hit.
    pub point: Vec3,
    /// The tile lookup direction: `point` minus the capture point, clamped to
    /// the horizon below it ([`reflection_lookup`]).
    pub lookup: Vec3,
    pub status: FluidReflectionSearchStatus,
    /// The probe term's weight: 1, except in the capture shadow, where it is
    /// `1 − smoothstep(0, κ, h(fragment))`.
    pub weight: f32,
}

/// `h(Q) = |v|₁ · w(v) − 1` for `v` the offset from the capture point to
/// `q`, clamped to the horizon as every tile read is: negative in front of
/// the surface the capture stored in that direction, at or above 0 behind
/// it. A stored 0 (no surface) gives −1, never a root. `distance` reads the
/// distance tile at a normalized tile UV.
pub fn reflection_search_h(
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

/// GLSL `smoothstep`.
fn smoothstep(edge0: f32, edge1: f32, x: f32) -> f32 {
    let t = ((x - edge0) / (edge1 - edge0)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

/// The distance search along unit `ray` from `origin` (the fragment, or the
/// pixel-style cell centre) inside the probe's box, against its distance
/// tile. A function of those inputs alone, so every fragment of a cell reads
/// one lookup.
///
/// 1. Two independent taps: at the origin and at the box hit. An origin at
///    or behind the stored surface (h ≥ 0) is in the capture shadow.
/// 2. Up to K sphere steps from the origin; the first sample behind a
///    surface closes the bracket. A sample in a capture miss, a step that
///    stalls, and a step reaching the box hit stop stepping.
/// 3. Otherwise the box hit closes the bracket when it lies behind a stored
///    surface; if not, nothing stored lies inside the box: the box hit.
/// 4. M false-position estimates on the bracket.
///
/// A wall on the box keeps box projection's answer: h is 0 at the box hit,
/// and false position returns it.
pub fn reflection_search(
    origin: Vec3,
    ray: Vec3,
    box_min: Vec3,
    box_max: Vec3,
    capture_point: Vec3,
    tile: [u32; 2],
    distance: impl Fn(Vec2) -> f32,
) -> FluidReflectionSearch {
    let h = |q: Vec3| reflection_search_h(q, capture_point, tile, &distance);
    let box_hit = reflection_box_hit(origin, ray, box_min, box_max);
    let t_box = (box_hit - origin).dot(ray);
    let h_origin = h(origin);
    let h_box = h(box_hit);
    let ends_at = |point: Vec3, status, weight| FluidReflectionSearch {
        point,
        lookup: reflection_lookup(point, capture_point, ray),
        status,
        weight,
    };
    if h_origin >= 0.0 {
        let weight = 1.0 - smoothstep(0.0, FLUID_REFLECTION_SHADOW_FADE, h_origin);
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
        None => return ends_at(box_hit, FluidReflectionSearchStatus::BeyondBox, 1.0),
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
    ends_at(origin + ray * t_hit, FluidReflectionSearchStatus::Hit, 1.0)
}

/// The distance tile as a linear-filtered, clamp-to-edge sampler reads it at
/// level 0: bilinear between the four texel centres around normalized `uv`.
/// `texels` is the tile row-major, border included, decoded to f32.
pub fn reflection_distance_sample(texels: &[f32], dimension: u32, uv: Vec2) -> f32 {
    let last = dimension as i32 - 1;
    let position = uv * dimension as f32 - Vec2::splat(0.5);
    let base = position.floor();
    let fraction = position - base;
    let at = |x: i32, y: i32| {
        let (x, y) = (x.clamp(0, last) as usize, y.clamp(0, last) as usize);
        texels[y * dimension as usize + x]
    };
    let (x0, y0) = (base.x as i32, base.y as i32);
    let top = at(x0, y0) * (1.0 - fraction.x) + at(x0 + 1, y0) * fraction.x;
    let bottom = at(x0, y0 + 1) * (1.0 - fraction.x) + at(x0 + 1, y0 + 1) * fraction.x;
    top * (1.0 - fraction.y) + bottom * fraction.y
}

#[cfg(test)]
#[path = "reflection_search_test_fixtures.rs"]
mod fixtures;

#[cfg(test)]
#[path = "reflection_search_tests.rs"]
mod tests;
