// Post-UI screen-space effects resolve. Samples the offscreen `scene_color`
// target (every gameplay scene + UI pass renders into it) and writes the
// swapchain. The sole swapchain writer for the gameplay path — runs every
// frame, never skipped at rest.
//
// Tonemaps HDR scene color, then composes screen effects (flash / vignette /
// shake) on top, all inside `compose_presented`. The flash limiter's measure
// pass (`flash_limiter.wgsl`, appended to this source) calls the same function,
// so it measures exactly what the resolve presents; the resolve then applies
// the limiter's per-cell result.
// Windowed effect values are packed CPU-side from the frame's screen-effect
// slots. Capture binds the same uniform at its default, at-rest value and skips
// the limiter. At rest every effect term is an exact no-op, so only the
// near-neutral tonemap applies.
//
// `scene_color` is linear Rgba16Float. This shader samples it without an sRGB
// decode and writes an sRGB target, which performs the sole store conversion.
// See context/lib/rendering_pipeline.md §7.8.

@group(0) @binding(0) var scene_color_tex: texture_2d<f32>;
@group(0) @binding(1) var scene_color_sampler: sampler;

// Mirrors `EffectUniform` in render-cpu/src/screen_effects.rs.
//   flash    — rgba; `flash.a` is the over-blend weight (0 at rest → no-op).
//   vignette — `xyz` linear tint + `w` strength (0 at rest → no edge tint).
//   shake    — UV offset (px→UV conversion done CPU-side); (0,0) at rest.
struct EffectUniform {
    flash: vec4<f32>,
    vignette: vec4<f32>,
    shake: vec2<f32>,
    _pad: vec2<f32>,
}
@group(0) @binding(2) var<uniform> effect: EffectUniform;

// Flash limiter grid: a fixed 16×9 at any resolution. Mirrors
// `LIMITER_CELLS_X` / `LIMITER_CELLS_Y` in render-cpu/src/flash_limiter.rs.
const LIMITER_CELLS_X: u32 = 16u;
const LIMITER_CELLS_Y: u32 = 9u;
const LIMITER_CELL_COUNT: u32 = 144u;

// Rec. 709 relative-luminance weights for linear display values.
const LUMA: vec3<f32> = vec3<f32>(0.2126, 0.7152, 0.0722);

// One cell's limiter result, written by `cs_limit_cells`. Identity is
// `gain = 1`, `blend = 0`, `desaturate = 0`.
//   gain       — scales the composite down toward a lower target luminance.
//   blend      — mixes toward `blend_rgb` (the cell's last presented mean
//                color) when the target lies above what the frame holds.
//   desaturate — mixes toward the pixel's own luminance (red limiting).
struct LimiterCellParams {
    gain: f32,
    blend: f32,
    desaturate: f32,
    _pad: f32,
    blend_rgb: vec4<f32>,
}
@group(1) @binding(5) var<storage, read> presented_cell_params: array<LimiterCellParams, 144>;

// The limiter cell holding framebuffer pixel `px`. The measure pass assigns
// pixels to cells by the same rule, so both passes agree on every boundary.
fn limiter_cell_of(px: vec2<u32>, dims: vec2<u32>) -> u32 {
    let cx = min(px.x * LIMITER_CELLS_X / dims.x, LIMITER_CELLS_X - 1u);
    let cy = min(px.y * LIMITER_CELLS_Y / dims.y, LIMITER_CELLS_Y - 1u);
    return cy * LIMITER_CELLS_X + cx;
}

struct VsOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) uv: vec2<f32>,
}

@vertex
fn vs_main(@builtin(vertex_index) vid: u32) -> VsOut {
    // Fullscreen triangle — three verts, covers the full clip-space quad.
    //   vid=0 → (-1,-1,0) / uv (0,1)
    //   vid=1 → ( 3,-1,0) / uv (2,1)
    //   vid=2 → (-1, 3,0) / uv (0,-1)
    var out: VsOut;
    let x = f32((vid << 1u) & 2u) * 2.0 - 1.0;
    let y = f32(vid & 2u) * 2.0 - 1.0;
    out.clip = vec4<f32>(x, y, 0.0, 1.0);
    out.uv = vec2<f32>((x + 1.0) * 0.5, (1.0 - y) * 0.5);
    return out;
}

