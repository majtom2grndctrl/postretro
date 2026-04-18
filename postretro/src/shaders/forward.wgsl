// Main forward pass — direct lighting via a flat per-fragment light loop
// plus a scalar ambient floor, with baked SH irradiance indirect.
// See: context/lib/rendering_pipeline.md §4

struct Uniforms {
    view_proj: mat4x4<f32>,
    camera_position: vec3<f32>,
    ambient_floor: f32,
    light_count: u32,
    // Elapsed seconds since renderer start. Consumed by SH animated-layer
    // evaluation; wrapping is handled per-light via fract().
    time: f32,
    // Lighting-term isolation for leak/bleed debugging. Cycled by the
    // Alt+Shift+4 diagnostic chord. Values:
    //   0 = Normal       (direct + indirect + ambient floor — production shading)
    //   1 = DirectOnly   (SH indirect forced to 0)
    //   2 = IndirectOnly (direct-light loop skipped)
    //   3 = AmbientOnly  (both terms skipped; only ambient floor contributes)
    lighting_isolation: u32,
    _pad: u32,
};

// Four vec4<f32> slots — see postretro/src/lighting/mod.rs for field semantics.
struct GpuLight {
    position_and_type: vec4<f32>,
    color_and_falloff_model: vec4<f32>,
    direction_and_range: vec4<f32>,
    cone_angles_and_pad: vec4<f32>,
};

@group(0) @binding(0) var<uniform> uniforms: Uniforms;

@group(1) @binding(0) var base_texture: texture_2d<f32>;
@group(1) @binding(1) var base_sampler: sampler;

@group(2) @binding(0) var<storage, read> lights: array<GpuLight>;
// Per-light influence volume: xyz = sphere center, w = radius.
@group(2) @binding(1) var<storage, read> light_influence: array<vec4<f32>>;

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
    // `@invariant` keeps clip-space Z bit-exact with depth_prepass.wgsl so
    // the `depth_compare: Equal` test doesn't miss fragments due to FMA
    // reassociation drift on some GPUs. See rendering_pipeline.md §7.2.
    @invariant @builtin(position) clip_position: vec4<f32>,
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

// Accumulate all 9 SH bands of one animated light in a single 8-corner
// traversal, using precomputed final (tri * visibility / weight_sum) weights.
//
// Replaces the earlier per-band `sample_anim_mono_band` helper. The animated
// buffer layout stores the 9 bands of one probe contiguously
// (`anim_sh_data[(base + probe_idx) * 9 + band]`); iterating bands inside the
// corner loop reads those 9 floats in order, giving a contiguous cache line
// per corner. The previous outer-band-inner-corner layout strided by 9 floats
// between reads, wasting cache.
//
// `final_weights[slot]` is the per-corner trilinear weight; weights sum
// to 1.0 across the 8-corner cube.
//
// Out-parameter: WGSL can't cleanly return `array<vec3<f32>, 9>` from a
// function in all targets, and the animated buffer is mono (f32 per band) —
// the vec3 math happens only when the caller multiplies by the color
// modulate. We emit scalar accumulators via a ptr<function, _> and let the
// caller do the modulate multiply and vec3 promotion.
fn accumulate_anim_all_bands(
    light_idx: u32,
    gi: vec3<u32>,
    final_weights: array<f32, 8>,
    accum: ptr<function, array<f32, 9>>,
) {
    let gx = sh_grid.grid_dimensions.x;
    let gy = sh_grid.grid_dimensions.y;
    let gz = sh_grid.grid_dimensions.z;
    let probe_count = gx * gy * gz;
    let base_offset = light_idx * probe_count;

    for (var dz: u32 = 0u; dz < 2u; dz = dz + 1u) {
        for (var dy: u32 = 0u; dy < 2u; dy = dy + 1u) {
            for (var dx: u32 = 0u; dx < 2u; dx = dx + 1u) {
                let cx = min(gi.x + dx, gx - 1u);
                let cy = min(gi.y + dy, gy - 1u);
                let cz = min(gi.z + dz, gz - 1u);
                let probe_idx = (cz * gy + cy) * gx + cx;
                let slot = dz * 4u + dy * 2u + dx;
                let w = final_weights[slot];
                let probe_base = (base_offset + probe_idx) * 9u;
                // Contiguous 9-float read per corner.
                for (var band: u32 = 0u; band < 9u; band = band + 1u) {
                    (*accum)[band] = (*accum)[band] + w * anim_sh_data[probe_base + band];
                }
            }
        }
    }
}

