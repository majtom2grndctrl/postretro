// Skinned depth-only pass — GPU vertex skinning projected by a per-render
// light-space matrix. Renders animated entity occluders into a shadow map.
// See: context/lib/rendering_pipeline.md §7.1, §9
//
// This is the union of `depth_prepass.wgsl` (vertex-only, no fragment stage)
// and `spot_shadow.wgsl` (light-space projection), PLUS the skinning kernel
// from `skinned_mesh.wgsl`. It drops the color attributes (base UV, normal,
// tangent) — only position + joints + weights are read.
//
// The light-space matrix is a PER-RENDER parameter (group 0), so one pipeline
// serves both spot slots (the existing per-slot `shadow_vs_bind_group` +
// dynamic offset) and cube faces (per-face dynamic offset into the cube shadow
// VS uniform buffer). Nothing here
// assumes a 2D target or one slot per light — the target view + matrix are
// supplied by the orchestration per render.
//
// Group 3 (palette + per-instance SSBO) is the SAME bind group the forward
// skinned-mesh pass binds (`render/mesh_pass.rs` builds it). Forced to index 3
// so this depth layout is bind-group-compatible with the mesh pass's group 3
// without re-uploading the buffers — the depth layout simply omits groups
// 1, 2, 4.

// --- Group 0: per-render light-space matrix ----------------------------------
// The spot path binds the renderer's `shadow_vs_bind_group` (a 64-byte mat4x4
// per slot, selected by dynamic offset) — the SAME buffer the world-geometry
// spot-shadow depth pass uses, so the entity occluders project into the slot
// with the exact light-space transform the slot was ranked/culled against.
struct LightSpaceUniforms {
    light_proj: mat4x4<f32>,
};
@group(0) @binding(0) var<uniform> light_space: LightSpaceUniforms;

// --- Group 3: skinned instance data (shared with the forward mesh pass) ------
// Layout-identical to `skinned_mesh.wgsl`'s group 3: the shared bone palette at
// binding 0 and the per-instance SSBO at binding 1, read by
// `@builtin(instance_index)`. The forward mesh pass writes these buffers once
// per frame BEFORE the shadow passes (the pose/upload hoist), so both passes
// read the identical, already-posed data with no one-frame lag.
struct BonePaletteEntry {
    matrix: mat4x4<f32>,
};
@group(3) @binding(0) var<storage, read> bone_palette: array<BonePaletteEntry>;

struct Instance {
    model: mat4x4<f32>,
    // x = base index into `bone_palette`; yzw padding (16-byte std430 align).
    base_and_pad: vec4<u32>,
};
@group(3) @binding(1) var<storage, read> instances: array<Instance>;

// Blend the four joint matrices for this vertex into one skinning matrix.
// Copied verbatim from `skinned_mesh.wgsl::skin_matrix` (the reusable kernel) —
// WGSL string-concat composition does not support sharing a function that reads
// module-scope buffers across two shaders that declare those buffers at the same
// bindings, so the kernel is duplicated. Keep it in lock-step with the forward
// path's `skin_matrix` if either changes.
fn skin_matrix(joints: vec4<u32>, weights: vec4<f32>, base: u32) -> mat4x4<f32> {
    let m0 = bone_palette[base + joints.x].matrix;
    let m1 = bone_palette[base + joints.y].matrix;
    let m2 = bone_palette[base + joints.z].matrix;
    let m3 = bone_palette[base + joints.w].matrix;
    return m0 * weights.x + m1 * weights.y + m2 * weights.z + m3 * weights.w;
}

// Only position + skinning attributes are consumed. The other attributes
// (base_uv / normal_oct / tangent_packed) are NOT declared here — the vertex
// buffer layout this pipeline declares omits them entirely (unlike the
// depth pre-pass, which keeps the full world layout to share a buffer; here the
// skinned-depth pipeline owns a position+joints+weights layout).
struct VertexInput {
    @location(0) position: vec3<f32>,
    @location(4) joints: vec4<u32>,
    @location(5) weights: vec4<f32>,
};

@vertex
fn vs_main(in: VertexInput, @builtin(instance_index) instance_index: u32) -> @builtin(position) vec4<f32> {
    let instance = instances[instance_index];
    let base = instance.base_and_pad.x;
    let skin = skin_matrix(in.joints, in.weights, base);

    // Skin → model → light-space. Mirrors the forward path's skin → model →
    // view-proj, with the camera view-projection swapped for the light-space
    // matrix so the depth written is the occluder's depth from the light.
    let skinned_pos = skin * vec4<f32>(in.position, 1.0);
    let world_pos = instance.model * skinned_pos;
    return light_space.light_proj * world_pos;
}
