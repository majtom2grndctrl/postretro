// Volumetric fog / beam raymarch.
// See: context/lib/rendering_pipeline.md §7.5
//
// Compute pass over a low-resolution scatter target; one thread per low-res texel.
// Reconstructs a world-space ray from the camera and the full-resolution depth buffer.
// Marches through the fog volume AABB buffer accumulating:
//   - Full L2 SH ambient scatter (world-up normal, composed SH volume)
//   - Dynamic spot-light beam scatter (shadow map occlusion)
//   - Dynamic point-light scatter
// Writes accumulated in-scattering radiance to an RGBA16F storage texture.
//
// Bind groups:
//   group 0  Camera uniforms (reserved; fog shader uses its own fog_params)
//   group 1  Vacant (None in pipeline layout)
//   group 2  Vacant (None in pipeline layout)
//   group 3  SH volume (shared with forward)
//   group 4  Vacant (None in pipeline layout)
//   group 5  Spot shadow maps (shared with forward)
//   group 6  Fog resources: depth, AABB buffer, scatter output, fog params, spot lights, point lights, fog planes

// --- Group 3: SH volume (subset of forward bindings) ---

struct ShGridInfo {
    grid_origin: vec3<f32>,
    has_sh_volume: u32,
    cell_size: vec3<f32>,
    _pad0: u32,
    grid_dimensions: vec3<u32>,
    _pad1: u32,
}

@group(3) @binding(0) var sh_sampler: sampler;
@group(3) @binding(1) var sh_band0: texture_3d<f32>;
@group(3) @binding(2) var sh_band1: texture_3d<f32>;
@group(3) @binding(3) var sh_band2: texture_3d<f32>;
@group(3) @binding(4) var sh_band3: texture_3d<f32>;
@group(3) @binding(5) var sh_band4: texture_3d<f32>;
@group(3) @binding(6) var sh_band5: texture_3d<f32>;
@group(3) @binding(7) var sh_band6: texture_3d<f32>;
@group(3) @binding(8) var sh_band7: texture_3d<f32>;
@group(3) @binding(9) var sh_band8: texture_3d<f32>;
@group(3) @binding(10) var<uniform> sh_grid: ShGridInfo;

// Animated buffers (bindings 11..12) are declared to satisfy the shared
// group-3 layout but are not read here; the fog pass uses only the static
// SH volume for ambient scatter.
struct AnimationDescriptor {
    period: f32,
    phase: f32,
    brightness_offset: u32,
    brightness_count: u32,
    base_color: vec3<f32>,
    color_offset: u32,
    color_count: u32,
    is_active: u32,
    direction_offset: u32,
    direction_count: u32,
}
@group(3) @binding(11) var<storage, read> anim_descriptors: array<AnimationDescriptor>;
@group(3) @binding(12) var<storage, read> anim_samples: array<f32>;

// --- Group 5: Spot shadow maps ---

@group(5) @binding(0) var spot_shadow_depth: texture_depth_2d_array;
@group(5) @binding(1) var spot_shadow_compare: sampler_comparison;

struct LightSpaceMatrices {
    m: array<mat4x4<f32>, 12>,
}
@group(5) @binding(2) var<uniform> light_space_matrices: LightSpaceMatrices;

// Maximum number of fog volumes the shader can process per frame.
// Must match MAX_FOG_VOLUMES in the Rust fog_volume module.
const MAX_FOG_VOLUMES: u32 = 16u;

// Upper bound on `sh_coverage_dist` expressed in steps: the clamp ceiling
// is `MAX_SH_RESAMPLE_STRIDE * step`, so a pathologically small `fog_step_size`
// cannot push the coverage window beyond ~16m (default: 32 × 0.5m).
// Preserves the historical stride-[1, 32] band while expressing the bound
// in distance rather than step count.
const MAX_SH_RESAMPLE_STRIDE: u32 = 32u;

// Upper bound on the depth-tap block size. Matches the FGD `fog_pixel_scale`
// range [1, 8]; runtime `pixel_scale` values truncate via the inner `break`s.
const MAX_PIXEL_SCALE: u32 = 8u;

// World-space coverage budget per cached SH sample, expressed as a multiple of
// the SH grid cell size. SH irradiance is band-limited and the historical
// quality bar (hardcoded stride 8 with default 1m probes / 0.5m step) cached
// one sample for every 4 cells of march distance — that ratio is what this
// constant preserves. Tightening `--probe-spacing` shrinks the cell, which
// shrinks coverage and shortens the stride proportionally; tightening
// `fog.step_size` lengthens the stride to keep meters-per-cache-sample roughly
// constant, capped by `MAX_SH_RESAMPLE_STRIDE`.
const SH_COVERAGE_CELLS: f32 = 4.0;

