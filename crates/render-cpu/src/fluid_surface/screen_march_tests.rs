// Tests: the screen-space march reference on a fixed synthetic depth buffer.
// Every tier reaches the same hit, a thin occluder included; misses, the
// budget and the edge band set confidence; a ray toward the camera steps like
// any other; a hidden origin skips its starting overlap.
// See: context/lib/testing_guide.md

use std::cell::RefCell;

use glam::{Mat4, UVec2, Vec2, Vec3};
use proptest::prelude::*;

use super::*;

/// The synthetic scene depth's extent: small, so every column and row the
/// tests name is easy to reason about.
const EXTENT: UVec2 = UVec2::new(64, 48);
/// One full-resolution pixel per step: the reflection pass's stride below
/// 1080 scene rows (`max(1, rows / 540)`).
const STRIDE: f32 = 1.0;
const BUDGET: u32 = 64;
const THICKNESS: f32 = 0.5;
/// The eye, 2 m above the water (the plane y = 0), looking down the pool.
const EYE: Vec3 = Vec3::new(0.0, 2.0, 0.0);

fn view_proj() -> Mat4 {
    glam::camera::rh::proj::directx::perspective(
        60f32.to_radians(),
        EXTENT.x as f32 / EXTENT.y as f32,
        0.1,
        100.0,
    ) * glam::camera::rh::view::look_at_mat4(EYE, Vec3::new(0.0, 0.0, -6.0), Vec3::Y)
}

/// NDC depth of a world point.
fn ndc_depth(point: Vec3) -> f32 {
    let clip = view_proj() * point.extend(1.0);
    clip.z / clip.w
}

/// View depth (clip w) of a world point.
fn view_depth(point: Vec3) -> f32 {
    (view_proj() * point.extend(1.0)).w
}

/// The full-resolution pixel position a world point projects to.
fn pixel_of(point: Vec3) -> Vec2 {
    screen_march_pixel(view_proj() * point.extend(1.0), EXTENT.as_vec2())
}

/// The water point under a full-resolution screen position.
fn water_point(pixel: Vec2) -> Vec3 {
    let ndc = Vec2::new(
        pixel.x / EXTENT.x as f32 * 2.0 - 1.0,
        1.0 - pixel.y / EXTENT.y as f32 * 2.0,
    );
    let inverse = view_proj().inverse();
    let near = inverse.project_point3(ndc.extend(0.0));
    let far = inverse.project_point3(ndc.extend(1.0));
    near + (far - near) * (near.y / (near.y - far.y))
}

/// The ray the water at `origin` reflects toward the eye, about the flat
/// normal.
fn mirrored(origin: Vec3) -> Vec3 {
    let view = (EYE - origin).normalize();
    Vec3::Y * (2.0 * view.dot(Vec3::Y)) - view
}

/// A depth buffer holding nothing: every texel at the far clear, 1.
fn empty_depth() -> Vec<f32> {
    vec![1.0; (EXTENT.x * EXTENT.y) as usize]
}

fn buffer(texels: &[f32]) -> ScreenMarchDepthBuffer<'_> {
    ScreenMarchDepthBuffer {
        texels,
        dimensions: EXTENT,
    }
}

/// Write `depth` into every texel of `texels` at `rows` × `columns`.
fn fill(texels: &mut [f32], rows: std::ops::Range<u32>, columns: std::ops::Range<u32>, depth: f32) {
    for row in rows {
        for column in columns.clone() {
            texels[(row * EXTENT.x + column) as usize] = depth;
        }
    }
}

/// A depth buffer that records the exact position of every sample the march
/// loads: the march's own loads, so its step sequence is observed, not
/// re-derived.
struct Recording<'a> {
    depth: ScreenMarchDepthBuffer<'a>,
    loads: RefCell<Vec<Vec2>>,
}

impl<'a> Recording<'a> {
    fn new(texels: &'a [f32]) -> Self {
        Self {
            depth: buffer(texels),
            loads: RefCell::new(Vec::new()),
        }
    }

