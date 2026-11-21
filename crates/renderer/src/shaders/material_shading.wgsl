// Shared world-material WGSL helpers for specular, texel reconstruction and
// normal-map shading. Declares no bindings; see the note below.
// See: context/lib/rendering_pipeline.md §8

// The composing shader must supply `aniso_sampler` and `sample_post_retro`:
// `sample_normal` calls `sample_post_retro(tex, aniso_sampler, ...)`.

// No `(1-ks)` attenuation, no Fresnel — retro aesthetic wants punchy additive
// highlights, not energy conservation.
fn blinn_phong(L: vec3<f32>, V: vec3<f32>, N: vec3<f32>,
               color: vec3<f32>, spec_exp: f32, spec_int: f32) -> vec3<f32> {
    let H = normalize(L + V);
    let NdH = max(dot(N, H), 0.0);
    return color * pow(NdH, spec_exp) * spec_int;
}

// Post Retro texel-grid reconstruction: the UV `sample_post_retro` samples at,
// warped toward the nearest texel centre and antialiased only across the
// `fwidth`-wide seam band between texels. `dims` is the sampled texture's own
// size, since slots can differ in resolution. The specular point maps the same
// UV to world, so a texel's sample and its highlight cannot disagree.
fn post_retro_texel_uv(uv: vec2<f32>, dims: vec2<f32>, ddx: vec2<f32>,
                       ddy: vec2<f32>) -> vec2<f32> {
    let uv_tex = uv * dims;
    let seam = floor(uv_tex + 0.5);
    // Seam width comes from the ORIGINAL screen-space derivatives, not from
    // fwidth() of `uv_tex`. Under Surface Depth `uv` is the MARCHED UV, which
    // jumps between adjacent pixels that hit different texel faces, so its
    // fwidth measures the parallax step rather than the pixel footprint and
    // would widen the AA band into a multi-texel blur along every carved edge.
    // fwidth(uv * dims) == (abs(dpdx(uv)) + abs(dpdy(uv))) * dims for a smooth
    // uv, so this is the same quantity the un-marched path always computed.
    // Floor the divisor: a constant-UV fragment (edge-on face, degenerate UV
    // chart, vanishing derivatives) gives zero, and clamp() does not reliably
    // sanitize the resulting NaN/Inf in WGSL.
    let seam_width = max((abs(ddx) + abs(ddy)) * dims, vec2<f32>(1.0e-6));
    let aa = clamp((uv_tex - seam) / seam_width, vec2(-0.5), vec2(0.5));
    return (seam + aa) / dims;
}

// At or below this squared sine between a fragment's two UV derivatives its
// UV→world Jacobian is degenerate: an edge-on fragment or a collapsed UV chart.
const SPECULAR_POINT_AXES_EPSILON: f32 = 1.0e-8;

// Where a highlight's geometry is evaluated. `position` feeds light direction,
// falloff, range and cone; `view` is the unit vector toward the eye.
struct SpecularPoint {
    position: vec3<f32>,
    view: vec3<f32>,
};

// The pixel-style specular point (rendering_pipeline.md §4, Texel-centre
// highlight): the albedo texel centre `sample_post_retro` reads at `texel_uv`,
// mapped to world through the fragment's UV→world Jacobian from `anchor`, the
// world point of the fragment's own `uv`. Under a Surface Depth march
// `texel_uv` is the marched UV while `uv` and `anchor` stay the fragment's own,
// so every fragment landing on one texel takes one point. Mapping the
// reconstructed UV rather than the cell centre ramps the highlight across the
// seam band texture reads blend over. Smooth style, and a degenerate Jacobian,
// return `per_pixel` unchanged. Calls no derivative: the caller hoists them.
fn specular_texel_point(smooth_style: bool, per_pixel: SpecularPoint, uv: vec2<f32>,
                        texel_uv: vec2<f32>, anchor: vec3<f32>, eye: vec3<f32>,
                        dims: vec2<f32>, ddx: vec2<f32>, ddy: vec2<f32>,
                        ddx_world: vec3<f32>, ddy_world: vec3<f32>) -> SpecularPoint {
    if smooth_style {
        return per_pixel;
    }
    let det = ddx.x * ddy.y - ddy.x * ddx.y;
    let scale = dot(ddx, ddx) * dot(ddy, ddy);
    if det * det <= SPECULAR_POINT_AXES_EPSILON * scale {
        return per_pixel;
    }
    // World step per unit of u and of v: solves dpdx(world) = J dpdx(uv) and
    // the same for dpdy.
    let axis_u = (ddx_world * ddy.y - ddy_world * ddx.y) / det;
    let axis_v = (ddy_world * ddx.x - ddx_world * ddy.x) / det;
    let to_texel = post_retro_texel_uv(texel_uv, dims, ddx, ddy) - uv;
    let position = anchor + axis_u * to_texel.x + axis_v * to_texel.y;
    return SpecularPoint(position, normalize(eye - position));
}