// --- Group 6: Fog resources ---

// 112 bytes; layout must match `FogVolume` in fx/fog_volume.rs. Each `vec3<f32>`
// is paired with a trailing scalar so WGSL's 16-byte vec3 alignment slots fill
// without internal padding holes. `plane_offset / plane_count` indexes into
// the `fog_planes` storage buffer (group 6 binding 6); `min_brightness`,
// `light_range`, and two pad floats follow as the final 16-byte block.
//
// `center` and `half_diag` are shader-active precomputed fields. `inv_half_ext`
// stores the reciprocal per-axis half-extent and is live on the ellipsoid path
// (`shape_mode == 1.0`); the legacy radial path (`shape_mode == 0.0`) ignores
// it. `shape_mode` is a discriminant flag (0.0 = legacy radial sphere/capsule
// fade against `half_diag`, 1.0 = ellipsoid using `inv_half_ext`); compared
// with `> 0.5` to avoid float precision issues.
//
// `tint` multiplies the per-step scatter color after saturation. Default [1,1,1].
// `saturation` controls color vividness via luma-mix: 0=greyscale, 1=natural,
// >1=boosted. Default 1.0.
// `min_brightness` sets a scatter floor applied *before* tint, so the glow
// takes on the fog's color. Default 0.0.
struct FogVolume {
    min: vec3<f32>,
    density: f32,
    max_v: vec3<f32>,
    // World-unit fade band for primitive (plane-bounded) volumes. Semantic /
    // zero-plane volumes (`fog_lamp`, `fog_tube`) ignore this field and fall
    // back to `radial_falloff` for soft falloff.
    edge_softness: f32,
    center: vec3<f32>,
    half_diag: f32,
    inv_half_ext: vec3<f32>,      // live when shape_mode == 1.0 (ellipsoid)
    shape_mode: f32,              // 0.0 = radial, 1.0 = ellipsoid (compare `> 0.5`)
    tint: vec3<f32>,              // scatter color multiplier; [1,1,1] = no effect
    saturation: f32,              // luma-mix weight; 1.0 = natural; >1 = boosted
    radial_falloff: f32,
    glow: f32,
    plane_offset: u32,
    plane_count: u32,
    // pre-tint scatter floor; `max(step_scatter, min_brightness)` applied before saturation
    // and tint. Default 0.0 (no floor).
    min_brightness: f32,
    // per-volume light range multiplier; higher = lights reach farther inside fog.
    // Default 1.0 (same reach as open air).
    light_range: f32,
    _pad6_a: f32,
    _pad6_b: f32,
}

struct FogPointLight {
    position: vec3<f32>,
    range: f32,
    color: vec3<f32>,
    _pad: f32,
}

// Must match `FogParams` layout in fog_volume.rs::pack_fog_params.
// WGSL rounds the 100-byte struct to 112 via 16-byte alignment (from `mat4x4`);
// CPU side adds explicit `_pad2` to match.
struct FogParams {
    inv_view_proj: mat4x4<f32>,
    camera_position: vec3<f32>,
    step_size: f32,
    active_count: u32,
    near_clip: f32,
    far_clip: f32,
    point_count: u32,
    spot_count: u32,
}

// One entry per dynamic spot shadow slot. Packed CPU-side from MapLight +
// slot assignment so the fog shader doesn't have to reach into the forward
// pass's dynamic-light buffer.
//
// Layout must match fog_volume.rs::pack_fog_spot_lights:
//   0..12    position   (vec3<f32>)
//   12..16   slot       (u32; 0xFFFFFFFF = unused)
//   16..28   direction  (vec3<f32>, unit)
//   28..32   cos_outer  (f32)
//   32..44   color      (vec3<f32>, color × intensity)
//   44..48   range      (f32)
struct FogSpotLight {
    position: vec3<f32>,
    slot: u32,
    direction: vec3<f32>,
    cos_outer: f32,
    color: vec3<f32>,
    range: f32,
}

@group(6) @binding(0) var depth_texture: texture_depth_2d;
@group(6) @binding(1) var<storage, read> fog_volumes: array<FogVolume>;
@group(6) @binding(2) var scatter_output: texture_storage_2d<rgba16float, write>;
@group(6) @binding(3) var<uniform> fog: FogParams;
@group(6) @binding(4) var<storage, read> fog_spots: array<FogSpotLight>;
@group(6) @binding(5) var<storage, read> fog_points: array<FogPointLight>;
// Flat plane buffer indexed by per-volume `(plane_offset, plane_count)`. Each
// plane is `(nx, ny, nz, d)`; a sample is inside the volume iff
// `dot(pos, n) <= d` for every plane in the volume's range.
@group(6) @binding(6) var<storage, read> fog_planes: array<vec4<f32>>;

