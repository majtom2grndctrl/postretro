// Main forward pass — direct lighting via a flat per-fragment light loop
// plus a scalar ambient floor, with shadow map sampling.
// See: context/lib/rendering_pipeline.md §4
//      context/plans/in-progress/lighting-foundation/3-direct-lighting.md
//      context/plans/in-progress/lighting-foundation/5-shadow-maps.md

struct Uniforms {
    view_proj: mat4x4<f32>,
    camera_position: vec3<f32>,
    ambient_floor: f32,
    light_count: u32,
    // pad out to 16-byte alignment for the UBO std140 rules.
    _pad_a: u32,
    _pad_b: u32,
    _pad_c: u32,
    // CSM cascade split distances (view-space Z far for cascades 0/1/2, w reserved).
    csm_splits: vec4<f32>,
    // View matrix for computing fragment view-space depth for cascade selection.
    view_matrix: mat4x4<f32>,
};

// Five vec4<f32> slots — see postretro/src/lighting/mod.rs for field semantics.
struct GpuLight {
    position_and_type: vec4<f32>,
    color_and_falloff_model: vec4<f32>,
    direction_and_range: vec4<f32>,
    cone_angles_and_pad: vec4<f32>,
    shadow_info: vec4<f32>,
};

@group(0) @binding(0) var<uniform> uniforms: Uniforms;

@group(1) @binding(0) var base_texture: texture_2d<f32>;
@group(1) @binding(1) var base_sampler: sampler;

@group(2) @binding(0) var<storage, read> lights: array<GpuLight>;
// Per-light influence volume: xyz = sphere center, w = radius.
@group(2) @binding(1) var<storage, read> light_influence: array<vec4<f32>>;
@group(2) @binding(2) var shadow_sampler: sampler_comparison;
@group(2) @binding(3) var csm_depth_array: texture_depth_2d_array;
@group(2) @binding(4) var<storage, read> csm_view_proj: array<mat4x4<f32>>;
// Point shadows stored as a 2D array: 6 layers per light (one per cube face).
// slot * 6 + face_index selects the layer.
@group(2) @binding(5) var point_shadow_array: texture_depth_2d_array;
@group(2) @binding(6) var spot_shadow_array: texture_depth_2d_array;
@group(2) @binding(7) var<storage, read> spot_view_proj: array<mat4x4<f32>>;

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

// Decode octahedral-encoded u16x2 to unit direction vector.
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
fn vs_main(in: VertexInput) -> VertexOutput {
    var out: VertexOutput;
    out.clip_position = uniforms.view_proj * vec4<f32>(in.position, 1.0);
    out.uv = in.base_uv;
    out.world_position = in.position;

    // Decode octahedral normal.
    out.world_normal = oct_decode(in.normal_oct);

    // Decode packed tangent: strip sign bit from v-component, remap 15-bit to 16-bit.
    let sign_bit = in.tangent_packed.y & 0x8000u;
    let v_15bit = in.tangent_packed.y & 0x7FFFu;
    let v_16bit = v_15bit * 65535u / 32767u;
    out.world_tangent = oct_decode(vec2<u32>(in.tangent_packed.x, v_16bit));
    out.bitangent_sign = select(-1.0, 1.0, sign_bit != 0u);

    return out;
}

// --- Falloff models ---
fn falloff(distance: f32, range: f32, model: u32) -> f32 {
    switch model {
        case 0u: {
            return max(1.0 - distance / max(range, 0.001), 0.0);
        }
        case 1u: {
            if distance > range {
                return 0.0;
            }
            return 1.0 / max(distance, 0.001);
        }
        case 2u: {
            if distance > range {
                return 0.0;
            }
            let d2 = max(distance * distance, 0.001);
            return 1.0 / d2;
        }
        default: {
            return 0.0;
        }
    }
}

// --- Spot cone attenuation ---
fn cone_attenuation(L: vec3<f32>, aim: vec3<f32>, inner_angle: f32, outer_angle: f32) -> f32 {
    let cos_angle = dot(-L, aim);
    let cos_inner = cos(inner_angle);
    let cos_outer = cos(outer_angle);
    return smoothstep(cos_outer, cos_inner, cos_angle);
}

