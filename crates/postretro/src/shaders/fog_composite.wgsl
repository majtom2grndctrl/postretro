// Fullscreen additive composite of the low-res fog scatter buffer over the
// surface. Nearest-neighbor upscale — the pixelated blocks are aesthetic
// intent, not a compromise. See context/lib/rendering_pipeline.md §7.5.

@group(0) @binding(0) var scatter_tex: texture_2d<f32>;
@group(0) @binding(1) var scatter_sampler: sampler;

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
    // Nearest sample — the pipeline's sampler is already NEAREST but
    // `textureSample` here honors whatever is bound.
    let scatter = textureSample(scatter_tex, scatter_sampler, in.uv).rgb;
    return vec4<f32>(scatter, 1.0);
}