// --- SH ambient sampling ---
//
// Copy-pasted from forward.wgsl (sh_irradiance + sample_sh_indirect_fast).
// WGSL has no include mechanism — this is a source-level copy, not string-concat
// composition. Current consumers: fog_volume.wgsl + forward.wgsl +
// billboard.wgsl. Extraction is deferred; see rendering_pipeline.md §8 for
// the extraction pattern.

fn sh_irradiance(
    b0: vec3<f32>, b1: vec3<f32>, b2: vec3<f32>, b3: vec3<f32>,
    b4: vec3<f32>, b5: vec3<f32>, b6: vec3<f32>, b7: vec3<f32>, b8: vec3<f32>,
    normal: vec3<f32>,
) -> vec3<f32> {
    let nx = normal.x;
    let ny = normal.y;
    let nz = normal.z;
    var r: vec3<f32> = b0 * 0.282095;
    r = r + b1 * (-0.488603 * ny);
    r = r + b2 * ( 0.488603 * nz);
    r = r + b3 * (-0.488603 * nx);
    r = r + b4 * ( 1.092548 * nx * ny);
    r = r + b5 * (-1.092548 * ny * nz);
    r = r + b6 * ( 0.315392 * (3.0 * nz * nz - 1.0));
    r = r + b7 * (-1.092548 * nx * nz);
    r = r + b8 * ( 0.546274 * (nx * nx - ny * ny));
    return r;
}

fn sample_sh_indirect_fast(
    normal: vec3<f32>,
    gi: vec3<u32>,
    gfrac: vec3<f32>,
) -> vec3<f32> {
    // Raw cell count used as the UV divisor. `gi` < `gdims_f` is guaranteed
    // because sample_sh_fog pre-clamps gf to [0, gdims_u - 1].
    let gdims_f = max(vec3<f32>(sh_grid.grid_dimensions), vec3<f32>(1.0));
    let cell_center_uvw = (vec3<f32>(gi) + vec3<f32>(0.5) + gfrac) / gdims_f;
    let b0 = textureSampleLevel(sh_band0, sh_sampler, cell_center_uvw, 0.0).rgb;
    let b1 = textureSampleLevel(sh_band1, sh_sampler, cell_center_uvw, 0.0).rgb;
    let b2 = textureSampleLevel(sh_band2, sh_sampler, cell_center_uvw, 0.0).rgb;
    let b3 = textureSampleLevel(sh_band3, sh_sampler, cell_center_uvw, 0.0).rgb;
    let b4 = textureSampleLevel(sh_band4, sh_sampler, cell_center_uvw, 0.0).rgb;
    let b5 = textureSampleLevel(sh_band5, sh_sampler, cell_center_uvw, 0.0).rgb;
    let b6 = textureSampleLevel(sh_band6, sh_sampler, cell_center_uvw, 0.0).rgb;
    let b7 = textureSampleLevel(sh_band7, sh_sampler, cell_center_uvw, 0.0).rgb;
    let b8 = textureSampleLevel(sh_band8, sh_sampler, cell_center_uvw, 0.0).rgb;

    return max(
        sh_irradiance(b0, b1, b2, b3, b4, b5, b6, b7, b8, normal),
        vec3<f32>(0.0),
    );
}

// World-up normal: fog is directionally isotropic, and an overhead-ambient
// reading is the most stable single-direction probe for the L2 reconstruction.
// No surface-normal offset — the wall-bleed mitigation in forward.wgsl has no
// meaning in fog.
fn sample_sh_fog(world_pos: vec3<f32>) -> vec3<f32> {
    if sh_grid.has_sh_volume == 0u {
        return vec3<f32>(0.0);
    }
    let normal = vec3<f32>(0.0, 1.0, 0.0);
    let gdims_u = sh_grid.grid_dimensions;
    // Last valid cell index — clamp ceiling for the trilinear fetch in sample_sh_indirect_fast.
    let gdims_f = max(vec3<f32>(gdims_u) - vec3<f32>(1.0), vec3<f32>(0.0));
    let cell_coord = (world_pos - sh_grid.grid_origin) /
        max(sh_grid.cell_size, vec3<f32>(1.0e-6));
    let gf = clamp(cell_coord, vec3<f32>(0.0), gdims_f);
    let gi = vec3<u32>(floor(gf));
    let gfrac = fract(gf);
    return sample_sh_indirect_fast(normal, gi, gfrac);
}

// --- Shadow sampling (matches forward.wgsl::sample_spot_shadow) ---

