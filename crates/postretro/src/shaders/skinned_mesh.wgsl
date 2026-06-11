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
// Lighting: SH-lit indirect + baked static direct. The fragment samples the
// material base-color texture (group 1), multiplies it by the baked indirect
// irradiance (`sample_sh_indirect`), and adds the baked static-direct term
// (`sample_sh_direct`) from the direct atlas at group 4 binding 15. Both SH
// reads use the same grid/sampler from the group-4 superset. The depth-aware
// octahedral helper from `sh_sample.wgsl` is appended to this source at
// pipeline creation (render/mesh_pass.rs), mirroring how forward.wgsl is
// assembled (render/mod.rs `SHADER_SOURCE`). Group 4 is the `mesh_bind_group`
// SUPERSET (shared SH entries + direct atlas at binding 15 +
// `DynamicDirectParams` uniform at binding 16) — NOT the shared `bind_group`
// the forward/billboard/fog passes hold.
//
// Entities follow the BILLBOARD precedent, not forward's static-surface variant:
// `reject_backface = false` (a moving skinned entity is not a static world
// surface) with Chebyshev probe-occlusion on. Baked static direct is computed
// here via `sample_sh_direct`; group 2 now carries the dynamic-direct light
// resources (M10 Task 2) — declared but not yet read; the per-fragment runtime
// dynamic-tier light loop (plan D10) lands in Task 3.
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

// --- Group 2: dynamic direct lighting ----------------------------------------
// Filled by M10 Task 2. Binding map PINNED across both M10 mesh specs (the BGL
// in render/mesh_pass.rs is authoritative): b0 dynamic-light records (the
// renderer's `is_dynamic`-filtered set — the dynamic tier only; static-tier
// direct for movers is the group-4 baked atlas, so no double-count), b1 per-light
// influence volumes, b2 scripted-animation descriptors (forward's group-3 b13
// `scripted_light_descriptors`, SAME buffer), b3 scripted-animation curve samples
// (forward's group-3 b12 `anim_samples`, SAME buffer), b4 the mesh-side params
// uniform. b5–b8 are RESERVED for the shadow-receipt spec — not declared here.
//
// These bindings are DECLARED but not yet READ: the per-fragment dynamic-light
// loop is Task 3. They exist now so the appended shared helpers resolve their
// module-scope names — `curve_eval.wgsl` reads `anim_samples` (b3) and
// `light_eval.wgsl` reads the `AnimationDescriptor` type (declared below) — and
// so the BGL and shader agree. Unused bindings/functions are legal in WGSL, so
// the shader still passes naga validation. `GpuLight` / `AnimationDescriptor`
// mirror forward.wgsl's same-named structs (the underlying buffers are the same).
struct GpuLight {
    position_and_type: vec4<f32>,
    color_and_falloff_model: vec4<f32>,
    direction_and_range: vec4<f32>,
    cone_angles_and_pad: vec4<f32>,
};
@group(2) @binding(0) var<storage, read> lights: array<GpuLight>;
// Per-light influence volume: xyz = sphere center, w = radius.
@group(2) @binding(1) var<storage, read> light_influence: array<vec4<f32>>;

// Per-light scripted-animation descriptor — mirrors forward.wgsl's
// `AnimationDescriptor` (48 B; see render/sh_volume.rs ANIMATION_DESCRIPTOR_SIZE).
// Consumed by the appended `light_eval.wgsl` helpers (e.g.
// `light_eval_animated_direction`) when the Task-3 loop lands.
struct AnimationDescriptor {
    period: f32,
    phase: f32,
    brightness_offset: u32,
    brightness_count: u32,
    base_color: vec3<f32>,
    color_offset: u32,
    color_count: u32,
    is_active: u32,
    direction_offset: u32,
    direction_count: u32,
};
@group(2) @binding(2) var<storage, read> scripted_light_descriptors: array<AnimationDescriptor>;
// Scripted-animation curve samples (packed f32). `curve_eval.wgsl` (appended to
// this source) reads `anim_samples` by lexical name; this declaration satisfies
// that reference. Same buffer forward binds at its group-3 b12.
@group(2) @binding(3) var<storage, read> anim_samples: array<f32>;

