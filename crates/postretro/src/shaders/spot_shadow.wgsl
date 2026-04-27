// Depth-only shadow pass for dynamic spot lights.
// Renders one frame per allocated shadow-map slot, transforming geometry
// into light-space coordinates and writing depth only.
//
// See: context/plans/in-progress/lighting-spot-shadows/index.md § Task B

struct LightSpaceUniforms {
    light_proj: mat4x4<f32>,
};

@group(0) @binding(0) var<uniform> light_space: LightSpaceUniforms;

// Only position is needed for shadow-map depth. The vertex buffer binds
// WorldVertex so the pipeline layout declares the other attributes to
// match the stride, but this shader ignores them.
@vertex
fn vs_main(@location(0) position: vec3<f32>) -> @builtin(position) vec4<f32> {
    return light_space.light_proj * vec4<f32>(position, 1.0);
}
