// Depth-only shadow pass for dynamic spot lights.
// Renders one frame per allocated shadow-map slot, transforming geometry
// into light-space coordinates and writing depth only.
//
// See: context/plans/in-progress/lighting-spot-shadows/index.md § Task B

struct LightSpaceUniforms {
    light_proj: mat4x4<f32>,
};

@group(0) @binding(0) var<uniform> light_space: LightSpaceUniforms;

struct VertexInput {
    @location(0) position: vec3<f32>,
    @location(1) base_uv: vec2<f32>,
    @location(2) normal_oct: vec2<u32>,
    @location(3) tangent_packed: vec2<u32>,
    @location(4) lightmap_uv_packed: vec2<u32>,
};

// `@invariant` on position matches the forward pass convention to ensure
// bit-exact depth computation on all GPUs. Not strictly necessary since
// shadow-map depth doesn't participate in Equal tests, but kept for consistency.
@vertex
fn vs_main(in: VertexInput) -> @invariant @builtin(position) vec4<f32> {
    return light_space.light_proj * vec4<f32>(in.position, 1.0);
}
