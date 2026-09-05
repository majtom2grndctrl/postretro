// Shared octahedral irradiance atlas sampling helper (binding-agnostic).
// See: context/lib/rendering_pipeline.md §4, §8

const SH_DEPTH_MIN_VARIANCE_M2: f32 = 1.0e-4;
const SH_DEPTH_BIAS_CELL_FRACTION: f32 = 0.05;
const SH_DEPTH_MIN_VISIBILITY: f32 = 0.03;
const SH_WEIGHT_EPSILON: f32 = 1.0e-5;
const SH_AFFINITY_FACTOR: u32 = 4u;

fn sh_probe_depth_bias() -> f32 {
    let cell_min = min(min(sh_grid.cell_size.x, sh_grid.cell_size.y), sh_grid.cell_size.z);
    return max(cell_min, 0.0) * SH_DEPTH_BIAS_CELL_FRACTION;
}

fn sh_depth_moment_pair(packed: vec4<u32>) -> vec2<f32> {
    return unpack2x16float(packed.r | (packed.g << 16u));
}

fn sh_indirection_from_moment_texel(packed: vec4<u32>) -> ShProbeIndirection {
    return decode_sh_probe_indirection(packed.b | (packed.a << 16u));
}

fn sh_probe_indirection(idx: vec3<i32>) -> ShProbeIndirection {
    return sh_indirection_from_moment_texel(textureLoad(sh_depth_moments, idx, 0));
}

