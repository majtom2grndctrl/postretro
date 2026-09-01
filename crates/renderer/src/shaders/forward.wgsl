// Main forward pass — direct lighting via a flat per-fragment light loop
// plus a scalar ambient floor, with baked octahedral-atlas irradiance indirect.
// See: context/lib/rendering_pipeline.md §4

struct Uniforms {
    view_proj: mat4x4<f32>,
    camera_position: vec3<f32>,
    ambient_floor: f32,
    light_count: u32,
    // Elapsed seconds since renderer start. Consumed by SH animated-layer
    // evaluation; wrapping is handled per-light via fract().
    time: f32,
    // `LightTermMask` from the dev-tools diagnostics checkboxes. Bits 0..=6
    // independently gate the lighting terms; bit 7 is reserved/emissive and
    // intentionally unwired.
    light_term_mask: u32,
    // Per-frame multiplier on the SH indirect term. 1.0 preserves baked
    // intensity; lower values suppress SH fill on static surfaces to keep
    // lightmap shadow contrast.
    indirect_scale: f32,
    // Gates whether the half-res SDF visibility target is sampled at all. See
    // `SDF_SHADOW_FLAG_*` in render/mod.rs:
    //   bit 0 — an SDF atlas is loaded, so the half-res factor target holds
    //           valid per-light visibility slices. When clear (legacy PRL / no
    //           SDF atlas) the forward skips the upsample and the per-light
    //           visibility defaults to fully lit.
    // The four RGBA channels are the K = 4 per-light slices, read via
    // `slice_for_visibility`.
    sdf_shadow_flags: u32,
    // `SdfShadowMode` debug selector:
    //   0 = On        — apply SDF shadow factors normally.
    //   1 = Off       — force all per-light SDF visibility to 1.0.
    //                   Shadow-map (enemy) shadows are unaffected.
    //   2 = Visualize — replace the final shaded color with a grayscale view
    //                   of the first per-light visibility slice (R = slot 0).
    //   5 = Visualize shadowmask union subtraction magnitude.
    //   6 = Visualize raw promoted-light pool visibility (darkest wins).
    sdf_shadow_mode: u32,
    // Dev toggle (non-zero ⇒ force per-light SDF visibility to 1.0). Used by
    // the "no double-count" visual AC: with every sdf light's visibility
    // forced fully lit, the per-light diffuse sum must reproduce the
    // pre-change render with no brightening (disjoint sets guarantee the term
    // is purely additive). Set via the Diagnostics panel checkbox.
    sdf_force_visibility_one: u32,
    // --- dynamic-direct tail (baked-static-direct-sh Task 6) ---
    // These belong to the DYNAMIC (entity / billboard) path and are NOT read by
    // the forward fragment. They are declared here only to keep the shared
    // group-0 `Uniforms` byte layout in lockstep (the 4-way contract: Rust
    // writer + forward.wgsl + billboard.wgsl + wireframe.wgsl). The first
    // field repurposes the former `_sdf_pad1` slot; the rest land in a fresh
    // 16-byte row so the struct stride is exactly 128 — wgpu rejects the
    // pipeline if the CPU-side `UNIFORM_SIZE` and WGSL-derived stride drift.
    dynamic_direct_scale: f32,
    // Level-load-fixed billboard scatter mode: 0 unavailable, 1 static base,
    // 2 composed animated. Forward does not read it; the field preserves the
    // shared 128-byte ABI.
    has_scatter: u32,
    has_direct: u32,
    total_light_count: u32,
    // Dev toggle: force static-light shadowmask visibility to 1.0 for the
    // manual pre-change A/B. It affects only the static world specular path.
    spec_shadowmask_force_one: u32,
};

// Four vec4<f32> slots — see postretro/src/lighting/mod.rs for field semantics.
struct GpuLight {
    position_and_type: vec4<f32>,
    color_and_falloff_model: vec4<f32>,
    direction_and_range: vec4<f32>,
    cone_angles_and_pad: vec4<f32>,
};

@group(0) @binding(0) var<uniform> uniforms: Uniforms;

@group(1) @binding(0) var base_texture: texture_2d<f32>;
// Per-material emissive color. `Rgba8UnormSrgb` decodes to linear through the
// hardware texture path, like base_texture; the black placeholder is a no-op.
@group(1) @binding(1) var emissive_texture: texture_2d<f32>;
// Per-material specular texture (R8Unorm sampled as .r). 1×1 black when the
// diffuse's `_s.png` sibling is absent — zeros `spec_int` without any
// shader branching. See context/lib/resource_management.md §4.1.
@group(1) @binding(2) var spec_texture: texture_2d<f32>;

struct MaterialUniform {
    // Blinn-Phong specular exponent; constant per-material variant.
    // Padded to 16 B for uniform-buffer alignment.
    shininess: f32,
    // Prefix-driven static multiplier for the emissive texture.
    emissive_strength: f32,
    _pad: vec2<f32>,
};
@group(1) @binding(3) var<uniform> material: MaterialUniform;
// Per-material tangent-space normal map. Sampled with `aniso_sampler`. The
// neutral placeholder is (127, 127, 255, 255) which decodes to ~(0, 0, 1)
// in tangent space, so surfaces with no `_n.png` sibling render identically
// to the mesh-normal path. See context/lib/resource_management.md §4.3.
@group(1) @binding(4) var t_normal: texture_2d<f32>;
// Linear + hardware-anisotropic sampler for world and mover materials. The
// Post Retro path samples through this so hardware aniso kills grazing-angle
// shimmer while in-shader texel-grid reconstruction keeps texels crisp up
// close. The BGL and world/mover material bind groups wire it from
// `Renderer::mip_count_aniso_samplers`; see `render/mod.rs`'s group-1 BGL
@group(1) @binding(5) var aniso_sampler: sampler;

@group(2) @binding(0) var<storage, read> lights: array<GpuLight>;
// Per-light influence volume: xyz = sphere center, w = radius.
@group(2) @binding(1) var<storage, read> light_influence: array<vec4<f32>>;

// Static light buffer: specular + per-light SDF diffuse for sdf-tagged lights.
// Four vec4 slots (64 B stride); see crates/lighting/src/spec_buffer.rs
// for the CPU-side layout.
struct SpecLight {
    position_and_range: vec4<f32>, // xyz = position, w = falloff_range
    color_and_pad:      vec4<f32>, // xyz = color × intensity, w = sdf flag (>0.5 ⇒ _shadow_type sdf)
    cone_dir_and_type:  vec4<f32>, // xyz = normalized aim, w = light type (1.0 ⇒ spot)
    cone_cos:           vec4<f32>, // x = cos(inner), y = cos(outer), z = baked shadowmask channel (0..3) or 4.0 (none); non-spot carries 1/-1 (full bright)
};
@group(2) @binding(2) var<storage, read> spec_lights: array<SpecLight>;

// Chunk grid metadata — uniform buffer with `has_chunk_grid` sentinel.
// 0 = no chunk list present (fallback: iterate full spec buffer).
struct ChunkGridInfo {
    grid_origin: vec3<f32>,
    cell_size: f32,
    dims: vec3<u32>,
    has_chunk_grid: u32,
};
@group(2) @binding(3) var<uniform> chunk_grid: ChunkGridInfo;
// Per-chunk offset table: (offset, count) pair per chunk, linearised by
// `z * dims.x * dims.y + y * dims.x + x`.
@group(2) @binding(4) var<storage, read> chunk_offsets: array<vec2<u32>>;
// Flat index list (u32 indices into spec_lights).
@group(2) @binding(5) var<storage, read> chunk_indices: array<u32>;

// Group 3 — octahedral irradiance atlas. The sampled total atlas carries
// composed indirect irradiance, with alpha as the baked per-probe validity bit.
// A 3D texture (@binding(14) sh_depth_moments) carries per-probe depth moments
// (R = mean, G = mean²) for the depth-aware visibility term.
// When `grid.has_sh_volume` is 0 the bindings point at dummy textures and
// the shader skips SH sampling. See crates/renderer/src/render/sh_volume.rs.
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
    tiles_per_layer: u32,
    atlas_layer_count: u32,
    _pad3: u32,
};

