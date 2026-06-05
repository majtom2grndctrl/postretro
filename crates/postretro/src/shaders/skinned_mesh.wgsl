// Skinned-mesh forward pass — GPU vertex skinning + SH-lit indirect base-color output.
// See: context/lib/rendering_pipeline.md §9
//
// Vertex skinning: each vertex carries 4 joint indices + 4 normalized weights.
// Each joint's matrix is fetched from a SHARED bone-palette storage buffer at
// `base_index + joint`, where `base_index` is this instance's contiguous run
// offset, read from the per-instance SSBO via `@builtin(instance_index)`. The
// blended skin matrix is applied to the bind-pose position, then the
// per-instance model matrix, then the camera view-projection.
//
// Vertex attribute decode (base_uv / normal_oct / tangent_packed) mirrors
// `forward.wgsl` so the skinned stream and world stream share one encoding;
// `gltf_loader.rs` encodes these with the same shared
// `postretro_level_format::octahedral` helper. `oct_decode` is copied verbatim
// from forward.wgsl. `tangent_packed` (location 3) is carried but unused by the SH-lit fragment
// (no normal map yet); its decode lands with the normal-mapping work.
//
// Lighting: SH-lit indirect baseline. The fragment samples the material
// base-color texture (group 1) and multiplies it by the baked spherical-harmonic
// irradiance read from the SH volume at NEW group 4 (the same `ShVolumeResources`
// bind group the forward/billboard/fog passes hold — bound here at slot 4, no
// new resource). The depth-aware octahedral helper from `sh_sample.wgsl` is
// appended to this source at pipeline creation (render/mesh_pass.rs), mirroring
// how forward.wgsl is assembled (render/mod.rs `SHADER_SOURCE`).
//
// Entities follow the BILLBOARD precedent, not forward's static-surface variant:
// `reject_backface = false` (a moving skinned entity is not a static world
// surface) with Chebyshev probe-occlusion on. No direct lighting is computed
// here); group 2 is reserved for the dynamic-direct additive term when that
// work lands.
//
// Design note (non-binding): the skinning vertex stage is kept separable so a
// future position-only depth-only skinned variant (the shadow task) can share
// `skin_matrix` and drop the color attributes (normal, UVs). Nothing depth-only
// is built here.

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
// SH-lit fragment samples only diffuse (binding 0) through the aniso sampler
// (binding 5); SH irradiance comes from group 4. The other bindings are
// declared for layout compatibility with the renderer's existing bind group.
@group(1) @binding(0) var base_texture: texture_2d<f32>;
@group(1) @binding(5) var aniso_sampler: sampler;

// --- Group 3: skinned instance data ------------------------------------------
// Group 2 is intentionally left UNALLOCATED — the provisional slot the
// dynamic-direct lighting task adds later (SH indirect already ships at
// group 4). The pipeline layout passes `None` for group 2 so that task adds
// a slot rather than renumbering existing groups.
//
// `bone_palette` is the SHARED palette storage buffer; every instance's run of
// `BonePaletteEntry` (one mat4 per joint) is appended into it. Each instance's
// `Instance.base_index` selects its run; `Instance.model` is its per-instance
// world transform (the entity transform; basis/scale conversion folded in
// Rust-side — see render/mesh_pass.rs — which for glTF Y-up/RH/meters → engine
// Y-up/RH/meters is the identity).
struct BonePaletteEntry {
    matrix: mat4x4<f32>,
};
@group(3) @binding(0) var<storage, read> bone_palette: array<BonePaletteEntry>;

// Per-instance data, one entry per batched instance, read by
// `@builtin(instance_index)`. std430 layout: `model` (mat4x4, 64 B) then a
// trailing `vec4<u32>` whose x is the palette base index (yzw padding) — total
// 80 B, base at byte 64. The base index NEVER travels through `first_instance`
// (DX12 reads it as 0, gfx-rs/wgpu#2471); it lives here, addressed by the
// instance index. This SSBO is shaped to drop into `multi_draw_indexed_indirect`
// later without a contract change; this pass draws with instanced `draw_indexed`.
struct Instance {
    model: mat4x4<f32>,
    // x = base index into `bone_palette`. yzw padding (16-byte std430 align).
    base_and_pad: vec4<u32>,
};
@group(3) @binding(1) var<storage, read> instances: array<Instance>;

