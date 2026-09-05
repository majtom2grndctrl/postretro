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
    tiles_per_layer: u32,
    atlas_layer_count: u32,
    _pad0: u32,
    _pad1: u32,
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

// Private Pass-A input. Its mask mirrors group-0's FrameUniforms snapshot
// because this compose pipeline has an independent bind-group layout.
struct DirectComposeParams {
    light_term_mask: u32,
    _pad0: u32,
    _pad1: u32,
    _pad2: u32,
};

@group(0) @binding(0) var direct_base_atlas: texture_2d_array<f32>;
// Non-filtering (nearest) sampler. The base atlas is BC6H block-compressed at
// rest; Metal disallows textureLoad (lowered to `.read()`) on compressed formats,
// so the base value is fetched via a point sample at the exact texel center.
@group(0) @binding(2) var base_sampler: sampler;
@group(0) @binding(1) var direct_composed_atlas: texture_storage_2d_array<rgba16float, write>;
@group(0) @binding(18) var<uniform> grid: GridDims;
@group(0) @binding(20) var<storage, read> delta_subblocks: array<u32>;
@group(0) @binding(21) var<storage, read> affinity_offsets: array<u32>;
@group(0) @binding(24) var<storage, read> affinity_lights: array<u32>;
@group(0) @binding(26) var<storage, read> selection_weights: array<f32>;
@group(0) @binding(27) var<uniform> debug_override: DebugOverride;
// Low/high u32 words for every affinity-cell valid-probe mask, followed by one
// widened coarsening level per cell, then one f16-half payload offset for every
// post-drop CSR entry. It describes the id-41 delta reconstruction only.
@group(0) @binding(28) var<storage, read> delta_compaction_meta: array<u32>;
@group(0) @binding(29) var<uniform> direct_compose_params: DirectComposeParams;
// Load-derived id-34 words are the sole source of base/direct stored-slot
// geometry. The direct base shares that exact slot space with id 34.
@group(0) @binding(30) var<storage, read> probe_indirection: array<u32>;

const AFFINITY_FACTOR: u32 = 4u;
const LIGHT_TERM_BAKED_DIRECT_STATIC: u32 = 0x08u;
const LIGHT_TERM_DYNAMIC_DIRECT: u32 = 0x20u;
// PRL validation pins the runtime tile dimension to 6. Keeping the shared
// lattice fixed-size makes one brick workgroup fit well below the 16 KiB
// WebGPU workgroup-storage floor.
const RUNTIME_TILE_DIMENSION: u32 = 6u;
const TILE_TEXEL_COUNT: u32 = RUNTIME_TILE_DIMENSION * RUNTIME_TILE_DIMENSION;
const MAX_KEPT_TILES: u32 = 8u;

// L1 stores at most its eight local corners; L2 stores one synthesized mean.
// The workgroup loads each stored tile once per CSR entry, then each valid
// output probe reconstructs from this brick-local lattice.
var<workgroup> shared_kept_tiles: array<vec4<f32>, 288>;
var<workgroup> shared_kept_present: array<u32, 8>;
var<workgroup> shared_brick_indirection: u32;

fn compaction_meta_offset_base() -> u32 {
    return grid.affinity_dims.x * grid.affinity_dims.y * grid.affinity_dims.z * 3u;
}

fn valid_probe_mask_word(cell: u32, word: u32) -> u32 {
    return delta_compaction_meta[cell * 2u + word];
}

fn cell_level(cell: u32) -> u32 {
    let cell_count = grid.affinity_dims.x * grid.affinity_dims.y * grid.affinity_dims.z;
    return delta_compaction_meta[cell_count * 2u + cell];
}

fn l1_corner_mask_word(word: u32) -> u32 {
    return select(0x00009009u, 0x90090000u, word == 1u);
}

fn l2_representative_local(cell: u32) -> u32 {
    let low = valid_probe_mask_word(cell, 0u);
    if (low != 0u) {
        return firstTrailingBit(low);
    }
    let high = valid_probe_mask_word(cell, 1u);
    if (high != 0u) {
        return 32u + firstTrailingBit(high);
    }
    return 0u;
}