fn sample_spot_shadow_pt(
    slot_index: u32,
    world_pos: vec3<f32>,
    light_proj: mat4x4<f32>,
) -> f32 {
    let light_clip = light_proj * vec4<f32>(world_pos, 1.0);
    if light_clip.w <= 0.0 {
        return 1.0;
    }
    let light_ndc = light_clip.xyz / light_clip.w;
    let uv = vec2<f32>(light_ndc.x * 0.5 + 0.5, light_ndc.y * -0.5 + 0.5);
    if uv.x < 0.0 || uv.x > 1.0
        || uv.y < 0.0 || uv.y > 1.0
        || light_ndc.z < 0.0 || light_ndc.z > 1.0 {
        return 1.0;
    }
    // textureSampleCompare is fragment-only; use textureLoad + manual compare.
    // Single-tap hard shadow is acceptable here — the volumetric integration
    // already smooths the result.
    let dims = textureDimensions(spot_shadow_depth);
    let tc = vec2<i32>(
        clamp(i32(uv.x * f32(dims.x)), 0, i32(dims.x) - 1),
        clamp(i32(uv.y * f32(dims.y)), 0, i32(dims.y) - 1),
    );
    let stored_depth = textureLoad(spot_shadow_depth, tc, i32(slot_index), 0);
    return select(0.0, 1.0, light_ndc.z <= stored_depth);
}

// --- World ray reconstruction from low-res fragment UV + full-res depth ---

struct ViewRay {
    origin: vec3<f32>,
    direction: vec3<f32>,
    /// World-space distance from camera to the first opaque surface. If the
    /// depth buffer sampled `1.0` (no geometry), this is the far clip.
    max_t: f32,
}

fn reconstruct_ray(uv: vec2<f32>, depth_ndc: f32) -> ViewRay {
    // Clip-space ray endpoints at the near plane (ndc.z=0) and at the sampled
    // depth. WGPU NDC xy is [-1, 1], depth is [0, 1].
    let ndc_xy = vec2<f32>(uv.x * 2.0 - 1.0, (1.0 - uv.y) * 2.0 - 1.0);
    let clip_near = vec4<f32>(ndc_xy, 0.0, 1.0);
    let clip_far = vec4<f32>(ndc_xy, 1.0, 1.0);
    let world_near = fog.inv_view_proj * clip_near;
    let world_far = fog.inv_view_proj * clip_far;
    let wn = world_near.xyz / world_near.w;
    let wf = world_far.xyz / world_far.w;
    let dir = normalize(wf - wn);

    var ray: ViewRay;
    ray.origin = fog.camera_position;
    ray.direction = dir;

    // Convert the sampled depth into a world-space ray distance. When
    // `depth_ndc == 1.0` the projected point is at infinity; cap at far clip.
    if depth_ndc >= 0.999999 {
        ray.max_t = fog.far_clip;
    } else {
        let clip_hit = vec4<f32>(ndc_xy, depth_ndc, 1.0);
        let world_hit = fog.inv_view_proj * clip_hit;
        let wp = world_hit.xyz / world_hit.w;
        ray.max_t = length(wp - ray.origin);
    }
    return ray;
}

// --- Compute entry ---

