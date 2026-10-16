// Fluid reflection distance search: box projection corrected against the
// probe's stored per-texel distance. GPU-free CPU reference for
// `fluid_surface.wgsl`'s search, which mirrors it and a test pins.
// See: context/lib/rendering_pipeline.md §7.3a (Reflection) · resource_management.md §5

use glam::{Vec2, Vec3};

use super::reflection::{reflection_box_hit, reflection_lookup, reflection_tile_uv};

/// K: sphere steps from the fragment before the box hit closes the bracket.
/// Each step moves to the point of the reflected ray as far from the capture
/// point as the surface the capture stored in the current sample's
/// direction. Three is enough for the reflected crate to track the true mirror
/// (`the_crate_top_edge_from_the_west_coping_matches_the_mirror`,
/// `the_crate_top_elevation_at_the_east_rim_matches_the_mirror`); a fourth
/// changes nothing.
pub const FLUID_REFLECTION_SEARCH_STEPS: u32 = 3;
/// M: false-position estimates on the bracket. The last needs no tap, so
/// two cost one tap: 2 + K + M − 1 = 6 search taps, beside the capture-shadow
/// fade's two to four. Two already settle the crate fixtures
/// (`the_crate_top_elevation_at_the_east_rim_matches_the_mirror`); a third
/// buys nothing visible.
pub const FLUID_REFLECTION_SEARCH_REFINEMENTS: u32 = 2;
/// N: the capture-shadow fade's width in capture texels of angle about the
/// capture point. One interior texel spans about π / (dimension − 2·border)
/// of azimuth, a half-turn of horizon crossing the interior (≈1.43° at the
/// 128² tile), so the fade follows the tile dimension the section header
/// names. Two texels soften the edge the stored distance's one bilinear
/// crossing draws, without eating far into the reflection beside the
/// occluder.
pub const FLUID_REFLECTION_SHADOW_FADE_TEXELS: f32 = 2.0;
/// The fade's occluder gate on one side, in fade angles: the side tap's
/// stored surface must stand nearer than the opposite side tap's by this
/// relative step (`H_side / H_opposite − 1`, `H = h + 1`) to fade at all, and
/// by the second to fade fully. Turning ± the fade angle across a plane seen
/// at φ from its normal steps about 2·tan φ fade angles, so a wall's foot
/// keeps its full term up to about 68° from the wall's normal; an occluder
/// fades fully, and continuously into its shadow, once its background lies
/// 10 fade angles (≈50% at the 128² tile) farther. A fragment within about
/// 30% of its background's distance opens the gate a little late, as the
/// far tap climbs the occluder's edge: its band narrows toward 1.5 texels.
pub const FLUID_REFLECTION_SHADOW_GATE_START: f32 = 6.0;
pub const FLUID_REFLECTION_SHADOW_GATE_FULL: f32 = 10.0;
/// κ: the capture shadow's depth margin, in h: a fragment this little behind
/// the stored surface (5% beyond it) still draws, fading out over it. It
/// absorbs the filtered distance reading a concave corner or a wall's foot a
/// little near; beside an occluder the angular fade has reached 0 already.
pub const FLUID_REFLECTION_SHADOW_MARGIN: f32 = 0.05;
/// π: the azimuth a half-turn of horizon spans across the tile's interior.
pub const FLUID_REFLECTION_HALF_TURN: f32 = std::f32::consts::PI;
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
    /// The search's start (the fragment, clamped into the box) lies behind a
    /// surface the capture point saw: the capture holds nothing it could
    /// reflect, so it draws no term past the depth margin (`weight` 0),
    /// looked up toward the box hit.
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
    /// The probe term's weight: 0 in the capture shadow, fading up to 1 over
    /// the N capture texels of angle beside it
    /// ([`reflection_shadow_fade`]) and over the depth margin κ behind the
    /// stored surface, 1 everywhere else.
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

/// The capture-shadow fade's angle in radians for a tile (dimension,
/// border): N interior texels of π / interior each.
pub fn reflection_shadow_fade_angle(tile: [u32; 2]) -> f32 {
    FLUID_REFLECTION_SHADOW_FADE_TEXELS * FLUID_REFLECTION_HALF_TURN
        / (tile[0] as f32 - 2.0 * tile[1] as f32)
}

