// Surface Depth — shared texel-space parallax for world brushes and kinematic
// brush movers. See: context/lib/rendering_pipeline.md §7.3, §8.
//
// Binding-agnostic in the `material_shading.wgsl` sense: this snippet declares
// NO bindings. It resolves two names lexically from whichever consumer appends
// it — `spec_texture` (group 1 binding 2) and `material` (group 1 binding 3) —
// and takes everything else as arguments, so `forward.wgsl` and
// `kinematic_brush.wgsl` share ONE copy of the march. They must: the two
// shaders already keep duplicate `sample_post_retro` bodies and those can
// drift; the DDA must never be a second thing to keep in sync.
//
// WHAT THIS IS
// The specular slot is a two-channel surface map: R = specular intensity,
// G = DEPTH BELOW THE SURFACE. Depth 0 means flat, so a material with no
// `_h.png` sibling — which binds a 1x1 black R8Unorm placeholder that WGSL
// expands to (r, 0, 0, 1) — is a true no-op through this code, not a special
// case around it.
//
// The field is piecewise-constant per texel: a grid of boxes whose lattice is
// the SAME texel grid `sample_post_retro` snaps albedo to, so stone side faces
// land exactly on albedo texel edges. That alignment is the point. Marching a
// grid of boxes is an exact 2D DDA (Amanatides-Woo), not a fixed-step POM —
// there is no sampling error to trade against step count.
//
// The carve is INWARD only: the displaced surface never exceeds its real plane,
// so there is no silhouette artifact and collision stays correct for free (the
// player walks on the stone tops, which IS the real plane).
//
// HARD CONSTRAINTS THIS CODE HONORS
//  * It never writes `@builtin(frag_depth)`. The depth pre-pass is vertex-only
//    and the forward pass runs `depth_compare: Equal` with depth writes off; a
//    fragment that wrote depth would fail its own equality test. The technique
//    is depth-free by construction.
//  * It offsets `base_uv` ONLY. `lightmap_uv` is never touched — lightmap
//    charts carry just `CHART_PADDING_TEXELS = 2` of gutter, so an offset would
//    pull a neighbouring chart. `base_uv` has no atlas and uses
//    `AddressMode::Repeat`, so marching it freely is safe.
//  * It calls NO derivative function. `dpdx`/`dpdy` must stay in uniform
//    control flow (naga's uniformity analysis is enforced by a test); the
//    consumer hoists them to the top of `fs_main` and hands them in. Height is
//    read with `textureLoad` at an explicit mip, which needs no sampler and
//    preserves the exact quantized values the DDA depends on.
//  * It adds no binding and no sampled texture. The forward pass is at exactly
//    16/16 (`FORWARD_SAMPLED_TEXTURE_BUDGET`), which is why height rides in the
//    specular slot in the first place.
//  * The face normal produced here must NEVER reach shadow-map receiver bias.
//    That path takes the GEOMETRIC normal on purpose; this normal is far
//    bumpier than a normal map and would wobble shadow boundaries.
//
// The CPU authority for every rule and constant below is
// `postretro_render_cpu::surface_depth`, which is unit-tested without a GPU.

const SURFACE_DEPTH_EPS: f32 = 1.0e-9;
const SURFACE_DEPTH_FAR: f32 = 3.4e38;
// Floor on the UV Jacobian's determinant. It is a PRODUCT of two UV
// derivatives, so it is naturally tiny — at close range a 1k texture can put it
// near 1e-9. The floor only has to catch a genuinely singular chart (which
// makes the solve 0/0); the honest check on the result is the scale range
// below, which also traps the NaN and Inf a near-singular solve produces.
const SURFACE_DEPTH_DET_EPS: f32 = 1.0e-20;
// Sanity range for world meters per UV unit. Outside it the solved frame is
// not describing a real brush face.
const SURFACE_DEPTH_MAX_UV_SCALE_M: f32 = 1.0e6;
// Cosine floor between the view ray and the surface normal. Below it the
// fragment is a sub-pixel sliver seen edge-on and the march has nothing to say.
const SURFACE_DEPTH_MIN_DESCENT: f32 = 1.0e-3;

