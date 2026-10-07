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
// G = the SIGNED HEIGHT field, stored as `255 - h` where `h` is the authored
// `_h.png` byte. Authored mid-gray (h = 128) is the polygon plane; darker sinks
// below it, lighter rises above it, each by up to the material's depth. A
// material with no `_h.png` sibling binds a 1x1 black R8Unorm placeholder that
// WGSL expands to (r, 0, 0, 1) — and `g = 0` now reads as MAXIMUM RAISE, not
// flat. The has-depth bit in the packed march word, set only for an `Rg8Unorm`
// slot, is therefore the ONLY guard: the march never runs without it.
//
// The field is piecewise-constant per texel: a grid of boxes whose lattice is
// the SAME texel grid `sample_post_retro` snaps albedo to, so stone side faces
// land exactly on albedo texel edges. That alignment is the point. Marching a
// grid of boxes is an exact 2D DDA (Amanatides-Woo), not a fixed-step POM —
// there is no sampling error to trade against step count.
//
// The relief rises as well as sinks. The march starts at the material's PEAK
// raise (uniform bytes 8..12) and walks only the band down to its TROUGH (bytes
// 12..16). Each uploaded mip's band is measured at load; the uniform carries
// the band of the mip the march reads. Raised texels are
// accepted artifacts, not mitigated: the polygon's silhouette stays flat, a
// raised edge slices at the polygon boundary, and nothing writes depth, so
// feet, props and projectiles draw at the true plane. Collision is untouched.
//
// HARD CONSTRAINTS THIS CODE HONORS
//  * It never writes `@builtin(frag_depth)`. The depth pre-pass is vertex-only
//    and the forward pass runs `depth_compare: Equal` with depth writes off; a
//    fragment that wrote depth would fail its own equality test. The technique
//    is depth-free by construction.
//  * It offsets `base_uv` ONLY. `lightmap_texel` is never touched — lightmap
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
// The CPU authority for the march, the packing and every tuning constant
// below is `postretro_render_cpu::surface_depth`, which is unit-tested without
// a GPU. The sections below follow its modules — `height.rs` (encoding),
// `uniform.rs`, `shading.rs` (fade, AO), `march.rs` (the DDAs) — so the two
// read side by side. A function mirroring one CPU function keeps its name,
// with one exception: `surface_depth_indirect_ao` mirrors
// `surface_depth_ambient_occlusion`. `surface_depth_resolve` composes the CPU's
// `surface_depth_basis`, `surface_depth_view_ray` and `march_surface_depth`.
// Parity is on RESULTS, not code shape: the view march below has no
// single-texel early-out, and the light march steps a pre-folded texel
// coordinate; neither changes a result. One rule is GPU-only and has no CPU
// mirror: the texel→meters conversion in the resolve (`texels_per_m`,
// `texel_rate`, `depth_scale_m` under `SURFACE_DEPTH_TEXEL_MODE`) needs a
// per-fragment UV Jacobian that only exists mid-shader. That is accepted
// because this is a purely graphical relief — collision uses the true brush
// plane, so a wrong conversion is a visible on-screen error, not a corrupted
// game-logic value, and nothing downstream (no save data, no netcode) depends
// on it.

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
// than a `const` because the player-facing on/off switch is applied
// by rewriting this BUFFER — this engine has no shader-variant system, so a
// switch that must turn the shadow march off has to reach the shader as data.
// Zero means the second (shadow) DDA never runs.
const SURFACE_DEPTH_SHADOW_BUDGET_SHIFT: u32 = 16u;
const SURFACE_DEPTH_SHADOW_BUDGET_MASK: u32 = 0xFu;

