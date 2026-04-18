// Main forward pass — direct lighting via a flat per-fragment light loop
// plus a scalar ambient floor, with shadow map sampling.
// See: context/lib/rendering_pipeline.md §4
//      context/plans/in-progress/lighting-foundation/3-direct-lighting.md
//      context/plans/in-progress/lighting-foundation/5-shadow-maps.md

struct Uniforms {
    view_proj: mat4x4<f32>,
    camera_position: vec3<f32>,
    ambient_floor: f32,
    light_count: u32,
    // Elapsed seconds since renderer start. Consumed by SH animated-layer
    // evaluation (sub-plan 7); wrapping is handled per-light via fract().
    time: f32,
    // 0 = off, 1 = on. Toggled by the Alt+Shift+2 diagnostic chord.
    // When on, fs_main replaces shading with a color derived from the
    // sign of the SDF sampled just inside the fragment surface —
    // diagnoses SDF-bake parity bugs.
    sdf_sign_viz: u32,
    // 0 = off, 1 = on. Toggled by Alt+Shift+3. When on, fs_main renders
    // `sample_sdf(world_pos)` as a grayscale distance ramp (0 m → black,
    // ≥1 m → white), with negative distances tinted red. Used to verify
    // the baked atlas is returning sane values before trusting the sphere
    // tracer that consumes it.
    sdf_distance_viz: u32,
    // CSM cascade split distances (view-space Z far for cascades 0/1/2, w reserved).
    csm_splits: vec4<f32>,
    // View matrix for computing fragment view-space depth for cascade selection.
    view_matrix: mat4x4<f32>,
    // Lighting-term isolation for leak/bleed debugging. Cycled by the
    // Alt+Shift+4 diagnostic chord. Values:
    //   0 = Normal       (direct + indirect + ambient floor — production shading)
    //   1 = DirectOnly   (SH indirect forced to 0)
    //   2 = IndirectOnly (direct-light loop skipped)
    //   3 = AmbientOnly  (both terms skipped; only ambient floor contributes)
    lighting_isolation: u32,
    _pad_lighting_0: u32,
    _pad_lighting_1: u32,
    _pad_lighting_2: u32,
};

// Five vec4<f32> slots — see postretro/src/lighting/mod.rs for field semantics.
struct GpuLight {
    position_and_type: vec4<f32>,
    color_and_falloff_model: vec4<f32>,
    direction_and_range: vec4<f32>,
    cone_angles_and_pad: vec4<f32>,
    shadow_info: vec4<f32>,
};

@group(0) @binding(0) var<uniform> uniforms: Uniforms;

@group(1) @binding(0) var base_texture: texture_2d<f32>;
@group(1) @binding(1) var base_sampler: sampler;

@group(2) @binding(0) var<storage, read> lights: array<GpuLight>;
// Per-light influence volume: xyz = sphere center, w = radius.
@group(2) @binding(1) var<storage, read> light_influence: array<vec4<f32>>;
@group(2) @binding(2) var shadow_sampler: sampler_comparison;
@group(2) @binding(3) var csm_depth_array: texture_depth_2d_array;
@group(2) @binding(4) var<storage, read> csm_view_proj: array<mat4x4<f32>>;

// --- SDF atlas (sub-plan 8) ---
//
// SdfMeta uniform: must match `sdf::SDF_META_SIZE` (64 bytes) and
// `sdf::build_sdf_meta_bytes` field order in postretro/src/render/sdf.rs.
//
// Layout (std140-friendly — all vec3 fields are followed by a same-slot scalar):
//   0..12   world_min            (vec3<f32>)
//   12..16  voxel_size_m         (f32)
//   16..28  world_max            (vec3<f32>)
//   28..32  brick_size_voxels    (u32)
//   32..44  grid_dims            (vec3<u32>)
//   44..48  has_sdf_atlas        (u32, 0 or 1)
//   48..60  atlas_bricks         (vec3<u32>) — bricks-per-axis packed in atlas
//   60..64  _pad                 (u32)
struct SdfMeta {
    world_min: vec3<f32>,
    voxel_size_m: f32,
    world_max: vec3<f32>,
    brick_size_voxels: u32,
    grid_dims: vec3<u32>,
    has_sdf_atlas: u32,
    atlas_bricks: vec3<u32>,
    _pad: u32,
};

@group(2) @binding(5) var sdf_atlas_tex: texture_3d<f32>;
@group(2) @binding(6) var sdf_atlas_sampler: sampler;
@group(2) @binding(7) var<storage, read> sdf_top_level: array<u32>;
@group(2) @binding(8) var<uniform> sdf_meta: SdfMeta;
// Coarse SDF: one texel per brick, trilinearly sampled. Provides a valid
// signed-distance field everywhere in the grid — not just inside SURFACE
// bricks. The sphere tracer falls back to this when the brick at the
// sample position is EMPTY or INTERIOR. Without it the tracer sees
// SDF_LARGE_POS in open-air bricks and jumps the entire light distance in
// one step, missing every occluder it hasn't already entered a SURFACE
// brick of.
@group(2) @binding(9) var sdf_coarse_tex: texture_3d<f32>;

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
    animated_light_count: u32,
};

// Per-light animation descriptor — matches ANIMATION_DESCRIPTOR_SIZE (48 B)
// in postretro/src/render/sh_volume.rs. Field order diverges from the spec
// prose to hit exactly 48 bytes: with the spec's original order, color_count
// ends at byte 44 and _padding: vec2<f32> (AlignOf=8) would be pushed to 48,
// making the struct 56 B and stride 64. Instead we pack four scalars after
// base_color so color_count ends at 36; _padding then lands at 40 (4-byte
// implicit gap at 36..40) and occupies 40..48 for a 48-byte stride.
struct AnimationDescriptor {
    period: f32,
    phase: f32,
    brightness_offset: u32,
    brightness_count: u32,
    base_color: vec3<f32>,
    color_offset: u32,
    color_count: u32,
    _padding: vec2<f32>,
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

// Animation buffers (sub-plan 7). Always bound; the shader guards on
// `sh_grid.animated_light_count == 0` so dummy bindings are never read.
@group(3) @binding(11) var<storage, read> anim_descriptors: array<AnimationDescriptor>;
@group(3) @binding(12) var<storage, read> anim_samples: array<f32>;
@group(3) @binding(13) var<storage, read> anim_sh_data: array<f32>;

struct VertexInput {
    @location(0) position: vec3<f32>,
    @location(1) base_uv: vec2<f32>,
    @location(2) normal_oct: vec2<u32>,
    @location(3) tangent_packed: vec2<u32>,
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
};

// Decode octahedral-encoded u16x2 to unit direction vector.
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