// Per-light animation descriptor — matches ANIMATION_DESCRIPTOR_SIZE (48 B)
// in crates/renderer/src/render/sh_volume.rs. Field order diverges from the spec
// prose to hit exactly 48 bytes: with the spec's original order, color_count
// ends at byte 44 and trailing vec2<f32> padding (AlignOf=8) would be pushed
// to 48, making the struct 56 B and stride 64. Instead we pack four scalars
// after base_color so color_count ends at 36; `is_active` fills the 4-byte
// implicit gap at 36..40 and the direction offsets occupy 40..48 for a 48-byte
// stride. The trailing two u32s carry the direction-channel offset + count;
// `direction_count == 0` means the spot light keeps its static `cone_direction`.
// `is_active` is toggled at runtime by the scripting layer — inactive lights
// contribute nothing to either the SH volume or the compose pass. Named
// `is_active` rather than `active` because WGSL reserves the latter as a keyword.
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

@group(3) @binding(1) var sh_total_atlas: texture_2d_array<f32>;
@group(3) @binding(2) var sh_atlas_sampler: sampler;
@group(3) @binding(10) var<uniform> sh_grid: ShGridInfo;

// Animation buffers. Always bound; anim_descriptors and anim_samples are
// consumed by the animated lightmap compose pass (group 4 binding 3) and
// also exposed here so the bind group layout is stable across passes.
@group(3) @binding(11) var<storage, read> anim_descriptors: array<AnimationDescriptor>;
@group(3) @binding(12) var<storage, read> anim_samples: array<f32>;

// One AnimationDescriptor per dynamic-direct light, indexed by the forward
// light-loop counter `i`. `is_active == 0` → static GpuLight.color used unchanged.
// Uploaded by `LightBridge::update → Renderer::upload_bridge_descriptors`.
@group(3) @binding(13) var<storage, read> scripted_light_descriptors: array<AnimationDescriptor>;
@group(3) @binding(14) var sh_depth_moments: texture_3d<f32>;

// Group 4 — baked directional lightmap (static direct lighting).
// See context/lib/rendering_pipeline.md §4.
@group(4) @binding(0) var lightmap_irradiance: texture_2d_array<f32>;
@group(4) @binding(1) var lightmap_direction: texture_2d_array<f32>;
// Non-filtering (Nearest) sampler — used only for the octahedral direction
// texture (binding 1): linear interpolation of octahedral unit vectors does
// not commute with slerp.
@group(4) @binding(2) var lightmap_sampler: sampler;
// Animated-light contribution atlas (Rgba16Float). Composed each frame by
// the compute pre-pass in `animated_lightmap.rs` from per-animated-light
// baked weight maps + runtime descriptor curves. `.rgb` carries pre-shaded
// irradiance. Array slices are dense animated slots, not static layers: the
// binding-7 lookup maps each static lightmap layer to its animated slot.
@group(4) @binding(3) var animated_lm_atlas: texture_2d_array<f32>;
// Filtering (Linear) sampler — used for the irradiance + animated atlases so
// baked penumbra ramps read as continuous gradients under magnification.
// `Rgba16Float` linear-filterability is a hard runtime requirement, checked at
// init (see context/lib/rendering_pipeline.md §4).
@group(4) @binding(4) var lightmap_filtering_sampler: sampler;
// Animated dominant-direction atlas (Rgba8Unorm, octahedral in .rg — decoded by
// `decode_lightmap_direction`, shared with the static direction atlas — and a
// coverage flag in .a). Composed each frame alongside the animated irradiance
// atlas. Read through the nearest sampler at binding 2 — like the static
// direction atlas, oct directions must not be linearly interpolated.
@group(4) @binding(5) var animated_lm_direction: texture_2d_array<f32>;
@group(4) @binding(6) var shadowmask_atlas: texture_2d_array<f32>;

// Four static layers per vec4 avoid uniform-space's 16-byte scalar-array
// stride. This 64-vec4 layout must match `STATIC_LIGHTMAP_LAYER_CAP` (256) in
// `lighting/lightmap.rs`. An absent layer holds INVALID_SLOT (0xFFFF_FFFF).
struct AnimatedLightmapSlots {
    static_layer_to_animated_slot: array<vec4<u32>, 64>,
};
@group(4) @binding(7) var<uniform> animated_lightmap_slots: AnimatedLightmapSlots;

// Sample the irradiance atlas with hardware bilinear filtering through the
// linear sampler at binding 4. `layer` selects the atlas array slice.
fn sample_lightmap_irradiance(uv: vec2<f32>, layer: u32) -> vec3<f32> {
    return textureSample(lightmap_irradiance, lightmap_filtering_sampler, uv, i32(layer)).rgb;
}

// Same for the animated-light contribution atlas.
fn sample_lightmap_animated(uv: vec2<f32>, slot: u32) -> vec3<f32> {
    return textureSample(animated_lm_atlas, lightmap_filtering_sampler, uv, i32(slot)).rgb;
}

fn animated_slot_for_static_layer(static_layer: u32) -> u32 {
    const INVALID_SLOT: u32 = 0xffffffffu;
    if static_layer >= 256u {
        return INVALID_SLOT;
    }
    let packed_slots = animated_lightmap_slots.static_layer_to_animated_slot[static_layer / 4u];
    return packed_slots[static_layer % 4u];
}

// Group 5 — dynamic spot light shadow maps.
// See context/lib/rendering_pipeline.md §4.
@group(5) @binding(0) var spot_shadow_depth: texture_depth_2d_array;
@group(5) @binding(1) var spot_shadow_compare: sampler_comparison;
// Uniform (not storage) so we stay under `max_storage_buffers_per_shader_stage`
// (default limit 8 on some adapters — wgpu refuses the pipeline if we add
// a 9th). The array length MUST match `SHADOW_POOL_SIZE` in
// `lighting/spot_shadow.rs` (pinned by `light_space_matrices_array_len_matches_pool`);
// 96 × mat4x4<f32> is 6144 bytes, well under the 16 KiB uniform cap.
struct LightSpaceMatrices {
    m: array<mat4x4<f32>, 96>,
};
@group(5) @binding(2) var<uniform> light_space_matrices: LightSpaceMatrices;
// SDF static-occluder shadow factor: half-res Rgba8Unorm. The four channels are
// the K = 4 per-light SDF visibility slices (K-selection slots 0..3):
//   R = slot 0   G = slot 1   B = slot 2   A = slot 3.
// Bilaterally upsampled per-channel inside this shader. Read via
// `textureLoad` — non-filterable on most adapters, and the bilateral filter
// re-derives its own weights so a hardware sampler buys nothing.
@group(5) @binding(3) var sdf_shadow_factor: texture_2d<f32>;
// Full-res scene depth (Depth32Float). Sampled via `textureLoad` to drive
// the depth-aware weight of each 2×2 bilateral tap so the upsample
// preserves the hard shadow edges that match the depth discontinuities of
// the geometry. The forward render pass binds the depth attachment as
// read-only (`depth_ops: None`) so this binding is legal alongside it.
@group(5) @binding(4) var sdf_shadow_depth: texture_depth_2d;
// Dynamic POINT-light cube-array shadow depth (Depth32Float, `CUBE_COUNT × 6`
// layers, `slot*6 + face`). Sampled by world-direction vector via
// `textureSampleCompareLevel` with `spot_shadow_compare` (the same `Less`
// comparison sampler — the cube path reuses it). Bound but NOT sampled by the
// fog volume pass (shared group-5 BGL stays layout-identical). See
// `lighting/cube_shadow.rs` and context/lib/rendering_pipeline.md §7.1.
//
// The `// CUBE_SHADOW_BINDING` tag marks this line for the no-cube shader
// variant: on an adapter without `CUBE_ARRAY_TEXTURES` the renderer strips this
// declaration (and neutralizes `sample_point_shadow` via the body markers below)
// before pipeline creation, so the shared group-5 BGL can omit binding 5. See
// `render::strip_point_shadow_cube`.
@group(5) @binding(5) var point_shadow_cube: texture_depth_cube_array; // CUBE_SHADOW_BINDING

struct VertexInput {
    @location(0) position: vec3<f32>,
    @location(1) base_uv: vec2<f32>,
    @location(2) normal_oct: vec2<u32>,
    @location(3) tangent_packed: vec2<u32>,
    @location(4) lightmap_uv_packed: vec2<u32>,
    @location(5) lightmap_layer: u32,
};

struct VertexOutput {
    // `@invariant` keeps clip-space Z bit-exact with depth_prepass.wgsl so
    // the `depth_compare: Equal` test doesn't miss fragments due to FMA
    // reassociation drift on some GPUs. See rendering_pipeline.md §7.2.
    @invariant @builtin(position) clip_position: vec4<f32>,
    @location(0) uv: vec2<f32>,
    @location(1) world_normal: vec3<f32>,
    @location(2) world_tangent: vec3<f32>,
    @location(3) bitangent_sign: f32,
    @location(4) world_position: vec3<f32>,
    @location(5) lightmap_uv: vec2<f32>,
    @location(6) @interpolate(flat) lightmap_layer: u32,
};

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