fn kept_probe_mask_word(cell: u32, word: u32) -> u32 {
    let valid = valid_probe_mask_word(cell, word);
    let level = cell_level(cell);
    if (level == 1u) {
        return valid & l1_corner_mask_word(word);
    }
    if (level == 2u) {
        if (valid_probe_mask_word(cell, 0u) == 0u && valid_probe_mask_word(cell, 1u) == 0u) {
            return 0u;
        }
        let representative = l2_representative_local(cell);
        if (representative / 32u == word) {
            return 1u << (representative % 32u);
        }
        return 0u;
    }
    // The loader rejects levels outside 0..=2. Treat an impossible value as
    // L0 rather than indexing a non-existent compact tile.
    return valid;
}

fn local_probe_is_kept(cell: u32, local_probe: u32) -> bool {
    let word = local_probe / 32u;
    let bit = local_probe % 32u;
    return (kept_probe_mask_word(cell, word) & (1u << bit)) != 0u;
}

fn within_cell_rank(cell: u32, local_probe: u32) -> u32 {
    let word = local_probe / 32u;
    let bit = local_probe % 32u;
    let prior_words = select(0u, countOneBits(kept_probe_mask_word(cell, 0u)), word == 1u);
    let earlier_in_word = countOneBits(kept_probe_mask_word(cell, word) & ((1u << bit) - 1u));
    return prior_words + earlier_in_word;
}

fn entry_delta_f16_offset(entry: u32) -> u32 {
    return delta_compaction_meta[compaction_meta_offset_base() + entry];
}

fn read_delta_texel(entry: u32, probe_rank: u32, tile_texel: vec2<u32>) -> vec4<f32> {
    let texel_index = tile_texel.y * grid.tile_dimension + tile_texel.x;
    let half_base = entry_delta_f16_offset(entry)
        + probe_rank * grid.delta_probe_f16_stride
        + texel_index * 4u;
    let word_base = half_base / 2u;
    let rg = unpack2x16float(delta_subblocks[word_base]);
    let ba = unpack2x16float(delta_subblocks[word_base + 1u]);
    return vec4<f32>(rg.x, rg.y, ba.x, ba.y);
}

fn local_probe_coord(local_probe: u32) -> vec3<u32> {
    return vec3<u32>(
        local_probe % AFFINITY_FACTOR,
        (local_probe / AFFINITY_FACTOR) % AFFINITY_FACTOR,
        local_probe / (AFFINITY_FACTOR * AFFINITY_FACTOR),
    );
}

fn slot_tile_origin(slot: u32) -> vec3<u32> {
    let tiles_per_layer = max(grid.tiles_per_layer, 1u);
    let tile_slot = slot % tiles_per_layer;
    let tiles_per_row = max(grid.atlas_tiles_per_row, 1u);
    return vec3<u32>(
        (tile_slot % tiles_per_row) * grid.tile_dimension,
        (tile_slot / tiles_per_row) * grid.tile_dimension,
        slot / tiles_per_layer,
    );
}

fn local_probe_is_l1_corner(local_probe: u32) -> bool {
    let local = local_probe_coord(local_probe);
    return (local.x == 0u || local.x == AFFINITY_FACTOR - 1u)
        && (local.y == 0u || local.y == AFFINITY_FACTOR - 1u)
        && (local.z == 0u || local.z == AFFINITY_FACTOR - 1u);
}

struct ComposeStoredSlot {
    write: bool,
    valid: bool,
    slot: u32,
}

