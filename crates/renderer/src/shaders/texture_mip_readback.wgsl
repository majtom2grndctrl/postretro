// Test-only GPU probe: copies mip 0 of a sampled texture into a storage buffer,
// for textures created without COPY_SRC.
@group(0) @binding(0) var source: texture_2d<f32>;
@group(0) @binding(1) var<storage, read_write> output: array<vec4<f32>>;

@compute @workgroup_size(1)
fn main(@builtin(global_invocation_id) id: vec3<u32>) {
    let size = textureDimensions(source, 0);
    output[id.y * size.x + id.x] = textureLoad(source, vec2<u32>(id.xy), 0);
}
