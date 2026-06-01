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
    // Lighting-term isolation for leak/bleed debugging. Set via the
    // Diagnostics panel dropdown (dev-tools). Values 0..=9 — see fs_main for the full
    // table; in summary 0 = Normal, 1 = NoLightmap, 2 = DirectOnly,
    // 3 = IndirectOnly, 4 = AmbientOnly, 5 = LightmapOnly,
    // 6 = StaticSHOnly, 7 = AnimatedDeltaOnly, 8 = DynamicOnly,
    // 9 = SpecularOnly.
    lighting_isolation: u32,
    // Per-frame multiplier on the SH indirect term. 1.0 preserves baked
    // intensity; lower values suppress SH fill on static surfaces to keep
    // lightmap shadow contrast. Forced to 1.0 in indirect-only isolation
    // modes so debug views aren't affected by runtime suppression.
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
    sdf_shadow_mode: u32,
    // Dev toggle (non-zero ⇒ force per-light SDF visibility to 1.0). Used by
    // the "no double-count" visual AC: with every sdf light's visibility
    // forced fully lit, the per-light diffuse sum must reproduce the
    // pre-change render with no brightening (disjoint sets guarantee the term
    // is purely additive). Set via the Diagnostics panel checkbox.
    sdf_force_visibility_one: u32,
    // One u32 padding slot — keeps the trailing 16-byte vec4 row of the
    // struct fully accounted for. A plain u32 (not folded into a vec) so the
    // struct's natural alignment stays 4 bytes and total stride lands
    // exactly at 112 bytes — wgpu rejects the pipeline if the CPU-side
    // `UNIFORM_SIZE` and WGSL-derived stride drift.
    _sdf_pad1: u32,
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
// Per-material specular texture (R8Unorm sampled as .r). 1×1 black when the
// diffuse's `_s.png` sibling is absent — zeros `spec_int` without any
// shader branching. See context/lib/resource_management.md §4.1.
@group(1) @binding(2) var spec_texture: texture_2d<f32>;

struct MaterialUniform {
    // Blinn-Phong specular exponent; constant per-material variant.
    // Padded to 16 B for uniform-buffer alignment.
    shininess: f32,
    _pad: vec3<f32>,
};
@group(1) @binding(3) var<uniform> material: MaterialUniform;
// Per-material tangent-space normal map. Sampled with `aniso_sampler`. The
// neutral placeholder is (127, 127, 255, 255) which decodes to ~(0, 0, 1)
// in tangent space, so surfaces with no `_n.png` sibling render identically
// to the mesh-normal path. See context/lib/resource_management.md §4.3.
@group(1) @binding(4) var t_normal: texture_2d<f32>;
// Linear + hardware-anisotropic sampler. The sole texture-filtering path: the
// Post Retro path samples through this so hardware aniso kills grazing-angle
// shimmer while in-shader texel-grid reconstruction keeps texels crisp up
// close. Wired by the BGL and every material bind group on the Rust side — see
// `Renderer::mip_count_aniso_samplers` and the group-1 BGL comment in
// render/mod.rs. (Binding 1 is intentionally vacated — renumbering the aniso sampler down would require a matching BGL and material-bind-group rebuild with no functional benefit.)
@group(1) @binding(5) var aniso_sampler: sampler;

@group(2) @binding(0) var<storage, read> lights: array<GpuLight>;
// Per-light influence volume: xyz = sphere center, w = radius.
@group(2) @binding(1) var<storage, read> light_influence: array<vec4<f32>>;