const SURFACE_DEPTH_FADE_LOD_START: f32 = 1.0;
const SURFACE_DEPTH_FADE_LOD_RANGE: f32 = 2.0;
const SURFACE_DEPTH_FADE_DISTANCE_FRACTION: f32 = 0.25;
const SURFACE_DEPTH_AO_STRENGTH: f32 = 0.75;
const SURFACE_DEPTH_SHADOW_BIAS_M: f32 = 1.0e-4;
const SURFACE_DEPTH_SIDE_UV_BIAS_TEXELS: f32 = 0.5;
// Hard ceiling on the RESOLVED height in EACH direction, in meters, whatever
// unit it was authored in. Mirrors `postretro_render_data::material::SURFACE_DEPTH_MAX_METERS`.
//
// In meters mode the CPU clamp already bounds this. In texel mode it does NOT:
// the CPU caps a TEXEL COUNT, and the per-fragment divide by the texel rate can
// turn a legal count into an arbitrarily tall relief on a coarsely-scaled face.
// Collision still uses the true plane, so an unbounded relief diverges from it
// visibly and pushes `world_position` — which feeds dynamic light direction and
// attenuation — off the surface with it.
const SURFACE_DEPTH_MAX_METERS: f32 = 0.2;
// Unit of `material.surface_depth_meters`: 0 = world meters, 1 = albedo texels.
// Mirrors `postretro_render_data::material::SURFACE_DEPTH_TEXEL_MODE`, which
// selects the matching authoring table; the two are pinned against each other.
const SURFACE_DEPTH_TEXEL_MODE: u32 = 1u;

// Signed height encoding. The stored G byte is `255 - h` for authored `h`;
// mid-gray (`h = 128`) is the polygon plane. Mirrors `height.rs`.
const SURFACE_HEIGHT_BYTE_MAX: f32 = 255.0;
const SURFACE_HEIGHT_PLANE_BYTE: f32 = 128.0;

// `LightTermMask::DEPTH_AMBIENT_OCCLUSION`. Bit 8 — bit 7 stays reserved for
// the intentionally unwired emissive category.
const LIGHT_TERM_DEPTH_AO: u32 = 0x100u;

