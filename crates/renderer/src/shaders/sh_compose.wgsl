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
    atlas_tiles_per_row: u32,
    tiles_per_layer: u32,
    atlas_layer_count: u32,
    compact_atlas_tiles_per_row: u32,
    compact_atlas_tiles_per_layer: u32,
};

struct GridFrame {
    grid_origin: vec3<f32>,
    _pad0: f32,
    cell_size: vec3<f32>,
    _pad1: f32,
};

@group(0) @binding(0) var<uniform> uniforms: Uniforms;

@group(1) @binding(0) var sh_base_atlas: texture_2d_array<f32>;
@group(1) @binding(1) var sh_total_atlas: texture_storage_2d_array<rgba16float, write>;
@group(1) @binding(2) var sh_base_atlas_sampler: sampler;

@group(1) @binding(18) var<uniform> grid: GridDims;
@group(1) @binding(19) var<uniform> grid_frame: GridFrame;
// Sparse delta payload: valid-probe octahedral tiles per CSR entry, RGBA16F
// texels packed two f16 halves per `u32`; `unpack2x16float` returns `(low,
// high)` matching the bake's even/odd channel order.
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
// Dense-grid probe index -> compact id-34 tile slot. The sentinel means the
// metadata record is invalid and no base-atlas fetch may occur.
@group(1) @binding(26) var<storage, read> probe_indirection: array<u32>;
// Two u32 words per affinity-cell valid-probe mask, followed by one f16-half
// payload base offset per post-drop CSR entry. Binding 26 remains the base
// global-slot mapping and invalid-probe guard; this binding resolves only the
// delta's within-cell compact slot.
@group(1) @binding(27) var<storage, read> delta_compaction_meta: array<u32>;

// Affinity cells are 4×4×4 base probes. Matches the compiler bake.
const AFFINITY_FACTOR: u32 = 4u;
const INVALID_DESCRIPTOR_INDEX: u32 = 0xffffffffu;
const INVALID_PROBE_INDIRECTION: u32 = 0xffffffffu;

struct AtlasTexelMapping {
    probe: vec3<u32>,
    probe_index: u32,
    tile_texel: vec2<u32>,
    in_grid: bool,
};

fn map_atlas_texel(atlas_texel: vec3<u32>) -> AtlasTexelMapping {
    let tile_dim = max(grid.tile_dimension, 1u);
    let tile = atlas_texel.xy / vec2<u32>(tile_dim);
    let tile_texel = atlas_texel.xy % vec2<u32>(tile_dim);

    let total_probes = grid.grid_dimensions.x * grid.grid_dimensions.y * grid.grid_dimensions.z;
    let tiles_per_row = max(grid.atlas_tiles_per_row, 1u);
    let tiles_per_layer = max(grid.tiles_per_layer, 1u);
    let tile_slot = tile.x + tile.y * tiles_per_row;
    let probe_index = atlas_texel.z * tiles_per_layer + tile_slot;
    if (
        total_probes == 0u
        || atlas_texel.z >= grid.atlas_layer_count
        || tile.x >= tiles_per_row
        || tile_slot >= tiles_per_layer
        || probe_index >= total_probes
        || grid.grid_dimensions.x == 0u
        || grid.grid_dimensions.y == 0u
    ) {
        return AtlasTexelMapping(vec3<u32>(0u), 0u, tile_texel, false);
    }

    let xy = grid.grid_dimensions.x * grid.grid_dimensions.y;
    let z = probe_index / xy;
    let rem = probe_index - z * xy;
    let probe = vec3<u32>(
        rem % grid.grid_dimensions.x,
        rem / grid.grid_dimensions.x,
        z,
    );
    return AtlasTexelMapping(probe, probe_index, tile_texel, true);
}

