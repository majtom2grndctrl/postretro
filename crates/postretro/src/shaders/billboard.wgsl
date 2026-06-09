// Billboard sprite pass: camera-facing quads for env_smoke_emitter entities,
// lit by the full lighting stack (SH ambient, static multi-source specular via
// the chunk light list, and dynamic direct diffuse). Alpha-additive, depth
// test enabled, depth write disabled.
//
// See: context/lib/rendering_pipeline.md §7.4

// --- Group 0: camera uniforms (shared with forward pass) ---
// Shares the forward pass's group-0 uniform buffer. The billboard path reads
// `view_proj`, `camera_position`, `light_count`, and the dynamic-direct tail
// (`direct_scale` / `dynamic_direct_isolation` / `has_direct`); the rest are
// declared so the field offsets line up with the Rust `Uniforms` writer (a
// 3-way byte contract: render/mod.rs + forward.wgsl + billboard.wgsl). The
// existing `lighting_isolation` stays the forward/static control and is NOT
// reused here.
struct Uniforms {
    view_proj: mat4x4<f32>,
    camera_position: vec3<f32>,
    ambient_floor: f32,
    light_count: u32,
    time: f32,
    lighting_isolation: u32,
    indirect_scale: f32,
    sdf_shadow_flags: u32,
    sdf_shadow_mode: u32,
    sdf_force_visibility_one: u32,
    // --- dynamic-direct tail (baked-static-direct-sh Task 6) ---
    // Multiplies the baked DIRECT SH term (0..1).
    direct_scale: f32,
    // 0 = combined (sh_ambient + scale·direct), 1 = direct-only (scale·direct),
    // 2 = indirect-only (sh_ambient). Separate from `lighting_isolation`.
    dynamic_direct_isolation: u32,
    // 0 when the baked DIRECT SH section is absent → skip the direct sample.
    has_direct: u32,
    _pad: u32,
    _dyn_pad1: u32,
};

@group(0) @binding(0) var<uniform> uniforms: Uniforms;

// --- Group 1: sprite sheet texture + sampler ---
@group(1) @binding(0) var sprite_texture: texture_2d<f32>;
@group(1) @binding(1) var sprite_sampler: sampler;

// --- Group 2: dynamic lights, spec-only buffer, chunk grid (shared with forward) ---
struct GpuLight {
    position_and_type: vec4<f32>,
    color_and_falloff_model: vec4<f32>,
    direction_and_range: vec4<f32>,
    cone_angles_and_pad: vec4<f32>,
};

@group(2) @binding(0) var<storage, read> lights: array<GpuLight>;
@group(2) @binding(1) var<storage, read> light_influence: array<vec4<f32>>;

struct SpecLight {
    position_and_range: vec4<f32>,
    color_and_pad: vec4<f32>,
    cone_dir_and_type: vec4<f32>, // xyz = normalized aim, w = light type (1.0 ⇒ spot)
    cone_cos: vec4<f32>,          // x = cos(inner), y = cos(outer); non-spot carries 1/-1
};
@group(2) @binding(2) var<storage, read> spec_lights: array<SpecLight>;

struct ChunkGridInfo {
    grid_origin: vec3<f32>,
    cell_size: f32,
    dims: vec3<u32>,
    has_chunk_grid: u32,
};
@group(2) @binding(3) var<uniform> chunk_grid: ChunkGridInfo;
@group(2) @binding(4) var<storage, read> chunk_offsets: array<vec2<u32>>;
@group(2) @binding(5) var<storage, read> chunk_indices: array<u32>;

// --- Group 3: octahedral irradiance atlas (shared with forward) ---
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
    atlas_tile_rows: u32, // computed Rust-side but not read by this shader — tile placement derives from atlas_tiles_per_row
    tile_interior: u32,
    _pad2: u32,
    probe_occlusion: u32,
    _pad3: u32,
    _pad4: u32,
    _pad5: u32,
};

@group(3) @binding(1) var sh_total_atlas: texture_2d<f32>;
@group(3) @binding(2) var sh_atlas_sampler: sampler;
@group(3) @binding(10) var<uniform> sh_grid: ShGridInfo;

