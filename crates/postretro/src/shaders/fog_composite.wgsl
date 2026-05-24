// Fullscreen additive composite of the low-res fog scatter buffer over the
// surface. The low-res scatter (quarter-res by default, governed by
// `fog_pixel_scale`) is upsampled to full res with a DEPTH-AWARE (bilateral)
// filter — see `fs_main`. A plain nearest upsample replicates each low-res
// texel into a `pixel_scale × pixel_scale` block, which reads as blocky
// pixelation when the camera is inside a volume; a plain bilinear would smooth
// the blocks but bleed fog across geometry depth edges (haze leaks over object
// silhouettes). The bilateral filter weights each low-res tap by bilinear
// proximity AND depth similarity to the full-res target pixel, so the blocks
// dissolve into a smooth gradient without crossing silhouettes.
// See context/lib/rendering_pipeline.md §7.5.
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
@group(0) @binding(1) var depth_tex: texture_depth_2d;

// Subset of the raymarch `FogParams` (fog_volume.rs::FogParams) — the composite
// reuses the SAME per-frame params uniform buffer (no separate upload). Only
// `near_clip`/`far_clip` are read here, to linearize the non-linear depth buffer
// for a perceptually-meaningful bilateral depth comparison. The leading fields
// are declared so the WGSL struct layout matches the bound buffer; the trailing
// fields after `far_clip` are elided (WGSL does not require declaring the tail
// of a uniform struct).
struct FogParams {
    inv_view_proj: mat4x4<f32>,
    camera_position: vec3<f32>,
    step_size: f32,
    active_count: u32,
    near_clip: f32,
    far_clip: f32,
}
@group(0) @binding(2) var<uniform> fog: FogParams;

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

// Convert a non-linear depth-buffer value (NDC z, [0,1], 0 at near) to a linear
// view-space distance. The raymarch pass uses the same near-at-0 / far-at-1
// convention. Linearizing first makes the bilateral depth weight behave
// consistently across the whole depth range — a fixed NDC threshold would be
// far too tight near the camera and far too loose in the distance, because NDC
// depth crowds nearly all its precision against the near plane.
fn linearize_depth(ndc: f32) -> f32 {
    let n = fog.near_clip;
    let f = fog.far_clip;
    return (n * f) / max(f - ndc * (f - n), 1.0e-6);
}

