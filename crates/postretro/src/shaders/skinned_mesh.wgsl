// Skinned-mesh forward pass — GPU vertex skinning + flat-lit base-color output.
// See: context/lib/rendering_pipeline.md §9
//
// Vertex skinning: each vertex carries 4 joint indices + 4 normalized weights.
// Each joint's matrix is fetched from a SHARED bone-palette storage buffer at
// `base_index + joint`, where `base_index` is this instance's contiguous run
// offset (one instance this slice → base 0). The blended skin matrix is applied
// to the bind-pose position, then the per-instance model matrix, then the
// camera view-projection.
//
// Vertex attribute decode (base_uv / normal_oct / tangent_packed) mirrors
// `forward.wgsl` so the skinned stream and world stream share one encoding;
// `gltf_loader.rs` encodes these with the same shared
// `postretro_level_format::octahedral` helper. `oct_decode` is copied verbatim
// from forward.wgsl. `tangent_packed` (location 3) is carried but unused this
// slice (flat-lit); its decode lands with the lighting / normal-mapping work.
//
// Lighting: flat-lit this slice — the fragment samples the material base-color
// texture (group 1) and outputs it with a trivial constant ambient term. The
// settled dynamic-entity lighting interface gets its own ADDITIVE bind group
// later (the broadening lighting task); no lighting group is allocated here.
//
// Design note (non-binding): the skinning vertex stage is kept separable so a
// future position-only depth-only skinned variant (the shadow task) can reuse
// `skin_position` without the color attributes. Nothing depth-only is built here.

// --- Group 0: camera ---------------------------------------------------------
// Reuses the renderer's camera uniform (the full forward `Uniforms` buffer).
// Only `view_proj` at offset 0 is referenced; trailing bytes are ignored, same
// as depth_prepass.wgsl. WGSL permits binding a larger uniform buffer.
struct CameraUniforms {
    view_proj: mat4x4<f32>,
};
@group(0) @binding(0) var<uniform> camera: CameraUniforms;

// --- Group 1: material -------------------------------------------------------
// Same layout `build_material_bind_group` produces (bindings 0,2,3,4,5). The
// flat-lit fragment only samples the diffuse (binding 0) through the aniso
// sampler (binding 5); the other bindings are declared so the bind group the
// renderer already builds is layout-compatible with this pipeline.
@group(1) @binding(0) var base_texture: texture_2d<f32>;
@group(1) @binding(5) var aniso_sampler: sampler;

// --- Group 3: skinned instance data ------------------------------------------
// Group 2 is intentionally left UNALLOCATED — it is the provisional lighting
// slot the broadening lighting task fills (SH ambient + dynamic direct). The
// pipeline layout passes `None` for group 2 so the future task adds a slot
// rather than renumbering existing groups.
//
// `bone_palette` is the SHARED palette storage buffer; every instance's run of
// `BonePaletteEntry` (one mat4 per joint) is appended into it. `instance.base`
// selects this instance's run. `instance.model` is the per-instance world
// transform (the entity transform; basis/scale conversion folded in Rust-side —
// see render/mesh_pass.rs — which for glTF Y-up/RH/meters → engine Y-up/RH/meters
// is the identity).
struct BonePaletteEntry {
    matrix: mat4x4<f32>,
};
@group(3) @binding(0) var<storage, read> bone_palette: array<BonePaletteEntry>;

struct InstanceUniforms {
    model: mat4x4<f32>,
    // x = base index into `bone_palette` (bitcast u32). yzw padding.
    base_and_pad: vec4<u32>,
};
@group(3) @binding(1) var<uniform> instance: InstanceUniforms;

struct VertexInput {
    @location(0) position: vec3<f32>,
    // base_uv is stored u16-quantized; the vertex layout declares it Unorm16x2
    // (render/mesh_pass.rs), so it arrives here hardware-decoded to 0..1 floats.
    @location(1) base_uv: vec2<f32>,
    @location(2) normal_oct: vec2<u32>,
    @location(3) tangent_packed: vec2<u32>,
    // u8x4 joints + u8x4 weights, both supplied as Uint32x... no — see
    // render/mesh_pass.rs vertex layout: joints arrive as Uint8x4 (→ vec4<u32>)
    // and weights as Unorm8x4 (→ vec4<f32> already normalized 0..1).
    @location(4) joints: vec4<u32>,
    @location(5) weights: vec4<f32>,
};

