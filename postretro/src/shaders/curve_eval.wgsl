// Uniform Catmull-Rom (tension 0.5) sampling over a closed-loop curve.
// Samples are uniformly spaced over cycle time [0, 1); the curve wraps
// continuously at the cycle boundary.
//
// Binding-agnostic: the consumer shader declares
//     @group(X) @binding(Y) var<storage, read> anim_samples: array<f32>;
// at its chosen (group, binding) before this file is textually
// concatenated. This helper reads `anim_samples` by lexical name and
// must not declare the buffer itself.
//
// Basis matrix: Wikipedia — Cubic Hermite spline § Catmull-Rom spline.

fn sample_curve_catmull_rom(samples_offset: u32, count: u32, cycle_t: f32) -> f32 {
    if (count == 0u) {
        return 1.0;
    }
    if (count == 1u) {
        return anim_samples[samples_offset];
    }

    let scaled = cycle_t * f32(count);
    let i1 = u32(floor(scaled)) % count;
    let i0 = (i1 + count - 1u) % count;
    let i2 = (i1 + 1u) % count;
    let i3 = (i1 + 2u) % count;
    let f = fract(scaled);

    let p0 = anim_samples[samples_offset + i0];
    let p1 = anim_samples[samples_offset + i1];
    let p2 = anim_samples[samples_offset + i2];
    let p3 = anim_samples[samples_offset + i3];

    let a = -0.5 * p0 + 1.5 * p1 - 1.5 * p2 + 0.5 * p3;
    let b =        p0 - 2.5 * p1 + 2.0 * p2 - 0.5 * p3;
    let c = -0.5 * p0              + 0.5 * p2;
    let d =              p1;

    return ((a * f + b) * f + c) * f + d;
}

fn sample_color_catmull_rom(
    samples_offset: u32,
    count: u32,
    cycle_t: f32,
    base_color: vec3<f32>,
) -> vec3<f32> {
    if (count == 0u) {
        return base_color;
    }
    if (count == 1u) {
        return vec3<f32>(
            anim_samples[samples_offset],
            anim_samples[samples_offset + 1u],
            anim_samples[samples_offset + 2u],
        );
    }

    let scaled = cycle_t * f32(count);
    let i1 = u32(floor(scaled)) % count;
    let i0 = (i1 + count - 1u) % count;
    let i2 = (i1 + 1u) % count;
    let i3 = (i1 + 2u) % count;
    let f = fract(scaled);

    let p0 = vec3<f32>(
        anim_samples[samples_offset + i0 * 3u + 0u],
        anim_samples[samples_offset + i0 * 3u + 1u],
        anim_samples[samples_offset + i0 * 3u + 2u],
    );
    let p1 = vec3<f32>(
        anim_samples[samples_offset + i1 * 3u + 0u],
        anim_samples[samples_offset + i1 * 3u + 1u],
        anim_samples[samples_offset + i1 * 3u + 2u],
    );
    let p2 = vec3<f32>(
        anim_samples[samples_offset + i2 * 3u + 0u],
        anim_samples[samples_offset + i2 * 3u + 1u],
        anim_samples[samples_offset + i2 * 3u + 2u],
    );
    let p3 = vec3<f32>(
        anim_samples[samples_offset + i3 * 3u + 0u],
        anim_samples[samples_offset + i3 * 3u + 1u],
        anim_samples[samples_offset + i3 * 3u + 2u],
    );

    let a = -0.5 * p0 + 1.5 * p1 - 1.5 * p2 + 0.5 * p3;
    let b =        p0 - 2.5 * p1 + 2.0 * p2 - 0.5 * p3;
    let c = -0.5 * p0              + 0.5 * p2;
    let d =              p1;

    return ((a * f + b) * f + c) * f + d;
}