// Hardware-trilinear fetch of all 9 SH bands, plus the animated layers with
// plain trilinear weights. One sample per band in lieu of eight manual
// fetches.
fn sample_sh_indirect_fast(
    offset_world: vec3<f32>,
    normal: vec3<f32>,
    gi: vec3<u32>,
    gfrac: vec3<f32>,
) -> vec3<f32> {
    // Hardware trilinear on the base SH textures. UVW computed from the
    // offset world position in [0, 1] texture space.
    let gdims_f = max(vec3<f32>(sh_grid.grid_dimensions), vec3<f32>(1.0));
    let cell_center_uvw = (vec3<f32>(gi) + vec3<f32>(0.5) + gfrac) / gdims_f;
    // `cell_center_uvw` lands between the 8 texel centers, so hardware
    // trilinear reproduces the per-corner weighting exactly — one sample
    // per band in lieu of eight manual fetches.
    var b0 = textureSampleLevel(sh_band0, sh_sampler, cell_center_uvw, 0.0).rgb;
    var b1 = textureSampleLevel(sh_band1, sh_sampler, cell_center_uvw, 0.0).rgb;
    var b2 = textureSampleLevel(sh_band2, sh_sampler, cell_center_uvw, 0.0).rgb;
    var b3 = textureSampleLevel(sh_band3, sh_sampler, cell_center_uvw, 0.0).rgb;
    var b4 = textureSampleLevel(sh_band4, sh_sampler, cell_center_uvw, 0.0).rgb;
    var b5 = textureSampleLevel(sh_band5, sh_sampler, cell_center_uvw, 0.0).rgb;
    var b6 = textureSampleLevel(sh_band6, sh_sampler, cell_center_uvw, 0.0).rgb;
    var b7 = textureSampleLevel(sh_band7, sh_sampler, cell_center_uvw, 0.0).rgb;
    var b8 = textureSampleLevel(sh_band8, sh_sampler, cell_center_uvw, 0.0).rgb;

    // Animated layers: same plain-trilinear weights (all corners fully
    // visible, weight_sum == 1 by construction, so final_weights = tri_w).
    let anim_count = sh_grid.animated_light_count;
    if anim_count != 0u {
        var tri_w: array<f32, 8>;
        for (var dz: u32 = 0u; dz < 2u; dz = dz + 1u) {
            for (var dy: u32 = 0u; dy < 2u; dy = dy + 1u) {
                for (var dx: u32 = 0u; dx < 2u; dx = dx + 1u) {
                    let tri = vec3<f32>(
                        select(1.0 - gfrac.x, gfrac.x, dx == 1u),
                        select(1.0 - gfrac.y, gfrac.y, dy == 1u),
                        select(1.0 - gfrac.z, gfrac.z, dz == 1u),
                    );
                    tri_w[dz * 4u + dy * 2u + dx] = tri.x * tri.y * tri.z;
                }
            }
        }
        for (var i: u32 = 0u; i < anim_count; i = i + 1u) {
            let desc = anim_descriptors[i];
            let cycle_t = fract(uniforms.time / max(desc.period, 1.0e-6) + desc.phase);
            let brightness = eval_animated_brightness(desc, cycle_t);
            let color = eval_animated_color(desc, cycle_t);
            let modulate = color * brightness;

            var accum: array<f32, 9>;
            for (var band: u32 = 0u; band < 9u; band = band + 1u) {
                accum[band] = 0.0;
            }
            accumulate_anim_all_bands(i, gi, tri_w, &accum);
            b0 = b0 + accum[0] * modulate;
            b1 = b1 + accum[1] * modulate;
            b2 = b2 + accum[2] * modulate;
            b3 = b3 + accum[3] * modulate;
            b4 = b4 + accum[4] * modulate;
            b5 = b5 + accum[5] * modulate;
            b6 = b6 + accum[6] * modulate;
            b7 = b7 + accum[7] * modulate;
            b8 = b8 + accum[8] * modulate;
        }
    }

    return max(
        sh_irradiance(b0, b1, b2, b3, b4, b5, b6, b7, b8, normal),
        vec3<f32>(0.0),
    );
}

fn sample_sh_indirect(world_pos: vec3<f32>, normal: vec3<f32>) -> vec3<f32> {
    if sh_grid.has_sh_volume == 0u {
        return vec3<f32>(0.0);
    }

    // Hardware trilinear through the 8-corner cell cube. Wall-bleed mitigation
    // is deferred to the forthcoming lightmap-based compiler rework, which
    // will address bleed at bake time.
    let gdims_u = sh_grid.grid_dimensions;
    let gdims_f = max(vec3<f32>(gdims_u) - vec3<f32>(1.0), vec3<f32>(0.0));
    let cell_coord = (world_pos - sh_grid.grid_origin) /
        max(sh_grid.cell_size, vec3<f32>(1.0e-6));
    let gf = clamp(cell_coord, vec3<f32>(0.0), gdims_f);
    let gi = vec3<u32>(floor(gf));
    let gfrac = fract(gf);

    return sample_sh_indirect_fast(world_pos, normal, gi, gfrac);
}

@fragment
fn fs_main(in: VertexOutput) -> @location(0) vec4<f32> {
    let base_color = textureSample(base_texture, base_sampler, in.uv);
    let N = normalize(in.world_normal);

    // Lighting isolation mode: split direct from indirect for leak debugging.
    //   0 = Normal        — direct + indirect (production shading)
    //   1 = DirectOnly    — zero the SH indirect term
    //   2 = IndirectOnly  — zero direct contributions (short-circuit the loop)
    //   3 = AmbientOnly   — zero both; only ambient floor survives
    // See `LightingIsolation` in postretro/src/render/mod.rs. The ambient
    // floor always contributes so interior geometry is never pitch black.
    let iso = uniforms.lighting_isolation;
    let use_indirect = (iso == 0u) || (iso == 2u);
    let use_direct = (iso == 0u) || (iso == 1u);

    // Indirect term: baked SH irradiance. Zero when no SH volume is loaded
    // or when the isolation mode suppresses indirect.
    var indirect = vec3<f32>(0.0);
    if use_indirect {
        indirect = sample_sh_indirect(in.world_position, N);
    }

    // Total light = ambient floor (minimum) + indirect + direct sum.
    var total_light = vec3<f32>(uniforms.ambient_floor) + indirect;

    // DirectOnly / AmbientOnly modes skip the direct-light loop entirely —
    // cheaper than zeroing contributions inside the loop.
    let light_count = select(0u, uniforms.light_count, use_direct);
    for (var i: u32 = 0u; i < light_count; i = i + 1u) {
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

        // No runtime shadows in this iteration — the legacy runtime shadow
        // systems have been retired ahead of the lighting rework that will
        // reintroduce baked static shadows plus a small runtime spot shadow
        // map pool.
        total_light = total_light + light.color_and_falloff_model.xyz * attenuation * NdotL;
    }

    let rgb = base_color.rgb * total_light;
    return vec4<f32>(rgb, base_color.a);
}
