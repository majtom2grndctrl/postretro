// Splash vertex shader — emits a fullscreen triangle (no vertex buffer) and
// computes aspect-correct UVs that letterbox the splash texture into the
// swapchain. The fragment shader pairs with this; sampler is ClampToEdge so
// the letterbox bars sample the splash's solid edge texels.

struct SplashUbo {
    screen_size: vec2<f32>,
    tex_size: vec2<f32>,
};

@group(0) @binding(0) var<uniform> ubo: SplashUbo;

struct VsOut {
    @builtin(position) position: vec4<f32>,
    @location(0) uv: vec2<f32>,
};

// Three-vertex fullscreen triangle. Positions chosen so the triangle covers
// the whole NDC rectangle [-1,1]^2; the unused fourth corner is outside the
// triangle and gets clipped. UV at the corners spans [0,1] across the full
// rect (the off-screen vertices land beyond [0,1] but get clipped).
@vertex
fn vs_main(@builtin(vertex_index) vid: u32) -> VsOut {
    // (x, y) in clip space; (u, v) in [0, 1] across the rect.
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

    // Aspect-correct letterbox: scale UVs around 0.5 so the texture fits
    // within the screen without stretching. The axis whose ratio is larger
    // (relative to the texture's aspect) gets shrunk; the other fills.
    //
    // ratio = (screen_aspect / tex_aspect). When >1, the screen is wider
    // than the texture, so the texture should fill height and leave
    // horizontal letterbox bars: shrink U by 1/ratio. When <1, the screen
    // is taller, so shrink V by ratio.
    let screen_aspect = ubo.screen_size.x / ubo.screen_size.y;
    let tex_aspect = ubo.tex_size.x / ubo.tex_size.y;
    let ratio = screen_aspect / tex_aspect;

    var scale = vec2<f32>(1.0, 1.0);
    if (ratio > 1.0) {
        scale.x = 1.0 / ratio;
    } else {
        scale.y = ratio;
    }

    // Divide (not multiply) by scale: this maps screen [0,1] to a UV range
    // wider than [0,1] on the letterboxed axis. ClampToEdge then samples the
    // edge texel for out-of-range UVs, filling the bars.
    let uv = vec2<f32>(0.5, 0.5) + (uv_full - vec2<f32>(0.5, 0.5)) / scale;

    var out: VsOut;
    out.position = vec4<f32>(pos, 0.0, 1.0);
    out.uv = uv;
    return out;
}
