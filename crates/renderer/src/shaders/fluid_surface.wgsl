// Fluid surface pass: translucent, two-sided fluid faces moving in whole texels,
// lit per fragment by baked SH indirect, baked direct SH, and dynamic lights.
// See: context/lib/rendering_pipeline.md §7.3a

// --- Group 0: camera uniforms (shared with forward and billboard) ---
// Field offsets mirror the Rust `Uniforms` writer; see billboard.wgsl.
struct Uniforms {
    view_proj: mat4x4<f32>,
    camera_position: vec3<f32>,
    ambient_floor: f32,
    // Dynamic-tier record count: the prefix of `lights`.
    light_count: u32,
    // Level-relative animation clock (`script_time`), seconds.
    time: f32,
    light_term_mask: u32,
    indirect_scale: f32,
    sdf_shadow_flags: u32,
    sdf_shadow_mode: u32,
    sdf_force_visibility_one: u32,
    direct_scale: f32,
    has_scatter: u32,
    // Bit 0: a baked DIRECT SH section is present. Bits 1..31: the raw
    // section-45 AnimatedBakedLights tail count.
    has_direct: u32,
    // Dynamic tier, then the mover-only animated-baked tail, then promoted static.
    total_light_count: u32,
    _spec_shadowmask_flags_inert: u32,
};
@group(0) @binding(0) var<uniform> uniforms: Uniforms;

// --- Group 1: the face's world material (shared with forward and movers) ---
// Only albedo, emissive, the per-material uniform and the sampler are read.
@group(1) @binding(0) var base_texture: texture_2d<f32>;
@group(1) @binding(1) var emissive_texture: texture_2d<f32>;
struct MaterialUniform {
    shininess: f32,
    emissive_strength: f32,
    // Surface Depth fields; a fluid face never marches.
    surface_depth_peak_raise: f32,
    surface_depth_trough: f32,
    surface_depth_meters: f32,
    surface_depth_fade_distance: f32,
    surface_depth_quantize_levels: f32,
    surface_depth_march: u32,
};
@group(1) @binding(3) var<uniform> material: MaterialUniform;
@group(1) @binding(5) var aniso_sampler: sampler;

// --- Group 2: dynamic lights (shared with forward and billboard) ---
struct GpuLight {
    position_and_type: vec4<f32>,
    color_and_falloff_model: vec4<f32>,
    direction_and_range: vec4<f32>,
    cone_angles_and_pad: vec4<f32>,
};
@group(2) @binding(0) var<storage, read> lights: array<GpuLight>;
@group(2) @binding(1) var<storage, read> light_influence: array<vec4<f32>>;

// --- Group 3: SH irradiance volume (shared with forward and billboard) ---
struct ShGridInfo {
    grid_origin: vec3<f32>,
    has_sh_volume: u32,
    cell_size: vec3<f32>,
    _pad0: u32,
    grid_dimensions: vec3<u32>,
    _pad1: u32,
    atlas_dimensions: vec2<u32>,
    tile_dimension: u32,
    tile_border: u32,
    atlas_tiles_per_row: u32,
    atlas_tile_rows: u32,
    tile_interior: u32,
    physical_tile_stride: u32,
    probe_occlusion: u32,
    tiles_per_layer: u32,
    atlas_layer_count: u32,
    _pad3: u32,
};
@group(3) @binding(1) var sh_total_atlas: texture_2d_array<f32>;
@group(3) @binding(2) var sh_atlas_sampler: sampler;
@group(3) @binding(10) var<uniform> sh_grid: ShGridInfo;

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
};
// Curve samples and per-dynamic-light descriptors, as the forward pass reads
// them, so a scripted light pulses on the water as it does on the walls.
@group(3) @binding(12) var<storage, read> anim_samples: array<f32>;
@group(3) @binding(13) var<storage, read> scripted_light_descriptors: array<AnimationDescriptor>;
@group(3) @binding(14) var sh_depth_moments: texture_3d<u32>;
// Baked direct SH. Fluid takes baked direct from here per fragment, never from
// the VERTEX-only billboard scatter volume (binding 17).
@group(3) @binding(15) var sh_direct_atlas: texture_2d_array<f32>;

// --- Group 4: per-volume parameters, one uniform window of draw slots ---
// x = surface opacity, yzw reserved. The window rides a dynamic offset; the
// draw's instance index picks the slot inside it.
struct FluidVolumeParams {
    slots: array<vec4<f32>, 4096>,
};
@group(4) @binding(0) var<uniform> fluid_volumes: FluidVolumeParams;

