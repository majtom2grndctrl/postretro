// Storage-written gradient used to verify the live diagnostic dependency.
@group(0) @binding(0) var atlas: texture_storage_2d_array<rgba16float, write>;

@compute @workgroup_size(8, 8)
fn main(@builtin(global_invocation_id) id: vec3<u32>) {
    if (any(id.xy >= textureDimensions(atlas)) || id.z >= textureNumLayers(atlas)) {
        return;
    }
    textureStore(atlas, vec2<i32>(id.xy), i32(id.z), vec4<f32>(vec3<f32>(id), 1.0));
}