struct SurfaceDepthResult {
    // False whenever the fragment must render exactly as it did before this
    // feature existed: no surface map bound, a flat material, a faded-out
    // surface, a degenerate UV chart, an edge-on fragment, or a march that
    // starved its step budget.
    carved: bool,
    // UV to sample the material's textures at. Biased half a texel past the
    // crossed boundary on a side hit so `sample_post_retro` reads the stone's
    // own color rather than blending across the edge it just hit.
    uv: vec2<f32>,
    // UV of the hit point itself. Self-shadow marches start here.
    march_uv: vec2<f32>,
    // Hit point on the view ray. Feeds dynamic light direction and
    // attenuation; it is deliberately NOT used for shadow-map lookups, which
    // must stay on the true plane the depth maps were rendered from. A raised
    // hit lies toward the camera.
    world_position: vec3<f32>,
    // World-space normal of the face that was hit: the geometric normal on a
    // top hit, an exact +/-U or +/-V axis on a side hit. The consumer applies
    // the normal map on top hits only.
    normal: vec3<f32>,
    hit_top: bool,
    // Signed height of the hit above the true plane, in meters. POSITIVE is
    // raised toward the viewer. |height_m| <= depth_scale_m.
    height_m: f32,
    // Post-fade relief scale for this fragment, in meters: the height of a
    // texel at +/-1.0.
    depth_scale_m: f32,
    // The material's quantized peak raise as a fraction of `depth_scale_m`, in
    // [0, 1]. AO and the self-shadow march measure from it, not from the plane.
    peak_raise: f32,
    // The distance/LOD fade that produced `depth_scale_m`, in [0, 1]. Carried
    // out of the resolve because height-derived terms whose inputs are BOTH
    // post-fade cancel it out and would pop at the fade boundary instead of
    // degrading. See `surface_depth_indirect_ao` (CPU:
    // `surface_depth_ambient_occlusion`).
    fade: f32,
    quantize_levels: f32,
    shadow_steps: u32,
    // How many DYNAMIC lights this fragment may self-shadow, from the player's
    // Surface Depth switch. Zero at `Off`, and zero whenever `carved` is
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
    out.height_m = 0.0;
    out.depth_scale_m = 0.0;
    out.peak_raise = 0.0;
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

// ---- height.rs ----------------------------------------------------------

// Signed height fraction of one texel from its stored G value:
// `s = (h - 128) / 128`, positive = RAISED, in [-1, 127/128].
//
// The byte is recovered first with `floor(x + 0.5)`, so every step after it is
// integer arithmetic over a power-of-two divisor and exact on every backend.
// `255 * (1 - g)` computed directly would not be: `g` is not exactly
// representable, and a fused multiply-add would leave mid-gray a hair off zero.
fn surface_height_fraction(stored_g: f32) -> f32 {
    let stored = floor(stored_g * SURFACE_HEIGHT_BYTE_MAX + 0.5);
    let authored = SURFACE_HEIGHT_BYTE_MAX - stored;
    return (authored - SURFACE_HEIGHT_PLANE_BYTE) / SURFACE_HEIGHT_PLANE_BYTE;
}

// Quantize a signed fraction onto `levels` terraces PER DIRECTION. `floor(x +
// 0.5)`, never the builtin rounding function: WGSL rounds half to even and the
// CPU authority rounds half up, and an exact half step is reachable. Mid-gray
// yields exactly 0.0 at every level count.
fn surface_height_quantize(s: f32, levels: f32) -> f32 {
    if levels >= 1.0 {
        return clamp(floor(s * levels + 0.5) / levels, -1.0, 1.0);
    }
    return s;
}

// Quantized signed height of one texel, at a coordinate already folded into
// the tile (`surface_depth_fold`).
//
// Quantization is a live aesthetic dial: neighbouring texels snap onto shared
// plateaus, giving fewer and larger terraces. It does not affect the DDA's
// exactness — that comes from the field being constant per texel, not from the
// value landing on a plateau.
fn surface_depth_texel(folded: vec2<i32>, level: u32, levels: f32) -> f32 {
    let stored_g = textureLoad(spec_texture, folded, level).g;
    return surface_height_quantize(surface_height_fraction(stored_g), levels);
}

// Fold a texel coordinate into the tile. `textureLoad` does not wrap, so the
// tile is folded by hand — `base_uv` samples through `AddressMode::Repeat` and
// the march is free to walk off the edge of the texture.
fn surface_depth_fold(coord: vec2<i32>, dims: vec2<i32>) -> vec2<i32> {
    let folded = coord % dims;
    return select(folded + dims, folded, folded >= vec2<i32>(0, 0));
}

// Step a folded coordinate one texel along one axis (`step` is -1, 0 or 1),
// wrapping at the tile edge. Equal to folding the unfolded coordinate after the
// same step, because the input is already inside `[0, dim)`.
fn surface_depth_fold_step(folded: i32, step: i32, dim: i32) -> i32 {
    let next = folded + step;
    return select(select(next, next - dim, next >= dim), next + dim, next < 0);
}

// ---- uniform.rs ---------------------------------------------------------

// The has-depth bit is the ONLY guard against the R8 placeholder, whose `g = 0`
// reads as maximum raise. The march must never run when it is clear.
fn surface_depth_has_map() -> bool {
    return (material.surface_depth_march & SURFACE_DEPTH_HAS_DEPTH_BIT) != 0u
        && material.surface_depth_meters > 0.0;
}

// ---- shading.rs ---------------------------------------------------------

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
// flattens gracefully instead of popping. It is also a straight perf
// win: at distance the texels go sub-pixel and the parallax is invisible.
fn surface_depth_lod_fade(lod: f32) -> f32 {
    return clamp(1.0 - (lod - SURFACE_DEPTH_FADE_LOD_START) / SURFACE_DEPTH_FADE_LOD_RANGE, 0.0, 1.0);
}

// Ambient occlusion for the SH INDIRECT term only, measured from the material's
// PEAK raise rather than the plane: mortar between raised stones darkens
// by its depth below the stone tops wherever the author put the plane. An
// all-mid-gray map or a texel at the peak gets none. Legitimate because SH
// probes sit at ~1 m spacing and "know nothing of the receiver's own geometry"
// (rendering_pipeline.md §4) — cobblestone-scale self-occlusion is a fact no
// other source owns, so this is not double-counting a light.
fn surface_depth_indirect_ao(depth: SurfaceDepthResult, light_terms: u32) -> f32 {
    if !depth.carved || (light_terms & LIGHT_TERM_DEPTH_AO) == 0u {
        return 1.0;
    }
    if !(depth.depth_scale_m > SURFACE_DEPTH_EPS) {
        return 1.0;
    }
    // Scale by the fade. `height_m` and `depth_scale_m` are both post-fade, so
    // their ratio is the raw texel value at EVERY fade — without this factor a
    // surface one epsilon inside the fade boundary still occludes at full
    // strength and then snaps to 1.0 the moment it crosses, which reads as a
    // moving arc of brightness as the LOD isoline sweeps the floor.
    return 1.0
        - SURFACE_DEPTH_AO_STRENGTH
            * depth.fade
            * clamp(depth.peak_raise - depth.height_m / depth.depth_scale_m, 0.0, 1.0);
}

// ---- march.rs -----------------------------------------------------------

// Amanatides-Woo state for a 2D walk over the texel grid.
struct SurfaceDepthDda {
    cell: vec2<i32>,
    // Vertical distance (meters) at which the ray next crosses each axis.
    t_max: vec2<f32>,
    // Vertical distance (meters) between successive crossings on each axis.
    t_delta: vec2<f32>,
    step_dir: vec2<i32>,
};

// Set up the walk from `origin` (texels) along `dir` (texels per vertical
// meter). A zero-direction axis keeps its sentinel and is never stepped,
// because the hit rules in the march always fire first.
fn surface_depth_dda_setup(origin: vec2<f32>, dir: vec2<f32>) -> SurfaceDepthDda {
    var dda: SurfaceDepthDda;
    dda.cell = vec2<i32>(floor(origin));
    dda.t_max = vec2<f32>(SURFACE_DEPTH_FAR, SURFACE_DEPTH_FAR);
    dda.t_delta = vec2<f32>(SURFACE_DEPTH_FAR, SURFACE_DEPTH_FAR);
    dda.step_dir = vec2<i32>(0, 0);
    if abs(dir.x) > SURFACE_DEPTH_EPS {
        let positive = dir.x > 0.0;
        dda.step_dir.x = select(-1, 1, positive);
        let boundary = select(f32(dda.cell.x), f32(dda.cell.x + 1), positive);
        dda.t_max.x = (boundary - origin.x) / dir.x;
        dda.t_delta.x = abs(1.0 / dir.x);
    }
    if abs(dir.y) > SURFACE_DEPTH_EPS {
        let positive = dir.y > 0.0;
        dda.step_dir.y = select(-1, 1, positive);
        let boundary = select(f32(dda.cell.y), f32(dda.cell.y + 1), positive);
        dda.t_max.y = (boundary - origin.y) / dir.y;
        dda.t_delta.y = abs(1.0 / dir.y);
    }
    return dda;
}

// Resolve a fragment's relief: derive the surface frame, fade, march the
// material's relief band, and report the hit. `view_to_eye` points from the
// surface toward the camera.
//
// The UV frame comes from `dpdx(world_position) / dpdx(uv)` rather than from
// the baked tangent on purpose. The march works in METERS whichever unit the
// height was authored in (texel mode converts below, with this fragment's own
// texel rate), and brush UV scale is authored freely in TrenchBroom. Taking the
// march axes AND the meters->UV scale from one Jacobian makes them consistent
// by construction. The baked tangent still owns normal mapping, which is a
// different, authored tangent space.
//
// The march measures DESCENT from where the ray starts: the peak raise, or the
// eye if lower. Per texel `T` the solid's top lies `solid(T) = (top - s(T)) *
// scale` below the start.
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
    // fails this the same way — means this material does not relieve. Taking
    // the early-out here keeps the degenerate-chart work below off a flat
    // material.
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