struct VertexInput {
    @location(0) position: vec3<f32>,
    @location(1) normal: vec3<f32>,
    @location(2) uv: vec2<f32>,
    // Second vertex buffer: the face's drift in texture periods per second,
    // rewritten in place whenever drift re-resolves.
    @location(3) drift_rate: vec2<f32>,
};

struct VertexOutput {
    @builtin(position) clip_position: vec4<f32>,
    @location(0) uv: vec2<f32>,
    @location(1) world_normal: vec3<f32>,
    @location(2) world_position: vec3<f32>,
    @location(3) @interpolate(flat) slot: u32,
    // The face's drift scroll this frame, texture periods in [-0.5, 0.5).
    @location(4) @interpolate(flat) drift_scroll: vec2<f32>,
};

@vertex
fn vs_main(in: VertexInput, @builtin(instance_index) slot: u32) -> VertexOutput {
    var out: VertexOutput;
    out.clip_position = uniforms.view_proj * vec4<f32>(in.position, 1.0);
    out.uv = in.uv;
    out.world_normal = in.normal;
    out.world_position = in.position;
    out.slot = slot;
    out.drift_scroll = fluid_drift_scroll(in.drift_rate, uniforms.time);
    return out;
}

// Whole-texel fluid motion on the face texture's own grid. Mirrors
// `postretro_render_cpu::fluid_surface::motion`; every constant is pinned to it.
const FLUID_MOTION_STEP_HZ: f32 = 2.5;
const FLUID_SHIMMER_AMPLITUDE: i32 = 1;
const FLUID_RING_AMPLITUDE: i32 = 1;
const FLUID_RING_BLOCK_SHIFT: i32 = 5;
const FLUID_RING_LIFETIME_STEPS: i32 = 5;
const FLUID_RING_MARGIN: i32 = 9;
const FLUID_RING_STEP_TEXELS: f32 = 2.0;
const FLUID_FADE_START_TEXELS_PER_PIXEL: f32 = 0.75;
const FLUID_FADE_END_TEXELS_PER_PIXEL: f32 = 1.5;
const FLUID_SALT_CELL: u32 = 0x9E3779B9u;
const FLUID_SALT_SHIMMER: u32 = 0x85EBCA6Bu;
const FLUID_SALT_FADE: u32 = 0xC2B2AE35u;
const FLUID_SALT_BLOCK: u32 = 0x27D4EB2Fu;
const FLUID_SALT_DROP: u32 = 0x165667B1u;
const FLUID_HASH_MUL_A: u32 = 0x7FEB352Du;
const FLUID_HASH_MUL_B: u32 = 0x846CA68Bu;

fn fluid_hash_u32(value: u32) -> u32 {
    var x = value;
    x = x ^ (x >> 16u);
    x = x * FLUID_HASH_MUL_A;
    x = x ^ (x >> 15u);
    x = x * FLUID_HASH_MUL_B;
    x = x ^ (x >> 16u);
    return x;
}

fn fluid_hash3(a: u32, b: u32, c: u32) -> u32 {
    return fluid_hash_u32(a ^ fluid_hash_u32(b ^ fluid_hash_u32(c)));
}

fn fluid_hash_unit(h: u32) -> f32 {
    return f32(h >> 8u) * (1.0 / 16777216.0);
}

// The cell's seed: its phase, and the root of its other draws.
fn fluid_cell_seed(cell: vec2<i32>) -> u32 {
    return fluid_hash3(bitcast<u32>(cell.x), bitcast<u32>(cell.y), FLUID_SALT_CELL);
}

// Floor division for a positive divisor.
fn fluid_floor_div(a: i32, b: i32) -> i32 {
    let q = a / b;
    return select(q, q - 1, (a % b) != 0 && a < 0);
}

fn fluid_cell_step(seed: u32, time: f32) -> i32 {
    return i32(floor(time * FLUID_MOTION_STEP_HZ + fluid_hash_unit(seed)));
}

// Four-step cycle 0, +d, 0, -d along the cell's fixed direction: zero mean.
fn fluid_shimmer_offset(seed: u32, step: i32) -> vec2<i32> {
    let position = step & 3;
    if (position & 1) == 0 {
        return vec2<i32>(0);
    }
    let h = fluid_hash_u32(seed ^ FLUID_SALT_SHIMMER);
    let direction = (h >> 8u) % 9u;
    let d = vec2<i32>(i32(direction % 3u) - 1, i32(direction / 3u) - 1);
    return select(-d, d, position == 1);
}

