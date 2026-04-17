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
    // Elapsed seconds since renderer start. Consumed by SH animated-layer
    // evaluation (sub-plan 7); wrapping is handled per-light via fract().
    time: f32,
    // pad out to 16-byte alignment for the UBO std140 rules.
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
// Bindings 5+ reserved for sub-plan 9 (SDF atlas, sampler, top-level index,
// meta uniform). Do not claim them for anything else.

// Group 3 — SH irradiance volume. 9 3D textures (one per SH L2 band) carry
// RGB coefficients in their .rgb channels (.a unused). When `grid.has_sh_volume`
// is 0 the bindings point at dummy 1×1×1 textures and the shader skips SH
// sampling. See sub-plan 6 and postretro/src/render/sh_volume.rs.
struct ShGridInfo {
    grid_origin: vec3<f32>,
    has_sh_volume: u32,
    cell_size: vec3<f32>,
    _pad0: u32,
    grid_dimensions: vec3<u32>,
    animated_light_count: u32,
};

// Per-light animation descriptor — matches ANIMATION_DESCRIPTOR_SIZE (48 B)
// in postretro/src/render/sh_volume.rs. Field order diverges from the spec
// prose to hit exactly 48 bytes: with the spec's original order, color_count
// ends at byte 44 and _padding: vec2<f32> (AlignOf=8) would be pushed to 48,
// making the struct 56 B and stride 64. Instead we pack four scalars after
// base_color so color_count ends at 36; _padding then lands at 40 (4-byte
// implicit gap at 36..40) and occupies 40..48 for a 48-byte stride.
struct AnimationDescriptor {
    period: f32,
    phase: f32,
    brightness_offset: u32,
    brightness_count: u32,
    base_color: vec3<f32>,
    color_offset: u32,
    color_count: u32,
    _padding: vec2<f32>,
};

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

// Animation buffers (sub-plan 7). Always bound; the shader guards on
// `sh_grid.animated_light_count == 0` so dummy bindings are never read.
@group(3) @binding(11) var<storage, read> anim_descriptors: array<AnimationDescriptor>;
@group(3) @binding(12) var<storage, read> anim_samples: array<f32>;
@group(3) @binding(13) var<storage, read> anim_sh_data: array<f32>;

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

// Shadow kinds other than CSM (e.g. `shadow_kind == 2` — reserved for
// sub-plan 9's SDF sphere-trace — and any unknown value) fall through to
// unshadowed (factor 1.0) in the fragment main until sub-plan 9 lands.

// --- SH irradiance volume sampling ---
//
// Constants are the standard real spherical harmonic L0..L2 basis
// normalization factors. Signs on bands 1, 3, 5, 7 match the signed basis
// used by the baker (postretro-level-compiler/src/sh_bake.rs::sh_basis_l2) —
// projection and reconstruction MUST use the same signed basis, or the
// L1-y / L1-x / L2-yz / L2-xz terms invert.
//
// The Ramamoorthi-Hanrahan cosine-lobe convolution (A_0=π, A_1=2π/3, A_2=π/4)
// is folded into the baked coefficients at bake time — see
// sh_bake.rs::apply_cosine_lobe_rgb. Runtime reconstruction applies only the
// basis. If the indirect looks wrong, the bug is in the baker or the upload
// path, not these constants. See sub-plan 6 §"Notes for implementation".
fn sh_irradiance(
    b0: vec3<f32>, b1: vec3<f32>, b2: vec3<f32>, b3: vec3<f32>,
    b4: vec3<f32>, b5: vec3<f32>, b6: vec3<f32>, b7: vec3<f32>, b8: vec3<f32>,
    normal: vec3<f32>,
) -> vec3<f32> {
    let nx = normal.x;
    let ny = normal.y;
    let nz = normal.z;
    var r: vec3<f32> = b0 * 0.282095;                 // L0
    r = r + b1 * (-0.488603 * ny);                    // L1 y  (signed basis)
    r = r + b2 * ( 0.488603 * nz);                    // L1 z
    r = r + b3 * (-0.488603 * nx);                    // L1 x  (signed basis)
    r = r + b4 * ( 1.092548 * nx * ny);               // L2 xy
    r = r + b5 * (-1.092548 * ny * nz);               // L2 yz (signed basis)
    r = r + b6 * ( 0.315392 * (3.0 * nz * nz - 1.0)); // L2 z^2
    r = r + b7 * (-1.092548 * nx * nz);               // L2 xz (signed basis)
    r = r + b8 * ( 0.546274 * (nx * nx - ny * ny));   // L2 x^2 - y^2
    return r;
}

