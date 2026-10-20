// Screen-space march: GPU-free CPU reference for `screen_march.wgsl`, which
// mirrors it statement for statement and a test pins.
// See: context/lib/rendering_pipeline.md §7.3a, §8 · context/lib/index.md §1.1

use glam::{Mat4, UVec2, Vec2, Vec3, Vec4};

// IP rule (owner review to change, rendering_pipeline.md §7.3a): the march
// and its confidence read no ray- or view-direction quantity. The direction
// only places the ray's far end; every step has the same screen-space length,
// and the only exits are a hit, leaving the screen, leaving the depth range
// (the near plane included) and the budget. Nothing exits early, skips,
// re-weights or changes step by direction, so a ray heading toward the camera
// marches like any other. `render/fluid_surface_tests.rs` scans both texts.

/// The ray's far end before near-plane clipping, metres: past any room, so
/// the segment's screen projection reaches its vanishing point.
pub const SCREEN_MARCH_RAY_LENGTH: f32 = 1.0e3;
/// Clip-space depth the far end is clipped to in front of the near plane.
pub const SCREEN_MARCH_NEAR_EPSILON: f32 = 1.0e-6;
/// Screen fraction, per axis, over which confidence fades to 0 at the edge.
pub const SCREEN_MARCH_EDGE_FADE: f32 = 0.1;
/// Fraction of the step budget, at its end, over which confidence fades to
/// 0, so the march's reach ends without a hard line.
pub const SCREEN_MARCH_REACH_FADE: f32 = 0.25;

/// Where a march stopped, and how far to trust it.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ScreenMarch {
    /// Full-resolution pixel the ray stopped on.
    pub pixel: Vec2,
    /// Hit confidence × screen-edge fade; 0 on a miss.
    pub confidence: f32,
}

/// The frame's full-resolution scene depth as the march reads it: standard
/// Z, 0 near, 1 far, 1 where nothing drew.
pub trait ScreenMarchDepth {
    /// `textureDimensions(depth)`.
    fn dimensions(&self) -> UVec2;
    /// `textureLoad(depth, vec2<i32>(pixel), 0)`: the texel `pixel` lies in,
    /// for a `pixel` inside the dimensions. The conversion lives here, so a
    /// test reading through this sees the exact position of every sample.
    fn load(&self, pixel: Vec2) -> f32;
}

/// A synthetic depth buffer standing in for the scene depth texture.
#[derive(Debug, Clone, Copy)]
pub struct ScreenMarchDepthBuffer<'a> {
    /// Row-major, `dimensions.x` texels a row, top row first.
    pub texels: &'a [f32],
    pub dimensions: UVec2,
}

impl ScreenMarchDepth for ScreenMarchDepthBuffer<'_> {
    fn dimensions(&self) -> UVec2 {
        self.dimensions
    }

    fn load(&self, pixel: Vec2) -> f32 {
        let texel = pixel.as_uvec2();
        self.texels[(texel.y * self.dimensions.x + texel.x) as usize]
    }
}

/// WGSL's scalar `mix`.
fn mix(x: f32, y: f32, a: f32) -> f32 {
    x * (1.0 - a) + y * a
}

/// Full-resolution pixel position of a clip-space point.
pub fn screen_march_pixel(clip: Vec4, extent: Vec2) -> Vec2 {
    let ndc = clip.truncate().truncate() / clip.w;
    Vec2::new(ndc.x * 0.5 + 0.5, 0.5 - ndc.y * 0.5) * extent
}

/// Confidence of a march that stopped at `pixel` on step `taken`: 1 inside
/// the screen and the budget, fading to 0 across the edge band and the
/// budget's last stretch. Reads the stop's screen position and step index
/// alone.
pub fn screen_march_confidence(pixel: Vec2, extent: Vec2, taken: u32, budget: u32) -> f32 {
    let uv = pixel / extent;
    let edge = uv.min(Vec2::ONE - uv);
    let fade = (edge.x.min(edge.y) / SCREEN_MARCH_EDGE_FADE).clamp(0.0, 1.0);
    let left = (budget - taken) as f32 / (budget as f32 * SCREEN_MARCH_REACH_FADE).max(1.0);
    fade * left.clamp(0.0, 1.0)
}

