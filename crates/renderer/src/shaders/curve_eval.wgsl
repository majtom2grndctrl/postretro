// Shared Catmull-Rom curve evaluation helpers for WGSL shaders (binding-agnostic).
// See: context/plans/in-progress/animated-curve-eval/index.md

// Uniform Catmull-Rom (tension 0.5) sampling. Looping descriptors use a
// closed curve over [0, 1). Finite descriptors carry a negative period;
// `animation_curve_t` returns an encoded endpoint-clamped position so the
// final keyframe is reached before CPU-side settlement.
//
// Binding-agnostic: the consumer shader declares
//     @group(X) @binding(Y) var<storage, read> anim_samples: array<f32>;
// at its chosen (group, binding) before this file is textually
// concatenated. This helper reads `anim_samples` by lexical name and
// must not declare the buffer itself.
//
// Basis matrix: Wikipedia — Cubic Hermite spline § Catmull-Rom spline.

fn animation_curve_t(period: f32, phase: f32, time: f32) -> f32 {
    if (period < 0.0) {
        let open_t = clamp(time / max(-period, 1.0e-6) + phase, 0.0, 1.0);
        // Closed positions are non-negative. Encode open positions below -1
        // so existing curve call sites need no extra mode argument.
        return -1.0 - open_t;
    }
    return fract(time / max(period, 1.0e-6) + phase);
}

fn sample_curve_catmull_rom(samples_offset: u32, count: u32, cycle_t: f32) -> f32 {
    if (count == 0u) {
        return 1.0;
    }
    if (count == 1u) {
        return anim_samples[samples_offset];
    }

    let is_open = cycle_t <= -1.0;
    let t = select(cycle_t, clamp(-cycle_t - 1.0, 0.0, 1.0), is_open);
    var scaled = t * f32(count);
    var i1 = u32(floor(scaled)) % count;
    var i0 = (i1 + count - 1u) % count;
    var i2 = (i1 + 1u) % count;
    var i3 = (i1 + 2u) % count;
    if (is_open) {
        let last = count - 1u;
        scaled = t * f32(last);
        i1 = min(u32(floor(scaled)), last);
        i0 = 0u;
        if (i1 > 0u) {
            i0 = i1 - 1u;
        }
        i2 = min(i1 + 1u, last);
        i3 = min(i1 + 2u, last);
    }
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

    let is_open = cycle_t <= -1.0;
    let t = select(cycle_t, clamp(-cycle_t - 1.0, 0.0, 1.0), is_open);
    var scaled = t * f32(count);
    var i1 = u32(floor(scaled)) % count;
    var i0 = (i1 + count - 1u) % count;
    var i2 = (i1 + 1u) % count;
    var i3 = (i1 + 2u) % count;
    if (is_open) {
        let last = count - 1u;
        scaled = t * f32(last);
        i1 = min(u32(floor(scaled)), last);
        i0 = 0u;
        if (i1 > 0u) {
            i0 = i1 - 1u;
        }
        i2 = min(i1 + 1u, last);
        i3 = min(i1 + 2u, last);
    }
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