// Packed layout of `material.surface_depth_march`.
const SURFACE_DEPTH_MAX_STEPS_MASK: u32 = 0xFFu;
const SURFACE_DEPTH_BASE_MIP_SHIFT: u32 = 8u;
const SURFACE_DEPTH_BASE_MIP_MASK: u32 = 0xFu;
const SURFACE_DEPTH_HAS_DEPTH_BIT: u32 = 0x1000u;
// Per-fragment dynamic-light self-shadow budget. It rides the uniform rather
// than a `const` because the player-facing quality tier (design D5) is applied
// by rewriting this BUFFER — this engine has no shader-variant system, so a
// tier that must switch the shadow march off has to reach the shader as data.
// Zero means the second (shadow) DDA never runs.
const SURFACE_DEPTH_SHADOW_BUDGET_SHIFT: u32 = 16u;
const SURFACE_DEPTH_SHADOW_BUDGET_MASK: u32 = 0xFu;

const SURFACE_DEPTH_FADE_LOD_START: f32 = 1.0;
const SURFACE_DEPTH_FADE_LOD_RANGE: f32 = 2.0;
const SURFACE_DEPTH_FADE_DISTANCE_FRACTION: f32 = 0.25;
const SURFACE_DEPTH_AO_STRENGTH: f32 = 0.75;
const SURFACE_DEPTH_SHADOW_BIAS_M: f32 = 1.0e-4;
const SURFACE_DEPTH_SIDE_UV_BIAS_TEXELS: f32 = 0.5;
// Unit of `material.surface_depth_meters`: 0 = world meters, 1 = albedo texels.
// Mirrors `postretro_render_data::material::SURFACE_DEPTH_TEXEL_MODE`, which
// selects the matching authoring table; the two are pinned against each other.
const SURFACE_DEPTH_TEXEL_MODE: u32 = 1u;

// `LightTermMask::DEPTH_AMBIENT_OCCLUSION`. Bit 8 — bit 7 stays reserved for
// the intentionally unwired emissive category.
const LIGHT_TERM_DEPTH_AO: u32 = 0x100u;

struct SurfaceDepthResult {
    // False whenever the fragment must render exactly as it did before this
    // feature existed: no surface map bound, a flat material, a faded-out
    // surface, a degenerate UV chart, or an edge-on fragment.
    carved: bool,
    // UV to sample the material's textures at. Biased half a texel past the
    // crossed boundary on a side hit so `sample_post_retro` reads the stone's
    // own color rather than blending across the edge it just hit.
    uv: vec2<f32>,
    // UV of the hit point itself. Self-shadow marches start here.
    march_uv: vec2<f32>,
    // Hit point on the view ray. Feeds dynamic light direction and
    // attenuation; it is deliberately NOT used for shadow-map lookups, which
    // must stay on the true plane the depth maps were rendered from.
    world_position: vec3<f32>,
    // World-space normal of the face that was hit: the geometric normal on a
    // top hit, an exact +/-U or +/-V axis on a side hit. The consumer applies
    // the normal map on top hits only.
    normal: vec3<f32>,
    hit_top: bool,
    // Depth below the true plane, in meters. Always <= depth_scale_m.
    depth_m: f32,
    // Post-fade carve depth for this fragment, in meters.
    depth_scale_m: f32,
    // The distance/LOD fade that produced `depth_scale_m`, in [0, 1]. Carried
    // out of the resolve because depth-derived terms whose inputs are BOTH
    // post-fade cancel it out and would pop at the fade boundary instead of
    // degrading. See `surface_depth_indirect_ao`.
    fade: f32,
    quantize_levels: f32,
    shadow_steps: u32,
    // How many DYNAMIC lights this fragment may self-shadow, from the player's
    // quality tier. Zero at `Low` and `Off`, and zero whenever `carved` is
    // false, so the consumer's budget test also covers the flat path.
    shadow_light_budget: u32,
    base_mip: u32,
    // UV axes in world space, and UV units per world meter along each. Derived
    // from the SAME Jacobian the meters->UV conversion uses, so the march's
    // axes and its scale cannot disagree.
    tangent: vec3<f32>,
    bitangent: vec3<f32>,
    geo_normal: vec3<f32>,
    uv_per_m: vec2<f32>,
};

