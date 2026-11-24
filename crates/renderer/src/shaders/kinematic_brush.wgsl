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
    // The prefix's Blinn-Phong exponent: what a texel shades at without
    // smoothness in the specular slot's B (resource_management.md §4.7).
    shininess: f32,
    // The texture's baked glow strength (PRL id 60) on its emissive map.
    emissive_strength: f32,
    // --- Surface Depth (bytes 8..32) ---
    // The CPU buffer was already 32 bytes while this struct was 16, so the
    // feature needs no buffer resize, no new binding, and no change to the
    // 128-byte group-0 `Uniforms` ABI. Every byte from 8 on is zero for the flat
    // material.
    //
    // Relief band (bytes 8..16): the QUANTIZED peak raise, as a fraction of the
    // relief scale in [0, 1], and the trough in [-1, 0]. Both come from the
    // band of the mip the march reads; the CPU measures every uploaded mip at
    // load. The march starts at the peak and walks only the band down to the
    // trough; an empty band marches nothing.
    surface_depth_peak_raise: f32,
    surface_depth_trough: f32,
    // Relief scale in each direction around the true surface plane (authored
    // mid-gray): black sinks this far, white rises ~this far. Units per
    // `SURFACE_DEPTH_TEXEL_MODE` (albedo texels today). There is no
    // texel-density convention for world materials — brush UV scale is authored
    // freely in TrenchBroom — so the shader converts to meters per fragment. 0
    // disables the march entirely.
    surface_depth_meters: f32,
    // Distance at which the relief has faded fully flat, in meters.
    surface_depth_fade_distance: f32,
    // Terraces PER DIRECTION for the in-shader quantization of the signed height
    // `floor(s * levels + 0.5) / levels`; 0 leaves the stored 8-bit value alone.
    surface_depth_quantize_levels: f32,
    // Packed: bits 0..7 = max DDA steps, bits 8..11 = the RESIDENT base mip the
    // DDA reads at (a parameter, never a hardcoded 0 in WGSL, because
    // streaming will move it), bit 12 = the has-depth flag the bind-group
    // builder sets from the loaded specular slot's format, bits 13..15 unused,
    // bits 16..19 = how many dynamic lights this fragment may self-shadow (the
    // player's on/off switch rides here; zero at Off), bits 20..31 unused.
    surface_depth_march: u32,
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
    // Dynamic tier plus raw AnimatedBakedLights tail. Keep this scalar-packed
    // row at 32 bytes; a vec3 here would make the uniform 48 bytes.
    scripted_light_count: u32,
    indirect_scale: f32,
    // Unread. The specular style picks the beauty pipeline instead
    // (`fs_pixel` or `fs_smooth`), so no per-frame word carries it. Kept so
    // the row stays 32 bytes, the binding's minimum size.
    _pad: u32,
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
    depth_tile_row_shift: u32,
    grid_dimensions: vec3<u32>,
    depth_tile_inv_width: f32,
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
    depth_tile_inv_height: f32,
};

@group(4) @binding(1) var sh_total_atlas: texture_2d_array<f32>;
@group(4) @binding(2) var sh_atlas_sampler: sampler;
@group(4) @binding(10) var<uniform> sh_grid: ShGridInfo;
@group(4) @binding(14) var sh_depth_moments: texture_3d<u32>;
// Per-direction depth tiles (id 59); a tiled probe's moment G names its tile.
@group(4) @binding(18) var sh_depth_tiles: texture_2d<f32>;
// The promotion handoff direct SH atlas (mesh superset binding 15): promoted
// lights subtracted, added back by the shadowed runtime loop.
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
    // `@invariant` keeps clip-space Z bit-exact with `vs_depth`, which writes
    // the mover's depth in the depth pre-pass that this pass tests `Equal`
    // against. See rendering_pipeline.md §7.2.
    @invariant @builtin(position) clip_position: vec4<f32>,
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

// The one clip-position path for both mover entry points: `vs_depth` and
// `vs_main` share it so the colour draw's `Equal` test finds the depth the
// pre-pass wrote.
fn mover_world_position(position: vec3<f32>, instance_index: u32) -> vec4<f32> {
    return instances[instance_index].model * vec4<f32>(position, 1.0);
}

fn mover_clip_position(world_pos: vec4<f32>) -> vec4<f32> {
    return camera.view_proj * world_pos;
}

struct DepthOutput {
    @invariant @builtin(position) clip_position: vec4<f32>,
};

// Depth pre-pass entry: vertex-only, position from location 0 and the
// instance transform, nothing else.
@vertex
fn vs_depth(in: VertexInput, @builtin(instance_index) instance_index: u32) -> DepthOutput {
    var out: DepthOutput;
    out.clip_position = mover_clip_position(mover_world_position(in.position, instance_index));
    return out;
}