    /// The screen-space length of each step between loaded samples.
    fn step_lengths(&self) -> Vec<f32> {
        self.loads
            .borrow()
            .windows(2)
            .map(|pair| pair[0].distance(pair[1]))
            .collect()
    }
}

impl ScreenMarchDepth for Recording<'_> {
    fn dimensions(&self) -> UVec2 {
        self.depth.dimensions()
    }

    fn load(&self, pixel: Vec2) -> f32 {
        self.loads.borrow_mut().push(pixel);
        self.depth.load(pixel)
    }
}

fn march(origin: Vec3, direction: Vec3, depth: &impl ScreenMarchDepth) -> ScreenMarch {
    screen_march(
        origin,
        direction,
        view_proj(),
        depth,
        STRIDE,
        BUDGET,
        THICKNESS,
    )
}

const MISS: ScreenMarch = ScreenMarch {
    pixel: Vec2::ZERO,
    confidence: 0.0,
};

/// The point on the ray from `origin` along `direction` whose projection
/// lies on `row`'s centre line, found by bisection: the ray climbs the
/// screen as it travels.
fn ray_point_at_row(origin: Vec3, direction: Vec3, row: u32) -> Vec3 {
    let target = row as f32 + 0.5;
    let (mut near, mut far) = (0.0f32, 200.0f32);
    for _ in 0..80 {
        let mid = 0.5 * (near + far);
        if pixel_of(origin + direction * mid).y > target {
            near = mid;
        } else {
            far = mid;
        }
    }
    origin + direction * near
}

/// The NDC depth of every point at view depth `w`: the point on the view
/// axis there, since a perspective depth is a function of view depth alone.
fn ndc_at_view_depth(w: f32) -> f32 {
    let forward = (Vec3::new(0.0, 0.0, -6.0) - EYE).normalize();
    ndc_depth(EYE + forward * w)
}

/// A depth texel standing `gap` metres of view depth in front of the ray's
/// point on `row`: the ray passes `gap` behind it there.
fn depth_in_front_of_ray(origin: Vec3, direction: Vec3, row: u32, gap: f32) -> f32 {
    ndc_at_view_depth(view_depth(ray_point_at_row(origin, direction, row)) - gap)
}

// --- Tier invariance --------------------------------------------------------

