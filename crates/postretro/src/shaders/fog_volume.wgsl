// Volumetric fog / beam raymarch.
// See: context/lib/rendering_pipeline.md §7.5
//
// Runs as a compute pass over a low-resolution scatter target.
// One thread per low-res texel. Reconstructs a world-space ray from the
// camera and the full-resolution depth buffer, marches through the fog
// volume AABB buffer accumulating SH ambient scatter + dynamic spot-light
// beam scatter (with shadow map occlusion), and writes the accumulated
// in-scattering radiance to an RGBA16F storage texture.
//
// Bind groups:
//   group 0  Camera uniforms (reserved; fog shader uses its own fog_params)
//   group 1  Vacant (None in pipeline layout)
//   group 2  Vacant (None in pipeline layout)
//   group 3  SH volume (shared with forward)
//   group 4  Vacant (None in pipeline layout)
//   group 5  Spot shadow maps (shared with forward)
//   group 6  Fog resources: depth, AABB buffer, scatter output, fog params

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

// --- Group 6: Fog resources ---

struct FogVolume {
    min: vec3<f32>,
    density: f32,
    max_v: vec3<f32>,
    falloff: f32,
    color: vec3<f32>,
    scatter: f32,
    height_gradient: f32,
    radial_falloff: f32,
    _pad0: f32,
    _pad1: f32,
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
    volume_count: u32,
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

// --- SH ambient sampling (positional only — cosine-lobe evaluation with a
// neutral "up" normal gives a reasonable fog ambient tint) ---

fn sh_reconstruct_l0(b0: vec3<f32>) -> vec3<f32> {
    // L0 band alone is a uniform ambient scalar; no directional shaping.
    return b0 * 0.282095;
}

fn sample_sh_ambient(world_pos: vec3<f32>) -> vec3<f32> {
    if sh_grid.has_sh_volume == 0u {
        return vec3<f32>(0.0);
    }
    let gdims_f = max(vec3<f32>(sh_grid.grid_dimensions), vec3<f32>(1.0));
    let grid_pos = (world_pos - sh_grid.grid_origin) / max(sh_grid.cell_size, vec3<f32>(1.0e-6));
    let clamped = clamp(grid_pos, vec3<f32>(0.0), gdims_f - vec3<f32>(1.0));
    let uvw = (clamped + vec3<f32>(0.5)) / gdims_f;
    let b0 = textureSampleLevel(sh_band0, sh_sampler, uvw, 0.0).rgb;
    // L0 is the DC (ambient) term — sufficient as the isotropic fog base.
    return max(sh_reconstruct_l0(b0), vec3<f32>(0.0));
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

// --- AABB membership + accumulated volume lookup ---

struct VolumeSample {
    density: f32,
    color: vec3<f32>,
    scatter: f32,
    hits: u32,
}

fn sample_fog_volumes(pos: vec3<f32>) -> VolumeSample {
    var out: VolumeSample;
    out.density = 0.0;
    out.color = vec3<f32>(0.0);
    out.scatter = 0.0;
    out.hits = 0u;
    let n = fog.volume_count;
    for (var i: u32 = 0u; i < n; i = i + 1u) {
        let v = fog_volumes[i];
        if pos.x < v.min.x || pos.y < v.min.y || pos.z < v.min.z {
            continue;
        }
        if pos.x > v.max_v.x || pos.y > v.max_v.y || pos.z > v.max_v.z {
            continue;
        }
        let half_ext = (v.max_v - v.min) * 0.5;
        let center   = (v.min + v.max_v) * 0.5;
        let local_abs = abs(pos - center) / max(half_ext, vec3<f32>(1.0e-6));
        let edge_t   = 1.0 - clamp(max(local_abs.x, max(local_abs.y, local_abs.z)), 0.0, 1.0);
        // Guard against pow(0,0) NaN: clamp both base and exponent away from zero.
        let box_fade = select(pow(max(edge_t, 1.0e-6), max(v.falloff, 1.0e-6)), 1.0, v.falloff <= 0.0);

        let height_t    = clamp((pos.y - v.min.y) / max(v.max_v.y - v.min.y, 1.0e-6), 0.0, 1.0);
        let height_fade = clamp(1.0 - height_t * v.height_gradient, 0.0, 1.0);

        let half_diag   = length(half_ext);
        let radial_t    = clamp(length(pos - center) / max(half_diag, 1.0e-6), 0.0, 1.0);
        let radial_inv  = 1.0 - radial_t;
        let radial_fade = select(pow(max(radial_inv, 1.0e-6), max(v.radial_falloff, 1.0e-6)), 1.0, v.radial_falloff <= 0.0);

        let fade = box_fade * height_fade * radial_fade;
        out.density = out.density + v.density * fade;
        out.color = out.color + v.color * v.density * fade;
        out.scatter = max(out.scatter, v.scatter);
        out.hits = out.hits + 1u;
    }
    if out.density > 0.0 {
        out.color = out.color / out.density;
    }
    return out;
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

    // Nearest full-res depth texel for this low-res fragment.
    let depth_xy = vec2<u32>(
        min(u32(uv.x * f32(depth_dims.x)), depth_dims.x - 1u),
        min(u32(uv.y * f32(depth_dims.y)), depth_dims.y - 1u),
    );
    let depth_ndc = textureLoad(depth_texture, vec2<i32>(depth_xy), 0);
    let ray = reconstruct_ray(uv, depth_ndc);

    let step = max(fog.step_size, 1.0e-3);
    let start_t = max(fog.near_clip, step * 0.5);
    var t = start_t;
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

    for (var s: u32 = 0u; s < max_steps; s = s + 1u) {
        if t >= ray.max_t { break; }
        if transmittance < 0.01 { break; }

        let pos = ray.origin + ray.direction * t;
        let vs = sample_fog_volumes(pos);
        if vs.density > 0.0 {
            // Scatter weight for this step.
            let weight = vs.density * vs.scatter * step;

            // SH ambient contribution (tinted by the fog color).
            let sh_amb = sample_sh_ambient(pos);
            accum = accum + transmittance * weight * vs.color * sh_amb;

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
                let atten = clamp(1.0 - dist / spot.range, 0.0, 1.0);

                // Shadow map occlusion.
                let lit = sample_spot_shadow_pt(
                    spot.slot,
                    pos,
                    light_space_matrices.m[spot.slot],
                );
                if lit <= 0.0 { continue; }

                accum = accum + transmittance * weight * vs.color * spot.color * atten * lit;
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
                let atten = clamp(1.0 - dist / pt.range, 0.0, 1.0);
                accum = accum + transmittance * weight * pt.color * atten;
            }

            transmittance = transmittance * exp(-vs.density * step);
        }

        t = t + step;
    }

    textureStore(scatter_output, vec2<i32>(gid.xy), vec4<f32>(accum, 1.0 - transmittance));
}

// Silence "unused binding" warnings for the animated buffers and the
// comparison sampler. The fog pass shares group 3 with the forward pipeline
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
