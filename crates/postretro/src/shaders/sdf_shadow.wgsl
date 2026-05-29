// Half-resolution SDF static-occluder shadow pass.
//
// Task 4 of sdf-static-occluder-shadows. Per half-res pixel:
//   1. Reconstruct world position from the depth pre-pass via inverse view-projection.
//   2. Sample (a) the static-lightmap baked dominant direction and (b) the
//      animated-baked atlas's per-frame dominant direction; trace one ray
//      per term against the static SDF.
//   3. Accumulate the closest-passing-distance penumbra factor along each march.
//   4. Early-out open regions via the DDGI E[d] depth moment at the pixel's
//      probe cell — `coarse` SDF distance is the cheap analogue.
//
// Writes two channels into one Rgba8Unorm half-res target:
//   R = static-lightmap aggregate factor
//   G = animated-baked aggregate factor
//   B, A = reserved for the future geometry-moving per-light factors —
//          no current consumer; documented seam, not coded here.
//
// Direction sampling (v2). The dominant-direction atlases are baked per-texel
// keyed on lightmap UV. The visible surface's lightmap UV is read from the
// depth pre-pass MRT (`lightmap_uv_tex`, Rg16Float), so both direction reads
// are now per-texel correct. The trace and penumbra math are unchanged.
//
// Group 0: SDF atlas (owned by SdfAtlasResources — Task 3). Bindings 0..3.
// Group 1: this pass's own bind group. Bindings 0..6 (see below).

// ---- Group 0: SDF atlas (mirrors crates/postretro/src/render/sdf_atlas.rs) ----

struct SdfAtlasMeta {
    world_min: vec3<f32>,
    voxel_size_m: f32,
    world_max: vec3<f32>,
    brick_size_voxels: u32,
    grid_dims: vec3<u32>,
    surface_brick_count: u32,
    atlas_bricks_per_axis: vec3<u32>,
    present: u32,
};

@group(0) @binding(0) var<uniform> sdf_meta: SdfAtlasMeta;
@group(0) @binding(1) var sdf_atlas: texture_3d<i32>;
@group(0) @binding(2) var sdf_coarse: texture_3d<f32>;
@group(0) @binding(3) var<storage, read> sdf_top_level: array<u32>;

// ---- Group 1: pass-owned bindings ----

struct ShadowPassParams {
    // Inverse view-projection: unprojects half-res NDC + depth to world space.
    inv_view_proj: mat4x4<f32>,
    // Camera position — march origin offset epsilon and SH cell lookups.
    camera_position: vec3<f32>,
    // Half-res target dimensions (matches `shadow_factor` extent).
    half_res_size_x: u32,
    half_res_size_y: u32,
    // Tuning knobs (Task 7 wires sliders to these — defaults for now).
    max_march_steps: u32,
    open_space_skip_threshold: f32,
    penumbra_k: f32,
    // SH grid for the open-space skip lookup. Mirrors `ShGridInfo` from sh_volume.rs,
    // re-stated here so we don't have to bind group 3 in this pass too.
    sh_grid_origin: vec3<f32>,
    sh_has_volume: u32,
    sh_cell_size: vec3<f32>,
    _pad0: u32,
    sh_grid_dimensions: vec3<u32>,
    _pad1: u32,
};

@group(1) @binding(0) var<uniform> params: ShadowPassParams;
@group(1) @binding(1) var depth_tex: texture_depth_2d;
// Static-lightmap dominant direction texture (Rgba8Unorm, octahedral in rg).
// See `forward.wgsl::decode_lightmap_direction`.
@group(1) @binding(2) var static_lm_direction: texture_2d<f32>;
// Animated-baked atlas's per-frame dominant direction (Task 2b).
@group(1) @binding(3) var animated_lm_direction: texture_2d<f32>;
// DDGI E[d] / E[d²] depth moments (R = mean, G = mean^2). 3D probe grid.
@group(1) @binding(4) var sh_depth_moments: texture_3d<f32>;
// Half-res output: R = static aggregate factor, G = animated aggregate factor.
@group(1) @binding(5) var shadow_factor: texture_storage_2d<rgba8unorm, write>;
// Full-res lightmap-UV gbuffer (Rg16Float) written by the depth pre-pass MRT.
// Read via textureLoad to recover the visible surface's lightmap UV per pixel.
@group(1) @binding(6) var lightmap_uv_tex: texture_2d<f32>;

// ---- Helpers ----

const SDF_TOP_LEVEL_EMPTY: u32 = 0xffffffffu;
const SDF_TOP_LEVEL_INTERIOR: u32 = 0xfffffffeu;
const SDF_I16_QUANT_STEPS_PER_VOXEL: f32 = 256.0;
const FULLY_LIT: f32 = 1.0;