fn surface_depth_flat(uv: vec2<f32>, world_position: vec3<f32>, geo_normal: vec3<f32>) -> SurfaceDepthResult {
    var out: SurfaceDepthResult;
    out.carved = false;
    out.uv = uv;
    out.march_uv = uv;
    out.world_position = world_position;
    out.normal = geo_normal;
    out.hit_top = true;
    out.depth_m = 0.0;
    out.depth_scale_m = 0.0;
    out.fade = 0.0;
    out.quantize_levels = 0.0;
    out.shadow_steps = 0u;
    out.shadow_light_budget = 0u;
    out.base_mip = 0u;
    out.tangent = vec3<f32>(1.0, 0.0, 0.0);
    out.bitangent = vec3<f32>(0.0, 1.0, 0.0);
    out.geo_normal = geo_normal;
    out.uv_per_m = vec2<f32>(0.0, 0.0);
    return out;
}

// Wrapped, quantized depth fetch. `textureLoad` does not wrap, so the tile is
// folded by hand — `base_uv` samples through `AddressMode::Repeat` and the
// march is free to walk off the edge of the texture.
//
// Quantization (`floor(h * levels) / levels`) is one ALU op and a live
// aesthetic dial: neighbouring texels snap onto shared plateaus, giving fewer
// and larger terraces. It does not affect the DDA's exactness — that comes from
// the field being constant per texel, not from the value landing on a plateau.
fn surface_depth_texel(coord: vec2<i32>, dims: vec2<i32>, level: u32, levels: f32) -> f32 {
    var folded = coord % dims;
    folded = select(folded + dims, folded, folded >= vec2<i32>(0, 0));
    let raw = textureLoad(spec_texture, folded, level).g;
    if levels >= 1.0 {
        return floor(raw * levels) / levels;
    }
    return raw;
}

fn surface_depth_has_map() -> bool {
    return (material.surface_depth_march & SURFACE_DEPTH_HAS_DEPTH_BIT) != 0u
        && material.surface_depth_meters > 0.0;
}

// Distance fade: 1 near, 0 at and beyond the material's fade distance.
fn surface_depth_distance_fade(distance_m: f32, fade_distance_m: f32) -> f32 {
    if fade_distance_m <= 0.0 {
        return 0.0;
    }
    let ramp = max(fade_distance_m * SURFACE_DEPTH_FADE_DISTANCE_FRACTION, SURFACE_DEPTH_EPS);
    return clamp((fade_distance_m - distance_m) / ramp, 0.0, 1.0);
}

// Residency/LOD fade. `lod` is log2 of the fragment's footprint measured in
// texels OF THE RESIDENT BASE MIP, so when streaming drops top mips the base
// dimensions shrink, the lod drops with them, and a streamed-out surface map
// flattens gracefully instead of popping (D6.2). It is also a straight perf
// win: at distance the texels go sub-pixel and the parallax is invisible.
fn surface_depth_lod_fade(lod: f32) -> f32 {
    return clamp(1.0 - (lod - SURFACE_DEPTH_FADE_LOD_START) / SURFACE_DEPTH_FADE_LOD_RANGE, 0.0, 1.0);
}

// Ambient occlusion for the SH INDIRECT term only. Legitimate because SH probes
// sit at ~1 m spacing and "know nothing of the receiver's own geometry"
// (rendering_pipeline.md §4) — cobblestone-scale self-occlusion is a fact no
// other source owns, so this is not double-counting a light.
fn surface_depth_indirect_ao(depth: SurfaceDepthResult, light_terms: u32) -> f32 {
    if !depth.carved || (light_terms & LIGHT_TERM_DEPTH_AO) == 0u {
        return 1.0;
    }
    if depth.depth_scale_m <= SURFACE_DEPTH_EPS {
        return 1.0;
    }
    // Scale by the fade. `depth_m` and `depth_scale_m` are both post-fade, so
    // their ratio is the raw texel value at EVERY fade — without this factor a
    // surface one epsilon inside the fade boundary still occludes at full
    // strength and then snaps to 1.0 the moment it crosses, which reads as a
    // moving arc of brightness as the LOD isoline sweeps the floor.
    return 1.0
        - SURFACE_DEPTH_AO_STRENGTH
            * depth.fade
            * clamp(depth.depth_m / depth.depth_scale_m, 0.0, 1.0);
}

