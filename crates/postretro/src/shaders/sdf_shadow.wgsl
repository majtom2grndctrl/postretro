// Half-resolution SDF static-occluder shadow compute pass.
// See: context/lib/rendering_pipeline.md §7

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
// Animated-baked atlas's per-frame dominant direction (Task 2b). The retained
// animated dominant-direction trace reads this; the static dominant-direction
// trace is gone — per-light static shadows key on light POSITION now (group 2).
@group(1) @binding(2) var animated_lm_direction: texture_2d<f32>;
// DDGI E[d] / E[d^2] depth moments (R = mean, G = mean^2). 3D probe grid.
@group(1) @binding(3) var sh_depth_moments: texture_3d<f32>;
// Half-res output. R/B/A = per-light SDF visibility slices (K = 3, see
// sdf_light_select.wgsl); G = animated aggregate factor (unchanged).
@group(1) @binding(4) var shadow_factor: texture_storage_2d<rgba8unorm, write>;
// Full-res lightmap-UV gbuffer (Rg16Float) written by the depth pre-pass MRT.
// Read via textureLoad to recover the visible surface's lightmap UV per pixel.
// KEPT (not dead): the animated trace still indexes animated_lm_direction
// through this per-texel UV.
@group(1) @binding(5) var lightmap_uv_tex: texture_2d<f32>;

// ---- Group 2: static light buffers (mirrors forward.wgsl's lighting group) ----
// Bound here so the SHARED K-selection helper (sdf_light_select.wgsl, appended
// at pipeline creation) can pick the same lights the forward shader will. The
// helper reads these by lexical name; this pass binds its own copy.

struct SpecLight {
    position_and_range: vec4<f32>, // xyz = position, w = falloff_range
    color_and_pad:      vec4<f32>, // xyz = color × intensity, w = sdf flag (>0.5 ⇒ sdf)
};
@group(2) @binding(0) var<storage, read> spec_lights: array<SpecLight>;

struct ChunkGridInfo {
    grid_origin: vec3<f32>,
    cell_size: f32,
    dims: vec3<u32>,
    has_chunk_grid: u32,
};
@group(2) @binding(1) var<uniform> chunk_grid: ChunkGridInfo;
@group(2) @binding(2) var<storage, read> chunk_offsets: array<vec2<u32>>;
@group(2) @binding(3) var<storage, read> chunk_indices: array<u32>;

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
    let coord = vec3<i32>(clamp(normalized * grid, vec3<f32>(0.0), grid - vec3<f32>(1.0)));
    let coarse = textureLoad(sdf_coarse, coord, 0).r;
    // The baked coarse value (`sdf_bake.rs::coarse_signed`) is already a metric
    // signed distance in meters — the per-brick mean of ±nearest-triangle
    // distance. Return its non-negative part directly; do NOT re-scale by the
    // brick edge length (that over-stepped the empty-brick fallback by ~4 m and
    // let the sphere-trace tunnel through sub-brick occluders). It is a mean,
    // not a tight lower bound, but that is fine here: this fallback only fires
    // for non-surface bricks — bricks classified EMPTY because their closest
    // triangle distance exceeds `surface_band_m` (sdf_bake.rs ~line 198:
    // `near_surface = min_unsigned <= surface_band_m`). Relaxing that threshold
    // would promote some near-surface bricks to EMPTY and allow this path to
    // under-count distance into them.
    return max(coarse, 0.0);
}