fn stored_slot_for_invocation(
    local_probe: u32,
    in_grid: bool,
    local_indirection: ShProbeIndirection,
    brick_indirection: ShProbeIndirection,
) -> ComposeStoredSlot {
    if (!in_grid || !brick_indirection.valid) {
        return ComposeStoredSlot(false, false, 0u);
    }
    if (brick_indirection.level == 0u) {
        return ComposeStoredSlot(
            local_indirection.valid,
            local_indirection.valid,
            local_indirection.slot,
        );
    }
    if (brick_indirection.level == 1u && local_probe_is_l1_corner(local_probe)) {
        return ComposeStoredSlot(
            true,
            local_indirection.valid,
            brick_indirection.slot + l1_shared_slot(local_probe),
        );
    }
    if (brick_indirection.level == 2u && local_probe == 0u) {
        return ComposeStoredSlot(true, true, brick_indirection.slot);
    }
    return ComposeStoredSlot(false, false, 0u);
}

fn l1_shared_slot(local_probe: u32) -> u32 {
    let local = local_probe_coord(local_probe);
    return (local.x / (AFFINITY_FACTOR - 1u))
        + (local.y / (AFFINITY_FACTOR - 1u)) * 2u
        + (local.z / (AFFINITY_FACTOR - 1u)) * 4u;
}

fn l1_corner_local(slot: u32) -> u32 {
    let local = vec3<u32>(
        select(0u, AFFINITY_FACTOR - 1u, (slot & 1u) != 0u),
        select(0u, AFFINITY_FACTOR - 1u, (slot & 2u) != 0u),
        select(0u, AFFINITY_FACTOR - 1u, (slot & 4u) != 0u),
    );
    return local.x + local.y * AFFINITY_FACTOR + local.z * AFFINITY_FACTOR * AFFINITY_FACTOR;
}

fn l1_corner_weight(target_local: u32, corner_local: u32) -> f32 {
    let target_coord = local_probe_coord(target_local);
    let corner = local_probe_coord(corner_local);
    let t = vec3<f32>(target_coord) / f32(AFFINITY_FACTOR - 1u);
    let wx = select(1.0 - t.x, t.x, corner.x == AFFINITY_FACTOR - 1u);
    let wy = select(1.0 - t.y, t.y, corner.y == AFFINITY_FACTOR - 1u);
    let wz = select(1.0 - t.z, t.z, corner.z == AFFINITY_FACTOR - 1u);
    return wx * wy * wz;
}

fn reconstruct_l1_shared_texel(target_local: u32, texel_index: u32) -> vec3<f32> {
    var accum = vec3<f32>(0.0);
    var weight_sum = 0.0;
    for (var slot = 0u; slot < MAX_KEPT_TILES; slot = slot + 1u) {
        if (shared_kept_present[slot] != 0u) {
            let weight = l1_corner_weight(target_local, l1_corner_local(slot));
            if (weight > 0.0) {
                accum = accum + shared_kept_tiles[slot * TILE_TEXEL_COUNT + texel_index].rgb * weight;
                weight_sum = weight_sum + weight;
            }
        }
    }
    if (weight_sum > 0.0) {
        return accum / weight_sum;
    }
    return vec3<f32>(0.0);
}