struct VertexOutput {
    @builtin(position) clip_position: vec4<f32>,
    @location(0) uv: vec2<f32>,
    @location(1) world_normal: vec3<f32>,
};

// Octahedral unit-vector decode — copied verbatim from forward.wgsl so the
// skinned stream decodes normals with identical math. This is a deliberate
// deviation from rendering_pipeline.md §8's shared-helper append pattern:
// WGSL's string-append composition doesn't support the binding-agnostic
// helper shape for a pure math function like this. As a consequence, this
// copy MUST be updated in lock-step if `forward.wgsl::oct_decode` ever changes.
fn oct_decode(enc: vec2<u32>) -> vec3<f32> {
    let ox = f32(enc.x) / 65535.0 * 2.0 - 1.0;
    let oy = f32(enc.y) / 65535.0 * 2.0 - 1.0;
    let z = 1.0 - abs(ox) - abs(oy);
    var x: f32;
    var y: f32;
    if z < 0.0 {
        x = (1.0 - abs(oy)) * select(-1.0, 1.0, ox >= 0.0);
        y = (1.0 - abs(ox)) * select(-1.0, 1.0, oy >= 0.0);
    } else {
        x = ox;
        y = oy;
    }
    return normalize(vec3<f32>(x, y, z));
}

// Blend the four joint matrices for this vertex into one skinning matrix.
// Weights arrive already normalized (Unorm8x4 → 0..1); they are expected to sum
// to ~1. A rigid (no-skin) vertex is joint 0 weight 1 (the degenerate case),
// which yields exactly `bone_palette[base].matrix`.
fn skin_matrix(joints: vec4<u32>, weights: vec4<f32>, base: u32) -> mat4x4<f32> {
    let m0 = bone_palette[base + joints.x].matrix;
    let m1 = bone_palette[base + joints.y].matrix;
    let m2 = bone_palette[base + joints.z].matrix;
    let m3 = bone_palette[base + joints.w].matrix;
    return m0 * weights.x + m1 * weights.y + m2 * weights.z + m3 * weights.w;
}

@vertex
fn vs_main(in: VertexInput) -> VertexOutput {
    var out: VertexOutput;

    let base = instance.base_and_pad.x;
    let skin = skin_matrix(in.joints, in.weights, base);

    // Skin → model → view-proj. Skinning acts in model space; the model matrix
    // places the instance in the world (and folds any basis conversion); the
    // camera view-proj projects to clip space.
    let skinned_pos = skin * vec4<f32>(in.position, 1.0);
    let world_pos = instance.model * skinned_pos;
    out.clip_position = camera.view_proj * world_pos;

    out.uv = in.base_uv;

    // Decoded bind-pose normal, transformed by the skin + model upper-3x3.
    // Flat-lit fragment ignores it this slice, but carrying it keeps the
    // skinning vertex stage shaped for the lighting/shadow broadening tasks.
    let n_bind = oct_decode(in.normal_oct);
    let skin3 = mat3x3<f32>(skin[0].xyz, skin[1].xyz, skin[2].xyz);
    let model3 = mat3x3<f32>(instance.model[0].xyz, instance.model[1].xyz, instance.model[2].xyz);
    // Upper-3×3 is correct only for rotation and uniform scale. Per-instance
    // non-uniform scale requires the inverse-transpose of model3 here instead;
    // the broadening lighting task must make that switch if it introduces
    // non-uniform scale on skinned instances.
    out.world_normal = normalize(model3 * (skin3 * n_bind));

    return out;
}

@fragment
fn fs_main(in: VertexOutput) -> @location(0) vec4<f32> {
    // Flat-lit: sample base color, apply a trivial constant ambient so the model
    // reads at full albedo. No light loop, no SH — that is the broadening
    // lighting task's additive group. A future lighting group multiplies
    // `in.world_normal` against the settled dynamic-entity interface here.
    let base_color = textureSample(base_texture, aniso_sampler, in.uv);
    const FLAT_AMBIENT: f32 = 1.0;
    return vec4<f32>(base_color.rgb * FLAT_AMBIENT, base_color.a);
}