@vertex
fn vs_main(in: VertexInput) -> VertexOutput {
    var out: VertexOutput;
    out.clip_position = uniforms.view_proj * vec4<f32>(in.position, 1.0);
    out.uv = in.base_uv;
    out.world_position = in.position;

    out.world_normal = oct_decode(in.normal_oct);

    // Strip sign bit from v-component, remap 15-bit to 16-bit range.
    let sign_bit = in.tangent_packed.y & 0x8000u;
    let v_15bit = in.tangent_packed.y & 0x7FFFu;
    let v_16bit = v_15bit * 65535u / 32767u;
    out.world_tangent = oct_decode(vec2<u32>(in.tangent_packed.x, v_16bit));
    out.bitangent_sign = select(-1.0, 1.0, sign_bit != 0u);

    out.lightmap_uv = vec2<f32>(
        f32(in.lightmap_uv_packed.x) / 65535.0,
        f32(in.lightmap_uv_packed.y) / 65535.0,
    );
    out.lightmap_layer = in.lightmap_layer;

    return out;
}

// The baker stores octahedral-encoded directions in the rg channels of an
// Rgba8Unorm texture; sampling returns 0..1, remapped to -1..1 here.
fn decode_lightmap_direction(enc: vec4<f32>) -> vec3<f32> {
    let ox = enc.r * 2.0 - 1.0;
    let oy = enc.g * 2.0 - 1.0;
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

// Cone falloff from pre-baked cos cutoffs (static `SpecLight` path). Non-spot
// lights pack cos_inner = 1, cos_outer = -1 so this returns 1.0 everywhere.
fn cone_attenuation_cos(L: vec3<f32>, aim: vec3<f32>, cos_inner: f32, cos_outer: f32) -> f32 {
    let cos_angle = dot(-L, aim);
    return smoothstep(cos_outer, cos_inner, cos_angle);
}

// The runtime shadow-map samplers — `sample_spot_shadow` (spot 2D-array PCF) and
// `sample_point_shadow` (point cube-array PCF), plus their bias/resolution
// constants (`SPOT_SHADOW_PCF_RADIUS`, `CUBE_NEAR_CLIP`, `CUBE_FACE_RESOLUTION`,
// `POINT_SHADOW_DEPTH_BIAS`) and the `cube_face_ndc_depth` reconstruction — live
// in `shadow_sample.wgsl`, concatenated after this source at pipeline-build time
// (render/mod.rs `SHADER_SOURCE`). The snippet declares no bindings: it reads the
// group-5 `spot_shadow_depth`, `spot_shadow_compare`,
// `light_space_matrices`, and `point_shadow_cube` declared above by lexical name.
// The no-cube body markers around `sample_point_shadow`'s body travel WITH the
// moved body into the snippet, so `strip_point_shadow_cube` still neutralizes it
// in the composed source on no-`CUBE_ARRAY_TEXTURES` adapters; the
// `// CUBE_SHADOW_BINDING` binding declaration stays here (a binding, not a
// helper). (This comment deliberately avoids the literal body-marker tokens so
// the strip's post-condition — no marker survives — holds on the composed
// source.)

// The depth-aware octahedral irradiance sampler lives in `sh_sample.wgsl`,
// concatenated after this source at pipeline-build time (render/mod.rs
// `SHADER_SOURCE`). It reads the composed atlas, filtering sampler, depth
// moments, and grid metadata declared above by lexical name. The helper drops
// invalid (in-wall) probes via atlas alpha, downweights backfacing probes,
// applies moment visibility, and renormalizes survivors.

// Normal-offset wrapper. Biases the lookup toward the lit side and derives the
// grid index / sub-cell fraction, then defers the corrected 8-corner blend to
// the shared helper with backface rejection enabled (forward-only). The
// geometric mesh normal keys the backface test; the (possibly normal-mapped)
// shading normal drives the octahedral direction lookup.
fn sample_sh_indirect(world_pos: vec3<f32>, shading_normal: vec3<f32>, geo_normal: vec3<f32>) -> vec3<f32> {
    if sh_grid.has_sh_volume == 0u {
        return vec3<f32>(0.0);
    }

    // Bias the lookup toward the lit side by offsetting along the surface
    // normal. Reduces SH bleed across thin walls.
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
        true,
        sh_grid.probe_occlusion != 0u,
    );
}

// Post Retro sample. Reconstructs the texel grid in UV space — warping the
// sample point toward the nearest texel center and antialiasing only the seam
// between texels (the `fwidth(uv_tex)`-wide transition band) — then samples
// through the hardware-anisotropic sampler. Keeps texels crisp up close while
// the linear+aniso sampler antialiases seams and kills grazing-angle shimmer.
//
// Reconstruction is per-slot because slots (diffuse / normal / specular) can
// differ in resolution, so `dims` must come from the texture being sampled.
//
// CRITICAL: the warped `uv_recon` only shifts the sample point; the ORIGINAL
// `ddx`/`ddy` are passed to textureSampleGrad so mip selection and the
// hardware-aniso footprint track the true screen-space pixel footprint. Taking
// derivatives of the warped UV instead would collapse the footprint at seams
// and break mip/aniso selection.
fn sample_post_retro(tex: texture_2d<f32>, samp: sampler, uv: vec2<f32>,
                     ddx: vec2<f32>, ddy: vec2<f32>) -> vec4<f32> {
    let dims = vec2<f32>(textureDimensions(tex, 0));
    let uv_tex = uv * dims;
    let seam = floor(uv_tex + 0.5);
    // Floor the seam-width divisor: a constant-UV fragment (edge-on face,
    // degenerate UV chart, vanishing derivatives) gives fwidth == 0, and
    // clamp() does not reliably sanitize the resulting NaN/Inf in WGSL.
    let seam_width = max(fwidth(uv_tex), vec2<f32>(1.0e-6));
    let aa = clamp((uv_tex - seam) / seam_width, vec2(-0.5), vec2(0.5));
    let uv_recon = (seam + aa) / dims;
    return textureSampleGrad(tex, samp, uv_recon, ddx, ddy);
}

// Per-slot diffuse/specular dispatch. Samples through the hardware-anisotropic
// sampler with the in-shader texel-grid reconstruction in `sample_post_retro`.
fn sample_color(tex: texture_2d<f32>, uv: vec2<f32>, ddx: vec2<f32>, ddy: vec2<f32>) -> vec4<f32> {
    return sample_post_retro(tex, aniso_sampler, uv, ddx, ddy);
}