// Animated-layer bindings must exist in the bind group layout so we can reuse
// the same group 3 bind group as the forward pass. The billboard lighting
// path does not evaluate animated layers (one sample per sprite vertex, not
// per fragment — animated pulses on smoke are invisible at this fidelity), so
// the bindings are declared but never read.
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
@group(3) @binding(11) var<storage, read> anim_descriptors: array<AnimationDescriptor>;
@group(3) @binding(12) var<storage, read> anim_samples: array<f32>;
// Depth moments are consumed by billboard's depth-aware indirect sampler. The
// path deliberately skips backface rejection because sprites have no stable
// geometric surface normal.
@group(3) @binding(14) var sh_depth_moments: texture_3d<f32>;
// Baked static direct SH atlas (BC6H-at-rest, hardware-decoded to f32). Bound at
// `BIND_SH_DIRECT_ATLAS` (binding 15) on the SHARED `ShVolumeResources` bind
// group layout — declared here at group 3, the same group billboard binds
// `sh_total_atlas` in. Same octahedral tile geometry as `sh_total_atlas`, so it
// samples through the shared `sh_sample.wgsl` chain with the same grid/sampler.
@group(3) @binding(15) var sh_direct_atlas: texture_2d<f32>;

// --- Group 6: sprite instance storage buffer ---
struct SpriteInstance {
    // Offsets match SPRITE_INSTANCE_SIZE in src/fx/smoke.rs.
    // xyz = world position, w = age (seconds).
    position_and_age: vec4<f32>,
    // x = size (world units), y = rotation (radians), z = opacity, w = pad.
    size_rot_opacity_pad: vec4<f32>,
};
@group(6) @binding(0) var<storage, read> sprites: array<SpriteInstance>;

// --- Per-draw push: frame count + specular intensity ---
// A single uniform buffer carrying per-collection draw parameters. Lives in
// group 1 binding 2 so it rides with the sprite sheet bind group; a separate
// group would burn a bind group slot we don't have spare.
struct SpriteDrawParams {
    // x = animation frame count (u32 in f32 bits — reinterpret).
    // y = spec_intensity (f32).
    // z = lifetime (f32, seconds).
    // w = pad.
    params: vec4<f32>,
};
@group(1) @binding(2) var<uniform> draw_params: SpriteDrawParams;

// --- Vertex input / output ---

struct VertexOutput {
    @builtin(position) clip_position: vec4<f32>,
    @location(0) uv: vec2<f32>,
    @location(1) world_position: vec3<f32>,
    @location(2) opacity: f32,
    // Full lighting term computed per-vertex: baked indirect + baked static
    // direct (with the dynamic-direct isolation debug mode applied) PLUS the
    // multi-source static-specular term and the dynamic-direct diffuse term
    // (Slice 2). Every lighting input derives from the sprite center
    // (`world_position`, identical at all four quad corners) and the
    // camera-facing `N = V`, so this term is constant across the quad —
    // interpolation reproduces the corner value exactly, matching the prior
    // per-fragment result with no visible change. The fragment shader does NO
    // lighting; it only samples the sprite texture and premultiplies.
    @location(3) lighting: vec3<f32>,
};

// Corner lookup table: the vertex shader expands each sprite into two triangles
// (6 vertices), with corners indexed by `lookup[vertex_index % 6]` into a
// 4-corner table (TL=0, TR=1, BL=2, BR=3).
//
// WGSL lacks compile-time `array<u32, 6>(…)` in global const in every version,
// so we use a runtime select chain inside the shader.
fn corner_for_vertex(v: u32) -> u32 {
    // lookup = [0, 1, 2, 2, 1, 3]
    let idx = v % 6u;
    var corner: u32 = 0u;
    if idx == 0u { corner = 0u; }
    else if idx == 1u { corner = 1u; }
    else if idx == 2u { corner = 2u; }
    else if idx == 3u { corner = 2u; }
    else if idx == 4u { corner = 1u; }
    else { corner = 3u; }
    return corner;
}