    // Relief scale, in meters, whichever unit it was authored in.
    //
    // In TEXEL mode the authored value is a count of albedo texels and is
    // converted with THIS fragment's texel rate, so the relief is a fixed
    // height in the texel lattice rather than in the world: a 2048px texture on
    // a small brush rises the same number of texels as a 256px one on a large
    // brush. The geometric mean is the neutral reading of a rate that differs
    // per axis — on the square-texel faces this engine's brushes normally
    // produce, either axis gives the same answer.
    //
    // It also bounds the march. Horizontal travel through the relief is
    // `height_m * dir`, and `dir` is texels per meter of descent, so the texel
    // rate cancels: travel is `N * tan(theta)` texels regardless of texture
    // resolution or brush scale. In meters mode it does not cancel, which is
    // why a high-resolution texture on a small brush can exhaust the budget.
    var depth_scale_m = carve_request;
    if SURFACE_DEPTH_TEXEL_MODE == 1u {
        let texels_per_m = uv_per_m * dims;
        let texel_rate = sqrt(max(texels_per_m.x * texels_per_m.y, SURFACE_DEPTH_EPS));
        depth_scale_m = carve_request / texel_rate;
    }
    // Clamp rather than bail: a face scaled past the ceiling should flatten
    // gracefully, not pop to unmarched. `fade` scales the ceiling too so the
    // clamp cannot re-grow a relief the fade is busy closing.
    depth_scale_m = min(depth_scale_m, SURFACE_DEPTH_MAX_METERS * fade);
    if !(depth_scale_m > SURFACE_DEPTH_EPS) {
        return flat_result;
    }