fn sh_corner_depth_visibility(idx: vec3<i32>, sample_world: vec3<f32>, is_valid: bool) -> f32 {
    if (!is_valid) {
        return 0.0;
    }

    let moments = sh_depth_moment_pair(textureLoad(sh_depth_moments, idx, 0));
    let mean = moments.x;
    let mean2 = moments.y;
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

struct ProbeAtlasLocation {
    layer: u32,
    tile_origin: vec2<u32>,
};

fn probe_slot_location(slot: u32) -> ProbeAtlasLocation {
    let tiles_per_layer = max(sh_grid.tiles_per_layer, 1u);
    let layer = slot / tiles_per_layer;
    let tile_slot = slot - layer * tiles_per_layer;
    let tiles_per_row = max(sh_grid.atlas_tiles_per_row, 1u);
    return ProbeAtlasLocation(
        layer,
        vec2<u32>(
            (tile_slot % tiles_per_row) * sh_grid.tile_dimension,
            (tile_slot / tiles_per_row) * sh_grid.tile_dimension,
        ),
    );
}

// Atlas-parameterized stored-slot fetch. Indirect and direct atlas tiles share
// the same word-derived slot geometry, while their physical extents may differ.
fn sample_probe_atlas_slot(atlas: texture_2d_array<f32>, slot: u32, dir: vec3<f32>) -> vec4<f32> {
    let location = probe_slot_location(slot);
    let oct = oct_encode_unquantized(dir);
    let interior = max(sh_grid.tile_interior, 1u);
    // Mirror `irradiance_interior_texel_direction`: interior texel centers
    // live at `border + (i + 0.5)`, so the inverse sample coordinate is
    // `border + oct * interior`. The 1-texel copied border catches seam taps.
    let texel = vec2<f32>(location.tile_origin)
        + vec2<f32>(f32(sh_grid.tile_border))
        + oct * vec2<f32>(f32(interior));
    // The direct BC6H atlas can have right/bottom block padding, so normalize by
    // the sampled texture's physical extent rather than logical grid dimensions.
    let atlas_dimensions = max(textureDimensions(atlas), vec2<u32>(1u));
    let uv = texel / vec2<f32>(atlas_dimensions);
    return textureSampleLevel(atlas, sh_atlas_sampler, uv, i32(location.layer), 0.0);
}

fn sh_l1_local(idx: vec3<i32>) -> vec3<u32> {
    let probe = vec3<u32>(u32(idx.x), u32(idx.y), u32(idx.z));
    return probe - (probe / vec3<u32>(SH_AFFINITY_FACTOR)) * vec3<u32>(SH_AFFINITY_FACTOR);
}

// Exact WGSL mirror of `sh_reconstruct::trilinear_weight`. `corner` follows
// `corner_locals`' x-fastest order, and a zero-alpha stored tile is absent.
fn sh_l1_corner_weight(probe_local: vec3<u32>, corner: u32) -> f32 {
    let fraction = vec3<f32>(probe_local) / f32(SH_AFFINITY_FACTOR - 1u);
    let high = sh_corner_offset(corner) != vec3<u32>(0u);
    let axis = select(vec3<f32>(1.0) - fraction, fraction, high);
    return axis.x * axis.y * axis.z;
}

fn sample_l1_probe_atlas(
    atlas: texture_2d_array<f32>,
    idx: vec3<i32>,
    slot: u32,
    dir: vec3<f32>,
) -> vec4<f32> {
    let local = sh_l1_local(idx);
    var sum = vec3<f32>(0.0);
    var weight_sum = 0.0;
    for (var corner: u32 = 0u; corner < 8u; corner = corner + 1u) {
        let weight = sh_l1_corner_weight(local, corner);
        if (weight <= 0.0) {
            continue;
        }
        let sample = sample_probe_atlas_slot(atlas, slot + corner, dir);
        let present_weight = weight * sample.a;
        sum = sum + present_weight * max(sample.rgb, vec3<f32>(0.0));
        weight_sum = weight_sum + present_weight;
    }
    if (weight_sum < SH_WEIGHT_EPSILON) {
        return vec4<f32>(0.0);
    }
    return vec4<f32>(sum / weight_sum, 1.0);
}

fn sample_probe_atlas_resolved(
    atlas: texture_2d_array<f32>,
    idx: vec3<i32>,
    indirection: ShProbeIndirection,
    dir: vec3<f32>,
) -> vec4<f32> {
    if (!indirection.valid) {
        return vec4<f32>(0.0);
    }
    if (indirection.level == 1u) {
        return sample_l1_probe_atlas(atlas, idx, indirection.slot, dir);
    }
    if (indirection.level == 0u || indirection.level == 2u) {
        return sample_probe_atlas_slot(atlas, indirection.slot, dir);
    }
    return vec4<f32>(0.0);
}

// Public, atlas-parameterized resolver. Its signature stays stable for every
// reader; validity is derived from the carried word, never composed alpha.
fn sample_probe_atlas_tex(atlas: texture_2d_array<f32>, idx: vec3<i32>, dir: vec3<f32>) -> vec4<f32> {
    return sample_probe_atlas_resolved(atlas, idx, sh_probe_indirection(idx), dir);
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

struct ShWholeCellResolution {
    available: bool,
    level: u32,
    slot: u32,
};

// The whole-cell path is legal only when all eight base lattice corners stay
// within one 4x4x4 brick. Face/edge/corner straddles use the per-corner path:
// its L1 subface property limits it to 32 taps and eight distinct tiles.
fn sh_whole_cell_resolution(gi: vec3<u32>) -> ShWholeCellResolution {
    let first = sh_corner_index(gi, vec3<u32>(0u));
    let first_probe = vec3<u32>(u32(first.x), u32(first.y), u32(first.z));
    let brick = first_probe / vec3<u32>(SH_AFFINITY_FACTOR);
    var found = false;
    var level = 0u;
    var slot = 0u;
    for (var c: u32 = 0u; c < 8u; c = c + 1u) {
        let idx = sh_corner_index(gi, sh_corner_offset(c));
        let probe = vec3<u32>(u32(idx.x), u32(idx.y), u32(idx.z));
        if (any(probe / vec3<u32>(SH_AFFINITY_FACTOR) != brick)) {
            return ShWholeCellResolution(false, 0u, 0u);
        }
        let indirection = sh_probe_indirection(idx);
        if (!indirection.valid) {
            continue;
        }
        if (indirection.level != 1u && indirection.level != 2u) {
            return ShWholeCellResolution(false, 0u, 0u);
        }
        if (!found) {
            found = true;
            level = indirection.level;
            slot = indirection.slot;
        } else if (indirection.level != level || indirection.slot != slot) {
            return ShWholeCellResolution(false, 0u, 0u);
        }
    }
    return ShWholeCellResolution(found, level, slot);
}

fn sh_whole_cell_weight_sum(
    gi: vec3<u32>,
    gfrac: vec3<f32>,
    sample_world: vec3<f32>,
    geo_normal: vec3<f32>,
    reject_backface: bool,
    use_depth_visibility: bool,
    probe_occlusion_enabled: bool,
) -> f32 {
    var sum = 0.0;
    for (var c: u32 = 0u; c < 8u; c = c + 1u) {
        let corner_offset = sh_corner_offset(c);
        let idx = sh_corner_index(gi, corner_offset);
        let indirection = sh_probe_indirection(idx);
        sum = sum + sh_probe_weight(
            idx, corner_offset, gfrac, sample_world, geo_normal, indirection.valid,
            reject_backface, use_depth_visibility, probe_occlusion_enabled,
        );
    }
    return sum;
}

// P9's mandatory L1 path. The eight stored corners are sampled once then
// reused to reconstruct all eight lattice values, exactly preserving the
// per-probe `reconstruct_l1_tile` weights under depth/backface weighting.
fn sample_l1_whole_cell_atlas(
    atlas: texture_2d_array<f32>,
    resolution: ShWholeCellResolution,
    gi: vec3<u32>,
    gfrac: vec3<f32>,
    sample_world: vec3<f32>,
    geo_normal: vec3<f32>,
    reject_backface: bool,
    use_depth_visibility: bool,
    probe_occlusion_enabled: bool,
    dir: vec3<f32>,
) -> vec3<f32> {
    var stored: array<vec4<f32>, 8>;
    for (var corner: u32 = 0u; corner < 8u; corner = corner + 1u) {
        stored[corner] = sample_probe_atlas_slot(atlas, resolution.slot + corner, dir);
    }

    var sum = vec3<f32>(0.0);
    var weight_sum = 0.0;
    for (var c: u32 = 0u; c < 8u; c = c + 1u) {
        let corner_offset = sh_corner_offset(c);
        let idx = sh_corner_index(gi, corner_offset);
        let indirection = sh_probe_indirection(idx);
        let outer_weight = sh_probe_weight(
            idx, corner_offset, gfrac, sample_world, geo_normal, indirection.valid,
            reject_backface, use_depth_visibility, probe_occlusion_enabled,
        );
        if (outer_weight <= 0.0) {
            continue;
        }
        let local = sh_l1_local(idx);
        var reconstructed = vec3<f32>(0.0);
        var reconstruction_weight = 0.0;
        for (var corner: u32 = 0u; corner < 8u; corner = corner + 1u) {
            let w = sh_l1_corner_weight(local, corner) * stored[corner].a;
            reconstructed = reconstructed + w * max(stored[corner].rgb, vec3<f32>(0.0));
            reconstruction_weight = reconstruction_weight + w;
        }
        if (reconstruction_weight < SH_WEIGHT_EPSILON) {
            continue;
        }
        sum = sum + outer_weight * (reconstructed / reconstruction_weight);
        weight_sum = weight_sum + outer_weight;
    }
    if (weight_sum < SH_WEIGHT_EPSILON) {
        return vec3<f32>(0.0);
    }
    return sum / weight_sum;
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
    let whole = sh_whole_cell_resolution(gi);
    if (whole.available && whole.level == 1u) {
        var result: ShDirPair;
        result.a = sample_l1_whole_cell_atlas(
            sh_total_atlas, whole, gi, gfrac, sample_world, geo_normal,
            reject_backface, use_depth_visibility, probe_occlusion_enabled, normal_a,
        );
        result.b = vec3<f32>(0.0);
        if (reconstruct_b) {
            result.b = sample_l1_whole_cell_atlas(
                sh_total_atlas, whole, gi, gfrac, sample_world, geo_normal,
                reject_backface, use_depth_visibility, probe_occlusion_enabled, normal_b,
            );
        }
        return result;
    }
    if (whole.available && whole.level == 2u) {
        let weight_sum = sh_whole_cell_weight_sum(
            gi, gfrac, sample_world, geo_normal, reject_backface,
            use_depth_visibility, probe_occlusion_enabled,
        );
        var result: ShDirPair;
        result.a = vec3<f32>(0.0);
        result.b = vec3<f32>(0.0);
        if (weight_sum >= SH_WEIGHT_EPSILON) {
            result.a = max(sample_probe_atlas_slot(sh_total_atlas, whole.slot, normal_a).rgb, vec3<f32>(0.0));
            if (reconstruct_b) {
                result.b = max(sample_probe_atlas_slot(sh_total_atlas, whole.slot, normal_b).rgb, vec3<f32>(0.0));
            }
        }
        return result;
    }

    var sum_a = vec3<f32>(0.0);
    var sum_b = vec3<f32>(0.0);
    var weight_sum = 0.0;

    for (var c: u32 = 0u; c < 8u; c = c + 1u) {
        let corner_offset = sh_corner_offset(c);
        let idx = sh_corner_index(gi, corner_offset);

        let indirection = sh_probe_indirection(idx);
        let sample_a = sample_probe_atlas_resolved(sh_total_atlas, idx, indirection, normal_a);
        let w = sh_probe_weight(
            idx,
            corner_offset,
            gfrac,
            sample_world,
            geo_normal,
            indirection.valid,
            reject_backface,
            use_depth_visibility,
            probe_occlusion_enabled,
        );
        sum_a = sum_a + w * max(sample_a.rgb, vec3<f32>(0.0));
        if (reconstruct_b) {
            sum_b = sum_b + w * max(
                sample_probe_atlas_resolved(sh_total_atlas, idx, indirection, normal_b).rgb,
                vec3<f32>(0.0),
            );
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

// Shared-weights indirect + direct corner blend. The word supplies validity;
// alpha is read only for L1 stored-corner presence. Returns `.a` = indirect,
// `.b` = direct.
//
// `direct_atlas` is passed as an argument so this helper stays binding-agnostic;
// only the dynamic-entity shaders that declare a direct atlas call it. Chebyshev
// stays ON for the direct term (entities are not static surfaces) and reads the
// shared `sh_depth_moments` (same grid → same moments) used by the indirect path.
fn sample_sh_indirect_direct_corners(
    direct_atlas: texture_2d_array<f32>,
    gi: vec3<u32>,
    gfrac: vec3<f32>,
    sample_world: vec3<f32>,
    shading_normal: vec3<f32>,
    geo_normal: vec3<f32>,
    reject_backface: bool,
    probe_occlusion_enabled: bool,
) -> ShDirPair {
    let whole = sh_whole_cell_resolution(gi);
    if (whole.available && whole.level == 1u) {
        var result: ShDirPair;
        result.a = sample_l1_whole_cell_atlas(
            sh_total_atlas, whole, gi, gfrac, sample_world, geo_normal,
            reject_backface, true, probe_occlusion_enabled, shading_normal,
        );
        result.b = sample_l1_whole_cell_atlas(
            direct_atlas, whole, gi, gfrac, sample_world, geo_normal,
            reject_backface, true, probe_occlusion_enabled, shading_normal,
        );
        return result;
    }
    if (whole.available && whole.level == 2u) {
        let weight_sum = sh_whole_cell_weight_sum(
            gi, gfrac, sample_world, geo_normal, reject_backface, true, probe_occlusion_enabled,
        );
        var result: ShDirPair;
        result.a = vec3<f32>(0.0);
        result.b = vec3<f32>(0.0);
        if (weight_sum >= SH_WEIGHT_EPSILON) {
            result.a = max(sample_probe_atlas_slot(sh_total_atlas, whole.slot, shading_normal).rgb, vec3<f32>(0.0));
            result.b = max(sample_probe_atlas_slot(direct_atlas, whole.slot, shading_normal).rgb, vec3<f32>(0.0));
        }
        return result;
    }

    var sum_indirect = vec3<f32>(0.0);
    var sum_direct = vec3<f32>(0.0);
    var weight_sum = 0.0;

    for (var c: u32 = 0u; c < 8u; c = c + 1u) {
        let corner_offset = sh_corner_offset(c);
        let idx = sh_corner_index(gi, corner_offset);

        let indirection = sh_probe_indirection(idx);
        let sample_indirect = sample_probe_atlas_resolved(
            sh_total_atlas, idx, indirection, shading_normal,
        );
        let w = sh_probe_weight(
            idx,
            corner_offset,
            gfrac,
            sample_world,
            geo_normal,
            indirection.valid,
            reject_backface,
            true,
            probe_occlusion_enabled,
        );
        sum_indirect = sum_indirect + w * max(sample_indirect.rgb, vec3<f32>(0.0));
        let sample_direct = sample_probe_atlas_resolved(
            direct_atlas, idx, indirection, shading_normal,
        );
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

// Direct-only corner blend (the `.b` of the shared-weights pair). The shared
// word-derived validity and renormalizing weight sum are retained.
fn sample_sh_direct_corners_depth_aware(
    direct_atlas: texture_2d_array<f32>,
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