// Sample the fine brick atlas at a world-space point, returning a metric
// signed distance in meters (negative inside solids). This is the fine voxel
// field (~0.5 m per voxel by default, driven by `sdf_meta.voxel_size_m`) that
// resolves sub-brick occluders the coarse field cannot.
//
//   - Out of bounds / no atlas  → large positive sentinel ("far open"), the
//                                  same 1.0e4 literal sample_coarse_distance
//                                  returns.
//   - SDF_TOP_LEVEL_EMPTY brick  → reuse the coarse field for a large
//                                  empty-space step (already meters, >= 0).
//   - SDF_TOP_LEVEL_INTERIOR     → inside solid; return a negative distance so
//                                  the march registers a hit.
//   - surface brick (real slot)  → de-pack the slot to its atlas brick coord,
//                                  map the brick-local position to a voxel,
//                                  textureLoad the nearest voxel (the intended
//                                  retro aesthetic; no trilinear), decode.
fn sample_fine_distance(world: vec3<f32>) -> f32 {
    if (sdf_meta.present == 0u) {
        return 1.0e4;
    }
    let extent = sdf_meta.world_max - sdf_meta.world_min;
    if (extent.x <= 0.0 || extent.y <= 0.0 || extent.z <= 0.0) {
        return 1.0e4;
    }
    let voxel = max(sdf_meta.voxel_size_m, 1.0e-4);
    let brick_size = max(sdf_meta.brick_size_voxels, 1u);
    let brick_world_size = f32(brick_size) * voxel;

    // Resolve the world point to its brick cell. Bounds-guard against the brick
    // grid BEFORE indexing sdf_top_level or the atlas.
    let local = (world - sdf_meta.world_min) / brick_world_size;
    let grid_dims = sdf_meta.grid_dims;
    let grid_f = vec3<f32>(grid_dims);
    if (any(local < vec3<f32>(0.0)) || any(local >= grid_f)) {
        return 1.0e4;
    }
    let brick_coord = vec3<u32>(clamp(local, vec3<f32>(0.0), grid_f - vec3<f32>(1.0)));

    // z-major flat index — mirrors the baker's `for bz { for by { for bx } }`
    // top-level traversal (sdf_bake.rs ~line 151) and the on-disk layout.
    let flat = brick_coord.z * grid_dims.x * grid_dims.y
        + brick_coord.y * grid_dims.x
        + brick_coord.x;
    let slot = sdf_top_level[flat];

    if (slot == SDF_TOP_LEVEL_EMPTY) {
        // Far from any surface on the open side — defer to the coarse field for
        // a large positive empty-space step (the fix above keeps this metric).
        return sample_coarse_distance(world);
    }
    if (slot == SDF_TOP_LEVEL_INTERIOR) {
        // Inside solid. Return a negative distance so the sphere-trace's
        // `d < voxel * 0.5` hit test fires.
        return -brick_world_size;
    }

    // Surface brick: `slot` is the linear surface-brick index. Recover the fine
    // voxel's atlas texel by mirroring the baker's + uploader's exact byte path,
    // NOT the brick-tiled formula in the spec sketch.
    //
    // WHY linear, not brick-tiled: the baker appends each surface brick's voxels
    // CONTIGUOUSLY into one flat `atlas` stream in z-major within-brick order
    // (sdf_bake.rs ~line 222: `atlas.extend_from_slice(&brick_samples)`, where
    // brick_samples is filled z-major: outer loop vz, inner loop vx). The uploader
    // (sdf_atlas.rs ~line 237) then `write_texture`s that flat stream into the
    // 3D atlas as a dense row-major fill of the (aw, ah, ad) extent
    // (bytes_per_row = 2*aw, rows_per_image = ah). So the flat element index of
    // a voxel is `slot * voxels_per_brick + (vz*bs*bs + vy*bs + vx)`, and the
    // texel is the row-major de-interleave of that single linear index:
    //   x = e % aw,  y = (e / aw) % ah,  z = e / (aw * ah).
    // The brick is NOT a contiguous bs^3 sub-cube of the texture — the contiguous
    // run wraps across atlas rows — so the brick-tiled `brick_atlas*bs + voxel`
    // formula would read the wrong voxels for a near-cube pack. Mirror the bytes.
    let bricks_per_axis = sdf_meta.atlas_bricks_per_axis;
    let atlas_w = max(bricks_per_axis.x, 1u) * brick_size;
    let atlas_h = max(bricks_per_axis.y, 1u) * brick_size;
    let voxels_per_brick = brick_size * brick_size * brick_size;

    // Brick-local position → voxel index. Half-texel-clamp to
    // [0.5, brick_size - 0.5]: bricks have NO apron, so the nearest sample must
    // stay strictly inside this brick to avoid bleeding into its neighbor.
    let frac = local - vec3<f32>(brick_coord); // [0,1) within the brick
    let voxel_local = clamp(
        frac * f32(brick_size),
        vec3<f32>(0.5),
        vec3<f32>(f32(brick_size) - 0.5),
    );
    let voxel_idx = vec3<u32>(voxel_local); // nearest voxel (floor of clamped)

    // Flat element index into the baked stream (z-major within-brick), then
    // de-interleave to the dense row-major texel the uploader wrote.
    let voxel_in_brick = voxel_idx.z * brick_size * brick_size
        + voxel_idx.y * brick_size
        + voxel_idx.x;
    let e = slot * voxels_per_brick + voxel_in_brick;
    let texel = vec3<i32>(
        i32(e % atlas_w),
        i32((e / atlas_w) % atlas_h),
        i32(e / (atlas_w * atlas_h)),
    );
    let raw = textureLoad(sdf_atlas, texel, 0).r;
    // Decode i16 quant steps → meters: step = voxel_size_m / 256 per i16.
    return f32(raw) * (voxel / SDF_I16_QUANT_STEPS_PER_VOXEL);
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
// Sphere-traces against the combined fine+coarse distance
// (`sample_fine_distance`): the fine brick atlas resolves sub-brick occluders
// near surfaces (pillars, doorways), falling back to the coarse per-brick field
// in empty bricks. The loop shape — self-shadow start bias, closest-passing
// penumbra estimate, bounded march length, open-space early-out — is unchanged
// from the coarse-only v1.
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
        let d = sample_fine_distance(p);
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

// Trace one per-light static shadow ray toward `light_idx`'s position. Returns
// the closest-passing-distance visibility factor (1 = lit). The per-light
// static trace keys on light POSITION — this is what lets it cast a specific
// light's shadow, unlike the removed single dominant-direction trace.
fn trace_light_visibility(world: vec3<f32>, light_idx: u32) -> f32 {
    let sl = spec_lights[light_idx];
    let to_light = sl.position_and_range.xyz - world;
    let dist = length(to_light);
    if (dist < 1.0e-4) {
        return FULLY_LIT;
    }
    return trace_shadow(world, to_light / dist);
}

// ---- Entry ----

// K-slice channel assignment. K = 3 (SDF_SELECT_K). G stays the animated
// aggregate factor (the animated dominant-direction trace is unchanged), so the
// three per-light slices pack into R / B / A:
//   slice 0 → R   slice 1 → B   slice 2 → A
// Forward (Task 3) reads each sdf light's slice back through this same mapping.
@compute @workgroup_size(8, 8, 1)
fn cs_main(@builtin(global_invocation_id) gid: vec3<u32>) {
    if (gid.x >= params.half_res_size_x || gid.y >= params.half_res_size_y) {
        return;
    }
    let half_xy = gid.xy;
    let store_xy = vec2<i32>(i32(half_xy.x), i32(half_xy.y));

    // Default: fully lit on every channel. Sky / no-atlas / out-of-volume paths
    // all degrade to 1.0 so the forward multiply is a no-op there. Slices map
    // R = slice0, G = animated, B = slice1, A = slice2.
    var slice0: f32 = FULLY_LIT; // R
    var animated: f32 = FULLY_LIT; // G
    var slice1: f32 = FULLY_LIT; // B
    var slice2: f32 = FULLY_LIT; // A

    if (sdf_meta.present != 0u) {
        let recon = reconstruct_world(half_xy);
        if (recon.w > 0.5) {
            let world = recon.xyz;

            // Sample the visible surface's lightmap UV from the depth pre-pass
            // MRT (Rg16Float). The animated dominant-direction trace still
            // indexes its per-texel direction atlas through this UV.
            let lm_dims = textureDimensions(lightmap_uv_tex);
            let scale_x = f32(lm_dims.x) / f32(params.half_res_size_x);
            let scale_y = f32(lm_dims.y) / f32(params.half_res_size_y);
            let full_x = i32(min((f32(half_xy.x) + 0.5) * scale_x, f32(lm_dims.x) - 1.0));
            let full_y = i32(min((f32(half_xy.y) + 0.5) * scale_y, f32(lm_dims.y) - 1.0));
            let lm_uv = textureLoad(lightmap_uv_tex, vec2<i32>(full_x, full_y), 0).rg;
            if (lm_uv.x < 0.0) {
                // Pre-pass sentinel (Rg16Float target cleared to (-1,-1)) — no
                // fragment wrote this pixel. Bail to fully lit on all channels.
                textureStore(shadow_factor, store_xy, vec4<f32>(FULLY_LIT));
                return;
            }

            // Animated term (G) — retained dominant-direction trace.
            let animated_dims = textureDimensions(animated_lm_direction, 0);
            let animated_coord = vec2<i32>(
                i32(lm_uv.x * f32(animated_dims.x)),
                i32(lm_uv.y * f32(animated_dims.y)),
            );
            let animated_enc = textureLoad(animated_lm_direction, animated_coord, 0);
            let animated_dir = decode_lm_direction(animated_enc);
            animated = trace_shadow(world, animated_dir);

            // Per-light static terms (R/B/A) — trace one ray toward each of the
            // K most-influential sdf lights, chosen by the SHARED selection
            // helper so the forward shader shades exactly these same lights.
            let sel = select_sdf_lights(world);
            if (sel.count > 0u) {
                slice0 = trace_light_visibility(world, sel.indices[0]);
            }
            if (sel.count > 1u) {
                slice1 = trace_light_visibility(world, sel.indices[1]);
            }
            if (sel.count > 2u) {
                slice2 = trace_light_visibility(world, sel.indices[2]);
            }
        }
    }

    textureStore(
        shadow_factor,
        store_xy,
        vec4<f32>(slice0, animated, slice1, slice2),
    );
}
