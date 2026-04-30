// Main forward pass — direct lighting via a flat per-fragment light loop
// plus a scalar ambient floor, with baked SH irradiance indirect.
// See: context/lib/rendering_pipeline.md §4

struct Uniforms {
    view_proj: mat4x4<f32>,
    camera_position: vec3<f32>,
    ambient_floor: f32,
    light_count: u32,
    // Elapsed seconds since renderer start. Consumed by SH animated-layer
    // evaluation; wrapping is handled per-light via fract().
    time: f32,
    // Lighting-term isolation for leak/bleed debugging. Cycled by the
    // Alt+Shift+4 diagnostic chord. Values 0..=9 — see fs_main for the full
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
@group(1) @binding(1) var base_sampler: sampler;
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
// Per-material tangent-space normal map. Sampled with `base_sampler`. The
// neutral placeholder is (127, 127, 255, 255) which decodes to ~(0, 0, 1)
// in tangent space, so surfaces with no `_n.png` sibling render identically
// to the mesh-normal path. See context/lib/resource_management.md §4.3.
@group(1) @binding(4) var t_normal: texture_2d<f32>;

@group(2) @binding(0) var<storage, read> lights: array<GpuLight>;
// Per-light influence volume: xyz = sphere center, w = radius.
@group(2) @binding(1) var<storage, read> light_influence: array<vec4<f32>>;

// Spec-only static light buffer. Two vec4 slots (32 B stride); see
// postretro/src/lighting/spec_buffer.rs for the CPU-side layout.
struct SpecLight {
    position_and_range: vec4<f32>, // xyz = position, w = falloff_range
    color_and_pad:      vec4<f32>, // xyz = color × intensity, w = 0
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

// Group 3 — SH irradiance volume. 9 3D textures (one per SH L2 band) carry
// RGB coefficients in their .rgb channels (.a unused). When `grid.has_sh_volume`
// is 0 the bindings point at dummy 1×1×1 textures and the shader skips SH
// sampling. See sub-plan 6 and postretro/src/render/sh_volume.rs.
struct ShGridInfo {
    grid_origin: vec3<f32>,
    has_sh_volume: u32,
    cell_size: vec3<f32>,
    _pad0: u32,
    grid_dimensions: vec3<u32>,
    _pad1: u32,
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

@group(3) @binding(0) var sh_sampler: sampler;
@group(3) @binding(1) var sh_band0: texture_3d<f32>;
@group(3) @binding(2) var sh_band1: texture_3d<f32>;
@group(3) @binding(3) var sh_band2: texture_3d<f32>;
@group(3) @binding(4) var sh_band3: texture_3d<f32>;
@group(3) @binding(5) var sh_band4: texture_3d<f32>;
@group(3) @binding(6) var sh_band5: texture_3d<f32>;
@group(3) @binding(7) var sh_band6: texture_3d<f32>;
@group(3) @binding(8) var sh_band7: texture_3d<f32>;
@group(3) @binding(9) var sh_band8: texture_3d<f32>;
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

// Group 4 — baked directional lightmap (static direct lighting).
// See context/plans/ready/lighting-lightmaps/index.md.
@group(4) @binding(0) var lightmap_irradiance: texture_2d<f32>;
@group(4) @binding(1) var lightmap_direction: texture_2d<f32>;
@group(4) @binding(2) var lightmap_sampler: sampler;
// Animated-light contribution atlas (Rgba16Float). Composed each frame by
// the compute pre-pass in `animated_lightmap.rs` from per-animated-light
// baked weight maps + runtime descriptor curves. `.rgb` carries pre-shaded
// irradiance (Lambert already baked in); `.a` is a coverage flag reserved
// for debug visualization. When the PRL has no animated weight maps, this
// slot binds a 1×1 zero texture so the fragment shader reads 0.
@group(4) @binding(3) var animated_lm_atlas: texture_2d<f32>;

// Group 5 — dynamic spot light shadow maps.
// See context/plans/in-progress/lighting-spot-shadows/index.md § Task B.
@group(5) @binding(0) var spot_shadow_depth: texture_depth_2d_array;
@group(5) @binding(1) var spot_shadow_compare: sampler_comparison;
// Uniform (not storage) so we stay under `max_storage_buffers_per_shader_stage`
// (default limit 8 on some adapters — wgpu refuses the pipeline if we add
// a 9th). 12 × mat4x4<f32> is 768 bytes, well under the 16 KiB uniform cap.
struct LightSpaceMatrices {
    m: array<mat4x4<f32>, 12>,
};
@group(5) @binding(2) var<uniform> light_space_matrices: LightSpaceMatrices;

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

// SH L0..L2 basis evaluation. Constants are standard real SH normalization factors.
// Signs on bands 1, 3, 5, 7 match the signed basis used by the baker
// (postretro-level-compiler/src/sh_bake.rs::sh_basis_l2) — projection and
// reconstruction MUST use the same signed basis, or L1-y / L1-x / L2-yz / L2-xz invert.
//
// The Ramamoorthi-Hanrahan cosine-lobe convolution (A_0=π, A_1=2π/3, A_2=π/4)
// is folded into the baked coefficients at bake time (sh_bake.rs::apply_cosine_lobe_rgb).
// Runtime reconstruction applies only the basis — if indirect looks wrong, suspect
// the baker or upload path, not these constants.
fn sh_irradiance(
    b0: vec3<f32>, b1: vec3<f32>, b2: vec3<f32>, b3: vec3<f32>,
    b4: vec3<f32>, b5: vec3<f32>, b6: vec3<f32>, b7: vec3<f32>, b8: vec3<f32>,
    normal: vec3<f32>,
) -> vec3<f32> {
    let nx = normal.x;
    let ny = normal.y;
    let nz = normal.z;
    var r: vec3<f32> = b0 * 0.282095;                 // L0
    r = r + b1 * (-0.488603 * ny);                    // L1 y  (signed basis)
    r = r + b2 * ( 0.488603 * nz);                    // L1 z
    r = r + b3 * (-0.488603 * nx);                    // L1 x  (signed basis)
    r = r + b4 * ( 1.092548 * nx * ny);               // L2 xy
    r = r + b5 * (-1.092548 * ny * nz);               // L2 yz (signed basis)
    r = r + b6 * ( 0.315392 * (3.0 * nz * nz - 1.0)); // L2 z^2
    r = r + b7 * (-1.092548 * nx * nz);               // L2 xz (signed basis)
    r = r + b8 * ( 0.546274 * (nx * nx - ny * ny));   // L2 x^2 - y^2
    return r;
}

// Hardware-trilinear fetch of all 9 SH bands. `cell_center_uvw` lands between
// the 8 texel centers so trilinear reproduces per-corner weighting exactly —
// one sample per band in lieu of eight manual fetches.
fn sample_sh_indirect_fast(
    normal: vec3<f32>,
    gi: vec3<u32>,
    gfrac: vec3<f32>,
) -> vec3<f32> {
    let gdims_f = max(vec3<f32>(sh_grid.grid_dimensions), vec3<f32>(1.0));
    let cell_center_uvw = (vec3<f32>(gi) + vec3<f32>(0.5) + gfrac) / gdims_f;
    let b0 = textureSampleLevel(sh_band0, sh_sampler, cell_center_uvw, 0.0).rgb;
    let b1 = textureSampleLevel(sh_band1, sh_sampler, cell_center_uvw, 0.0).rgb;
    let b2 = textureSampleLevel(sh_band2, sh_sampler, cell_center_uvw, 0.0).rgb;
    let b3 = textureSampleLevel(sh_band3, sh_sampler, cell_center_uvw, 0.0).rgb;
    let b4 = textureSampleLevel(sh_band4, sh_sampler, cell_center_uvw, 0.0).rgb;
    let b5 = textureSampleLevel(sh_band5, sh_sampler, cell_center_uvw, 0.0).rgb;
    let b6 = textureSampleLevel(sh_band6, sh_sampler, cell_center_uvw, 0.0).rgb;
    let b7 = textureSampleLevel(sh_band7, sh_sampler, cell_center_uvw, 0.0).rgb;
    let b8 = textureSampleLevel(sh_band8, sh_sampler, cell_center_uvw, 0.0).rgb;

    return max(
        sh_irradiance(b0, b1, b2, b3, b4, b5, b6, b7, b8, normal),
        vec3<f32>(0.0),
    );
}

fn sample_sh_indirect(world_pos: vec3<f32>, normal: vec3<f32>) -> vec3<f32> {
    if sh_grid.has_sh_volume == 0u {
        return vec3<f32>(0.0);
    }

    // Bias the lookup toward the lit side by offsetting along the surface
    // normal. Reduces SH bleed across thin walls.
    const SH_NORMAL_OFFSET_M: f32 = 0.1;
    let offset_world = world_pos + normal * SH_NORMAL_OFFSET_M * sh_grid.cell_size;
    let gdims_u = sh_grid.grid_dimensions;
    let gdims_f = max(vec3<f32>(gdims_u) - vec3<f32>(1.0), vec3<f32>(0.0));
    let cell_coord = (offset_world - sh_grid.grid_origin) /
        max(sh_grid.cell_size, vec3<f32>(1.0e-6));
    let gf = clamp(cell_coord, vec3<f32>(0.0), gdims_f);
    let gi = vec3<u32>(floor(gf));
    let gfrac = fract(gf);

    return sample_sh_indirect_fast(normal, gi, gfrac);
}

@fragment
fn fs_main(in: VertexOutput) -> @location(0) vec4<f32> {
    let base_color = textureSample(base_texture, base_sampler, in.uv);

    let mesh_n = normalize(in.world_normal);

    // Tangent-space normal map + TBN construction. The neutral placeholder
    // (127, 127, 255, 255) decodes to ~(0, 0, 1), which TBN transforms back
    // to the mesh normal — surfaces without `_n.png` are identical to the
    // pre-bump path. Skipped in AmbientOnly (iso == 4u): ambient is
    // view-independent and no N_bump consumer is active in that mode.
    let iso = uniforms.lighting_isolation;
    var N_bump: vec3<f32> = mesh_n;
    if iso != 4u {
        let n_ts = textureSample(t_normal, base_sampler, in.uv).rgb * 2.0 - 1.0;
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
        indirect = sample_sh_indirect(in.world_position, N_bump) * indirect_scale;
    }

    // Static direct term: baked directional lightmap. NdotL is already folded
    // in by the baker — sampling gives correct static direct contribution for
    // a mesh-normal surface.
    var static_direct = vec3<f32>(0.0);
    if use_lightmap {
        let lm_irr = textureSample(lightmap_irradiance, lightmap_sampler, in.lightmap_uv).rgb;
        // Pre-shaded Lambert irradiance from the animated compose pre-pass.
        // Uncovered atlas texels are zero so this is safe to add unconditionally.
        let lm_anim = textureSample(animated_lm_atlas, lightmap_sampler, in.lightmap_uv).rgb;

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
        static_direct = lm_irr * scale + lm_anim;
    }

    var total_light = vec3<f32>(uniforms.ambient_floor) + indirect + static_direct;

    var specular_sum = vec3<f32>(0.0);
    if use_specular {
        let V = normalize(uniforms.camera_position - in.world_position);
        let spec_int = textureSample(spec_texture, base_sampler, in.uv).r;
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
            let contribution = blinn_phong(
                L, V, N_bump, sl.color_and_pad.xyz, spec_exp, spec_int
            ) * atten;
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
            if scripted_desc.color_count > 0u {
                effective_color = sample_color_catmull_rom(
                    scripted_desc.color_offset,
                    scripted_desc.color_count,
                    cycle_t,
                    scripted_desc.base_color,
                );
            } else if scripted_desc.brightness_count > 0u {
                let brightness = sample_curve_catmull_rom(
                    scripted_desc.brightness_offset,
                    scripted_desc.brightness_count,
                    cycle_t,
                );
                effective_color = scripted_desc.base_color * brightness;
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
    return vec4<f32>(rgb, base_color.a);
}
