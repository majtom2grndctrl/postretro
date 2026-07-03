// Fullscreen additive composite of the low-res fog scatter buffer over the
// surface. Nearest-neighbor upscale — the pixelated blocks are aesthetic
// intent, not a compromise. See context/lib/rendering_pipeline.md §7.5.
//
// Banding note: the scatter target is RGBA16Float (no quantization there), but
// the surface is an 8-bit *UnormSrgb* swapchain. This pass additively blends
// the scatter into that 8-bit target (blend = src One + dst One), so the smooth
// radial scatter gradient is quantized to 256 levels per channel at write time
// — exactly where the concentric "Mach band" rings come from (worst at high
// scatter_bias, where the broad directional SH ramp is smoothest). The cure is
// to dither the value at the point of quantization: add a sub-LSB, per-pixel
// triangular-PDF offset (Interleaved Gradient Noise base) so the hard
// quantization steps dissolve into imperceptible noise instead of rings. This
// is the textbook fix for output-quantization banding and is essentially free.

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

// Interleaved Gradient Noise (Jimenez, "Next Generation Post Processing in
// Call of Duty: Advanced Warfare"). A cheap, well-distributed screen-space
// hash in [0, 1) keyed on integer pixel coordinates — the standard base for
// ordered dithering. Stable per-pixel (no temporal animation) so the dither
// reads as fixed film grain, not shimmer, which suits the static pixelated
// fog aesthetic.
fn interleaved_gradient_noise(pixel: vec2<f32>) -> f32 {
    return fract(52.9829189 * fract(dot(pixel, vec2<f32>(0.06711056, 0.00583715))));
}

// sRGB transfer functions. The swapchain is `*UnormSrgb`: the hardware applies
// the linear→sRGB encode on write, then quantizes to 8 bits. To size the dither
// at exactly one 8-bit LSB *in the domain where quantization happens*, we move
// into encoded (sRGB) space, dither there, and decode back to linear — the
// hardware re-encodes on store. A fixed linear-domain offset would under-dither
// bright values and over-dither dark ones; doing it in encoded space keeps the
// amplitude ~1 LSB everywhere.
fn linear_to_srgb(c: vec3<f32>) -> vec3<f32> {
    let lo = c * 12.92;
    let hi = 1.055 * pow(max(c, vec3<f32>(0.0)), vec3<f32>(1.0 / 2.4)) - 0.055;
    return select(hi, lo, c <= vec3<f32>(0.0031308));
}

fn srgb_to_linear(c: vec3<f32>) -> vec3<f32> {
    let lo = c / 12.92;
    let hi = pow((c + 0.055) / 1.055, vec3<f32>(2.4));
    return select(hi, lo, c <= vec3<f32>(0.04045));
}

@fragment
fn fs_main(in: VsOut) -> @location(0) vec4<f32> {
    // Nearest sample — the pipeline's sampler is already NEAREST but
    // `textureSample` here honors whatever is bound.
    let scatter = textureSample(scatter_tex, scatter_sampler, in.uv).rgb;

    // One 8-bit LSB in the encoded (sRGB) domain.
    let lsb = 1.0 / 255.0;

    // Triangular-PDF (TPDF) dither: two independent uniform noise samples
    // differenced give a triangular distribution in [-1, 1], which fully
    // decorrelates the quantization error from the signal (clean band removal)
    // where a single uniform sample (RPDF) leaves residual low-frequency
    // structure. Offset the second sample's pixel coords so the two hashes are
    // independent. `in.clip.xy` is the pixel center in framebuffer space.
    let n0 = interleaved_gradient_noise(in.clip.xy);
    let n1 = interleaved_gradient_noise(in.clip.xy + vec2<f32>(113.0, 71.0));
    let tpdf = (n0 - n1) * lsb;

    // Dither in encoded space, where the GPU's 8-bit quantization lives, then
    // decode back to linear so the hardware sRGB-encode-on-store lands the
    // dithered value across the quantization boundary.
    let encoded = linear_to_srgb(scatter) + vec3<f32>(tpdf);
    let dithered = srgb_to_linear(encoded);

    return vec4<f32>(dithered, 1.0);
}