// Per-texel sharpness and tint from the specular slot (resource_management.md
// §4.7). Mirrors render-cpu `specular_texel`, pinned by
// `specular_texel_shader_constants_match_the_cpu_reference`.
const SPECULAR_EXPONENT_MIN: f32 = 1.0;
const SPECULAR_EXPONENT_MAX: f32 = 8192.0;
// r⁴ at an exponent of 8198, a hair past the cap: byte 255 (r = 0) never
// divides by zero (clamp() does not reliably sanitize Inf in WGSL), and an
// approximate divide at the floor still clamps to exactly the cap.
const SPECULAR_ROUGHNESS4_FLOOR: f32 = 2.0 / 8200.0;
// Half a step inside each marker: B = 0 is "no `_p`", A = 255 "no `_t`", the
// values a narrower slot and the placeholder read. Filter rounding never
// crosses either.
const SPECULAR_SMOOTHNESS_MARKER: f32 = 0.5 / 255.0;
const SPECULAR_TINT_MARKER: f32 = 254.5 / 255.0;
// Stored A runs 0..=254, so t = A·255/254.
const SPECULAR_TINT_SCALE: f32 = 255.0 / 254.0;
// At or below this largest channel an albedo has no hue; it glints white.
const SPECULAR_HUE_EPSILON: f32 = 1.0e-4;

// What one read of the specular slot says about a texel's highlight: R
// intensity, B smoothness (the exponent; the prefix's without it), A tint.
// G is the Surface Depth march's height and is read there.
struct SpecularTexel {
    intensity: f32,
    exponent: f32,
    // Tinted: the highlight leaves the albedo multiply and takes
    // `specular_tint(albedo, tint)`. Untinted keeps the raw-albedo multiply.
    tinted: bool,
    tint: f32,
};

fn specular_texel(slot: vec4<f32>, prefix_exponent: f32) -> SpecularTexel {
    return SpecularTexel(
        slot.r,
        specular_texel_exponent(slot.b, prefix_exponent),
        slot.a < SPECULAR_TINT_MARKER,
        min(slot.a * SPECULAR_TINT_SCALE, 1.0),
    );
}

// Smoothness to a Blinn-Phong exponent: r = 1 − B, n = 2/r⁴ − 2, clamped to
// 1..=8192, with r⁴ floored before the divide. Without smoothness (B below the
// marker) the material's prefix exponent, unchanged.
fn specular_texel_exponent(smoothness: f32, prefix_exponent: f32) -> f32 {
    let r = 1.0 - smoothness;
    let r2 = r * r;
    let r4 = max(r2 * r2, SPECULAR_ROUGHNESS4_FLOOR);
    let decoded = clamp(2.0 / r4 - 2.0, SPECULAR_EXPONENT_MIN, SPECULAR_EXPONENT_MAX);
    return select(prefix_exponent, decoded, smoothness >= SPECULAR_SMOOTHNESS_MARKER);
}

// A tinted highlight's colour: a mix from white (t = 0) to the albedo's hue at
// full brightness (t = 1), the albedo over its largest channel, white where it
// has none. It multiplies the highlight in place of the albedo, so an author
// can dim a metal's diffuse without dimming its glint. Diffuse never changes.
fn specular_tint(albedo: vec3<f32>, tint: f32) -> vec3<f32> {
    let peak = max(albedo.r, max(albedo.g, albedo.b));
    let hue = select(
        vec3<f32>(1.0),
        albedo / max(peak, SPECULAR_HUE_EPSILON),
        peak > SPECULAR_HUE_EPSILON,
    );
    return mix(vec3<f32>(1.0), hue, tint);
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

fn reconstruct_tbn_normal(
    mesh_n: vec3<f32>,
    world_tangent: vec3<f32>,
    bitangent_sign: f32,
    n_ts: vec3<f32>,
) -> vec3<f32> {
    // Degenerate-tangent guard: meshes with collapsed UVs produce zero-length
    // tangents. Skip TBN in that case to avoid NaN propagation.
    const TBN_EPS: f32 = 1.0e-4;
    if dot(world_tangent, world_tangent) >= TBN_EPS * TBN_EPS {
        // Gram-Schmidt: project out mesh_n component so T stays in the tangent plane.
        let T = normalize(world_tangent - mesh_n * dot(world_tangent, mesh_n));
        let B = cross(mesh_n, T) * bitangent_sign;
        let TBN = mat3x3<f32>(T, B, mesh_n);
        let n_ts_world = TBN * n_ts;
        if dot(n_ts_world, n_ts_world) >= TBN_EPS * TBN_EPS {
            return normalize(n_ts_world);
        }
    }
    return mesh_n;
}
