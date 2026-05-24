// Shared SH irradiance volume sampling helper (binding-agnostic).
// See: context/lib/rendering_pipeline.md §4, §8

// Manual 8-corner SH irradiance blend. Replaces the hardware-trilinear fetch:
// per-corner weights cannot be reweighted through a linear sampler, so each
// corner is loaded explicitly with `textureLoad` and reweighted by validity
// and (optionally) backface rejection before renormalization.
//
// Binding-agnostic (§8): this helper declares no buffers. The consumer shader
// must declare, at the (group, binding) the helper expects, BEFORE this file
// is textually concatenated:
//     var sh_band0 .. sh_band8: texture_3d<f32>
//     var sh_depth_moments: texture_3d<f32>
//     var sh_grid: ShGridInfo   (with grid_origin / grid_dimensions / cell_size)
// The consumer must NOT declare its own `sh_irradiance` / `sample_sh_indirect*`
// — this helper owns those symbols; a local copy is a duplicate-definition error.

const SH_DEPTH_MIN_VARIANCE_M2: f32 = 1.0e-4;
const SH_DEPTH_BIAS_CELL_FRACTION: f32 = 0.05;
const SH_DEPTH_MIN_VISIBILITY: f32 = 0.03;

// SH L0..L2 basis evaluation. Constants are standard real SH normalization
// factors. Signs on bands 1, 3, 5, 7 match the signed basis used by the baker
// (postretro-level-compiler/src/sh_bake.rs::sh_basis_l2) — projection and
// reconstruction MUST use the same signed basis, or L1-y / L1-x / L2-yz / L2-xz
// invert.
//
// The Ramamoorthi-Hanrahan cosine-lobe convolution (A_0=π, A_1=2π/3, A_2=π/4)
// is folded into the baked coefficients at bake time (sh_bake.rs::apply_cosine_lobe_rgb).
// Runtime reconstruction applies only the basis — if indirect looks wrong,
// suspect the baker or upload path, not these constants.
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

// Isolates per-corner reconstruction so the 8-corner loop in
// `sample_sh_indirect_corners_pair` stays readable and the clamp-to-non-negative
// is applied in exactly one place.
fn sh_corner_irradiance(bands: array<vec3<f32>, 9>, shading_normal: vec3<f32>) -> vec3<f32> {
    return max(
        sh_irradiance(
            bands[0], bands[1], bands[2], bands[3], bands[4],
            bands[5], bands[6], bands[7], bands[8],
            shading_normal,
        ),
        vec3<f32>(0.0),
    );
}

fn sh_probe_depth_bias() -> f32 {
    let cell_min = min(min(sh_grid.cell_size.x, sh_grid.cell_size.y), sh_grid.cell_size.z);
    return max(cell_min, 0.0) * SH_DEPTH_BIAS_CELL_FRACTION;
}