// Every tier marches the same full-resolution scene depth at the same
// full-resolution stride: the reflection pass sets
// `stride = max(1, scene_rows / 540)` from the scene's extent, never the
// target's, and binds scene depth, never a target-resolution copy. Neither
// the target extent nor the tier is an input of `screen_march`. A tier only
// changes which origins are shaded: the low tier (a quarter per axis) shades
// one per 4 × 4 block of scene pixels, at the block's centre; the high tier
// one per scene pixel. So the low tier's origin, marched with the inputs
// each tier's pass hands the march, reaches the same hit at the same
// confidence, and so does every high-tier origin in that block, on a bar
// standing across the pool one depth texel tall. A one-pixel stride along a
// mostly vertical screen path samples every row and cannot step over it. The
// bar stands a few rows above the block, where the block's rays cross it
// within the thickness of one another. A march that scaled its stride with
// the tier (four pixels at the quarter tier) steps over the same bar, so the
// bar is thin for what this test claims.
#[test]
fn every_tier_reaches_the_same_hit_on_a_one_texel_occluder() {
    // The low tier's target pixel (4, 8) covers scene pixels 16..20 × 32..36;
    // its fragment shades the water at the block's centre.
    let low_pixel = UVec2::new(4, 8);
    let low_origin = water_point(low_pixel.as_vec2() * 4.0 + Vec2::splat(2.0));
    let direction = mirrored(low_origin);
    let block: Vec<Vec3> = (32..36)
        .flat_map(|y| (16..20).map(move |x| Vec2::new(x as f32 + 0.5, y as f32 + 0.5)))
        .map(water_point)
        .collect();
    // The bar: row 28, every column, just in front of where each of the
    // block's rays crosses that row, and within the thickness of each.
    let bar_row = 28;
    let crossings: Vec<f32> = std::iter::once(low_origin)
        .chain(block.iter().copied())
        .map(|origin| view_depth(ray_point_at_row(origin, mirrored(origin), bar_row)))
        .collect();
    let nearest = crossings.iter().copied().fold(f32::MAX, f32::min);
    let farthest = crossings.iter().copied().fold(f32::MIN, f32::max);
    assert!(
        farthest - nearest < THICKNESS - 0.1,
        "{nearest}..{farthest}"
    );
    let mut texels = empty_depth();
    let bar = ndc_at_view_depth(nearest - 0.05);
    fill(&mut texels, bar_row..bar_row + 1, 0..EXTENT.x, bar);

    // Each tier's pass hands the march the same scene depth, stride, budget
    // and thickness; its own target extent is no input.
    let mut ends = Vec::new();
    for tier in ["high", "medium", "low"] {
        let end = march(low_origin, direction, &buffer(&texels));
        assert_eq!(end.pixel.y.floor() as u32, bar_row, "{tier}: {end:?}");
        assert!(end.confidence > 0.9, "{tier}: {end:?}");
        ends.push(end);
    }
    assert!(ends.iter().all(|end| *end == ends[0]), "{ends:?}");

    // Every high-tier origin in the block hits the same bar.
    for origin in block {
        let hit = march(origin, mirrored(origin), &buffer(&texels));
        assert_eq!(hit.pixel.y.floor() as u32, bar_row, "{origin}: {hit:?}");
        assert!(hit.confidence > 0.9, "{origin}: {hit:?}");
    }

    // The bar is thin: the same ray marched at the low tier's own stride
    // samples no texel of it.
    let scaled = Recording::new(&texels);
    let jumped = screen_march(
        low_origin,
        direction,
        view_proj(),
        &scaled,
        4.0 * STRIDE,
        BUDGET,
        THICKNESS,
    );
    assert!(
        scaled
            .loads
            .borrow()
            .iter()
            .all(|pixel| pixel.y.floor() as u32 != bar_row),
        "a four-pixel stride steps over the bar: {:?}",
        scaled.loads.borrow()
    );
    assert_eq!(jumped, MISS);
}

// --- Confidence -------------------------------------------------------------

