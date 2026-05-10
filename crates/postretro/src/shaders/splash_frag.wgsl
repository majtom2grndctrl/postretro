// Splash fragment shader — nearest-neighbor sample of the splash texture.
// The texture is Rgba8UnormSrgb and the swapchain is sRGB, so wgpu
// performs sRGB decode on sample and sRGB encode on write — no manual
// gamma here.

@group(0) @binding(1) var splash_tex: texture_2d<f32>;
@group(0) @binding(2) var splash_sampler: sampler;

@fragment
fn fs_main(@location(0) uv: vec2<f32>) -> @location(0) vec4<f32> {
    return textureSample(splash_tex, splash_sampler, uv);
}
