// Fluid surface pass: translucent, two-sided fluid faces moving in whole texels,
// lit by baked SH indirect and baked direct SH per grid vertex and by dynamic
// lights per fragment, and mirroring their room through distance-corrected
// box-projected reflection probes (fluid_reflection.wgsl). Motion lives in
// fluid_motion.wgsl.
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
// Per animated-baked light compose descriptors, roster-indexed (the loader
// refuses section 54 unless the roster-to-descriptor table is identity): the
// reflection's live lighting reads them as SH compose does.
@group(3) @binding(11) var<storage, read> anim_descriptors: array<AnimationDescriptor>;
// Curve samples and per-dynamic-light descriptors, as the forward pass reads
// them, so a scripted light pulses on the water as it does on the walls.
@group(3) @binding(12) var<storage, read> anim_samples: array<f32>;
@group(3) @binding(13) var<storage, read> scripted_light_descriptors: array<AnimationDescriptor>;
@group(3) @binding(14) var sh_depth_moments: texture_3d<u32>;
// Baked direct SH. Fluid takes baked direct from here per fragment, never from
// the VERTEX-only billboard scatter volume (binding 17).
@group(3) @binding(15) var sh_direct_atlas: texture_2d_array<f32>;

// --- Group 4: per-volume parameters, one uniform window of draw slots ---
// Mirrors render-cpu `FluidParamsRow`, packed CPU-side. The window rides a
// dynamic offset; the draw's instance index picks the slot inside it.
struct FluidVolumeRow {
    // x = surface opacity, y = fog extinction (3 / fogDistance, 0 = no fog),
    // z = reflectivity (0 for a volume with no probe-bound face), w spare.
    surface: vec4<f32>,
    // rgb = fog albedo (fogColor scaled), w spare.
    fog_albedo: vec4<f32>,
    // x = 1 / wavePeriod, y = amplitude, z = length min, w = length max; all 0 = no wave.
    wave: vec4<f32>,
};
struct FluidVolumeParams {
    slots: array<FluidVolumeRow, 1360>,
};
@group(4) @binding(0) var<uniform> fluid_volumes: FluidVolumeParams;

// The level's reflection probes: one capture tile per array layer, sampled at
// level 0 through the SH atlas's linear-clamp sampler, and the share table,
// written once at install. Mirrors render-cpu `fluid_surface::reflection`'s
// `FluidProbeLightGpu`, `FluidProbeGpu` and table header.
struct FluidProbeLight {
    reference: vec3<f32>,
    roster_index: u32,
    // One unorm10x3 RGB share per lobe, red lowest; the tail pads.
    shares: array<vec4<u32>, 2>,
};
struct FluidProbe {
    box_min: vec3<f32>,
    _pad0: u32,
    box_max: vec3<f32>,
    _pad1: u32,
    capture_point: vec3<f32>,
    layer: u32,
    lights: array<FluidProbeLight, 4>,
};
struct FluidProbeTable {
    // x = tile dimension, y = tile border, z = probe count, w spare.
    tile: vec4<u32>,
    probes: array<FluidProbe, 256>,
};
@group(4) @binding(1) var fluid_probe_array: texture_2d_array<f32>;
@group(4) @binding(2) var<uniform> fluid_probes: FluidProbeTable;
// Beside each capture tile, in the same layer, mapping and border: the mean
// inverse L1 distance from the capture point to what each texel saw, 0 where
// it saw nothing. Sampled at level 0 through the same sampler.
@group(4) @binding(3) var fluid_probe_distance: texture_2d_array<f32>;

// --- Group 5: fluid scene inputs, rebuilt only with the scene targets ---
// Scene depth, loaded under the pass's read-only depth attachment.
@group(5) @binding(0) var scene_depth: texture_depth_2d;
struct FluidSceneInputs {
    // The frame's eye fog weight.
    fog_weight: f32,
    // The mod's reflection style: FLUID_REFLECTION_STYLE_SMOOTH, else pixel.
    reflection_style: u32,
    spare: vec2<f32>,
};
@group(5) @binding(1) var<uniform> fluid_scene: FluidSceneInputs;

struct VertexInput {
    @location(0) position: vec3<f32>,
    @location(1) normal: vec3<f32>,
    @location(2) uv: vec2<f32>,
    // Second vertex buffer: the face's drift in texture periods per second,
    // rewritten in place whenever drift re-resolves.
    @location(3) drift_rate: vec2<f32>,
    // Third vertex buffer: the face's reflection probe, written at install;
    // FLUID_REFLECTION_NO_PROBE for a face no binding names.
    @location(4) probe: u32,
};

struct VertexOutput {
    @builtin(position) clip_position: vec4<f32>,
    @location(0) uv: vec2<f32>,
    @location(1) world_normal: vec3<f32>,
    @location(2) world_position: vec3<f32>,
    @location(3) @interpolate(flat) slot: u32,
    // The face's drift scroll this frame, texture periods in [-0.5, 0.5).
    @location(4) @interpolate(flat) drift_scroll: vec2<f32>,
    @location(5) @interpolate(flat) probe: u32,
    // Ambient floor, SH indirect and baked direct SH at the vertex, masked and
    // scaled (`fluid_baked_light`), interpolated across the face's grid.
    @location(6) baked: vec3<f32>,
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
    out.probe = in.probe;
    out.baked = fluid_baked_light(in.position, in.normal);
    return out;
}

