// Post-UI screen-space effects resolve. Samples the offscreen `scene_color`
// target (every gameplay scene + UI pass renders into it) and writes the
// swapchain. The sole swapchain writer for the gameplay path — runs every
// frame, never skipped at rest.
//
// Foundation task (M13 Goal SE Task 1): this is an IDENTITY BLIT. A later task
// packs flash/vignette/shake into an effect uniform and applies the math here;
// pre-effects there is no uniform bound and the pass just copies through.
//
// sRGB byte-identity: `scene_color` is the sRGB surface format at single sample,
// and the resolve sampler is NEAREST / pixel-aligned, so each source texel maps
// 1:1 to its swapchain texel with no resample. The per-pass sRGB-encode + 8-bit
// quantize that landed in `scene_color` round-trips losslessly to the swapchain.
// See context/lib/rendering_pipeline.md §7 and the M13 Goal SE plan.

@group(0) @binding(0) var scene_color_tex: texture_2d<f32>;
@group(0) @binding(1) var scene_color_sampler: sampler;

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

@fragment
fn fs_main(in: VsOut) -> @location(0) vec4<f32> {
    // NEAREST sample (pipeline sampler is NEAREST) — 1:1 texel passthrough.
    // Identity blit: no effect uniform yet; a later task applies effect math
    // here against the sampled scene color.
    return textureSample(scene_color_tex, scene_color_sampler, in.uv);
}