@fragment
fn fs_main(in: VsOut) -> @location(0) vec4<f32> {
    let scatter_dims = vec2<f32>(textureDimensions(scatter_tex));
    let depth_dims = textureDimensions(depth_tex);

    // Full-res target pixel. `in.clip.xy` is the pixel center in framebuffer
    // space; floor gives the integer texel for the depth load.
    let full_px = vec2<i32>(in.clip.xy);
    let target_depth_ndc = textureLoad(
        depth_tex,
        clamp(full_px, vec2<i32>(0), vec2<i32>(depth_dims) - vec2<i32>(1)),
        0,
    );

    // Locate this pixel in low-res scatter texel space. Scatter texel centers
    // sit at integer+0.5, so subtract 0.5 to get the fractional position for
    // bilinear bracketing: `base` is the lower-left of the 2×2 tap quad,
    // `frac` the interpolation weight within it.
    let scatter_coord = in.uv * scatter_dims - vec2<f32>(0.5);
    let base = floor(scatter_coord);
    let frac = scatter_coord - base;
    let base_i = vec2<i32>(base);
    let scatter_max = vec2<i32>(scatter_dims) - vec2<i32>(1);

    // Sample depth at the EXIT of geometry the raymarch used as `max_t`: the
    // raymarch derives each low-res texel's ray endpoint from a min-over-block
    // depth tap, so the foreground silhouette sits at the depth of the closest
    // covered pixel. For the bilateral comparison we read the full-res depth at
    // each low-res tap's pixel center and weight by similarity to the target
    // pixel's depth — taps on the same surface as the target contribute, taps
    // on a different surface (across a silhouette) are suppressed, so fog does
    // not bleed past the edge.
    let depth_per_scatter = vec2<f32>(depth_dims) / scatter_dims;
    let target_lin = linearize_depth(target_depth_ndc);

    // Depth-similarity falloff scale. Relative to the target's own linear
    // distance (sigma = 5% of view depth): an absolute world-space sigma would
    // over-reject distant surfaces (where one depth texel spans many world
    // units) and under-reject near ones. A relative scale tracks the depth
    // buffer's own precision distribution, so the edge stays equally crisp at
    // any range. The `+near_clip` floor keeps the denominator sane right at the
    // camera where `target_lin` approaches zero.
    let sigma = 0.05 * target_lin + fog.near_clip;

    var rgb_accum = vec3<f32>(0.0);
    var weight_accum = 0.0;
    for (var j: i32 = 0; j < 2; j = j + 1) {
        for (var i: i32 = 0; i < 2; i = i + 1) {
            let tap = clamp(base_i + vec2<i32>(i, j), vec2<i32>(0), scatter_max);

            // Bilinear weight for this corner of the 2×2 quad.
            let wx = mix(1.0 - frac.x, frac.x, f32(i));
            let wy = mix(1.0 - frac.y, frac.y, f32(j));
            let w_bilinear = wx * wy;

            // Depth at this tap's full-res pixel center, linearized and compared
            // to the target. exp(-Δ/σ) is the standard bilateral kernel: taps
            // straddling a silhouette have a large Δ and get ~zero weight.
            let tap_depth_px = vec2<i32>(
                (vec2<f32>(tap) + vec2<f32>(0.5)) * depth_per_scatter,
            );
            let tap_depth_ndc = textureLoad(
                depth_tex,
                clamp(tap_depth_px, vec2<i32>(0), vec2<i32>(depth_dims) - vec2<i32>(1)),
                0,
            );
            let tap_lin = linearize_depth(tap_depth_ndc);
            let w_depth = exp(-abs(tap_lin - target_lin) / sigma);

            let w = w_bilinear * w_depth;
            rgb_accum = rgb_accum + textureLoad(scatter_tex, tap, 0).rgb * w;
            weight_accum = weight_accum + w;
        }
    }
    // weight_accum is always > 0: the bilinear weights sum to 1 and the nearest
    // tap (smallest Δdepth) carries a non-zero depth weight, so no fallback is
    // needed. Divide to renormalize the bilateral kernel.
    let scatter = rgb_accum / weight_accum;

    // One 8-bit LSB in the encoded (sRGB) domain.
    let lsb = 1.0 / 255.0;

    // Triangular-PDF (TPDF) dither, the textbook fix for output-quantization
    // banding: a triangle on [-1, 1] LSB makes the quantization error's first
    // two moments signal-independent (clean band removal) where a single uniform
    // (RPDF) leaves residual low-frequency structure. The triangle is derived
    // analytically from ONE Interleaved Gradient Noise sample, NOT by
    // differencing two. Differencing two IGN samples at a fixed pixel offset
    // does not decorrelate them — both carry IGN's single diagonal frequency
    // through the same nonlinear `fract`, so their difference beats into a
    // low-frequency diagonal stripe that surfaces against dark, smoothly-lit fog.
    // The single-sample remap keeps IGN's high-frequency distribution with no
    // self-beat. `in.clip.xy` is the pixel center in framebuffer space.
    let u = interleaved_gradient_noise(in.clip.xy);
    let t = 2.0 * u;
    let tri = select(1.0 - sqrt(2.0 - t), sqrt(t) - 1.0, t < 1.0);
    let tpdf = tri * lsb;

    // Dither in encoded space, where the GPU's 8-bit quantization lives, then
    // decode back to linear so the hardware sRGB-encode-on-store lands the
    // dithered value across the quantization boundary.
    let encoded = linear_to_srgb(scatter) + vec3<f32>(tpdf);
    let dithered = srgb_to_linear(encoded);

    return vec4<f32>(dithered, 1.0);
}