// Resolve the fragment's carve: derive the surface frame, fade, march, and
// report the hit. `view_to_eye` points from the surface toward the camera.
//
// The UV frame comes from `dpdx(world_position) / dpdx(uv)` rather than from
// the baked tangent on purpose. Depth is expressed in METERS — there is no
// texel-density convention for world materials, brush UV scale is authored
// freely in TrenchBroom, and a texture-space scale would give the same material
// a different physical depth on differently scaled brushes. Taking the march
// axes AND the meters->UV scale from one Jacobian makes them consistent by
// construction. The baked tangent still owns normal mapping, which is a
// different, authored tangent space.
fn surface_depth_resolve(
    uv: vec2<f32>,
    world_position: vec3<f32>,
    geo_normal: vec3<f32>,
    view_to_eye: vec3<f32>,
    view_distance: f32,
    ddx_uv: vec2<f32>,
    ddy_uv: vec2<f32>,
    ddx_world: vec3<f32>,
    ddy_world: vec3<f32>,
) -> SurfaceDepthResult {
    let flat_result = surface_depth_flat(uv, world_position, geo_normal);
    if !surface_depth_has_map() {
        return flat_result;
    }

    let packed = material.surface_depth_march;
    let base_mip = (packed >> SURFACE_DEPTH_BASE_MIP_SHIFT) & SURFACE_DEPTH_BASE_MIP_MASK;
    let max_steps = max(packed & SURFACE_DEPTH_MAX_STEPS_MASK, 1u);

    let dims_u = textureDimensions(spec_texture, base_mip);
    let dims = vec2<f32>(dims_u);
    let dims_i = vec2<i32>(dims_u);

    // Fade first: a fully faded fragment must cost nothing beyond this point.
    let footprint = max(length(ddx_uv * dims), length(ddy_uv * dims));
    let lod = log2(max(footprint, SURFACE_DEPTH_EPS));
    let fade = min(
        surface_depth_distance_fade(view_distance, material.surface_depth_fade_distance),
        surface_depth_lod_fade(lod),
    );
    // Every degenerate test below is written as `!(x > lo && x < hi)` rather
    // than `x <= lo || x >= hi`, so a NaN — which compares false against
    // everything — falls out to the flat path instead of slipping through and
    // poisoning the march.
    if !(fade > 0.0) {
        return flat_result;
    }
    // Whatever unit the field carries, a non-positive value — or a NaN, which
    // fails this the same way — means this material does not carve. Taking the
    // early-out here keeps the degenerate-chart work below off a flat material.
    let carve_request = material.surface_depth_meters * fade;
    if !(carve_request > SURFACE_DEPTH_EPS) {
        return flat_result;
    }

    // Solve dW/du and dW/dv from the two screen-space derivative pairs.
    let det = ddx_uv.x * ddy_uv.y - ddx_uv.y * ddy_uv.x;
    if !(abs(det) > SURFACE_DEPTH_DET_EPS) {
        return flat_result;
    }
    let w_u = (ddx_world * ddy_uv.y - ddy_world * ddx_uv.y) / det;
    let w_v = (ddy_world * ddx_uv.x - ddx_world * ddy_uv.x) / det;
    let scale_u = length(w_u);
    let scale_v = length(w_v);
    if !(scale_u > SURFACE_DEPTH_EPS && scale_u < SURFACE_DEPTH_MAX_UV_SCALE_M)
        || !(scale_v > SURFACE_DEPTH_EPS && scale_v < SURFACE_DEPTH_MAX_UV_SCALE_M) {
        return flat_result;
    }
    // Project out the normal: interpolation leaves the derivatives slightly off
    // the tangent plane, and a side-wall normal that is not perpendicular to the
    // surface normal reads as a lighting seam.
    let t_raw = w_u - geo_normal * dot(w_u, geo_normal);
    let b_raw = w_v - geo_normal * dot(w_v, geo_normal);
    let t_len2 = dot(t_raw, t_raw);
    let b_len2 = dot(b_raw, b_raw);
    if !(t_len2 > SURFACE_DEPTH_EPS) || !(b_len2 > SURFACE_DEPTH_EPS) {
        return flat_result;
    }
    let tangent = normalize(t_raw);
    let bitangent = normalize(b_raw);
    let uv_per_m = vec2<f32>(1.0 / scale_u, 1.0 / scale_v);

    // Carve depth, in meters, whichever unit it was authored in.
    //
    // In TEXEL mode the authored value is a count of albedo texels and is
    // converted with THIS fragment's texel rate, so the carve is a fixed depth
    // in the texel lattice rather than in the world: a 2048px texture on a
    // small brush carves the same number of texels as a 256px one on a large
    // brush. The geometric mean is the neutral reading of a rate that differs
    // per axis — on the square-texel faces this engine's brushes normally
    // produce, either axis gives the same answer.
    //
    // It also bounds the march. Horizontal travel through the carve is
    // `depth_m * dir`, and `dir` is texels per meter of descent, so the texel
    // rate cancels: travel is `N * tan(theta)` texels regardless of texture
    // resolution or brush scale. In meters mode it does not cancel, which is
    // why a high-resolution texture on a small brush can exhaust the budget.
    var depth_scale_m = carve_request;
    if SURFACE_DEPTH_TEXEL_MODE == 1u {
        let texels_per_m = uv_per_m * dims;
        let texel_rate = sqrt(max(texels_per_m.x * texels_per_m.y, SURFACE_DEPTH_EPS));
        depth_scale_m = carve_request / texel_rate;
    }
    if !(depth_scale_m > SURFACE_DEPTH_EPS) {
        return flat_result;
    }

    // Descent rate along the view ray. An edge-on fragment never descends.
    let descent = dot(view_to_eye, geo_normal);
    if !(descent > SURFACE_DEPTH_MIN_DESCENT) {
        return flat_result;
    }
    let into = -view_to_eye;
    let dir_uv_per_m = vec2<f32>(
        dot(into, tangent) * uv_per_m.x / descent,
        dot(into, bitangent) * uv_per_m.y / descent,
    );

    let levels = material.surface_depth_quantize_levels;
    let p0 = uv * dims;
    // Texels per meter of descent.
    let dir = dir_uv_per_m * dims;
    var cell = vec2<i32>(floor(p0));

    // Amanatides-Woo setup, parameterised by depth in meters rather than by
    // ray length. A zero-direction axis keeps its sentinel and is never
    // stepped, because the hit rules below always fire first.
    var t_max = vec2<f32>(SURFACE_DEPTH_FAR, SURFACE_DEPTH_FAR);
    var t_delta = vec2<f32>(SURFACE_DEPTH_FAR, SURFACE_DEPTH_FAR);
    var step_dir = vec2<i32>(0, 0);
    if abs(dir.x) > SURFACE_DEPTH_EPS {
        let positive = dir.x > 0.0;
        step_dir.x = select(-1, 1, positive);
        let boundary = select(f32(cell.x), f32(cell.x + 1), positive);
        t_max.x = (boundary - p0.x) / dir.x;
        t_delta.x = abs(1.0 / dir.x);
    }
    if abs(dir.y) > SURFACE_DEPTH_EPS {
        let positive = dir.y > 0.0;
        step_dir.y = select(-1, 1, positive);
        let boundary = select(f32(cell.y), f32(cell.y + 1), positive);
        t_max.y = (boundary - p0.y) / dir.y;
        t_delta.y = abs(1.0 / dir.y);
    }

    var z_enter = 0.0;
    // The first texel is entered through the plane itself, so its entry face is
    // the geometric top. That is what makes an all-zero field resolve on the
    // first iteration at depth 0 with the geometric normal and the original UV.
    var entry_normal_ts = vec3<f32>(0.0, 0.0, 1.0);
    var entry_bias = vec2<f32>(0.0, 0.0);
    var hit_depth = 0.0;
    var hit_normal_ts = vec3<f32>(0.0, 0.0, 1.0);
    var hit_bias = vec2<f32>(0.0, 0.0);
    var walked = 0u;

    loop {
        let solid = surface_depth_texel(cell, dims_i, base_mip, levels) * depth_scale_m;
        let z_exit = min(t_max.x, t_max.y);
        // The ray was already inside this texel's solid when it entered: it hit
        // the SIDE wall it came through.
        if z_enter >= solid {
            hit_depth = z_enter;
            hit_normal_ts = entry_normal_ts;
            hit_bias = entry_bias;
            break;
        }
        // The ray meets the TOP of this texel's solid before leaving it.
        if z_exit > solid {
            hit_depth = solid;
            hit_normal_ts = vec3<f32>(0.0, 0.0, 1.0);
            hit_bias = vec2<f32>(0.0, 0.0);
            break;
        }
        // Budget exhausted with the ray still in open space. Resolve HERE, at
        // the last boundary the walk actually crossed.
        //
        // The obvious alternative — treating this texel as unbounded so the TOP
        // rule fires — resolves at its full `solid` depth, and the sample point
        // is `p0 + dir * hit_depth` where `dir` is texels per METER OF DESCENT.
        // At a grazing angle that lands the albedo, normal and specular samples
        // tens of texels past anything the march visited, so a tighter budget
        // produced a LARGER artifact: `Low` cuts the cap to 8 while only halving
        // the fade that would have hidden it. Stopping at `z_enter` keeps the
        // sample inside the walked region, and on the first iteration it IS the
        // flat result (depth 0, geometric normal, original UV), so a budget too
        // small to march degrades toward flat rather than toward an arbitrary
        // texel.
        //
        // This is also what makes termination structural rather than a property
        // of the sampled values: the test is INTEGER, so the loop exits after
        // `max_steps` iterations whatever `solid` is — including a NaN, which
        // compares false against both hit rules.
        if walked + 1u >= max_steps {
            hit_depth = z_enter;
            hit_normal_ts = entry_normal_ts;
            hit_bias = entry_bias;
            break;
        }
        if t_max.x <= t_max.y {
            cell.x = cell.x + step_dir.x;
            z_enter = t_max.x;
            t_max.x = t_max.x + t_delta.x;
            // Normal = the crossed axis, negated.
            entry_normal_ts = vec3<f32>(-f32(step_dir.x), 0.0, 0.0);
            entry_bias = vec2<f32>(f32(step_dir.x) * SURFACE_DEPTH_SIDE_UV_BIAS_TEXELS, 0.0);
        } else {
            cell.y = cell.y + step_dir.y;
            z_enter = t_max.y;
            t_max.y = t_max.y + t_delta.y;
            entry_normal_ts = vec3<f32>(0.0, -f32(step_dir.y), 0.0);
            entry_bias = vec2<f32>(0.0, f32(step_dir.y) * SURFACE_DEPTH_SIDE_UV_BIAS_TEXELS);
        }
        walked = walked + 1u;
    }

    let hit_texel = p0 + dir * hit_depth;

    var out: SurfaceDepthResult;
    out.carved = true;
    out.uv = (hit_texel + hit_bias) / dims;
    out.march_uv = hit_texel / dims;
    // Stay on the view ray: the hit is where this pixel's ray meets the solid,
    // not a point pushed along the normal.
    out.world_position = world_position - view_to_eye * (hit_depth / descent);
    out.normal = normalize(
        tangent * hit_normal_ts.x + bitangent * hit_normal_ts.y + geo_normal * hit_normal_ts.z
    );
    out.hit_top = hit_normal_ts.z > 0.5;
    out.depth_m = hit_depth;
    out.depth_scale_m = depth_scale_m;
    out.fade = fade;
    out.quantize_levels = levels;
    // A shorter march than the view ray: self-shadow rays travel at most the
    // hit depth, and the budget is spent on the primary hit, not on lighting.
    out.shadow_steps = max(max_steps / 2u, 1u);
    out.shadow_light_budget =
        (packed >> SURFACE_DEPTH_SHADOW_BUDGET_SHIFT) & SURFACE_DEPTH_SHADOW_BUDGET_MASK;
    out.base_mip = base_mip;
    out.tangent = tangent;
    out.bitangent = bitangent;
    out.geo_normal = geo_normal;
    out.uv_per_m = uv_per_m;
    return out;
}