    // The relief band, from the uniform (quantized on the CPU with this
    // material's level count). An empty band — an all-mid-gray map — marches
    // nothing, so it costs the same as having no map.
    let peak = material.surface_depth_peak_raise;
    let trough = material.surface_depth_trough;
    let band_m = (peak - trough) * depth_scale_m;
    if !(band_m > 0.0) {
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
    // Eye bound: the march never starts behind the camera. The eye sits
    // `view_distance * descent` above the plane, so an eye inside the band (a
    // low slide eye, a camera hugging raised brick) starts the ray at the eye.
    // A far eye leaves `top == peak` exactly.
    let top = min(peak, view_distance * descent / depth_scale_m);
    let top_m = top * depth_scale_m;
    // The ray enters the band at the peak (or the eye), `top_m` above the
    // plane, which is `dir * top_m` texels back toward the viewer from `p0`.
    let start = p0 - dir * top_m;
    var dda = surface_depth_dda_setup(start, dir);

    // The first texel is entered through the start height, so its entry face
    // is the geometric top. That is what makes a texel at the peak resolve on
    // the first iteration at zero descent with the geometric normal.
    var z_hit = 0.0;
    var hit_normal_ts = vec3<f32>(0.0, 0.0, 1.0);
    var hit_bias = vec2<f32>(0.0, 0.0);

    // No single-texel early-out here, unlike the CPU authority: it
    // resolves exactly what the loop's first iteration does, so dropping it
    // changes no result. Measured on AMD Metal, the extra branch made the
    // whole forward shader slower, even with Surface Depth off.
    //
    // The view loop keeps folding each fetch and returning from inside on
    // starvation. Stepping a pre-folded coordinate, or breaking out to resolve
    // below, each cut the loop body by a third, yet each made every fragment
    // slower — Surface Depth off included — through how the compiler laid out
    // the rest of the shader. So did removing the scope block below, which
    // pushed the mover pipeline from 121 to 186 registers. Measure on the AMD
    // Mac before reshaping this function.
    {
        var z_enter = 0.0;
        var entry_normal_ts = vec3<f32>(0.0, 0.0, 1.0);
        var entry_bias = vec2<f32>(0.0, 0.0);
        var walked = 0u;

        loop {
            // The eye's own column is see-through: with the eye inside the
            // band every ray starts at its foot, so a texel there rising above
            // the eye would stop them all at one point. It reads as the band's
            // floor; every other texel blocks as usual.
            let s = surface_depth_texel(surface_depth_fold(dda.cell, dims_i), base_mip, levels);
            let solid = (top - select(s, trough, walked == 0u && s > top)) * depth_scale_m;
            let z_exit = min(dda.t_max.x, dda.t_max.y);
            // The ray was already inside this texel's solid when it entered: it
            // hit the SIDE wall it came through.
            if z_enter >= solid {
                z_hit = z_enter;
                hit_normal_ts = entry_normal_ts;
                hit_bias = entry_bias;
                break;
            }
            // The ray meets the TOP of this texel's solid before leaving it.
            if z_exit > solid {
                z_hit = solid;
                break;
            }
            // Budget exhausted with the ray still in open space: resolve FLAT at
            // the true plane — original UV, height 0, geometric normal, top
            // hit, and `carved = false` so the consumer skips AO and self-shadow
            // exactly as it does for no march. Resolving at the last crossed
            // boundary instead smeared the texture toward the viewer at grazing
            // angles, because `dir` is texels per meter of descent; flat reads as
            // the effect switched off for this fragment, which is honest.
            //
            // Termination is structural rather than a property of the sampled
            // values: this test is INTEGER, so the loop exits after `max_steps`
            // iterations whatever `solid` is — including a NaN, which compares
            // false against both hit rules.
            if walked + 1u >= max_steps {
                return flat_result;
            }
            if dda.t_max.x <= dda.t_max.y {
                dda.cell.x = dda.cell.x + dda.step_dir.x;
                z_enter = dda.t_max.x;
                dda.t_max.x = dda.t_max.x + dda.t_delta.x;
                // Normal = the crossed axis, negated.
                entry_normal_ts = vec3<f32>(-f32(dda.step_dir.x), 0.0, 0.0);
                entry_bias = vec2<f32>(f32(dda.step_dir.x) * SURFACE_DEPTH_SIDE_UV_BIAS_TEXELS, 0.0);
            } else {
                dda.cell.y = dda.cell.y + dda.step_dir.y;
                z_enter = dda.t_max.y;
                dda.t_max.y = dda.t_max.y + dda.t_delta.y;
                entry_normal_ts = vec3<f32>(0.0, -f32(dda.step_dir.y), 0.0);
                entry_bias = vec2<f32>(0.0, f32(dda.step_dir.y) * SURFACE_DEPTH_SIDE_UV_BIAS_TEXELS);
            }
            walked = walked + 1u;
        }
    }

    let hit_texel = start + dir * z_hit;
    let height_m = top_m - z_hit;

    var out: SurfaceDepthResult;
    out.carved = true;
    out.uv = (hit_texel + hit_bias) / dims;
    out.march_uv = hit_texel / dims;
    // Stay on the view ray: the hit is where this pixel's ray meets the solid,
    // not a point pushed along the normal. A raised hit lies toward the camera.
    out.world_position = world_position + view_to_eye * (height_m / descent);
    out.normal = normalize(
        tangent * hit_normal_ts.x + bitangent * hit_normal_ts.y + geo_normal * hit_normal_ts.z
    );
    out.hit_top = hit_normal_ts.z > 0.5;
    out.height_m = height_m;
    out.depth_scale_m = depth_scale_m;
    out.peak_raise = peak;
    out.fade = fade;
    out.quantize_levels = levels;
    // A shorter march than the view ray: the budget is spent on the primary
    // hit, not on lighting.
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
// The ray rises from the hit and ends as soon as it climbs above the material's
// PEAK raise — not the plane: with raised texels around, a hit on the
// plane can still be shadowed. A top hit at the peak height has nothing above
// it and skips the march.
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
    if !depth.carved {
        return 1.0;
    }
    // How far the light ray must rise before it clears the band.
    let clearance = depth.peak_raise * depth.depth_scale_m - depth.height_m;
    if !(clearance > SURFACE_DEPTH_SHADOW_BIAS_M) {
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
    var dda = surface_depth_dda_setup(p0, dir);
    // Fold once, then step the folded coordinate: a signed `%` per fetch lowers
    // to an integer divide plus naga's guards, about sixty instructions.
    var texel = surface_depth_fold(dda.cell, dims_i);

    for (var i: u32 = 0u; i < depth.shadow_steps; i = i + 1u) {
        // Rising past the peak means the ray has left the relief band.
        if min(dda.t_max.x, dda.t_max.y) >= clearance {
            return 1.0;
        }
        var risen: f32;
        if dda.t_max.x <= dda.t_max.y {
            texel.x = surface_depth_fold_step(texel.x, dda.step_dir.x, dims_i.x);
            risen = dda.t_max.x;
            dda.t_max.x = dda.t_max.x + dda.t_delta.x;
        } else {
            texel.y = surface_depth_fold_step(texel.y, dda.step_dir.y, dims_i.y);
            risen = dda.t_max.y;
            dda.t_max.y = dda.t_max.y + dda.t_delta.y;
        }
        // Both sides measured down from the peak: the ray sits `clearance -
        // risen` below it, the texel's top `solid` below it. Plateaus share
        // exact quantized values, so equality must read as lit.
        let solid = (depth.peak_raise
            - surface_depth_texel(texel, depth.base_mip, depth.quantize_levels))
            * depth.depth_scale_m;
        if clearance - risen > solid + SURFACE_DEPTH_SHADOW_BIAS_M {
            return 0.0;
        }
    }
    return 1.0;
}
