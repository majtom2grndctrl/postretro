// Kinematic brush mover pass: local brush geometry instanced by mover transform.
// Mirrors the dynamic-object lighting model used by skinned meshes, without
// skinning or animation palette reads.

struct CameraUniforms {
    view_proj: mat4x4<f32>,
};
@group(0) @binding(0) var<uniform> camera: CameraUniforms;

@group(1) @binding(0) var base_texture: texture_2d<f32>;
@group(1) @binding(5) var aniso_sampler: sampler;

struct GpuLight {
    position_and_type: vec4<f32>,
    color_and_falloff_model: vec4<f32>,
    direction_and_range: vec4<f32>,
    cone_angles_and_pad: vec4<f32>,
};
@group(2) @binding(0) var<storage, read> lights: array<GpuLight>;
@group(2) @binding(1) var<storage, read> light_influence: array<vec4<f32>>;

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
@group(2) @binding(2) var<storage, read> scripted_light_descriptors: array<AnimationDescriptor>;
@group(2) @binding(3) var<storage, read> anim_samples: array<f32>;

struct KinematicLightParams {
    light_count: u32,
    time: f32,
    lighting_isolation: u32,
    ambient_floor: f32,
};
@group(2) @binding(4) var<uniform> kinematic_light_params: KinematicLightParams;

@group(2) @binding(5) var spot_shadow_depth: texture_depth_2d_array;
@group(2) @binding(6) var spot_shadow_compare: sampler_comparison;
struct LightSpaceMatrices {
    m: array<mat4x4<f32>, 96>,
};
@group(2) @binding(7) var<uniform> light_space_matrices: LightSpaceMatrices;
@group(2) @binding(8) var point_shadow_cube: texture_depth_cube_array; // CUBE_SHADOW_BINDING

struct Instance {
    model: mat4x4<f32>,
};
@group(3) @binding(0) var<storage, read> instances: array<Instance>;

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
    _pad2: u32,
    probe_occlusion: u32,
    tiles_per_layer: u32,
    atlas_layer_count: u32,
    _pad3: u32,
};

@group(4) @binding(1) var sh_total_atlas: texture_2d_array<f32>;
@group(4) @binding(2) var sh_atlas_sampler: sampler;
@group(4) @binding(10) var<uniform> sh_grid: ShGridInfo;
@group(4) @binding(14) var sh_depth_moments: texture_3d<f32>;
@group(4) @binding(15) var sh_direct_atlas: texture_2d_array<f32>;

struct DynamicDirectParams {
    scale: f32,
    isolation: u32,
    has_direct: u32,
    _pad: u32,
};
@group(4) @binding(16) var<uniform> dynamic_direct: DynamicDirectParams;

struct VertexInput {
    @location(0) position: vec3<f32>,
    @location(1) base_uv: vec2<f32>,
    @location(2) normal_oct: vec2<u32>,
    @location(3) tangent_packed: vec2<u32>,
};

struct VertexOutput {
    @builtin(position) clip_position: vec4<f32>,
    @location(0) uv: vec2<f32>,
    @location(1) world_normal: vec3<f32>,
    @location(2) world_position: vec3<f32>,
};

fn oct_decode(enc: vec2<u32>) -> vec3<f32> {
    let ox = f32(enc.x) / 65535.0 * 2.0 - 1.0;
    let oy = f32(enc.y) / 65535.0 * 2.0 - 1.0;
    let z = 1.0 - abs(ox) - abs(oy);
    var x: f32;
    var y: f32;
    if z < 0.0 {
        x = (1.0 - abs(oy)) * select(-1.0, 1.0, ox >= 0.0);
        y = (1.0 - abs(ox)) * select(-1.0, 1.0, oy >= 0.0);
    } else {
        x = ox;
        y = oy;
    }
    return normalize(vec3<f32>(x, y, z));
}

@vertex
fn vs_main(in: VertexInput, @builtin(instance_index) instance_index: u32) -> VertexOutput {
    var out: VertexOutput;
    let instance = instances[instance_index];
    let world_pos = instance.model * vec4<f32>(in.position, 1.0);
    out.clip_position = camera.view_proj * world_pos;
    out.world_position = world_pos.xyz;
    out.uv = in.base_uv;

    let n_local = oct_decode(in.normal_oct);
    let model3 = mat3x3<f32>(instance.model[0].xyz, instance.model[1].xyz, instance.model[2].xyz);
    out.world_normal = normalize(model3 * n_local);
    return out;
}

fn sample_sh_indirect(world_pos: vec3<f32>, shading_normal: vec3<f32>, geo_normal: vec3<f32>) -> vec3<f32> {
    if sh_grid.has_sh_volume == 0u {
        return vec3<f32>(0.0);
    }
    const SH_NORMAL_OFFSET_M: f32 = 0.1;
    let offset_world = world_pos + shading_normal * SH_NORMAL_OFFSET_M * sh_grid.cell_size;
    let gdims_u = sh_grid.grid_dimensions;
    let gdims_f = max(vec3<f32>(gdims_u) - vec3<f32>(1.0), vec3<f32>(0.0));
    let cell_coord = (offset_world - sh_grid.grid_origin) / max(sh_grid.cell_size, vec3<f32>(1.0e-6));
    let gf = clamp(cell_coord, vec3<f32>(0.0), gdims_f);
    let gi = vec3<u32>(floor(gf));
    let gfrac = fract(gf);
    return sample_sh_indirect_corners_depth_aware(
        gi, gfrac, offset_world, shading_normal, geo_normal, false, sh_grid.probe_occlusion != 0u
    );
}