// Depth-aware 2×2 bilateral upsample of the half-res SDF shadow factor at
// the current fragment. Returns the per-channel sampled factor: R/G/B/A carry
// the K=4 per-light SDF visibility slices (slice i → channel i, matching
// `sdf_shadow.wgsl`). `slice_for_visibility` maps a selection slot to its channel.
// Re-derived locally — the fog_composite upsample was reverted in commit
// f50314d for perf; see
// `context/plans/done/sdf-static-occluder-shadows/research.md`.
//
// Approach:
//   1. Map the fragment's pixel coord (`frag_pos`) into half-res space. The
//      4 nearest half-res taps are the integer neighbours of the projected
//      half-res coordinate.
//   2. Each tap contributes with the standard bilinear weight (from the
//      sub-pixel fraction) times an `exp(-|Δdepth|/sigma)` depth weight.
//      The depth at a tap is the full-res depth at the tap's half-res
//      center mapped back to full-res — same lookup the SDF pass used when
//      it wrote the factor, so the bilateral preserves true scene edges.
//   3. Renormalize by the summed weights; degenerate (all-zero) cases fall
//      back to the nearest-tap value so the multiply stays sane in tiny
//      surfaces where every weight collapses.
//
// Why `textureLoad` rather than a hardware bilinear sampler: the half-res
// target is `Rgba8Unorm` and `sdf_shadow_depth` is `Depth32Float`; both are
// typed as non-filterable in the group-5 BGL so a sampler would buy nothing
// here, and the bilateral weights are computed explicitly anyway.
fn upsample_shadow_factor(frag_xy: vec2<f32>, frag_depth: f32) -> vec4<f32> {
    let depth_dims_u = textureDimensions(sdf_shadow_depth);
    let depth_dims = vec2<f32>(depth_dims_u);
    let half_dims_u = textureDimensions(sdf_shadow_factor);
    let half_dims = vec2<f32>(half_dims_u);

    // Full-res → half-res projection. The SDF pass used `(half_xy + 0.5) *
    // (depth/half)` to sample the depth texture; invert that here so each
    // full-res fragment finds its 2×2 half-res neighbours.
    let half_uv = (frag_xy / depth_dims) * half_dims;
    let h_floor = floor(half_uv - 0.5);
    let frac = clamp(half_uv - 0.5 - h_floor, vec2<f32>(0.0), vec2<f32>(1.0));

    // The 4 half-res taps. Clamp to the texture bounds so an edge fragment
    // duplicates the boundary tap rather than wrapping.
    let h_max = vec2<f32>(half_dims) - vec2<f32>(1.0);
    let h00 = vec2<i32>(clamp(h_floor, vec2<f32>(0.0), h_max));
    let h10 = vec2<i32>(clamp(h_floor + vec2<f32>(1.0, 0.0), vec2<f32>(0.0), h_max));
    let h01 = vec2<i32>(clamp(h_floor + vec2<f32>(0.0, 1.0), vec2<f32>(0.0), h_max));
    let h11 = vec2<i32>(clamp(h_floor + vec2<f32>(1.0, 1.0), vec2<f32>(0.0), h_max));

    let s00 = textureLoad(sdf_shadow_factor, h00, 0);
    let s10 = textureLoad(sdf_shadow_factor, h10, 0);
    let s01 = textureLoad(sdf_shadow_factor, h01, 0);
    let s11 = textureLoad(sdf_shadow_factor, h11, 0);

    // Depth at each tap — same `half→full` mapping the SDF pass used when it
    // wrote the factor, so the bilateral preserves the exact scene edges the
    // half-res shadow respects.
    let scale = depth_dims / half_dims;
    let d_max = depth_dims - vec2<f32>(1.0);
    let d00 = textureLoad(sdf_shadow_depth, vec2<i32>(clamp((vec2<f32>(h00) + vec2<f32>(0.5)) * scale, vec2<f32>(0.0), d_max)), 0);
    let d10 = textureLoad(sdf_shadow_depth, vec2<i32>(clamp((vec2<f32>(h10) + vec2<f32>(0.5)) * scale, vec2<f32>(0.0), d_max)), 0);
    let d01 = textureLoad(sdf_shadow_depth, vec2<i32>(clamp((vec2<f32>(h01) + vec2<f32>(0.5)) * scale, vec2<f32>(0.0), d_max)), 0);
    let d11 = textureLoad(sdf_shadow_depth, vec2<i32>(clamp((vec2<f32>(h11) + vec2<f32>(0.5)) * scale, vec2<f32>(0.0), d_max)), 0);

    // Bilinear weights from the sub-pixel fraction.
    let bw00 = (1.0 - frac.x) * (1.0 - frac.y);
    let bw10 = frac.x * (1.0 - frac.y);
    let bw01 = (1.0 - frac.x) * frac.y;
    let bw11 = frac.x * frac.y;

    // Depth weight: exponential falloff with a sigma scaled by the
    // fragment's own depth so far geometry (where small Δdepth still flags a
    // true edge) doesn't blur shadows across silhouettes. The 0.05 ratio
    // matches the half-res sample step the SDF pass uses.
    let sigma = max(frag_depth * 0.05, 1.0e-4);
    let dw00 = exp(-abs(d00 - frag_depth) / sigma);
    let dw10 = exp(-abs(d10 - frag_depth) / sigma);
    let dw01 = exp(-abs(d01 - frag_depth) / sigma);
    let dw11 = exp(-abs(d11 - frag_depth) / sigma);

    let w00 = bw00 * dw00;
    let w10 = bw10 * dw10;
    let w01 = bw01 * dw01;
    let w11 = bw11 * dw11;
    let w_sum = w00 + w10 + w01 + w11;

    // Degenerate sum — all 4 taps rejected by the depth weight. Fall back to
    // the nearest tap (by bilinear fraction) so the multiply stays sane on
    // silhouettes where every neighbour spans a depth discontinuity.
    if (w_sum <= 1.0e-6) {
        if (frac.x < 0.5 && frac.y < 0.5) { return s00; }
        if (frac.x >= 0.5 && frac.y < 0.5) { return s10; }
        if (frac.x < 0.5 && frac.y >= 0.5) { return s01; }
        return s11;
    }

    let inv = 1.0 / w_sum;
    return (s00 * w00 + s10 * w10 + s01 * w01 + s11 * w11) * inv;
}

// Map a K-selection slot (0..SDF_SELECT_K) to its visibility channel in the
// upsampled factor. Matches `sdf_shadow.wgsl`'s write layout exactly: slice i →
// channel i (slot 0 → R, 1 → G, 2 → B, 3 → A). The visibility pass and this
// reader must agree by construction — this is the same mapping documented at
// `sdf_shadow.wgsl`'s K-slice channel assignment.
fn slice_for_visibility(factor: vec4<f32>, slot: u32) -> f32 {
    switch slot {
        case 0u: { return factor.r; }
        case 1u: { return factor.g; }
        case 2u: { return factor.b; }
        default: { return factor.a; } // slot 3
    }
}

// Per-light SDF visibility for an arbitrary spec-light index, resolved through
// the fragment's K-selection (`sel`). Used by the specular loop, which walks the
// chunk light list in chunk order rather than selection order: for an `sdf`
// light it must read the SAME slice its diffuse term used, so it finds the
// light's slot in the selection (slot i ↔ channel i, by construction the same
// `sel` the diffuse loop read) and returns that channel. A light that is not
// `sdf`, or an `sdf` light that ranked beyond K (dropped from the selection,
// treated lit — matching the diffuse loop), returns 1.0 so the specular term is
// left unshadowed. `slice_for_visibility` does the slot→channel mapping.
fn sdf_visibility_for_light(sel: SdfLightSelection, factor: vec4<f32>, light_idx: u32) -> f32 {
    for (var s: u32 = 0u; s < sel.count; s = s + 1u) {
        if sel.indices[s] == light_idx {
            return slice_for_visibility(factor, s);
        }
    }
    return 1.0;
}

// Static-light entity-shadow receipt on world geometry. Normal mode subtracts
// this union term from baked static direct; mode 5 visualizes the subtraction
// magnitude so baked-vs-runtime darker-wins checks are repeatable.
const SHADOWMASK_VISUALIZE_MODE: u32 = 5u;
const SHADOWMASK_RAW_POOL_VISIBILITY_MODE: u32 = 6u;
const SHADOWMASK_INVALID_INDEX_VALUE: f32 = -1.0;
const SHADOWMASK_CHANNEL_DROPPED: f32 = 4.0;
const SHADOWMASK_POOL_SPOT: u32 = 0u;
const SHADOWMASK_POOL_CUBE: u32 = 1u;
const SHADOWMASK_POOL_SPOT_VALUE: f32 = 0.0;
const SHADOWMASK_POOL_CUBE_VALUE: f32 = 1.0;
const SHADOWMASK_SPOT_SLOT_COUNT: u32 = 96u;
const SHADOWMASK_CUBE_SLOT_COUNT: u32 = 6u;
const SHADOWMASK_EPS: f32 = 1.0e-4;
const SHADOWMASK_NDOTL_EPS: f32 = 1.0e-2;
const SHADOWMASK_SPOT_KERNEL_RADIUS: i32 = 2;
const SHADOWMASK_SPOT_KERNEL_TEXELS: f32 = 2.0;
const SHADOWMASK_META_VEC4S_PER_RECORD: u32 = 2u;
// Ignore one comparison tap of residual noise in each pool's union sampler.
// Renormalization preserves full subtraction after the pool-specific dead zone.
const SHADOWMASK_SPOT_VISIBILITY_DEAD_ZONE: f32 = 1.0 / 25.0;
const SHADOWMASK_POINT_VISIBILITY_DEAD_ZONE: f32 = 1.0 / 9.0;

struct ShadowmaskDirect {
    value: vec3<f32>,
    valid: u32,
};

struct ShadowmaskUnion {
    subtraction: vec3<f32>,
    raw_pool_visibility: f32,
};

fn shadowmask_channel_value(mask: vec4<f32>, channel: u32) -> f32 {
    switch channel {
        case 0u: { return mask.r; }
        case 1u: { return mask.g; }
        case 2u: { return mask.b; }
        case 3u: { return mask.a; }
        default: { return 1.0; }
    }
}