// Mesh-side group-2 params uniform: dynamic-light count, the frame's render-clock
// `time` (the SAME value the renderer writes to forward `Uniforms.time` that
// frame, so the scripted curves stay phase-coherent), and `lighting_isolation` —
// the SAME `LightingIsolation` value the renderer uploads to forward
// `Uniforms.lighting_isolation` that frame. The mesh dynamic-direct term
// participates in the forward lighting-isolation debug modes exactly as the world
// dynamic term does: the loop is gated by `use_dynamic` derived from the SAME mode
// set forward uses (see `fs_main`). Mirrors `MeshLightParams` in
// render/mesh_pass.rs. std140-padded to 16 B.
struct MeshLightParams {
    light_count: u32,
    time: f32,
    lighting_isolation: u32,
    _pad: u32,
};
@group(2) @binding(4) var<uniform> mesh_light_params: MeshLightParams;

// --- Group 2 (cont.): shadow receipt (M10 mesh shadow receipt Task 2) ---------
// b5–b8 alias the SAME pool-owned GPU resources the forward pass binds in its
// group 5, via a MESH-SPECIFIC layout (the BGL in render/mesh_pass.rs is
// authoritative; it omits forward's SDF-factor + scene-depth entries the mesh
// must not sample). The appended `shadow_sample.wgsl` references these four
// names (`spot_shadow_depth`, `spot_shadow_compare`, `light_space_matrices`,
// `point_shadow_cube`) by lexical resolution — the SAME binding-agnostic
// composition forward.wgsl uses. DECLARED here but not yet SAMPLED: Task 3's
// per-light visibility term calls `sample_spot_shadow` / `sample_point_shadow`.
// Read-but-unused bindings are legal WGSL, so the shader still validates.
//
// b7 is a UNIFORM (`array<mat4x4<f32>, 96>` = SHADOW_POOL_SIZE) — NOT storage —
// to keep the fragment storage-buffer count at 4 (rendering_pipeline.md §10);
// it stays well under the 16 KiB uniform cap. Matches forward's group-5 b2.
@group(2) @binding(5) var spot_shadow_depth: texture_depth_2d_array;
@group(2) @binding(6) var spot_shadow_compare: sampler_comparison;
struct LightSpaceMatrices {
    m: array<mat4x4<f32>, 96>,
};
@group(2) @binding(7) var<uniform> light_space_matrices: LightSpaceMatrices;
// CUBE_SHADOW_BINDING — on a no-`CUBE_ARRAY_TEXTURES` adapter,
// `render::strip_point_shadow_cube` drops this `// CUBE_SHADOW_BINDING`-tagged
// declaration (and neutralizes `sample_point_shadow`), so the shader matches a
// group-2 BGL that omits b8. Mirrors forward.wgsl's group-5 b5 cube binding.
@group(2) @binding(8) var point_shadow_cube: texture_depth_cube_array;

// --- Group 3: skinned instance data ------------------------------------------
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

// --- Group 4: SH volume superset (baked indirect + baked static direct) ------
// Binds `mesh_bind_group` / `mesh_bind_group_layout` — the SUPERSET of the
// shared SH bind group. It adds direct atlas at binding 15 and the mesh-only
// `DynamicDirectParams` uniform at binding 16 on top of the shared entries
// (b1/b2/b10/b11/b12/b13/b14). Group 3 is locked to instance data; group 2
// stays UNALLOCATED (reserved for the future dynamic-direct light loop, D10).
// The appended `sh_sample.wgsl` helper reads the shared bindings by lexical
// name. The animated-layer storage buffers (b11/b12/b13) live in the layout
// but are not declared here — the mesh path never evaluates animated layers
// (mirrors billboard.wgsl); omitting undeclared bindings is legal in WGSL.
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
// Baked static direct SH atlas (BC6H-at-rest, hardware-decoded to f32). Bound at
// `BIND_SH_DIRECT_ATLAS` (group 4 binding 15) on the mesh group-4 superset by
// render/sh_volume.rs. Same octahedral tile geometry as `sh_total_atlas`, so it
// samples through the shared `sh_sample.wgsl` chain with the same grid/sampler.
@group(4) @binding(15) var sh_direct_atlas: texture_2d<f32>;

