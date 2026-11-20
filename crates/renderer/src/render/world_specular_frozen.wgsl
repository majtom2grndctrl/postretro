// FROZEN: world static specular as forward `fs_main` evaluated it at 1c7fc9e3a,
// the build before specular-texel-sharpness. Never edit: it is the byte-identity
// oracle for `world_specular_harness.rs` (smooth style, untinted, no sharpness).
// Regenerate only by re-extracting from that commit (`git show
// 1c7fc9e3a:<path>`), prefixing every identifier `frozen_` / `FROZEN_`.
// Sources: forward.wgsl, material_shading.wgsl (`blinn_phong`) and
// light_falloff.wgsl (`light_eval_falloff`). Resolves `uniforms`,
// `spec_lights`, `SpecLight` and `SdfLightSelection` from the harness by name.

// `fs_main`'s wiring at 1c7fc9e3a: V from the interpolated position, then the
// owners and the SDF selection evaluated at that same position.
fn frozen_fs_main_static_specular(
    world_position: vec3<f32>,
    camera_position: vec3<f32>,
    N_shade: vec3<f32>,
    spec_exp: f32,
    spec_int: f32,
    owners: vec4<u32>,
    specular_shadowmask: vec4<f32>,
    owners_readable: bool,
    sdf_sel: SdfLightSelection,
    sdf_factor: vec4<f32>,
    sdf_force_lit: bool,
) -> vec3<f32> {
    let view_vector = camera_position - world_position;
    let V = normalize(view_vector);
    return frozen_world_static_specular(
        world_position,
        N_shade,
        V,
        spec_exp,
        spec_int,
        owners,
        specular_shadowmask,
        owners_readable,
        sdf_sel,
        sdf_factor,
        sdf_force_lit,
    );
}

const FROZEN_SPEC_OWNER_EMPTY: u32 = 0xFFFFu;

const FROZEN_SPEC_OWNER_CHANNELS: u32 = 4u;

const FROZEN_SPEC_LIGHT_TYPE_DIRECTIONAL: u32 = 2u;

const FROZEN_SPEC_SHADOWMASK_FLAG_FORCE_ONE: u32 = 1u;

fn frozen_world_static_specular(
    world_pos: vec3<f32>,
    N: vec3<f32>,
    V: vec3<f32>,
    spec_exp: f32,
    spec_int: f32,
    owners: vec4<u32>,
    mask: vec4<f32>,
    owners_readable: bool,
    sdf_sel: SdfLightSelection,
    sdf_factor: vec4<f32>,
    sdf_force_lit: bool,
) -> vec3<f32> {
    var sum = vec3<f32>(0.0);
    if owners_readable {
        let spec_len = arrayLength(&spec_lights);
        for (var s: u32 = 0u; s < FROZEN_SPEC_OWNER_CHANNELS; s = s + 1u) {
            let owner = owners[s];
            if owner == FROZEN_SPEC_OWNER_EMPTY {
                break;
            }
            let visibility = select(
                frozen_shadowmask_channel_value(mask, s),
                1.0,
                frozen_spec_shadowmask_forced_one(),
            );
            if owner >= spec_len || visibility <= 0.0 {
                continue;
            }
            sum = sum + frozen_world_static_light_specular(spec_lights[owner], world_pos, N, V, spec_exp, spec_int, true) * visibility;
        }
    }
    // An SDF light has no baked direct to follow; it keeps the positional
    // highlight its SDF diffuse and visibility slice share.
    for (var s: u32 = 0u; s < sdf_sel.count; s = s + 1u) {
        let visibility = select(frozen_slice_for_visibility(sdf_factor, s), 1.0, sdf_force_lit);
        sum = sum + frozen_world_static_light_specular(
            spec_lights[sdf_sel.indices[s]],
            world_pos,
            N,
            V,
            spec_exp,
            spec_int,
            false,
        ) * visibility;
    }
    return sum;
}

