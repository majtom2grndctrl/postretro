// The glossy floor layer's blur at the march's hit: the scene-colour chain
// level a hit reads, and its read. Composed by glossy_floor_march.wgsl beside
// reflection_probe_blur.wgsl, whose exponent -> mip function gives the probe
// term's blur. Mirrors render-cpu `glossy_floor_blur`.
// See: context/lib/rendering_pipeline.md §7.3a (Glossy world faces, Floors)

// IP rules (owner review to change, ip-check.md B1-B4): the level reads the
// texel's exponent, the minimum blur, the hit length and the hit's view depth
// alone, with the camera's focal length and the chain's scale: never s, the
// distance tile, a capture-shadow flag, probe colour or the hit's confidence,
// so it never widens, renormalises or fills where the probe term is withheld.
// The chain holds the opaque scene alone, never the probe term. The read is
// one trilinear sample at the hit, isotropic, weighted by no neighbour's
// exponent or normal.

// --- Group 3 (in part): the level's scene-colour chain, every level, and its
// trilinear clamped sampler.
@group(3) @binding(0) var floor_chain: texture_2d<f32>;
@group(3) @binding(1) var floor_chain_sampler: sampler;

// Colour mips of the engine probe tile's full chain: the probe term's blur is
// read over it, its last mip included.
const GLOSSY_FLOOR_PROBE_MIPS: u32 = 6u;
// The widest cone half-angle a hit's footprint takes, radians: keeps its
// tangent finite and positive at the probe chain's last mip.
const GLOSSY_FLOOR_WIDEST_HALF_ANGLE: f32 = 1.5;
// The nearest hit view depth the footprint divides by, metres.
const GLOSSY_FLOOR_NEAREST_DEPTH: f32 = 0.001;

// The cone half-angle a texel of `exponent` blurs its hit by: the probe
// term's blur at that exponent, its minimum blur included, capped.
fn glossy_floor_blur_half_angle(exponent: f32) -> f32 {
    return min(REFLECTION_PROBE_MIP0_TEXEL_ANGLE
        * exp2(reflection_probe_exponent_mip(exponent, GLOSSY_FLOOR_PROBE_MIPS)),
        GLOSSY_FLOOR_WIDEST_HALF_ANGLE);
}

// The chain level a hit reads, fractional: the footprint of a cone of the
// texel's blur half-angle over `hit_length` metres of the reflected ray,
// projected to the screen at the hit's view depth, so the blur widens with
// hit length as the probe term's angular blur does. `focal` is scene pixels
// per unit of tangent at depth 1, `chain_scale` the chain's level-0 texels
// per scene pixel. A level's texel spans the footprint's full width at half
// maximum, clamped to the chain.
fn glossy_floor_blur_level(exponent: f32, hit_length: f32, hit_depth: f32, focal: f32,
                           chain_scale: f32, levels: u32) -> f32 {
    let half_width = tan(glossy_floor_blur_half_angle(exponent)) * hit_length * focal
        / max(hit_depth, GLOSSY_FLOOR_NEAREST_DEPTH);
    return clamp(log2(2.0 * half_width * chain_scale), 0.0, f32(levels - 1u));
}

// The blurred colour at the hit: one trilinear read of the chain at the hit's
// own screen position `uv` and `level`, clamped to [0, f16 max] so no Inf or
// negative reaches the floor target's bilinear read.
fn glossy_floor_blurred_hit(uv: vec2<f32>, level: f32) -> vec3<f32> {
    return clamp(textureSampleLevel(floor_chain, floor_chain_sampler, uv, level).rgb,
        vec3<f32>(0.0), vec3<f32>(65504.0));
}
