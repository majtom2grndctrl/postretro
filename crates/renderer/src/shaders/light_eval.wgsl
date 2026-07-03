// Shared dynamic-light per-fragment evaluation helpers (binding-agnostic).
// See: context/lib/rendering_pipeline.md §4, §8
//
// The runtime dynamic-tier light loop in forward.wgsl evaluates these per
// fragment; skinned_mesh.wgsl mirrors the same loop against its own group-2
// bindings. Helpers take light/descriptor values as parameters and declare no
// buffers, so each consumer binds the underlying storage at its own
// (group, binding) before this file is textually concatenated.
//
// Append order: `light_eval_animated_direction` reads `AnimationDescriptor`
// (declared by each consumer) and calls `sample_color_catmull_rom` from
// `curve_eval.wgsl`. WGSL resolves module-scope names regardless of textual
// order, so this snippet may be appended before or after curve_eval, but the
// consumer MUST append curve_eval too (forward already does).
//
// Names are prefixed `light_eval_` to avoid colliding with billboard.wgsl's
// own same-shaped `falloff` / `cone_attenuation` copies if this snippet is
// later appended to the billboard pipeline.

fn light_eval_falloff(distance: f32, range: f32, model: u32) -> f32 {
    let r = max(range, 0.001);
    switch model {
        case 0u: {
            return max(1.0 - distance / r, 0.0);
        }
        case 1u: {
            // Linear window drives inverse-distance smoothly to 0 at range.
            return (1.0 / max(distance, 0.001)) * max(1.0 - distance / r, 0.0);
        }
        case 2u: {
            let d2 = max(distance * distance, 0.001);
            return (1.0 / d2) * max(1.0 - distance / r, 0.0);
        }
        default: {
            return 0.0;
        }
    }
}

fn light_eval_cone_attenuation(L: vec3<f32>, aim: vec3<f32>, inner_angle: f32, outer_angle: f32) -> f32 {
    let cos_angle = dot(-L, aim);
    let cos_inner = cos(inner_angle);
    let cos_outer = cos(outer_angle);
    return smoothstep(cos_outer, cos_inner, cos_angle);
}

// Sample the direction channel of an AnimationDescriptor at `cycle_t` and fall
// back to `static_aim` when the descriptor carries no direction samples.
// Samples are normalized at write time; Catmull-Rom between unit vectors drifts
// only slightly off the sphere at typical authored sample rates.
fn light_eval_animated_direction(desc: AnimationDescriptor, cycle_t: f32, static_aim: vec3<f32>) -> vec3<f32> {
    if desc.direction_count == 0u {
        return static_aim;
    }
    let zero_base = vec3<f32>(0.0, 0.0, 0.0);
    return sample_color_catmull_rom(desc.direction_offset, desc.direction_count, cycle_t, zero_base);
}

fn light_eval_scripted_intensity_scalar(premultiplied_color: vec3<f32>, base_color: vec3<f32>) -> f32 {
    var color_channel = base_color.z;
    var premultiplied_channel = premultiplied_color.z;
    if base_color.x >= base_color.y && base_color.x >= base_color.z {
        color_channel = base_color.x;
        premultiplied_channel = premultiplied_color.x;
    } else if base_color.y >= base_color.z {
        color_channel = base_color.y;
        premultiplied_channel = premultiplied_color.y;
    }
    if color_channel <= 1.0e-6 {
        return 0.0;
    }
    return premultiplied_channel / color_channel;
}
