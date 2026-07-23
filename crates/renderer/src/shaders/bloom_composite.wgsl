struct BloomParams {
    texel_size: vec2<f32>,
    threshold: f32,
    intensity: f32,
    direction: vec2<f32>,
    _padding: vec2<f32>,
};

@group(0) @binding(0) var bloom_source: texture_2d<f32>;
@group(0) @binding(1) var bloom_sampler: sampler;
@group(0) @binding(2) var<uniform> bloom: BloomParams;

struct FullscreenOutput {
    @builtin(position) position: vec4<f32>,
    @location(0) uv: vec2<f32>,
};

@vertex
fn vs_main(@builtin(vertex_index) vertex_index: u32) -> FullscreenOutput {
    var positions = array<vec2<f32>, 3>(
        vec2<f32>(-1.0, -1.0),
        vec2<f32>(3.0, -1.0),
        vec2<f32>(-1.0, 3.0),
    );
    let position = positions[vertex_index];
    var output: FullscreenOutput;
    output.position = vec4<f32>(position, 0.0, 1.0);
    output.uv = position * vec2<f32>(0.5, -0.5) + vec2<f32>(0.5);
    return output;
}

@fragment
fn fs_upsample(input: FullscreenOutput) -> @location(0) vec4<f32> {
    let offset = bloom.texel_size * 0.5;
    let color = (
        textureSample(bloom_source, bloom_sampler, input.uv + vec2<f32>(-offset.x, -offset.y)).rgb +
        textureSample(bloom_source, bloom_sampler, input.uv + vec2<f32>( offset.x, -offset.y)).rgb +
        textureSample(bloom_source, bloom_sampler, input.uv + vec2<f32>(-offset.x,  offset.y)).rgb +
        textureSample(bloom_source, bloom_sampler, input.uv + vec2<f32>( offset.x,  offset.y)).rgb
    ) * 0.25;
    return vec4<f32>(color, 1.0);
}

@fragment
fn fs_composite(input: FullscreenOutput) -> @location(0) vec4<f32> {
    let color = textureSample(bloom_source, bloom_sampler, input.uv).rgb * bloom.intensity;
    return vec4<f32>(color, 1.0);
}