    // Decode octahedral normal.
    out.world_normal = oct_decode(in.normal_oct);

    // Decode packed tangent: strip sign bit from v-component, remap 15-bit to 16-bit.
    let sign_bit = in.tangent_packed.y & 0x8000u;
    let v_15bit = in.tangent_packed.y & 0x7FFFu;
    let v_16bit = v_15bit * 65535u / 32767u;
    out.world_tangent = oct_decode(vec2<u32>(in.tangent_packed.x, v_16bit));
    out.bitangent_sign = select(-1.0, 1.0, sign_bit != 0u);

    return out;
}

// --- Falloff models ---
fn falloff(distance: f32, range: f32, model: u32) -> f32 {
    switch model {
        case 0u: {
            return max(1.0 - distance / max(range, 0.001), 0.0);
        }
        case 1u: {
            if distance > range {
                return 0.0;
            }
            return 1.0 / max(distance, 0.001);
        }
        case 2u: {
            if distance > range {
                return 0.0;
            }
            let d2 = max(distance * distance, 0.001);
            return 1.0 / d2;
        }
        default: {
            return 0.0;
        }
    }
}

// --- Spot cone attenuation ---
fn cone_attenuation(L: vec3<f32>, aim: vec3<f32>, inner_angle: f32, outer_angle: f32) -> f32 {
    let cos_angle = dot(-L, aim);
    let cos_inner = cos(inner_angle);
    let cos_outer = cos(outer_angle);
    return smoothstep(cos_outer, cos_inner, cos_angle);
}

// --- SDF atlas sampling ---

// Sentinel values matching BRICK_SLOT_EMPTY / BRICK_SLOT_INTERIOR in
// postretro-level-format/src/sdf_atlas.rs. u32::MAX and u32::MAX-1.
const SDF_SLOT_EMPTY: u32 = 0xFFFFFFFFu;
const SDF_SLOT_INTERIOR: u32 = 0xFFFFFFFEu;

// Large positive / negative distance for sentinel bricks (meters).
const SDF_LARGE_POS: f32 = 1.0e6;
const SDF_LARGE_NEG: f32 = -1.0e6;

// Sphere-trace march cap. Starting constant per sub-plan 8 spec (32);
// tune on real levels once visible-light count + budget are measured.
const SDF_MAX_STEPS: i32 = 32;

// Sample the coarse per-brick SDF texture at a world-space position using
// trilinear interpolation. The texture has one texel per brick, with texel
// centers at brick centers (world_min + (i + 0.5) * brick_m on each axis).
// Returns a signed distance in meters, or SDF_LARGE_POS when the SDF atlas
// isn't loaded or `world_pos` lies outside the brick grid.
//
// Out-of-grid guard: ClampToEdge would return the nearest boundary texel's
// value, which can be negative if that brick is INTERIOR — the sphere tracer
// would interpret that as a surface hit and fully occlude the light. Callers
// like the distance-viz probe that offset along the normal can easily land
// outside the grid, so we check here rather than trusting every caller.
//
// Known trade-off: across a SURFACE/EMPTY brick boundary the trilinear
// interpolation can over-estimate step distance (mixing a SURFACE brick
// center's small distance with an EMPTY neighbor's larger one). In practice
// the tracer catches the occluder on the next step once it lands in the
// SURFACE brick — worth profiling if shadow quality regresses.
fn sample_coarse_sdf(world_pos: vec3<f32>) -> f32 {
    if sdf_meta.has_sdf_atlas == 0u {
        return SDF_LARGE_POS;
    }
    let brick_m = sdf_meta.voxel_size_m * f32(sdf_meta.brick_size_voxels);
    let grid_extent = vec3<f32>(sdf_meta.grid_dims) * brick_m;
    let rel = world_pos - sdf_meta.world_min;
    if any(rel < vec3<f32>(0.0)) || any(rel > grid_extent) {
        return SDF_LARGE_POS;
    }
    let uv = rel / grid_extent;
    return textureSample(sdf_coarse_tex, sdf_atlas_sampler, uv).r;
}