// --- Shadow sampling ---

// Sample CSM shadow map for a directional light.
fn sample_csm_shadow(frag_world_pos: vec3<f32>, shadow_map_index: u32) -> f32 {
    // Compute view-space depth for cascade selection.
    let view_pos = uniforms.view_matrix * vec4<f32>(frag_world_pos, 1.0);
    let view_depth = -view_pos.z; // RH: view-space Z is negative in front of camera.

    // Select the tightest cascade that contains this fragment.
    var cascade: u32 = 0u;
    if view_depth > uniforms.csm_splits.x {
        cascade = 1u;
    }
    if view_depth > uniforms.csm_splits.y {
        cascade = 2u;
    }

    let cascade_index = shadow_map_index * 3u + cascade;
    let vp = csm_view_proj[cascade_index];
    let light_space_pos = vp * vec4<f32>(frag_world_pos, 1.0);
    let ndc = light_space_pos.xyz / light_space_pos.w;
    let shadow_uv = ndc.xy * 0.5 + 0.5;
    // Flip Y: NDC Y increases upward, texture V increases downward.
    let uv = vec2<f32>(shadow_uv.x, 1.0 - shadow_uv.y);
    let depth = ndc.z;

    // Reject fragments outside the shadow map UV range.
    if uv.x < 0.0 || uv.x > 1.0 || uv.y < 0.0 || uv.y > 1.0 {
        return 1.0; // Lit (no shadow data).
    }

    return textureSampleCompareLevel(csm_depth_array, shadow_sampler, uv, cascade_index, depth);
}

// Determine which cube face to sample from a direction vector.
// Returns (face_index, uv) for sampling from a 2D array texture.
fn cube_face_from_dir(dir: vec3<f32>) -> u32 {
    let abs_dir = abs(dir);
    if abs_dir.x >= abs_dir.y && abs_dir.x >= abs_dir.z {
        if dir.x > 0.0 { return 0u; } else { return 1u; }
    } else if abs_dir.y >= abs_dir.x && abs_dir.y >= abs_dir.z {
        if dir.y > 0.0 { return 2u; } else { return 3u; }
    } else {
        if dir.z > 0.0 { return 4u; } else { return 5u; }
    }
}

// Point light shadow maps use per-face view-projection matrices stored in
// csm_view_proj buffer starting at offset MAX_CSM_LIGHTS * 3 cascades.
// Actually, point light shadows store linear depth and are sampled via the
// comparison sampler against normalized distance. We project the fragment
// through the face's VP matrix and compare.
fn sample_point_shadow(light_pos: vec3<f32>, frag_world_pos: vec3<f32>, light_range: f32, shadow_map_index: u32) -> f32 {
    let to_frag = frag_world_pos - light_pos;
    let dist = length(to_frag);
    // Linear depth: normalized distance to light, with a small bias to
    // reduce shadow acne. Hardware depth bias has no effect on point shadow
    // maps because the fragment shader writes frag_depth explicitly.
    let depth = dist / max(light_range, 0.001) - 0.002;
    let dir = normalize(to_frag);
    let face = cube_face_from_dir(dir);

    // Face major axis and uv derivation for 2D array lookup.
    let abs_dir = abs(dir);
    var uv: vec2<f32>;
    if face == 0u {
        // +X
        uv = vec2<f32>(-dir.z / abs_dir.x, -dir.y / abs_dir.x) * 0.5 + 0.5;
    } else if face == 1u {
        // -X
        uv = vec2<f32>(dir.z / abs_dir.x, -dir.y / abs_dir.x) * 0.5 + 0.5;
    } else if face == 2u {
        // +Y
        uv = vec2<f32>(dir.x / abs_dir.y, dir.z / abs_dir.y) * 0.5 + 0.5;
    } else if face == 3u {
        // -Y
        uv = vec2<f32>(dir.x / abs_dir.y, -dir.z / abs_dir.y) * 0.5 + 0.5;
    } else if face == 4u {
        // +Z
        uv = vec2<f32>(dir.x / abs_dir.z, -dir.y / abs_dir.z) * 0.5 + 0.5;
    } else {
        // -Z
        uv = vec2<f32>(-dir.x / abs_dir.z, -dir.y / abs_dir.z) * 0.5 + 0.5;
    }

    let layer = shadow_map_index * 6u + face;
    return textureSampleCompareLevel(point_shadow_array, shadow_sampler, uv, layer, depth);
}

