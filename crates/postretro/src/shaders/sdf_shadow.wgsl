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
@group(0) @binding(1) var sdf_atlas: texture_3d<f32>;
@group(0) @binding(2) var sdf_coarse: texture_3d<f32>;
@group(0) @binding(3) var<storage, read> sdf_top_level: array<u32>;
@group(0) @binding(4) var sdf_sampler: sampler;

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
    // Self-shadow surface bias, in MULTIPLES of the SDF voxel size (occupies the
    // former _pad0 slot — same 144-byte layout). The march starts at
    // `t = surface_bias × voxel` and the closest-passing-distance penumbra term
    // is suppressed inside that window, so the caster's own near-surface field
    // can't shadow itself (the distance-field self-intersection fix; cf. UE
    // mesh/global DF shadows).
    surface_bias: f32,
    sh_grid_dimensions: vec3<u32>,
    _pad1: u32,
};

@group(1) @binding(0) var<uniform> params: ShadowPassParams;
@group(1) @binding(1) var depth_tex: texture_depth_2d;
// DDGI E[d] / E[d^2] depth moments (R = mean, G = mean^2). 3D probe grid.
@group(1) @binding(2) var sh_depth_moments: texture_3d<f32>;
// Half-res output. R/G/B/A = the K = 4 per-light SDF visibility slices (see
// sdf_light_select.wgsl). The animated dominant-direction trace that once
// reserved the G channel is removed — all four channels are per-light now.
@group(1) @binding(3) var shadow_factor: texture_storage_2d<rgba8unorm, write>;

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
    // The baked coarse value (`sdf_bake.rs`, the `coarse_clearance` computation)
    // is a CONSERVATIVE LOWER BOUND on the unsigned distance-to-surface for any
    // point inside the brick, in meters: `min(per-voxel clearance) − half-voxel-
    // diagonal margin`, clamped >= 0. This is a valid sphere-trace step (Hart
    // 1996); it replaced the old per-brick MEAN, which overstated clearance and
    // let the march tunnel through sub-brick occluders (the ~4 m banding). Do NOT
    // re-scale by the brick edge length. The value is already non-negative, so
    // `max(coarse, 0.0)` is a belt-and-braces clamp — no shader change was needed
    // for the mean→min switch. This fallback only fires for non-surface (EMPTY)
    // bricks — bricks whose closest triangle distance exceeds `surface_band_m`
    // (sdf_bake.rs: `near_surface = min_unsigned <= surface_band_m`).
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
//   - surface brick (real slot)  → de-pack the slot to its 3D atlas brick coord,
//                                  map the brick-local position to a texel inside
//                                  the apron'd sub-cube, and `textureSampleLevel`
//                                  once for hardware trilinear filtering.
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
    // top-level traversal and the on-disk layout.
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

    // Surface brick: `slot` is the linear surface-brick index. Each surface brick
    // is stored as a contiguous `(brick_size + 2)^3` sub-cube in the 3D atlas
    // (the uploader scatters it there; the baker fills it z-major with a 1-voxel
    // apron on every side — interior voxels at stored indices [1, brick_size],
    // apron at 0 and brick_size + 1). Sample it once with hardware trilinear.
    //
    // Slot → 3D atlas-brick coordinate (z-major slot order, matching the
    // uploader's `slot % apx, (slot/apx) % apy, slot/(apx*apy)` placement):
    let bricks_per_axis = sdf_meta.atlas_bricks_per_axis;
    let apx = max(bricks_per_axis.x, 1u);
    let apy = max(bricks_per_axis.y, 1u);
    let atlas_brick_coord = vec3<u32>(
        slot % apx,
        (slot / apx) % apy,
        slot / (apx * apy),
    );

    // Stored brick edge includes the 1-voxel apron on both sides.
    let stored_edge = brick_size + 2u;
    let atlas_dim = vec3<f32>(vec3<u32>(apx, apy, max(bricks_per_axis.z, 1u)) * stored_edge);

    // Base texel of this brick's sub-cube in the dense atlas. `+1` skips the
    // low-side apron. `frac * brick_size` ∈ [0, brick_size) is the position in
    // voxels from the brick's low corner — voxel i's center sits at i + 0.5, so
    // hardware trilinear lands on true voxel centers. The apron supplies the
    // neighbor tap at brick edges, keeping seams seamless.
    let base = vec3<f32>(atlas_brick_coord * stored_edge);
    let frac = local - vec3<f32>(brick_coord); // [0,1) within the brick interior
    let texel = base + vec3<f32>(1.0) + frac * f32(brick_size);
    let uvw = texel / atlas_dim;
    let raw = textureSampleLevel(sdf_atlas, sdf_sampler, uvw, 0.0).r;
    // Decode i16 quant steps → meters: step = voxel_size_m / 256 per i16. The
    // f16 atlas stores the same quant-step magnitudes the bake produced.
    return raw * (voxel / SDF_I16_QUANT_STEPS_PER_VOXEL);
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

    // Self-shadow surface bias (distance-field self-intersection fix; cf. UE
    // mesh/global DF shadows). The caster IS baked into the field, so a ray that
    // starts on a lit surface grazes that surface's own ≈0 field near the origin
    // and the `k·d/t` penumbra term reads it as occlusion — the coarse field
    // (0.5 m voxel / 4 m brick mean) rounds this into soft round dark blobs on
    // faces that point at the light. Bias the START off the surface, AND suppress
    // the penumbra `min` while the ray is still inside the bias window, so the
    // caster's own near-surface field is never counted as a shadow caster.
    let voxel = max(sdf_meta.voxel_size_m, 1.0e-4);
    let bias = voxel * max(params.surface_bias, 0.0);
    var t: f32 = bias;
    var factor: f32 = FULLY_LIT;
    let k = max(params.penumbra_k, 1.0);
    let max_t = 64.0; // meters — bounded march length

    // Aaltonen interpolated closest-passing-distance estimator (iquilezles,
    // "Soft Shadows in Raymarched SDFs"): the plain `k·d/t` term samples the
    // penumbra only at discrete steps and misses the true closest approach when
    // it falls between two samples (the inter-step ripple at corners/grazing
    // angles). Reconstruct that closest approach from consecutive samples
    // `ph` (previous) and `h` (current). Seed `ph` large so the first iteration
    // contributes no spurious dark term.
    var ph: f32 = 1.0e10;
    let steps: u32 = clamp(params.max_march_steps, 1u, 256u);
    for (var i: u32 = 0u; i < steps; i = i + 1u) {
        let p = origin + dir * t;
        let h = sample_fine_distance(p);
        // A true hit (ray actually reaches a solid) always shadows — even a hit
        // can't fire inside the bias window because `t` starts past it, so a
        // contact shadow on the floor/wall around the block's base is preserved.
        if (h < voxel * 0.5) {
            return 0.0;
        }
        // Closest-passing-distance penumbra estimate. Skip it while the ray is
        // within the start-bias window: there `h` is dominated by the caster's
        // own surface field, not a separate occluder.
        if (t > bias) {
            let y = h * h / (2.0 * max(ph, voxel * 0.5));
            let estimate = sqrt(max(h * h - y * y, 0.0));
            factor = min(factor, k * estimate / max(t - y, voxel));
        }
        ph = h;
        t = t + max(h, voxel * 0.5);
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

// K-slice channel assignment. K = 4 (SDF_SELECT_K): the four per-light slices
// pack 1:1 into the RGBA channels —
//   slice 0 → R   slice 1 → G   slice 2 → B   slice 3 → A
// Forward reads each sdf light's slice back through this same mapping
// (`slice_for_visibility` in forward.wgsl). The animated dominant-direction
// trace that once owned G is gone.
@compute @workgroup_size(8, 8, 1)
fn cs_main(@builtin(global_invocation_id) gid: vec3<u32>) {
    if (gid.x >= params.half_res_size_x || gid.y >= params.half_res_size_y) {
        return;
    }
    let half_xy = gid.xy;
    let store_xy = vec2<i32>(i32(half_xy.x), i32(half_xy.y));

    // Default: fully lit on every channel. Sky / no-atlas / out-of-volume paths
    // all degrade to 1.0 so the forward multiply is a no-op there.
    var slice0: f32 = FULLY_LIT; // R
    var slice1: f32 = FULLY_LIT; // G
    var slice2: f32 = FULLY_LIT; // B
    var slice3: f32 = FULLY_LIT; // A

    if (sdf_meta.present != 0u) {
        let recon = reconstruct_world(half_xy);
        if (recon.w > 0.5) {
            let world = recon.xyz;

            // Per-light static terms — trace one ray toward each of the K
            // most-influential sdf lights, chosen by the SHARED selection helper
            // so the forward shader shades exactly these same lights. The trace
            // keys on light POSITION, so no lightmap-UV gbuffer is needed.
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
            if (sel.count > 3u) {
                slice3 = trace_light_visibility(world, sel.indices[3]);
            }
        }
    }

    textureStore(
        shadow_factor,
        store_xy,
        vec4<f32>(slice0, slice1, slice2, slice3),
    );
}