fn sample_compact_base_atlas(compact_slot: u32, tile_texel: vec2<u32>) -> vec4<f32> {
    let compact_tiles_per_row = max(grid.compact_atlas_tiles_per_row, 1u);
    let compact_tiles_per_layer = max(grid.compact_atlas_tiles_per_layer, 1u);
    let layer = compact_slot / compact_tiles_per_layer;
    let tile_slot = compact_slot - layer * compact_tiles_per_layer;
    let tile_origin = vec2<u32>(
        tile_slot % compact_tiles_per_row,
        tile_slot / compact_tiles_per_row,
    ) * grid.tile_dimension;
    let compact_texel = tile_origin + tile_texel;
    let uv = (vec2<f32>(compact_texel) + 0.5)
        / vec2<f32>(textureDimensions(sh_base_atlas));
    return textureSampleLevel(
        sh_base_atlas,
        sh_base_atlas_sampler,
        uv,
        i32(layer),
        0.0,
    );
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

fn compaction_meta_offset_base() -> u32 {
    return grid.affinity_dims.x * grid.affinity_dims.y * grid.affinity_dims.z * 2u;
}

fn valid_probe_mask_word(cell: u32, word: u32) -> u32 {
    return delta_compaction_meta[cell * 2u + word];
}

fn within_cell_rank(cell: u32, local_probe: u32) -> u32 {
    let word = local_probe / 32u;
    let bit = local_probe % 32u;
    let prior_words = select(0u, countOneBits(valid_probe_mask_word(cell, 0u)), word == 1u);
    let earlier_in_word = countOneBits(valid_probe_mask_word(cell, word) & ((1u << bit) - 1u));
    return prior_words + earlier_in_word;
}

fn entry_delta_f16_offset(entry: u32) -> u32 {
    return delta_compaction_meta[compaction_meta_offset_base() + entry];
}

fn read_delta_texel(
    entry: u32,
    probe_rank: u32,
    tile_texel: vec2<u32>,
) -> vec4<f32> {
    let texel_index = tile_texel.y * grid.tile_dimension + tile_texel.x;
    let half_base = entry_delta_f16_offset(entry)
        + probe_rank * grid.delta_probe_f16_stride
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

    let t = animation_curve_t(desc.period, desc.phase, uniforms.time);
    let brightness = max(
        sample_curve_catmull_rom(desc.brightness_offset, desc.brightness_count, t),
        0.0,
    );
    var color = desc.base_color;
    if (desc.color_count > 0u) {
        // For color-animation descriptors base_color is intensity splatted
        // across RGB. Delta tiles contain unit-radiance transport, so this is
        // the single authored-radiance application.
        color = max(
            sample_color_catmull_rom(desc.color_offset, desc.color_count, t, vec3<f32>(1.0)),
            vec3<f32>(0.0),
        ) * desc.base_color;
    }
    return color * brightness;
}

@compute @workgroup_size(8, 8, 1)
fn compose_main(
    @builtin(global_invocation_id) gid: vec3<u32>,
) {
    if (
        gid.x >= grid.atlas_dimensions.x
        || gid.y >= grid.atlas_dimensions.y
        || gid.z >= grid.atlas_layer_count
    ) {
        return;
    }
    let p = vec2<i32>(i32(gid.x), i32(gid.y));
    let layer = i32(gid.z);

    let atlas_mapping = map_atlas_texel(gid);
    if (!atlas_mapping.in_grid) {
        textureStore(sh_total_atlas, p, layer, vec4<f32>(0.0));
        return;
    }
    let compact_slot = probe_indirection[atlas_mapping.probe_index];
    if (compact_slot == INVALID_PROBE_INDIRECTION) {
        textureStore(sh_total_atlas, p, layer, vec4<f32>(0.0));
        return;
    }
    let base = sample_compact_base_atlas(compact_slot, atlas_mapping.tile_texel);

    let affinity = map_probe_to_affinity(atlas_mapping.probe);
    if (!affinity.in_range) {
        textureStore(sh_total_atlas, p, layer, vec4<f32>(base.rgb, 1.0));
        return;
    }
    let probe_rank = within_cell_rank(affinity.cell_index, affinity.local_probe);

    let start = affinity_offsets[affinity.cell_index];
    let end = affinity_offsets[affinity.cell_index + 1u];
    var accum = base.rgb;
    for (var entry: u32 = start; entry < end; entry = entry + 1u) {
        let light_index = affinity_lights[entry];
        let scale = animated_light_scale(light_index);
        let delta = read_delta_texel(
            entry,
            probe_rank,
            atlas_mapping.tile_texel,
        );
        accum = accum + delta.rgb * scale;
    }

    textureStore(sh_total_atlas, p, layer, vec4<f32>(accum, 1.0));
}