// Rejected or absent shadowmask resources bind a one-layer all-white texture.
// Clamp baked multi-layer vertex indices so that fallback always samples that
// fully-visible layer instead of addressing outside the bound texture.
fn sample_shadowmask_atlas(lightmap_uv: vec2<f32>, lightmap_layer: u32) -> vec4<f32> {
    let last_layer = textureNumLayers(shadowmask_atlas) - 1u;
    let safe_layer = min(lightmap_layer, last_layer);
    return textureSample(
        shadowmask_atlas,
        lightmap_filtering_sampler,
        lightmap_uv,
        i32(safe_layer),
    );
}

// Static non-SDF lights carry their baked shadowmask channel in `cone_cos.z`.
// A dropped/no-mask channel samples as fully lit, preserving the prior behavior.
fn shadowmask_visibility_for_spec_light(sl: SpecLight, mask: vec4<f32>) -> f32 {
    if uniforms.spec_shadowmask_force_one != 0u {
        return 1.0;
    }
    let spec_channel = round(sl.cone_cos.z);
    if spec_channel >= SHADOWMASK_CHANNEL_DROPPED {
        return 1.0;
    }
    return shadowmask_channel_value(mask, u32(spec_channel));
}

fn shadowmask_direct(
    sl: SpecLight,
    world_pos: vec3<f32>,
    mesh_n: vec3<f32>,
    bump_n: vec3<f32>,
) -> ShadowmaskDirect {
    var out: ShadowmaskDirect;
    out.value = vec3<f32>(0.0);
    out.valid = 0u;

    if u32(round(sl.cone_dir_and_type.w)) == 2u {
        return out;
    }
    let to_light = sl.position_and_range.xyz - world_pos;
    let dist = length(to_light);
    let range = sl.position_and_range.w;
    if range <= 0.0 || dist > range {
        return out;
    }
    let L = to_light / max(dist, 0.0001);
    let n_dot_l_mesh = max(dot(mesh_n, L), 0.0);
    let n_dot_l_bump = max(dot(bump_n, L), 0.0);
    let atten = max(1.0 - dist / max(range, 0.001), 0.0);
    let cone = cone_attenuation_cos(L, sl.cone_dir_and_type.xyz, sl.cone_cos.x, sl.cone_cos.y);
    let direct_mesh = sl.color_and_pad.xyz * (atten * cone * n_dot_l_mesh);
    if dot(direct_mesh, direct_mesh) <= SHADOWMASK_EPS * SHADOWMASK_EPS {
        return out;
    }
    let bump_scale = min(n_dot_l_bump / max(n_dot_l_mesh, SHADOWMASK_NDOTL_EPS), 4.0);
    out.value = direct_mesh * bump_scale;
    out.valid = 1u;
    return out;
}

fn shadowmask_sample_spot_shadow_wide(
    slot_index: u32,
    light_pos: vec3<f32>,
    world_pos: vec3<f32>,
    mesh_n: vec3<f32>,
    bias_scale: f32,
    light_proj: mat4x4<f32>,
) -> f32 {
    // Recover tan(fov_y / 2) from the bound matrix's y-row scale. The light
    // view is rigid, so the row length retains the projection scale.
    let projection_y_scale = length(vec3<f32>(
        light_proj[0].y,
        light_proj[1].y,
        light_proj[2].y,
    ));
    let tan_half_fov_y = 1.0 / max(projection_y_scale, 1.0e-4);
    let distance_to_light = length(world_pos - light_pos);
    let shadow_dims = textureDimensions(spot_shadow_depth);
    let texel_world_footprint =
        2.0 * distance_to_light * tan_half_fov_y / max(f32(shadow_dims.y), 1.0);
    let receiver_offset = mesh_n * (texel_world_footprint * bias_scale);
    let light_clip = light_proj * vec4<f32>(world_pos + receiver_offset, 1.0);
    if light_clip.w <= 0.0 {
        return 1.0;
    }
    let light_ndc = light_clip.xyz / light_clip.w;
    let uv = vec2<f32>(light_ndc.x * 0.5 + 0.5, light_ndc.y * -0.5 + 0.5);
    if uv.x < 0.0 || uv.x > 1.0 ||
       uv.y < 0.0 || uv.y > 1.0 ||
       light_ndc.z < 0.0 || light_ndc.z > 1.0 {
        return 1.0;
    }

    let texel = 1.0 / vec2<f32>(shadow_dims);
    let step = texel * SHADOWMASK_SPOT_KERNEL_TEXELS;
    var lit = 0.0;
    var taps = 0.0;
    for (var dy: i32 = -SHADOWMASK_SPOT_KERNEL_RADIUS; dy <= SHADOWMASK_SPOT_KERNEL_RADIUS; dy = dy + 1) {
        for (var dx: i32 = -SHADOWMASK_SPOT_KERNEL_RADIUS; dx <= SHADOWMASK_SPOT_KERNEL_RADIUS; dx = dx + 1) {
            let offset = vec2<f32>(f32(dx), f32(dy)) * step;
            lit = lit + textureSampleCompare(
                spot_shadow_depth,
                spot_shadow_compare,
                uv + offset,
                i32(slot_index),
                light_ndc.z
            );
            taps = taps + 1.0;
        }
    }
    return lit / max(taps, 1.0);
}

fn shadowmask_shadow_visibility(
    pool_kind: u32,
    slot: u32,
    sl: SpecLight,
    world_pos: vec3<f32>,
    mesh_n: vec3<f32>,
) -> f32 {
    if pool_kind == SHADOWMASK_POOL_SPOT {
        if slot >= SHADOWMASK_SPOT_SLOT_COUNT {
            return 1.0;
        }
        let light_proj = light_space_matrices.m[slot];
        return shadowmask_sample_spot_shadow_wide(
            slot,
            sl.position_and_range.xyz,
            world_pos,
            mesh_n,
            WORLD_RECEIVER_BIAS_SCALE,
            light_proj,
        );
    }
    if pool_kind == SHADOWMASK_POOL_CUBE {
        if slot >= SHADOWMASK_CUBE_SLOT_COUNT {
            return 1.0;
        }
        return sample_point_shadow(
            slot,
            sl.position_and_range.xyz,
            world_pos,
            mesh_n,
            WORLD_RECEIVER_BIAS_SCALE,
            sl.position_and_range.w,
        );
    }
    return 1.0;
}

// Both the union's baked-visibility skip and the difference renormalization must
// call this, so the skip threshold and the dead zone cannot drift apart.
fn shadowmask_dead_zone(pool_kind: u32) -> f32 {
    return select(
        SHADOWMASK_SPOT_VISIBILITY_DEAD_ZONE,
        SHADOWMASK_POINT_VISIBILITY_DEAD_ZONE,
        pool_kind == SHADOWMASK_POOL_CUBE,
    );
}

fn shadowmask_visibility_difference(
    pool_kind: u32,
    baked_vis: f32,
    shadow_map_vis: f32,
) -> f32 {
    let dead_zone = shadowmask_dead_zone(pool_kind);
    let difference = max(baked_vis - shadow_map_vis, 0.0);
    return max(difference - dead_zone, 0.0) / (1.0 - dead_zone);
}