// Return (offset_x, offset_y, uv_x, uv_y) for a given corner index 0..3.
//   TL = (-1,  1, 0, 0)
//   TR = ( 1,  1, 1, 0)
//   BL = (-1, -1, 0, 1)
//   BR = ( 1, -1, 1, 1)
fn corner_data(c: u32) -> vec4<f32> {
    if c == 0u { return vec4<f32>(-1.0,  1.0, 0.0, 0.0); }
    if c == 1u { return vec4<f32>( 1.0,  1.0, 1.0, 0.0); }
    if c == 2u { return vec4<f32>(-1.0, -1.0, 0.0, 1.0); }
    return vec4<f32>( 1.0, -1.0, 1.0, 1.0);
}

// Recover the view-space right and up vectors from the view-projection matrix.
// The view matrix's basis vectors are row-0 (right) and row-1 (up) of the view
// matrix; with a perspective projection the view_proj's first two rows include
// the x/y projection scales, so we normalize to get unit basis directions.
fn camera_right_up(vp: mat4x4<f32>) -> mat2x3<f32> {
    // view_proj column-major; rows are (vp[0][0], vp[1][0], vp[2][0], vp[3][0]).
    let r = normalize(vec3<f32>(vp[0][0], vp[1][0], vp[2][0]));
    let u = normalize(vec3<f32>(vp[0][1], vp[1][1], vp[2][1]));
    return mat2x3<f32>(r, u);
}

