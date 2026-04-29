// SH compose compute pass.
//
// Per-frame compose of the static base SH irradiance volume plus per-light
// animated deltas into a parallel set of "total" SH band textures. SH
// consumers (forward, billboard, fog) sample the total textures, so any
// animated-delta data this pass writes is automatically picked up without
// consumer-side branching.
//
// Algorithm: for each output probe, load the 9 base SH bands, then iterate
// the animated lights. For each light, project the probe's world-space
// position into the light's delta-grid local coordinates, trilinearly
// sample the 9 delta bands, evaluate the Catmull-Rom animation curve, and
// accumulate `delta × brightness × color` into the running total.
//
// When `delta_light_count == 0` (no animated lights) the loop is skipped
// and the result is identical to a base→total copy.
//
// Curve helpers (`sample_curve_catmull_rom`, `sample_color_catmull_rom`)
// come from `curve_eval.wgsl`, concatenated after this source at
// pipeline-build time. Both helpers read `anim_samples` by lexical name.

struct Uniforms {
    view_proj: mat4x4<f32>,
    camera_position: vec3<f32>,
    ambient_floor: f32,
    light_count: u32,
    time: f32,
    lighting_isolation: u32,
    _pad: u32,
};

// Same 48-byte layout as `forward.wgsl` and `animated_lightmap_compose.wgsl`.
// The buffer is shared via `AnimatedLightBuffers` so the field order must
// match exactly.
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

// Per-light delta SH grid light_metadata. Must match the std430 layout produced
// by `sh_compose.rs::build_delta_buffers` (48-byte stride):
//   0..12  aabb_origin       vec3<f32>
//   12..16 cell_size         f32
//   16..28 grid_dimensions   vec3<u32>
//   28..32 probe_offset      u32
//   32..36 descriptor_index  u32
//   36..48 padding
struct DeltaLightMeta {
    aabb_origin: vec3<f32>,
    cell_size: f32,
    grid_dimensions: vec3<u32>,
    probe_offset: u32,
    descriptor_index: u32,
    _pad0: u32,
    _pad1: u32,
    _pad2: u32,
};

struct GridDims {
    dims: vec3<u32>,
    delta_light_count: u32,
};

struct GridFrame {
    grid_origin: vec3<f32>,
    _pad0: f32,
    cell_size: vec3<f32>,
    _pad1: f32,
};

@group(0) @binding(0) var<uniform> uniforms: Uniforms;

@group(1) @binding(0) var sh_base_band0: texture_3d<f32>;
@group(1) @binding(1) var sh_base_band1: texture_3d<f32>;
@group(1) @binding(2) var sh_base_band2: texture_3d<f32>;
@group(1) @binding(3) var sh_base_band3: texture_3d<f32>;
@group(1) @binding(4) var sh_base_band4: texture_3d<f32>;
@group(1) @binding(5) var sh_base_band5: texture_3d<f32>;
@group(1) @binding(6) var sh_base_band6: texture_3d<f32>;
@group(1) @binding(7) var sh_base_band7: texture_3d<f32>;
@group(1) @binding(8) var sh_base_band8: texture_3d<f32>;

@group(1) @binding(9)  var sh_total_band0: texture_storage_3d<rgba16float, write>;
@group(1) @binding(10) var sh_total_band1: texture_storage_3d<rgba16float, write>;
@group(1) @binding(11) var sh_total_band2: texture_storage_3d<rgba16float, write>;
@group(1) @binding(12) var sh_total_band3: texture_storage_3d<rgba16float, write>;
@group(1) @binding(13) var sh_total_band4: texture_storage_3d<rgba16float, write>;
@group(1) @binding(14) var sh_total_band5: texture_storage_3d<rgba16float, write>;
@group(1) @binding(15) var sh_total_band6: texture_storage_3d<rgba16float, write>;
@group(1) @binding(16) var sh_total_band7: texture_storage_3d<rgba16float, write>;
@group(1) @binding(17) var sh_total_band8: texture_storage_3d<rgba16float, write>;

@group(1) @binding(18) var<uniform> grid: GridDims;
@group(1) @binding(19) var<uniform> grid_frame: GridFrame;
@group(1) @binding(20) var<storage, read> delta_lights: array<DeltaLightMeta>;
@group(1) @binding(21) var<storage, read> delta_probes: array<f32>;
@group(1) @binding(22) var<storage, read> descriptors: array<AnimationDescriptor>;
@group(1) @binding(23) var<storage, read> anim_samples: array<f32>;

// Number of f32 slots per probe in `delta_probes`: 9 SH bands × RGB.
const PROBE_F32_COUNT: u32 = 27u;

