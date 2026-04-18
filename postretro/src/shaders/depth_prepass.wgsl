// Depth pre-pass — minimal vertex-only shader that populates the shared
// depth buffer so the subsequent forward pass can run with a Equal depth
// test and zero overdraw in the fragment shader.
//
// The vertex layout mirrors `forward.wgsl` so the same vertex buffer can
// be bound without a separate layout. Only `position` is consumed; the
// remaining attributes are declared to satisfy the shared layout but
// otherwise unused.

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
};

// `@invariant` on the position output guarantees that the clip-space Z
// produced here matches the value the forward pass recomputes from the
// same vertex data, so the `depth_compare: Equal` test doesn't fall
// victim to fused-multiply-add reassociation drift on some GPUs.
@vertex
fn vs_main(in: VertexInput) -> @invariant @builtin(position) vec4<f32> {
    return uniforms.view_proj * vec4<f32>(in.position, 1.0);
}
