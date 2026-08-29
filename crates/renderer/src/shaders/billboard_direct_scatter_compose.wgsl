// Dense section-48 direct-scatter compose. `curve_eval.wgsl` is concatenated
// after this source by the renderer.

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
    spec_shadowmask_force_one: u32,
};

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

struct ScatterGrid {
    grid_dimensions: vec3<u32>,
    _pad0: u32,
    affinity_dimensions: vec3<u32>,
    _pad1: u32,
};

@group(0) @binding(0) var<uniform> uniforms: Uniforms;
@group(1) @binding(0) var base_scatter: texture_3d<f32>;
@group(1) @binding(1) var composed_scatter: texture_storage_3d<rgba16float, write>;
@group(1) @binding(2) var<uniform> grid: ScatterGrid;
// Two f16 values per u32. Every CSR entry contains 64 RGBA16F samples.
@group(1) @binding(3) var<storage, read> delta_rgba: array<u32>;
@group(1) @binding(4) var<storage, read> affinity_offsets: array<u32>;
@group(1) @binding(5) var<storage, read> descriptors: array<AnimationDescriptor>;
@group(1) @binding(6) var<storage, read> anim_samples: array<f32>;
@group(1) @binding(7) var<storage, read> affinity_lights: array<u32>;
@group(1) @binding(8) var<storage, read> animation_descriptor_indices: array<u32>;

const INVALID_DESCRIPTOR_INDEX: u32 = 0xffffffffu;
const LIGHT_TERM_BAKED_DIRECT_STATIC: u32 = 0x08u;
const LIGHT_TERM_BAKED_DIRECT_ANIMATED: u32 = 0x10u;
const AFFINITY_FACTOR: u32 = 4u;
const SAMPLES_PER_ENTRY: u32 = 64u;
const F16_PER_SAMPLE: u32 = 4u;

fn animated_light_scale(light_index: u32) -> vec3<f32> {
    if ((uniforms.light_term_mask & LIGHT_TERM_BAKED_DIRECT_ANIMATED) == 0u) {
        return vec3<f32>(0.0);
    }
    let descriptor_index = animation_descriptor_indices[light_index];
    if (descriptor_index == INVALID_DESCRIPTOR_INDEX || descriptor_index >= arrayLength(&descriptors)) {
        return vec3<f32>(0.0);
    }
    let desc = descriptors[descriptor_index];
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

fn read_delta(entry: u32, local_probe: u32) -> vec3<f32> {
    let half_offset = (entry * SAMPLES_PER_ENTRY + local_probe) * F16_PER_SAMPLE;
    let word_offset = half_offset / 2u;
    let rg = unpack2x16float(delta_rgba[word_offset]);
    let ba = unpack2x16float(delta_rgba[word_offset + 1u]);
    return vec3<f32>(rg.x, rg.y, ba.x);
}

@compute @workgroup_size(4, 4, 4)
fn compose_main(
    @builtin(workgroup_id) affinity: vec3<u32>,
    @builtin(local_invocation_id) local_id: vec3<u32>,
) {
    let probe = affinity * AFFINITY_FACTOR + local_id;
    if (any(probe >= grid.grid_dimensions)) {
        return;
    }
    let cell_index = affinity.x
        + affinity.y * grid.affinity_dimensions.x
        + affinity.z * grid.affinity_dimensions.x * grid.affinity_dimensions.y;
    let local_probe = local_id.x + local_id.y * AFFINITY_FACTOR
        + local_id.z * AFFINITY_FACTOR * AFFINITY_FACTOR;
    var accum = textureLoad(base_scatter, vec3<i32>(probe), 0);
    if ((uniforms.light_term_mask & LIGHT_TERM_BAKED_DIRECT_STATIC) == 0u) {
        accum = vec4<f32>(vec3<f32>(0.0), accum.a);
    }
    let start = affinity_offsets[cell_index];
    let end = affinity_offsets[cell_index + 1u];
    for (var entry = start; entry < end; entry = entry + 1u) {
        accum = vec4<f32>(
            accum.rgb + read_delta(entry, local_probe) * animated_light_scale(affinity_lights[entry]),
            accum.a,
        );
    }
    textureStore(composed_scatter, vec3<i32>(probe), accum);
}