fn frozen_world_static_light_specular(
    sl: SpecLight,
    world_pos: vec3<f32>,
    N: vec3<f32>,
    V: vec3<f32>,
    spec_exp: f32,
    spec_int: f32,
    follow_aim: bool,
) -> vec3<f32> {
    let range = sl.position_and_range.w;
    var L: vec3<f32>;
    var aim = frozen_positional_cone_aim(sl);
    var dist = 0.0;
    let directional = u32(round(sl.cone_dir_and_type.w)) == FROZEN_SPEC_LIGHT_TYPE_DIRECTIONAL;
    if follow_aim && directional {
        let packed = sl.cone_dir_and_type.xyz;
        aim = select(vec3<f32>(0.0, -1.0, 0.0), packed, dot(packed, packed) > 1.0e-12);
        L = -normalize(aim);
    } else {
        let to_light = sl.position_and_range.xyz - world_pos;
        dist = length(to_light);
        // Range is the tight per-light cutoff.
        if range > 0.0 && dist > range {
            return vec3<f32>(0.0);
        }
        L = to_light / max(dist, 0.0001);
    }
    if dot(N, L) <= 0.0 {
        return vec3<f32>(0.0);
    }
    let atten = select(1.0, frozen_light_eval_falloff(dist, range, u32(round(sl.cone_cos.w))), range > 0.0);
    let cone = frozen_cone_attenuation_cos(L, aim, sl.cone_cos.x, sl.cone_cos.y);
    return frozen_blinn_phong(L, V, N, sl.color_and_pad.xyz, spec_exp, spec_int) * (atten * cone);
}

fn frozen_positional_cone_aim(sl: SpecLight) -> vec3<f32> {
    let directional = u32(round(sl.cone_dir_and_type.w)) == FROZEN_SPEC_LIGHT_TYPE_DIRECTIONAL;
    return select(sl.cone_dir_and_type.xyz, vec3<f32>(0.0), directional);
}

fn frozen_spec_shadowmask_forced_one() -> bool {
    return (uniforms.spec_shadowmask_flags & FROZEN_SPEC_SHADOWMASK_FLAG_FORCE_ONE) != 0u;
}

fn frozen_shadowmask_channel_value(mask: vec4<f32>, channel: u32) -> f32 {
    switch channel {
        case 0u: { return mask.r; }
        case 1u: { return mask.g; }
        case 2u: { return mask.b; }
        case 3u: { return mask.a; }
        // Callers pass 0..FROZEN_SPEC_OWNER_CHANNELS-1; the lit default is not a contract.
        default: { return 1.0; }
    }
}

fn frozen_slice_for_visibility(factor: vec4<f32>, slot: u32) -> f32 {
    switch slot {
        case 0u: { return factor.r; }
        case 1u: { return factor.g; }
        case 2u: { return factor.b; }
        default: { return factor.a; } // slot 3
    }
}

fn frozen_cone_attenuation_cos(L: vec3<f32>, aim: vec3<f32>, cos_inner: f32, cos_outer: f32) -> f32 {
    let cos_angle = dot(-L, aim);
    return smoothstep(cos_outer, cos_inner, cos_angle);
}

fn frozen_blinn_phong(L: vec3<f32>, V: vec3<f32>, N: vec3<f32>,
               color: vec3<f32>, spec_exp: f32, spec_int: f32) -> vec3<f32> {
    let H = normalize(L + V);
    let NdH = max(dot(N, H), 0.0);
    return color * pow(NdH, spec_exp) * spec_int;
}

fn frozen_light_eval_falloff(distance: f32, range: f32, model: u32) -> f32 {
    switch model {
        case 0u: {
            let r = max(range, 0.001);
            return max(1.0 - distance / r, 0.0);
        }
        case 1u: {
            let r = max(range, 0.0001);
            if distance > r {
                return 0.0;
            }
            return 1.0 / max(distance, 0.0001);
        }
        case 2u: {
            let r = max(range, 0.0001);
            if distance > r {
                return 0.0;
            }
            let d2 = max(distance * distance, 0.0001);
            return 1.0 / d2;
        }
        default: {
            return 0.0;
        }
    }
}
