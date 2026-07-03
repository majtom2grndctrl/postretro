// Depth pre-pass — populates the shared depth buffer so the subsequent
// forward pass can run with an Equal depth test and zero overdraw in the
// fragment shader. Vertex-only: there is no fragment stage and no color
// attachment. (The lightmap-UV gbuffer MRT this pass once wrote was removed
// with the animated dominant-direction trace — the per-light SDF trace keys
// on light position, not lightmap UV.)
//
// The vertex layout mirrors `forward.wgsl` so the same vertex buffer can
// be bound without a separate layout. Only `position` is consumed; the
// remaining attributes are declared to satisfy the shared layout but unused.

struct Uniforms {
    view_proj: mat4x4<f32>,
    // The remainder of the forward Uniforms struct is not referenced by
    // this shader; we only bind group 0 via a pipeline layout that uses
    // the same BGL, so naga needs the field list to at least include
    // view_proj at offset 0. Any trailing bytes in the buffer are safe
    // to ignore — WGSL allows binding a larger uniform buffer.
};

@group(0) @binding(0) var<uniform> uniforms: Uniforms;

struct VertexInput {
    @location(0) position: vec3<f32>,
    @location(1) base_uv: vec2<f32>,
    @location(2) normal_oct: vec2<u32>,
    @location(3) tangent_packed: vec2<u32>,
    @location(4) lightmap_uv_packed: vec2<u32>,
};

struct VertexOutput {
    // `@invariant` on the clip-space position guarantees that the Z produced
    // here matches the value the forward pass recomputes from the same vertex
    // data, so the `depth_compare: Equal` test doesn't fall victim to
    // fused-multiply-add reassociation drift on some GPUs.
    @invariant @builtin(position) clip_position: vec4<f32>,
};

@vertex
fn vs_main(in: VertexInput) -> VertexOutput {
    var out: VertexOutput;
    out.clip_position = uniforms.view_proj * vec4<f32>(in.position, 1.0);
    return out;
}