// Sample the SDF atlas at a world-space position. Returns a signed distance
// in meters. Positive = outside geometry, negative = inside.
//
// Two-tier sampling:
//   - SURFACE brick → fine trilinear sample of the per-voxel atlas.
//   - EMPTY / INTERIOR brick → trilinear sample of the coarse per-brick
//     distance texture. This gives a valid SDF everywhere in the grid so the
//     sphere tracer can detect and step toward distant surfaces; without it
//     open-air bricks would return a huge-positive sentinel and the tracer
//     would jump past every occluder in a single step.
//
// Returns SDF_LARGE_POS when the SDF atlas is absent or the position is
// outside the world AABB (atlas samples outside the grid are meaningless).
fn sample_sdf(world_pos: vec3<f32>) -> f32 {
    if sdf_meta.has_sdf_atlas == 0u {
        return SDF_LARGE_POS;
    }
    // Check if position is inside the world AABB.
    if any(world_pos < sdf_meta.world_min) || any(world_pos > sdf_meta.world_max) {
        return SDF_LARGE_POS;
    }

    let brick_m = sdf_meta.voxel_size_m * f32(sdf_meta.brick_size_voxels);
    let rel = world_pos - sdf_meta.world_min;
    let brick_coord = vec3<i32>(floor(rel / brick_m));
    let gd = vec3<i32>(sdf_meta.grid_dims);

    // Clamp to valid brick grid.
    if any(brick_coord < vec3<i32>(0)) || any(brick_coord >= gd) {
        return SDF_LARGE_POS;
    }

    let bx = u32(brick_coord.x);
    let by = u32(brick_coord.y);
    let bz = u32(brick_coord.z);
    let gx = sdf_meta.grid_dims.x;
    let gy = sdf_meta.grid_dims.y;
    let flat_idx = bz * gy * gx + by * gx + bx;
    let slot = sdf_top_level[flat_idx];

    if slot == SDF_SLOT_EMPTY || slot == SDF_SLOT_INTERIOR {
        return sample_coarse_sdf(world_pos);
    }

    // Surface brick: trilinear sample from the atlas.
    // Atlas layout: bricks are packed in a 3D grid `sdf_meta.atlas_bricks`
    // within the texture. Slot `s` maps to brick coords
    //   (s % ax, (s / ax) % ay, s / (ax*ay))
    // and occupies voxel range [brick_coord*brick_size, (+1)*brick_size) on
    // each axis. Must match the CPU packing in `sdf::SdfResources::build`.
    let bsv = sdf_meta.brick_size_voxels;
    let bsv_f = f32(bsv);
    let ax = sdf_meta.atlas_bricks.x;
    let ay = sdf_meta.atlas_bricks.y;
    let bxa = slot % ax;
    let bya = (slot / ax) % ay;
    let bza = slot / (ax * ay);

    let brick_origin = sdf_meta.world_min + vec3<f32>(vec3<u32>(bx, by, bz)) * brick_m;
    let local = world_pos - brick_origin;
    // Half-texel clamp on ALL axes: with 3D packing every axis has brick
    // neighbors, so without the clamp trilinear interpolation would bleed
    // into a neighbor brick's voxel (unrelated memory).
    let local_n = local / brick_m; // [0,1] within the brick along each axis
    let voxel_pos = clamp(
        local_n * bsv_f,
        vec3<f32>(0.5),
        vec3<f32>(bsv_f - 0.5),
    );
    let brick_atlas = vec3<f32>(f32(bxa), f32(bya), f32(bza));
    let tex_dims = vec3<f32>(textureDimensions(sdf_atlas_tex));
    let tex_uv = (brick_atlas * bsv_f + voxel_pos) / tex_dims;

    return textureSample(sdf_atlas_tex, sdf_atlas_sampler, tex_uv).r;
}

// Inigo Quilez sphere-tracing soft shadow (sub-plan 8 spec).
// Traces from `frag_pos` toward `light_pos`. Returns a [0..1] shadow factor
// (0 = fully occluded, 1 = fully lit). `cone_half_angle` controls softness.
fn sample_sdf_shadow(
    frag_pos: vec3<f32>,
    light_pos: vec3<f32>,
    cone_half_angle: f32,
) -> f32 {
    if sdf_meta.has_sdf_atlas == 0u {
        return 1.0;
    }

    let to_light = light_pos - frag_pos;
    let light_dist = length(to_light);
    if light_dist < 0.001 {
        return 1.0;
    }
    let ray_dir = to_light / light_dist;

    let k = 1.0 / max(tan(cone_half_angle), 0.001);
    let self_shadow_bias = sdf_meta.voxel_size_m * 2.0;
    let self_shadow_epsilon = sdf_meta.voxel_size_m * 0.25;
    let min_step = sdf_meta.voxel_size_m * 0.5;

    var t: f32 = self_shadow_bias;
    var occlusion: f32 = 1.0;

    for (var i: i32 = 0; i < SDF_MAX_STEPS; i = i + 1) {
        if t >= light_dist {
            break;
        }
        let p = frag_pos + ray_dir * t;
        let d = sample_sdf(p);
        if d < self_shadow_epsilon {
            occlusion = 0.0;
            break;
        }
        occlusion = min(occlusion, k * d / t);
        t = t + max(d, min_step);
    }

    return clamp(occlusion, 0.0, 1.0);
}

// --- Shadow sampling ---

// Sample CSM shadow map for a directional light.
fn sample_csm_shadow(frag_world_pos: vec3<f32>, shadow_map_index: u32) -> f32 {
    // Compute view-space depth for cascade selection.
    let view_pos = uniforms.view_matrix * vec4<f32>(frag_world_pos, 1.0);
    let view_depth = -view_pos.z; // RH: view-space Z is negative in front of camera.

    // Select the tightest cascade that contains this fragment.
    var cascade: u32 = 0u;
    if view_depth > uniforms.csm_splits.x {
        cascade = 1u;
    }
    if view_depth > uniforms.csm_splits.y {
        cascade = 2u;
    }

    let cascade_index = shadow_map_index * 3u + cascade;
    let vp = csm_view_proj[cascade_index];
    let light_space_pos = vp * vec4<f32>(frag_world_pos, 1.0);
    let ndc = light_space_pos.xyz / light_space_pos.w;
    let shadow_uv = ndc.xy * 0.5 + 0.5;
    // Flip Y: NDC Y increases upward, texture V increases downward.
    let uv = vec2<f32>(shadow_uv.x, 1.0 - shadow_uv.y);
    let depth = ndc.z;

    // Reject fragments outside the shadow map UV range.
    if uv.x < 0.0 || uv.x > 1.0 || uv.y < 0.0 || uv.y > 1.0 {
        return 1.0; // Lit (no shadow data).
    }

    return textureSampleCompareLevel(csm_depth_array, shadow_sampler, uv, cascade_index, depth);
}

// Shadow kinds other than CSM (e.g. `shadow_kind == 2` — reserved for
// sub-plan 9's SDF sphere-trace — and any unknown value) fall through to
// unshadowed (factor 1.0) in the fragment main until sub-plan 9 lands.