// The ring of the cell's block drop, if it reaches the cell. Drops land at
// least FLUID_RING_MARGIN texels inside their block, so no ring leaves it.
fn fluid_ring_offset(cell: vec2<i32>, step: i32) -> vec2<i32> {
    let block_shift = u32(FLUID_RING_BLOCK_SHIFT);
    let block = cell >> vec2<u32>(block_shift);
    let seed = fluid_hash3(bitcast<u32>(block.x), bitcast<u32>(block.y), FLUID_SALT_BLOCK);
    let shifted = step + i32(seed % u32(FLUID_RING_LIFETIME_STEPS));
    let epoch = fluid_floor_div(shifted, FLUID_RING_LIFETIME_STEPS);
    let age = shifted - epoch * FLUID_RING_LIFETIME_STEPS;
    if age == 0 {
        return vec2<i32>(0);
    }
    let position = fluid_hash_u32(seed ^ fluid_hash_u32(bitcast<u32>(epoch) ^ FLUID_SALT_DROP));
    let span = u32((1 << block_shift) - 2 * FLUID_RING_MARGIN);
    let center = (block << vec2<u32>(block_shift))
        + vec2<i32>(FLUID_RING_MARGIN)
        + vec2<i32>(i32(position % span), i32((position >> 16u) % span));
    let diff = vec2<f32>(cell - center);
    let distance_sq = dot(diff, diff);
    let radius = f32(age) * FLUID_RING_STEP_TEXELS;
    // |distance - radius| < 0.5, without a square root off the ring.
    if distance_sq <= (radius - 0.5) * (radius - 0.5) || distance_sq >= (radius + 0.5) * (radius + 0.5) {
        return vec2<i32>(0);
    }
    return vec2<i32>(floor(diff / sqrt(distance_sq) + 0.5));
}

fn fluid_faded_flat(seed: u32, texels_per_pixel: f32) -> bool {
    let threshold = FLUID_FADE_START_TEXELS_PER_PIXEL
        + (FLUID_FADE_END_TEXELS_PER_PIXEL - FLUID_FADE_START_TEXELS_PER_PIXEL)
            * fluid_hash_unit(fluid_hash_u32(seed ^ FLUID_SALT_FADE));
    return texels_per_pixel > threshold;
}

fn fluid_displayed_offset(cell: vec2<i32>, time: f32, texels_per_pixel: f32) -> vec2<i32> {
    let seed = fluid_cell_seed(cell);
    if fluid_faded_flat(seed, texels_per_pixel) {
        return vec2<i32>(0);
    }
    let step = fluid_cell_step(seed, time);
    return fluid_shimmer_offset(seed, step) + fluid_ring_offset(cell, step);
}

// fract(a * b) without the product's rounding: 12-bit halves multiply exactly,
// so a day of script time still scrolls on speed.
fn fluid_high_half(x: f32) -> f32 {
    return bitcast<f32>(bitcast<u32>(x) & 0xFFFFF000u);
}

fn fluid_exact_fract_product(a: f32, b: f32) -> f32 {
    let a_hi = fluid_high_half(a);
    let b_hi = fluid_high_half(b);
    let a_lo = a - a_hi;
    let b_lo = b - b_hi;
    return fract(fract(a_hi * b_hi) + fract(a_hi * b_lo) + fract(a_lo * b_hi) + fract(a_lo * b_lo));
}

fn fluid_drift_scroll(rate: vec2<f32>, time: f32) -> vec2<f32> {
    let periods = vec2<f32>(
        fluid_exact_fract_product(time, rate.x),
        fluid_exact_fract_product(time, rate.y),
    ) + vec2<f32>(0.5);
    return periods - floor(periods) - vec2<f32>(0.5);
}

// The warped sample UV. Cells are floored from the UNWARPED texel UV, so
// shimmer and rings stay on the fixed grid while the drift scrolls beneath.
fn fluid_warp_uv(uv: vec2<f32>, drift_scroll: vec2<f32>, dims: vec2<f32>, time: f32,
                 texels_per_pixel: f32) -> vec2<f32> {
    let texel_uv = uv * dims;
    let cell = vec2<i32>(floor(texel_uv));
    let drift_texels = floor(drift_scroll * dims);
    let cell_texels = vec2<f32>(fluid_displayed_offset(cell, time, texels_per_pixel));
    return (texel_uv + drift_texels + cell_texels) / dims;
}