@vertex
fn vs_main(in: VertexInput, @builtin(instance_index) instance_index: u32) -> VertexOutput {
    var out: VertexOutput;
    let instance = instances[instance_index];
    let world_pos = mover_world_position(in.position, instance_index);
    out.clip_position = mover_clip_position(world_pos);
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

// `offset_normal` biases the lookup; `shading_normal` evaluates the irradiance.
// See the same split in `forward.wgsl`: Surface Depth's side-wall normal points
// ALONG the surface (perpendicular to the surface NORMAL), so biasing along it
// would slide the lookup sideways instead of lifting it off the surface.
//
// Both reads lift the lookup SH_MOVER_LOOKUP_OFFSET_CELLS (0.4) cell off the
// face, not the world's 0.1, and record the lift above the geometric plane so
// the smooth backface form measures each probe from the surface point itself
// (`sh_sample.wgsl` SH_BACKFACE_SMOOTH, owner Ruling A).
fn sample_sh_indirect(
    world_pos: vec3<f32>,
    shading_normal: vec3<f32>,
    offset_normal: vec3<f32>,
    geo_normal: vec3<f32>,
) -> vec3<f32> {
    if sh_grid.has_sh_volume == 0u {
        return vec3<f32>(0.0);
    }
    const SH_NORMAL_OFFSET_M: f32 = SH_MOVER_LOOKUP_OFFSET_CELLS;
    let offset_world = world_pos + offset_normal * SH_NORMAL_OFFSET_M * sh_grid.cell_size;
    sh_receiver_lookup_lift = dot(offset_world - world_pos, geo_normal);
    let gdims_u = sh_grid.grid_dimensions;
    let gdims_f = max(vec3<f32>(gdims_u) - vec3<f32>(1.0), vec3<f32>(0.0));
    let cell_coord = (offset_world - sh_grid.grid_origin) / max(sh_grid.cell_size, vec3<f32>(1.0e-6));
    let gf = clamp(cell_coord, vec3<f32>(0.0), gdims_f);
    let gi = vec3<u32>(floor(gf));
    let gfrac = fract(gf);
    return sample_sh_indirect_corners_depth_aware(
        gi, gfrac, offset_world, shading_normal, geo_normal, SH_BACKFACE_SMOOTH, sh_grid.probe_occlusion != 0u
    );
}

fn sample_sh_direct(
    world_pos: vec3<f32>,
    shading_normal: vec3<f32>,
    offset_normal: vec3<f32>,
    geo_normal: vec3<f32>,
) -> vec3<f32> {
    if sh_grid.has_sh_volume == 0u {
        return vec3<f32>(0.0);
    }
    const SH_NORMAL_OFFSET_M: f32 = SH_MOVER_LOOKUP_OFFSET_CELLS;
    let offset_world = world_pos + offset_normal * SH_NORMAL_OFFSET_M * sh_grid.cell_size;
    sh_receiver_lookup_lift = dot(offset_world - world_pos, geo_normal);
    let gdims_u = sh_grid.grid_dimensions;
    let gdims_f = max(vec3<f32>(gdims_u) - vec3<f32>(1.0), vec3<f32>(0.0));
    let cell_coord = (offset_world - sh_grid.grid_origin) / max(sh_grid.cell_size, vec3<f32>(1.0e-6));
    let gf = clamp(cell_coord, vec3<f32>(0.0), gdims_f);
    let gi = vec3<u32>(floor(gf));
    let gfrac = fract(gf);
    return sample_sh_direct_corners_depth_aware(
        sh_direct_atlas, gi, gfrac, offset_world, shading_normal, geo_normal, SH_BACKFACE_SMOOTH, sh_grid.probe_occlusion != 0u
    );
}

// The mover's pixel-style specular point (rendering_pipeline.md §4,
// Texel-centre highlight): one point per albedo texel, shared by the texel's
// top and every visible side face, which differ only by normal. It is the
// texel centre `sample_post_retro` reads at the marched UV, placed on the
// plane from the fragment's own UV and position, then raised along the
// geometric normal by the struck texel's quantized top height: on a top hit
// the hit's own height, on a side hit the top of the wall it struck. The lift
// rides the builder's anchor, so the view is taken from the raised point.
// A degenerate Jacobian keeps the per-pixel split the smooth pipeline uses:
// light from the Surface Depth hit, view from the fragment.
fn mover_specular_point(
    depth: SurfaceDepthResult,
    uv: vec2<f32>,
    world_position: vec3<f32>,
    geo_normal: vec3<f32>,
    view_to_eye: vec3<f32>,
    eye: vec3<f32>,
    albedo_dims: vec2<f32>,
    ddx: vec2<f32>,
    ddy: vec2<f32>,
    ddx_world: vec3<f32>,
    ddy_world: vec3<f32>,
) -> SpecularPoint {
    var top_height_m = 0.0;
    if depth.carved {
        // One height load at the texel the march struck, outside the march.
        // `depth.uv` lies inside that texel on its own lattice: a side hit's
        // UV is biased half a texel past the wall it crossed.
        // The cell is re-derived from the hit UV rather than carried out of the
        // march (registers; surface_depth.wgsl is unchanged), so a top hit
        // grazing a texel's entry edge can floor into the neighbouring cell: a
        // rare one-pixel lift error, accepted. The specular point (albedo
        // level-0 lattice) and this lookup (`depth.base_mip` lattice) agree
        // only while the resident base mip is 0
        // (`SURFACE_DEPTH_RESIDENT_BASE_MIP`, render-cpu surface_depth/uniform.rs).
        let dims = textureDimensions(spec_texture, depth.base_mip);
        let struck = surface_depth_fold(
            vec2<i32>(floor(depth.uv * vec2<f32>(dims))),
            vec2<i32>(dims),
        );
        let top = surface_depth_texel(struck, depth.base_mip, depth.quantize_levels);
        // `surface_depth_resolve`'s see-through eye column: with the eye inside
        // the band, the texel under the eye reads as the band's floor, so its
        // top hits sit at the trough, not at its top. The hit's own height
        // tells the two apart with half a height step to spare. Testing
        // the eye's height against the texel instead cost the mover ~100
        // registers on AMD Metal; measure before reshaping this.
        let trough = material.surface_depth_trough;
        let eye_column = depth.hit_top
            && depth.height_m < 0.5 * (top + trough) * depth.depth_scale_m;
        top_height_m = select(top, trough, eye_column) * depth.depth_scale_m;
    }
    return specular_texel_point(
        false,
        SpecularPoint(depth.world_position, view_to_eye),
        uv,
        depth.uv,
        world_position + geo_normal * top_height_m,
        eye,
        albedo_dims,
        ddx,
        ddy,
        ddx_world,
        ddy_world,
    );
}

// One runtime light record at `position`: its direction (xyz) and unshadowed
// attenuation (w), the distance falloff with its range cutoff, times a spot's
// cone. A directional record has no position and attenuates nothing.
fn mover_light_geometry(light: GpuLight, aim: vec3<f32>, position: vec3<f32>) -> vec4<f32> {
    let light_type = bitcast<u32>(light.position_and_type.w);
    if light_type > 1u {
        return vec4<f32>(-aim, 1.0);
    }
    let to_light = light.position_and_type.xyz - position;
    let dist = length(to_light);
    let L = to_light / max(dist, 0.0001);
    let falloff_model = bitcast<u32>(light.color_and_falloff_model.w);
    var attenuation = light_eval_falloff(dist, light.direction_and_range.w, falloff_model);
    if light_type == 1u {
        attenuation = attenuation
            * light_eval_cone_attenuation(L, aim, light.cone_angles_and_pad.x, light.cone_angles_and_pad.y);
    }
    return vec4<f32>(L, attenuation);
}

// Pixel-style specular from one selected-static record. Direction,
// falloff, range, cone and the N·L test all come from the texel's point, so a
// texel draws one value however the per-pixel influence radius, range, cone
// edge or diffuse terminator cross it. `shadow` is this fragment's own, kept
// as its own factor so specular shadows match diffuse ones.
fn mover_texel_specular(
    light: GpuLight,
    color: vec3<f32>,
    aim: vec3<f32>,
    point: SpecularPoint,
    n: vec3<f32>,
    mesh_n: vec3<f32>,
    hit_top: bool,
    shadow: f32,
    spec_exp: f32,
    spec_int: f32,
) -> vec3<f32> {
    let geometry = mover_light_geometry(light, aim, point.position);
    let L = geometry.xyz;
    // The diffuse gate's shape: a side face also needs the light above the
    // geometric plane.
    if !(dot(n, L) > 0.0 && (hit_top || dot(mesh_n, L) > 0.0)) {
        return vec3<f32>(0.0);
    }
    return blinn_phong(L, point.view, n, color, spec_exp, spec_int) * (geometry.w * shadow);
}

// The cache layer of record `i`'s promoted static depth. Animated tails
// retain descriptors through `scripted_light_count`, but both they and
// selected-static suffix records use cache metadata beginning directly after
// the dynamic prefix; a dynamic record has none.
fn mover_light_cache_layer(i: u32) -> i32 {
    var cache_layer = -1i;
    if i >= kinematic_light_params.dynamic_light_count {
        let promoted_index = i - kinematic_light_params.dynamic_light_count;
        let meta_index = kinematic_light_params.light_count
            + promoted_index * SHADOWMASK_META_VEC4S_PER_RECORD;
        if meta_index + 1u < arrayLength(&light_influence) {
            cache_layer = i32(light_influence[meta_index + 1u].w);
        }
    }
    return cache_layer;
}

// A record's colour and aim this frame.
struct MoverLightDrive {
    color: vec3<f32>,
    aim: vec3<f32>,
};

// The dynamic tier and raw animated-baked tail own descriptors that animate
// colour, brightness and a spot's aim. Selected-static records append after
// that prefix and shine as stored.
fn mover_light_drive(i: u32, light: GpuLight) -> MoverLightDrive {
    var effective_color = light.color_and_falloff_model.xyz;
    var effective_aim = light.direction_and_range.xyz;
    if i < kinematic_light_params.scripted_light_count {
        let light_type = bitcast<u32>(light.position_and_type.w);
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
        } else if light_eval_scripted_descriptor_present(scripted_desc) {
            effective_color = vec3<f32>(0.0);
        }
    }
    return MoverLightDrive(effective_color, effective_aim);
}