// Depth fog. Mirrors `postretro_render_cpu::fluid_surface::fog`: each fog
// function and constant here is pinned to it, and `fluid_shade`'s tail inlines its
// `fog_fragment`. The extinction constants are CPU-only, used in packing.
const FLUID_FOG_BAND: f32 = 0.08;
const FLUID_FOG_PATH_MAX: f32 = 1.0e4;
const FLUID_FOG_DENOM_EPSILON: f32 = 1.0e-8;
const FLUID_FOG_ALPHA_EPSILON: f32 = 1.0e-4;

// Metres the view ray runs past the fragment to the scene depth behind it.
// The ray is linear in clip space, so solving its NDC depth needs no inverse.
fn fluid_fog_path(view_proj: mat4x4<f32>, camera: vec3<f32>, frag: vec3<f32>, depth: f32) -> f32 {
    let c0 = view_proj * vec4<f32>(camera, 1.0);
    let c1 = view_proj * vec4<f32>(frag, 1.0);
    let denom = (c1.z - c0.z) - depth * (c1.w - c0.w);
    let s = select(FLUID_FOG_PATH_MAX, (depth * c0.w - c0.z) / denom,
        abs(denom) > FLUID_FOG_DENOM_EPSILON);
    return clamp(s - 1.0, 0.0, FLUID_FOG_PATH_MAX) * length(frag - camera);
}

fn fluid_fog_transmittance(extinction: f32, path: f32) -> f32 {
    return exp(-extinction * path);
}

// 0 at and below the face's own plane, 1 one band above it along the
// outward normal, so an eye bobbing at a plane never flips the murk.
fn fluid_fog_proximity(camera: vec3<f32>, frag: vec3<f32>, outward: vec3<f32>) -> f32 {
    return clamp(dot(camera - frag, outward) / FLUID_FOG_BAND, 0.0, 1.0);
}

fn fluid_fog_seen_from_outside(outward: vec3<f32>, view: vec3<f32>) -> bool {
    return dot(outward, view) > 0.0;
}

fn fluid_fog_layer_opacity(weight: f32, proximity: f32, transmittance: f32) -> f32 {
    return weight * proximity * (1.0 - transmittance);
}

