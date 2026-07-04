// Direct SH compose compute pass.
// See: context/lib/rendering_pipeline.md §4
//
// Produces a sampled Rgba16Float direct atlas from the BC6H/Rgba16Float base
// direct atlas minus selected static-light direct delta tiles.

struct GridDims {
    grid_dimensions: vec3<u32>,
    tile_dimension: u32,
    atlas_dimensions: vec2<u32>,
    tile_border: u32,
    delta_probe_f16_stride: u32,
    affinity_dims: vec3<u32>,
    atlas_tiles_per_row: u32,
};

struct DebugOverride {
    enabled: u32,
    selection_index: u32,
    _pad0: u32,
    _pad1: u32,
    weight: f32,
    _pad2: f32,
    _pad3: f32,
    _pad4: f32,
};

@group(0) @binding(0) var direct_base_atlas: texture_2d<f32>;
// Non-filtering (nearest) sampler. The base atlas is BC6H block-compressed at
// rest; Metal disallows textureLoad (lowered to `.read()`) on compressed formats,
// so the base value is fetched via a point sample at the exact texel center.
@group(0) @binding(2) var base_sampler: sampler;
@group(0) @binding(1) var direct_composed_atlas: texture_storage_2d<rgba16float, write>;
@group(0) @binding(18) var<uniform> grid: GridDims;
@group(0) @binding(20) var<storage, read> delta_subblocks: array<u32>;
@group(0) @binding(21) var<storage, read> affinity_offsets: array<u32>;
@group(0) @binding(24) var<storage, read> affinity_lights: array<u32>;
@group(0) @binding(26) var<storage, read> selection_weights: array<f32>;
@group(0) @binding(27) var<uniform> debug_override: DebugOverride;

const AFFINITY_FACTOR: u32 = 4u;
const PROBES_PER_CELL: u32 = 64u;

struct AtlasTexelMapping {
    probe: vec3<u32>,
    tile_texel: vec2<u32>,
    in_grid: bool,
};

fn map_atlas_texel(atlas_texel: vec2<u32>) -> AtlasTexelMapping {
    let tile_dim = max(grid.tile_dimension, 1u);
    let tile = atlas_texel / vec2<u32>(tile_dim);
    let tile_texel = atlas_texel % vec2<u32>(tile_dim);

    let total_probes = grid.grid_dimensions.x * grid.grid_dimensions.y * grid.grid_dimensions.z;
    let tiles_per_row = max(grid.atlas_tiles_per_row, 1u);
    if (
        total_probes == 0u
        || tile.x >= tiles_per_row
        || tile.x + tile.y * tiles_per_row >= total_probes
        || grid.grid_dimensions.x == 0u
        || grid.grid_dimensions.y == 0u
    ) {
        return AtlasTexelMapping(vec3<u32>(0u), tile_texel, false);
    }

    let probe_index = tile.x + tile.y * tiles_per_row;
    let xy = grid.grid_dimensions.x * grid.grid_dimensions.y;
    let z = probe_index / xy;
    let rem = probe_index - z * xy;
    let probe = vec3<u32>(
        rem % grid.grid_dimensions.x,
        rem / grid.grid_dimensions.x,
        z,
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

fn selection_weight(selection_index: u32) -> f32 {
    if (debug_override.enabled != 0u) {
        if (selection_index == debug_override.selection_index) {
            return clamp(debug_override.weight, 0.0, 1.0);
        }
        return 0.0;
    }
    if (selection_index >= arrayLength(&selection_weights)) {
        return 0.0;
    }
    return clamp(selection_weights[selection_index], 0.0, 1.0);
}

@compute @workgroup_size(8, 8, 1)
fn compose_main(@builtin(global_invocation_id) gid: vec3<u32>) {
    if (gid.x >= grid.atlas_dimensions.x || gid.y >= grid.atlas_dimensions.y) {
        return;
    }

    let p = vec2<i32>(i32(gid.x), i32(gid.y));
    // BC6H base atlas: sample at the exact texel center with a non-filtering
    // sampler. Nearest sampling at a texel-center UV returns the BC6H-decoded
    // texel verbatim — identical to textureLoad, which Metal rejects on
    // compressed formats — with no cross-texel blend. `p` (integer coord) still
    // drives the textureStore output below.
    let uv = (vec2<f32>(p) + 0.5) / vec2<f32>(textureDimensions(direct_base_atlas));
    let base = textureSampleLevel(direct_base_atlas, base_sampler, uv, 0.0);
    let atlas_mapping = map_atlas_texel(gid.xy);
    if (!atlas_mapping.in_grid) {
        textureStore(direct_composed_atlas, p, base);
        return;
    }

    let affinity = map_probe_to_affinity(atlas_mapping.probe);
    if (!affinity.in_range) {
        textureStore(direct_composed_atlas, p, base);
        return;
    }

    let start = affinity_offsets[affinity.cell_index];
    let end = affinity_offsets[affinity.cell_index + 1u];
    var accum = base.rgb;
    for (var entry: u32 = start; entry < end; entry = entry + 1u) {
        let selection_index = affinity_lights[entry];
        let w = selection_weight(selection_index);
        if (w > 0.0) {
            let delta = read_delta_texel(entry, affinity.local_probe, atlas_mapping.tile_texel);
            // Sum weighted deltas unclamped: per-light L2-SH deltas ring negative,
            // so clamping mid-sum in a multi-light cell recovers radiance the
            // final subtraction should remove. Clamp once, after the full sum.
            accum = accum - delta.rgb * w;
        }
    }

    textureStore(direct_composed_atlas, p, vec4<f32>(max(accum, vec3<f32>(0.0)), base.a));
}