// Static light buffer: specular + per-light SDF diffuse for sdf-tagged lights.
// Two vec4 slots (32 B stride); see postretro/src/lighting/spec_buffer.rs
// for the CPU-side layout.
struct SpecLight {
    position_and_range: vec4<f32>, // xyz = position, w = falloff_range
    color_and_pad:      vec4<f32>, // xyz = color × intensity, w = sdf flag (>0.5 ⇒ _shadow_type sdf)
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
// the shader skips SH sampling. See postretro/src/render/sh_volume.rs.
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

// Per-light animation descriptor — matches ANIMATION_DESCRIPTOR_SIZE (48 B)
// in postretro/src/render/sh_volume.rs. Field order diverges from the spec
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

@group(3) @binding(1) var sh_total_atlas: texture_2d<f32>;
@group(3) @binding(2) var sh_atlas_sampler: sampler;
@group(3) @binding(10) var<uniform> sh_grid: ShGridInfo;

// Animation buffers. Always bound; anim_descriptors and anim_samples are
// consumed by the animated lightmap compose pass (group 4 binding 3) and
// also exposed here so the bind group layout is stable across passes.
@group(3) @binding(11) var<storage, read> anim_descriptors: array<AnimationDescriptor>;
@group(3) @binding(12) var<storage, read> anim_samples: array<f32>;

// One AnimationDescriptor per map light, indexed by the forward light-loop
// counter `i`. `is_active == 0` → static GpuLight.color used unchanged.
// Uploaded by `LightBridge::update → Renderer::upload_bridge_descriptors`.
@group(3) @binding(13) var<storage, read> scripted_light_descriptors: array<AnimationDescriptor>;
@group(3) @binding(14) var sh_depth_moments: texture_3d<f32>;

// Group 4 — baked directional lightmap (static direct lighting).
// See context/lib/rendering_pipeline.md §4.
@group(4) @binding(0) var lightmap_irradiance: texture_2d<f32>;
@group(4) @binding(1) var lightmap_direction: texture_2d<f32>;
// Non-filtering (Nearest) sampler — used only for the octahedral direction
// texture (binding 1): linear interpolation of octahedral unit vectors does
// not commute with slerp.
@group(4) @binding(2) var lightmap_sampler: sampler;
// Animated-light contribution atlas (Rgba16Float). Composed each frame by
// the compute pre-pass in `animated_lightmap.rs` from per-animated-light
// baked weight maps + runtime descriptor curves. `.rgb` carries pre-shaded
// irradiance (Lambert already baked in); `.a` is a coverage flag reserved
// for debug visualization. When the PRL has no animated weight maps, this
// slot binds a 1×1 zero texture so the fragment shader reads 0.
@group(4) @binding(3) var animated_lm_atlas: texture_2d<f32>;
// Filtering (Linear) sampler — used for the irradiance + animated atlases so
// baked penumbra ramps read as continuous gradients under magnification. Bound
// in every variant; unused when `use_hw_filter == false` (the manual 4-tap
// fallback path). See baked-soft-lightmap-shadows/ §Task 5.
@group(4) @binding(4) var lightmap_filtering_sampler: sampler;

// Pipeline-override constant, decided once at pipeline creation (see
// `render/mod.rs` `Renderer::new`): `true` when the atlas format (Rgba16Float) advertises
// hardware bilinear filtering, sampling irradiance + animated atlas through the
// linear sampler; `false` falls back to a manual 4-tap bilinear lerp. One
// pipeline is built per init, so this never becomes a per-fragment branch.
override use_hw_filter: bool = true;

// Manual 4-tap bilinear lerp for the fallback path (`use_hw_filter == false`).
// Reproduces hardware bilinear filtering with `textureLoad` so the resulting
// ramp is identical to the HW-filtered path on backends where Rgba16Float
// filtering is unavailable. `uv` is in [0,1] texture space.
fn bilinear_rgb(tex: texture_2d<f32>, uv: vec2<f32>) -> vec3<f32> {
    let dims = vec2<f32>(textureDimensions(tex, 0));
    // Texel-center sample space: shift by -0.5 so the four taps bracket `uv`.
    let coord = uv * dims - vec2<f32>(0.5);
    let base = floor(coord);
    let frac = coord - base;
    let i0 = vec2<i32>(base);
    let maxc = vec2<i32>(dims) - vec2<i32>(1);
    // Clamp to edge (matches the samplers' ClampToEdge address mode).
    let x0 = clamp(i0.x, 0, maxc.x);
    let y0 = clamp(i0.y, 0, maxc.y);
    let x1 = clamp(i0.x + 1, 0, maxc.x);
    let y1 = clamp(i0.y + 1, 0, maxc.y);
    let c00 = textureLoad(tex, vec2<i32>(x0, y0), 0).rgb;
    let c10 = textureLoad(tex, vec2<i32>(x1, y0), 0).rgb;
    let c01 = textureLoad(tex, vec2<i32>(x0, y1), 0).rgb;
    let c11 = textureLoad(tex, vec2<i32>(x1, y1), 0).rgb;
    let top = mix(c00, c10, frac.x);
    let bot = mix(c01, c11, frac.x);
    return mix(top, bot, frac.y);
}

// Sample the irradiance atlas with hardware bilinear filtering or the manual
// 4-tap fallback, selected once by the `use_hw_filter` override constant.
fn sample_lightmap_irradiance(uv: vec2<f32>) -> vec3<f32> {
    if use_hw_filter {
        return textureSample(lightmap_irradiance, lightmap_filtering_sampler, uv).rgb;
    }
    return bilinear_rgb(lightmap_irradiance, uv);
}

// Same path selection for the animated-light contribution atlas.
fn sample_lightmap_animated(uv: vec2<f32>) -> vec3<f32> {
    if use_hw_filter {
        return textureSample(animated_lm_atlas, lightmap_filtering_sampler, uv).rgb;
    }
    return bilinear_rgb(animated_lm_atlas, uv);
}

// Group 5 — dynamic spot light shadow maps.
// See context/lib/rendering_pipeline.md §4.
@group(5) @binding(0) var spot_shadow_depth: texture_depth_2d_array;
@group(5) @binding(1) var spot_shadow_compare: sampler_comparison;
// Uniform (not storage) so we stay under `max_storage_buffers_per_shader_stage`
// (default limit 8 on some adapters — wgpu refuses the pipeline if we add
// a 9th). 12 × mat4x4<f32> is 768 bytes, well under the 16 KiB uniform cap.
struct LightSpaceMatrices {
    m: array<mat4x4<f32>, 12>,
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

struct VertexInput {
    @location(0) position: vec3<f32>,
    @location(1) base_uv: vec2<f32>,
    @location(2) normal_oct: vec2<u32>,
    @location(3) tangent_packed: vec2<u32>,
    @location(4) lightmap_uv_packed: vec2<u32>,
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

fn falloff(distance: f32, range: f32, model: u32) -> f32 {
    let r = max(range, 0.001);
    switch model {
        case 0u: {
            return max(1.0 - distance / r, 0.0);
        }
        case 1u: {
            // Linear window drives inverse-distance smoothly to 0 at range.
            return (1.0 / max(distance, 0.001)) * max(1.0 - distance / r, 0.0);
        }
        case 2u: {
            let d2 = max(distance * distance, 0.001);
            return (1.0 / d2) * max(1.0 - distance / r, 0.0);
        }
        default: {
            return 0.0;
        }
    }
}

// No `(1-ks)` attenuation, no Fresnel — retro aesthetic wants punchy additive
// highlights, not energy conservation.
fn blinn_phong(L: vec3<f32>, V: vec3<f32>, N: vec3<f32>,
               color: vec3<f32>, spec_exp: f32, spec_int: f32) -> vec3<f32> {
    let H = normalize(L + V);
    let NdH = max(dot(N, H), 0.0);
    return color * pow(NdH, spec_exp) * spec_int;
}

fn cone_attenuation(L: vec3<f32>, aim: vec3<f32>, inner_angle: f32, outer_angle: f32) -> f32 {
    let cos_angle = dot(-L, aim);
    let cos_inner = cos(inner_angle);
    let cos_outer = cos(outer_angle);
    return smoothstep(cos_outer, cos_inner, cos_angle);
}

// Sample the direction channel of an AnimationDescriptor at `cycle_t` and fall
// back to `static_aim` when the descriptor carries no direction samples.
// Samples are normalized at write time; Catmull-Rom between unit vectors drifts
// only slightly off the sphere at typical authored sample rates.
fn sample_animated_direction(desc: AnimationDescriptor, cycle_t: f32, static_aim: vec3<f32>) -> vec3<f32> {
    if desc.direction_count == 0u {
        return static_aim;
    }
    let zero_base = vec3<f32>(0.0, 0.0, 0.0);
    return sample_color_catmull_rom(desc.direction_offset, desc.direction_count, cycle_t, zero_base);
}

// Sample the shadow map for a dynamic spot light. Returns 0.0 (fully shadowed)
// to 1.0 (fully lit). Fragments outside the shadow map's projection are treated
// as unshadowed (1.0).
//
// `slot_index`: shadow-map slot (0..7) from GpuLight.cone_angles_and_pad.z.
fn sample_spot_shadow(slot_index: u32, world_pos: vec3<f32>, light_proj: mat4x4<f32>) -> f32 {
    let light_clip = light_proj * vec4<f32>(world_pos, 1.0);
    // Points behind the light produce negative w; reject to avoid folding the
    // perspective divide onto the near plane.
    if light_clip.w <= 0.0 {
        return 1.0;
    }
    let light_ndc = light_clip.xyz / light_clip.w;

    // NDC x,y are in [-1, 1] (wgpu convention). Flip y for texture top-left origin.
    let uv = vec2<f32>(light_ndc.x * 0.5 + 0.5, light_ndc.y * -0.5 + 0.5);
    if uv.x < 0.0 || uv.x > 1.0 ||
       uv.y < 0.0 || uv.y > 1.0 ||
       light_ndc.z < 0.0 || light_ndc.z > 1.0 {
        return 1.0; // Unshadowed — outside cone.
    }

    // CompareFunction::Less returns 1.0 when fragment depth < stored depth (lit).
    return textureSampleCompare(
        spot_shadow_depth,
        spot_shadow_compare,
        uv,
        i32(slot_index),
        light_ndc.z
    );
}

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

// Normal-map dispatch: BC5 stores only tangent-space (x, y) in RG, so decode
// those (`* 2 - 1`) and reconstruct z = sqrt(1 - x² - y²). Renormalize
// unconditionally — BC5 endpoint quantisation plus bilinear filtering leaves
// the sampled vector slightly off unit length.
fn sample_normal(tex: texture_2d<f32>, uv: vec2<f32>, ddx: vec2<f32>, ddy: vec2<f32>) -> vec3<f32> {
    let rg = sample_post_retro(tex, aniso_sampler, uv, ddx, ddy).rg * 2.0 - 1.0;
    let z  = sqrt(max(0.0, 1.0 - dot(rg, rg)));
    return normalize(vec3<f32>(rg, z));
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

@fragment
fn fs_main(in: VertexOutput) -> @location(0) vec4<f32> {
    // UV footprint derivatives — computed once here in uniform control flow.
    // WGSL requires dpdx/dpdy to be called from uniform control flow, so they
    // are hoisted out of the per-slot sampling helpers and handed to
    // textureSampleGrad as explicit gradients. Shared by all three texture slots.
    let ddx = dpdx(in.uv);
    let ddy = dpdy(in.uv);

    let base_color = sample_color(base_texture, in.uv, ddx, ddy);

    let mesh_n = normalize(in.world_normal);

    // Tangent-space normal map + TBN construction. The neutral placeholder
    // (127, 127, 255, 255) decodes to ~(0, 0, 1), which TBN transforms back
    // to the mesh normal — surfaces without `_n.png` are identical to the
    // pre-bump path. Skipped in AmbientOnly (iso == 4u): ambient is
    // view-independent and no N_bump consumer is active in that mode.
    let iso = uniforms.lighting_isolation;
    var N_bump: vec3<f32> = mesh_n;
    if iso != 4u {
        let n_ts = sample_normal(t_normal, in.uv, ddx, ddy);
        // Degenerate-tangent guard: meshes with collapsed UVs produce zero-length
        // tangents. Skip TBN in that case to avoid NaN propagation.
        const TBN_EPS: f32 = 1.0e-4;
        if dot(in.world_tangent, in.world_tangent) >= TBN_EPS * TBN_EPS {
            // Gram-Schmidt: project out mesh_n component so T stays in the tangent plane.
            let T = normalize(in.world_tangent - mesh_n * dot(in.world_tangent, mesh_n));
            let B = cross(mesh_n, T) * in.bitangent_sign;
            let TBN = mat3x3<f32>(T, B, mesh_n);
            let n_ts_world = TBN * n_ts;
            if dot(n_ts_world, n_ts_world) >= TBN_EPS * TBN_EPS {
                N_bump = normalize(n_ts_world);
            }
        }
    }

    // Lighting isolation mode — enables each contributing term independently
    // for leak/bleed debugging. Values:
    //   0 = Normal             — all terms
    //   1 = NoLightmap         — all terms except static lightmap
    //   2 = DirectOnly         — lightmap + dynamic + specular
    //   3 = IndirectOnly       — SH indirect + specular
    //   4 = AmbientOnly        — ambient floor only
    //   5 = LightmapOnly       — static lightmap (incl. animated atlas)
    //   6 = StaticSHOnly       — static SH indirect only
    //   7 = AnimatedDeltaOnly  — animated SH delta (no separate term yet)
    //   8 = DynamicOnly        — dynamic direct lights only
    //   9 = SpecularOnly       — specular only
    // See `LightingIsolation` in postretro/src/render/mod.rs.
    let use_lightmap = (iso == 0u) || (iso == 2u) || (iso == 5u);
    // Modes 6 and 7 both route through `use_indirect` until Task E adds a
    // separate animated-delta term; mode 7 shows nothing useful intentionally.
    let use_indirect = (iso == 0u) || (iso == 1u) || (iso == 3u) || (iso == 6u);
    let use_specular = (iso == 0u) || (iso == 1u) || (iso == 2u) || (iso == 3u) || (iso == 9u);
    let use_dynamic = (iso == 0u) || (iso == 1u) || (iso == 2u) || (iso == 8u);

    // Force scale to 1.0 in modes that exist to view the indirect term directly
    // (IndirectOnly = 3, StaticSHOnly = 6) so runtime suppression doesn't distort them.
    let indirect_scale = select(uniforms.indirect_scale, 1.0, iso == 3u || iso == 6u);
    var indirect = vec3<f32>(0.0);
    if use_indirect {
        indirect = sample_sh_indirect(in.world_position, N_bump, mesh_n) * indirect_scale;
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
    if use_lightmap {
        // Irradiance + animated atlas filter bilinear (HW linear sampler or the
        // 4-tap fallback, selected by `use_hw_filter`) so baked penumbra ramps
        // read as continuous gradients under magnification. The direction
        // channel below stays on the nearest sampler (octahedral lerp ≠ slerp).
        let lm_irr = sample_lightmap_irradiance(in.lightmap_uv);
        // Pre-shaded Lambert irradiance from the animated compose pre-pass.
        // Uncovered atlas texels are zero so this is safe to add unconditionally.
        let lm_anim = sample_lightmap_animated(in.lightmap_uv);

        // Bumped-Lambert correction: the baker pre-multiplied by mesh-normal NdotL
        // using the dominant incident direction. Divide out mesh NdotL and
        // remultiply with N_bump NdotL to make the static term respond to normal-map
        // detail. lm_anim is not corrected. See normal-maps/ Task 4.
        let dom = decode_lightmap_direction(textureSample(lightmap_direction, lightmap_sampler, in.lightmap_uv));
        let n_dot_l_mesh = max(dot(mesh_n, dom), 0.0);
        let n_dot_l_bump = max(dot(N_bump, dom), 0.0);
        // NDOTL_EPS ~10°: dominant-direction bake is unreliable below ~10° and
        // a tighter epsilon lets the ratio produce brightness pops at near-grazing angles.
        const NDOTL_EPS: f32 = 1.0e-2;
        // Skip correction when irradiance is negligible — dominant direction is
        // unreliable for unlit texels.
        const LM_IRR_EPS: f32 = 1.0e-4;
        let use_correction = dot(lm_irr, lm_irr) >= LM_IRR_EPS * LM_IRR_EPS && n_dot_l_mesh > NDOTL_EPS;
        // Cap at 4.0: prevents unbounded spike when N_bump tilts toward the light
        // on a near-backfacing mesh surface.
        let scale = select(1.0, min(n_dot_l_bump / max(n_dot_l_mesh, NDOTL_EPS), 4.0), use_correction);
        // Both `lm_irr` (baked-tag lights) and `lm_anim` (animated-baked lights)
        // carry their shadow baked in — neither is SDF-multiplied. The animated
        // shadow is occlusion-tested into the weight-map bake (`lm_anim`); the
        // retired runtime SDF factor was double-shadowing it. Shadow-map (enemy)
        // results never run through these factors — they carry their own
        // dynamic-occluder shadow in the dynamic-light loop below.
        static_direct = lm_irr * scale + lm_anim;
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
    // → R/G/B/A via `slice_for_visibility`). Gated by `use_lightmap` so it shows
    // in exactly the direct-static-light isolation modes the baked term does.
    if use_lightmap {
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
            let visibility = select(slice_for_visibility(sdf_factor, s), 1.0, sdf_force_lit);
            static_direct = static_direct + sl.color_and_pad.xyz * (n_dot_l * atten * visibility);
        }
    }

    var total_light = vec3<f32>(uniforms.ambient_floor) + indirect + static_direct;

    var specular_sum = vec3<f32>(0.0);
    if use_specular {
        let V = normalize(uniforms.camera_position - in.world_position);
        let spec_int = sample_color(spec_texture, in.uv, ddx, ddy).r;
        let spec_exp = max(material.shininess, 1.0);

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
            // Specular is shadowed by the light's OWN technique (invariant 9). An
            // `sdf`-tagged light's specular multiplies by the SAME per-light
            // visibility slice its diffuse used — resolved through the shared
            // `sdf_sel` selection so slot/channel line up by construction; the
            // slice is already sampled (`sdf_factor`), so this is near-zero cost
            // and removes specular-through-walls for sdf lights. Non-`sdf`
            // (`static_light_map`) lights' specular stays unshadowed (they carry
            // no runtime visibility; baked = free) — a known limitation — so
            // `sdf_visibility_for_light` returns 1.0 for them. The dev force-lit
            // toggle (and `SdfShadowMode::Off`) forces 1.0, matching the diffuse.
            let is_sdf = sdf_select_is_sdf(sl);
            let visibility = select(
                sdf_visibility_for_light(sdf_sel, sdf_factor, light_idx),
                1.0,
                sdf_force_lit || !is_sdf,
            );
            let contribution = blinn_phong(
                L, V, N_bump, sl.color_and_pad.xyz, spec_exp, spec_int
            ) * (atten * visibility);
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
            let cycle_t = fract(uniforms.time / max(scripted_desc.period, 0.0001) + scripted_desc.phase);
            // Color channel wins when present; otherwise apply brightness to base_color.
            // Clamp non-negative: Catmull-Rom overshoot between keyframes can go
            // below zero, which would make an animated light emit negative,
            // sign-flipped (wrong-colored) light.
            if scripted_desc.color_count > 0u {
                effective_color = max(
                    sample_color_catmull_rom(
                        scripted_desc.color_offset,
                        scripted_desc.color_count,
                        cycle_t,
                        scripted_desc.base_color,
                    ),
                    vec3<f32>(0.0),
                );
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
                effective_aim = sample_animated_direction(scripted_desc, cycle_t, effective_aim);
            }
        }

        var L: vec3<f32>;
        var attenuation: f32;

        switch light_type {
            case 0u: {
                let to_light = light.position_and_type.xyz - in.world_position;
                let dist = length(to_light);
                L = to_light / max(dist, 0.0001);
                attenuation = falloff(dist, light.direction_and_range.w, falloff_model);
            }
            case 1u: {
                let to_light = light.position_and_type.xyz - in.world_position;
                let dist = length(to_light);
                L = to_light / max(dist, 0.0001);
                let dist_falloff = falloff(dist, light.direction_and_range.w, falloff_model);
                let cone = cone_attenuation(
                    L,
                    effective_aim,
                    light.cone_angles_and_pad.x,
                    light.cone_angles_and_pad.y,
                );
                attenuation = dist_falloff * cone;

                let slot_index = bitcast<u32>(light.cone_angles_and_pad.z);
                if slot_index != 0xFFFFFFFFu {
                    let light_proj = light_space_matrices.m[slot_index];
                    let shadow = sample_spot_shadow(slot_index, in.world_position, light_proj);
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

    let rgb = base_color.rgb * total_light;
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
    return vec4<f32>(rgb, base_color.a);
}