// Read one probe's 27 SH coefficients from the flat `delta_probes` buffer.
// `probe_index` is in probe units; `probe_offset` is in f32 units.
fn read_delta_probe(probe_offset: u32, probe_index: u32) -> array<vec3<f32>, 9> {
    let base = probe_offset + probe_index * PROBE_F32_COUNT;
    var bands: array<vec3<f32>, 9>;
    for (var b: u32 = 0u; b < 9u; b = b + 1u) {
        let off = base + b * 3u;
        bands[b] = vec3<f32>(
            delta_probes[off],
            delta_probes[off + 1u],
            delta_probes[off + 2u],
        );
    }
    return bands;
}

// Trilinearly sample the delta grid for a given animated light at
// `local_pos` (in cells, AABB-local; e.g. 0.5 means halfway between the
// origin and the next cell along x). Returns 9 SH bands × RGB.
//
// Out-of-bounds positions return zero; the caller's bounds check filters
// most of those, but corner-clamping inside the function keeps the code
// safe against sub-cell drift at the AABB edge.
fn sample_delta_trilinear(
    light_meta: DeltaLightMeta,
    local_pos: vec3<f32>,
) -> array<vec3<f32>, 9> {
    var result: array<vec3<f32>, 9>;
    for (var b: u32 = 0u; b < 9u; b = b + 1u) {
        result[b] = vec3<f32>(0.0);
    }

    let dims = vec3<i32>(
        i32(light_meta.grid_dimensions.x),
        i32(light_meta.grid_dimensions.y),
        i32(light_meta.grid_dimensions.z),
    );
    if (dims.x <= 0 || dims.y <= 0 || dims.z <= 0) {
        return result;
    }

    // Clamp the floor and ceil to in-bounds cells.
    let max_idx = vec3<f32>(f32(dims.x - 1), f32(dims.y - 1), f32(dims.z - 1));
    let p = clamp(local_pos, vec3<f32>(0.0), max_idx);
    let p0 = vec3<i32>(i32(floor(p.x)), i32(floor(p.y)), i32(floor(p.z)));
    let p1 = vec3<i32>(
        min(p0.x + 1, dims.x - 1),
        min(p0.y + 1, dims.y - 1),
        min(p0.z + 1, dims.z - 1),
    );
    let f = fract(p);

    // Z-major then Y then X — same convention as the base SH section.
    let strides = vec3<u32>(
        1u,
        light_meta.grid_dimensions.x,
        light_meta.grid_dimensions.x * light_meta.grid_dimensions.y,
    );

    let i000 = u32(p0.x) * strides.x + u32(p0.y) * strides.y + u32(p0.z) * strides.z;
    let i100 = u32(p1.x) * strides.x + u32(p0.y) * strides.y + u32(p0.z) * strides.z;
    let i010 = u32(p0.x) * strides.x + u32(p1.y) * strides.y + u32(p0.z) * strides.z;
    let i110 = u32(p1.x) * strides.x + u32(p1.y) * strides.y + u32(p0.z) * strides.z;
    let i001 = u32(p0.x) * strides.x + u32(p0.y) * strides.y + u32(p1.z) * strides.z;
    let i101 = u32(p1.x) * strides.x + u32(p0.y) * strides.y + u32(p1.z) * strides.z;
    let i011 = u32(p0.x) * strides.x + u32(p1.y) * strides.y + u32(p1.z) * strides.z;
    let i111 = u32(p1.x) * strides.x + u32(p1.y) * strides.y + u32(p1.z) * strides.z;

    let b000 = read_delta_probe(light_meta.probe_offset, i000);
    let b100 = read_delta_probe(light_meta.probe_offset, i100);
    let b010 = read_delta_probe(light_meta.probe_offset, i010);
    let b110 = read_delta_probe(light_meta.probe_offset, i110);
    let b001 = read_delta_probe(light_meta.probe_offset, i001);
    let b101 = read_delta_probe(light_meta.probe_offset, i101);
    let b011 = read_delta_probe(light_meta.probe_offset, i011);
    let b111 = read_delta_probe(light_meta.probe_offset, i111);

    let w000 = (1.0 - f.x) * (1.0 - f.y) * (1.0 - f.z);
    let w100 = f.x * (1.0 - f.y) * (1.0 - f.z);
    let w010 = (1.0 - f.x) * f.y * (1.0 - f.z);
    let w110 = f.x * f.y * (1.0 - f.z);
    let w001 = (1.0 - f.x) * (1.0 - f.y) * f.z;
    let w101 = f.x * (1.0 - f.y) * f.z;
    let w011 = (1.0 - f.x) * f.y * f.z;
    let w111 = f.x * f.y * f.z;

    for (var b: u32 = 0u; b < 9u; b = b + 1u) {
        result[b] =
            b000[b] * w000 + b100[b] * w100 +
            b010[b] * w010 + b110[b] * w110 +
            b001[b] * w001 + b101[b] * w101 +
            b011[b] * w011 + b111[b] * w111;
    }
    return result;
}