@vertex
fn vs_main(@builtin(vertex_index) vidx: u32) -> VertexOutput {
    let sprite_index = vidx / 6u;
    let corner = corner_for_vertex(vidx);
    let cd = corner_data(corner);

    let inst = sprites[sprite_index];
    let sprite_pos = inst.position_and_age.xyz;
    let age = inst.position_and_age.w;
    let size = inst.size_rot_opacity_pad.x;
    let rotation = inst.size_rot_opacity_pad.y;
    let opacity = inst.size_rot_opacity_pad.z;

    // Camera-facing basis from the view matrix.
    let basis = camera_right_up(uniforms.view_proj);
    let right = basis[0];
    let up = basis[1];

    // Rotate the corner offset in the camera plane.
    let cs = cos(rotation);
    let sn = sin(rotation);
    let rx = cd.x * cs - cd.y * sn;
    let ry = cd.x * sn + cd.y * cs;

    let half = size * 0.5;
    let world_pos = sprite_pos + right * (rx * half) + up * (ry * half);

    // Frame index from age.
    let frame_count = max(bitcast<u32>(draw_params.params.x), 1u);
    let lifetime = max(draw_params.params.z, 1.0e-6);
    let frame_duration = lifetime / f32(frame_count);
    let frame_idx = u32(floor(age / max(frame_duration, 1.0e-6))) % frame_count;
    // Sprite sheet convention: frames laid out horizontally (N wide, 1 tall)
    // within a single texture. Since frames live in separate files and the
    // renderer stitches them into a single horizontal strip at upload time
    // (see renderer smoke pipeline), per-frame UV is
    //   u = (frame_idx + cd.z) / frame_count
    let u = (f32(frame_idx) + cd.z) / f32(frame_count);
    let v = cd.w;

    // Full lighting, hoisted from the fragment stage (Slice 1 SH terms + Slice 2
    // static-specular and dynamic-light loops). Every input derives from the
    // sprite center (`sprite_pos`) and the camera-facing normal `N = V`, so the
    // term is constant across the quad — computing it once per vertex and
    // interpolating reproduces the prior per-fragment value with no visible
    // change. The SH reads use `textureSampleLevel`/`textureLoad` and the loops
    // use only arithmetic / buffer reads (no implicit derivatives), all valid in
    // the vertex stage. Loop control flow stays uniform: every iteration count
    // (`chunk_count`, `light_count`) is a uniform value and every `continue`/
    // `break` predicate is uniform here because the loops run over the
    // per-sprite center, identical across the (single) invocation's data.
    let V = normalize(uniforms.camera_position - sprite_pos);
    let N = V;
    let sh_ambient = sample_sh_indirect(sprite_pos, N);
    var sh_direct = vec3<f32>(0.0);
    if uniforms.has_direct != 0u {
        sh_direct = uniforms.direct_scale * sample_sh_direct(sprite_pos, N);
    }
    // Dynamic-direct isolation over the SH terms (debug instrument):
    //   0 = combined     → sh_ambient + scale·direct
    //   1 = direct-only   → scale·direct
    //   2 = indirect-only → sh_ambient
    var sh_lighting = sh_ambient + sh_direct;
    if uniforms.dynamic_direct_isolation == 1u {
        sh_lighting = sh_direct;
    } else if uniforms.dynamic_direct_isolation == 2u {
        sh_lighting = sh_ambient;
    }

    // Multi-source static specular via the chunk light list (hoisted from the
    // fragment stage in Slice 2). Evaluated at the sprite center `sprite_pos`.
    //
    // Chunk-list fallback: when `has_chunk_grid == 0` (no chunk index built),
    // fall back to SH + dynamic only. Iterating the full spec buffer here would
    // be expensive and the spec contribution is a "gravy" term — the acceptance
    // gate only requires no panic / no black sprites in the fallback, which the
    // early-skip delivers.
    var static_specular = vec3<f32>(0.0);
    let spec_int = max(draw_params.params.y, 0.0);
    if chunk_grid.has_chunk_grid != 0u && spec_int > 0.0 {
        let local = sprite_pos - chunk_grid.grid_origin;
        let cell = vec3<i32>(floor(local / max(chunk_grid.cell_size, 1.0e-6)));
        let dims = vec3<i32>(chunk_grid.dims);
        if all(cell >= vec3<i32>(0)) && all(cell < dims) {
            let ci = u32(cell.z) * chunk_grid.dims.x * chunk_grid.dims.y
                   + u32(cell.y) * chunk_grid.dims.x
                   + u32(cell.x);
            let pair = chunk_offsets[ci];
            let chunk_offset = pair.x;
            let chunk_count = pair.y;
            for (var j: u32 = 0u; j < chunk_count; j = j + 1u) {
                let light_idx = chunk_indices[chunk_offset + j];
                let sl = spec_lights[light_idx];
                let to_light = sl.position_and_range.xyz - sprite_pos;
                let dist = length(to_light);
                let range = sl.position_and_range.w;
                if range > 0.0 && dist > range {
                    continue;
                }
                let L = to_light / max(dist, 0.0001);
                let atten = select(1.0, max(1.0 - dist / max(range, 0.001), 0.0), range > 0.0);
                let cone = cone_attenuation_cos(L, sl.cone_dir_and_type.xyz, sl.cone_cos.x, sl.cone_cos.y);
                // Broad highlight on smoke: low specular exponent.
                let contribution = blinn_phong(L, V, N, sl.color_and_pad.xyz, 4.0, spec_int) * (atten * cone);
                static_specular = static_specular + contribution;
            }
        }
    }

    // Dynamic direct (diffuse only — sharp specular highlights on billboards
    // read as artifact). Hoisted from the fragment stage in Slice 2; iterates a
    // uniform `light_count`, keeping vertex-stage control flow uniform.
    var dynamic_diffuse = vec3<f32>(0.0);
    let light_count = uniforms.light_count;
    for (var i: u32 = 0u; i < light_count; i = i + 1u) {
        let influence = light_influence[i];
        let inf_radius = influence.w;
        if inf_radius <= 1.0e30 {
            let dd = sprite_pos - influence.xyz;
            if dot(dd, dd) > inf_radius * inf_radius {
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
                let to_light = light.position_and_type.xyz - sprite_pos;
                let dist = length(to_light);
                L = to_light / max(dist, 0.0001);
                attenuation = falloff(dist, light.direction_and_range.w, falloff_model);
            }
            case 1u: {
                let to_light = light.position_and_type.xyz - sprite_pos;
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
                L = -light.direction_and_range.xyz;
                attenuation = 1.0;
            }
        }
        // Diffuse with N = camera forward — sprites treated as a flat disk
        // facing the viewer, so NdotL reduces to the angle between the light
        // direction and the view direction.
        let NdotL = max(dot(N, L), 0.0);
        dynamic_diffuse = dynamic_diffuse + light.color_and_falloff_model.xyz * attenuation * NdotL;
    }

    // Fold the Slice 1 SH term together with the Slice 2 static-specular and
    // dynamic-diffuse terms into the single interpolated lighting output.
    let lighting = sh_lighting + static_specular + dynamic_diffuse;

    var out: VertexOutput;
    out.clip_position = uniforms.view_proj * vec4<f32>(world_pos, 1.0);
    out.uv = vec2<f32>(u, v);
    out.world_position = sprite_pos;
    out.opacity = opacity;
    out.lighting = lighting;
    return out;
}

