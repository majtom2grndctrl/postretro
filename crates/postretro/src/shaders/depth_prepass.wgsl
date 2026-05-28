// Depth pre-pass — populates the shared depth buffer so the subsequent
// forward pass can run with a Equal depth test and zero overdraw in the
// fragment shader. It also writes a full-res lightmap-UV gbuffer (one
// Rg16Float MRT slot) that the half-res SDF shadow pass samples for
// per-texel direction-texture lookups. The fragment stage does no shading
// work — one ROP write behind early-Z — so it stays cheap.
//
// The vertex layout mirrors `forward.wgsl` so the same vertex buffer can
// be bound without a separate layout. Only `position` and
// `lightmap_uv_packed` are consumed; the remaining attributes are declared
// to satisfy the shared layout but otherwise unused.

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
    @location(0) lightmap_uv: vec2<f32>,
};

@vertex
fn vs_main(in: VertexInput) -> VertexOutput {
    var out: VertexOutput;
    out.clip_position = uniforms.view_proj * vec4<f32>(in.position, 1.0);
    // Unpack identically to forward.wgsl. The UV is born as u16/65535; the
    // resulting float is stored in the Rg16Float target (16-bit float loses a
    // little mantissa precision near 1.0 vs an exact u16 round-trip).
    out.lightmap_uv = vec2<f32>(
        f32(in.lightmap_uv_packed.x) / 65535.0,
        f32(in.lightmap_uv_packed.y) / 65535.0,
    );
    return out;
}

// Pass the interpolated lightmap UV straight to the gbuffer. Behind early-Z
// the only surviving fragments are the visible surface, so the written UV is
// the visible-surface UV the shadow pass needs.
@fragment
fn fs_main(in: VertexOutput) -> @location(0) vec2<f32> {
    return in.lightmap_uv;
}
