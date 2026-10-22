// Kinematic ghost shader: unlit translucent mover geometry at a link's mapped model.
// See: context/lib/rendering_pipeline.md §7.9
//
// Composed with `ghost_look.wgsl` only (shade, opacity, screen-door): ghosts
// bind no lights, shadows or SH, so the layout is camera (0), the world
// material (1) and the ghost instances (2). `sample_post_retro` is the
// kinematic brush pass's body verbatim, so a ghost samples its mover's texels
// as the mover does; a test pins the copies equal. Two fragment entries, one
// per `render.ghosts` mode, each sampling before any `discard` so the
// derivatives stay in uniform control flow; neither loops (§8 FXC).

struct CameraUniforms {
    view_proj: mat4x4<f32>,
};
@group(0) @binding(0) var<uniform> camera: CameraUniforms;

// The world material bind group the mover pass binds; only the base color and
// its sampler are read.
@group(1) @binding(0) var base_texture: texture_2d<f32>;
@group(1) @binding(5) var aniso_sampler: sampler;

struct Instance {
    model: mat4x4<f32>,
    // xy = the packed look (`GhostInstanceLook::pack`), zw = 0.
    look: vec4<u32>,
};
@group(2) @binding(0) var<storage, read> instances: array<Instance>;

// The world vertex stream; unlit, so the normal and tangent are not declared.
struct VertexInput {
    @location(0) position: vec3<f32>,
    @location(1) base_uv: vec2<f32>,
};

struct VertexOutput {
    @builtin(position) clip_position: vec4<f32>,
    @location(0) uv: vec2<f32>,
    @location(1) @interpolate(flat) look: vec2<u32>,
};

@vertex
fn vs_main(in: VertexInput, @builtin(instance_index) instance_index: u32) -> VertexOutput {
    var out: VertexOutput;
    let instance = instances[instance_index];
    out.clip_position = camera.view_proj * (instance.model * vec4<f32>(in.position, 1.0));
    out.uv = in.base_uv;
    out.look = instance.look.xy;
    return out;
}

fn sample_post_retro(tex: texture_2d<f32>, samp: sampler, uv: vec2<f32>,
                     ddx: vec2<f32>, ddy: vec2<f32>) -> vec4<f32> {
    let dims = vec2<f32>(textureDimensions(tex, 0));
    let uv_tex = uv * dims;
    let seam = floor(uv_tex + 0.5);
    // Seam width comes from the ORIGINAL screen-space derivatives, not from
    // fwidth() of `uv_tex`. Under Surface Depth `uv` is the MARCHED UV, which
    // jumps between adjacent pixels that hit different texel faces, so its
    // fwidth measures the parallax step rather than the pixel footprint and
    // would widen the AA band into a multi-texel blur along every carved edge.
    // fwidth(uv * dims) == (abs(dpdx(uv)) + abs(dpdy(uv))) * dims for a smooth
    // uv, so this is the same quantity the un-marched path always computed.
    // Floor the divisor: a constant-UV fragment (edge-on face, degenerate UV
    // chart, vanishing derivatives) gives zero, and clamp() does not reliably
    // sanitize the resulting NaN/Inf in WGSL.
    let seam_width = max((abs(ddx) + abs(ddy)) * dims, vec2<f32>(1.0e-6));
    let aa = clamp((uv_tex - seam) / seam_width, vec2(-0.5), vec2(0.5));
    let uv_recon = (seam + aa) / dims;
    return textureSampleGrad(tex, samp, uv_recon, ddx, ddy);
}

// The mover's base color at this fragment. Ghosts are flat: no Surface Depth
// march, which would need the world position and normal an unlit draw drops.
fn ghost_base(uv: vec2<f32>) -> vec4<f32> {
    return sample_post_retro(base_texture, aniso_sampler, uv, dpdx(uv), dpdy(uv));
}

// `render.ghosts: "blend"`: SrcAlpha / OneMinusSrcAlpha at the presented opacity.
@fragment
fn fs_blend(in: VertexOutput) -> @location(0) vec4<f32> {
    let base = ghost_base(in.uv);
    return vec4<f32>(ghost_shade(base.rgb, in.look), ghost_opacity(in.look));
}

// `render.ghosts: "dither"`: blend off; a pixel draws opaque when the presented
// opacity exceeds its screen-door threshold.
@fragment
fn fs_dither(in: VertexOutput) -> @location(0) vec4<f32> {
    let base = ghost_base(in.uv);
    let color = ghost_shade(base.rgb, in.look);
    if ghost_opacity(in.look) <= screen_door_threshold(vec2<u32>(in.clip_position.xy)) {
        discard;
    }
    return vec4<f32>(color, 1.0);
}
