// Shared octahedral irradiance atlas sampling helper (binding-agnostic).
// See: context/lib/rendering_pipeline.md §4, §8

const SH_DEPTH_MIN_VARIANCE_M2: f32 = 1.0e-4;
const SH_DEPTH_BIAS_CELL_FRACTION: f32 = 0.05;
const SH_DEPTH_MIN_VISIBILITY: f32 = 0.03;
const SH_WEIGHT_EPSILON: f32 = 1.0e-5;

fn sh_probe_depth_bias() -> f32 {
    let cell_min = min(min(sh_grid.cell_size.x, sh_grid.cell_size.y), sh_grid.cell_size.z);
    return max(cell_min, 0.0) * SH_DEPTH_BIAS_CELL_FRACTION;
}

fn sh_corner_depth_visibility(idx: vec3<i32>, sample_world: vec3<f32>, is_valid: bool) -> f32 {
    if (!is_valid) {
        return 0.0;
    }

    let moments = textureLoad(sh_depth_moments, idx, 0).rg;
    let mean = moments.r;
    let mean2 = moments.g;
    let variance = max(mean2 - mean * mean, SH_DEPTH_MIN_VARIANCE_M2);
    let probe_world = sh_grid.grid_origin + vec3<f32>(idx) * sh_grid.cell_size;
    let distance = length(sample_world - probe_world);
    let delta = max(distance - mean - sh_probe_depth_bias(), 0.0);
    let visibility = select(1.0, variance / (variance + delta * delta), delta > 0.0);
    return clamp(visibility, SH_DEPTH_MIN_VISIBILITY, 1.0);
}

struct ShDirPair {
    a: vec3<f32>,
    b: vec3<f32>,
}

// Hand-mirrored from the Rust octahedral encoder. Source of truth for the
// shared convention is `octahedral_oct_uv_matches_wgsl_reference` in
// `crates/level-format/src/octahedral.rs`: if you change this mapping (L1
// projection, the `z < 0` fold, or the `* 0.5 + 0.5` remap), update that test's
// reference UVs to match, or the two sides will silently drift.
fn oct_encode_unquantized(dir_in: vec3<f32>) -> vec2<f32> {
    let dir = normalize(dir_in);
    var p = dir.xy / max(abs(dir.x) + abs(dir.y) + abs(dir.z), 1.0e-6);
    if (dir.z < 0.0) {
        let old = p;
        p = vec2<f32>(
            (1.0 - abs(old.y)) * select(-1.0, 1.0, old.x >= 0.0),
            (1.0 - abs(old.x)) * select(-1.0, 1.0, old.y >= 0.0),
        );
    }
    return p * 0.5 + vec2<f32>(0.5);
}

fn sh_corner_offset(corner: u32) -> vec3<u32> {
    return vec3<u32>(
        corner & 1u,
        (corner >> 1u) & 1u,
        (corner >> 2u) & 1u,
    );
}

fn sh_corner_index(gi: vec3<u32>, corner_offset: vec3<u32>) -> vec3<i32> {
    let gmax = vec3<i32>(sh_grid.grid_dimensions) - vec3<i32>(1);
    return clamp(vec3<i32>(gi + corner_offset), vec3<i32>(0), gmax);
}

fn probe_tile_origin(idx: vec3<i32>) -> vec2<u32> {
    let x = u32(idx.x);
    let y = u32(idx.y);
    let z = u32(idx.z);
    let probe_index = x + y * sh_grid.grid_dimensions.x
        + z * sh_grid.grid_dimensions.x * sh_grid.grid_dimensions.y;
    let tiles_per_row = max(sh_grid.atlas_tiles_per_row, 1u);
    return vec2<u32>(
        (probe_index % tiles_per_row) * sh_grid.tile_dimension,
        (probe_index / tiles_per_row) * sh_grid.tile_dimension,
    );
}

// Atlas-parameterized tile fetch. The tile geometry (origin, octahedral remap,
// border, dimensions) is identical across the indirect and direct octahedral
// atlases because they share one probe grid layout, so the only difference is
// which `texture_2d<f32>` is sampled. Passing the atlas as an argument keeps
// `sh_sample.wgsl` binding-agnostic: consumers that only declare the indirect
// atlas (forward.wgsl, fog_volume.wgsl) never name the direct atlas, while the
// dynamic-entity shaders (skinned_mesh, billboard) can fetch a second atlas with
// the same math. `sh_atlas_sampler`/`sh_grid` remain shared module bindings.
fn sample_probe_atlas_tex(atlas: texture_2d<f32>, idx: vec3<i32>, dir: vec3<f32>) -> vec4<f32> {
    let origin = probe_tile_origin(idx);
    let oct = oct_encode_unquantized(dir);
    let interior = max(sh_grid.tile_interior, 1u);
    // Mirror `irradiance_interior_texel_direction`: interior texel centers
    // live at `border + (i + 0.5)`, so the inverse sample coordinate is
    // `border + oct * interior`. The 1-texel copied border catches seam taps.
    let texel = vec2<f32>(origin)
        + vec2<f32>(f32(sh_grid.tile_border))
        + oct * vec2<f32>(f32(interior));
    let atlas_dimensions = max(sh_grid.atlas_dimensions, vec2<u32>(1u));
    let uv = texel / vec2<f32>(atlas_dimensions);
    return textureSampleLevel(atlas, sh_atlas_sampler, uv, 0.0);
}