@compute @workgroup_size(8, 8, 1)
fn cs_main(@builtin(global_invocation_id) gid: vec3<u32>) {
    let out_dims = textureDimensions(scatter_output);
    if gid.x >= out_dims.x || gid.y >= out_dims.y {
        return;
    }
    let depth_dims = textureDimensions(depth_texture);
    let uv = (vec2<f32>(gid.xy) + vec2<f32>(0.5)) / vec2<f32>(out_dims);

    // Min-over-block depth tap: take the closest hit across every full-res
    // depth texel covered by this low-res scatter texel. A single nearest
    // sample lets fog bleed through thin silhouettes when the sub-pixel that
    // actually contained the foreground geometry isn't the one we picked;
    // min-reducing the block selects the nearest surface, which is the right
    // upper bound for the ray's `max_t`. The loop is bounded by the compile-
    // time constant `MAX_PIXEL_SCALE` so WGSL can unroll/bound it; runtime
    // `pixel_scale` values truncate via the inner `break`s. The
    // `min(..., depth_dims - 1)` clamp handles window sizes that aren't an
    // exact multiple of `pixel_scale`.
    let ps_x = depth_dims.x / out_dims.x;
    let ps_y = depth_dims.y / out_dims.y;
    let base = vec2<u32>(gid.x * ps_x, gid.y * ps_y);
    // depth_dims is always > 0 (the surface depth texture is never zero-sized), so this subtraction never wraps.
    let depth_max = depth_dims - vec2<u32>(1u);
    var depth_ndc: f32 = 1.0;
    for (var dy: u32 = 0u; dy < MAX_PIXEL_SCALE; dy = dy + 1u) {
        if dy >= ps_y { break; }
        for (var dx: u32 = 0u; dx < MAX_PIXEL_SCALE; dx = dx + 1u) {
            if dx >= ps_x { break; }
            let sx = min(base.x + dx, depth_max.x);
            let sy = min(base.y + dy, depth_max.y);
            let sample = textureLoad(depth_texture, vec2<i32>(vec2<u32>(sx, sy)), 0);
            depth_ndc = min(depth_ndc, sample);
        }
    }
    let ray = reconstruct_ray(uv, depth_ndc);

    let step = max(fog.step_size, 1.0e-3);
    let start_t = fog.near_clip;
    var transmittance: f32 = 1.0;
    var accum: vec3<f32> = vec3<f32>(0.0);

    // Cap the iteration count so a huge far distance doesn't hang the shader.
    // 256 steps × default 0.5m = 128m reach before early-out. Maps that need
    // more can reduce `fog_step_size`; plan target is <2ms/pass.
    let max_steps: u32 = 256u;
    // Loop over the CPU-tracked prefix (`fog.spot_count`) instead of
    // `arrayLength(&fog_spots)` so a frame that uploads fewer spots than the
    // previous frame doesn't re-iterate stale records left in the buffer
    // (the buffer is sized for SHADOW_POOL_SIZE and never shrinks).
    let spot_count = fog.spot_count;

    // --- Slab-clip prologue ---------------------------------------------------
    // Compute the union of [t_enter, t_exit] intervals over all active fog
    // volumes, clamped to [start_t, ray.max_t]. The march only iterates inside
    // these sub-intervals; an empty union skips the loop entirely.
    //
    // IEEE-inf on zero ray-direction components is the standard slab-test
    // behavior — `(min - origin) / 0` propagates to ±inf and the min/max
    // composition handles axis-aligned rays correctly without epsilon hacks.
    let inv_d = vec3<f32>(1.0) / ray.direction;

    // We track raw [enter, exit] hits per active volume, then sort-merge into
    // a disjoint union. Array sized to MAX_FOG_VOLUMES. `raw_idx` carries the
    // original `fog_volumes` index for each hit so the inlined per-step volume
    // sampling can iterate only volumes whose AABB the ray crosses (a strict
    // subset of `fog.active_count`).
    var raw_enter: array<f32, MAX_FOG_VOLUMES>;
    var raw_exit: array<f32, MAX_FOG_VOLUMES>;
    var raw_idx: array<u32, MAX_FOG_VOLUMES>;
    var raw_count: u32 = 0u;

    let vc = fog.active_count;
    for (var i: u32 = 0u; i < vc; i = i + 1u) {
        let v = fog_volumes[i];
        let t_min = (v.min - ray.origin) * inv_d;
        let t_max = (v.max_v - ray.origin) * inv_d;
        let t_lo = min(t_min, t_max);
        let t_hi = max(t_min, t_max);
        let t_near = max(max(t_lo.x, t_lo.y), t_lo.z);
        let t_far = min(min(t_hi.x, t_hi.y), t_hi.z);

        if t_near < t_far && t_far > start_t && t_near < ray.max_t {
            let enter = max(t_near, start_t);
            let exit = min(t_far, ray.max_t);
            if enter < exit {
                if raw_count < MAX_FOG_VOLUMES {
                    raw_enter[raw_count] = enter;
                    raw_exit[raw_count] = exit;
                    raw_idx[raw_count] = i;
                    raw_count = raw_count + 1u;
                }
            }
        }
    }

    // Merge raw hits into a disjoint, sorted union.
    // `fog.active_count` is capped at MAX_FOG_VOLUMES, so raw_count <=
    // MAX_FOG_VOLUMES is always satisfied — no overflow path is needed.
    var union_enter: array<f32, MAX_FOG_VOLUMES>;
    var union_exit: array<f32, MAX_FOG_VOLUMES>;
    var union_count: u32 = 0u;

    // Selection sort by enter (raw_count <= MAX_FOG_VOLUMES, so O(n^2) is fine).
    for (var a: u32 = 0u; a < raw_count; a = a + 1u) {
        var best = a;
        for (var b: u32 = a + 1u; b < raw_count; b = b + 1u) {
            if raw_enter[b] < raw_enter[best] {
                best = b;
            }
        }
        if best != a {
            let te = raw_enter[a];
            let tx = raw_exit[a];
            let ti = raw_idx[a];
            raw_enter[a] = raw_enter[best];
            raw_exit[a] = raw_exit[best];
            raw_idx[a] = raw_idx[best];
            raw_enter[best] = te;
            raw_exit[best] = tx;
            raw_idx[best] = ti;
        }
    }
    // Sweep-merge overlapping/touching intervals.
    for (var k: u32 = 0u; k < raw_count; k = k + 1u) {
        let e = raw_enter[k];
        let x = raw_exit[k];
        if union_count == 0u || e > union_exit[union_count - 1u] {
            union_enter[union_count] = e;
            union_exit[union_count] = x;
            union_count = union_count + 1u;
        } else {
            union_exit[union_count - 1u] = max(union_exit[union_count - 1u], x);
        }
    }

    // No fog volume intersects the ray — skip the march entirely.
    if union_count == 0u {
        textureStore(scatter_output, vec2<i32>(gid.xy), vec4<f32>(accum, 1.0 - transmittance));
        return;
    }

    var step_count: u32 = 0u;

    // SH irradiance is band-limited and the SH grid is much coarser than the
    // march step size. Sampling 9 trilinear 3D fetches per step is wasted
    // bandwidth — we cache one sample and refresh whenever the march has
    // advanced more than `sh_coverage_dist` world units since the last sample
    // (reset at each sub-interval boundary) to bound drift without per-step cost.
    // The cache lives in scalar locals (no array, no callee pointer) so it stays
    // in registers and never hits the Metal private-memory trap.
    //
    // Why distance-based, not step-count-based: an animated fog_lamp density
    // shifts the `transmittance < 0.01` early-out break point by ±1 step
    // frame-to-frame. With a step-count schedule, that ±1 step shift changes
    // which `cached_sh` value governs the final step (up to stride-1 steps
    // stale) for radial volumes, where per-step `fade` varies sharply with
    // position — producing a frame-to-frame radiance discontinuity (flicker).
    // A distance-based schedule is frame-stable: the t-sequence is
    // deterministic per ray, so `cached_sh` at step k holds the same value
    // every frame regardless of where the early-out fires. The animated
    // early-out only controls whether a smooth additional contribution lands
    // — it does not alter which cached value governed prior steps.
    //
    // `sh_coverage_dist` is derived once per ray from the SH grid cell size,
    // so the meters-per-cache-sample budget stays proportional to the baked
    // SH resolution regardless of `--probe-spacing` or `fog_step_size`.
    // `cell_size` is a per-axis world-space length (matches
    // `probe_spacing_meters` on the host); taking the minimum component is
    // the conservative choice for anisotropic grids. Floored at `step` (one
    // sample per step minimum) and capped at `MAX_SH_RESAMPLE_STRIDE * step`
    // to keep pathological inputs (very small `step_size`) bounded — these
    // bounds preserve the historical stride [1, MAX_SH_RESAMPLE_STRIDE]
    // semantics, just expressed in distance.
    //
    // Default-case sanity: cell_size = 1.0m, step = 0.5m →
    //   sh_coverage_dist = clamp(4.0, 0.5, 16.0) = 4.0m, i.e. ~one sample
    //   every 8 steps — matches the previous hardcoded stride 8, so default
    //   visual output is unchanged.
    let cell_min = min(min(sh_grid.cell_size.x, sh_grid.cell_size.y), sh_grid.cell_size.z);
    let sh_coverage_dist = clamp(
        SH_COVERAGE_CELLS * cell_min,
        step,
        f32(MAX_SH_RESAMPLE_STRIDE) * step,
    );
    var cached_sh: vec3<f32> = vec3<f32>(0.0);
    // Sentinel triggering a fresh sample on the first eligible step. Any value
    // <= -sh_coverage_dist works; -1e30 is safely beyond any plausible `t`.
    var t_last_sh_sample: f32 = -1.0e30;

    for (var ui: u32 = 0u; ui < union_count; ui = ui + 1u) {
        if transmittance < 0.01 { break; }
        if step_count >= max_steps { break; }

        let sub_enter = union_enter[ui];
        let sub_exit = union_exit[ui];
        // Align the first step inside the sub-interval to a half-step offset
        // (matches the original `start_t = step * 0.5` cadence).
        var t = sub_enter + step * 0.5;
        // Force a fresh SH sample at the first eligible step in each new
        // sub-interval (gaps between sub-intervals can be large).
        t_last_sh_sample = -1.0e30;

        loop {
            if t >= sub_exit { break; }
            if transmittance < 0.01 { break; }
            if step_count >= max_steps { break; }
            step_count = step_count + 1u;

            let pos = ray.origin + ray.direction * t;

            // Inlined per-step volume sampling (Metal/Apple Silicon constraint).
            // A callee taking `ptr<function, array<...>>` for the per-ray
            // active-volume index list cannot be register-promoted on Metal —
            // the array spills to device-private memory, replacing well-coalesced
            // storage-buffer reads with poorly-coalesced private reads. A previous
            // attempt at a function-local cache (commit b93d31e, reverted by
            // bda93f4) hit exactly this trap. Keeping the body inline keeps the
            // index array in registers/local scope. Iterates only volumes whose
            // AABB the ray crosses (`raw_idx[0..raw_count]`, a strict subset of
            // `fog.active_count`), reading each volume from the storage buffer
            // with the same coalesced loads as before — just fewer of them per step.
            var vs_density: f32 = 0.0;
            var vs_glow: f32 = 0.0;
            // Density-weighted tint and saturation accumulated over overlapping volumes.
            // Divided by vs_density after the loop to get the blended value.
            var vs_tint_accum: vec3<f32> = vec3<f32>(0.0);
            var vs_sat_accum: f32 = 0.0;
            // min_brightness and light_range use the same density-weighted blend
            // as tint/saturation: accumulated proportionally to each volume's density
            // contribution, then divided by total density after the loop.
            var vs_min_brightness_accum: f32 = 0.0;
            var vs_light_range_accum: f32 = 0.0;
            for (var rk: u32 = 0u; rk < raw_count; rk = rk + 1u) {
                let v = fog_volumes[raw_idx[rk]];
                // AABB still gates entry — the slab-clip prologue narrowed the
                // ray-vs-volume box, but a step inside the union envelope can
                // still fall outside an individual volume's box.
                if pos.x < v.min.x || pos.y < v.min.y || pos.z < v.min.z {
                    continue;
                }
                if pos.x > v.max_v.x || pos.y > v.max_v.y || pos.z > v.max_v.z {
                    continue;
                }

                var fade: f32;

                if v.plane_count > 0u {
                    // Primitive path: convex brush bounded by `plane_count`
                    // half-spaces. `min_signed_dist` is the signed distance to
                    // the nearest face boundary — positive inside, negative
                    // outside. We iterate every plane (no early-exit) so the
                    // same value drives both the inside test and the
                    // edge-softness fade.
                    var min_signed_dist: f32 = 1.0e30;
                    for (var pi: u32 = 0u; pi < v.plane_count; pi = pi + 1u) {
                        let p = fog_planes[v.plane_offset + pi];
                        min_signed_dist = min(min_signed_dist, p.w - dot(pos, p.xyz));
                    }
                    if min_signed_dist < 0.0 {
                        continue;
                    }
                    // Strict `> 0.0` guard avoids divide-by-zero: when
                    // edge_softness == 0 the volume is a hard cutoff — full
                    // density inside, no fade band.
                    fade = select(1.0, saturate(min_signed_dist / v.edge_softness), v.edge_softness > 0.0);
                } else {
                    if v.shape_mode > 0.5 {
                        // Ellipsoid path: normalize the offset by per-axis
                        // half-extents so |rel * inv_half_ext| == 1 traces
                        // the ellipsoid surface.
                        let rel = pos - v.center;
                        let d = rel * v.inv_half_ext;
                        let ellipsoid_t2 = saturate(dot(d, d));
                        let radial_inv = 1.0 - ellipsoid_t2;
                        if v.radial_falloff <= 0.0 {
                            fade = 1.0;
                        } else {
                            fade = pow(max(radial_inv, 1.0e-6), v.radial_falloff);
                        }
                    } else {
                        // Semantic path: AABB-only membership with a centered
                        // radial fade shaped by `radial_falloff` (`fog_lamp`
                        // sphere, `fog_tube` capsule).
                        let radial_t = clamp(length(pos - v.center) / max(v.half_diag, 1.0e-6), 0.0, 1.0);
                        let radial_inv = 1.0 - radial_t;
                        // Guard against pow(0,0) NaN: clamp base away from
                        // zero. `pow` is only reached when radial_falloff > 0
                        // (wave-uniform branch).
                        if v.radial_falloff <= 0.0 {
                            fade = 1.0;
                        } else {
                            fade = pow(max(radial_inv, 1.0e-6), v.radial_falloff);
                        }
                    }
                }

                let contrib = v.density * fade;
                vs_density = vs_density + contrib;
                vs_glow = max(vs_glow, v.glow);
                vs_tint_accum = vs_tint_accum + contrib * v.tint;
                vs_sat_accum = vs_sat_accum + contrib * v.saturation;
                vs_min_brightness_accum = vs_min_brightness_accum + contrib * v.min_brightness;
                vs_light_range_accum = vs_light_range_accum + contrib * v.light_range;
            }

            if vs_density > 0.0 {
                // Normalize density-weighted tint and saturation.
                let inv_density = 1.0 / vs_density;
                let vs_tint = vs_tint_accum * inv_density;
                let vs_saturation = vs_sat_accum * inv_density;
                let vs_min_brightness = vs_min_brightness_accum * inv_density;
                let vs_light_range = vs_light_range_accum * inv_density;

                // Glow weight for this step.
                let weight = vs_density * vs_glow * step;

                // Accumulate all light contributions for this step into a local
                // color, then apply saturation and tint before folding into accum.
                var step_scatter: vec3<f32> = vec3<f32>(0.0);

                // Distance-based refresh: resample when the march has advanced
                // at least `sh_coverage_dist` world units past the last sample.
                // This keeps the cache schedule a function of world position
                // (not step index), so animated-density-induced ±1 step shifts
                // at the early-out don't change which cached value governs the
                // boundary step.
                if t - t_last_sh_sample >= sh_coverage_dist {
                    cached_sh = sample_sh_fog(pos);
                    t_last_sh_sample = t;
                }
                step_scatter = step_scatter + cached_sh;

                // Dynamic spot beams.
                for (var li: u32 = 0u; li < spot_count; li = li + 1u) {
                    let spot = fog_spots[li];
                    if spot.slot == 0xFFFFFFFFu { continue; }

                    let to_light = spot.position - pos;
                    let dist = length(to_light);
                    if dist > spot.range || dist < 1.0e-4 { continue; }
                    let l = to_light / dist;

                    // Cone test: the stored `direction` is the aim (light → target),
                    // so compare dot(-l, direction) against cos(outer).
                    let cos_aim = dot(-l, spot.direction);
                    if cos_aim < spot.cos_outer { continue; }

                    // Distance falloff (linear — matches FalloffModel::Linear baseline;
                    // beams are aesthetic, subtle differences between falloff models
                    // aren't worth an extra branch here).
                    let atten = clamp(1.0 - dist / (spot.range * vs_light_range), 0.0, 1.0);

                    // Shadow map occlusion.
                    let lit = sample_spot_shadow_pt(
                        spot.slot,
                        pos,
                        light_space_matrices.m[spot.slot],
                    );
                    if lit <= 0.0 { continue; }

                    step_scatter = step_scatter + spot.color * atten * lit;
                }

                // Loop over the CPU-tracked prefix (`fog.point_count`) instead of
                // `arrayLength(&fog_points)` so a frame that uploads zero point
                // lights doesn't re-iterate stale records left in the buffer from
                // a previous frame.
                for (var pi: u32 = 0u; pi < fog.point_count; pi = pi + 1u) {
                    let pt = fog_points[pi];
                    let to_light = pt.position - pos;
                    let dist = length(to_light);
                    if dist > pt.range || dist < 1.0e-4 { continue; }
                    let atten = clamp(1.0 - dist / (pt.range * vs_light_range), 0.0, 1.0);
                    step_scatter = step_scatter + pt.color * atten;
                }

                step_scatter = max(step_scatter, vec3<f32>(vs_min_brightness));

                // Apply saturation: mix luma toward full color.
                // vs_saturation > 1 extrapolates beyond natural color (boosted saturation).
                let luma = dot(step_scatter, vec3<f32>(0.299, 0.587, 0.114));
                step_scatter = mix(vec3<f32>(luma), step_scatter, vs_saturation);
                // Apply tint.
                step_scatter = step_scatter * vs_tint;

                accum = accum + transmittance * weight * step_scatter;

                transmittance = transmittance * exp(-vs_density * step);
            }

            t = t + step;
        }
    }

    textureStore(scatter_output, vec2<i32>(gid.xy), vec4<f32>(accum, 1.0 - transmittance));
}

// Keeps bindings reflected so wgpu does not reject the pipeline. wgpu
// rejects a pipeline when the BindGroupLayout declares a binding that shader
// reflection omits. The fog pass shares group 3 with the forward pipeline
// and group 5's sampler_comparison with forward's shadow pass — both
// layouts must be satisfied even though fog reads depth via textureLoad.
fn _keep_bindings_live() -> f32 {
    let d = anim_descriptors[0];
    let a = anim_samples[0];
    // WGSL dead-code elimination strips bindings that are never referenced.
    // wgpu then rejects the pipeline because the BindGroupLayout declares those
    // bindings but the shader reflection omits them. Touching each binding here
    // keeps it in the reflected interface without affecting rendering output.
    _ = textureSampleCompareLevel(spot_shadow_depth, spot_shadow_compare, vec2<f32>(0.0), 0, 0.0);
    return d.period + a;
}