// Post Retro sample, the same body as kinematic_brush.wgsl (pinned by a parity
// test). The seam width and mip footprint come from the UNWARPED derivatives:
// the warp shifts the sample point, it does not change the pixel footprint.
fn sample_post_retro(tex: texture_2d<f32>, samp: sampler, uv: vec2<f32>,
                     ddx: vec2<f32>, ddy: vec2<f32>) -> vec4<f32> {
    let dims = vec2<f32>(textureDimensions(tex, 0));
    let uv_tex = uv * dims;
    let seam = floor(uv_tex + 0.5);
    let seam_width = max((abs(ddx) + abs(ddy)) * dims, vec2<f32>(1.0e-6));
    let aa = clamp((uv_tex - seam) / seam_width, vec2(-0.5), vec2(0.5));
    let uv_recon = (seam + aa) / dims;
    return textureSampleGrad(tex, samp, uv_recon, ddx, ddy);
}

fn has_direct_sh() -> bool {
    return (uniforms.has_direct & 1u) != 0u;
}

// Indirect (`.a`) and, when baked direct SH exists, direct (`.b`) through one
// shared corner blend. The lookup lifts off the surface along the viewer-side
// normal, and backface rejection is on, as for static world faces.
fn fluid_sample_sh(world_pos: vec3<f32>, n: vec3<f32>, want_direct: bool) -> ShDirPair {
    var result: ShDirPair;
    result.a = vec3<f32>(0.0);
    result.b = vec3<f32>(0.0);
    if sh_grid.has_sh_volume == 0u {
        return result;
    }
    const SH_NORMAL_OFFSET_M: f32 = 0.1;
    let offset_world = world_pos + n * SH_NORMAL_OFFSET_M * sh_grid.cell_size;
    let gdims_f = max(vec3<f32>(sh_grid.grid_dimensions) - vec3<f32>(1.0), vec3<f32>(0.0));
    let cell_coord = (offset_world - sh_grid.grid_origin) /
        max(sh_grid.cell_size, vec3<f32>(1.0e-6));
    let gf = clamp(cell_coord, vec3<f32>(0.0), gdims_f);
    let gi = vec3<u32>(floor(gf));
    let gfrac = fract(gf);
    let probe_occlusion = sh_grid.probe_occlusion != 0u;
    if want_direct {
        return sample_sh_indirect_direct_corners(
            sh_direct_atlas, gi, gfrac, offset_world, n, n, true, probe_occlusion,
        );
    }
    result.a = sample_sh_indirect_corners_depth_aware(
        gi, gfrac, offset_world, n, n, true, probe_occlusion,
    );
    return result;
}

// One runtime light's diffuse contribution. Scripted descriptors exist only
// for the dynamic prefix; promoted-static records pass `false`.
fn fluid_light(light_idx: u32, world_pos: vec3<f32>, n: vec3<f32>, scripted: bool) -> vec3<f32> {
    let influence = light_influence[light_idx];
    let inf_radius = influence.w;
    if inf_radius <= 1.0e30 {
        let d = world_pos - influence.xyz;
        if dot(d, d) > inf_radius * inf_radius {
            return vec3<f32>(0.0);
        }
    }

    let light = lights[light_idx];
    let light_type = bitcast<u32>(light.position_and_type.w);
    let falloff_model = bitcast<u32>(light.color_and_falloff_model.w);
    var effective_color = light.color_and_falloff_model.xyz;
    var effective_aim = light.direction_and_range.xyz;
    if scripted {
        let desc = scripted_light_descriptors[light_idx];
        if desc.is_active != 0u {
            let cycle_t = animation_curve_t(desc.period, desc.phase, uniforms.time);
            if desc.color_count > 0u {
                let unit_sample = max(
                    sample_color_catmull_rom(desc.color_offset, desc.color_count, cycle_t, desc.base_color),
                    vec3<f32>(0.0),
                );
                let intensity = light_eval_scripted_intensity_scalar(light.color_and_falloff_model.xyz, desc.base_color);
                let brightness = max(sample_curve_catmull_rom(desc.brightness_offset, desc.brightness_count, cycle_t), 0.0);
                effective_color = unit_sample * intensity * brightness;
            } else if desc.brightness_count > 0u {
                let brightness = max(sample_curve_catmull_rom(desc.brightness_offset, desc.brightness_count, cycle_t), 0.0);
                effective_color = light.color_and_falloff_model.xyz * brightness;
            }
            if light_type == 1u && desc.direction_count > 0u {
                effective_aim = light_eval_animated_direction(desc, cycle_t, effective_aim);
            }
        } else if light_eval_scripted_descriptor_present(desc) {
            effective_color = vec3<f32>(0.0);
        }
    }

    var L: vec3<f32>;
    var attenuation: f32;
    switch light_type {
        case 0u: {
            let to_light = light.position_and_type.xyz - world_pos;
            let dist = length(to_light);
            L = to_light / max(dist, 0.0001);
            attenuation = light_eval_falloff(dist, light.direction_and_range.w, falloff_model);
        }
        case 1u: {
            let to_light = light.position_and_type.xyz - world_pos;
            let dist = length(to_light);
            L = to_light / max(dist, 0.0001);
            attenuation = light_eval_falloff(dist, light.direction_and_range.w, falloff_model)
                * light_eval_cone_attenuation(L, effective_aim, light.cone_angles_and_pad.x, light.cone_angles_and_pad.y);
        }
        default: {
            L = -effective_aim;
            attenuation = 1.0;
        }
    }
    return effective_color * attenuation * max(dot(n, L), 0.0);
}

