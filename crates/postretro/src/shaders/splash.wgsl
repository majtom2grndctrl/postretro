// Boot splash logo quad: a single textured quad placed by a device-pixel rect
// over the cleared swapchain. No instancing, no 9-slice — one quad, one texture.
// See: context/lib/rendering_pipeline.md §7.8 · context/lib/boot_sequence.md §1

// Device viewport in pixels (vec2) + the logo's device-pixel rect [x, y, w, h].
// The vertex stage maps a unit quad into the rect, then into clip space against
// the viewport — top-left origin, matching the UI quad convention.
struct SplashUniform {
    viewport: vec2<f32>,
    _pad: vec2<f32>,
    rect: vec4<f32>,
}
@group(0) @binding(0) var<uniform> u: SplashUniform;
@group(0) @binding(1) var logo_tex: texture_2d<f32>;
@group(0) @binding(2) var logo_sampler: sampler;

struct VsOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) uv: vec2<f32>,
}

// Two triangles (6 vertices) covering the rect. `unit` is the quad corner in
// [0,1]; `uv` matches it so the texture maps left→right, top→bottom.
@vertex
fn vs_main(@builtin(vertex_index) vid: u32) -> VsOut {
    var corners = array<vec2<f32>, 6>(
        vec2<f32>(0.0, 0.0),
        vec2<f32>(1.0, 0.0),
        vec2<f32>(0.0, 1.0),
        vec2<f32>(0.0, 1.0),
        vec2<f32>(1.0, 0.0),
        vec2<f32>(1.0, 1.0),
    );
    let unit = corners[vid];

    // Device-pixel position of this corner within the rect (top-left origin).
    let px = u.rect.xy + unit * u.rect.zw;
    // Pixels → NDC: x to [-1, 1], y flipped (pixel y grows downward, NDC up).
    let ndc = vec2<f32>(
        px.x / u.viewport.x * 2.0 - 1.0,
        1.0 - px.y / u.viewport.y * 2.0,
    );

    var out: VsOut;
    out.clip = vec4<f32>(ndc, 0.0, 1.0);
    out.uv = unit;
    return out;
}

@fragment
fn fs_main(in: VsOut) -> @location(0) vec4<f32> {
    return textureSample(logo_tex, logo_sampler, in.uv);
}
