// Skinned ghost shader: unlit translucent skinned instances at a mapped pose.
// See: context/lib/rendering_pipeline.md §7.9
//
// Composed with `ghost_look.wgsl` only (shade, opacity, screen-door): ghosts
// bind no lights, shadows or SH, so the pipeline layout is camera (0), material
// (1) and the shared skinned instance data at group 3, the slot the skinned
// pass binds it at; group 2 is empty, as in the skinned depth pipeline.
// `skin_matrix` is mirrored verbatim from `skinned_mesh.wgsl`; a test pins the
// copies equal. The look rides `Instance.base_and_pad.zw`
// (`postretro_render_cpu::ghost_instance::GhostInstanceLook::pack`):
// z = pack2x16unorm(opacity, saturation), w = pack4x8unorm(tint.rgb, 0).
//
// Two fragment entry points, one per `render.ghosts` mode. Each samples the
// base color before any `discard`, so the implicit-derivative sample stays in
// uniform control flow, and neither has a loop (rendering_pipeline.md §8 FXC).

struct CameraUniforms {
    view_proj: mat4x4<f32>,
};
@group(0) @binding(0) var<uniform> camera: CameraUniforms;

// Same material bind group the skinned pass binds; only diffuse and the
// character-model sampler are read.
@group(1) @binding(0) var base_texture: texture_2d<f32>;
@group(1) @binding(5) var character_model_sampler: sampler;

struct BonePaletteEntry {
    matrix: mat4x4<f32>,
};
@group(3) @binding(0) var<storage, read> bone_palette: array<BonePaletteEntry>;

struct Instance {
    model: mat4x4<f32>,
    // x = palette base, y = shadow-bias scale bits (unused here),
    // z = opacity | saturation << 16 (unorm16x2), w = tint rgb (unorm8x4).
    base_and_pad: vec4<u32>,
};
@group(3) @binding(1) var<storage, read> instances: array<Instance>;

// The skinned vertex stream; unlit, so normal and tangent are not declared.
struct VertexInput {
    @location(0) position: vec3<f32>,
    @location(1) base_uv: vec2<f32>,
    @location(4) joints: vec4<u32>,
    @location(5) weights: vec4<f32>,
};

struct VertexOutput {
    @builtin(position) clip_position: vec4<f32>,
    @location(0) uv: vec2<f32>,
    // The packed look lanes, unpacked per fragment.
    @location(1) @interpolate(flat) look: vec2<u32>,
};

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
fn vs_main(in: VertexInput, @builtin(instance_index) instance_index: u32) -> VertexOutput {
    var out: VertexOutput;
    let instance = instances[instance_index];
    let skin = skin_matrix(in.joints, in.weights, instance.base_and_pad.x);
    let world_pos = instance.model * (skin * vec4<f32>(in.position, 1.0));
    out.clip_position = camera.view_proj * world_pos;
    out.uv = in.base_uv;
    out.look = instance.base_and_pad.zw;
    return out;
}

// `render.ghosts: "blend"`: SrcAlpha / OneMinusSrcAlpha at the presented opacity.
@fragment
fn fs_blend(in: VertexOutput) -> @location(0) vec4<f32> {
    let base = textureSample(base_texture, character_model_sampler, in.uv);
    return vec4<f32>(ghost_shade(base.rgb, in.look), ghost_opacity(in.look));
}

// `render.ghosts: "dither"`: blend off; a pixel draws opaque when the presented
// opacity exceeds its screen-door threshold, so a 4×4 tile keeps the opacity's
// share of its pixels.
@fragment
fn fs_dither(in: VertexOutput) -> @location(0) vec4<f32> {
    let base = textureSample(base_texture, character_model_sampler, in.uv);
    let color = ghost_shade(base.rgb, in.look);
    if ghost_opacity(in.look) <= screen_door_threshold(vec2<u32>(in.clip_position.xy)) {
        discard;
    }
    return vec4<f32>(color, 1.0);
}