// This fragment's pool shadow from record `i`, 1 for a record with no shadow
// slot. The lookups stay on the true plane, which is the geometry the depth
// maps hold and their bias is tuned for.
fn mover_light_shadow(
    i: u32,
    light: GpuLight,
    cache_layer: i32,
    world_pos: vec3<f32>,
    mesh_n: vec3<f32>,
) -> f32 {
    var shadow = 1.0;
    switch bitcast<u32>(light.position_and_type.w) {
        case 0u: {
            let cube_slot = bitcast<u32>(light.cone_angles_and_pad.w);
            if cube_slot != 0xFFFFFFFFu {
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
            }
        }
        case 1u: {
            let slot_index = bitcast<u32>(light.cone_angles_and_pad.z);
            if slot_index != 0xFFFFFFFFu {
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
            }
        }
        default: {}
    }
    return shadow;
}

// Record `i`'s diffuse at this fragment. `attenuation` already carries the
// pool shadow; `n_dot_l` is zero for a side hit lit from below the
// geometric plane.
//
// Surface Depth self-shadowing is budgeted per fragment by
// `depth.shadow_light_budget` (the player's on/off switch, carried in the
// per-material uniform; zero at `Off`) and applies to the DYNAMIC prefix
// only — the animated-baked tail and the selected-static suffix are
// baked-tier records whose occlusion the bake already owns.
// `depth_shadow_marches` counts the budget spent so far this fragment.
fn mover_light_diffuse(
    i: u32,
    depth: SurfaceDepthResult,
    L: vec3<f32>,
    n_dot_l: f32,
    above_plane: bool,
    attenuation: f32,
    effective_color: vec3<f32>,
    depth_shadow_marches: ptr<function, u32>,
) -> vec3<f32> {
    var depth_visibility = 1.0;
    // Spend the self-shadow budget only on lights that actually reach this
    // fragment. A spot in range but aimed elsewhere, or a scripted light
    // whose descriptor is present but inactive, has zero attenuation or zero
    // color: marching for it burns one of the two slots on a result that is
    // then multiplied by zero, and leaves the light that genuinely casts the
    // shadow unshadowed. `above_plane` matters too:
    // `surface_depth_light_visibility` returns 1.0 at once for a light at or
    // below the geometric plane, so a below-plane light on a TOP hit whose
    // normal map tilts toward it would spend a slot and get "lit" back.
    let contributes = n_dot_l > 0.0
        && above_plane
        && attenuation > 0.0
        && dot(effective_color, vec3<f32>(1.0)) > 0.0;
    if depth.carved
        && contributes
        && i < kinematic_light_params.dynamic_light_count
        && *depth_shadow_marches < depth.shadow_light_budget {
        *depth_shadow_marches = *depth_shadow_marches + 1u;
        depth_visibility = surface_depth_light_visibility(depth, L);
    }
    return effective_color * attenuation * max(n_dot_l, 0.0) * depth_visibility;
}

