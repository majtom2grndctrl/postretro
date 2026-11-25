// The fluid probe term's per-fragment live-lighting ratio: the sums' oracle,
// composed by the fluid shader alone, since it reads the stored lights'
// descriptors through fluid_reflection_lights.wgsl.
// See: context/lib/rendering_pipeline.md §7.3a (Per-probe sums)

// The per-fragment form the sums replace, kept unreachable from every entry:
// the sums' parity oracle, and the fallback for a reflector the IP rules
// return to per-fragment evaluation (rendering_pipeline.md §7.3a). Lights
// past the stored K have no entry and hold their reference.
fn fluid_reflection_ratio(probe: u32, direction: vec3<f32>) -> vec3<f32> {
    let lobes = fluid_reflection_lobe_weights(direction);
    var ratio = vec3<f32>(1.0);
    for (var i: u32 = 0u; i < FLUID_REFLECTION_LIGHTS_PER_PROBE; i = i + 1u) {
        let light = fluid_probes.probes[probe].lights[i];
        if !fluid_reflection_light_present(light.roster_index) {
            continue;
        }
        let current = fluid_reflection_light_value(light.roster_index);
        ratio = ratio + fluid_reflection_light_share(light.shares, lobes)
            * fluid_reflection_light_change(current, light.reference);
    }
    return max(ratio, vec3<f32>(0.0));
}