// Preserve the palette below a very late knee, then compress HDR overshoot by
// the scene's peak channel so hue remains stable. The 0.98 knee is within one
// percent of unity at 1.0, but avoids a hard clip and asymptotically approaches
// the display ceiling for bright emissive/bloom content.
fn soft_knee_tonemap(color: vec3<f32>) -> vec3<f32> {
    let peak = max(max(color.r, color.g), color.b);
    let knee_start = 0.98;
    if peak <= knee_start {
        return color;
    }
    let knee_width = 1.0 - knee_start;
    let excess = peak - knee_start;
    let compressed_peak = knee_start + knee_width * excess / (excess + knee_width);
    return color * (compressed_peak / peak);
}

// The presented composite for one pixel: shake → soft-knee tonemap →
// vignette → flash, clamped to the display range the sRGB store would clamp to.
// The one place these terms apply: the resolve, the capture tonemap and the
// flash limiter's measure pass all call it.
fn compose_presented(uv: vec2<f32>) -> vec4<f32> {
    // Shake: pure UV add (px→UV conversion already done CPU-side). At rest
    // `effect.shake == (0,0)`, so the sample is the same NEAREST 1:1 texel
    // passthrough as the identity blit. `textureSampleLevel` rather than
    // `textureSample` so the compute measure pass can call this too; with one
    // mip and a NEAREST sampler the two are identical.
    // Note: a large shake amplitude can smear the edge row/column because the
    // resolve sampler is ClampToEdge with no over-render margin.
    let sample_uv = uv + effect.shake;
    let scene = textureSampleLevel(scene_color_tex, scene_color_sampler, sample_uv, 0.0);
    var color = soft_knee_tonemap(scene.rgb);

    // Vignette: tint/darken toward `vignette.rgb` near the edges, center
    // unaffected. The radial falloff is 0 at the center and rises toward the
    // corners; it is scaled by the authored strength `vignette.w`. At rest
    // `vignette.w == 0`, so `mix(color, _, 0.0)` returns `color` unchanged.
    // Clamped here as a shader-side guard: over-1 vignette-strength would
    // otherwise extrapolate past the tint color.
    let centered = uv - vec2<f32>(0.5, 0.5);
    let radial = clamp(dot(centered, centered) * 2.0, 0.0, 1.0);
    let vignette_factor = clamp(effect.vignette.w * radial, 0.0, 1.0);
    color = mix(color, effect.vignette.xyz, vignette_factor);

    // Flash: over-blend toward `flash.rgb` by `flash.a`. At rest `flash.a == 0`,
    // so `mix(color, _, 0.0)` returns `color` unchanged. Clamped here as a
    // shader-side guard: over-1 alpha would otherwise extrapolate past the
    // flash color.
    color = mix(color, effect.flash.xyz, clamp(effect.flash.a, 0.0, 1.0));

    // Alpha is not part of the HDR tonemap; preserve the sampled coverage.
    return vec4<f32>(clamp(color, vec3<f32>(0.0), vec3<f32>(1.0)), scene.a);
}

@fragment
fn fs_main(in: VsOut) -> @location(0) vec4<f32> {
    let composed = compose_presented(in.uv);
    let dims = textureDimensions(scene_color_tex);
    let cell = presented_cell_params[limiter_cell_of(vec2<u32>(in.clip.xy), dims)];
    var color = mix(composed.rgb * cell.gain, cell.blend_rgb.rgb, cell.blend);
    color = mix(color, vec3<f32>(dot(color, LUMA)), cell.desaturate);
    return vec4<f32>(color, composed.a);
}

// Capture tonemap: the composite alone. Capture never presents, so it stays
// outside the flash limiter and never reads its per-cell result.
@fragment
fn fs_capture(in: VsOut) -> @location(0) vec4<f32> {
    return compose_presented(in.uv);
}