// Gate a SIDE hit on the geometric plane as well as on its shading normal. A
// side-wall normal is an exact +/-U or +/-V axis lying IN the tangent plane,
// so `dot(n, L) > 0` holds across half the sub-plane hemisphere — a light
// behind opaque brush geometry would otherwise light the carved face at full
// strength, with no shadow map to stop it when the light won no slot. Top
// hits are left exactly as they were: their normal is the normal-mapped one,
// which is what keeps an uncarved fragment (and every fragment at `Off`)
// byte-identical.
fn mover_light_n_dot_l(depth: SurfaceDepthResult, n: vec3<f32>, above_plane: bool, L: vec3<f32>) -> f32 {
    let plane_lit = depth.hit_top || above_plane;
    return select(0.0, dot(n, L), plane_lit);
}

// The pixel pipeline's light-loop sums. `lit` is what the albedo multiplies:
// diffuse, plus an untinted texel's specular. `tinted_specular` holds a tinted
// texel's specular apart for `specular_tint` (resource_management.md §4.7).
struct MoverPixelDirect {
    lit: vec3<f32>,
    tinted_specular: vec3<f32>,
    // The untinted specular `lit` already folds in light by light, summed
    // apart for the highlight-free output alone: dead code in `fs_pixel`
    // (rendering_pipeline.md §7.3a, Glossy world faces, Floors).
    untinted_specular: vec3<f32>,
};

