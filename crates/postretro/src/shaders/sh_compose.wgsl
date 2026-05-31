// SH compose compute pass.
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

struct GridDims {
    grid_dimensions: vec3<u32>,
    tile_dimension: u32,
    atlas_dimensions: vec2<u32>,
    tile_border: u32,
    delta_probe_f16_stride: u32,
    affinity_dims: vec3<u32>,
    _pad0: u32,
};

struct GridFrame {
    grid_origin: vec3<f32>,
    _pad0: f32,
    cell_size: vec3<f32>,
    _pad1: f32,
};

@group(0) @binding(0) var<uniform> uniforms: Uniforms;

@group(1) @binding(0) var sh_base_atlas: texture_2d<f32>;
@group(1) @binding(1) var sh_total_atlas: texture_storage_2d<rgba16float, write>;

@group(1) @binding(18) var<uniform> grid: GridDims;
@group(1) @binding(19) var<uniform> grid_frame: GridFrame;
// Sparse delta payload: one 64-probe octahedral-tile sub-block per CSR entry,
// RGBA16F texels packed two f16 halves per `u32`; `unpack2x16float` returns
// `(low, high)` matching the bake's even/odd channel order.
@group(1) @binding(20) var<storage, read> delta_subblocks: array<u32>;
// CSR offsets into `affinity_lights`, indexed by affinity-cell linear index;
// length is `affinity_cell_count + 1` (trailing total).
@group(1) @binding(21) var<storage, read> affinity_offsets: array<u32>;
@group(1) @binding(22) var<storage, read> descriptors: array<AnimationDescriptor>;
@group(1) @binding(23) var<storage, read> anim_samples: array<f32>;
// Flat CSR light indices, index-parallel to the delta sub-blocks: CSR entry
// `i` (light `affinity_lights[i]`) owns sub-block `i`.
@group(1) @binding(24) var<storage, read> affinity_lights: array<u32>;
// Maps delta-light index to the SH animation descriptor slot. `0xffffffff`
// means "no descriptor" and contributes nothing.
@group(1) @binding(25) var<storage, read> animation_descriptor_indices: array<u32>;

// Probes per affinity cell (4×4×4). Matches `PROBES_PER_CELL` in the bake.
const AFFINITY_FACTOR: u32 = 4u;
const PROBES_PER_CELL: u32 = 64u;
const INVALID_DESCRIPTOR_INDEX: u32 = 0xffffffffu;

struct AtlasTexelMapping {
    probe: vec3<u32>,
    tile_texel: vec2<u32>,
    in_grid: bool,
};

fn map_atlas_texel(atlas_texel: vec2<u32>) -> AtlasTexelMapping {
    let tile_dim = max(grid.tile_dimension, 1u);
    let tile = atlas_texel / vec2<u32>(tile_dim);
    let tile_texel = atlas_texel % vec2<u32>(tile_dim);

    let tile_rows = grid.grid_dimensions.y * grid.grid_dimensions.z;
    if (tile.x >= grid.grid_dimensions.x || tile.y >= tile_rows || grid.grid_dimensions.y == 0u) {
        return AtlasTexelMapping(vec3<u32>(0u), tile_texel, false);
    }

    let probe = vec3<u32>(
        tile.x,
        tile.y % grid.grid_dimensions.y,
        tile.y / grid.grid_dimensions.y,
    );
    return AtlasTexelMapping(probe, tile_texel, true);
}

struct AffinityMapping {
    cell_index: u32,
    local_probe: u32,
    in_range: bool,
};

fn map_probe_to_affinity(probe: vec3<u32>) -> AffinityMapping {
    let cell = probe / vec3<u32>(AFFINITY_FACTOR);
    if (any(cell >= grid.affinity_dims)) {
        return AffinityMapping(0u, 0u, false);
    }
    let local_coord = probe - cell * vec3<u32>(AFFINITY_FACTOR);
    let local = local_coord.x
        + local_coord.y * AFFINITY_FACTOR
        + local_coord.z * AFFINITY_FACTOR * AFFINITY_FACTOR;
    let cell_index = cell.x
        + cell.y * grid.affinity_dims.x
        + cell.z * grid.affinity_dims.x * grid.affinity_dims.y;
    return AffinityMapping(cell_index, local, true);
}

fn read_delta_texel(entry: u32, local_probe: u32, tile_texel: vec2<u32>) -> vec4<f32> {
    let texel_index = tile_texel.y * grid.tile_dimension + tile_texel.x;
    let half_base = (entry * PROBES_PER_CELL + local_probe) * grid.delta_probe_f16_stride
        + texel_index * 4u;
    let word_base = half_base / 2u;
    let rg = unpack2x16float(delta_subblocks[word_base]);
    let ba = unpack2x16float(delta_subblocks[word_base + 1u]);
    return vec4<f32>(rg.x, rg.y, ba.x, ba.y);
}

fn animated_light_scale(light_index: u32) -> vec3<f32> {
    let descriptor_index = animation_descriptor_indices[light_index];
    if (descriptor_index == INVALID_DESCRIPTOR_INDEX || descriptor_index >= arrayLength(&descriptors)) {
        return vec3<f32>(0.0);
    }
    let desc = descriptors[descriptor_index];
    if (desc.is_active == 0u) {
        return vec3<f32>(0.0);
    }

    let t = fract(uniforms.time / max(desc.period, 1.0e-6) + desc.phase);
    let brightness = max(
        sample_curve_catmull_rom(desc.brightness_offset, desc.brightness_count, t),
        0.0,
    );
    let color = max(
        sample_color_catmull_rom(desc.color_offset, desc.color_count, t, desc.base_color),
        vec3<f32>(0.0),
    );
    return color * brightness;
}

@compute @workgroup_size(8, 8, 1)
fn compose_main(
    @builtin(global_invocation_id) gid: vec3<u32>,
) {
    if (gid.x >= grid.atlas_dimensions.x || gid.y >= grid.atlas_dimensions.y) {
        return;
    }
    let p = vec2<i32>(i32(gid.x), i32(gid.y));
    let base = textureLoad(sh_base_atlas, p, 0);

    let atlas_mapping = map_atlas_texel(gid.xy);
    if (!atlas_mapping.in_grid || base.a < 0.5) {
        textureStore(sh_total_atlas, p, base);
        return;
    }

    let affinity = map_probe_to_affinity(atlas_mapping.probe);
    if (!affinity.in_range) {
        textureStore(sh_total_atlas, p, base);
        return;
    }

    let start = affinity_offsets[affinity.cell_index];
    let end = affinity_offsets[affinity.cell_index + 1u];
    var accum = base.rgb;
    for (var entry: u32 = start; entry < end; entry = entry + 1u) {
        let light_index = affinity_lights[entry];
        let scale = animated_light_scale(light_index);
        let delta = read_delta_texel(entry, affinity.local_probe, atlas_mapping.tile_texel);
        accum = accum + delta.rgb * scale;
    }

    textureStore(sh_total_atlas, p, vec4<f32>(accum, base.a));
}
