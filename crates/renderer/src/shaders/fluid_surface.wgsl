// Fluid surface pass: translucent, two-sided, turbulently warped fluid faces,
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
};

struct VertexOutput {
    @builtin(position) clip_position: vec4<f32>,
    @location(0) uv: vec2<f32>,
    @location(1) world_normal: vec3<f32>,
    @location(2) world_position: vec3<f32>,
    @location(3) @interpolate(flat) slot: u32,
};

@vertex
fn vs_main(in: VertexInput, @builtin(instance_index) slot: u32) -> VertexOutput {
    var out: VertexOutput;
    out.clip_position = uniforms.view_proj * vec4<f32>(in.position, 1.0);
    out.uv = in.uv;
    out.world_normal = in.normal;
    out.world_position = in.position;
    out.slot = slot;
    return out;
}

// Quake's turbulent warp in texture-tile units: each axis shifts by a sine of
// the other, 8 texels of a 64-texel tile at 0.125 rad per texel, 1 rad/s.
const FLUID_WARP_AMPLITUDE: f32 = 0.125;
const FLUID_WARP_FREQUENCY: f32 = 8.0;
const FLUID_WARP_SPEED: f32 = 1.0;

fn fluid_warp_uv(uv: vec2<f32>, time: f32) -> vec2<f32> {
    return uv + FLUID_WARP_AMPLITUDE * sin(uv.yx * FLUID_WARP_FREQUENCY + vec2<f32>(time * FLUID_WARP_SPEED));
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
    let uv = fluid_warp_uv(in.uv, uniforms.time);
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
