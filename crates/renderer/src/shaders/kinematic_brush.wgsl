// Renderer-owned kinematic brush mover pass.
// See: context/lib/rendering_pipeline.md §7.3

struct CameraUniforms {
    view_proj: mat4x4<f32>,
    camera_position: vec3<f32>,
    // Layout-only: remains to mirror the shared camera uniform buffer.
    ambient_floor: f32,
};
@group(0) @binding(0) var<uniform> camera: CameraUniforms;

@group(1) @binding(0) var base_texture: texture_2d<f32>;
@group(1) @binding(1) var emissive_texture: texture_2d<f32>;
@group(1) @binding(2) var spec_texture: texture_2d<f32>;

struct MaterialUniform {
    shininess: f32,
    emissive_strength: f32,
    _pad: vec2<f32>,
};
@group(1) @binding(3) var<uniform> material: MaterialUniform;
@group(1) @binding(4) var t_normal: texture_2d<f32>;
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
    light_term_mask: u32,
    ambient_floor: f32,
    dynamic_light_count: u32,
    // Keep the second 16-byte row scalar-packed. A vec3 here would align to
    // byte 32 and make the uniform 48 bytes, diverging from the Rust upload.
    _pad0: u32,
    _pad1: u32,
    _pad2: u32,
};
@group(2) @binding(4) var<uniform> kinematic_light_params: KinematicLightParams;

@group(2) @binding(5) var spot_shadow_depth: texture_depth_2d_array;
@group(2) @binding(6) var spot_shadow_compare: sampler_comparison;
struct LightSpaceMatrices {
    m: array<mat4x4<f32>, 96>,
};
@group(2) @binding(7) var<uniform> light_space_matrices: LightSpaceMatrices;
@group(2) @binding(8) var point_shadow_cube: texture_depth_cube_array; // CUBE_SHADOW_BINDING
@group(2) @binding(9) var promoted_spot_depth_cache: texture_depth_2d_array;
@group(2) @binding(10) var promoted_cube_depth_cache: texture_depth_cube_array; // CUBE_SHADOW_BINDING
const SHADOWMASK_META_VEC4S_PER_RECORD: u32 = 2u;

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
    _pad0: u32,
    has_direct: u32,
    _pad1: u32,
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
    @location(2) world_tangent: vec3<f32>,
    @location(3) bitangent_sign: f32,
    @location(4) world_position: vec3<f32>,
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

    // `tangent_packed.y` stores the handedness in its high bit; the lower 15
    // bits carry the octahedral v component. Match the static-world decode
    // before transforming the tangent into the mover's world-space basis.
    let sign_bit = in.tangent_packed.y & 0x8000u;
    let v_15bit = in.tangent_packed.y & 0x7FFFu;
    let v_16bit = v_15bit * 65535u / 32767u;
    out.world_tangent = normalize(model3 * oct_decode(vec2<u32>(in.tangent_packed.x, v_16bit)));
    out.bitangent_sign = select(-1.0, 1.0, sign_bit != 0u);
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

fn accumulate_dynamic_direct(
    world_pos: vec3<f32>,
    n: vec3<f32>,
    mesh_n: vec3<f32>,
    V: vec3<f32>,
    spec_exp: f32,
    spec_int: f32,
    use_dynamic: bool,
    use_specular: bool,
) -> vec3<f32> {
    var total = vec3<f32>(0.0);
    let light_count = select(0u, kinematic_light_params.light_count, use_dynamic);
    for (var i: u32 = 0u; i < light_count; i = i + 1u) {
        var cache_layer = -1i;
        if i >= kinematic_light_params.dynamic_light_count {
            let promoted_index = i - kinematic_light_params.dynamic_light_count;
            let meta_index = kinematic_light_params.light_count
                + promoted_index * SHADOWMASK_META_VEC4S_PER_RECORD;
            if meta_index + 1u < arrayLength(&light_influence) {
                cache_layer = i32(light_influence[meta_index + 1u].w);
            }
        }
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

        var effective_color = light.color_and_falloff_model.xyz;
        var effective_aim = light.direction_and_range.xyz;
        // Only dynamic-tier lights own descriptor slots. Promoted static records
        // append after that prefix and must not read stale descriptor tail bytes.
        if i < kinematic_light_params.dynamic_light_count {
            let scripted_desc = scripted_light_descriptors[i];
            if scripted_desc.is_active != 0u {
                let cycle_t = animation_curve_t(
                    scripted_desc.period,
                    scripted_desc.phase,
                    kinematic_light_params.time,
                );
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
                    var shadow: f32;
                    if i >= kinematic_light_params.dynamic_light_count {
                        shadow = sample_point_shadow_with_static(
                            cube_slot,
                            cache_layer,
                            light.position_and_type.xyz,
                            world_pos,
                            mesh_n,
                            MOVER_RECEIVER_BIAS_SCALE,
                            light.direction_and_range.w,
                        );
                    } else {
                        shadow = sample_point_shadow(
                            cube_slot, light.position_and_type.xyz, world_pos, mesh_n,
                            MOVER_RECEIVER_BIAS_SCALE, light.direction_and_range.w,
                        );
                    }
                    attenuation = attenuation * shadow;
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
                    var shadow: f32;
                    if i >= kinematic_light_params.dynamic_light_count {
                        shadow = sample_spot_shadow_with_static(
                            slot_index,
                            cache_layer,
                            light.position_and_type.xyz,
                            world_pos,
                            mesh_n,
                            MOVER_RECEIVER_BIAS_SCALE,
                            light_space_matrices.m[slot_index],
                        );
                    } else {
                        shadow = sample_spot_shadow(
                            slot_index, light.position_and_type.xyz, world_pos, mesh_n,
                            MOVER_RECEIVER_BIAS_SCALE, light_space_matrices.m[slot_index],
                        );
                    }
                    attenuation = attenuation * shadow;
                }
            }
            default: {
                L = -effective_aim;
                attenuation = 1.0;
            }
        }
        let n_dot_l = dot(n, L);
        total = total + effective_color * attenuation * max(n_dot_l, 0.0);

        // The runtime buffer lists dynamic-tier lights first, then promoted
        // static records. Dynamic lights remain diffuse-only; a promoted
        // record's effective color already carries its de-promotion weight.
        if use_specular && i >= kinematic_light_params.dynamic_light_count && n_dot_l > 0.0 {
            total = total + blinn_phong(L, V, n, effective_color, spec_exp, spec_int) * attenuation;
        }
    }
    return total;
}

