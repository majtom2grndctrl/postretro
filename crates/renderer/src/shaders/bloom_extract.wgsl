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
fn fs_main(input: FullscreenOutput) -> @location(0) vec4<f32> {
    let source_dimensions = vec2<i32>(textureDimensions(bloom_source));
    let max_source_coord = source_dimensions - vec2<i32>(1);
    let source_coord = vec2<i32>(input.position.xy) * 2;
    let sample_coords = array<vec2<i32>, 4>(
        source_coord,
        source_coord + vec2<i32>(1, 0),
        source_coord + vec2<i32>(0, 1),
        source_coord + vec2<i32>(1, 1),
    );
    var extracted = vec3<f32>(0.0);
    for (var sample_index = 0u; sample_index < 4u; sample_index++) {
        let color = textureLoad(
            bloom_source,
            clamp(sample_coords[sample_index], vec2<i32>(0), max_source_coord),
            0,
        ).rgb;
        let luminance = dot(color, vec3<f32>(0.2126, 0.7152, 0.0722));
        let excess = max(luminance - bloom.threshold, 0.0);
        extracted += color * (excess / max(luminance, 1.0e-4));
    }
    return vec4<f32>(extracted * 0.25, 1.0);
}
