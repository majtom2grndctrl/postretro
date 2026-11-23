// Reflection probe share lobes: the six lobes' order and the blend weights
// of a lookup direction. Binding-free, so the fluid shader, the sums pass and
// the glossy overlay compose one definition.
// See: context/lib/rendering_pipeline.md §7.3a (Live lighting)

// Share lobes per probe: +Y, +X, -X, +Z, -Z, -Y.
const FLUID_REFLECTION_LOBES: u32 = 6u;

// The share lobes' blend weights for a lookup direction: +Y, +X, -X, +Z, -Z,
// -Y. Mirrors level-format `reflection_probe_lobe_weights`, which the
// compiler split the capture's light by.
fn fluid_reflection_lobe_weights(direction: vec3<f32>) -> array<f32, 6> {
    let positive = max(direction, vec3<f32>(0.0));
    let negative = max(-direction, vec3<f32>(0.0));
    let weights = array<f32, 6>(
        positive.y * positive.y,
        positive.x * positive.x,
        negative.x * negative.x,
        positive.z * positive.z,
        negative.z * negative.z,
        negative.y * negative.y,
    );
    let sum = weights[0] + weights[1] + weights[2] + weights[3] + weights[4] + weights[5];
    if !(sum > 0.0) {
        return array<f32, 6>(1.0, 0.0, 0.0, 0.0, 0.0, 0.0);
    }
    return array<f32, 6>(weights[0] / sum, weights[1] / sum, weights[2] / sum, weights[3] / sum,
        weights[4] / sum, weights[5] / sum);
}