// A ray leaving the screen and a ray exhausting its budget each end with
// c = 0; a hit inside the edge band, or on the budget's last stretch, lies
// strictly between 0 and 1, and a hit clear of both reads 1.
#[test]
fn a_miss_reads_zero_and_the_edge_band_and_reach_fade_lie_between() {
    let texels = empty_depth();
    // Sideways along the water: off the right edge with nothing drawn.
    let origin = water_point(Vec2::new(32.5, 40.5));
    let sideways = Recording::new(&texels);
    assert_eq!(march(origin, Vec3::X, &sideways), MISS);
    let last = *sideways.loads.borrow().last().expect("the ray stepped");
    assert!(
        last.x + STRIDE >= EXTENT.x as f32 && (sideways.loads.borrow().len() as u32) < BUDGET,
        "it left the screen before its budget: {last}"
    );

    // A bar the ray reaches only after its budget.
    let direction = mirrored(origin);
    let bar_row = 6;
    let mut far_bar = empty_depth();
    let bar = depth_in_front_of_ray(origin, direction, bar_row, 0.2);
    fill(&mut far_bar, bar_row..bar_row + 1, 0..EXTENT.x, bar);
    let reached = march(origin, direction, &buffer(&far_bar));
    assert_eq!(reached.pixel.y.floor() as u32, bar_row, "{reached:?}");
    let steps_to_bar = (pixel_of(origin).y - reached.pixel.y) / STRIDE;
    let short_budget = (steps_to_bar * 0.5) as u32;
    let short_loads = Recording::new(&far_bar);
    let short = screen_march(
        origin,
        direction,
        view_proj(),
        &short_loads,
        STRIDE,
        short_budget,
        THICKNESS,
    );
    assert_eq!(short, MISS, "the budget ran out before the bar");
    assert_eq!(
        short_loads.loads.borrow().len() as u32,
        short_budget,
        "every step of the budget was taken, on screen and in range"
    );

    // The same bar with a budget it reaches on its last stretch.
    let tight = screen_march(
        origin,
        direction,
        view_proj(),
        &buffer(&far_bar),
        STRIDE,
        steps_to_bar as u32 + 2,
        THICKNESS,
    );
    assert_eq!(tight.pixel.y.floor() as u32, bar_row, "{tight:?}");
    assert!(
        tight.confidence > 0.0 && tight.confidence < 1.0,
        "reach fade: {tight:?}"
    );

    // A bar inside the top edge band (10% of 48 rows: rows 0..4.8).
    let edge_row = 2;
    let mut edge_bar = empty_depth();
    let bar = depth_in_front_of_ray(origin, direction, edge_row, 0.2);
    fill(&mut edge_bar, edge_row..edge_row + 1, 0..EXTENT.x, bar);
    let faded = march(origin, direction, &buffer(&edge_bar));
    assert_eq!(faded.pixel.y.floor() as u32, edge_row, "{faded:?}");
    assert!(
        faded.confidence > 0.0 && faded.confidence < 1.0,
        "edge fade: {faded:?}"
    );
    let expected = (faded.pixel.y / EXTENT.y as f32) / SCREEN_MARCH_EDGE_FADE;
    assert!(
        (faded.confidence - expected).abs() < 1.0e-5,
        "{faded:?} vs {expected}"
    );

    // Clear of the band and the reach fade: full confidence.
    let mid_row = 24;
    let mut mid_bar = empty_depth();
    let bar = depth_in_front_of_ray(origin, direction, mid_row, 0.2);
    fill(&mut mid_bar, mid_row..mid_row + 1, 0..EXTENT.x, bar);
    let full = march(origin, direction, &buffer(&mid_bar));
    assert_eq!(full.pixel.y.floor() as u32, mid_row, "{full:?}");
    assert_eq!(full.confidence, 1.0);
}

// --- Direction ---------------------------------------------------------------

// Pin P11. A ray heading toward the camera takes the same screen-space step
// as one heading away: every step between the samples the march loads is one
// stride long, for both. The toward ray then leaves the depth range or the
// screen and ends as the same miss a ray leaving the screen sideways ends
// as, before its budget. The step lengths are read from the march's own
// depth loads.
#[test]
fn a_ray_toward_the_camera_steps_like_one_heading_away_and_ends_as_a_miss() {
    let texels = empty_depth();
    let origin = water_point(Vec2::new(30.5, 30.5));
    // Toward the camera: back up past the eye's side, under the near plane.
    let toward = (EYE + Vec3::new(1.5, -0.5, 0.0) - origin).normalize();
    let forward = (Vec3::new(0.0, 0.0, -6.0) - EYE).normalize();
    assert!(toward.dot(forward) < 0.0, "the ray heads toward the camera");
    let away = -toward;

    let toward_loads = Recording::new(&texels);
    let away_loads = Recording::new(&texels);
    let toward_end = march(origin, toward, &toward_loads);
    let away_end = march(origin, away, &away_loads);
    for (name, loads) in [("toward", &toward_loads), ("away", &away_loads)] {
        let steps = loads.step_lengths();
        assert!(steps.len() >= 4, "{name} took {} steps", steps.len());
        for (index, step) in steps.iter().enumerate() {
            assert!(
                (step - STRIDE).abs() < 1.0e-3,
                "{name} step {index} is {step} pixels"
            );
        }
    }
    assert_eq!(toward_end, MISS);
    assert_eq!(away_end, MISS);
    assert!(
        (toward_loads.loads.borrow().len() as u32) < BUDGET,
        "the toward ray left before its budget"
    );
}

// --- Hidden origin ----------------------------------------------------------