// --- SH visibility-weighted sampling constants (sub-plan 10) ---
//
// The SH irradiance volume's trilinear lookup is wall-blind: probes on both
// sides of a thin wall contribute to every fragment whose cell cube straddles
// the wall. These constants drive the fix from sub-plan 10 §"Fix B" (normal
// offset) and §"Fix A" (SDF-weighted trilinear). All bias / step / epsilon
// values are expressed as voxel multiples of `sdf_meta.voxel_size_m`, matching
// `sample_sdf_shadow`'s tuning pattern — retuning the SDF voxel size
// automatically retunes these.
//
// Fix B: push the sample point off the surface before the grid lookup, so
// fragments sitting on a wall don't get equal weight from probes on the
// exterior side. Expressed as a fraction of the probe cell size (not an
// absolute meter value) because the offset's job is to clear the
// probe-interpolation footprint — the relevant length scale is the SH cell
// size, not the SDF voxel size (10× smaller at current settings). 10% is
// small enough that concave-corner fragments don't offset through a
// perpendicular wall, large enough to move the lookup cleanly off the
// surface.
const SH_NORMAL_OFFSET_CELLS: f32 = 0.1;

// Fix A: per-corner sphere trace budget and softness.
const SH_VIS_MAX_STEPS: u32 = 12u;               // segments are at most one cell diagonal
const SH_VIS_SELF_BIAS_VOXELS: f32 = 0.5;
const SH_VIS_MIN_STEP_VOXELS: f32 = 0.5;
const SH_VIS_HIT_EPSILON_VOXELS: f32 = 0.75;
const SH_VIS_MIN_DIST: f32 = 0.01;                // skip degenerate same-point case
const SH_VIS_SOFTNESS: f32 = 4.0;
const SH_VIS_WEIGHT_EPS: f32 = 1.0e-4;

// If the SDF reports the offset sample point is farther from any surface
// than this many cell-sizes, every corner of the 8-probe cube is fully
// visible — no trace can possibly hit an occluder. Slightly above √3 ≈
// 1.732 (the worst-case cell diagonal in cells). Used by the fast path
// to skip 8 sphere traces and fall back to hardware trilinear.
const SH_VIS_SKIP_DIST_CELLS: f32 = 1.75;

// Visibility weight for one corner probe. Returns 1.0 if the corner is
// fully visible from `from`, 0.0 if the SDF reports the segment is blocked,
// and smoothly in between (Inigo Quilez soft-shadow formula — same one
// `sample_sdf_shadow` uses, so direct and indirect occlusion agree).
fn sh_corner_visibility(origin: vec3<f32>, corner_world: vec3<f32>) -> f32 {
    if sdf_meta.has_sdf_atlas == 0u {
        return 1.0;
    }
    let delta = corner_world - origin;
    let dist = length(delta);
    if dist < SH_VIS_MIN_DIST {
        return 1.0;
    }
    let dir = delta / dist;
    var t: f32 = sdf_meta.voxel_size_m * SH_VIS_SELF_BIAS_VOXELS;
    var occlusion: f32 = 1.0;
    let k = SH_VIS_SOFTNESS;
    let hit_eps = sdf_meta.voxel_size_m * SH_VIS_HIT_EPSILON_VOXELS;
    let min_step = sdf_meta.voxel_size_m * SH_VIS_MIN_STEP_VOXELS;
    for (var i: u32 = 0u; i < SH_VIS_MAX_STEPS; i = i + 1u) {
        if t > dist {
            break;
        }
        let p = origin + dir * t;
        let d = sample_sdf(p);
        if d < hit_eps {
            occlusion = 0.0;
            break;
        }
        occlusion = min(occlusion, k * d / t);
        t = t + max(d, min_step);
    }
    return clamp(occlusion, 0.0, 1.0);
}

// --- SH irradiance volume sampling ---
//
// Constants are the standard real spherical harmonic L0..L2 basis
// normalization factors. Signs on bands 1, 3, 5, 7 match the signed basis
// used by the baker (postretro-level-compiler/src/sh_bake.rs::sh_basis_l2) —
// projection and reconstruction MUST use the same signed basis, or the
// L1-y / L1-x / L2-yz / L2-xz terms invert.
//
// The Ramamoorthi-Hanrahan cosine-lobe convolution (A_0=π, A_1=2π/3, A_2=π/4)
// is folded into the baked coefficients at bake time — see
// sh_bake.rs::apply_cosine_lobe_rgb. Runtime reconstruction applies only the
// basis. If the indirect looks wrong, the bug is in the baker or the upload
// path, not these constants. See sub-plan 6 §"Notes for implementation".
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

// Evaluate an animated light's current brightness by linearly interpolating
// its brightness samples over its period, with wrap-around. Returns 1.0
// when the light has no brightness animation.
fn eval_animated_brightness(desc: AnimationDescriptor, cycle_t: f32) -> f32 {
    if desc.brightness_count == 0u {
        return 1.0;
    }
    let sample_pos = cycle_t * f32(desc.brightness_count);
    let idx0 = u32(floor(sample_pos)) % desc.brightness_count;
    let idx1 = (idx0 + 1u) % desc.brightness_count;
    let frac_t = fract(sample_pos);
    return mix(
        anim_samples[desc.brightness_offset + idx0],
        anim_samples[desc.brightness_offset + idx1],
        frac_t,
    );
}

// Evaluate an animated light's current color. Falls back to `base_color`
// when `color_count == 0`.
fn eval_animated_color(desc: AnimationDescriptor, cycle_t: f32) -> vec3<f32> {
    if desc.color_count == 0u {
        return desc.base_color;
    }
    let sample_pos = cycle_t * f32(desc.color_count);
    let idx0 = u32(floor(sample_pos)) % desc.color_count;
    let idx1 = (idx0 + 1u) % desc.color_count;
    let frac_t = fract(sample_pos);
    let off0 = desc.color_offset + idx0 * 3u;
    let off1 = desc.color_offset + idx1 * 3u;
    let c0 = vec3<f32>(
        anim_samples[off0],
        anim_samples[off0 + 1u],
        anim_samples[off0 + 2u],
    );
    let c1 = vec3<f32>(
        anim_samples[off1],
        anim_samples[off1 + 1u],
        anim_samples[off1 + 2u],
    );
    return mix(c0, c1, frac_t);
}