// Sample 2D shadow map for a spot light.
fn sample_spot_shadow(frag_world_pos: vec3<f32>, shadow_map_index: u32) -> f32 {
    let vp = spot_view_proj[shadow_map_index];
    let light_space_pos = vp * vec4<f32>(frag_world_pos, 1.0);
    let ndc = light_space_pos.xyz / light_space_pos.w;
    let shadow_uv = ndc.xy * 0.5 + 0.5;
    let uv = vec2<f32>(shadow_uv.x, 1.0 - shadow_uv.y);
    let depth = ndc.z;

    if uv.x < 0.0 || uv.x > 1.0 || uv.y < 0.0 || uv.y > 1.0 {
        return 1.0;
    }

    return textureSampleCompareLevel(spot_shadow_array, shadow_sampler, uv, shadow_map_index, depth);
}

@fragment
fn fs_main(in: VertexOutput) -> @location(0) vec4<f32> {
    let base_color = textureSample(base_texture, base_sampler, in.uv);
    let N = normalize(in.world_normal);

    var total_light = vec3<f32>(uniforms.ambient_floor);

    for (var i: u32 = 0u; i < uniforms.light_count; i = i + 1u) {
        // Influence-volume early-out: skip lights whose sphere bound does
        // not contain this fragment. Pure optimization — no pixel change.
        let influence = light_influence[i];
        let inf_radius = influence.w;
        if inf_radius <= 1.0e30 {
            let d = in.world_position - influence.xyz;
            if dot(d, d) > inf_radius * inf_radius {
                continue;
            }
        }

        let light = lights[i];
        let light_type = bitcast<u32>(light.position_and_type.w);
        let falloff_model = bitcast<u32>(light.color_and_falloff_model.w);

        var L: vec3<f32>;
        var attenuation: f32;

        switch light_type {
            case 0u: {
                // Point light
                let to_light = light.position_and_type.xyz - in.world_position;
                let dist = length(to_light);
                L = to_light / max(dist, 0.0001);
                attenuation = falloff(dist, light.direction_and_range.w, falloff_model);
            }
            case 1u: {
                // Spot light
                let to_light = light.position_and_type.xyz - in.world_position;
                let dist = length(to_light);
                L = to_light / max(dist, 0.0001);
                let dist_falloff = falloff(dist, light.direction_and_range.w, falloff_model);
                let cone = cone_attenuation(
                    L,
                    light.direction_and_range.xyz,
                    light.cone_angles_and_pad.x,
                    light.cone_angles_and_pad.y,
                );
                attenuation = dist_falloff * cone;
            }
            default: {
                // Directional light (case 2u and any unknown discriminant)
                L = -light.direction_and_range.xyz;
                attenuation = 1.0;
            }
        }

        let NdotL = max(dot(N, L), 0.0);

        // Shadow modulation: sample shadow map if this light casts shadows.
        let shadow_kind = bitcast<u32>(light.shadow_info.z);
        var shadow_factor = 1.0;
        if shadow_kind == 1u {
            // CSM (directional)
            shadow_factor = sample_csm_shadow(in.world_position, bitcast<u32>(light.shadow_info.y));
        } else if shadow_kind == 2u {
            // Cube (point) — stored in 2D array with 6 layers per slot.
            shadow_factor = sample_point_shadow(
                light.position_and_type.xyz,
                in.world_position,
                light.direction_and_range.w,
                bitcast<u32>(light.shadow_info.y),
            );
        } else if shadow_kind == 3u {
            // Spot 2D
            shadow_factor = sample_spot_shadow(in.world_position, bitcast<u32>(light.shadow_info.y));
        }

        total_light = total_light + light.color_and_falloff_model.xyz * attenuation * NdotL * shadow_factor;
    }

    let rgb = base_color.rgb * total_light;
    return vec4<f32>(rgb, base_color.a);
}
