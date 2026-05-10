// Splash fragment shader. Rgba8UnormSrgb texture + sRGB swapchain: wgpu handles
// both conversions — no manual gamma. UVs outside [0,1]^2 fill with SPLASH_BG.

// Linear-space sRGB(21, 27, 35). Keep in sync with SPLASH_BG_COLOR in splash.rs.
const SPLASH_BG: vec4f = vec4f(0.00750, 0.01093, 0.01672, 1.0);

@group(0) @binding(1) var splash_tex: texture_2d<f32>;
@group(0) @binding(2) var splash_sampler: sampler;

@fragment
fn fs_main(@location(0) uv: vec2<f32>) -> @location(0) vec4<f32> {
    if (uv.x < 0.0 || uv.x > 1.0 || uv.y < 0.0 || uv.y > 1.0) {
        return SPLASH_BG;
    }
    // Alpha-composite over SPLASH_BG. Both are linear: SPLASH_BG by definition,
    // texture by sRGB-decode-on-sample.
    let s = textureSample(splash_tex, splash_sampler, uv);
    return vec4f(mix(SPLASH_BG.rgb, s.rgb, s.a), 1.0);
}
