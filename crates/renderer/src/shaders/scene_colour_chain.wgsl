// The glossy floor layer's scene-colour mip chain: each level an isotropic
// box downsample of the one above, level 0 of the highlight-free scene
// colour. Its source holds the opaque scene alone: no highlight, no overlay
// output and so no probe term (ip-check B2). Every tap weighs the same and
// sits at a fixed offset from the destination texel's centre: no weight or
// offset reads a neighbour's exponent, normal or any probe quantity (B4).
// See: context/lib/rendering_pipeline.md §7.3a (Glossy world faces, Floors)

@group(0) @binding(0) var chain_source: texture_2d<f32>;
@group(0) @binding(1) var chain_sampler: sampler;

struct ChainVertex {
    @builtin(position) position: vec4<f32>,
    @location(0) uv: vec2<f32>,
};

// One triangle covering the destination level; uv spans [0, 1] over it.
@vertex
fn vs_chain(@builtin(vertex_index) vertex: u32) -> ChainVertex {
    let uv = vec2<f32>(f32((vertex << 1u) & 2u), f32(vertex & 2u));
    return ChainVertex(vec4<f32>(uv.x * 2.0 - 1.0, 1.0 - uv.y * 2.0, 0.0, 1.0), uv);
}

// The mean of four bilinear taps a quarter of a destination texel either
// side of its centre on each axis. At two source texels per destination
// texel each tap lands on one source texel's centre, and at four each lands
// between four: either way the destination texel is the plain mean of the
// source texels it covers.
@fragment
fn fs_chain_downsample(in: ChainVertex) -> @location(0) vec4<f32> {
    let quarter = 0.25 * vec2<f32>(dpdx(in.uv).x, dpdy(in.uv).y);
    let sum = textureSampleLevel(chain_source, chain_sampler, in.uv + vec2<f32>(-quarter.x, -quarter.y), 0.0)
        + textureSampleLevel(chain_source, chain_sampler, in.uv + vec2<f32>(quarter.x, -quarter.y), 0.0)
        + textureSampleLevel(chain_source, chain_sampler, in.uv + vec2<f32>(-quarter.x, quarter.y), 0.0)
        + textureSampleLevel(chain_source, chain_sampler, in.uv + vec2<f32>(quarter.x, quarter.y), 0.0);
    return vec4<f32>(sum.rgb * 0.25, 1.0);
}
