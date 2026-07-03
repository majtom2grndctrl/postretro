// Wireframe overlay — world-triangle diagnostics.
// The cull-status mode draws each BVH leaf's geometry color-coded by the value
// the compute-cull shader wrote into `cull_status`:
//   0 → cyan  (not submitted by the GPU cull pass; may include skipped subtree descendants)
//   1 → red   (leaf explicitly marked frustum-culled)
//             Path note: on the candidate cull path, a leaf that is both
//             in a hidden cell AND frustum-rejected is never submitted, so
//             its slot reads 0 (cyan), not 1. The legacy tree walk always
//             writes 1 for a frustum reject. Overlay-only cosmetic difference
//             — the drawn output is identical on both paths.
//   2 → green (rendered: survived both tests)
// The CPU-visible mode uses the same vertex path, but a flat fragment color so
// it cannot be mistaken for final GPU BVH/frustum survivors.
//
// The leaf index is passed via `instance_index` (first_instance in the
// draw call). See: context/lib/rendering_pipeline.md §7.1

// Layout must match the `Uniforms` struct in forward.wgsl — the two
// shaders share a single uniform buffer binding.
struct Uniforms {
    view_proj: mat4x4<f32>,
    camera_position: vec3<f32>,
    ambient_floor: f32,
    light_count: u32,
    time: f32,
    lighting_isolation: u32,
    indirect_scale: f32,
    // See forward.wgsl — same bitset gating the SDF shadow multiplies; the
    // wireframe pass shares the same uniform buffer so the struct strides
    // must match. The wireframe pipeline never references this field.
    sdf_shadow_flags: u32,
    // Same slot as `sdf_shadow_mode` in forward.wgsl. The wireframe pass
    // never reads it; the field preserves the shared uniform layout/stride.
    sdf_shadow_mode: u32,
    // `sdf_force_visibility_one` in forward.wgsl (offset 104..108) — never read
    // by the wireframe pipeline, present only to keep the shared group-0
    // `Uniforms` stride in lockstep with forward.wgsl. See forward.wgsl for
    // semantics.
    _sdf_force_visibility_one_inert: u32,
    // `dynamic_direct_scale` in forward.wgsl (offset 108..112) — never read by
    // the wireframe pipeline, present only to keep the shared group-0 `Uniforms`
    // stride in lockstep with forward.wgsl (128 bytes). See forward.wgsl for
    // semantics.
    _dynamic_direct_scale_inert: u32,
    _dyn_pad0: u32,
    _dyn_pad1: u32,
    _dyn_pad2: u32,
    _dyn_pad3: u32,
};

@group(0) @binding(0) var<uniform> uniforms: Uniforms;
@group(1) @binding(0) var<storage, read> cull_status: array<u32>;

struct VertexInput {
    @location(0) position: vec3<f32>,
    @location(1) base_uv: vec2<f32>,
    @location(2) normal_oct: vec2<u32>,
    @location(3) tangent_packed: vec2<u32>,
};

struct VertexOutput {
    @builtin(position) clip_position: vec4<f32>,
    @location(0) @interpolate(flat) chunk_idx: u32,
};

@vertex
fn vs_main(in: VertexInput, @builtin(instance_index) instance_idx: u32) -> VertexOutput {
    var out: VertexOutput;
    out.clip_position = uniforms.view_proj * vec4<f32>(in.position, 1.0);
    out.chunk_idx = instance_idx;
    return out;
}

@fragment
fn fs_cull_status(in: VertexOutput) -> @location(0) vec4<f32> {
    let status = cull_status[in.chunk_idx];
    // 0 = not submitted, 1 = explicitly frustum-culled, 2 = visible
    switch status {
        case 2u: { return vec4<f32>(0.0, 1.0, 0.2, 1.0); }  // green: rendered
        case 1u: { return vec4<f32>(1.0, 0.2, 0.15, 1.0); } // red: explicitly frustum-culled
        default: { return vec4<f32>(0.0, 0.9, 0.9, 1.0); }  // cyan: not submitted
    }
}

@fragment
fn fs_visible(_in: VertexOutput) -> @location(0) vec4<f32> {
    return vec4<f32>(1.0, 0.92, 0.18, 1.0);
}
