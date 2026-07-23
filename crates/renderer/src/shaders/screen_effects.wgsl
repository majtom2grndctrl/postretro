// Post-UI screen-space effects resolve. Samples the offscreen `scene_color`
// target (every gameplay scene + UI pass renders into it) and writes the
// swapchain. The sole swapchain writer for the gameplay path — runs every
// frame, never skipped at rest.
//
// Tonemaps HDR scene color, then composes screen effects (flash / vignette /
// shake) on top.
// The effect values arrive in `EffectUniform`, packed CPU-side from the frame's
// `screen.flash` / `screen.vignette` / `screen.shake` slots (see
// render/screen_effects.rs::pack_effect_uniform). At rest every term is an exact
// no-op, so only the near-neutral tonemap changes an at-rest scene.
//
// `scene_color` is linear Rgba16Float. This shader samples it without an sRGB
// decode and writes an sRGB target, which performs the sole store conversion.
// See context/lib/rendering_pipeline.md §7.8.

@group(0) @binding(0) var scene_color_tex: texture_2d<f32>;
@group(0) @binding(1) var scene_color_sampler: sampler;

// Mirrors `EffectUniform` in render/screen_effects.rs.
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

@fragment
fn fs_main(in: VsOut) -> @location(0) vec4<f32> {
    // Shake: pure UV add (px→UV conversion already done CPU-side). At rest
    // `effect.shake == (0,0)`, so `in.uv + shake == in.uv` exactly — the sample
    // is the same NEAREST 1:1 texel passthrough as the identity blit.
    // Note: a large shake amplitude can smear the edge row/column because the
    // resolve sampler is ClampToEdge with no over-render margin.
    let sample_uv = in.uv + effect.shake;
    let scene = textureSample(scene_color_tex, scene_color_sampler, sample_uv);
    var color = soft_knee_tonemap(scene.rgb);

    // Vignette: tint/darken toward `vignette.rgb` near the edges, center
    // unaffected. The radial falloff is 0 at the center and rises toward the
    // corners; it is scaled by the authored strength `vignette.w`. At rest
    // `vignette.w == 0`, so `clamp(0 * radial, 0.0, 1.0) == 0.0` →
    // `mix(color, _, 0.0)` returns `color` unchanged.
    // Clamped here as a shader-side guard: over-1 vignette-strength would
    // otherwise extrapolate past the tint color.
    let centered = in.uv - vec2<f32>(0.5, 0.5);
    let radial = clamp(dot(centered, centered) * 2.0, 0.0, 1.0);
    let vignette_factor = clamp(effect.vignette.w * radial, 0.0, 1.0);
    color = mix(color, effect.vignette.xyz, vignette_factor);

    // Flash: over-blend toward `flash.rgb` by `flash.a`. At rest `flash.a == 0`,
    // so `clamp(0.0, 0.0, 1.0) == 0.0` → `mix(color, _, 0.0)` returns `color`
    // unchanged. Clamped here as a shader-side guard: over-1 alpha would
    // otherwise extrapolate past the flash color.
    color = mix(color, effect.flash.xyz, clamp(effect.flash.a, 0.0, 1.0));

    // Alpha is not part of the HDR tonemap; preserve the sampled coverage.
    return vec4<f32>(color, scene.a);
}
