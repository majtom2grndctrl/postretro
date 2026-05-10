// Splash vertex shader — emits a fullscreen triangle (no vertex buffer) and
// computes aspect-correct UVs that center the splash texture at 1/5 scale.
// UVs outside [0,1] are handled by the fragment shader (background fill),
// not by ClampToEdge.

struct SplashUbo {
    screen_size: vec2<f32>,
    tex_size: vec2<f32>,
};

@group(0) @binding(0) var<uniform> ubo: SplashUbo;

struct VsOut {
    @builtin(position) position: vec4<f32>,
    @location(0) uv: vec2<f32>,
};

// Logo scale relative to letterboxed full-fit size. UVs outside [0,1]^2
// spill to the fragment shader's background fill rather than edge texels.
const LOGO_SCALE: f32 = 0.4;

@vertex
fn vs_main(@builtin(vertex_index) vid: u32) -> VsOut {
    var positions = array<vec2<f32>, 3>(
        vec2<f32>(-1.0, -1.0),
        vec2<f32>( 3.0, -1.0),
        vec2<f32>(-1.0,  3.0),
    );
    var uvs = array<vec2<f32>, 3>(
        vec2<f32>(0.0, 1.0),
        vec2<f32>(2.0, 1.0),
        vec2<f32>(0.0, -1.0),
    );

    let pos = positions[vid];
    let uv_full = uvs[vid];

    // ratio > 1 = screen wider than texture (letterbox x); < 1 = taller (letterbox y).
    // Dividing by scale * LOGO_SCALE expands UVs beyond [0,1]; fragment fills the bars.
    let screen_aspect = ubo.screen_size.x / ubo.screen_size.y;
    let tex_aspect = ubo.tex_size.x / ubo.tex_size.y;
    let ratio = screen_aspect / tex_aspect;

    var scale = vec2<f32>(1.0, 1.0);
    if (ratio > 1.0) {
        scale.x = 1.0 / ratio;
    } else {
        scale.y = ratio;
    }

    let uv = vec2<f32>(0.5, 0.5) + (uv_full - vec2<f32>(0.5, 0.5)) / (scale * LOGO_SCALE);

    var out: VsOut;
    out.position = vec4<f32>(pos, 0.0, 1.0);
    out.uv = uv;
    return out;
}
