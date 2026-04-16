// Depth-only shadow pass — vertex shader for directional and spot lights.
// No fragment shader: standard depth write via fixed-function pipeline.
// See: context/plans/in-progress/lighting-foundation/5-shadow-maps.md

struct ShadowUniforms {
    light_view_proj: mat4x4<f32>,
};

@group(0) @binding(0) var<uniform> shadow_uniforms: ShadowUniforms;

struct VertexInput {
    @location(0) position: vec3<f32>,
};

struct VertexOutput {
    @builtin(position) clip_position: vec4<f32>,
};

@vertex
fn vs_main(in: VertexInput) -> VertexOutput {
    var out: VertexOutput;
    out.clip_position = shadow_uniforms.light_view_proj * vec4<f32>(in.position, 1.0);
    return out;
}