// One-tailed Chebyshev upper-bound estimate of the unoccluded fraction of
// directions from the probe at `sample_world`. Returns 1.0 at or below the
// mean depth (plus bias) — the sample point is in front of the probe's
// depth horizon. Attenuates toward SH_DEPTH_MIN_VISIBILITY beyond it.
// Invalid probes return 0.0 so they are excluded from the blend.
// The `delta > 0` guard keeps visibility at 1.0 when the sample is inside
// the mean: the Chebyshev bound only tightens for over-mean distances.
fn sh_corner_depth_visibility(idx: vec3<i32>, sample_world: vec3<f32>, is_valid: bool) -> f32 {
    if !is_valid {
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

// Canonical manual 8-corner SH blend — the single source of truth for the
// fetch / trilinear weight / validity / backface / depth-visibility loop.
// Reconstructs up to two directions (`normal_a`, `normal_b`) from one fetch
// and returns both in a `ShDirPair`. Every public entry point below funnels
// through this; the math lives in exactly one place.
//
// `gi`     — integer grid index of the lower corner. Billboard/fog clamp only
//            the low side before computing `gi`, so it may equal or exceed the
//            grid dimensions on the high side; the per-corner clamp below
//            handles this correctly.
// `gfrac`  — sub-cell fraction in [0, 1) within the cell. Forward derives this
//            from a normal-offset sample position; billboard and fog pass the
//            raw grid-space coordinate with no offset.
// `normal_a`/`normal_b` — directions used for SH reconstruction. The
//            single-direction wrappers pass the same (shading) normal in both
//            slots and set `reconstruct_b = false`; the fog dual path passes
//            two distinct directions and sets `reconstruct_b = true`.
// `geo_normal`     — geometric mesh normal, used for backface rejection.
// `reject_backface`— when true, downweight corners on the far side of the
//                    surface. Forward sets this; billboard/fog do not.
// `use_depth_visibility` — when true, apply the Chebyshev moment-visibility
//                    term. Forward/billboard set this; fog does not (fog has no
//                    surface, so there is no depth horizon to test against).
// `reconstruct_b`  — when false, skip the second reconstruction entirely so the
//                    hot single-direction path pays for exactly one
//                    `sh_corner_irradiance` per corner. `result.b` is then
//                    unspecified and must be ignored by the caller.
//
// IMPORTANT — Metal constraint: the 9-band array stays register-resident inside
// this function. Do NOT factor the loop body into a callee taking
// `ptr<function, array<...>>` — that spills to device-private memory and
// destroys read coalescing on Apple Silicon (commit b93d31e, reverted by
// bda93f4; see the `fog_volume.wgsl` cs_main comment).
//
// Per corner: clamp the index to the valid grid range (matching clamp-to-edge
// sampling; an out-of-range load returns 0, which the validity test would
// misread as invalid). Load all 9 total bands. Weight by
//   w = trilinear * validity * bf * depth_visibility
// where `validity` is 1 when band-0 alpha >= 0.5 (the baked validity bit),
// `bf` is 1 when `reject_backface` is false, else max(dot(dir, geo_normal), 0)
// with `dir = (corner_offset - gfrac) * cell_size` (un-normalized; magnitude
// divides out under renormalization), and `depth_visibility` is the
// Chebyshev moment-visibility term — 1.0 when depth visibility is disabled
// or the corner is in front of its mean-depth horizon, attenuating toward
// SH_DEPTH_MIN_VISIBILITY beyond. Accumulate Σ w·irradiance and Σ w, then
// renormalize. When Σ w is below epsilon (all corners invalid/backfacing),
// return zero SH — matching the `has_sh_volume == 0` path so there is no
// div-by-zero, NaN, or black flash.
//
// Normalization note: both components divide by `weight_sum` directly (not by a
// shared reciprocal), so `.a` is bit-identical to the legacy single-direction
// path and `.b` matches what the legacy dual helper produced for its second
// direction within rounding of the same accumulation order.
fn sample_sh_indirect_corners_pair(
    gi: vec3<u32>,
    gfrac: vec3<f32>,
    sample_world: vec3<f32>,
    normal_a: vec3<f32>,
    normal_b: vec3<f32>,
    geo_normal: vec3<f32>,
    reject_backface: bool,
    use_depth_visibility: bool,
    reconstruct_b: bool,
) -> ShDirPair {
    let gmax = vec3<i32>(sh_grid.grid_dimensions) - vec3<i32>(1);

    var sum_a = vec3<f32>(0.0);
    var sum_b = vec3<f32>(0.0);
    var weight_sum = 0.0;

    for (var c: u32 = 0u; c < 8u; c = c + 1u) {
        // Corner offset: bit 0 -> x, bit 1 -> y, bit 2 -> z.
        let corner_offset = vec3<f32>(
            f32(c & 1u),
            f32((c >> 1u) & 1u),
            f32((c >> 2u) & 1u),
        );

        // Clamp BEFORE loading so out-of-range corners read the edge probe
        // rather than an all-zero texel the validity test would reject.
        let idx = clamp(
            vec3<i32>(gi) + vec3<i32>(corner_offset),
            vec3<i32>(0),
            gmax,
        );

        var bands: array<vec3<f32>, 9>;
        let t0 = textureLoad(sh_band0, idx, 0);
        bands[0] = t0.rgb;
        bands[1] = textureLoad(sh_band1, idx, 0).rgb;
        bands[2] = textureLoad(sh_band2, idx, 0).rgb;
        bands[3] = textureLoad(sh_band3, idx, 0).rgb;
        bands[4] = textureLoad(sh_band4, idx, 0).rgb;
        bands[5] = textureLoad(sh_band5, idx, 0).rgb;
        bands[6] = textureLoad(sh_band6, idx, 0).rgb;
        bands[7] = textureLoad(sh_band7, idx, 0).rgb;
        bands[8] = textureLoad(sh_band8, idx, 0).rgb;

        // Trilinear weight from gfrac toward this corner.
        let lerp_w = select(1.0 - gfrac, gfrac, corner_offset > vec3<f32>(0.5));
        let trilinear = lerp_w.x * lerp_w.y * lerp_w.z;

        // Validity rides in band-0 alpha (>= 0.5 → valid).
        let is_valid = t0.a >= 0.5;
        let validity = select(0.0, 1.0, is_valid);

        // Backface term: corner direction from the fragment, projected onto the
        // geometric normal. Un-normalized — magnitude divides out below.
        var bf = 1.0;
        if reject_backface {
            let dir = (corner_offset - gfrac) * sh_grid.cell_size;
            bf = max(dot(dir, geo_normal), 0.0);
        }

        var depth_visibility = 1.0;
        if use_depth_visibility {
            depth_visibility = sh_corner_depth_visibility(idx, sample_world, is_valid);
        }

        let w = trilinear * validity * bf * depth_visibility;
        sum_a = sum_a + w * sh_corner_irradiance(bands, normal_a);
        // Skip the second reconstruction on the hot single-direction path; the
        // bands are already register-resident, so when needed it is cheap.
        if reconstruct_b {
            sum_b = sum_b + w * sh_corner_irradiance(bands, normal_b);
        }
        weight_sum = weight_sum + w;
    }

    // All corners dropped: return zero SH contribution, matching the
    // `has_sh_volume == 0` early-out. Forward callers then see only the
    // ambient floor they add outside this function; billboard/fog callers
    // see zero SH. Epsilon guard avoids div-by-zero / NaN / black flash.
    var result: ShDirPair;
    if weight_sum < 1.0e-5 {
        result.a = vec3<f32>(0.0);
        result.b = vec3<f32>(0.0);
        return result;
    }
    // Divide (not multiply-by-reciprocal) so `.a` is bit-identical to the
    // legacy single-direction path.
    result.a = sum_a / weight_sum;
    result.b = sum_b / weight_sum;
    return result;
}

fn sample_sh_indirect_corners_depth_aware(
    gi: vec3<u32>,
    gfrac: vec3<f32>,
    sample_world: vec3<f32>,
    shading_normal: vec3<f32>,
    geo_normal: vec3<f32>,
    reject_backface: bool,
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
    ).a;
}

// Two-direction SH read sharing a single 8-corner fetch. The 72 textureLoads
// and the per-corner trilinear/validity weights depend only on position, not
// the reconstruction direction — so a consumer that needs two reads at the same
// point (the fog pass: a world-up isotropic read and a view-derived directional
// read) fetches the corners once and reconstructs both directions. Each
// returned component is bit-identical to a corresponding
// `sample_sh_indirect_corners_without_depth` call, at half the texture
// bandwidth. No depth-visibility, no backface rejection (fog has no surface
// normal); matches the `without_depth` / `reject_backface = false` path.
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
        true,
    );
}