@fragment
fn fs_main(in: VertexOutput) -> @location(0) vec4<f32> {
    // Derivatives in uniform control flow, from the unwarped UV.
    let ddx = dpdx(in.uv);
    let ddy = dpdy(in.uv);
    let dims = vec2<f32>(textureDimensions(base_texture, 0));
    let texels_per_pixel = max(length(ddx * dims), length(ddy * dims));
    let uv = fluid_warp_uv(in.uv, in.drift_scroll, dims, uniforms.time, texels_per_pixel);
    let albedo = sample_post_retro(base_texture, aniso_sampler, uv, ddx, ddy);

    // Two-sided: shade the side the camera sees. For a planar face this is
    // exactly the back-face test, and it holds whatever the winding.
    let V = normalize(uniforms.camera_position - in.world_position);
    var n = normalize(in.world_normal);
    if dot(n, V) < 0.0 {
        n = -n;
    }

    const LIGHT_TERM_AMBIENT_FLOOR: u32 = 0x01u;
    const LIGHT_TERM_INDIRECT_STATIC: u32 = 0x02u;
    const LIGHT_TERM_INDIRECT_ANIMATED: u32 = 0x04u;
    const LIGHT_TERM_DYNAMIC_DIRECT: u32 = 0x20u;
    let light_terms = uniforms.light_term_mask;
    let use_ambient_floor = (light_terms & LIGHT_TERM_AMBIENT_FLOOR) != 0u;
    let use_indirect = (light_terms & (LIGHT_TERM_INDIRECT_STATIC | LIGHT_TERM_INDIRECT_ANIMATED)) != 0u;
    let use_dynamic = (light_terms & LIGHT_TERM_DYNAMIC_DIRECT) != 0u;

    // Baked terms. Static/animated isolation happens in the compose passes;
    // the composed direct atlas already has promoted static lights removed
    // while dynamic direct is on, so the loop below adds them back.
    let sh = fluid_sample_sh(in.world_position, n, has_direct_sh());
    var lighting = select(vec3<f32>(0.0), vec3<f32>(uniforms.ambient_floor), use_ambient_floor);
    if use_indirect {
        lighting = lighting + sh.a * uniforms.indirect_scale;
    }
    lighting = lighting + sh.b * uniforms.direct_scale;

    // Runtime lights: the dynamic prefix, then the promoted-static suffix. The
    // mover-only animated-baked tail between them is skipped; fluid takes those
    // lights from the composed direct atlas, as billboards do.
    if use_dynamic {
        for (var i: u32 = 0u; i < uniforms.light_count; i = i + 1u) {
            lighting = lighting + fluid_light(i, in.world_position, n, true);
        }
        let promoted_begin = min(
            uniforms.light_count + (uniforms.has_direct >> 1u),
            uniforms.total_light_count,
        );
        for (var i: u32 = promoted_begin; i < uniforms.total_light_count; i = i + 1u) {
            lighting = lighting + fluid_light(i, in.world_position, n, false);
        }
    }

    var rgb = albedo.rgb * lighting;
    if material.emissive_strength > 0.0 {
        rgb = rgb + sample_post_retro(emissive_texture, aniso_sampler, uv, ddx, ddy).rgb
            * material.emissive_strength;
    }
    let opacity = clamp(fluid_volumes.slots[in.slot].x, 0.0, 1.0);
    return vec4<f32>(rgb, albedo.a * opacity);
}