// Accumulate all 9 SH bands of one animated light in a single 8-corner
// traversal, using precomputed final (tri * visibility / weight_sum) weights.
//
// Replaces the earlier per-band `sample_anim_mono_band` helper. The animated
// buffer layout stores the 9 bands of one probe contiguously
// (`anim_sh_data[(base + probe_idx) * 9 + band]`); iterating bands inside the
// corner loop reads those 9 floats in order, giving a contiguous cache line
// per corner. The previous outer-band-inner-corner layout strided by 9 floats
// between reads, wasting cache.
//
// `final_weights[slot]` must equal `tri_weight * corner_vis / weight_sum` —
// normalization folded in so the per-band division the old helper performed
// is gone. Caller guarantees `weight_sum >= SH_VIS_WEIGHT_EPS` before
// computing `final_weights`.
//
// Out-parameter: WGSL can't cleanly return `array<vec3<f32>, 9>` from a
// function in all targets, and the animated buffer is mono (f32 per band) —
// the vec3 math happens only when the caller multiplies by the color
// modulate. We emit scalar accumulators via a ptr<function, _> and let the
// caller do the modulate multiply and vec3 promotion.
fn accumulate_anim_all_bands(
    light_idx: u32,
    gi: vec3<u32>,
    final_weights: array<f32, 8>,
    accum: ptr<function, array<f32, 9>>,
) {
    let gx = sh_grid.grid_dimensions.x;
    let gy = sh_grid.grid_dimensions.y;
    let gz = sh_grid.grid_dimensions.z;
    let probe_count = gx * gy * gz;
    let base_offset = light_idx * probe_count;

    for (var dz: u32 = 0u; dz < 2u; dz = dz + 1u) {
        for (var dy: u32 = 0u; dy < 2u; dy = dy + 1u) {
            for (var dx: u32 = 0u; dx < 2u; dx = dx + 1u) {
                let cx = min(gi.x + dx, gx - 1u);
                let cy = min(gi.y + dy, gy - 1u);
                let cz = min(gi.z + dz, gz - 1u);
                let probe_idx = (cz * gy + cy) * gx + cx;
                let slot = dz * 4u + dy * 2u + dx;
                let w = final_weights[slot];
                let probe_base = (base_offset + probe_idx) * 9u;
                // Contiguous 9-float read per corner.
                for (var band: u32 = 0u; band < 9u; band = band + 1u) {
                    (*accum)[band] = (*accum)[band] + w * anim_sh_data[probe_base + band];
                }
            }
        }
    }
}

// Fast path: hardware-trilinear fetch of all 9 SH bands, plus the animated
// layers with plain trilinear weights. Used when we're far enough from any
// surface (per a single SDF probe at the offset point) that no corner trace
// can hit an occluder — saves 72 textureSampleLevel fetches + 8 sphere
// traces vs. the full path.
fn sample_sh_indirect_fast(
    offset_world: vec3<f32>,
    normal: vec3<f32>,
    gi: vec3<u32>,
    gfrac: vec3<f32>,
) -> vec3<f32> {
    // Hardware trilinear on the base SH textures. UVW computed from the
    // offset world position in [0, 1] texture space.
    let gdims_f = max(vec3<f32>(sh_grid.grid_dimensions), vec3<f32>(1.0));
    let cell_center_uvw = (vec3<f32>(gi) + vec3<f32>(0.5) + gfrac) / gdims_f;
    // `cell_center_uvw` lands between the 8 texel centers, so hardware
    // trilinear reproduces the per-corner weighting exactly — one sample
    // per band in lieu of eight manual fetches.
    var b0 = textureSampleLevel(sh_band0, sh_sampler, cell_center_uvw, 0.0).rgb;
    var b1 = textureSampleLevel(sh_band1, sh_sampler, cell_center_uvw, 0.0).rgb;
    var b2 = textureSampleLevel(sh_band2, sh_sampler, cell_center_uvw, 0.0).rgb;
    var b3 = textureSampleLevel(sh_band3, sh_sampler, cell_center_uvw, 0.0).rgb;
    var b4 = textureSampleLevel(sh_band4, sh_sampler, cell_center_uvw, 0.0).rgb;
    var b5 = textureSampleLevel(sh_band5, sh_sampler, cell_center_uvw, 0.0).rgb;
    var b6 = textureSampleLevel(sh_band6, sh_sampler, cell_center_uvw, 0.0).rgb;
    var b7 = textureSampleLevel(sh_band7, sh_sampler, cell_center_uvw, 0.0).rgb;
    var b8 = textureSampleLevel(sh_band8, sh_sampler, cell_center_uvw, 0.0).rgb;

    // Animated layers: same plain-trilinear weights (all corners fully
    // visible, weight_sum == 1 by construction, so final_weights = tri_w).
    let anim_count = sh_grid.animated_light_count;
    if anim_count != 0u {
        var tri_w: array<f32, 8>;
        for (var dz: u32 = 0u; dz < 2u; dz = dz + 1u) {
            for (var dy: u32 = 0u; dy < 2u; dy = dy + 1u) {
                for (var dx: u32 = 0u; dx < 2u; dx = dx + 1u) {
                    let tri = vec3<f32>(
                        select(1.0 - gfrac.x, gfrac.x, dx == 1u),
                        select(1.0 - gfrac.y, gfrac.y, dy == 1u),
                        select(1.0 - gfrac.z, gfrac.z, dz == 1u),
                    );
                    tri_w[dz * 4u + dy * 2u + dx] = tri.x * tri.y * tri.z;
                }
            }
        }
        for (var i: u32 = 0u; i < anim_count; i = i + 1u) {
            let desc = anim_descriptors[i];
            let cycle_t = fract(uniforms.time / max(desc.period, 1.0e-6) + desc.phase);
            let brightness = eval_animated_brightness(desc, cycle_t);
            let color = eval_animated_color(desc, cycle_t);
            let modulate = color * brightness;

            var accum: array<f32, 9>;
            for (var band: u32 = 0u; band < 9u; band = band + 1u) {
                accum[band] = 0.0;
            }
            accumulate_anim_all_bands(i, gi, tri_w, &accum);
            b0 = b0 + accum[0] * modulate;
            b1 = b1 + accum[1] * modulate;
            b2 = b2 + accum[2] * modulate;
            b3 = b3 + accum[3] * modulate;
            b4 = b4 + accum[4] * modulate;
            b5 = b5 + accum[5] * modulate;
            b6 = b6 + accum[6] * modulate;
            b7 = b7 + accum[7] * modulate;
            b8 = b8 + accum[8] * modulate;
        }
    }

    return max(
        sh_irradiance(b0, b1, b2, b3, b4, b5, b6, b7, b8, normal),
        vec3<f32>(0.0),
    );
}