// The pixel pipeline's runtime light loop. Selected-static specular is
// evaluated at the texel's point (`mover_texel_specular`); animated direct
// tails are diffuse-only.
fn accumulate_pixel_direct(
    world_pos: vec3<f32>,
    n: vec3<f32>,
    mesh_n: vec3<f32>,
    spec_point: SpecularPoint,
    spec_exp: f32,
    spec_int: f32,
    tinted: bool,
    use_dynamic: bool,
    use_specular: bool,
    depth: SurfaceDepthResult,
) -> MoverPixelDirect {
    var total = vec3<f32>(0.0);
    // Only a tinted texel's specular lands here; an untinted texel adds its
    // specular into `total` right after each light's diffuse, as before tint.
    var tinted_total = vec3<f32>(0.0);
    var untinted_total = vec3<f32>(0.0);
    let light_count = select(0u, kinematic_light_params.light_count, use_dynamic);
    var depth_shadow_marches: u32 = 0u;
    for (var i: u32 = 0u; i < light_count; i = i + 1u) {
        let cache_layer = mover_light_cache_layer(i);
        // Pixel-style specular from a selected-static record has its own gates
        // at the texel's point: the per-pixel influence test gates diffuse
        // only, and the record is skipped only when neither reaches.
        let texel_specular = use_specular
            && i >= kinematic_light_params.scripted_light_count;
        let influence = light_influence[i];
        let inf_radius = influence.w;
        var pixel_reached = true;
        if inf_radius <= 1.0e30 {
            let inf_radius_sq = inf_radius * inf_radius;
            let d = world_pos - influence.xyz;
            pixel_reached = !(dot(d, d) > inf_radius_sq);
            let p = spec_point.position - influence.xyz;
            let point_reached = texel_specular && !(dot(p, p) > inf_radius_sq);
            if !pixel_reached && !point_reached {
                continue;
            }
        }

        let light = lights[i];
        let drive = mover_light_drive(i, light);
        // The DISPLACED hit position feeds direction and attenuation.
        let geometry = mover_light_geometry(light, drive.aim, depth.world_position);
        let L = geometry.xyz;
        // This fragment's own pool shadow, also kept apart for pixel-style
        // specular, which takes its texel's value times this.
        let shadow = mover_light_shadow(i, light, cache_layer, world_pos, mesh_n);
        let attenuation = geometry.w * shadow;
        let above_plane = dot(mesh_n, L) > 0.0;
        let n_dot_l = mover_light_n_dot_l(depth, n, above_plane, L);
        if pixel_reached {
            total = total + mover_light_diffuse(
                i, depth, L, n_dot_l, above_plane, attenuation, drive.color, &depth_shadow_marches,
            );
        }

        // An untinted term adds right after this light's diffuse; a tinted
        // one joins its own sum.
        if texel_specular {
            let term = mover_texel_specular(
                light,
                drive.color,
                drive.aim,
                spec_point,
                n,
                mesh_n,
                depth.hit_top,
                shadow,
                spec_exp,
                spec_int,
            );
            if tinted {
                tinted_total = tinted_total + term;
            } else {
                total = total + term;
                untinted_total = untinted_total + term;
            }
        }
    }
    return MoverPixelDirect(total, tinted_total, untinted_total);
}

// The smooth pipeline's light-loop sums: diffuse, and every selected-static
// record's specular, tinted or not. One specular sum keeps this loop under the
// register cliff; a second (untinted beside tinted) crosses it
// (rendering_pipeline.md §7.3, Mover register cliff).
struct MoverSmoothDirect {
    diffuse: vec3<f32>,
    specular: vec3<f32>,
};