// Self-shadow one DYNAMIC light: a second, shorter DDA from the hit point
// toward the light. 1.0 lit, 0.0 occluded.
//
// Dynamic lights only. The bake knows nothing about dynamic bodies
// (rendering_pipeline.md §4: "the runtime owns only the facts that involve a
// dynamic body"), so this adds a fact no baked source owns. There is
// deliberately NO march against baked static light: the lightmap owns
// static-onto-static occlusion at 4 cm/texel and competing with it would risk
// the no-double-counting invariant.
//
// The texel the ray starts in is never tested, so a hit never shadows itself:
// a top hit sits exactly on that texel's solid, and a side hit sits on its
// wall — a light behind that wall is already culled by `NdotL <= 0`, because
// the wall normal IS the shading normal.
fn surface_depth_light_visibility(depth: SurfaceDepthResult, to_light: vec3<f32>) -> f32 {
    if !depth.carved || depth.depth_m <= SURFACE_DEPTH_SHADOW_BIAS_M {
        return 1.0;
    }
    let rise = dot(to_light, depth.geo_normal);
    // Written `!(x > lo)` like every other degenerate gate in this file, so a
    // NaN falls out to "lit" by construction rather than by the accident of a
    // downstream sentinel. Mirrors `surface_depth_light_ray`'s `above()`.
    if !(rise > SURFACE_DEPTH_EPS) {
        return 1.0;
    }

    let dims_u = textureDimensions(spec_texture, depth.base_mip);
    let dims = vec2<f32>(dims_u);
    let dims_i = vec2<i32>(dims_u);

    // Texels per meter of RISE.
    let dir = vec2<f32>(
        dot(to_light, depth.tangent) * depth.uv_per_m.x / rise,
        dot(to_light, depth.bitangent) * depth.uv_per_m.y / rise,
    ) * dims;

    let p0 = depth.march_uv * dims;
    var cell = vec2<i32>(floor(p0));
    var t_max = vec2<f32>(SURFACE_DEPTH_FAR, SURFACE_DEPTH_FAR);
    var t_delta = vec2<f32>(SURFACE_DEPTH_FAR, SURFACE_DEPTH_FAR);
    var step_dir = vec2<i32>(0, 0);
    if abs(dir.x) > SURFACE_DEPTH_EPS {
        let positive = dir.x > 0.0;
        step_dir.x = select(-1, 1, positive);
        let boundary = select(f32(cell.x), f32(cell.x + 1), positive);
        t_max.x = (boundary - p0.x) / dir.x;
        t_delta.x = abs(1.0 / dir.x);
    }
    if abs(dir.y) > SURFACE_DEPTH_EPS {
        let positive = dir.y > 0.0;
        step_dir.y = select(-1, 1, positive);
        let boundary = select(f32(cell.y), f32(cell.y + 1), positive);
        t_max.y = (boundary - p0.y) / dir.y;
        t_delta.y = abs(1.0 / dir.y);
    }

    for (var i: u32 = 0u; i < depth.shadow_steps; i = i + 1u) {
        // Rising past the hit depth means the ray has cleared the carved band.
        if min(t_max.x, t_max.y) >= depth.depth_m {
            return 1.0;
        }
        var risen: f32;
        if t_max.x <= t_max.y {
            cell.x = cell.x + step_dir.x;
            risen = t_max.x;
            t_max.x = t_max.x + t_delta.x;
        } else {
            cell.y = cell.y + step_dir.y;
            risen = t_max.y;
            t_max.y = t_max.y + t_delta.y;
        }
        let solid = surface_depth_texel(cell, dims_i, depth.base_mip, depth.quantize_levels)
            * depth.depth_scale_m;
        // Plateaus share exact quantized values, so equality must read as lit.
        if depth.depth_m - risen > solid + SURFACE_DEPTH_SHADOW_BIAS_M {
            return 0.0;
        }
    }
    return 1.0;
}