@compute @workgroup_size(4, 4, 4)
fn compose_main(@builtin(global_invocation_id) gid: vec3<u32>) {
    if (gid.x >= grid.dims.x || gid.y >= grid.dims.y || gid.z >= grid.dims.z) {
        return;
    }
    let p = vec3<i32>(i32(gid.x), i32(gid.y), i32(gid.z));

    // Load base SH bands at this probe.
    var bands: array<vec3<f32>, 9>;
    bands[0] = textureLoad(sh_base_band0, p, 0).rgb;
    bands[1] = textureLoad(sh_base_band1, p, 0).rgb;
    bands[2] = textureLoad(sh_base_band2, p, 0).rgb;
    bands[3] = textureLoad(sh_base_band3, p, 0).rgb;
    bands[4] = textureLoad(sh_base_band4, p, 0).rgb;
    bands[5] = textureLoad(sh_base_band5, p, 0).rgb;
    bands[6] = textureLoad(sh_base_band6, p, 0).rgb;
    bands[7] = textureLoad(sh_base_band7, p, 0).rgb;
    bands[8] = textureLoad(sh_base_band8, p, 0).rgb;

    // World-space position of this probe (cell center is the index, since
    // the bake plants probes at integer multiples of cell_size from origin).
    let world_pos =
        grid_frame.grid_origin
        + grid_frame.cell_size * vec3<f32>(f32(gid.x), f32(gid.y), f32(gid.z));

    // Accumulate weighted delta contributions. Empty delta_lights array
    // (delta_light_count == 0) skips this loop entirely.
    for (var li: u32 = 0u; li < grid.delta_light_count; li = li + 1u) {
        let light_meta = delta_lights[li];

        // Skip lights flagged with sentinel descriptor index (descriptor
        // out of range — see `build_delta_buffers`).
        if (light_meta.descriptor_index == 0xffffffffu) {
            continue;
        }

        // Project world position into delta-grid local cell space.
        let inv_cell = 1.0 / max(light_meta.cell_size, 1.0e-6);
        let local_pos = (world_pos - light_meta.aabb_origin) * inv_cell;

        // Bounds check: the probe must fall within the AABB cell range.
        let max_xyz = vec3<f32>(
            f32(light_meta.grid_dimensions.x - 1u),
            f32(light_meta.grid_dimensions.y - 1u),
            f32(light_meta.grid_dimensions.z - 1u),
        );
        if (local_pos.x < 0.0 || local_pos.y < 0.0 || local_pos.z < 0.0
            || local_pos.x > max_xyz.x || local_pos.y > max_xyz.y || local_pos.z > max_xyz.z) {
            continue;
        }

        let desc = descriptors[light_meta.descriptor_index];
        if (desc.is_active == 0u) {
            continue;
        }

        // Evaluate the animation curve at the current time. `period` may
        // be 0 for static-by-construction descriptors; clamp to avoid
        // divide-by-zero. The animated-lightmap pass uses the same guard.
        let t = fract(uniforms.time / max(desc.period, 1.0e-6) + desc.phase);
        let brightness =
            sample_curve_catmull_rom(desc.brightness_offset, desc.brightness_count, t);
        let color = sample_color_catmull_rom(
            desc.color_offset,
            desc.color_count,
            t,
            desc.base_color,
        );

        // Trilinearly sample the delta grid and accumulate.
        let delta = sample_delta_trilinear(light_meta, local_pos);
        let weight = brightness;
        for (var b: u32 = 0u; b < 9u; b = b + 1u) {
            // Per-channel modulate by `color` so a colored animation curve
            // tints the delta SH the same way the lightmap pass does.
            bands[b] = bands[b] + delta[b] * color * weight;
        }
    }

    textureStore(sh_total_band0, p, vec4<f32>(bands[0], 0.0));
    textureStore(sh_total_band1, p, vec4<f32>(bands[1], 0.0));
    textureStore(sh_total_band2, p, vec4<f32>(bands[2], 0.0));
    textureStore(sh_total_band3, p, vec4<f32>(bands[3], 0.0));
    textureStore(sh_total_band4, p, vec4<f32>(bands[4], 0.0));
    textureStore(sh_total_band5, p, vec4<f32>(bands[5], 0.0));
    textureStore(sh_total_band6, p, vec4<f32>(bands[6], 0.0));
    textureStore(sh_total_band7, p, vec4<f32>(bands[7], 0.0));
    textureStore(sh_total_band8, p, vec4<f32>(bands[8], 0.0));
}