// The smooth pipeline's runtime light loop: every input per fragment.
// Light comes from the Surface Depth hit, view from the fragment.
fn accumulate_smooth_direct(
    world_pos: vec3<f32>,
    n: vec3<f32>,
    mesh_n: vec3<f32>,
    view: vec3<f32>,
    spec_exp: f32,
    spec_int: f32,
    use_dynamic: bool,
    use_specular: bool,
    depth: SurfaceDepthResult,
) -> MoverSmoothDirect {
    var diffuse = vec3<f32>(0.0);
    var specular = vec3<f32>(0.0);
    let light_count = select(0u, kinematic_light_params.light_count, use_dynamic);
    var depth_shadow_marches: u32 = 0u;
    for (var i: u32 = 0u; i < light_count; i = i + 1u) {
        let cache_layer = mover_light_cache_layer(i);
        let influence = light_influence[i];
        let inf_radius = influence.w;
        if inf_radius <= 1.0e30 {
            let d = world_pos - influence.xyz;
            if dot(d, d) > inf_radius * inf_radius {
                continue;
            }
        }

        let light = lights[i];
        let drive = mover_light_drive(i, light);
        // The DISPLACED hit position feeds direction and attenuation.
        let geometry = mover_light_geometry(light, drive.aim, depth.world_position);
        let L = geometry.xyz;
        let attenuation = geometry.w
            * mover_light_shadow(i, light, cache_layer, world_pos, mesh_n);
        let above_plane = dot(mesh_n, L) > 0.0;
        let n_dot_l = mover_light_n_dot_l(depth, n, above_plane, L);
        diffuse = diffuse + mover_light_diffuse(
            i, depth, L, n_dot_l, above_plane, attenuation, drive.color, &depth_shadow_marches,
        );

        // Animated direct tails are diffuse-only.
        if use_specular
            && i >= kinematic_light_params.scripted_light_count
            && n_dot_l > 0.0 {
            specular = specular
                + blinn_phong(L, view, n, drive.color, spec_exp, spec_int) * attenuation;
        }
    }
    return MoverSmoothDirect(diffuse, specular);
}

// Post Retro sample. Reconstructs the texel grid in UV space through the
// shared `post_retro_texel_uv` (`material_shading.wgsl`), antialiasing only the
// seam between texels, then samples through the hardware-anisotropic material
// sampler. Static world and kinematic brush movers share one reconstruction,
// so their texture-filtering look and their pixel-style specular point agree.
//
// CRITICAL: the warped UV only shifts the sample point; the ORIGINAL
// `ddx`/`ddy` are passed to textureSampleGrad so mip selection and the
// hardware-aniso footprint track the true screen-space pixel footprint.
fn sample_post_retro(tex: texture_2d<f32>, samp: sampler, uv: vec2<f32>,
                     ddx: vec2<f32>, ddy: vec2<f32>) -> vec4<f32> {
    let dims = vec2<f32>(textureDimensions(tex, 0));
    let uv_recon = post_retro_texel_uv(uv, dims, ddx, ddy);
    return textureSampleGrad(tex, samp, uv_recon, ddx, ddy);
}

// What both beauty entries share up to their light loops: the fragment's
// derivatives, its Surface Depth hit and normal, the light it gathers ahead
// of the loop, and its specular slot.
struct MoverSurface {
    ddx: vec2<f32>,
    ddy: vec2<f32>,
    ddx_world: vec3<f32>,
    ddy_world: vec3<f32>,
    mesh_n: vec3<f32>,
    view: vec3<f32>,
    depth: SurfaceDepthResult,
    n: vec3<f32>,
    // Indirect, baked direct and the ambient floor; the loop's diffuse adds
    // to it after the loop.
    lighting: vec3<f32>,
    use_dynamic: bool,
    use_specular: bool,
    spec_texel: SpecularTexel,
};

