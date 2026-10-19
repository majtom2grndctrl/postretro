// Screen-space march: a world-space ray walked across the current frame's
// full-resolution depth at a fixed screen-space stride, under a fixed step
// budget. Binding-agnostic: the caller passes the depth texture.
// See: context/lib/rendering_pipeline.md §7.3a, §8 · context/lib/index.md §1.1

// IP rule (owner review to change, rendering_pipeline.md §7.3a): the march
// and its confidence read no ray- or view-direction quantity. The direction
// only places the ray's far end; every step has the same screen-space length,
// and the only exits are a hit, leaving the screen, leaving the depth range
// (the near plane included) and the budget. Nothing exits early, skips,
// re-weights or changes step by direction, so a ray heading toward the camera
// marches like any other.

// The ray's far end before near-plane clipping, metres: past any room, so the
// segment's screen projection reaches its vanishing point.
const SCREEN_MARCH_RAY_LENGTH: f32 = 1.0e3;
// Clip-space depth the far end is clipped to in front of the near plane.
const SCREEN_MARCH_NEAR_EPSILON: f32 = 1.0e-6;
// Screen fraction, per axis, over which confidence fades to 0 at the edge.
const SCREEN_MARCH_EDGE_FADE: f32 = 0.1;
// Fraction of the step budget, at its end, over which confidence fades to 0,
// so the march's reach ends without a hard line.
const SCREEN_MARCH_REACH_FADE: f32 = 0.25;

struct ScreenMarch {
    // Full-resolution pixel the ray stopped on.
    pixel: vec2<f32>,
    // Hit confidence x screen-edge fade; 0 on a miss.
    confidence: f32,
};

// Full-resolution pixel position of a clip-space point.
fn screen_march_pixel(clip: vec4<f32>, extent: vec2<f32>) -> vec2<f32> {
    let ndc = clip.xy / clip.w;
    return vec2<f32>(ndc.x * 0.5 + 0.5, 0.5 - ndc.y * 0.5) * extent;
}

// Confidence of a march that stopped at `pixel` on step `taken`: 1 inside the
// screen and the budget, fading to 0 across the edge band and the budget's
// last stretch. Reads the stop's screen position and step index alone.
fn screen_march_confidence(pixel: vec2<f32>, extent: vec2<f32>, taken: u32, budget: u32) -> f32 {
    let uv = pixel / extent;
    let edge = min(uv, vec2<f32>(1.0) - uv);
    let fade = clamp(min(edge.x, edge.y) / SCREEN_MARCH_EDGE_FADE, 0.0, 1.0);
    let left = f32(budget - taken) / max(f32(budget) * SCREEN_MARCH_REACH_FADE, 1.0);
    return fade * clamp(left, 0.0, 1.0);
}

// March `direction` (unit, world) from `origin` across `depth`, the frame's
// full-resolution scene depth (standard Z: 0 near, 1 far, 1 where nothing
// drew), `stride` full-resolution pixels per step for at most `budget` steps.
// A step hits where the ray lies behind the depth buffer and the span of
// view depth it covered since the last sample reaches within `thickness`
// metres behind the buffer, so a ray that crosses a surface between two
// samples still hits it; but only once the ray has been in front of the
// buffer: an origin hidden behind an occluder never hits that occluder at its
// start. Depth is compared as view depth w = b / (d - a), the projection's own
// a and b read off `view_proj`'s z and w rows; a projection with no
// perspective divide gets no hits.
fn screen_march(origin: vec3<f32>, direction: vec3<f32>, view_proj: mat4x4<f32>,
                depth: texture_depth_2d, stride: f32, budget: u32, thickness: f32) -> ScreenMarch {
    let extent = vec2<f32>(textureDimensions(depth));
    let miss = ScreenMarch(vec2<f32>(0.0), 0.0);
    // The rows of `view_proj` that yield clip z and w.
    let row_z = vec4<f32>(view_proj[0].z, view_proj[1].z, view_proj[2].z, view_proj[3].z);
    let row_w = vec4<f32>(view_proj[0].w, view_proj[1].w, view_proj[2].w, view_proj[3].w);
    let w_axis = dot(row_w.xyz, row_w.xyz);
    if !(w_axis > 0.0) {
        return miss;
    }
    let depth_a = dot(row_z.xyz, row_w.xyz) / w_axis;
    let depth_b = row_z.w - depth_a * row_w.w;

    let clip_origin = view_proj * vec4<f32>(origin, 1.0);
    if !(clip_origin.z > 0.0 && clip_origin.w > 0.0) {
        return miss;
    }
    // The far end, clipped just in front of the near plane: clip space is
    // affine along the ray, so the plane z = 0 is a linear solve.
    var clip_far = view_proj * vec4<f32>(origin + direction * SCREEN_MARCH_RAY_LENGTH, 1.0);
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
    let span = max(length(p1 - p0), 1.0e-6);
    let s_step = stride / span;

    var in_front = false;
    // The ray's view depth at the last sample: the near end of the span the
    // next step covers.
    var last_w = 1.0 / k0;
    for (var taken: u32 = 0u; taken < budget; taken = taken + 1u) {
        let s = f32(taken) * s_step;
        if s > 1.0 {
            // Past the clipped far end: the ray left the depth range.
            return miss;
        }
        let pixel = mix(p0, p1, s);
        if any(pixel < vec2<f32>(0.0)) || any(pixel >= extent) {
            return miss;
        }
        let ray_depth = mix(d0, d1, s);
        if !(ray_depth >= 0.0 && ray_depth <= 1.0) {
            return miss;
        }
        let ray_w = 1.0 / mix(k0, k1, s);
        let span_near = min(ray_w, last_w);
        last_w = ray_w;
        let scene = textureLoad(depth, vec2<i32>(pixel), 0);
        if scene >= 1.0 || ray_depth <= scene {
            in_front = true;
            continue;
        }
        let scene_w = depth_b / (scene - depth_a);
        if in_front && span_near <= scene_w + thickness {
            return ScreenMarch(pixel, screen_march_confidence(pixel, extent, taken, budget));
        }
    }
    return miss;
}