// Surface over fog over scene, as one straight-alpha output.
fn fluid_fog_composite(rgb: vec3<f32>, alpha: f32, fog_rgb: vec3<f32>, fog_opacity: f32) -> vec4<f32> {
    let total = 1.0 - (1.0 - alpha) * (1.0 - fog_opacity);
    let out = (rgb * alpha + fog_rgb * ((1.0 - alpha) * fog_opacity))
        / max(total, FLUID_FOG_ALPHA_EPSILON);
    return vec4<f32>(out, total);
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

// The baked terms at one grid vertex: ambient floor, SH indirect and baked
// direct SH under the frame's mask and scales. The field varies at probe
// spacing and the grid cuts every face at sub-probe spacing, so evaluating it
// per vertex and interpolating buys what per fragment did, at a fraction of
// the cost. The lookup lifts along the viewer-side normal, as the fragment's
// runtime lights shade: for a planar face every vertex takes the same side.
// Static/animated isolation happens in the compose passes; the composed
// direct atlas already has promoted static lights removed while dynamic
// direct is on, so the fragment's runtime loop adds them back.
fn fluid_baked_light(world_pos: vec3<f32>, outward: vec3<f32>) -> vec3<f32> {
    var n = normalize(outward);
    if dot(n, uniforms.camera_position - world_pos) < 0.0 {
        n = -n;
    }
    const LIGHT_TERM_AMBIENT_FLOOR: u32 = 0x01u;
    const LIGHT_TERM_INDIRECT_STATIC: u32 = 0x02u;
    const LIGHT_TERM_INDIRECT_ANIMATED: u32 = 0x04u;
    let light_terms = uniforms.light_term_mask;
    let use_ambient_floor = (light_terms & LIGHT_TERM_AMBIENT_FLOOR) != 0u;
    let use_indirect = (light_terms & (LIGHT_TERM_INDIRECT_STATIC | LIGHT_TERM_INDIRECT_ANIMATED)) != 0u;
    let sh = fluid_sample_sh(world_pos, n, has_direct_sh());
    var lighting = select(vec3<f32>(0.0), vec3<f32>(uniforms.ambient_floor), use_ambient_floor);
    if use_indirect {
        lighting = lighting + sh.a * uniforms.indirect_scale;
    }
    return lighting + sh.b * uniforms.direct_scale;
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

// What the reflection tail needs from the shared shading. Only
// `fs_main_reflecting` reads past `surface`.
struct FluidShaded {
    surface: vec4<f32>,
    outward: vec3<f32>,
    view_dir: vec3<f32>,
    uv_dx_texels: vec2<f32>,
    uv_dy_texels: vec2<f32>,
    dims: vec2<f32>,
    texels_per_pixel: f32,
    reflectivity: f32,
};

// Everything up to and including the fog composite. Both fragment entry
// points share it, so they match pixel for pixel wherever the reflecting
// gate is shut.
fn fluid_shade(in: VertexOutput) -> FluidShaded {
    // Derivatives in uniform control flow, from the unwarped UV.
    let ddx = dpdx(in.uv);
    let ddy = dpdy(in.uv);
    let dims = vec2<f32>(textureDimensions(base_texture, 0));
    let texels_per_pixel = max(length(ddx * dims), length(ddy * dims));
    let row = fluid_volumes.slots[in.slot];
    let uv = fluid_warp_uv(in.uv, in.drift_scroll, dims, row.wave, uniforms.time, texels_per_pixel);
    let albedo = sample_post_retro(base_texture, aniso_sampler, uv, ddx, ddy);

    // Two-sided: shade the side the camera sees. For a planar face this is
    // exactly the back-face test, and it holds whatever the winding. Fog reads
    // the outward normal, never this viewer-flipped one.
    let V = normalize(uniforms.camera_position - in.world_position);
    let outward = normalize(in.world_normal);
    var n = outward;
    if dot(n, V) < 0.0 {
        n = -n;
    }

    const LIGHT_TERM_DYNAMIC_DIRECT: u32 = 0x20u;
    let use_dynamic = (uniforms.light_term_mask & LIGHT_TERM_DYNAMIC_DIRECT) != 0u;

    // Baked terms arrive per vertex, interpolated across the grid.
    var lighting = in.baked;

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
    let opacity = clamp(row.surface.x, 0.0, 1.0);
    let alpha = albedo.a * opacity;
    // Seen from outside a fogged fluid, the scene behind the face lies under
    // it: composite surface over fog over scene in one output. Clear fluids
    // and faces seen from inside keep exactly the surface.
    var surface = vec4<f32>(rgb, alpha);
    let extinction = row.surface.y;
    if extinction > 0.0 && fluid_fog_seen_from_outside(outward, V) {
        let depth = textureLoad(scene_depth, vec2<i32>(in.clip_position.xy), 0);
        let path = fluid_fog_path(uniforms.view_proj, uniforms.camera_position, in.world_position,
            depth);
        let transmittance = fluid_fog_transmittance(extinction, path);
        let proximity = fluid_fog_proximity(uniforms.camera_position, in.world_position, outward);
        let fog_opacity = fluid_fog_layer_opacity(fluid_scene.fog_weight, proximity, transmittance);
        surface = fluid_fog_composite(rgb, alpha, row.fog_albedo.rgb * lighting, fog_opacity);
    }
    return FluidShaded(surface, outward, V, ddx * dims, ddy * dims, dims, texels_per_pixel,
        row.surface.z);
}

// Reflection-free entry: the shading alone. It is its own entry point, not an
// override branch, so naga leaves the reflection globals out of its signature.
@fragment
fn fs_main(in: VertexOutput) -> @location(0) vec4<f32> {
    return fluid_shade(in).surface;
}

@fragment
fn fs_main_reflecting(in: VertexOutput) -> @location(0) vec4<f32> {
    // The world position's derivatives, for the pixel reflection style's
    // texel axes, in uniform control flow before the first branch.
    let ddx_world = dpdx(in.world_position);
    let ddy_world = dpdy(in.world_position);
    let shaded = fluid_shade(in);
    let surface = shaded.surface;
    let outward = shaded.outward;
    let V = shaded.view_dir;
    let dims = shaded.dims;
    let texels_per_pixel = shaded.texels_per_pixel;
    // A probe-bound face seen from above under the all-terms mask mirrors its
    // probe over that at union coverage; every other fragment is unchanged.
    // The reflection starts from the style's point and sees the eye from there.
    let reflectivity = shaded.reflectivity;
    if !fluid_reflection_gate(in.probe, outward, V, uniforms.light_term_mask, reflectivity) {
        return surface;
    }
    let axes = fluid_reflection_texel_axes(shaded.uv_dx_texels, shaded.uv_dy_texels, ddx_world, ddy_world);
    let point = fluid_reflection_point(fluid_scene.reflection_style, in.uv * dims, in.world_position,
        outward, axes, uniforms.time, texels_per_pixel, fluid_probes.probes[in.probe].box_min,
        fluid_probes.probes[in.probe].box_max, fluid_probes.probes[in.probe].capture_point);
    let view = normalize(uniforms.camera_position - point.position);
    let reflection = fluid_reflection_sample(in.probe, point.position, view, point.normal);
    let fresnel = fluid_reflection_fresnel(dot(view, outward), reflectivity) * reflection.weight;
    // From above, an eye within the fog's band of the plane scales the term
    // by its height through the band, the fog's own proximity, reaching 0 at
    // the plane: an eye bobbing there never flips the mirror.
    let band = fluid_fog_proximity(uniforms.camera_position, in.world_position, outward);
    return fluid_reflection_composite(surface, reflection.capture, fresnel * band);
}