fn sample_sh_direct(world_pos: vec3<f32>, shading_normal: vec3<f32>, geo_normal: vec3<f32>) -> vec3<f32> {
    if sh_grid.has_sh_volume == 0u {
        return vec3<f32>(0.0);
    }
    const SH_NORMAL_OFFSET_M: f32 = 0.1;
    let offset_world = world_pos + shading_normal * SH_NORMAL_OFFSET_M * sh_grid.cell_size;
    let gdims_u = sh_grid.grid_dimensions;
    let gdims_f = max(vec3<f32>(gdims_u) - vec3<f32>(1.0), vec3<f32>(0.0));
    let cell_coord = (offset_world - sh_grid.grid_origin) / max(sh_grid.cell_size, vec3<f32>(1.0e-6));
    let gf = clamp(cell_coord, vec3<f32>(0.0), gdims_f);
    let gi = vec3<u32>(floor(gf));
    let gfrac = fract(gf);
    return sample_sh_direct_corners_depth_aware(
        sh_direct_atlas, gi, gfrac, offset_world, shading_normal, geo_normal, false, sh_grid.probe_occlusion != 0u
    );
}

fn accumulate_dynamic_direct(world_pos: vec3<f32>, n: vec3<f32>, use_dynamic: bool) -> vec3<f32> {
    var total = vec3<f32>(0.0);
    let light_count = select(0u, kinematic_light_params.light_count, use_dynamic);
    for (var i: u32 = 0u; i < light_count; i = i + 1u) {
        let influence = light_influence[i];
        let inf_radius = influence.w;
        if inf_radius <= 1.0e30 {
            let d = world_pos - influence.xyz;
            if dot(d, d) > inf_radius * inf_radius {
                continue;
            }
        }

        let light = lights[i];
        let light_type = bitcast<u32>(light.position_and_type.w);
        let falloff_model = bitcast<u32>(light.color_and_falloff_model.w);

        let scripted_desc = scripted_light_descriptors[i];
        var effective_color = light.color_and_falloff_model.xyz;
        var effective_aim = light.direction_and_range.xyz;
        if scripted_desc.is_active != 0u {
            let cycle_t = fract(kinematic_light_params.time / max(scripted_desc.period, 0.0001) + scripted_desc.phase);
            if scripted_desc.color_count > 0u {
                let unit_sample = max(
                    sample_color_catmull_rom(scripted_desc.color_offset, scripted_desc.color_count, cycle_t, scripted_desc.base_color),
                    vec3<f32>(0.0),
                );
                let intensity = light_eval_scripted_intensity_scalar(light.color_and_falloff_model.xyz, scripted_desc.base_color);
                let brightness = max(sample_curve_catmull_rom(scripted_desc.brightness_offset, scripted_desc.brightness_count, cycle_t), 0.0);
                effective_color = unit_sample * intensity * brightness;
            } else if scripted_desc.brightness_count > 0u {
                let brightness = max(sample_curve_catmull_rom(scripted_desc.brightness_offset, scripted_desc.brightness_count, cycle_t), 0.0);
                effective_color = light.color_and_falloff_model.xyz * brightness;
            }
            if light_type == 1u && scripted_desc.direction_count > 0u {
                effective_aim = light_eval_animated_direction(scripted_desc, cycle_t, effective_aim);
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
                let cube_slot = bitcast<u32>(light.cone_angles_and_pad.w);
                if cube_slot != 0xFFFFFFFFu {
                    attenuation = attenuation * sample_point_shadow(cube_slot, light.position_and_type.xyz, world_pos, light.direction_and_range.w);
                }
            }
            case 1u: {
                let to_light = light.position_and_type.xyz - world_pos;
                let dist = length(to_light);
                L = to_light / max(dist, 0.0001);
                let dist_falloff = light_eval_falloff(dist, light.direction_and_range.w, falloff_model);
                let cone = light_eval_cone_attenuation(L, effective_aim, light.cone_angles_and_pad.x, light.cone_angles_and_pad.y);
                attenuation = dist_falloff * cone;
                let slot_index = bitcast<u32>(light.cone_angles_and_pad.z);
                if slot_index != 0xFFFFFFFFu {
                    attenuation = attenuation * sample_spot_shadow(slot_index, world_pos, light_space_matrices.m[slot_index]);
                }
            }
            default: {
                L = -effective_aim;
                attenuation = 1.0;
            }
        }
        total = total + effective_color * attenuation * max(dot(n, L), 0.0);
    }
    return total;
}

@fragment
fn fs_main(in: VertexOutput) -> @location(0) vec4<f32> {
    let base_color = textureSample(base_texture, aniso_sampler, in.uv);
    let n = normalize(in.world_normal);
    let indirect = sample_sh_indirect(in.world_position, n, n);
    var direct = vec3<f32>(0.0);
    if dynamic_direct.has_direct != 0u {
        direct = dynamic_direct.scale * sample_sh_direct(in.world_position, n, n);
    }

    let iso = kinematic_light_params.lighting_isolation;
    let use_dynamic = (iso == 0u) || (iso == 1u) || (iso == 2u) || (iso == 8u);
    let dynamic = accumulate_dynamic_direct(in.world_position, n, use_dynamic);

    var lighting = indirect + direct;
    if dynamic_direct.isolation == 1u {
        lighting = direct;
    } else if dynamic_direct.isolation == 2u {
        lighting = indirect;
    }
    lighting = vec3<f32>(kinematic_light_params.ambient_floor) + lighting + dynamic;
    return vec4<f32>(base_color.rgb * lighting, base_color.a);
}