/// March `direction` (unit, world) from `origin` across `depth`, the frame's
/// full-resolution scene depth, `stride` full-resolution pixels per step for
/// at most `budget` steps. A step hits where the ray lies behind the depth
/// buffer and the span of view depth it covered since the last sample reaches
/// within `thickness` metres behind the buffer, so a ray that crosses a
/// surface between two samples still hits it; but only once the ray has been
/// in front of the buffer: an origin hidden behind an occluder never hits
/// that occluder at its start. Depth is compared as view depth
/// `w = b / (d − a)`, the projection's own a and b read off `view_proj`'s z
/// and w rows; a projection with no perspective divide gets no hits. The
/// negated comparisons send a NaN to the miss, as the shader's do.
#[allow(clippy::neg_cmp_op_on_partial_ord)]
pub fn screen_march(
    origin: Vec3,
    direction: Vec3,
    view_proj: Mat4,
    depth: &impl ScreenMarchDepth,
    stride: f32,
    budget: u32,
    thickness: f32,
) -> ScreenMarch {
    let extent = depth.dimensions().as_vec2();
    let miss = ScreenMarch {
        pixel: Vec2::ZERO,
        confidence: 0.0,
    };
    // The rows of `view_proj` that yield clip z and w.
    let row_z = Vec4::new(
        view_proj.col(0).z,
        view_proj.col(1).z,
        view_proj.col(2).z,
        view_proj.col(3).z,
    );
    let row_w = Vec4::new(
        view_proj.col(0).w,
        view_proj.col(1).w,
        view_proj.col(2).w,
        view_proj.col(3).w,
    );
    let w_axis = row_w.truncate().dot(row_w.truncate());
    if !(w_axis > 0.0) {
        return miss;
    }
    let depth_a = row_z.truncate().dot(row_w.truncate()) / w_axis;
    let depth_b = row_z.w - depth_a * row_w.w;

    let clip_origin = view_proj * origin.extend(1.0);
    if !(clip_origin.z > 0.0 && clip_origin.w > 0.0) {
        return miss;
    }
    // The far end, clipped just in front of the near plane: clip space is
    // affine along the ray, so the plane z = 0 is a linear solve.
    let mut clip_far = view_proj * (origin + direction * SCREEN_MARCH_RAY_LENGTH).extend(1.0);
    if clip_far.z < SCREEN_MARCH_NEAR_EPSILON {
        let t = (clip_origin.z - SCREEN_MARCH_NEAR_EPSILON) / (clip_origin.z - clip_far.z);
        clip_far = clip_origin + (clip_far - clip_origin) * t;
    }
    let p0 = screen_march_pixel(clip_origin, extent);
    let p1 = screen_march_pixel(clip_far, extent);
    // NDC depth and 1/w are linear across the screen-space segment.
    let d0 = clip_origin.z / clip_origin.w;
    let d1 = clip_far.z / clip_far.w;
    let k0 = 1.0 / clip_origin.w;
    let k1 = 1.0 / clip_far.w;
    let span = (p1 - p0).length().max(1.0e-6);
    let s_step = stride / span;

    let mut in_front = false;
    // The ray's view depth at the last sample: the near end of the span the
    // next step covers.
    let mut last_w = 1.0 / k0;
    for taken in 0..budget {
        let s = taken as f32 * s_step;
        if s > 1.0 {
            // Past the clipped far end: the ray left the depth range.
            return miss;
        }
        let pixel = p0.lerp(p1, s);
        if pixel.cmplt(Vec2::ZERO).any() || pixel.cmpge(extent).any() {
            return miss;
        }
        let ray_depth = mix(d0, d1, s);
        if !(0.0..=1.0).contains(&ray_depth) {
            return miss;
        }
        let ray_w = 1.0 / mix(k0, k1, s);
        let span_near = ray_w.min(last_w);
        last_w = ray_w;
        let scene = depth.load(pixel);
        if scene >= 1.0 || ray_depth <= scene {
            in_front = true;
            continue;
        }
        let scene_w = depth_b / (scene - depth_a);
        if in_front && span_near <= scene_w + thickness {
            return ScreenMarch {
                pixel,
                confidence: screen_march_confidence(pixel, extent, taken, budget),
            };
        }
    }
    miss
}

#[cfg(test)]
#[path = "screen_march_tests.rs"]
mod tests;