fn mover_surface(in: VertexOutput) -> MoverSurface {
    // UV footprint derivatives are computed once in uniform control flow and
    // handed to textureSampleGrad explicitly, matching the world forward pass.
    let ddx = dpdx(in.uv);
    let ddy = dpdy(in.uv);
    // World-space footprint of the same fragment. Surface Depth marches in
    // METERS and needs world-units-per-UV-unit to reach UV space; taking it
    // here keeps every derivative in uniform control flow. The march itself
    // calls no derivative.
    let ddx_world = dpdx(in.world_position);
    let ddy_world = dpdy(in.world_position);

    let mesh_n = normalize(in.world_normal);
    let view_vector = camera.camera_position - in.world_position;
    let view_distance = length(view_vector);
    // normalize(), not a divide by `view_distance`: this runs with Surface
    // Depth on or off, and v/sqrt(dot(v,v)) is not required to round to the
    // same bits as the rsqrt normalize() lowers to. Keeping it exact is what
    // makes Off byte-identical to the pre-Surface-Depth render.
    let V = normalize(view_vector);

    // Surface Depth, through the SAME shared march the static world forward
    // pass uses (see `surface_depth.wgsl`) — movers must not drift from world
    // geometry. (The per-pass `sample_post_retro` wrappers likewise reconstruct
    // through the shared `post_retro_texel_uv`.) Inactive
    // returns the interpolated UV, position and geometric normal unchanged.
    let depth = surface_depth_resolve(
        in.uv,
        in.world_position,
        mesh_n,
        V,
        view_distance,
        ddx,
        ddy,
        ddx_world,
        ddy_world,
    );
    let shade_uv = depth.uv;

    let n_ts = sample_normal(t_normal, shade_uv, ddx, ddy);
    let n_bump = reconstruct_tbn_normal(mesh_n, in.world_tangent, in.bitangent_sign, n_ts);
    // The normal map still applies on TOP faces; a side wall uses the exact
    // axis normal. `mesh_n` stays geometric for shadow-map receiver bias.
    let n = select(depth.normal, n_bump, depth.hit_top);
    var indirect = sample_sh_indirect(in.world_position, n, n_bump, mesh_n)
        * kinematic_light_params.indirect_scale;
    var direct = vec3<f32>(0.0);
    if dynamic_direct.has_direct != 0u {
        direct = dynamic_direct.scale * sample_sh_direct(in.world_position, n, n_bump, mesh_n);
    }

    // SH indirect and baked-direct isolation happens in their respective atlas
    // compose passes. The mover only gates its in-shader ambient, runtime diffuse,
    // and promoted-static specular terms with the group-2 frame snapshot.
    let light_terms = kinematic_light_params.light_term_mask;
    let use_ambient_floor = (light_terms & 0x01u) != 0u;
    let use_dynamic = (light_terms & 0x20u) != 0u;
    let use_specular = (light_terms & 0x40u) != 0u;
    // Depth-derived ambient occlusion, INDIRECT term only, behind its own
    // LightTermMask bit. The baked direct SH term is untouched.
    indirect = indirect * surface_depth_indirect_ao(depth, light_terms);
    // One read of the slot: R intensity, B smoothness, A tint; the march read
    // its G channel.
    let spec_texel = specular_texel(
        sample_post_retro(spec_texture, aniso_sampler, shade_uv, ddx, ddy),
        max(material.shininess, 1.0),
    );

    var lighting = indirect + direct;
    if use_ambient_floor {
        lighting = vec3<f32>(kinematic_light_params.ambient_floor) + lighting;
    }
    return MoverSurface(
        ddx,
        ddy,
        ddx_world,
        ddy_world,
        mesh_n,
        V,
        depth,
        n,
        lighting,
        use_dynamic,
        use_specular,
        spec_texel,
    );
}

// The material's glow at the marched UV, times its strength.
fn mover_emissive(uv: vec2<f32>, ddx: vec2<f32>, ddy: vec2<f32>) -> vec3<f32> {
    var emissive = vec3<f32>(0.0);
    if material.emissive_strength > 0.0 {
        emissive = sample_post_retro(emissive_texture, aniso_sampler, uv, ddx, ddy).rgb;
    }
    return emissive * material.emissive_strength;
}

// The pixel pipeline's output after its loop. The tint mix runs once: an
// untinted fragment adds an exact zero after the expression it had before
// tint, so FMA contraction cannot move its bits.
fn mover_pixel_shade(
    base_color: vec3<f32>,
    lighting: vec3<f32>,
    dynamic: MoverPixelDirect,
    emissive: vec3<f32>,
    spec_texel: SpecularTexel,
) -> vec3<f32> {
    let lit = lighting + dynamic.lit;
    let tinted_specular = dynamic.tinted_specular * specular_tint(base_color, spec_texel.tint);
    return base_color * lit + emissive + tinted_specular;
}

// The smooth pipeline's output after its loop: the one specular sum takes the
// albedo, or a tinted texel's tint colour, in one multiply. It differs from
// the pixel pipeline's composition only by float reassociation.
fn mover_smooth_shade(
    base_color: vec3<f32>,
    lighting: vec3<f32>,
    dynamic: MoverSmoothDirect,
    emissive: vec3<f32>,
    spec_texel: SpecularTexel,
) -> vec3<f32> {
    let lit = lighting + dynamic.diffuse;
    let spec_color = select(
        base_color,
        specular_tint(base_color, spec_texel.tint),
        spec_texel.tinted,
    );
    return base_color * lit + emissive + dynamic.specular * spec_color;
}

