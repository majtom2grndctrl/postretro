// Depth-only shadow pass for point lights — linear depth encoding.
// Point light shadow maps store linear distance (length(frag - light_pos) / range)
// rather than perspective-Z, avoiding non-linear precision issues for omnidirectional
// shadow maps.
// See: context/plans/in-progress/lighting-foundation/5-shadow-maps.md

struct ShadowUniforms {
    light_view_proj: mat4x4<f32>,
};

struct PointLightParams {
    light_pos: vec3<f32>,
    light_range: f32,
};

@group(0) @binding(0) var<uniform> shadow_uniforms: ShadowUniforms;
@group(0) @binding(1) var<uniform> point_params: PointLightParams;

struct VertexInput {
    @location(0) position: vec3<f32>,
};

struct VertexOutput {
    @builtin(position) clip_position: vec4<f32>,
    @location(0) world_position: vec3<f32>,
};

@vertex
fn vs_main(in: VertexInput) -> VertexOutput {
    var out: VertexOutput;
    out.clip_position = shadow_uniforms.light_view_proj * vec4<f32>(in.position, 1.0);
    out.world_position = in.position;
    return out;
}

@fragment
fn fs_main(in: VertexOutput) -> @builtin(frag_depth) f32 {
    let to_frag = in.world_position - point_params.light_pos;
    let dist = length(to_frag);
    return dist / max(point_params.light_range, 0.001);
}