// --- Lighting helpers (copy of subset of forward.wgsl) ---

// Crisp texel snapping (copy of forward.wgsl `sample_post_retro`). Converts UV
// to texel space, snaps toward the nearest texel center, and antialiases only
// the seam between texels across an `fwidth`-wide band. Samples through the
// hardware linear sampler via `textureSampleGrad`, passing the ORIGINAL
// (unwarped) derivatives so mip/aniso footprint selection tracks the true
// screen-space pixel footprint. Snapping against the full sprite-strip
// `textureDimensions` is correct because the strip's texels are the sprite's
// texels.
fn sample_post_retro(tex: texture_2d<f32>, samp: sampler, uv: vec2<f32>,
                     ddx: vec2<f32>, ddy: vec2<f32>) -> vec4<f32> {
    let dims = vec2<f32>(textureDimensions(tex, 0));
    let uv_tex = uv * dims;
    let seam = floor(uv_tex + 0.5);
    // Floor the seam-width divisor: a fragment with constant UV (edge-on face or
    // vanishing derivatives) gives fwidth == 0; clamp() does not sanitize the
    // resulting NaN/Inf reliably in WGSL.
    let seam_width = max(fwidth(uv_tex), vec2<f32>(1.0e-6));
    let aa = clamp((uv_tex - seam) / seam_width, vec2(-0.5), vec2(0.5));
    let uv_recon = (seam + aa) / dims;
    // Pass the ORIGINAL derivatives, not derivatives of `uv_recon`. The warp
    // only shifts the sample point; mip selection and the aniso footprint must
    // track the true screen-space pixel footprint. Derivatives of the warped UV
    // collapse the footprint at seams and break mip/aniso selection.
    return textureSampleGrad(tex, samp, uv_recon, ddx, ddy);
}

fn blinn_phong(L: vec3<f32>, V: vec3<f32>, N: vec3<f32>,
               color: vec3<f32>, spec_exp: f32, spec_int: f32) -> vec3<f32> {
    let H = normalize(L + V);
    let NdH = max(dot(N, H), 0.0);
    return color * pow(NdH, spec_exp) * spec_int;
}

fn falloff(distance: f32, range: f32, model: u32) -> f32 {
    let r = max(range, 0.001);
    switch model {
        case 0u: { return max(1.0 - distance / r, 0.0); }
        case 1u: { return (1.0 / max(distance, 0.001)) * max(1.0 - distance / r, 0.0); }
        case 2u: {
            let d2 = max(distance * distance, 0.001);
            return (1.0 / d2) * max(1.0 - distance / r, 0.0);
        }
        default: { return 0.0; }
    }
}

fn cone_attenuation(L: vec3<f32>, aim: vec3<f32>, inner_angle: f32, outer_angle: f32) -> f32 {
    let cos_angle = dot(-L, aim);
    let cos_inner = cos(inner_angle);
    let cos_outer = cos(outer_angle);
    return smoothstep(cos_outer, cos_inner, cos_angle);
}

// Cone falloff from pre-baked cos cutoffs (static `SpecLight` path). Non-spot
// lights pack cos_inner = 1, cos_outer = -1 so this returns 1.0 everywhere.
fn cone_attenuation_cos(L: vec3<f32>, aim: vec3<f32>, cos_inner: f32, cos_outer: f32) -> f32 {
    let cos_angle = dot(-L, aim);
    return smoothstep(cos_outer, cos_inner, cos_angle);
}

