// Fluid reflection live-lighting sums: once per frame with a reflecting fluid
// draw, every installed probe's per-lobe sum of share x (current / reference
// - 1) over its stored lights, which fs_main_reflecting blends by its lobe
// weights. Concatenated with fluid_reflection_lights.wgsl and curve_eval.wgsl.
// IP rules (owner review to change): the sums live in their own buffer and
// never write a probe tile or the share table; every installed probe is
// evaluated every dispatching frame; the coefficient stays current / ref - 1
// per light.
// See: context/lib/rendering_pipeline.md §7.3a

// --- Group 0: camera uniforms, the fluid pass's group 0. Only `time` is read.
struct Uniforms {
    view_proj: mat4x4<f32>,
    camera_position: vec3<f32>,
    ambient_floor: f32,
    light_count: u32,
    time: f32,
    light_term_mask: u32,
    indirect_scale: f32,
    sdf_shadow_flags: u32,
    sdf_shadow_mode: u32,
    sdf_force_visibility_one: u32,
    direct_scale: f32,
    has_scatter: u32,
    has_direct: u32,
    total_light_count: u32,
    _spec_shadowmask_flags_inert: u32,
};
@group(0) @binding(0) var<uniform> uniforms: Uniforms;

// --- Group 1: the level's SH group, the fluid pass's group 3. Only the
// roster-indexed compose descriptors and their curve samples are read.
struct AnimationDescriptor {
    period: f32,
    phase: f32,
    brightness_offset: u32,
    brightness_count: u32,
    base_color: vec3<f32>,
    color_offset: u32,
    color_count: u32,
    is_active: u32,
    direction_offset: u32,
    direction_count: u32,
};
@group(1) @binding(11) var<storage, read> anim_descriptors: array<AnimationDescriptor>;
@group(1) @binding(12) var<storage, read> anim_samples: array<f32>;

// --- Group 2: the level's sums and share table.
struct FluidProbeLight {
    reference: vec3<f32>,
    roster_index: u32,
    shares: array<vec4<u32>, 2>,
};
struct FluidProbe {
    box_min: vec3<f32>,
    _pad0: u32,
    box_max: vec3<f32>,
    _pad1: u32,
    capture_point: vec3<f32>,
    layer: u32,
    lights: array<FluidProbeLight, 4>,
};
struct FluidProbeTable {
    tile: vec4<u32>,
    probes: array<FluidProbe, 256>,
};
struct FluidProbeSums {
    lobes: array<vec4<f32>, 1536>,
};
// The one writable binding: this pass writes the sums and nothing else.
@group(2) @binding(0) var<storage, read_write> fluid_probe_sums: FluidProbeSums;
@group(2) @binding(1) var<uniform> fluid_probes: FluidProbeTable;

// The largest change one light adds to the sums: f16's largest finite value,
// the capture's and the target's format. A live value overflowing its
// reference to +Inf would turn a zero share, then a zero lobe weight, into
// NaN and blank the whole probe. Mirrors render-cpu
// `FLUID_REFLECTION_CHANGE_CEILING`.
const FLUID_REFLECTION_CHANGE_CEILING: f32 = 65504.0;

// A probe's per-lobe sums over its stored lights, each light's change capped
// at the ceiling. Mirrors render-cpu `reflection_probe_sums`.
fn fluid_reflection_probe_sums(probe: u32) -> array<vec3<f32>, 6> {
    var sums = array<vec3<f32>, 6>();
    for (var i: u32 = 0u; i < FLUID_REFLECTION_LIGHTS_PER_PROBE; i = i + 1u) {
        let light = fluid_probes.probes[probe].lights[i];
        if !fluid_reflection_light_present(light.roster_index) {
            continue;
        }
        let change = min(
            fluid_reflection_light_change(
                fluid_reflection_light_value(light.roster_index), light.reference),
            vec3<f32>(FLUID_REFLECTION_CHANGE_CEILING));
        sums[0] = sums[0] + fluid_reflection_unpack_unorm10x3(light.shares[0].x) * change;
        sums[1] = sums[1] + fluid_reflection_unpack_unorm10x3(light.shares[0].y) * change;
        sums[2] = sums[2] + fluid_reflection_unpack_unorm10x3(light.shares[0].z) * change;
        sums[3] = sums[3] + fluid_reflection_unpack_unorm10x3(light.shares[0].w) * change;
        sums[4] = sums[4] + fluid_reflection_unpack_unorm10x3(light.shares[1].x) * change;
        sums[5] = sums[5] + fluid_reflection_unpack_unorm10x3(light.shares[1].y) * change;
    }
    return sums;
}

// One invocation per installed probe (the table's probe count); invocations
// past it write nothing.
@compute @workgroup_size(64)
fn fluid_reflection_sums_main(@builtin(global_invocation_id) id: vec3<u32>) {
    let probe = id.x;
    if probe >= fluid_probes.tile.z {
        return;
    }
    let sums = fluid_reflection_probe_sums(probe);
    let base = probe * FLUID_REFLECTION_LOBES;
    fluid_probe_sums.lobes[base] = vec4<f32>(sums[0], 0.0);
    fluid_probe_sums.lobes[base + 1u] = vec4<f32>(sums[1], 0.0);
    fluid_probe_sums.lobes[base + 2u] = vec4<f32>(sums[2], 0.0);
    fluid_probe_sums.lobes[base + 3u] = vec4<f32>(sums[3], 0.0);
    fluid_probe_sums.lobes[base + 4u] = vec4<f32>(sums[4], 0.0);
    fluid_probe_sums.lobes[base + 5u] = vec4<f32>(sums[5], 0.0);
}