fn selection_weight(selection_index: u32) -> f32 {
    // Promoted static transport is removed only while both the baked-static
    // source and its runtime direct counterpart are present. Otherwise the
    // static-direct view either retains the full base or stays exactly zero.
    let required_terms = LIGHT_TERM_BAKED_DIRECT_STATIC | LIGHT_TERM_DYNAMIC_DIRECT;
    if ((direct_compose_params.light_term_mask & required_terms) != required_terms) {
        return 0.0;
    }
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
fn compose_main(
    @builtin(workgroup_id) brick: vec3<u32>,
    @builtin(local_invocation_id) local_id: vec3<u32>,
) {
    // One workgroup owns one 4×4×4 affinity brick. Only stored-slot owners
    // write; the remaining invocations still participate in shared barriers.
    let local_probe = local_id.x + local_id.y * 8u;
    let cell_index = brick.x
        + brick.y * grid.affinity_dims.x
        + brick.z * grid.affinity_dims.x * grid.affinity_dims.y;
    let probe = brick * AFFINITY_FACTOR + local_probe_coord(local_probe);
    let in_grid = !any(probe >= grid.grid_dimensions);
    let probe_index = probe.x
        + probe.y * grid.grid_dimensions.x
        + probe.z * grid.grid_dimensions.x * grid.grid_dimensions.y;
    var local_indirection = decode_sh_probe_indirection(0u);
    if (in_grid) {
        local_indirection = decode_sh_probe_indirection(probe_indirection[probe_index]);
    }
    if (local_probe == 0u) {
        shared_brick_indirection = 0u;
        for (var candidate_local = 0u; candidate_local < AFFINITY_FACTOR * AFFINITY_FACTOR * AFFINITY_FACTOR; candidate_local = candidate_local + 1u) {
            let candidate_probe = brick * AFFINITY_FACTOR + local_probe_coord(candidate_local);
            if (!any(candidate_probe >= grid.grid_dimensions)) {
                let candidate_index = candidate_probe.x
                    + candidate_probe.y * grid.grid_dimensions.x
                    + candidate_probe.z * grid.grid_dimensions.x * grid.grid_dimensions.y;
                let candidate_word = probe_indirection[candidate_index];
                if (decode_sh_probe_indirection(candidate_word).valid) {
                    shared_brick_indirection = candidate_word;
                    break;
                }
            }
        }
    }
    workgroupBarrier();
    let stored_slot = stored_slot_for_invocation(
        local_probe,
        in_grid,
        local_indirection,
        decode_sh_probe_indirection(shared_brick_indirection),
    );
    let output_is_stored = stored_slot.write;
    let tile_origin = slot_tile_origin(stored_slot.slot);
    let use_baked_direct_static = (direct_compose_params.light_term_mask & LIGHT_TERM_BAKED_DIRECT_STATIC) != 0u;
    let use_dynamic_direct = (direct_compose_params.light_term_mask & LIGHT_TERM_DYNAMIC_DIRECT) != 0u;
    let use_promotion_subtraction = use_baked_direct_static && use_dynamic_direct;

    // Keeping the accumulator private lets one shared kept lattice serve all
    // 64 output tiles without a second global delta read. The runtime tile
    // geometry is fixed at 6×6 by PRL validation.
    var accum: array<vec4<f32>, 36>;
    for (var texel_index = 0u; texel_index < TILE_TEXEL_COUNT; texel_index = texel_index + 1u) {
        if (output_is_stored && use_baked_direct_static) {
            let tile_texel = vec2<u32>(
                texel_index % RUNTIME_TILE_DIMENSION,
                texel_index / RUNTIME_TILE_DIMENSION,
            );
            let atlas_texel = tile_origin.xy + tile_texel;
            let uv = (vec2<f32>(atlas_texel) + 0.5)
                / vec2<f32>(textureDimensions(direct_base_atlas));
            accum[texel_index] = textureSampleLevel(
                direct_base_atlas,
                base_sampler,
                uv,
                i32(tile_origin.z),
                0.0,
            );
        } else {
            accum[texel_index] = vec4<f32>(0.0);
        }
    }

    let level = cell_level(cell_index);
    let start = affinity_offsets[cell_index];
    let end = affinity_offsets[cell_index + 1u];

    if (level == 0u) {
        // L0 omits invalid probes; L1 reserves all 8 corners. Keep L0's
        // compact-payload reads and do not spend shared memory loading 64 tiles.
        if (output_is_stored && use_promotion_subtraction) {
            let probe_rank = within_cell_rank(cell_index, local_probe);
            for (var entry = start; entry < end; entry = entry + 1u) {
                let w = selection_weight(affinity_lights[entry]);
                if (w > 0.0) {
                    for (var texel_index = 0u; texel_index < TILE_TEXEL_COUNT; texel_index = texel_index + 1u) {
                        let tile_texel = vec2<u32>(
                            texel_index % RUNTIME_TILE_DIMENSION,
                            texel_index / RUNTIME_TILE_DIMENSION,
                        );
                        // Sum weighted deltas unclamped: per-light L2-SH deltas
                        // ring negative, so clamping mid-sum in a multi-light
                        // cell recovers radiance the final subtraction should
                        // remove. Clamp once after the full sum.
                        let prior = accum[texel_index];
                        accum[texel_index] = vec4<f32>(
                            prior.rgb - read_delta_texel(entry, probe_rank, tile_texel).rgb * w,
                            prior.a,
                        );
                    }
                }
            }
        }
    } else if (use_promotion_subtraction) {
        // Coarsened L1/L2 cells load only their kept lattice into workgroup
        // memory. A load happens once per (brick, CSR entry, tile texel), then
        // every dropped-valid output probe reconstructs from those values.
        for (var entry = start; entry < end; entry = entry + 1u) {
            if (local_probe < MAX_KEPT_TILES) {
                shared_kept_present[local_probe] = 0u;
            }
            workgroupBarrier();

            if (level == 1u && local_probe_is_kept(cell_index, local_probe)) {
                let slot = l1_shared_slot(local_probe);
                let probe_rank = within_cell_rank(cell_index, local_probe);
                shared_kept_present[slot] = 1u;
                for (var texel_index = 0u; texel_index < TILE_TEXEL_COUNT; texel_index = texel_index + 1u) {
                    let tile_texel = vec2<u32>(
                        texel_index % RUNTIME_TILE_DIMENSION,
                        texel_index / RUNTIME_TILE_DIMENSION,
                    );
                    shared_kept_tiles[slot * TILE_TEXEL_COUNT + texel_index] = read_delta_texel(
                        entry,
                        probe_rank,
                        tile_texel,
                    );
                }
            }
            if (level == 2u) {
                let representative = l2_representative_local(cell_index);
                if (local_probe == representative && local_probe_is_kept(cell_index, local_probe)) {
                    let probe_rank = within_cell_rank(cell_index, local_probe);
                    shared_kept_present[0] = 1u;
                    for (var texel_index = 0u; texel_index < TILE_TEXEL_COUNT; texel_index = texel_index + 1u) {
                        let tile_texel = vec2<u32>(
                            texel_index % RUNTIME_TILE_DIMENSION,
                            texel_index / RUNTIME_TILE_DIMENSION,
                        );
                        shared_kept_tiles[texel_index] = read_delta_texel(
                            entry,
                            probe_rank,
                            tile_texel,
                        );
                    }
                }
            }
            workgroupBarrier();

            if (output_is_stored) {
                let w = selection_weight(affinity_lights[entry]);
                if (w > 0.0) {
                    for (var texel_index = 0u; texel_index < TILE_TEXEL_COUNT; texel_index = texel_index + 1u) {
                        var delta = vec3<f32>(0.0);
                        if (level == 2u) {
                            delta = shared_kept_tiles[texel_index].rgb
                                * f32(shared_kept_present[0]);
                        } else {
                            delta = reconstruct_l1_shared_texel(local_probe, texel_index);
                        }
                        let prior = accum[texel_index];
                        accum[texel_index] = vec4<f32>(prior.rgb - delta * w, prior.a);
                    }
                }
            }
            // No invocation may start loading the next entry until every
            // invocation has consumed this entry's shared lattice.
            workgroupBarrier();
        }
    }

    if (output_is_stored) {
        for (var texel_index = 0u; texel_index < TILE_TEXEL_COUNT; texel_index = texel_index + 1u) {
            let tile_texel = vec2<u32>(
                texel_index % RUNTIME_TILE_DIMENSION,
                texel_index / RUNTIME_TILE_DIMENSION,
            );
            textureStore(
                direct_composed_atlas,
                vec2<i32>(tile_origin.xy + tile_texel),
                i32(tile_origin.z),
                vec4<f32>(
                    max(accum[texel_index].rgb, vec3<f32>(0.0)),
                    select(0.0, 1.0, stored_slot.valid),
                ),
            );
        }
    }
}
