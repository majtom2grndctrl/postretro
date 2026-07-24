struct BloomParams {
    texel_size: vec2<f32>,
    threshold: f32,
    intensity: f32,
    direction: vec2<f32>,
    source_block_divisor: u32,
    _padding: u32,
    output_dimensions: vec2<u32>,
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
    let destination_coord = vec2<i32>(input.position.xy);
    let source_origin = destination_coord * i32(bloom.source_block_divisor);
    var extracted = vec3<f32>(0.0);
    var valid_source_texels = 0u;
    for (var block_y = 0; block_y < i32(bloom.source_block_divisor); block_y++) {
        for (var block_x = 0; block_x < i32(bloom.source_block_divisor); block_x++) {
            let source_coord = source_origin + vec2<i32>(block_x, block_y);
            if (all(source_coord < source_dimensions)) {
                let color = textureLoad(bloom_source, source_coord, 0).rgb;
                let luminance = dot(color, vec3<f32>(0.2126, 0.7152, 0.0722));
                let excess = max(luminance - bloom.threshold, 0.0);
                extracted += color * (excess / max(luminance, 1.0e-4));
                valid_source_texels += 1u;
            }
        }
    }
    return vec4<f32>(extracted / f32(valid_source_texels), 1.0);
}
