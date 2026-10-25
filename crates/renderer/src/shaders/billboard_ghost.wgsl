// Billboard ghost shader: unlit camera-facing quads for projectile ghosts.
// See: context/lib/rendering_pipeline.md §7.9, §7.4
//
// Composed with `ghost_look.wgsl` only: no lights, shadows or SH. Layout:
// camera (0), the real billboard pass's sprite-sheet group (1) and the ghost
// sprite instances (2), bound per collection at a dynamic offset as the
// billboard pass binds its own. `corner_for_vertex`, `corner_data`,
// `camera_right_up`, `sample_post_retro` and the flipbook frame selection are
// `billboard.wgsl`'s verbatim, so a ghost quad covers and samples exactly what
// its projectile's does; a test pins the copies equal. One fragment entry:
// billboard ghosts always blend, additively as every billboard does, so their
// draw order changes no pixel.

struct CameraUniforms {
    view_proj: mat4x4<f32>,
};
@group(0) @binding(0) var<uniform> uniforms: CameraUniforms;

@group(1) @binding(0) var sprite_texture: texture_2d_array<f32>;
@group(1) @binding(1) var sprite_sampler: sampler;
struct SpriteDrawParams {
    // x = animation frame count (u32 bits), z = lifetime (seconds).
    params: vec4<f32>,
    params2: vec4<f32>,
};
@group(1) @binding(2) var<uniform> draw_params: SpriteDrawParams;

struct GhostSpriteInstance {
    // xyz = mapped world position, w = age (seconds).
    position_and_age: vec4<f32>,
    size: f32,
    rotation: f32,
    // The packed look (`GhostInstanceLook::pack`); its opacity already folds
    // in the sprite's own.
    look: vec2<u32>,
};
@group(2) @binding(0) var<storage, read> sprites: array<GhostSpriteInstance>;

struct VertexOutput {
    @builtin(position) clip_position: vec4<f32>,
    @location(0) uv: vec2<f32>,
    @location(1) @interpolate(flat) frame_idx: u32,
    @location(2) @interpolate(flat) look: vec2<u32>,
};

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

fn corner_data(c: u32) -> vec4<f32> {
    if c == 0u { return vec4<f32>(-1.0,  1.0, 0.0, 0.0); }
    if c == 1u { return vec4<f32>( 1.0,  1.0, 1.0, 0.0); }
    if c == 2u { return vec4<f32>(-1.0, -1.0, 0.0, 1.0); }
    return vec4<f32>( 1.0, -1.0, 1.0, 1.0);
}

fn camera_right_up(vp: mat4x4<f32>) -> mat2x3<f32> {
    // view_proj column-major; rows are (vp[0][0], vp[1][0], vp[2][0], vp[3][0]).
    let r = normalize(vec3<f32>(vp[0][0], vp[1][0], vp[2][0]));
    let u = normalize(vec3<f32>(vp[0][1], vp[1][1], vp[2][1]));
    return mat2x3<f32>(r, u);
}

fn sample_post_retro(tex: texture_2d_array<f32>, samp: sampler, uv: vec2<f32>, layer: u32,
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
    return textureSampleGrad(tex, samp, uv_recon, i32(layer), ddx, ddy);
}

@vertex
fn vs_main(@builtin(vertex_index) vidx: u32) -> VertexOutput {
    let sprite_index = vidx / 6u;
    let cd = corner_data(corner_for_vertex(vidx));
    let inst = sprites[sprite_index];
    let sprite_pos = inst.position_and_age.xyz;
    let age = inst.position_and_age.w;

    let basis = camera_right_up(uniforms.view_proj);
    let cs = cos(inst.rotation);
    let sn = sin(inst.rotation);
    let rx = cd.x * cs - cd.y * sn;
    let ry = cd.x * sn + cd.y * cs;
    let half = inst.size * 0.5;
    let world_pos = sprite_pos + basis[0] * (rx * half) + basis[1] * (ry * half);

    let frame_count = max(bitcast<u32>(draw_params.params.x), 1u);
    let lifetime = max(draw_params.params.z, 1.0e-6);
    let frame_duration = lifetime / f32(frame_count);
    let frame_idx = u32(floor(age / max(frame_duration, 1.0e-6))) % frame_count;

    var out: VertexOutput;
    out.clip_position = uniforms.view_proj * vec4<f32>(world_pos, 1.0);
    out.uv = vec2<f32>(cd.z, cd.w);
    out.frame_idx = frame_idx;
    out.look = inst.look;
    return out;
}

// Unlit, premultiplied by the sprite's coverage and the presented opacity for
// the additive (One + One) blend: a fading ghost dims toward nothing, never
// toward black.
@fragment
fn fs_main(in: VertexOutput) -> @location(0) vec4<f32> {
    let ddx = dpdx(in.uv);
    let ddy = dpdy(in.uv);
    let sprite = sample_post_retro(sprite_texture, sprite_sampler, in.uv, in.frame_idx, ddx, ddy);
    let coverage = sprite.a * ghost_opacity(in.look);
    return vec4<f32>(ghost_shade(sprite.rgb, in.look) * coverage, coverage);
}