fn shadowmask_union_subtraction(
    world_pos: vec3<f32>,
    lightmap_uv: vec2<f32>,
    lightmap_layer: u32,
    mesh_n: vec3<f32>,
    bump_n: vec3<f32>,
) -> ShadowmaskUnion {
    var out: ShadowmaskUnion;
    out.subtraction = vec3<f32>(0.0);
    // White means no eligible promoted light covers this receiver.
    out.raw_pool_visibility = 1.0;
    if uniforms.total_light_count <= uniforms.light_count {
        return out;
    }
    // Hoisted because every promoted light shares this fragment's lightmap
    // UV/layer.
    let mask = sample_shadowmask_atlas(lightmap_uv, lightmap_layer);
    let promoted_count = uniforms.total_light_count - uniforms.light_count;
    let influence_len = arrayLength(&light_influence);
    let spec_len = arrayLength(&spec_lights);
    for (var p: u32 = 0u; p < promoted_count; p = p + 1u) {
        let influence_index = uniforms.light_count + p;
        if influence_index >= influence_len {
            break;
        }
        let influence = light_influence[influence_index];
        let inf_radius = influence.w;
        if inf_radius <= 1.0e30 {
            let d = world_pos - influence.xyz;
            if dot(d, d) > inf_radius * inf_radius {
                continue;
            }
        }

        let meta_index = uniforms.total_light_count + p * SHADOWMASK_META_VEC4S_PER_RECORD;
        if meta_index + 1u >= influence_len {
            break;
        }
        let meta0 = light_influence[meta_index];
        let meta1 = light_influence[meta_index + 1u];
        let spec_idx_value = meta0.z;
        let weight = clamp(meta0.w, 0.0, 1.0);
        let pool_kind_value = meta1.x;
        let slot_value = meta1.y;
        let channel_value = meta1.z;

        if weight <= 0.0 ||
           spec_idx_value <= SHADOWMASK_INVALID_INDEX_VALUE ||
           spec_idx_value >= f32(spec_len) ||
           channel_value < 0.0 ||
           channel_value >= SHADOWMASK_CHANNEL_DROPPED {
            continue;
        }
        if pool_kind_value != SHADOWMASK_POOL_SPOT_VALUE && pool_kind_value != SHADOWMASK_POOL_CUBE_VALUE {
            continue;
        }
        if floor(spec_idx_value) != spec_idx_value ||
           floor(slot_value) != slot_value ||
           floor(channel_value) != channel_value {
            continue;
        }

        let spec_idx = u32(spec_idx_value);
        let pool_kind = u32(pool_kind_value);
        let slot = u32(slot_value);
        let channel = u32(channel_value);
        let sl = spec_lights[spec_idx];
        let direct = shadowmask_direct(sl, world_pos, mesh_n, bump_n);
        if direct.valid == 0u {
            continue;
        }

        let baked_vis = shadowmask_channel_value(mask, channel);
        // A baked-dark channel can never survive the dead zone, so the PCF
        // kernel would be sampled only to be multiplied out. The uniform mode
        // test comes first so the wavefront-coherent half short-circuits.
        if uniforms.sdf_shadow_mode != SHADOWMASK_RAW_POOL_VISIBILITY_MODE &&
           baked_vis <= shadowmask_dead_zone(pool_kind) {
            continue;
        }
        let shadow_map_vis = shadowmask_shadow_visibility(pool_kind, slot, sl, world_pos, mesh_n);
        // The raw-pool diagnostic follows the union's promoted-light coverage;
        // when several lights cover a receiver, the darkest map visibility wins.
        out.raw_pool_visibility = min(out.raw_pool_visibility, shadow_map_vis);
        let union_difference = shadowmask_visibility_difference(pool_kind, baked_vis, shadow_map_vis);
        out.subtraction = out.subtraction + direct.value * union_difference * weight;
    }
    return out;
}