// Mesh-only dynamic-direct debug params (binding 16). The mesh path reads a
// trimmed group-0 camera uniform (only `view_proj`), so the scale / isolation /
// has_direct knobs reach it through this dedicated uniform instead of the
// group-0 `Uniforms` tail that billboard.wgsl uses. std140: padded to 16 bytes.
//   scale      — multiplies the baked direct term (0..1).
//   isolation  — 0 = combined (indirect + scale·direct),
//                1 = direct-only (scale·direct), 2 = indirect-only.
//   has_direct — 0 when the baked DIRECT SH section is absent; the direct term
//                is forced to 0 (fall back to indirect-only) with no error.
struct DynamicDirectParams {
    scale: f32,
    isolation: u32,
    has_direct: u32,
    _pad: u32,
};
@group(4) @binding(16) var<uniform> dynamic_direct: DynamicDirectParams;

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

// Baked static direct SH read, the sibling of `sample_sh_indirect` against the
// direct atlas. Same normal-offset bias and grid derivation (so the direct term
// lines up with the indirect one), then defers to the shared-weights direct
// corner blend. Backface rejection stays OFF (entities are not static surfaces)
// and Chebyshev stays ON, reading the shared `sh_depth_moments`.
fn sample_sh_direct(world_pos: vec3<f32>, shading_normal: vec3<f32>, geo_normal: vec3<f32>) -> vec3<f32> {
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

    return sample_sh_direct_corners_depth_aware(
        sh_direct_atlas,
        gi,
        gfrac,
        offset_world,
        shading_normal,
        geo_normal,
        false,
        sh_grid.probe_occlusion != 0u,
    );
}

