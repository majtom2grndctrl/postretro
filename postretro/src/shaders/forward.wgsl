// Main forward pass — direct lighting via a flat per-fragment light loop
// plus a scalar ambient floor.
// See: context/lib/rendering_pipeline.md §4
//      context/plans/in-progress/lighting-foundation/3-direct-lighting.md

struct Uniforms {
    view_proj: mat4x4<f32>,
    camera_position: vec3<f32>,
    ambient_floor: f32,
    light_count: u32,
    // pad out to 16-byte alignment for the UBO std140 rules.
    _pad_a: u32,
    _pad_b: u32,
    _pad_c: u32,
};

// Five vec4<f32> slots — see postretro/src/lighting.rs for field semantics.
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
//
// Distance attenuation for Point and Spot lights. Discriminant is bitcast
// out of color_and_falloff_model.w:
//   0 = Linear         — 1 - d/range, clamped to [0,1]
//   1 = InverseDistance — 1/d, zeroed past range
//   2 = InverseSquared  — 1/d², zeroed past range
//
// See 3-direct-lighting.md §Falloff models for why the InverseDistance /
// InverseSquared cases are not upper-clamped: color × intensity controls
// absolute brightness on the CPU side, so close-up values > 1 are the
// intended response curve.
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
//
// `aim` is the direction the light points — from the light position
// outward toward the illuminated area. `L` is from fragment toward the
// light, so `-L` points from the light toward the fragment, and the dot
// product measures how closely the fragment falls inside the aimed cone.
// `smoothstep` produces full brightness inside the inner cone and a
// clean falloff to zero at the outer cone edge.
fn cone_attenuation(L: vec3<f32>, aim: vec3<f32>, inner_angle: f32, outer_angle: f32) -> f32 {
    let cos_angle = dot(-L, aim);
    let cos_inner = cos(inner_angle);
    let cos_outer = cos(outer_angle);
    return smoothstep(cos_outer, cos_inner, cos_angle);
}

@fragment
fn fs_main(in: VertexOutput) -> @location(0) vec4<f32> {
    let base_color = textureSample(base_texture, base_sampler, in.uv);
    let N = normalize(in.world_normal);

    var total_light = vec3<f32>(uniforms.ambient_floor);

    for (var i: u32 = 0u; i < uniforms.light_count; i = i + 1u) {
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
        total_light = total_light + light.color_and_falloff_model.xyz * attenuation * NdotL;
    }

    let rgb = base_color.rgb * total_light;
    return vec4<f32>(rgb, base_color.a);
}