// Evaluate an animated light's current brightness by linearly interpolating
// its brightness samples over its period, with wrap-around. Returns 1.0
// when the light has no brightness animation.
fn eval_animated_brightness(desc: AnimationDescriptor, cycle_t: f32) -> f32 {
    if desc.brightness_count == 0u {
        return 1.0;
    }
    let sample_pos = cycle_t * f32(desc.brightness_count);
    let idx0 = u32(floor(sample_pos)) % desc.brightness_count;
    let idx1 = (idx0 + 1u) % desc.brightness_count;
    let frac_t = fract(sample_pos);
    return mix(
        anim_samples[desc.brightness_offset + idx0],
        anim_samples[desc.brightness_offset + idx1],
        frac_t,
    );
}

// Evaluate an animated light's current color. Falls back to `base_color`
// when `color_count == 0`.
fn eval_animated_color(desc: AnimationDescriptor, cycle_t: f32) -> vec3<f32> {
    if desc.color_count == 0u {
        return desc.base_color;
    }
    let sample_pos = cycle_t * f32(desc.color_count);
    let idx0 = u32(floor(sample_pos)) % desc.color_count;
    let idx1 = (idx0 + 1u) % desc.color_count;
    let frac_t = fract(sample_pos);
    let off0 = desc.color_offset + idx0 * 3u;
    let off1 = desc.color_offset + idx1 * 3u;
    let c0 = vec3<f32>(
        anim_samples[off0],
        anim_samples[off0 + 1u],
        anim_samples[off0 + 2u],
    );
    let c1 = vec3<f32>(
        anim_samples[off1],
        anim_samples[off1 + 1u],
        anim_samples[off1 + 2u],
    );
    return mix(c0, c1, frac_t);
}

// Manual trilinear interpolation of one band of one animated light's
// monochrome SH from the packed per-light storage buffer. Works in
// integer grid space so clamp-to-edge behavior matches the base SH path.
fn sample_anim_mono_band(
    light_idx: u32,
    band: u32,
    gi: vec3<u32>,
    gfrac: vec3<f32>,
) -> f32 {
    let gx = sh_grid.grid_dimensions.x;
    let gy = sh_grid.grid_dimensions.y;
    let gz = sh_grid.grid_dimensions.z;
    let probe_count = gx * gy * gz;
    let base_offset = light_idx * probe_count;

    // Fetch 8 corner coefficients.
    var c: array<f32, 8>;
    for (var dz: u32 = 0u; dz < 2u; dz = dz + 1u) {
        for (var dy: u32 = 0u; dy < 2u; dy = dy + 1u) {
            for (var dx: u32 = 0u; dx < 2u; dx = dx + 1u) {
                let cx = min(gi.x + dx, gx - 1u);
                let cy = min(gi.y + dy, gy - 1u);
                let cz = min(gi.z + dz, gz - 1u);
                let probe_idx = (cz * gy + cy) * gx + cx;
                let slot = dz * 4u + dy * 2u + dx;
                c[slot] = anim_sh_data[(base_offset + probe_idx) * 9u + band];
            }
        }
    }

    let c00 = mix(c[0], c[1], gfrac.x);
    let c01 = mix(c[2], c[3], gfrac.x);
    let c10 = mix(c[4], c[5], gfrac.x);
    let c11 = mix(c[6], c[7], gfrac.x);
    let c0 = mix(c00, c01, gfrac.y);
    let c1 = mix(c10, c11, gfrac.y);
    return mix(c0, c1, gfrac.z);
}