@fragment
fn fs_main(in: VertexOutput) -> @location(0) vec4<f32> {
    // UV footprint derivatives — computed once here in uniform control flow.
    // WGSL requires dpdx/dpdy to be called from uniform control flow, so they
    // are hoisted out of the per-slot sampling helpers and handed to
    // textureSampleGrad as explicit gradients before conditional texture reads.
    let ddx = dpdx(in.uv);
    let ddy = dpdy(in.uv);

    let base_color = sample_color(base_texture, in.uv, ddx, ddy);

    let mesh_n = normalize(in.world_normal);

    // Tangent-space normal map + TBN construction. The neutral placeholder
    // (127, 127, 255, 255) decodes to ~(0, 0, 1), which TBN transforms back
    // to the mesh normal — surfaces without `_n.png` are identical to the
    // pre-bump path.
    let n_ts = sample_normal(t_normal, in.uv, ddx, ddy);
    let N_bump = reconstruct_tbn_normal(mesh_n, in.world_tangent, in.bitangent_sign, n_ts);

    const LIGHT_TERM_AMBIENT_FLOOR: u32 = 0x01u;
    const LIGHT_TERM_INDIRECT_STATIC: u32 = 0x02u;
    const LIGHT_TERM_INDIRECT_ANIMATED: u32 = 0x04u;
    const LIGHT_TERM_BAKED_DIRECT_STATIC: u32 = 0x08u;
    const LIGHT_TERM_BAKED_DIRECT_ANIMATED: u32 = 0x10u;
    const LIGHT_TERM_DYNAMIC_DIRECT: u32 = 0x20u;
    const LIGHT_TERM_SPECULAR: u32 = 0x40u;
    let light_terms = uniforms.light_term_mask;
    let use_ambient_floor = (light_terms & LIGHT_TERM_AMBIENT_FLOOR) != 0u;
    let use_indirect = (light_terms & (LIGHT_TERM_INDIRECT_STATIC | LIGHT_TERM_INDIRECT_ANIMATED)) != 0u;
    let use_baked_direct_static = (light_terms & LIGHT_TERM_BAKED_DIRECT_STATIC) != 0u;
    let use_baked_direct_animated = (light_terms & LIGHT_TERM_BAKED_DIRECT_ANIMATED) != 0u;
    let use_specular = (light_terms & LIGHT_TERM_SPECULAR) != 0u;
    let use_dynamic = (light_terms & LIGHT_TERM_DYNAMIC_DIRECT) != 0u;

    var indirect = vec3<f32>(0.0);
    if use_indirect {
        indirect = sample_sh_indirect(in.world_position, N_bump, mesh_n) * uniforms.indirect_scale;
    }

    // SDF static-occluder shadow factor. The four RGBA channels are the K = 4
    // per-light visibility slices consumed by the sdf-tag diffuse/specular loop
    // below. `vec4(1.0)` when no SDF atlas is loaded, so the multiply downstream
    // is a no-op. `lm_irr` (baked-tag lights) and `lm_anim` (animated-baked
    // lights) carry their shadow baked in — neither is SDF-multiplied, so the
    // disjoint direct sets stay additive with no re-weighting.
    var sdf_factor = vec4<f32>(1.0, 1.0, 1.0, 1.0);
    if uniforms.sdf_shadow_flags != 0u {
        sdf_factor = upsample_shadow_factor(in.clip_position.xy, in.clip_position.z);
    }
    // `SdfShadowMode::Off` (1) short-circuits the per-light SDF visibility to 1.0.
    let sdf_mode_off = uniforms.sdf_shadow_mode == 1u;

    // Static direct term: baked directional lightmap. NdotL is already folded
    // in by the baker — sampling gives correct static direct contribution for
    // a mesh-normal surface.
    var static_direct = vec3<f32>(0.0);
    var shadowmask_union = vec3<f32>(0.0);
    var shadowmask_raw_pool_visibility = 1.0;
    if use_baked_direct_static || use_baked_direct_animated {
        // Irradiance + animated atlas filter bilinear (HW linear sampler at
        // binding 4) so baked penumbra ramps read as continuous gradients under
        // magnification. The direction channel below stays on the nearest
        // sampler (octahedral lerp ≠ slerp).
        var lm_irr = vec3<f32>(0.0);
        if use_baked_direct_static {
            lm_irr = sample_lightmap_irradiance(in.lightmap_uv, in.lightmap_layer);
        }
        // Pre-shaded Lambert irradiance from the animated compose pre-pass.
        // A static layer absent from section 25 resolves to INVALID_SLOT, so it
        // contributes zero rather than accidentally sampling animated slot 0.
        var lm_anim = vec3<f32>(0.0);
        var anim_dir_sample = vec4<f32>(0.5, 1.0, 0.5, 0.0);
        let animated_slot = animated_slot_for_static_layer(in.lightmap_layer);
        if use_baked_direct_animated && animated_slot != 0xffffffffu {
            lm_anim = sample_lightmap_animated(in.lightmap_uv, animated_slot);
            anim_dir_sample = textureSample(
                animated_lm_direction,
                lightmap_sampler,
                in.lightmap_uv,
                i32(animated_slot),
            );
        }

        // Bumped-Lambert correction: the baker pre-multiplied by mesh-normal NdotL
        // using the dominant incident direction. Divide out mesh NdotL and
        // remultiply with N_bump NdotL to make the static term respond to normal-map
        // detail. lm_anim gets the same correction below via its own fused animated
        // dominant direction, so style-animated lights respond to normal-map detail
        // identically to static ones.
        let dom = decode_lightmap_direction(textureSample(lightmap_direction, lightmap_sampler, in.lightmap_uv, i32(in.lightmap_layer)));
        let n_dot_l_mesh = max(dot(mesh_n, dom), 0.0);
        let n_dot_l_bump = max(dot(N_bump, dom), 0.0);
        // NDOTL_EPS is a tight cosine floor (~0.57° from grazing) — a divide-by-zero
        // guard, not a wide grazing-angle fade. It gates the correction off and floors
        // the ratio denominator so the bumped/mesh NdotL ratio can't blow up when the
        // dominant light is near-perpendicular to the mesh normal.
        const NDOTL_EPS: f32 = 1.0e-2;
        // Skip correction when irradiance is negligible — dominant direction is
        // unreliable for unlit texels.
        const LM_IRR_EPS: f32 = 1.0e-4;
        let use_correction = dot(lm_irr, lm_irr) >= LM_IRR_EPS * LM_IRR_EPS && n_dot_l_mesh > NDOTL_EPS;
        // Cap at 4.0: prevents unbounded spike when N_bump tilts toward the light
        // on a near-backfacing mesh surface.
        let scale = select(1.0, min(n_dot_l_bump / max(n_dot_l_mesh, NDOTL_EPS), 4.0), use_correction);

        // Same bumped-Lambert correction for the animated term. The animated
        // direction atlas is octahedral in .rg (decoded by the shared
        // `decode_lightmap_direction`) with a coverage flag in .a; NDOTL_EPS floor
        // and 4.0 cap are shared with the static path. The compose pass clears
        // coverage to 0.0 for uncovered/canceling texels (oct decode of those
        // yields a valid-but-meaningless direction, so a NaN sentinel no longer
        // works) — the use_correction_anim gate reads .a to skip them. The
        // sample itself is hoisted above into the static-layer-to-slot lookup;
        // layers without animated receivers keep the zero-coverage sentinel.
        let dom_anim = decode_lightmap_direction(anim_dir_sample);
        let n_dot_l_mesh_anim = max(dot(mesh_n, dom_anim), 0.0);
        let n_dot_l_bump_anim = max(dot(N_bump, dom_anim), 0.0);
        // Mirror the static gate: when lm_anim is ~zero (no animated weight maps or
        // dark this frame) the fused direction is unreliable, so leave the term as-is.
        const LM_ANIM_EPS: f32 = 1.0e-4;
        let anim_covered = anim_dir_sample.a > 0.5;
        let use_correction_anim = anim_covered && dot(lm_anim, lm_anim) >= LM_ANIM_EPS * LM_ANIM_EPS && n_dot_l_mesh_anim > NDOTL_EPS;
        let scale_anim = select(1.0, min(n_dot_l_bump_anim / max(n_dot_l_mesh_anim, NDOTL_EPS), 4.0), use_correction_anim);
        // Both `lm_irr` (baked-tag lights) and `lm_anim` (animated-baked lights)
        // carry their shadow baked in — neither is SDF-multiplied. The animated
        // shadow is occlusion-tested into the weight-map bake (`lm_anim`); the
        // retired runtime SDF factor was double-shadowing it. Shadow-map (enemy)
        // results never run through these factors — they carry their own
        // dynamic-occluder shadow in the dynamic-light loop below.
        static_direct = lm_irr * scale + lm_anim * scale_anim;
    }

    // K-selection of `sdf`-tagged lights for this fragment, computed ONCE and
    // shared by the per-light diffuse loop (below) and the per-light specular
    // loop (further down). Both terms of an `sdf` light must read the SAME
    // visibility slice, so they must read it off the SAME selection: a single
    // `select_sdf_lights` call pins slot i → light i → channel i for both.
    //
    // NOTE (Task 4 visual check): `select_sdf_lights` uses the interpolated
    // full-res world position; the half-res visibility pass reconstructs
    // position from half-res depth. Near a `chunk_grid` cell boundary the
    // two can select a different K-set — watch for boundary seam artifacts.
    let sdf_sel = select_sdf_lights(in.world_position);
    // Dev toggle: force visibility to 1.0 for the "no double-count" A/B
    // (forced-1.0 must match the pre-change render — disjoint sets mean the
    // additive sum is the only thing this loop introduces). `SdfShadowMode::Off`
    // also forces 1.0 so the sdf terms still land but unshadowed, mirroring the
    // baked-term Off behavior. Applies to BOTH diffuse and specular.
    let sdf_force_lit = uniforms.sdf_force_visibility_one != 0u || sdf_mode_off;

    // Per-light SDF diffuse (sdf-tagged static lights). Disjoint from `lm_irr`
    // /`lm_anim` by construction (the compiler excludes sdf lights from both
    // bake sets), so this is purely additive — no re-weighting. Multiplies each
    // selected light's Lambert diffuse by its upsampled visibility slice (slot i
    // → R/G/B/A via `slice_for_visibility`). It shares the baked-direct static
    // gate because it is that term's runtime shadow implementation.
    if use_baked_direct_static {
        for (var s: u32 = 0u; s < sdf_sel.count; s = s + 1u) {
            let sl = spec_lights[sdf_sel.indices[s]];
            let to_light = sl.position_and_range.xyz - in.world_position;
            let dist = length(to_light);
            let range = sl.position_and_range.w;
            if range > 0.0 && dist > range {
                continue;
            }
            let L = to_light / max(dist, 0.0001);
            let n_dot_l = dot(N_bump, L);
            if n_dot_l <= 0.0 {
                continue;
            }
            let atten = select(1.0, max(1.0 - dist / max(range, 0.001), 0.0), range > 0.0);
            let cone = cone_attenuation_cos(L, sl.cone_dir_and_type.xyz, sl.cone_cos.x, sl.cone_cos.y);
            let visibility = select(slice_for_visibility(sdf_factor, s), 1.0, sdf_force_lit);
            static_direct = static_direct + sl.color_and_pad.xyz * (n_dot_l * atten * cone * visibility);
        }
    }

    if use_baked_direct_static || uniforms.sdf_shadow_mode == SHADOWMASK_VISUALIZE_MODE || uniforms.sdf_shadow_mode == SHADOWMASK_RAW_POOL_VISIBILITY_MODE {
        let shadowmask = shadowmask_union_subtraction(
            in.world_position,
            in.lightmap_uv,
            in.lightmap_layer,
            mesh_n,
            N_bump,
        );
        shadowmask_union = shadowmask.subtraction;
        shadowmask_raw_pool_visibility = shadowmask.raw_pool_visibility;
        if use_baked_direct_static {
            static_direct = max(static_direct - shadowmask_union, vec3<f32>(0.0));
        }
    }

    var total_light = select(vec3<f32>(0.0), vec3<f32>(uniforms.ambient_floor), use_ambient_floor)
        + indirect
        + static_direct;

    var specular_sum = vec3<f32>(0.0);
    if use_specular {
        let V = normalize(uniforms.camera_position - in.world_position);
        let spec_int = sample_color(spec_texture, in.uv, ddx, ddy).r;
        let spec_exp = max(material.shininess, 1.0);
        // Hoisted because every static specular light shares this fragment's
        // lightmap UV/layer. Undo this if specular gains per-light UVs.
        let specular_shadowmask = sample_shadowmask_atlas(in.lightmap_uv, in.lightmap_layer);

        // Chunk lookup when the offline index is populated; otherwise walk
        // the full spec buffer.
        var chunk_offset: u32 = 0u;
        var chunk_count: u32 = arrayLength(&spec_lights);
        if chunk_grid.has_chunk_grid != 0u {
            let local = in.world_position - chunk_grid.grid_origin;
            let cell = vec3<i32>(floor(local / chunk_grid.cell_size));
            let dims = vec3<i32>(chunk_grid.dims);
            // Fragments outside the authored grid have no static lights by construction.
            if all(cell >= vec3<i32>(0)) && all(cell < dims) {
                let ci = u32(cell.z) * chunk_grid.dims.x * chunk_grid.dims.y
                       + u32(cell.y) * chunk_grid.dims.x
                       + u32(cell.x);
                let pair = chunk_offsets[ci];
                chunk_offset = pair.x;
                chunk_count = pair.y;
            } else {
                chunk_count = 0u;
            }
        }

        for (var j: u32 = 0u; j < chunk_count; j = j + 1u) {
            var light_idx: u32 = j;
            if chunk_grid.has_chunk_grid != 0u {
                light_idx = chunk_indices[chunk_offset + j];
            }
            let sl = spec_lights[light_idx];
            let to_light = sl.position_and_range.xyz - in.world_position;
            let dist = length(to_light);
            let range = sl.position_and_range.w;
            // The chunk list is a conservative spatial index; range is the tight
            // per-light cutoff.
            if range > 0.0 && dist > range {
                continue;
            }
            let L = to_light / max(dist, 0.0001);
            let NdotL = dot(N_bump, L);
            if NdotL <= 0.0 {
                continue;
            }
            let atten = select(1.0, max(1.0 - dist / max(range, 0.001), 0.0), range > 0.0);
            let cone = cone_attenuation_cos(L, sl.cone_dir_and_type.xyz, sl.cone_cos.x, sl.cone_cos.y);
            // Exactly one technique applies: SDF lights retain the per-light
            // visibility slice shared with their diffuse term; other static
            // lights use the baked shadowmask recorded on the SpecLight.
            var visibility = 1.0;
            if sdf_select_is_sdf(sl) {
                visibility = sdf_visibility_for_light(sdf_sel, sdf_factor, light_idx);
                if sdf_force_lit {
                    visibility = 1.0;
                }
            } else {
                visibility = shadowmask_visibility_for_spec_light(sl, specular_shadowmask);
            }
            let contribution = blinn_phong(
                L, V, N_bump, sl.color_and_pad.xyz, spec_exp, spec_int
            ) * (atten * cone * visibility);
            specular_sum = specular_sum + contribution;
        }
    }
    total_light = total_light + specular_sum;

    let light_count = select(0u, uniforms.light_count, use_dynamic);
    for (var i: u32 = 0u; i < light_count; i = i + 1u) {
        // Influence-volume early-out: pure optimization — no pixel change.
        let influence = light_influence[i];
        let inf_radius = influence.w;
        if inf_radius <= 1.0e30 {
            let d = in.world_position - influence.xyz;
            if dot(d, d) > inf_radius * inf_radius {
                continue;
            }
        }

        let light = lights[i];
        let light_type = bitcast<u32>(light.position_and_type.w);
        let falloff_model = bitcast<u32>(light.color_and_falloff_model.w);

        // Scripted per-light animation. `is_active == 0` is the sentinel path:
        // effective_color and effective_aim stay as the static GpuLight values.
        // Active descriptors override brightness, color, and (for spots) aim
        // from Catmull-Rom curves on the shared anim_samples buffer.
        let scripted_desc = scripted_light_descriptors[i];
        var effective_color = light.color_and_falloff_model.xyz;
        var effective_aim = light.direction_and_range.xyz;
        if scripted_desc.is_active != 0u {
            let cycle_t = animation_curve_t(
                scripted_desc.period,
                scripted_desc.phase,
                uniforms.time,
            );
            // Color curves set hue; the static light slot carries intensity.
            // Brightness curves multiply whichever color path is active.
            // Clamp non-negative: Catmull-Rom overshoot between keyframes can go
            // below zero, which would make an animated light emit negative,
            // sign-flipped (wrong-colored) light.
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
                let to_light = light.position_and_type.xyz - in.world_position;
                let dist = length(to_light);
                L = to_light / max(dist, 0.0001);
                attenuation = light_eval_falloff(dist, light.direction_and_range.w, falloff_model);

                // Dynamic point-light cube shadow. The cube slot rides in
                // `cone_angles_and_pad.w` (sentinel 0xFFFFFFFF = no slot, i.e.
                // the light was not ranked into the cube pool). A point light
                // without a slot keeps its normal unshadowed attenuation. The
                // multiply lives ONLY here in the world light loop — no lightmap
                // is touched, so a dynamic point light's direct term is shadowed
                // exactly once (same construction as the spot path).
                let cube_slot = bitcast<u32>(light.cone_angles_and_pad.w);
                if cube_slot != 0xFFFFFFFFu {
                    let shadow = sample_point_shadow(
                        cube_slot,
                        light.position_and_type.xyz,
                        in.world_position,
                        mesh_n,
                        WORLD_RECEIVER_BIAS_SCALE,
                        light.direction_and_range.w
                    );
                    attenuation = attenuation * shadow;
                }
            }
            case 1u: {
                let to_light = light.position_and_type.xyz - in.world_position;
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

                let slot_index = bitcast<u32>(light.cone_angles_and_pad.z);
                if slot_index != 0xFFFFFFFFu {
                    let light_proj = light_space_matrices.m[slot_index];
                    let shadow = sample_spot_shadow(
                        slot_index,
                        light.position_and_type.xyz,
                        in.world_position,
                        mesh_n,
                        WORLD_RECEIVER_BIAS_SCALE,
                        light_proj,
                    );
                    attenuation = attenuation * shadow;
                }
            }
            default: {
                // Directional light (case 2u and any unknown discriminant).
                L = -effective_aim;
                attenuation = 1.0;
            }
        }

        let NdotL = max(dot(N_bump, L), 0.0);
        total_light = total_light + effective_color * attenuation * NdotL;
    }

    var emissive = vec3<f32>(0.0);
    if material.emissive_strength > 0.0 {
        emissive = sample_color(emissive_texture, in.uv, ddx, ddy).rgb;
    }
    let rgb = base_color.rgb * total_light + emissive * material.emissive_strength;
    // `SdfShadowMode::Visualize` (2) replaces the shaded color with a
    // grayscale view of the first per-light visibility slice (R = slot 0,
    // the most-influential sdf light) — sampled through the same bilateral
    // upsample as the shading path. White = lit, black = fully occluded. When
    // no SDF atlas is loaded `sdf_factor` is `vec4(1.0)`, so Visualize on a
    // legacy PRL renders a flat white frame — self-documenting "nothing to
    // visualize".
    if uniforms.sdf_shadow_mode == 2u {
        let g = sdf_factor.r;
        return vec4<f32>(g, g, g, base_color.a);
    }
    // TEMP DEBUG: SDF shadow path visualization (mode 3). The half-res pass
    // encoded the slot-0 trace OUTCOME as an RGB code (see `debug_trace_outcome`
    // in sdf_shadow.wgsl). Sample it directly with a NEAREST half-res tap —
    // NOT the per-light bilateral upsample, which would blend distinct outcome
    // codes into meaningless intermediate colors. Legend:
    //   BLUE          open-space skip early-out
    //   RED→ORANGE    hard hit (green = normalized hit distance: red=near, orange/yellow=far)
    //   dark GREEN    penumbra-limited shadow (darker = stronger)
    //   WHITE         fully lit
    //   MAGENTA       no SDF light selected (no trace ran)
    if uniforms.sdf_shadow_mode == 3u {
        let depth_dims = vec2<f32>(textureDimensions(sdf_shadow_depth));
        let half_dims = vec2<f32>(textureDimensions(sdf_shadow_factor));
        let h_max = half_dims - vec2<f32>(1.0);
        let half_xy = (in.clip_position.xy / depth_dims) * half_dims;
        let h = vec2<i32>(clamp(floor(half_xy), vec2<f32>(0.0), h_max));
        let code = textureLoad(sdf_shadow_factor, h, 0).rgb;
        return vec4<f32>(code, base_color.a);
    }
    // TEMP DEBUG: SDF shadow path visualization (mode 4). The half-res pass
    // encoded the reconstructed GEOMETRIC NORMAL as RGB = normal*0.5+0.5 (see
    // the debug branch in sdf_shadow.wgsl's `cs_main`). Sample it with a NEAREST
    // half-res tap — NOT the per-light bilateral upsample, which would blend
    // distinct normals into meaningless intermediate colors. Color meaning:
    //   +X→reddish  +Y→greenish  +Z→bluish; flat faces show a smooth constant
    //   color, edges/corners may show seams. Mid-gray (0.5,0.5,0.5) = the
    //   reconstruction was unusable (degenerate / off-screen neighborhood).
    if uniforms.sdf_shadow_mode == 4u {
        let depth_dims = vec2<f32>(textureDimensions(sdf_shadow_depth));
        let half_dims = vec2<f32>(textureDimensions(sdf_shadow_factor));
        let h_max = half_dims - vec2<f32>(1.0);
        let half_xy = (in.clip_position.xy / depth_dims) * half_dims;
        let h = vec2<i32>(clamp(floor(half_xy), vec2<f32>(0.0), h_max));
        let n = textureLoad(sdf_shadow_factor, h, 0).rgb;
        return vec4<f32>(n, base_color.a);
    }
    if uniforms.sdf_shadow_mode == SHADOWMASK_VISUALIZE_MODE {
        return vec4<f32>(shadowmask_union, base_color.a);
    }
    if uniforms.sdf_shadow_mode == SHADOWMASK_RAW_POOL_VISIBILITY_MODE {
        let g = shadowmask_raw_pool_visibility;
        return vec4<f32>(g, g, g, base_color.a);
    }
    return vec4<f32>(rgb, base_color.a);
}
