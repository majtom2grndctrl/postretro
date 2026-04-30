// Billboard sprite pass: camera-facing quads for env_smoke_emitter entities,
// lit by the full lighting stack (SH ambient, static multi-source specular via
// the chunk light list, and dynamic direct diffuse). Alpha-additive, depth
// test enabled, depth write disabled.
//
// See: context/lib/rendering_pipeline.md §7.4

// --- Group 0: camera uniforms (shared with forward pass) ---
struct Uniforms {
    view_proj: mat4x4<f32>,
    camera_position: vec3<f32>,
    ambient_floor: f32,
    light_count: u32,
    time: f32,
    lighting_isolation: u32,
    _pad: u32,
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

// --- Group 3: SH irradiance volume (shared with forward) ---
struct ShGridInfo {
    grid_origin: vec3<f32>,
    has_sh_volume: u32,
    cell_size: vec3<f32>,
    _pad0: u32,
    grid_dimensions: vec3<u32>,
    _pad1: u32,
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

    var out: VertexOutput;
    out.clip_position = uniforms.view_proj * vec4<f32>(world_pos, 1.0);
    out.uv = vec2<f32>(u, v);
    out.world_position = sprite_pos;
    out.opacity = opacity;
    return out;
}

// --- Lighting helpers (copy of subset of forward.wgsl) ---

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

fn sh_irradiance(
    b0: vec3<f32>, b1: vec3<f32>, b2: vec3<f32>, b3: vec3<f32>,
    b4: vec3<f32>, b5: vec3<f32>, b6: vec3<f32>, b7: vec3<f32>, b8: vec3<f32>,
    normal: vec3<f32>,
) -> vec3<f32> {
    let nx = normal.x;
    let ny = normal.y;
    let nz = normal.z;
    var r: vec3<f32> = b0 * 0.282095;
    r = r + b1 * (-0.488603 * ny);
    r = r + b2 * ( 0.488603 * nz);
    r = r + b3 * (-0.488603 * nx);
    r = r + b4 * ( 1.092548 * nx * ny);
    r = r + b5 * (-1.092548 * ny * nz);
    r = r + b6 * ( 0.315392 * (3.0 * nz * nz - 1.0));
    r = r + b7 * (-1.092548 * nx * nz);
    r = r + b8 * ( 0.546274 * (nx * nx - ny * ny));
    return r;
}

fn sample_sh_indirect(world_pos: vec3<f32>, normal: vec3<f32>) -> vec3<f32> {
    if sh_grid.has_sh_volume == 0u {
        return vec3<f32>(0.0);
    }
    let gdims_u = sh_grid.grid_dimensions;
    let gdims_f = max(vec3<f32>(gdims_u), vec3<f32>(1.0));
    let cell_coord = (world_pos - sh_grid.grid_origin) /
        max(sh_grid.cell_size, vec3<f32>(1.0e-6));
    let gf = clamp(cell_coord, vec3<f32>(0.0), gdims_f - vec3<f32>(1.0));
    let gi = vec3<u32>(floor(gf));
    let gfrac = fract(gf);
    let cell_center_uvw = (vec3<f32>(gi) + vec3<f32>(0.5) + gfrac) / gdims_f;

    let b0 = textureSampleLevel(sh_band0, sh_sampler, cell_center_uvw, 0.0).rgb;
    let b1 = textureSampleLevel(sh_band1, sh_sampler, cell_center_uvw, 0.0).rgb;
    let b2 = textureSampleLevel(sh_band2, sh_sampler, cell_center_uvw, 0.0).rgb;
    let b3 = textureSampleLevel(sh_band3, sh_sampler, cell_center_uvw, 0.0).rgb;
    let b4 = textureSampleLevel(sh_band4, sh_sampler, cell_center_uvw, 0.0).rgb;
    let b5 = textureSampleLevel(sh_band5, sh_sampler, cell_center_uvw, 0.0).rgb;
    let b6 = textureSampleLevel(sh_band6, sh_sampler, cell_center_uvw, 0.0).rgb;
    let b7 = textureSampleLevel(sh_band7, sh_sampler, cell_center_uvw, 0.0).rgb;
    let b8 = textureSampleLevel(sh_band8, sh_sampler, cell_center_uvw, 0.0).rgb;

    return max(
        sh_irradiance(b0, b1, b2, b3, b4, b5, b6, b7, b8, normal),
        vec3<f32>(0.0),
    );
}

@fragment
fn fs_main(in: VertexOutput) -> @location(0) vec4<f32> {
    let sprite_sample = textureSample(sprite_texture, sprite_sampler, in.uv);

    // Camera forward points from the fragment toward the camera; we use it
    // as the normal for Blinn-Phong so sprites respond to lights as if their
    // "face" always points at the viewer. Matches the billboard aesthetic:
    // broad, soft highlights, not per-texel shading.
    let V = normalize(uniforms.camera_position - in.world_position);
    let N = V;

    // SH ambient.
    let sh_ambient = sample_sh_indirect(in.world_position, N);

    // Multi-source static specular via the chunk light list.
    //
    // Chunk-list fallback: when `has_chunk_grid == 0` (no chunk index built),
    // fall back to SH + dynamic only. Iterating the full spec buffer here
    // would be expensive and the spec contribution is a "gravy" term — the
    // acceptance gate only requires no panic / no black sprites in the
    // fallback, which the early-skip delivers.
    var static_specular = vec3<f32>(0.0);
    let spec_int = max(draw_params.params.y, 0.0);
    if chunk_grid.has_chunk_grid != 0u && spec_int > 0.0 {
        let local = in.world_position - chunk_grid.grid_origin;
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
                let to_light = sl.position_and_range.xyz - in.world_position;
                let dist = length(to_light);
                let range = sl.position_and_range.w;
                if range > 0.0 && dist > range {
                    continue;
                }
                let L = to_light / max(dist, 0.0001);
                let atten = select(1.0, max(1.0 - dist / max(range, 0.001), 0.0), range > 0.0);
                // Broad highlight on smoke: low specular exponent.
                let contribution = blinn_phong(L, V, N, sl.color_and_pad.xyz, 4.0, spec_int) * atten;
                static_specular = static_specular + contribution;
            }
        }
    }

    // Dynamic direct (diffuse only — sharp specular highlights on billboards
    // read as artifact).
    var dynamic_diffuse = vec3<f32>(0.0);
    let light_count = uniforms.light_count;
    for (var i: u32 = 0u; i < light_count; i = i + 1u) {
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
                let to_light = light.position_and_type.xyz - in.world_position;
                let dist = length(to_light);
                L = to_light / max(dist, 0.0001);
                attenuation = falloff(dist, light.direction_and_range.w, falloff_model);
            }
            case 1u: {
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

    let lighting = sh_ambient + static_specular + dynamic_diffuse;
    let rgb = sprite_sample.rgb * lighting * in.opacity;
    // Alpha channel is used as the additive blend factor; driver expects
    // straight color. The pipeline's blend state is set to additive
    // (src=ONE, dst=ONE) so the alpha here is not consumed for blending.
    return vec4<f32>(rgb * sprite_sample.a, sprite_sample.a * in.opacity);
}