fn sample_sh_indirect(world_pos: vec3<f32>, normal: vec3<f32>) -> vec3<f32> {
    if sh_grid.has_sh_volume == 0u {
        return vec3<f32>(0.0);
    }
    // Grid UV: place probes at texel centers. With probe i sitting at
    // `origin + i * cell_size` in world space, the texel-center UV for
    // probe i is `(i + 0.5) / N`. Fragments outside the probe grid clamp
    // to the nearest edge probe via the sampler's `clamp-to-edge` mode.
    let dims = max(vec3<f32>(sh_grid.grid_dimensions), vec3<f32>(1.0));
    let cell_coord = (world_pos - sh_grid.grid_origin) / max(sh_grid.cell_size, vec3<f32>(1.0e-6));
    let grid_uv = (cell_coord + vec3<f32>(0.5)) / dims;

    var b0 = textureSample(sh_band0, sh_sampler, grid_uv).rgb;
    var b1 = textureSample(sh_band1, sh_sampler, grid_uv).rgb;
    var b2 = textureSample(sh_band2, sh_sampler, grid_uv).rgb;
    var b3 = textureSample(sh_band3, sh_sampler, grid_uv).rgb;
    var b4 = textureSample(sh_band4, sh_sampler, grid_uv).rgb;
    var b5 = textureSample(sh_band5, sh_sampler, grid_uv).rgb;
    var b6 = textureSample(sh_band6, sh_sampler, grid_uv).rgb;
    var b7 = textureSample(sh_band7, sh_sampler, grid_uv).rgb;
    var b8 = textureSample(sh_band8, sh_sampler, grid_uv).rgb;

    // Accumulate animated-light contributions into the SH coefficient
    // vector before reconstruction. SH is linear in its coefficients, so
    // one reconstruction pass suffices regardless of light count.
    let anim_count = sh_grid.animated_light_count;
    if anim_count != 0u {
        // Integer grid coordinates for manual trilinear interpolation.
        // clamp_cell keeps us inside [0, dim-1] so the `min(gi+dx, dim-1)`
        // guards in sample_anim_mono_band cover the upper bound only.
        let gdims_u = sh_grid.grid_dimensions;
        let gdims_f = max(vec3<f32>(gdims_u) - vec3<f32>(1.0), vec3<f32>(0.0));
        let gf = clamp(cell_coord, vec3<f32>(0.0), gdims_f);
        let gi = vec3<u32>(floor(gf));
        let gfrac = fract(gf);

        for (var i: u32 = 0u; i < anim_count; i = i + 1u) {
            let desc = anim_descriptors[i];
            let cycle_t = fract(uniforms.time / max(desc.period, 1.0e-6) + desc.phase);
            let brightness = eval_animated_brightness(desc, cycle_t);
            let color = eval_animated_color(desc, cycle_t);
            let modulate = color * brightness;

            b0 = b0 + sample_anim_mono_band(i, 0u, gi, gfrac) * modulate;
            b1 = b1 + sample_anim_mono_band(i, 1u, gi, gfrac) * modulate;
            b2 = b2 + sample_anim_mono_band(i, 2u, gi, gfrac) * modulate;
            b3 = b3 + sample_anim_mono_band(i, 3u, gi, gfrac) * modulate;
            b4 = b4 + sample_anim_mono_band(i, 4u, gi, gfrac) * modulate;
            b5 = b5 + sample_anim_mono_band(i, 5u, gi, gfrac) * modulate;
            b6 = b6 + sample_anim_mono_band(i, 6u, gi, gfrac) * modulate;
            b7 = b7 + sample_anim_mono_band(i, 7u, gi, gfrac) * modulate;
            b8 = b8 + sample_anim_mono_band(i, 8u, gi, gfrac) * modulate;
        }
    }

    // Clamp negative irradiance to zero per channel — SH L2 can ring for
    // sharp transitions (Gibbs). Standard practice; see sub-plan 6.
    return max(
        sh_irradiance(b0, b1, b2, b3, b4, b5, b6, b7, b8, normal),
        vec3<f32>(0.0),
    );
}

@fragment
fn fs_main(in: VertexOutput) -> @location(0) vec4<f32> {
    let base_color = textureSample(base_texture, base_sampler, in.uv);
    let N = normalize(in.world_normal);

    // Indirect term: baked SH irradiance. Zero when no SH volume is loaded.
    let indirect = sample_sh_indirect(in.world_position, N);

    // Total light = ambient floor (minimum) + indirect + direct sum.
    var total_light = vec3<f32>(uniforms.ambient_floor) + indirect;

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
        // `shadow_kind == 1` → CSM (directional). `shadow_kind == 2` is
        // reserved for sub-plan 9's SDF sphere-trace (point + spot); until
        // that lands, it falls through to unshadowed. Any other value is
        // also unshadowed.
        let shadow_kind = bitcast<u32>(light.shadow_info.z);
        var shadow_factor = 1.0;
        if shadow_kind == 1u {
            shadow_factor = sample_csm_shadow(in.world_position, bitcast<u32>(light.shadow_info.y));
        }

        total_light = total_light + light.color_and_falloff_model.xyz * attenuation * NdotL * shadow_factor;
    }

    let rgb = base_color.rgb * total_light;
    return vec4<f32>(rgb, base_color.a);
}