// Post Retro sample. Reconstructs the texel grid in UV space, antialiases only
// the seam between texels, then samples through the hardware-anisotropic
// material sampler. Matches forward.wgsl so static world and kinematic brush
// movers share the same texture-filtering look.
fn sample_post_retro(tex: texture_2d<f32>, samp: sampler, uv: vec2<f32>,
                     ddx: vec2<f32>, ddy: vec2<f32>) -> vec4<f32> {
    let dims = vec2<f32>(textureDimensions(tex, 0));
    let uv_tex = uv * dims;
    let seam = floor(uv_tex + 0.5);
    // Floor the seam-width divisor: a constant-UV fragment (edge-on face,
    // degenerate UV chart, vanishing derivatives) gives fwidth == 0, and
    // clamp() does not reliably sanitize the resulting NaN/Inf in WGSL.
    let seam_width = max(fwidth(uv_tex), vec2<f32>(1.0e-6));
    let aa = clamp((uv_tex - seam) / seam_width, vec2(-0.5), vec2(0.5));
    let uv_recon = (seam + aa) / dims;
    return textureSampleGrad(tex, samp, uv_recon, ddx, ddy);
}

@fragment
fn fs_main(in: VertexOutput) -> @location(0) vec4<f32> {
    // UV footprint derivatives are computed once in uniform control flow and
    // handed to textureSampleGrad explicitly, matching the world forward pass.
    let ddx = dpdx(in.uv);
    let ddy = dpdy(in.uv);

    let base_color = sample_post_retro(base_texture, aniso_sampler, in.uv, ddx, ddy);
    let mesh_n = normalize(in.world_normal);
    let n_ts = sample_normal(t_normal, in.uv, ddx, ddy);
    let n = reconstruct_tbn_normal(mesh_n, in.world_tangent, in.bitangent_sign, n_ts);
    let indirect = sample_sh_indirect(in.world_position, n, mesh_n);
    var direct = vec3<f32>(0.0);
    if dynamic_direct.has_direct != 0u {
        direct = dynamic_direct.scale * sample_sh_direct(in.world_position, n, mesh_n);
    }

    // SH indirect and baked-direct isolation happens in their respective atlas
    // compose passes. The mover only gates its in-shader ambient, runtime diffuse,
    // and promoted-static specular terms with the group-2 frame snapshot.
    let light_terms = kinematic_light_params.light_term_mask;
    let use_ambient_floor = (light_terms & 0x01u) != 0u;
    let use_dynamic = (light_terms & 0x20u) != 0u;
    let use_specular = (light_terms & 0x40u) != 0u;
    let V = normalize(camera.camera_position - in.world_position);
    let spec_exp = max(material.shininess, 1.0);
    let spec_int = sample_post_retro(spec_texture, aniso_sampler, in.uv, ddx, ddy).r;
    let dynamic = accumulate_dynamic_direct(
        in.world_position,
        n,
        mesh_n,
        V,
        spec_exp,
        spec_int,
        use_dynamic,
        use_specular,
    );

    var lighting = indirect + direct;
    if use_ambient_floor {
        lighting = vec3<f32>(kinematic_light_params.ambient_floor) + lighting;
    }
    lighting = lighting + dynamic;
    var emissive = vec3<f32>(0.0);
    if material.emissive_strength > 0.0 {
        emissive = sample_post_retro(emissive_texture, aniso_sampler, in.uv, ddx, ddy).rgb;
    }
    return vec4<f32>(base_color.rgb * lighting + emissive * material.emissive_strength, base_color.a);
}
