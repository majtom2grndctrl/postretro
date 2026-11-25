// Reflection probe colour mips: the exponent -> mip function. Binding-free,
// composed by the glossy overlay. Mirrors level-format
// `reflection_probes::blur`, which the compiler's prefilter inverts per mip;
// a source scan pins every line here to it. Water never reads it: water
// samples colour mip 0.
// See: context/lib/rendering_pipeline.md §7.3a (Glossy world faces)

// alpha_0: the mip-0 interior texel's angle, pi / 64 radians (180 degrees over
// the 64 texels pole to antipode of the engine tile). Mip m's texel is
// alpha_0 x 2^m.
const REFLECTION_PROBE_MIP0_TEXEL_ANGLE: f32 = 0.049087387;
// m_min: no glossy world face reads sharper than this mip, so none is a
// mirror.
const REFLECTION_PROBE_MIN_BLUR_MIP: f32 = 1.0;

// The reflected-direction half-width at half maximum of a Blinn-Phong lobe of
// `exponent`, in radians: 2 x acos(0.5^(1/n)).
fn reflection_lobe_half_width(exponent: f32) -> f32 {
    return 2.0 * acos(pow(0.5, 1.0 / exponent));
}

// The colour mip a texel of `exponent` reads, fractional between two mips:
// log2(half-width / alpha_0), at least the minimum blur and at most the last
// of `colour_mip_count` mips, the last winning on a shorter chain.
fn reflection_probe_exponent_mip(exponent: f32, colour_mip_count: u32) -> f32 {
    let last = f32(colour_mip_count - 1u);
    let mip = log2(reflection_lobe_half_width(exponent) / REFLECTION_PROBE_MIP0_TEXEL_ANGLE);
    return min(max(mip, REFLECTION_PROBE_MIN_BLUR_MIP), last);
}