fn sample_probe_atlas(idx: vec3<i32>, dir: vec3<f32>) -> vec4<f32> {
    return sample_probe_atlas_tex(sh_total_atlas, idx, dir);
}

fn sh_trilinear_weight(corner_offset: vec3<u32>, gfrac: vec3<f32>) -> f32 {
    let high = corner_offset > vec3<u32>(0u);
    let axis = select(vec3<f32>(1.0) - gfrac, gfrac, high);
    return axis.x * axis.y * axis.z;
}

fn sh_backface_weight(
    corner_offset: vec3<u32>,
    gfrac: vec3<f32>,
    geo_normal: vec3<f32>,
    reject_backface: bool,
) -> f32 {
    if (!reject_backface) {
        return 1.0;
    }

    let dir_to_probe = (vec3<f32>(corner_offset) - gfrac) * sh_grid.cell_size;
    return max(dot(dir_to_probe, geo_normal), 0.0);
}

fn sh_probe_weight(
    idx: vec3<i32>,
    corner_offset: vec3<u32>,
    gfrac: vec3<f32>,
    sample_world: vec3<f32>,
    geo_normal: vec3<f32>,
    is_valid: bool,
    reject_backface: bool,
    use_depth_visibility: bool,
    probe_occlusion_enabled: bool,
) -> f32 {
    let validity = select(0.0, 1.0, is_valid);
    let trilinear = sh_trilinear_weight(corner_offset, gfrac);
    let backface = sh_backface_weight(corner_offset, gfrac, geo_normal, reject_backface);
    var depth_visibility = 1.0;
    if (use_depth_visibility && probe_occlusion_enabled) {
        depth_visibility = sh_corner_depth_visibility(idx, sample_world, is_valid);
    }
    return trilinear * validity * backface * depth_visibility;
}

fn sample_sh_indirect_corners_pair(
    gi: vec3<u32>,
    gfrac: vec3<f32>,
    sample_world: vec3<f32>,
    normal_a: vec3<f32>,
    normal_b: vec3<f32>,
    geo_normal: vec3<f32>,
    reject_backface: bool,
    use_depth_visibility: bool,
    probe_occlusion_enabled: bool,
    reconstruct_b: bool,
) -> ShDirPair {
    var sum_a = vec3<f32>(0.0);
    var sum_b = vec3<f32>(0.0);
    var weight_sum = 0.0;

    for (var c: u32 = 0u; c < 8u; c = c + 1u) {
        let corner_offset = sh_corner_offset(c);
        let idx = sh_corner_index(gi, corner_offset);

        let sample_a = sample_probe_atlas(idx, normal_a);
        let is_valid = sample_a.a >= 0.5;
        let w = sh_probe_weight(
            idx,
            corner_offset,
            gfrac,
            sample_world,
            geo_normal,
            is_valid,
            reject_backface,
            use_depth_visibility,
            probe_occlusion_enabled,
        );
        sum_a = sum_a + w * max(sample_a.rgb, vec3<f32>(0.0));
        if (reconstruct_b) {
            sum_b = sum_b + w * max(sample_probe_atlas(idx, normal_b).rgb, vec3<f32>(0.0));
        }
        weight_sum = weight_sum + w;
    }

    var result: ShDirPair;
    if (weight_sum < SH_WEIGHT_EPSILON) {
        result.a = vec3<f32>(0.0);
        result.b = vec3<f32>(0.0);
        return result;
    }
    result.a = sum_a / weight_sum;
    result.b = sum_b / weight_sum;
    return result;
}

