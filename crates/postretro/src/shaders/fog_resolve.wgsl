// Temporal accumulation + reprojection resolve for the low-res fog scatter.
// See context/lib/rendering_pipeline.md §7.5.
//
// Compute pass, one thread per low-res scatter texel. Reads the raw current-
// frame scatter (written by the raymarch), the previous frame's accumulated
// history, and the full-res depth buffer; writes the blended accumulated
// result. The composite then reads this accumulation target instead of the raw
// scatter.
//
// The animated ray-start jitter in fog_volume.wgsl staggers each frame's sample
// positions, so a single frame is noisy but the noise is unbiased frame-to-
// frame. Averaging successive frames with an EMA converges that grain to a
// smooth integral over a few frames — provided the history is REPROJECTED to
// follow camera motion and CLAMPED to the local neighborhood so legitimate fast
// changes (a pulsing light) are not smeared.

// Subset of the raymarch `FogParams` (fog_volume.rs::FogParams). This pass reads
// the camera matrices and near/far; it declares through `prev_view_proj` (the
// appended tail) but stops there. `_pad2` must be present so `prev_view_proj`
// lands at the correct offset.
struct FogParams {
    inv_view_proj: mat4x4<f32>,
    camera_position: vec3<f32>,
    step_size: f32,
    active_count: u32,
    near_clip: f32,
    far_clip: f32,
    point_count: u32,
    spot_count: u32,
    frame_index: u32,
    _pad2: vec2<u32>,
    prev_view_proj: mat4x4<f32>,
}

@group(0) @binding(0) var raw_scatter: texture_2d<f32>;
@group(0) @binding(1) var history: texture_2d<f32>;
@group(0) @binding(2) var accum_out: texture_storage_2d<rgba16float, write>;
@group(0) @binding(3) var depth_texture: texture_depth_2d;
@group(0) @binding(4) var<uniform> fog: FogParams;

// EMA blend weight: result = mix(current, clamped_history, ACCUM_ALPHA). Higher
// = more history retained = smoother but slower to react. 0.9 keeps ~10% of the
// current frame per step, converging grain over a handful of frames. The
// neighborhood clamp below is what lets this stay high without lagging the
// pulsing spot — the history is clamped into the current neighborhood's color
// range before blending, so a fast legitimate change clamps the stale history
// up to the new value rather than averaging slowly toward it. TUNE HERE if the
// pulse smears (lower) or grain persists (raise).
const ACCUM_ALPHA: f32 = 0.9;

// Reconstruct the current-frame world position for a low-res texel from the
// scene/background depth. Fog is not a surface, so this uses the depth of
// whatever opaque geometry (or the far plane) sits behind the fog along the
// ray — the standard screen-space reprojection approximation. It is exact for
// distant fog (depth -> far: rotation reprojects correctly, translation does
// not move infinity) and approximate for near fog. Acceptable per the design.
fn reconstruct_world_pos(uv: vec2<f32>, depth_ndc: f32) -> vec3<f32> {
    let ndc_xy = vec2<f32>(uv.x * 2.0 - 1.0, (1.0 - uv.y) * 2.0 - 1.0);
    // depth_ndc == 1.0 projects to infinity; clip-space w handles it, but cap
    // just under 1.0 so the prev-frame projection stays finite for the rotate-
    // only far-plane case.
    let z = min(depth_ndc, 0.999999);
    let clip = vec4<f32>(ndc_xy, z, 1.0);
    let world = fog.inv_view_proj * clip;
    return world.xyz / world.w;
}

@compute @workgroup_size(8, 8, 1)
fn cs_main(@builtin(global_invocation_id) gid: vec3<u32>) {
    let dims = textureDimensions(accum_out);
    if gid.x >= dims.x || gid.y >= dims.y {
        return;
    }
    let coord = vec2<i32>(gid.xy);
    let current = textureLoad(raw_scatter, coord, 0);

    // --- Current-pixel low-res neighborhood (3x3) statistics ---
    // Clamp the reprojected history into [cmin, cmax] before blending. This is
    // the standard TAA anti-ghosting technique: noise (which differs only
    // within the neighborhood spread) survives the clamp and averages away,
    // while a fast legitimate change — the pulsing spot pushing the whole
    // neighborhood brighter/darker — pulls the clamp window with it, so the
    // stale history is clamped to (near) the new value instead of dragging the
    // result toward last frame. Without it a high ACCUM_ALPHA would visibly lag
    // and trail the pulse.
    let max_coord = vec2<i32>(dims) - vec2<i32>(1);
    var cmin = current;
    var cmax = current;
    for (var dy: i32 = -1; dy <= 1; dy = dy + 1) {
        for (var dx: i32 = -1; dx <= 1; dx = dx + 1) {
            let n = textureLoad(raw_scatter, clamp(coord + vec2<i32>(dx, dy), vec2<i32>(0), max_coord), 0);
            cmin = min(cmin, n);
            cmax = max(cmax, n);
        }
    }

    // --- Reproject to find this texel's prior-frame UV ---
    let uv = (vec2<f32>(gid.xy) + vec2<f32>(0.5)) / vec2<f32>(dims);
    // Min-over-block depth tap, matching the raymarch's `max_t` derivation so
    // the reconstructed position lines up with the geometry the fog marched to.
    let depth_dims = textureDimensions(depth_texture);
    let ps = depth_dims / dims;
    let base = vec2<u32>(gid.x * ps.x, gid.y * ps.y);
    let depth_max = depth_dims - vec2<u32>(1u);
    var depth_ndc: f32 = 1.0;
    // MAX_PIXEL_SCALE bound mirrors fog_volume.wgsl; inner break truncates to
    // the runtime block size.
    for (var dy: u32 = 0u; dy < 8u; dy = dy + 1u) {
        if dy >= ps.y { break; }
        for (var dx: u32 = 0u; dx < 8u; dx = dx + 1u) {
            if dx >= ps.x { break; }
            let sx = min(base.x + dx, depth_max.x);
            let sy = min(base.y + dy, depth_max.y);
            depth_ndc = min(depth_ndc, textureLoad(depth_texture, vec2<i32>(vec2<u32>(sx, sy)), 0));
        }
    }

    let world_pos = reconstruct_world_pos(uv, depth_ndc);
    let prev_clip = fog.prev_view_proj * vec4<f32>(world_pos, 1.0);

    var result = current;
    // Reject behind-camera (w <= 0) and off-screen reprojections: no valid
    // history -> output current (also the first-frame / uninitialized-history
    // case, since the history texture starts cleared and reprojection of the
    // first frame's static camera maps onto cleared texels — clamping a cleared
    // history into the current neighborhood already collapses it toward current,
    // but the explicit UV/w rejection covers disocclusion and screen edges).
    if prev_clip.w > 0.0 {
        let prev_ndc = prev_clip.xy / prev_clip.w;
        let prev_uv = vec2<f32>(prev_ndc.x * 0.5 + 0.5, 1.0 - (prev_ndc.y * 0.5 + 0.5));
        if prev_uv.x >= 0.0 && prev_uv.x <= 1.0 && prev_uv.y >= 0.0 && prev_uv.y <= 1.0 {
            let hist_texel = vec2<i32>(clamp(
                prev_uv * vec2<f32>(dims),
                vec2<f32>(0.0),
                vec2<f32>(max_coord),
            ));
            let hist = textureLoad(history, hist_texel, 0);
            let clamped_hist = clamp(hist, cmin, cmax);
            result = mix(current, clamped_hist, ACCUM_ALPHA);
        }
    }

    textureStore(accum_out, coord, result);
}