// Runtime dynamic-direct light loop — mirrors forward.wgsl's dynamic-tier loop
// (the b0 buffer is the renderer's `is_dynamic`-filtered set, so static lights
// cannot leak in), but DIFFUSE-ONLY: Lambert against the interpolated skinned
// normal `n`, no specular and no normal-map perturbation (the mesh path has
// neither — see rendering_pipeline.md §9). Each per-light term is attenuated by
// the light's shadow map: the spot slot from `cone_angles_and_pad.z` indexes
// `light_space_matrices` for `sample_spot_shadow`, the cube slot from
// `cone_angles_and_pad.w` drives `sample_point_shadow` — sentinel 0xFFFFFFFF on
// either ⇒ unshadowed (×1.0). Slot logic is identical to forward.wgsl's dynamic
// loop; the shadow factor folds into the per-light attenuation.
//
// BIAS / NORMAL-OFFSET TUNING SEAM (M10 mesh shadow receipt) — read before
// touching self-shadow acne on skinned entities:
//   * This loop — where `sample_spot_shadow` / `sample_point_shadow` are called
//     below — is the SOLE SANCTIONED place to add or tune a mesh-receiver
//     bias / normal-offset. Do NOT edit the bias inside the shared
//     `shadow_sample.wgsl`: the forward and fog passes share those helpers, so a
//     change there alters world shadows (peter-panning risk) and breaks forward's
//     no-behavior-change AC. Keep mesh-only acne fixes here.
//   * PREFERRED remedy if acne appears on curved skinned surfaces: a sample-site
//     NORMAL-OFFSET — push `world_pos` along the interpolated normal `n` by a
//     small world-space (normal-scaled) amount before passing it to the sampler,
//     rather than raising the shared depth bias. The entity's own depth is already
//     in the maps (occluders render via `record_skinned_depth`), so the receiver
//     offset is the cleaner lever.
//   * OPEN QUESTION (HUMAN CHECKPOINT — not resolved in-tree): the exact
//     normal-offset / bias VALUES. They require a human visual check of self-
//     shadow acne on dev skinned models at gameplay distance under BOTH spot- and
//     point-shadowed lights. No value is invented here — the call sites below
//     sample at the un-offset `world_pos` today (byte-identical to the
//     pre-tuning behavior); a human introduces the offset constant here after
//     judging acne, without touching the shared snippet.
//
// `use_dynamic` is the forward lighting-isolation gate (computed in `fs_main`
// from `mesh_light_params.lighting_isolation`, mirroring forward.wgsl). When the
// active mode excludes the dynamic term, the loop bound is forced to 0 — the SAME
// `select(0u, light_count, use_dynamic)` clamp forward applies — so the term
// contributes nothing. With `light_count == 0` (or the gate off) the loop returns
// zero and the composition reduces to indirect + baked direct; the accumulator
// starts at zero, so a zero-trip loop adds nothing.
fn accumulate_dynamic_direct(world_pos: vec3<f32>, n: vec3<f32>, use_dynamic: bool) -> vec3<f32> {
    var total = vec3<f32>(0.0);
    let light_count = select(0u, mesh_light_params.light_count, use_dynamic);
    for (var i: u32 = 0u; i < light_count; i = i + 1u) {
        // Influence-volume early-out: pure optimization — no pixel change.
        let influence = light_influence[i];
        let inf_radius = influence.w;
        if inf_radius <= 1.0e30 {
            let d = world_pos - influence.xyz;
            if dot(d, d) > inf_radius * inf_radius {
                continue;
            }
        }

        let light = lights[i];
        let light_type = bitcast<u32>(light.position_and_type.w);
        let falloff_model = bitcast<u32>(light.color_and_falloff_model.w);

        // Scripted per-light animation. `is_active == 0` keeps the static
        // GpuLight color/aim; active descriptors override from Catmull-Rom
        // curves on the shared anim_samples buffer. `mesh_light_params.time` is
        // the same frame time forward uses, so the curves stay phase-coherent.
        let scripted_desc = scripted_light_descriptors[i];
        var effective_color = light.color_and_falloff_model.xyz;
        var effective_aim = light.direction_and_range.xyz;
        if scripted_desc.is_active != 0u {
            let cycle_t = fract(mesh_light_params.time / max(scripted_desc.period, 0.0001) + scripted_desc.phase);
            // Catmull-Rom overshoot can dip below zero; clamp so an animated
            // light never emits negative, sign-flipped light.
            if scripted_desc.color_count > 0u {
                let unit_sample = max(
                    sample_color_catmull_rom(
                        scripted_desc.color_offset,
                        scripted_desc.color_count,
                        cycle_t,
                        scripted_desc.base_color,
                    ),
                    vec3<f32>(0.0),
                );
                let intensity = light_eval_scripted_intensity_scalar(
                    light.color_and_falloff_model.xyz,
                    scripted_desc.base_color,
                );
                let brightness = max(
                    sample_curve_catmull_rom(
                        scripted_desc.brightness_offset,
                        scripted_desc.brightness_count,
                        cycle_t,
                    ),
                    0.0,
                );
                effective_color = unit_sample * intensity * brightness;
            } else if scripted_desc.brightness_count > 0u {
                let brightness = max(
                    sample_curve_catmull_rom(
                        scripted_desc.brightness_offset,
                        scripted_desc.brightness_count,
                        cycle_t,
                    ),
                    0.0,
                );
                effective_color = light.color_and_falloff_model.xyz * brightness;
            }
            if light_type == 1u && scripted_desc.direction_count > 0u {
                effective_aim = light_eval_animated_direction(scripted_desc, cycle_t, effective_aim);
            }
        }

        var L: vec3<f32>;
        var attenuation: f32;

        switch light_type {
            case 0u: {
                let to_light = light.position_and_type.xyz - world_pos;
                let dist = length(to_light);
                L = to_light / max(dist, 0.0001);
                attenuation = light_eval_falloff(dist, light.direction_and_range.w, falloff_model);

                // Dynamic point-light cube shadow — mirrors forward.wgsl's case
                // 0u. The cube slot rides in `cone_angles_and_pad.w` (sentinel
                // 0xFFFFFFFF = no slot, i.e. the light was not ranked into the
                // cube pool). A point light without a slot keeps its unshadowed
                // attenuation. On a no-`CUBE_ARRAY_TEXTURES` adapter the stripped
                // `sample_point_shadow` returns 1.0, so this call site validates
                // and lights the mesh unshadowed. Folded into `attenuation` so the
                // dynamic point term is shadowed exactly once (no lightmap touched).
                let cube_slot = bitcast<u32>(light.cone_angles_and_pad.w);
                if cube_slot != 0xFFFFFFFFu {
                    let shadow = sample_point_shadow(
                        cube_slot,
                        light.position_and_type.xyz,
                        world_pos,
                        light.direction_and_range.w
                    );
                    attenuation = attenuation * shadow;
                }
            }
            case 1u: {
                let to_light = light.position_and_type.xyz - world_pos;
                let dist = length(to_light);
                L = to_light / max(dist, 0.0001);
                let dist_falloff = light_eval_falloff(dist, light.direction_and_range.w, falloff_model);
                let cone = light_eval_cone_attenuation(
                    L,
                    effective_aim,
                    light.cone_angles_and_pad.x,
                    light.cone_angles_and_pad.y,
                );
                attenuation = dist_falloff * cone;

                // Dynamic spot shadow — mirrors forward.wgsl's case 1u. The spot
                // slot rides in `cone_angles_and_pad.z` (sentinel 0xFFFFFFFF = no
                // slot ⇒ unshadowed). A valid slot indexes `light_space_matrices`
                // and folds the PCF visibility into `attenuation`.
                let slot_index = bitcast<u32>(light.cone_angles_and_pad.z);
                if slot_index != 0xFFFFFFFFu {
                    let light_proj = light_space_matrices.m[slot_index];
                    let shadow = sample_spot_shadow(slot_index, world_pos, light_proj);
                    attenuation = attenuation * shadow;
                }
            }
            default: {
                // Directional light (case 2u and any unknown discriminant).
                L = -effective_aim;
                attenuation = 1.0;
            }
        }

        let NdotL = max(dot(n, L), 0.0);
        total = total + effective_color * attenuation * NdotL;
    }
    return total;
}