// A mover fragment's colour, and the highlight it adds: albedo times the
// untinted specular plus the tinted specular.
struct MoverShaded {
    colour: vec4<f32>,
    highlight: vec3<f32>,
};

// The floor layer's tiers: the same colour, then the colour with its
// highlight left out (rendering_pipeline.md §7.3a, Glossy world faces,
// Floors).
struct MoverHighlightFreeOutput {
    @location(0) colour: vec4<f32>,
    @location(1) highlight_free: vec4<f32>,
};

// The colour less its highlight, taken in f32 and floored at 0 against the
// colour's own rounding.
fn mover_highlight_free(shaded: MoverShaded) -> vec4<f32> {
    return vec4<f32>(max(shaded.colour.rgb - shaded.highlight, vec3<f32>(0.0)), shaded.colour.a);
}

// The pixel specular style's shading (rendering_pipeline.md §4,
// Texel-centre highlight): one highlight point per albedo texel.
fn mover_pixel_fragment(in: VertexOutput) -> MoverShaded {
    let s = mover_surface(in);
    let spec_point = mover_specular_point(
        s.depth,
        in.uv,
        in.world_position,
        s.mesh_n,
        s.view,
        camera.camera_position,
        vec2<f32>(textureDimensions(base_texture, 0)),
        s.ddx,
        s.ddy,
        s.ddx_world,
        s.ddy_world,
    );
    let dynamic = accumulate_pixel_direct(
        in.world_position,
        s.n,
        s.mesh_n,
        spec_point,
        s.spec_texel.exponent,
        s.spec_texel.intensity,
        s.spec_texel.tinted,
        s.use_dynamic,
        s.use_specular,
        s.depth,
    );
    // Albedo is read after the light loop, not before it: held across the
    // loop beside the tint accumulator it took the shader from 128 to 224
    // VGPRs on AMD Metal (tools/metal_register_screen). Screen before moving it.
    let base_color = sample_post_retro(base_texture, aniso_sampler, s.depth.uv, s.ddx, s.ddy);
    let emissive = mover_emissive(s.depth.uv, s.ddx, s.ddy);
    let highlight = base_color.rgb * dynamic.untinted_specular
        + dynamic.tinted_specular * specular_tint(base_color.rgb, s.spec_texel.tint);
    return MoverShaded(
        vec4<f32>(
            mover_pixel_shade(base_color.rgb, s.lighting, dynamic, emissive, s.spec_texel),
            base_color.a,
        ),
        highlight,
    );
}

// The smooth specular style's shading: every highlight input per fragment,
// so sharp texels crawl with the camera.
fn mover_smooth_fragment(in: VertexOutput) -> MoverShaded {
    let s = mover_surface(in);
    let dynamic = accumulate_smooth_direct(
        in.world_position,
        s.n,
        s.mesh_n,
        s.view,
        s.spec_texel.exponent,
        s.spec_texel.intensity,
        s.use_dynamic,
        s.use_specular,
        s.depth,
    );
    // Albedo is read after the light loop (rendering_pipeline.md §7.3, Mover
    // register cliff). Screen before moving it.
    let base_color = sample_post_retro(base_texture, aniso_sampler, s.depth.uv, s.ddx, s.ddy);
    let emissive = mover_emissive(s.depth.uv, s.ddx, s.ddy);
    let spec_color = select(
        base_color.rgb,
        specular_tint(base_color.rgb, s.spec_texel.tint),
        s.spec_texel.tinted,
    );
    return MoverShaded(
        vec4<f32>(
            mover_smooth_shade(base_color.rgb, s.lighting, dynamic, emissive, s.spec_texel),
            base_color.a,
        ),
        dynamic.specular * spec_color,
    );
}

// The pixel style's beauty entry.
@fragment
fn fs_pixel(in: VertexOutput) -> @location(0) vec4<f32> {
    return mover_pixel_fragment(in).colour;
}

// The smooth style's beauty entry.
@fragment
fn fs_smooth(in: VertexOutput) -> @location(0) vec4<f32> {
    return mover_smooth_fragment(in).colour;
}

// The pixel style's entry at a tier that runs the floor layer.
@fragment
fn fs_pixel_highlight_free(in: VertexOutput) -> MoverHighlightFreeOutput {
    let shaded = mover_pixel_fragment(in);
    return MoverHighlightFreeOutput(shaded.colour, mover_highlight_free(shaded));
}

// The smooth style's entry at a tier that runs the floor layer.
@fragment
fn fs_smooth_highlight_free(in: VertexOutput) -> MoverHighlightFreeOutput {
    let shaded = mover_smooth_fragment(in);
    return MoverHighlightFreeOutput(shaded.colour, mover_highlight_free(shaded));
}