// Octahedral decode mirroring forward.wgsl. Input enc is sampled Rgba8Unorm
// [0,1]; rg channels carry the octahedral direction.
fn decode_lm_direction(enc: vec4<f32>) -> vec3<f32> {
    let ox = enc.r * 2.0 - 1.0;
    let oy = enc.g * 2.0 - 1.0;
    let z = 1.0 - abs(ox) - abs(oy);
    var x: f32;
    var y: f32;
    if (z < 0.0) {
        x = (1.0 - abs(oy)) * select(-1.0, 1.0, ox >= 0.0);
        y = (1.0 - abs(ox)) * select(-1.0, 1.0, oy >= 0.0);
    } else {
        x = ox;
        y = oy;
    }
    let v = vec3<f32>(x, y, z);
    let len2 = dot(v, v);
    if (len2 < 1.0e-6) {
        return vec3<f32>(0.0, 1.0, 0.0);
    }
    return v * inverseSqrt(len2);
}

// Reconstruct world position from a half-res pixel + the depth pre-pass sample.
// Returns vec4(world.xyz, valid) — valid == 0 when depth is the cleared sentinel (1.0).
fn reconstruct_world(half_xy: vec2<u32>) -> vec4<f32> {
    // Map half-res pixel center to full-res pixel for depth lookup.
    let depth_dims = textureDimensions(depth_tex);
    let scale_x = f32(depth_dims.x) / f32(params.half_res_size_x);
    let scale_y = f32(depth_dims.y) / f32(params.half_res_size_y);
    let fx = i32(min((f32(half_xy.x) + 0.5) * scale_x, f32(depth_dims.x) - 1.0));
    let fy = i32(min((f32(half_xy.y) + 0.5) * scale_y, f32(depth_dims.y) - 1.0));
    let depth = textureLoad(depth_tex, vec2<i32>(fx, fy), 0);
    if (depth >= 0.9999) {
        return vec4<f32>(0.0, 0.0, 0.0, 0.0);
    }
    // NDC in wgpu: x,y in [-1,1] (y up), z in [0,1].
    let ndc_x = (f32(half_xy.x) + 0.5) / f32(params.half_res_size_x) * 2.0 - 1.0;
    let ndc_y = 1.0 - (f32(half_xy.y) + 0.5) / f32(params.half_res_size_y) * 2.0;
    let clip = vec4<f32>(ndc_x, ndc_y, depth, 1.0);
    let world_h = params.inv_view_proj * clip;
    if (abs(world_h.w) < 1.0e-6) {
        return vec4<f32>(0.0, 0.0, 0.0, 0.0);
    }
    return vec4<f32>(world_h.xyz / world_h.w, 1.0);
}

// Sample the coarse SDF distance (meters) at a world-space point. Returns a
// large positive value (treat as "far open") when the point is outside the
// atlas — the trace will then either step out of the bounded region or hit
// the early-out budget.
fn sample_coarse_distance(world: vec3<f32>) -> f32 {
    if (params.sh_has_volume == 0u && sdf_meta.present == 0u) {
        return 1.0e4;
    }
    if (sdf_meta.present == 0u) {
        return 1.0e4;
    }
    let extent = sdf_meta.world_max - sdf_meta.world_min;
    if (extent.x <= 0.0 || extent.y <= 0.0 || extent.z <= 0.0) {
        return 1.0e4;
    }
    let normalized = (world - sdf_meta.world_min) / extent;
    if (any(normalized < vec3<f32>(0.0)) || any(normalized > vec3<f32>(1.0))) {
        return 1.0e4;
    }
    let grid = vec3<f32>(sdf_meta.grid_dims);
    let brick_size = max(f32(sdf_meta.brick_size_voxels), 1.0);
    let voxel = max(sdf_meta.voxel_size_m, 1.0e-4);
    let brick_world_size = brick_size * voxel;
    let coord = vec3<i32>(clamp(normalized * grid, vec3<f32>(0.0), grid - vec3<f32>(1.0)));
    let coarse = textureLoad(sdf_coarse, coord, 0).r;
    // Treat the per-brick coarse value as a metric distance lower bound for
    // the brick's interior. Conservative.
    return max(coarse, 0.0) * brick_world_size;
}

// Sample the SH depth moments E[d] at a world-space point. Returns the mean
// ray distance in meters, or a large sentinel when the point is outside the
// SH probe volume / no SH volume is loaded.
fn sample_open_distance(world: vec3<f32>) -> f32 {
    if (params.sh_has_volume == 0u) {
        return 1.0e4;
    }
    let extent = vec3<f32>(params.sh_grid_dimensions) * params.sh_cell_size;
    if (extent.x <= 0.0 || extent.y <= 0.0 || extent.z <= 0.0) {
        return 1.0e4;
    }
    let local = (world - params.sh_grid_origin) / params.sh_cell_size;
    let grid_f = vec3<f32>(params.sh_grid_dimensions);
    if (any(local < vec3<f32>(0.0)) || any(local >= grid_f)) {
        return 1.0e4;
    }
    let coord = vec3<i32>(clamp(local, vec3<f32>(0.0), grid_f - vec3<f32>(1.0)));
    let moments = textureLoad(sh_depth_moments, coord, 0);
    return moments.r; // E[d] — mean ray distance to occluder
}