@fragment
fn fs_main(in: VertexOutput) -> @location(0) vec4<f32> {
    // SH-lit: sample base color, multiply by baked indirect irradiance, then add
    // the baked static-direct term (group 4 binding 15; gated by `has_direct`).
    // The future dynamic-direct light loop (D10, group 2) is not yet built.
    // The skinned world normal drives the octahedral direction lookup and
    // (unused, backface rejection off) the geometric backface test.
    let base_color = textureSample(base_texture, aniso_sampler, in.uv);
    let n = normalize(in.world_normal);
    let indirect = sample_sh_indirect(in.world_position, n, n);
    // Baked static direct SH term, sampled with the same normal/grid as the
    // indirect read (corners-depth-aware, backface rejection off). The direct
    // term is gated off (0) when the baked DIRECT section is absent
    // (`has_direct == 0`) — the absent-section fallback to indirect-only.
    var direct = vec3<f32>(0.0);
    if dynamic_direct.has_direct != 0u {
        direct = dynamic_direct.scale * sample_sh_direct(in.world_position, n, n);
    }
    // Lighting-isolation gate for the runtime dynamic-direct term — the SAME
    // `LightingIsolation` mode set forward.wgsl uses to gate its world dynamic
    // term (`use_dynamic = iso 0|1|2|8`: Normal, NoLightmap, DirectOnly,
    // DynamicOnly). NOT a new boolean: `mesh_light_params.lighting_isolation`
    // carries the identical value the renderer writes to forward
    // `Uniforms.lighting_isolation` that frame, so the mesh dynamic term appears
    // in exactly the debug modes the world dynamic term does. This is ORTHOGONAL
    // to the group-4 `dynamic_direct.isolation` gate below (baked direct-vs-
    // indirect isolation) — the two compose multiplicatively: the dynamic term
    // renders only when its `use_dynamic` gate passes, and the baked SH terms are
    // selected independently by `dynamic_direct.isolation`.
    let iso = mesh_light_params.lighting_isolation;
    let use_dynamic = (iso == 0u) || (iso == 1u) || (iso == 2u) || (iso == 8u);

    // Runtime dynamic-direct term, summed alongside the baked indirect + direct
    // terms (forward adds dynamic into the composition; it does not re-weight).
    // Diffuse-only against the interpolated skinned normal `n`. Gated by
    // `use_dynamic` (forced to zero outside the dynamic-visible modes).
    let dynamic = accumulate_dynamic_direct(in.world_position, n, use_dynamic);

    // Baked-SH dynamic-direct isolation (debug instrument) gates only the baked SH
    // terms — UNTOUCHED by the runtime gate above; the two multiply.
    //   0 = combined    → indirect + scale·direct
    //   1 = direct-only  → scale·direct
    //   2 = indirect-only → indirect
    var lighting = indirect + direct;
    if dynamic_direct.isolation == 1u {
        lighting = direct;
    } else if dynamic_direct.isolation == 2u {
        lighting = indirect;
    }
    lighting = lighting + dynamic;
    return vec4<f32>(base_color.rgb * lighting, base_color.a);
}
