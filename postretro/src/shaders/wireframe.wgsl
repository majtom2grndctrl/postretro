// Wireframe overlay — BVH-leaf cull-status debug visualization.
// Draws each leaf's geometry color-coded by the value the compute-cull
// shader wrote into `cull_status`:
//   0 → cyan  (portal-culled: cell not in visible set)
//   1 → red   (frustum-culled: leaf AABB outside frustum)
//   2 → green (rendered: survived both tests)
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
    _pad_a: u32,
    _pad_b: u32,
    _pad_c: u32,
    csm_splits: vec4<f32>,
    view_matrix: mat4x4<f32>,
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
fn fs_main(in: VertexOutput) -> @location(0) vec4<f32> {
    let status = cull_status[in.chunk_idx];
    // 0 = portal-culled, 1 = frustum-culled, 2 = visible
    switch status {
        case 2u: { return vec4<f32>(0.0, 1.0, 0.2, 1.0); }  // green: rendered
        case 1u: { return vec4<f32>(1.0, 0.2, 0.15, 1.0); } // red: frustum-culled
        default: { return vec4<f32>(0.0, 0.9, 0.9, 1.0); }  // cyan: portal-culled
    }
}