// --- Group 4: SH irradiance volume (baked indirect) --------------------------
// The SAME `ShVolumeResources` bind group the forward/billboard/fog passes use,
// bound here at group 4 (group 3 is locked to instance data; group 2 stays
// UNALLOCATED). Binding indices mirror forward.wgsl exactly (b1/b2/b10/b14); the
// renderer puts the shared `bind_group_layout` at slot 4 in the mesh pipeline
// layout and binds the shared `bind_group` there (bind groups are
// group-index-agnostic at `set_bind_group` time). The appended `sh_sample.wgsl`
// helper reads these four bindings by lexical name. The animated-layer storage
// buffers (b11/b12/b13) live in the same bind group layout but are not declared
// here — the mesh indirect path never evaluates animated layers, mirroring
// billboard.wgsl, which only needs the four indirect-sampling bindings... except
// the bind group layout still carries them, and WGSL binds by declared name, so
// omitting the unused bindings is legal and layout-compatible.
struct ShGridInfo {
    grid_origin: vec3<f32>,
    has_sh_volume: u32,
    cell_size: vec3<f32>,
    _pad0: u32,
    grid_dimensions: vec3<u32>,
    _pad1: u32,
    atlas_dimensions: vec2<u32>,
    tile_dimension: u32,
    tile_border: u32,
    atlas_tiles_per_row: u32,
    atlas_tile_rows: u32, // computed Rust-side but not read by this shader — tile placement derives from atlas_tiles_per_row
    tile_interior: u32,
    _pad2: u32,
    probe_occlusion: u32,
    _pad3: u32,
    _pad4: u32,
    _pad5: u32,
};

@group(4) @binding(1) var sh_total_atlas: texture_2d<f32>;
@group(4) @binding(2) var sh_atlas_sampler: sampler;
@group(4) @binding(10) var<uniform> sh_grid: ShGridInfo;
@group(4) @binding(14) var sh_depth_moments: texture_3d<f32>;

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
    // Skinned world-space position (`model * skin * bind_pos`), used by the
    // fragment to key the SH irradiance lookup. The clip position above is this
    // same point projected; the SH sampler needs the un-projected world point.
    @location(2) world_position: vec3<f32>,
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
fn vs_main(in: VertexInput, @builtin(instance_index) instance_index: u32) -> VertexOutput {
    var out: VertexOutput;

    let instance = instances[instance_index];
    let base = instance.base_and_pad.x;
    let skin = skin_matrix(in.joints, in.weights, base);

    // Skin → model → view-proj. Skinning acts in model space; the model matrix
    // places the instance in the world (and folds any basis conversion); the
    // camera view-proj projects to clip space.
    let skinned_pos = skin * vec4<f32>(in.position, 1.0);
    let world_pos = instance.model * skinned_pos;
    out.clip_position = camera.view_proj * world_pos;
    out.world_position = world_pos.xyz;

    out.uv = in.base_uv;

    // Decoded bind-pose normal, transformed by the skin + model upper-3x3. The
    // SH-lit fragment uses it as both the shading normal (octahedral irradiance
    // direction lookup) and the geometric normal (the backface test, which the
    // entity path disables — see `sample_sh_indirect`).
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

// The depth-aware octahedral irradiance sampler lives in `sh_sample.wgsl`,
// concatenated after this source at pipeline-build time (render/mesh_pass.rs).
// It reads `sh_total_atlas`, `sh_atlas_sampler`, `sh_grid`, and
// `sh_depth_moments` (declared at group 4 above) by lexical name. The helper
// drops invalid (in-wall) probes via atlas alpha, applies moment visibility, and
// renormalizes survivors.

// Normal-offset wrapper, mirrored from forward.wgsl's `sample_sh_indirect` but
// with backface rejection OFF (entities are not static surfaces — matches the
// billboard precedent). Biases the lookup toward the lit side by a fraction of a
// cell along the surface normal, derives the grid index / sub-cell fraction
// (clamped to the grid), then defers the corrected 8-corner blend to the shared
// helper with Chebyshev probe-occlusion gated by `sh_grid.probe_occlusion`.
fn sample_sh_indirect(world_pos: vec3<f32>, shading_normal: vec3<f32>, geo_normal: vec3<f32>) -> vec3<f32> {
    if sh_grid.has_sh_volume == 0u {
        return vec3<f32>(0.0);
    }

    const SH_NORMAL_OFFSET_M: f32 = 0.1;
    let offset_world = world_pos + shading_normal * SH_NORMAL_OFFSET_M * sh_grid.cell_size;
    let gdims_u = sh_grid.grid_dimensions;
    let gdims_f = max(vec3<f32>(gdims_u) - vec3<f32>(1.0), vec3<f32>(0.0));
    let cell_coord = (offset_world - sh_grid.grid_origin) /
        max(sh_grid.cell_size, vec3<f32>(1.0e-6));
    let gf = clamp(cell_coord, vec3<f32>(0.0), gdims_f);
    let gi = vec3<u32>(floor(gf));
    let gfrac = fract(gf);

    return sample_sh_indirect_corners_depth_aware(
        gi,
        gfrac,
        offset_world,
        shading_normal,
        geo_normal,
        false,
        sh_grid.probe_occlusion != 0u,
    );
}

@fragment
fn fs_main(in: VertexOutput) -> @location(0) vec4<f32> {
    // SH-lit: sample base color, then multiply by the local baked indirect
    // irradiance read from the SH volume at the skinned world position. No direct
    // light loop yet — that is the dynamic-direct task's additive group (group 2,
    // unallocated). The skinned world normal drives both the octahedral direction
    // lookup and (unused, backface rejection off) the geometric backface test.
    let base_color = textureSample(base_texture, aniso_sampler, in.uv);
    let n = normalize(in.world_normal);
    let indirect = sample_sh_indirect(in.world_position, n, n);
    return vec4<f32>(base_color.rgb * indirect, base_color.a);
}