/// h at the origin turned about the capture point's vertical by `turn`
/// radians, keeping its distance from the capture point. `toward` is the
/// origin minus the capture point. The fluid shader calls no trig: the turn's
/// sine and cosine are their Taylor series to the fifth and fourth order,
/// within 1e-8 of exact at the fade's ≈3°.
pub fn reflection_shadow_tap(
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
    reflection_search_h(capture_point + turned, capture_point, tile, distance)
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
pub fn reflection_shadow_edge(
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
    let gate = smoothstep(
        FLUID_REFLECTION_SHADOW_GATE_START * angle,
        FLUID_REFLECTION_SHADOW_GATE_FULL * angle,
        step,
    );
    if h_far <= 0.0 || gate <= 0.0 {
        return angle;
    }
    let h_half = reflection_shadow_tap(capture_point, toward, 0.5 * side, tile, distance);
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
/// vertical by the angle, keeping its distance; each side then places its
/// edge ([`reflection_shadow_edge`]), and the nearer counts. Two taps with no
/// edge near, three with one, four with one each side. A function of the
/// origin, the capture point and the tile alone.
pub fn reflection_shadow_fade(
    origin: Vec3,
    capture_point: Vec3,
    tile: [u32; 2],
    h_origin: f32,
    distance: &impl Fn(Vec2) -> f32,
) -> f32 {
    let angle = reflection_shadow_fade_angle(tile);
    let toward = origin - capture_point;
    let h_plus = reflection_shadow_tap(capture_point, toward, angle, tile, distance);
    let h_minus = reflection_shadow_tap(capture_point, toward, -angle, tile, distance);
    let edge = reflection_shadow_edge(
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
    .min(reflection_shadow_edge(
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
    smoothstep(0.0, angle, edge)
}

/// The distance search along unit `ray` from `origin` (the fragment, or the
/// pixel-style cell centre) against the probe's box and distance tile. A
/// function of those inputs alone, so every fragment of a cell reads one
/// lookup.
///
/// 0. The search starts from the origin clamped into the box, keeping the
///    ray. An origin past a side face (a straddling face's water outside an
///    authored box, a pixel cell centred inside the wall at a rim) starts on
///    that face, and a derived box's floor, the capture plane 2 mm above the
///    water, lifts every origin onto it. A start the clamp moved onto a side
///    face lies on what the capture saw there (a derived box's wall): within
///    the depth margin it reads as on that stored surface, never behind it,
///    so a cell straddling a wall keeps its term and its search reads the
///    wall's foot.
/// 1. Two independent taps: at the start and at the box hit. A start at or
///    behind the stored surface (h ≥ 0) is in the capture shadow: no term
///    past the depth margin κ. Otherwise two to four more taps, independent
///    of the search's, fade the term beside the shadow
///    ([`reflection_shadow_fade`]); within κ the term also fades out by
///    depth, looked up toward the box hit.
/// 2. Up to K sphere steps from the start; the first sample behind a surface
///    closes the bracket. A sample in a capture miss, a step that stalls,
///    and a step reaching the box hit stop stepping.
/// 3. Otherwise the box hit closes the bracket when it lies behind a stored
///    surface; if not, nothing stored lies inside the box: the box hit.
/// 4. M false-position estimates on the bracket.
///
/// A wall on the box keeps box projection's answer: h is 0 at the box hit,
/// and false position returns it. A start on a face the ray leaves through
/// leaves the box through another ([`reflection_box_hit`]), so water outside
/// an authored box resolves along its ray rather than at its own azimuth.
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
    let start = origin.clamp(box_min, box_max);
    let on_side = start.x != origin.x || start.z != origin.z;
    let box_hit = reflection_box_hit(start, ray, box_min, box_max);
    let t_box = (box_hit - start).dot(ray);
    let h_start = h(start);
    let h_box = h(box_hit);
    let h_origin = if on_side && h_start < FLUID_REFLECTION_SHADOW_MARGIN {
        h_start.min(0.0)
    } else {
        h_start
    };
    let ends_at = |point: Vec3, status, weight| FluidReflectionSearch {
        point,
        lookup: reflection_lookup(point, capture_point, ray),
        status,
        weight,
    };
    if h_origin >= FLUID_REFLECTION_SHADOW_MARGIN {
        return ends_at(box_hit, FluidReflectionSearchStatus::CaptureShadow, 0.0);
    }
    let weight = reflection_shadow_fade(start, capture_point, tile, h_origin, &distance)
        * (1.0 - smoothstep(0.0, FLUID_REFLECTION_SHADOW_MARGIN, h_origin));
    if h_origin >= 0.0 && !on_side {
        return ends_at(box_hit, FluidReflectionSearchStatus::CaptureShadow, weight);
    }

    // Front end of the bracket: the last sample in front of a surface (or,
    // for a start on a side face, on it).
    let (mut t_front, mut h_front) = (0.0, h_origin);
    let mut back = None;
    let from_capture = start - capture_point;
    let b = from_capture.dot(ray);
    for _ in 0..FLUID_REFLECTION_SEARCH_STEPS {
        // The stored surface toward this sample, as a distance from the
        // capture point: |v|₂ / (|v|₁ · w), and |v|₁ · w is h + 1.
        if h_front <= -1.0 {
            break;
        }
        let q = start + ray * t_front;
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
        let h_next = h(start + ray * t_next);
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
        let h_hit = h(start + ray * t_hit);
        if h_hit >= 0.0 {
            (t_back, h_back) = (t_hit, h_hit);
        } else {
            (t_front, h_front) = (t_hit, h_hit);
        }
    }
    ends_at(
        start + ray * t_hit,
        FluidReflectionSearchStatus::Hit,
        weight,
    )
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