// Pin P15. An origin hidden behind an object standing in the water starts
// behind that object's depth, within the thickness of it: the starting
// overlap would be a hit if the ray had ever been in front. It is not; the
// ray emerges above the object and hits the next thing it passes behind, or
// misses.
#[test]
fn a_hidden_origin_never_hits_its_occluder_at_its_start() {
    let origin = water_point(Vec2::new(32.5, 36.5));
    let direction = mirrored(origin);
    let start = pixel_of(origin);
    // The object: rows 30..38 across the pool, 0.3 m in front of the origin.
    let object = ndc_depth(origin + (EYE - origin).normalize() * 0.3);
    let mut texels = empty_depth();
    fill(&mut texels, 30..38, 0..EXTENT.x, object);
    let scene_w = view_depth(origin + (EYE - origin).normalize() * 0.3);
    assert!(
        ndc_depth(origin) > object && view_depth(origin) <= scene_w + THICKNESS,
        "the origin starts behind the object, within the thickness"
    );
    let hidden = Recording::new(&texels);
    assert_eq!(march(origin, direction, &hidden), MISS);
    assert_eq!(
        hidden.loads.borrow()[0].y.floor(),
        start.y.floor(),
        "the first sample is the origin's own, behind the object"
    );

    // A bar past the object: the ray hits it, not the object.
    let bar_row = 20;
    let bar = depth_in_front_of_ray(origin, direction, bar_row, 0.2);
    fill(&mut texels, bar_row..bar_row + 1, 0..EXTENT.x, bar);
    let beyond = march(origin, direction, &buffer(&texels));
    assert_eq!(beyond.pixel.y.floor() as u32, bar_row, "{beyond:?}");
}

// --- Always -----------------------------------------------------------------

/// A water origin in view and a unit direction, from proptest's draws.
fn origin_and_direction(pixel: (f32, f32), angles: (f32, f32)) -> (Vec3, Vec3) {
    let origin = water_point(Vec2::new(pixel.0, pixel.1));
    let (azimuth, elevation) = angles;
    let direction = Vec3::new(
        elevation.cos() * azimuth.cos(),
        elevation.sin(),
        elevation.cos() * azimuth.sin(),
    );
    (origin, direction)
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(256))]

    // Over any origin on the water in view, any direction, stride, budget
    // and depth buffer, confidence lies in [0, 1].
    #[test]
    fn confidence_always_lies_in_zero_to_one(
        pixel in (0.5f32..63.5, 26.0f32..47.5),
        angles in (-std::f32::consts::PI..std::f32::consts::PI, -1.5f32..1.5),
        texels in prop::collection::vec(0.0f32..=1.0, (EXTENT.x * EXTENT.y) as usize),
        stride in 1.0f32..4.0,
        budget in 1u32..96,
    ) {
        let (origin, direction) = origin_and_direction(pixel, angles);
        let end = screen_march(
            origin, direction, view_proj(), &buffer(&texels), stride, budget, THICKNESS,
        );
        prop_assert!((0.0..=1.0).contains(&end.confidence), "{end:?}");
    }

    // Over any origin and direction, the ray and its reverse step the same
    // screen-space length, one stride, between every two samples: the
    // march's step never reads the direction's sign.
    #[test]
    fn the_step_length_never_depends_on_the_directions_sign(
        pixel in (0.5f32..63.5, 26.0f32..47.5),
        angles in (-std::f32::consts::PI..std::f32::consts::PI, -1.5f32..1.5),
        stride in 1.0f32..4.0,
    ) {
        let (origin, direction) = origin_and_direction(pixel, angles);
        let texels = empty_depth();
        for sign in [1.0f32, -1.0] {
            let loads = Recording::new(&texels);
            screen_march(
                origin, direction * sign, view_proj(), &loads, stride, BUDGET, THICKNESS,
            );
            for step in loads.step_lengths() {
                prop_assert!(
                    (step - stride).abs() < 1.0e-3 * stride,
                    "sign {sign}: step {step} at stride {stride}"
                );
            }
        }
    }
}