// Trace the static SDF from `origin` toward `dir` (unit) for shadow occlusion.
// Returns the closest-passing-distance penumbra factor in [0, 1]; 1 = lit.
//
// Uses sphere-tracing against the coarse per-brick distances. The fine atlas
// is intentionally not consumed in v1 — the coarse fallback alone produces a
// usable shadow factor and keeps the per-pixel cost bounded. Refining to the
// fine atlas is the named follow-up (see plan §Open questions).
fn trace_shadow(origin: vec3<f32>, dir: vec3<f32>) -> f32 {
    if (sdf_meta.present == 0u) {
        return FULLY_LIT;
    }
    // Open-space skip: if the SH moment at the origin suggests the geometry
    // ahead is far away, return fully lit immediately.
    let cell_scale = max(max(params.sh_cell_size.x, params.sh_cell_size.y), params.sh_cell_size.z);
    let open = sample_open_distance(origin);
    if (open > params.open_space_skip_threshold * cell_scale) {
        return FULLY_LIT;
    }

    // Offset the march start slightly to avoid self-shadowing at the surface.
    let voxel = max(sdf_meta.voxel_size_m, 1.0e-4);
    let bias = voxel * 1.5;
    var t: f32 = bias;
    var factor: f32 = FULLY_LIT;
    let k = max(params.penumbra_k, 1.0);
    let max_t = 64.0; // meters — bounded march length

    let steps: u32 = clamp(params.max_march_steps, 1u, 256u);
    for (var i: u32 = 0u; i < steps; i = i + 1u) {
        let p = origin + dir * t;
        let d = sample_coarse_distance(p);
        if (d < voxel * 0.5) {
            return 0.0;
        }
        // Closest-passing-distance penumbra estimate.
        factor = min(factor, k * d / max(t, voxel));
        t = t + max(d, voxel * 0.5);
        if (t > max_t) {
            break;
        }
    }
    return clamp(factor, 0.0, 1.0);
}

// ---- Entry ----

@compute @workgroup_size(8, 8, 1)
fn cs_main(@builtin(global_invocation_id) gid: vec3<u32>) {
    if (gid.x >= params.half_res_size_x || gid.y >= params.half_res_size_y) {
        return;
    }
    let half_xy = gid.xy;

    // Default: fully lit on both terms. Sky / no-atlas / out-of-volume paths
    // all degrade to 1.0 so the forward multiply is a no-op there.
    var r: f32 = FULLY_LIT;
    var g: f32 = FULLY_LIT;

    if (sdf_meta.present != 0u) {
        let recon = reconstruct_world(half_xy);
        if (recon.w > 0.5) {
            let world = recon.xyz;

            // Sample the visible surface's lightmap UV from the depth pre-pass
            // MRT (Rg16Float), then index the dominant-direction atlases
            // per-texel. Replaces the v1 screen-UV approximation. scale_x/y are
            // recomputed here (the ones in reconstruct_world are local to it);
            // the lightmap-UV target shares full-res dims with depth, so the
            // ratios are identical.
            let lm_dims = textureDimensions(lightmap_uv_tex);
            let scale_x = f32(lm_dims.x) / f32(params.half_res_size_x);
            let scale_y = f32(lm_dims.y) / f32(params.half_res_size_y);
            let full_x = i32(min((f32(half_xy.x) + 0.5) * scale_x, f32(lm_dims.x) - 1.0));
            let full_y = i32(min((f32(half_xy.y) + 0.5) * scale_y, f32(lm_dims.y) - 1.0));
            let lm_uv = textureLoad(lightmap_uv_tex, vec2<i32>(full_x, full_y), 0).rg;
            if (lm_uv.x < 0.0) {
                // Pre-pass sentinel (Rg16Float target cleared to (-1,-1)) — no
                // fragment wrote this pixel. Bail to fully lit.
                textureStore(
                    shadow_factor,
                    vec2<i32>(i32(half_xy.x), i32(half_xy.y)),
                    vec4<f32>(FULLY_LIT, FULLY_LIT, 1.0, 1.0),
                );
                return;
            }
            let static_dims = textureDimensions(static_lm_direction, 0);
            let static_coord = vec2<i32>(
                i32(lm_uv.x * f32(static_dims.x)),
                i32(lm_uv.y * f32(static_dims.y)),
            );
            let static_enc = textureLoad(static_lm_direction, static_coord, 0);
            let static_dir = decode_lm_direction(static_enc);

            let animated_dims = textureDimensions(animated_lm_direction, 0);
            let animated_coord = vec2<i32>(
                i32(lm_uv.x * f32(animated_dims.x)),
                i32(lm_uv.y * f32(animated_dims.y)),
            );
            let animated_enc = textureLoad(animated_lm_direction, animated_coord, 0);
            let animated_dir = decode_lm_direction(animated_enc);

            r = trace_shadow(world, static_dir);
            g = trace_shadow(world, animated_dir);
        }
    }

    // B/A reserved for the future geometry-moving per-light factors — write 1
    // (fully lit) so a consumer that adds these later sees an honest default.
    textureStore(
        shadow_factor,
        vec2<i32>(i32(half_xy.x), i32(half_xy.y)),
        vec4<f32>(r, g, 1.0, 1.0),
    );
}