// Shared-weights indirect + direct corner blend. The per-probe weights (probe
// selection, trilinear, validity from atlas alpha, backface, Chebyshev depth
// visibility) are computed ONCE from the indirect atlas and reused for both
// octahedral fetches — the two atlases differ only in the radiance they store,
// not in probe layout or validity (both keyed on the shared grid, and validity
// alpha is authored identically). Returns `.a` = indirect, `.b` = direct.
//
// `direct_atlas` is passed as an argument so this helper stays binding-agnostic;
// only the dynamic-entity shaders that declare a direct atlas call it. Chebyshev
// stays ON for the direct term (entities are not static surfaces) and reads the
// shared `sh_depth_moments` (same grid → same moments) used by the indirect path.
fn sample_sh_indirect_direct_corners(
    direct_atlas: texture_2d<f32>,
    gi: vec3<u32>,
    gfrac: vec3<f32>,
    sample_world: vec3<f32>,
    shading_normal: vec3<f32>,
    geo_normal: vec3<f32>,
    reject_backface: bool,
    probe_occlusion_enabled: bool,
) -> ShDirPair {
    var sum_indirect = vec3<f32>(0.0);
    var sum_direct = vec3<f32>(0.0);
    var weight_sum = 0.0;

    for (var c: u32 = 0u; c < 8u; c = c + 1u) {
        let corner_offset = sh_corner_offset(c);
        let idx = sh_corner_index(gi, corner_offset);

        let sample_indirect = sample_probe_atlas_tex(sh_total_atlas, idx, shading_normal);
        let is_valid = sample_indirect.a >= 0.5;
        let w = sh_probe_weight(
            idx,
            corner_offset,
            gfrac,
            sample_world,
            geo_normal,
            is_valid,
            reject_backface,
            true,
            probe_occlusion_enabled,
        );
        sum_indirect = sum_indirect + w * max(sample_indirect.rgb, vec3<f32>(0.0));
        let sample_direct = sample_probe_atlas_tex(direct_atlas, idx, shading_normal);
        sum_direct = sum_direct + w * max(sample_direct.rgb, vec3<f32>(0.0));
        weight_sum = weight_sum + w;
    }

    var result: ShDirPair;
    if (weight_sum < SH_WEIGHT_EPSILON) {
        result.a = vec3<f32>(0.0);
        result.b = vec3<f32>(0.0);
        return result;
    }
    result.a = sum_indirect / weight_sum;
    result.b = sum_direct / weight_sum;
    return result;
}

// Direct-only corner blend (the `.b` of the shared-weights pair). The indirect
// term is still fetched to derive validity alpha and to share the renormalizing
// weight sum, but only the direct radiance is returned. Consumers that already
// computed the indirect term separately use this to add the direct contribution.
fn sample_sh_direct_corners_depth_aware(
    direct_atlas: texture_2d<f32>,
    gi: vec3<u32>,
    gfrac: vec3<f32>,
    sample_world: vec3<f32>,
    shading_normal: vec3<f32>,
    geo_normal: vec3<f32>,
    reject_backface: bool,
    probe_occlusion_enabled: bool,
) -> vec3<f32> {
    return sample_sh_indirect_direct_corners(
        direct_atlas,
        gi,
        gfrac,
        sample_world,
        shading_normal,
        geo_normal,
        reject_backface,
        probe_occlusion_enabled,
    ).b;
}

fn sample_sh_indirect_corners_depth_aware(
    gi: vec3<u32>,
    gfrac: vec3<f32>,
    sample_world: vec3<f32>,
    shading_normal: vec3<f32>,
    geo_normal: vec3<f32>,
    reject_backface: bool,
    probe_occlusion_enabled: bool,
) -> vec3<f32> {
    return sample_sh_indirect_corners_pair(
        gi,
        gfrac,
        sample_world,
        shading_normal,
        shading_normal,
        geo_normal,
        reject_backface,
        true,
        probe_occlusion_enabled,
        false,
    ).a;
}

fn sample_sh_indirect_corners_without_depth(
    gi: vec3<u32>,
    gfrac: vec3<f32>,
    shading_normal: vec3<f32>,
    geo_normal: vec3<f32>,
    reject_backface: bool,
) -> vec3<f32> {
    let unused_sample_world = sh_grid.grid_origin + vec3<f32>(gi) * sh_grid.cell_size;
    return sample_sh_indirect_corners_pair(
        gi,
        gfrac,
        unused_sample_world,
        shading_normal,
        shading_normal,
        geo_normal,
        reject_backface,
        false,
        false,
        false,
    ).a;
}

fn sample_sh_indirect_corners_two_without_depth(
    gi: vec3<u32>,
    gfrac: vec3<f32>,
    normal_a: vec3<f32>,
    normal_b: vec3<f32>,
) -> ShDirPair {
    let unused_sample_world = sh_grid.grid_origin + vec3<f32>(gi) * sh_grid.cell_size;
    return sample_sh_indirect_corners_pair(
        gi,
        gfrac,
        unused_sample_world,
        normal_a,
        normal_b,
        normal_a,
        false,
        false,
        false,
        true,
    );
}
