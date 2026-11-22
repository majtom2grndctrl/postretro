// Fluid reflection live lighting: a stored light's live value, change and
// lobe shares, shared by the sums pass (fluid_reflection_sums.wgsl) and the
// fluid shader, which keeps the per-fragment ratio as the sums' oracle.
// Binding-agnostic: the consumer declares `uniforms` (with `time`),
// `anim_descriptors` and `anim_samples` before this text is concatenated.
// See: context/lib/rendering_pipeline.md §7.3a

const FLUID_REFLECTION_REFERENCE_FLOOR: f32 = 1.0e-30;
const FLUID_REFLECTION_UNORM10_MAX: u32 = 1023u;
const FLUID_REFLECTION_NO_LIGHT: u32 = 0xFFFFFFFFu;
const FLUID_REFLECTION_LIGHTS_PER_PROBE: u32 = 4u;
// Share lobes per probe: up, +X, -X, +Z, -Z.
const FLUID_REFLECTION_LOBES: u32 = 5u;

// Live lighting by light share. The capture holds every animated light at its
// reference; each stored light scales it by its share toward the lookup
// direction times its live-over-reference change. Reads no SH and no
// promotion weight, so a switch reaches every reflection on the next frame at
// any distance, and promotion never moves it.

// The share lobes' blend weights for a lookup direction: up, +X, -X, +Z, -Z.
// Mirrors level-format `reflection_probe_lobe_weights`, which the compiler
// split the capture's light by.
fn fluid_reflection_lobe_weights(direction: vec3<f32>) -> array<f32, 5> {
    let positive = max(direction, vec3<f32>(0.0));
    let negative = max(-direction, vec3<f32>(0.0));
    let down = negative.y * negative.y * 0.25;
    let weights = array<f32, 5>(
        positive.y * positive.y,
        positive.x * positive.x + down,
        negative.x * negative.x + down,
        positive.z * positive.z + down,
        negative.z * negative.z + down,
    );
    let sum = weights[0] + weights[1] + weights[2] + weights[3] + weights[4];
    if !(sum > 0.0) {
        return array<f32, 5>(1.0, 0.0, 0.0, 0.0, 0.0);
    }
    return array<f32, 5>(weights[0] / sum, weights[1] / sum, weights[2] / sum, weights[3] / sum,
        weights[4] / sum);
}

// The reserved roster value, and any index past the descriptors, indexes
// nothing: the entry adds no term.
fn fluid_reflection_light_present(roster_index: u32) -> bool {
    return roster_index != FLUID_REFLECTION_NO_LIGHT
        && roster_index < arrayLength(&anim_descriptors);
}

// A stored light's live value: sh_compose.wgsl's `animated_light_scale`, read
// straight from descriptor `roster_index` (identity table). Clamped colour x
// clamped brightness, 0 while inactive; no promotion weight, lighting mask or
// debug override.
fn fluid_reflection_light_value(roster_index: u32) -> vec3<f32> {
    let desc = anim_descriptors[roster_index];
    if (desc.is_active == 0u) {
        return vec3<f32>(0.0);
    }

    let t = animation_curve_t(desc.period, desc.phase, uniforms.time);
    let brightness = max(
        sample_curve_catmull_rom(desc.brightness_offset, desc.brightness_count, t),
        0.0,
    );
    var color = desc.base_color;
    if (desc.color_count > 0u) {
        color = max(
            sample_color_catmull_rom(desc.color_offset, desc.color_count, t, vec3<f32>(1.0)),
            vec3<f32>(0.0),
        ) * desc.base_color;
    }
    return color * brightness;
}

fn fluid_reflection_unpack_unorm10x3(word: u32) -> vec3<f32> {
    let field = FLUID_REFLECTION_UNORM10_MAX;
    return vec3<f32>(f32(word & field), f32((word >> 10u) & field), f32((word >> 20u) & field))
        / f32(field);
}

// A stored light's RGB share toward the lookup: its lobe shares, blended.
fn fluid_reflection_light_share(shares: array<vec4<u32>, 2>, lobes: array<f32, 5>) -> vec3<f32> {
    return fluid_reflection_unpack_unorm10x3(shares[0].x) * lobes[0]
        + fluid_reflection_unpack_unorm10x3(shares[0].y) * lobes[1]
        + fluid_reflection_unpack_unorm10x3(shares[0].z) * lobes[2]
        + fluid_reflection_unpack_unorm10x3(shares[0].w) * lobes[3]
        + fluid_reflection_unpack_unorm10x3(shares[1].x) * lobes[4];
}

// current / reference - 1 per channel; a channel the reference holds no light
// in adds no term.
fn fluid_reflection_light_change(current: vec3<f32>, reference: vec3<f32>) -> vec3<f32> {
    return select(vec3<f32>(0.0),
        current / max(reference, vec3<f32>(FLUID_REFLECTION_REFERENCE_FLOOR)) - vec3<f32>(1.0),
        reference > vec3<f32>(0.0));
}
