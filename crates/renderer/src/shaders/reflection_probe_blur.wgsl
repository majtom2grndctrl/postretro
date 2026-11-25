// Reflection probe colour mips: the exponent -> mip function, composed by the glossy overlay and floor march; mirrors level-format `reflection_probes::blur`.
// See: context/lib/rendering_pipeline.md §7.3a (Glossy world faces, Blur)

// Binding-free: the section's tile dimension and colour mip count arrive as
// arguments, so a section captured at another size reads its own mips. The
// compiler's prefilter inverts the same function per mip; a source scan pins
// every line here to it. Water never reads it: water samples colour mip 0.

// pi, as f32: alpha_0's numerator, level-format's `std::f32::consts::PI`.
const REFLECTION_PROBE_PI: f32 = 3.1415927;
// m_min: no glossy world face reads sharper than this mip, so none is a
// mirror.
const REFLECTION_PROBE_MIN_BLUR_MIP: f32 = 1.0;

// The reflected-direction half-width at half maximum of a Blinn-Phong lobe of
// `exponent`, in radians: 2 x acos(0.5^(1/n)).
fn reflection_lobe_half_width(exponent: f32) -> f32 {
    return 2.0 * acos(pow(0.5, 1.0 / exponent));
}

// alpha_0, the mip-0 interior texel's angle of a `tile_dimension` tile
// (border included), radians: 180 degrees over the tile_dimension / 2 texels
// pole to antipode, pi / 64 at the engine's 128^2. Mip m's texel is
// alpha_0 x 2^m.
fn reflection_probe_mip0_texel_angle(tile_dimension: u32) -> f32 {
    return REFLECTION_PROBE_PI / f32(tile_dimension / 2u);
}

// The colour mip a texel of `exponent` reads, fractional between two mips:
// log2(half-width / alpha_0), alpha_0 the section's `mip0_texel_angle`, at
// least the minimum blur and at most the last of `colour_mip_count` mips, the
// last winning on a shorter chain.
fn reflection_probe_exponent_mip(exponent: f32, mip0_texel_angle: f32,
                                 colour_mip_count: u32) -> f32 {
    let last = f32(colour_mip_count - 1u);
    let mip = log2(reflection_lobe_half_width(exponent) / mip0_texel_angle);
    return min(max(mip, REFLECTION_PROBE_MIN_BLUR_MIP), last);
}