fn sample_sh_indirect(world_pos: vec3<f32>, normal: vec3<f32>) -> vec3<f32> {
    if sh_grid.has_sh_volume == 0u {
        return vec3<f32>(0.0);
    }

    // Sub-plan 10 Fix B: push the sample point off the surface before the
    // grid lookup. Without this, a fragment on the interior face of a wall
    // lies *on* the wall, and trilinear weights pull equally from probes
    // on both sides. Offsetting by the normal biases the fetch toward the
    // interior side. Offset scales with the probe cell size (see constant
    // comment) — use the smallest axis so anisotropic cell dimensions
    // don't over-offset on the short axis.
    let cell_min = min(
        sh_grid.cell_size.x,
        min(sh_grid.cell_size.y, sh_grid.cell_size.z),
    );
    let offset_world = world_pos + normal * (cell_min * SH_NORMAL_OFFSET_CELLS);

    let gdims_u = sh_grid.grid_dimensions;
    let gdims_f = max(vec3<f32>(gdims_u) - vec3<f32>(1.0), vec3<f32>(0.0));
    let cell_coord = (offset_world - sh_grid.grid_origin) /
        max(sh_grid.cell_size, vec3<f32>(1.0e-6));
    let gf = clamp(cell_coord, vec3<f32>(0.0), gdims_f);
    let gi = vec3<u32>(floor(gf));
    let gfrac = fract(gf);

    // Fast path A: no SDF atlas → every corner visibility weight is 1.0 —
    // skip the 8 sphere traces and the 72 manual texture fetches, use
    // hardware trilinear.
    // Fast path B: the fragment's unclamped cell_coord is outside the grid
    // on some axis — the cell cube is degenerate and visibility weights are
    // meaningless. Falling through to the clamped hardware trilinear is the
    // cleanest degradation.
    // Fast path C: single SDF probe at offset_world reports distance >
    // √3 cell_size (rounded up to SH_VIS_SKIP_DIST_CELLS). No corner trace
    // could possibly hit, because the corner set is contained in a cube of
    // diagonal √3 × cell_size centred on the sample point.
    let max_cell = max(
        sh_grid.cell_size.x,
        max(sh_grid.cell_size.y, sh_grid.cell_size.z),
    );
    let skip_dist = max_cell * SH_VIS_SKIP_DIST_CELLS;
    let out_of_grid = any(cell_coord < vec3<f32>(0.0)) ||
                      any(cell_coord > vec3<f32>(gdims_u) - vec3<f32>(1.0));
    if sdf_meta.has_sdf_atlas == 0u || out_of_grid {
        return sample_sh_indirect_fast(offset_world, normal, gi, gfrac);
    }
    let probe_d = sample_sdf(offset_world);
    if probe_d > skip_dist {
        return sample_sh_indirect_fast(offset_world, normal, gi, gfrac);
    }

    let gx = gdims_u.x;
    let gy = gdims_u.y;
    let gz = gdims_u.z;
    let dims_f = max(vec3<f32>(gdims_u), vec3<f32>(1.0));

    // Sub-plan 10 Fix A: manual 8-corner fetch, each corner weighted by
    // its trilinear weight times its SDF visibility from `offset_world`.
    var band_accum: array<vec3<f32>, 9>;
    for (var b: u32 = 0u; b < 9u; b = b + 1u) {
        band_accum[b] = vec3<f32>(0.0);
    }
    var tri_w_arr: array<f32, 8>;
    var corner_vis: array<f32, 8>;
    var weight_sum: f32 = 0.0;

    for (var dz: u32 = 0u; dz < 2u; dz = dz + 1u) {
        for (var dy: u32 = 0u; dy < 2u; dy = dy + 1u) {
            for (var dx: u32 = 0u; dx < 2u; dx = dx + 1u) {
                let cx = min(gi.x + dx, gx - 1u);
                let cy = min(gi.y + dy, gy - 1u);
                let cz = min(gi.z + dz, gz - 1u);

                let tri = vec3<f32>(
                    select(1.0 - gfrac.x, gfrac.x, dx == 1u),
                    select(1.0 - gfrac.y, gfrac.y, dy == 1u),
                    select(1.0 - gfrac.z, gfrac.z, dz == 1u),
                );
                let tri_w = tri.x * tri.y * tri.z;

                // Corner world position uses the *unclamped* corner so cell-
                // boundary corners don't collapse to the same trace point (two
                // of the 8 corners would otherwise duplicate at the edge of
                // the grid, wasting a trace and biasing the weight sum).
                let corner_world = sh_grid.grid_origin +
                    vec3<f32>(f32(gi.x + dx), f32(gi.y + dy), f32(gi.z + dz)) *
                    sh_grid.cell_size;
                // Texture UVW uses the *clamped* corner so the fetch stays
                // in-bounds (clamp-to-edge semantics) — the probe index the
                // animated buffer reads is the same clamped index.
                let vis = sh_corner_visibility(offset_world, corner_world);
                let slot = dz * 4u + dy * 2u + dx;
                tri_w_arr[slot] = tri_w;
                corner_vis[slot] = vis;
                let w = tri_w * vis;
                weight_sum = weight_sum + w;

                // Point-sample each band at the corner probe's texel center.
                // `textureSampleLevel` with explicit mip 0 avoids the
                // derivative-based LOD selection `textureSample` would do.
                // The SH textures have a single mip so the effective LOD is
                // 0 anyway; being explicit sidesteps any "derivative inside
                // non-uniform control flow" concerns a compiler might flag
                // if this code is ever reused under a dynamic branch.
                let uvw = (vec3<f32>(f32(cx), f32(cy), f32(cz)) + vec3<f32>(0.5)) / dims_f;
                band_accum[0] = band_accum[0] +
                    w * textureSampleLevel(sh_band0, sh_sampler, uvw, 0.0).rgb;
                band_accum[1] = band_accum[1] +
                    w * textureSampleLevel(sh_band1, sh_sampler, uvw, 0.0).rgb;
                band_accum[2] = band_accum[2] +
                    w * textureSampleLevel(sh_band2, sh_sampler, uvw, 0.0).rgb;
                band_accum[3] = band_accum[3] +
                    w * textureSampleLevel(sh_band3, sh_sampler, uvw, 0.0).rgb;
                band_accum[4] = band_accum[4] +
                    w * textureSampleLevel(sh_band4, sh_sampler, uvw, 0.0).rgb;
                band_accum[5] = band_accum[5] +
                    w * textureSampleLevel(sh_band5, sh_sampler, uvw, 0.0).rgb;
                band_accum[6] = band_accum[6] +
                    w * textureSampleLevel(sh_band6, sh_sampler, uvw, 0.0).rgb;
                band_accum[7] = band_accum[7] +
                    w * textureSampleLevel(sh_band7, sh_sampler, uvw, 0.0).rgb;
                band_accum[8] = band_accum[8] +
                    w * textureSampleLevel(sh_band8, sh_sampler, uvw, 0.0).rgb;
            }
        }
    }

    // Full-occlusion path: every corner is blocked. Return 0 and let the
    // caller's ambient floor carry the baseline. Any other choice
    // (nearest-visible probe, reflect into interior SDF) is future work.
    if weight_sum < SH_VIS_WEIGHT_EPS {
        return vec3<f32>(0.0);
    }
    let inv_w = 1.0 / weight_sum;
    var b0 = band_accum[0] * inv_w;
    var b1 = band_accum[1] * inv_w;
    var b2 = band_accum[2] * inv_w;
    var b3 = band_accum[3] * inv_w;
    var b4 = band_accum[4] * inv_w;
    var b5 = band_accum[5] * inv_w;
    var b6 = band_accum[6] * inv_w;
    var b7 = band_accum[7] * inv_w;
    var b8 = band_accum[8] * inv_w;

    // Precompute final normalized corner weights once (tri * vis / weight_sum)
    // — hoists the per-band division the earlier animated-helper did inside
    // its band loop.
    var final_weights: array<f32, 8>;
    for (var s: u32 = 0u; s < 8u; s = s + 1u) {
        final_weights[s] = tri_w_arr[s] * corner_vis[s] * inv_w;
    }

    // Animated-light contributions. SH is linear in its coefficients, so a
    // single reconstruction pass suffices regardless of light count. Reuse
    // the per-corner visibilities — no new SDF traces per animated light.
    let anim_count = sh_grid.animated_light_count;
    if anim_count != 0u {
        for (var i: u32 = 0u; i < anim_count; i = i + 1u) {
            let desc = anim_descriptors[i];
            let cycle_t = fract(uniforms.time / max(desc.period, 1.0e-6) + desc.phase);
            let brightness = eval_animated_brightness(desc, cycle_t);
            let color = eval_animated_color(desc, cycle_t);
            let modulate = color * brightness;

            var accum: array<f32, 9>;
            for (var band: u32 = 0u; band < 9u; band = band + 1u) {
                accum[band] = 0.0;
            }
            accumulate_anim_all_bands(i, gi, final_weights, &accum);
            b0 = b0 + accum[0] * modulate;
            b1 = b1 + accum[1] * modulate;
            b2 = b2 + accum[2] * modulate;
            b3 = b3 + accum[3] * modulate;
            b4 = b4 + accum[4] * modulate;
            b5 = b5 + accum[5] * modulate;
            b6 = b6 + accum[6] * modulate;
            b7 = b7 + accum[7] * modulate;
            b8 = b8 + accum[8] * modulate;
        }
    }

    // Clamp negative irradiance to zero per channel — SH L2 can ring for
    // sharp transitions (Gibbs). Standard practice; see sub-plan 6.
    return max(
        sh_irradiance(b0, b1, b2, b3, b4, b5, b6, b7, b8, normal),
        vec3<f32>(0.0),
    );
}

