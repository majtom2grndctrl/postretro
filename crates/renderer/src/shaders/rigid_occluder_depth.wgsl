// Rigid depth-only shadow occluders projected by a per-render light-space matrix.
// See: context/lib/rendering_pipeline.md §7.1
//
// Group 0 is the slot/face light matrix selected through a dynamic offset. Group
// 1 is a dense array of rigid model transforms shared with the caller's beauty
// pass. Material, lighting, and camera bindings are intentionally absent.

struct LightSpaceUniforms {
    light_proj: mat4x4<f32>,
};
@group(0) @binding(0) var<uniform> light_space: LightSpaceUniforms;

struct InstanceTransform {
    model: mat4x4<f32>,
};
@group(1) @binding(0) var<storage, read> instances: array<InstanceTransform>;

struct VertexInput {
    @location(0) position: vec3<f32>,
};

@vertex
fn vs_main(
    in: VertexInput,
    @builtin(instance_index) instance_index: u32,
) -> @builtin(position) vec4<f32> {
    let world_position = instances[instance_index].model * vec4<f32>(in.position, 1.0);
    return light_space.light_proj * world_position;
}