// The depth-aware octahedral irradiance sampler lives in `sh_sample.wgsl`,
// concatenated after this source at pipeline-build time (render/smoke.rs
// `BILLBOARD_SHADER_SOURCE`). Billboard passes `reject_backface = false`: a
// camera-facing sprite has no real surface normal, so validity excludes invalid
// probes and depth visibility attenuates occluded probes; backface rejection
// does not apply.

// Derive the raw grid index / sub-cell fraction and defer the depth-aware
// 8-corner blend to the shared helper. `gi`/`gfrac` clamp the low side to the
// grid origin; the helper owns high-side edge clamping per corner. Billboard
// uses camera-forward (`N = V`) as its normal; with `reject_backface = false`
// the geo-normal arg is unused, so the shading normal fills both slots.
fn sample_sh_indirect(world_pos: vec3<f32>, normal: vec3<f32>) -> vec3<f32> {
    if sh_grid.has_sh_volume == 0u {
        return vec3<f32>(0.0);
    }
    let cell_coord = (world_pos - sh_grid.grid_origin) /
        max(sh_grid.cell_size, vec3<f32>(1.0e-6));
    // Clamp before deriving the fraction so a sample below the grid origin
    // resolves to clamp-to-edge instead of biasing the blend toward the interior.
    let clamped = max(cell_coord, vec3<f32>(0.0));
    let gi = vec3<u32>(floor(clamped));
    let gfrac = fract(clamped);

    return sample_sh_indirect_corners_depth_aware(
        gi,
        gfrac,
        world_pos,
        normal,
        normal,
        false,
        sh_grid.probe_occlusion != 0u,
    );
}

// Baked static direct SH read — the sibling of billboard's `sample_sh_indirect`
// against the direct atlas. Single-normal convention: the caller passes
// camera-forward (`N = V`) and this fills both the shading- and geo-normal slots
// (`reject_backface = false`, so the geo-normal arg is inert). Grid derivation
// is identical to the indirect wrapper so the two terms line up; the shared
// direct corner blend keeps Chebyshev ON, reading the shared `sh_depth_moments`.
fn sample_sh_direct(world_pos: vec3<f32>, normal: vec3<f32>) -> vec3<f32> {
    if sh_grid.has_sh_volume == 0u {
        return vec3<f32>(0.0);
    }
    let cell_coord = (world_pos - sh_grid.grid_origin) /
        max(sh_grid.cell_size, vec3<f32>(1.0e-6));
    let clamped = max(cell_coord, vec3<f32>(0.0));
    let gi = vec3<u32>(floor(clamped));
    let gfrac = fract(clamped);

    return sample_sh_direct_corners_depth_aware(
        sh_direct_atlas,
        gi,
        gfrac,
        world_pos,
        normal,
        normal,
        false,
        sh_grid.probe_occlusion != 0u,
    );
}

@fragment
fn fs_main(in: VertexOutput) -> @location(0) vec4<f32> {
    // Derivatives of the unwarped UV, computed in uniform control flow (WGSL
    // requirement) so they can feed `textureSampleGrad` inside `sample_post_retro`.
    let ddx = dpdx(in.uv);
    let ddy = dpdy(in.uv);
    let sprite_sample = sample_post_retro(sprite_texture, sprite_sampler, in.uv, ddx, ddy);

    // The fragment shader does NO lighting (Slice 2). The full lighting term —
    // baked indirect + baked static direct + static specular + dynamic diffuse —
    // is computed per-vertex and arrives interpolated. Every lighting input
    // derives from the sprite center and the camera-facing `N = V`, so the term
    // is constant across the quad and the interpolated value equals the prior
    // per-fragment result. See `vs_main` / `VertexOutput.lighting`.
    let lighting = in.lighting;
    let rgb = sprite_sample.rgb * lighting * in.opacity;
    // Alpha channel is used as the additive blend factor; driver expects
    // straight color. The pipeline's blend state is set to additive
    // (src=ONE, dst=ONE) so the alpha here is not consumed for blending.
    return vec4<f32>(rgb * sprite_sample.a, sprite_sample.a * in.opacity);
}