@fragment
fn fs_main(in: VertexOutput) -> @location(0) vec4<f32> {
    let base_color = textureSample(base_texture, base_sampler, in.uv);
    let N = normalize(in.world_normal);

    // SDF-sign debug viz (Alt+Shift+2). Sample a few voxels *inside* the
    // surface along -N — a correctly-baked atlas should report a negative
    // SDF there. Red = positive (parity bug, ray would tunnel through);
    // Green = negative (correct interior); Blue = near-zero surface brick
    // band. No SDF atlas → magenta so the viz state is still obvious.
    if uniforms.sdf_sign_viz != 0u {
        if sdf_meta.has_sdf_atlas == 0u {
            return vec4<f32>(1.0, 0.0, 1.0, 1.0);
        }
        // Probe 0.5 voxels (4 cm at default 8 cm voxels) inside the surface.
        // Deeper probes pierce thin walls and land in exterior-empty bricks on
        // the far side, showing up as false red. 0.5 × voxel clears the
        // boundary voxel while staying inside any wall thicker than ~8 cm.
        let probe = in.world_position - N * (sdf_meta.voxel_size_m * 0.5);
        let d = sample_sdf(probe);
        if d >= 1.0e5 {
            // SDF_SLOT_EMPTY reached — interior brick misclassified as void.
            return vec4<f32>(1.0, 0.0, 0.0, 1.0);
        }
        if d <= -1.0e5 {
            // SDF_SLOT_INTERIOR — explicitly marked inside.
            return vec4<f32>(0.0, 1.0, 0.0, 1.0);
        }
        // Surface-brick band: map [-voxel, +voxel] to blue→red gradient.
        let norm = clamp(d / sdf_meta.voxel_size_m, -1.0, 1.0);
        if norm < 0.0 {
            // Negative side: blue→green as we get further inside.
            return vec4<f32>(0.0, -norm, 1.0 + norm, 1.0);
        } else {
            // Positive side: blue→red as we get further outside.
            return vec4<f32>(norm, 0.0, 1.0 - norm, 1.0);
        }
    }

    // SDF distance debug viz (Alt+Shift+3). Probes the SDF 0.5 m outward
    // from the surface along N — sampling *at* the surface returns d ≈ 0
    // everywhere (useless) because that's the definition of an SDF. This
    // offset reveals the gradient in open space, which is what the sphere
    // tracer actually consumes.
    //
    // Expected: distance to the *next* nearby surface. On an open floor
    // with nothing above, the sample sits 0.5 m above the floor in empty
    // space — if the ceiling is far, d ≈ 0.5 m (mid-gray). Near a wall,
    // d drops toward 0 (darker). If every surface reads black regardless
    // of what's nearby, the atlas has no real gradient and the tracer is
    // doomed. If outside-offset samples read *negative*, the bake's sign
    // is inverted.
    //
    // Color key:
    //   magenta   → SDF_SLOT_EMPTY (atlas missing or unreachable)
    //   dark blue → SDF_SLOT_INTERIOR (probe ended up in a solid brick)
    //   red ramp  → negative distance (SDF thinks open space is inside solid)
    //   grayscale → positive distance, [0, 1 m] → [black, white]
    if uniforms.sdf_distance_viz != 0u {
        if sdf_meta.has_sdf_atlas == 0u {
            return vec4<f32>(1.0, 0.0, 1.0, 1.0);
        }
        let probe = in.world_position + N * 0.5;
        let d = sample_sdf(probe);
        if d >= 1.0e5 {
            return vec4<f32>(1.0, 0.0, 1.0, 1.0);
        }
        if d <= -1.0e5 {
            return vec4<f32>(0.0, 0.0, 0.25, 1.0);
        }
        if d < 0.0 {
            let r = clamp(-d, 0.0, 1.0);
            return vec4<f32>(r, 0.0, 0.0, 1.0);
        }
        let g = clamp(d, 0.0, 1.0);
        return vec4<f32>(g, g, g, 1.0);
    }

    // Lighting isolation mode: split direct from indirect for leak debugging.
    //   0 = Normal        — direct + indirect (production shading)
    //   1 = DirectOnly    — zero the SH indirect term
    //   2 = IndirectOnly  — zero direct contributions (short-circuit the loop)
    //   3 = AmbientOnly   — zero both; only ambient floor survives
    // See `LightingIsolation` in postretro/src/render/mod.rs. The ambient
    // floor always contributes so interior geometry is never pitch black.
    let iso = uniforms.lighting_isolation;
    let use_indirect = (iso == 0u) || (iso == 2u);
    let use_direct = (iso == 0u) || (iso == 1u);

    // Indirect term: baked SH irradiance. Zero when no SH volume is loaded
    // or when the isolation mode suppresses indirect.
    var indirect = vec3<f32>(0.0);
    if use_indirect {
        indirect = sample_sh_indirect(in.world_position, N);
    }

    // Total light = ambient floor (minimum) + indirect + direct sum.
    var total_light = vec3<f32>(uniforms.ambient_floor) + indirect;

    // DirectOnly / AmbientOnly modes skip the direct-light loop entirely —
    // cheaper than zeroing contributions inside the loop.
    let light_count = select(0u, uniforms.light_count, use_direct);
    for (var i: u32 = 0u; i < light_count; i = i + 1u) {
        // Influence-volume early-out: skip lights whose sphere bound does
        // not contain this fragment. Pure optimization — no pixel change.
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

        var L: vec3<f32>;
        var attenuation: f32;

        switch light_type {
            case 0u: {
                // Point light
                let to_light = light.position_and_type.xyz - in.world_position;
                let dist = length(to_light);
                L = to_light / max(dist, 0.0001);
                attenuation = falloff(dist, light.direction_and_range.w, falloff_model);
            }
            case 1u: {
                // Spot light
                let to_light = light.position_and_type.xyz - in.world_position;
                let dist = length(to_light);
                L = to_light / max(dist, 0.0001);
                let dist_falloff = falloff(dist, light.direction_and_range.w, falloff_model);
                let cone = cone_attenuation(
                    L,
                    light.direction_and_range.xyz,
                    light.cone_angles_and_pad.x,
                    light.cone_angles_and_pad.y,
                );
                attenuation = dist_falloff * cone;
            }
            default: {
                // Directional light (case 2u and any unknown discriminant)
                L = -light.direction_and_range.xyz;
                attenuation = 1.0;
            }
        }

        let NdotL = max(dot(N, L), 0.0);

        // Shadow modulation: sample shadow map if this light casts shadows.
        // `shadow_kind == 1` → CSM (directional).
        // `shadow_kind == 2` → SDF sphere-trace (point + spot).
        // Any other value is unshadowed.
        let shadow_kind = bitcast<u32>(light.shadow_info.z);
        var shadow_factor = 1.0;
        if shadow_kind == 1u {
            shadow_factor = sample_csm_shadow(in.world_position, bitcast<u32>(light.shadow_info.y));
        } else if shadow_kind == 2u {
            // Soft shadow via SDF sphere-trace. Cone half-angle of 1.5°
            // keeps shadows fairly hard, which matters in dense scenes:
            // a wider cone is dragged down by every pillar the ray
            // passes *near*, producing muddy AO-like shadows instead of
            // crisp directional ones. Retro aesthetic also favors harder
            // shadows — a wider cone can be exposed per-light later.
            shadow_factor = sample_sdf_shadow(
                in.world_position,
                light.position_and_type.xyz,
                0.02617994, // 1.5 degrees in radians
            );
        }

        total_light = total_light + light.color_and_falloff_model.xyz * attenuation * NdotL * shadow_factor;
    }

    let rgb = base_color.rgb * total_light;
    return vec4<f32>(rgb, base_color.a);
}
