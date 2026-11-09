// Mover probe channel reads, shared by both direct-SH compose passes.
// See: context/lib/rendering_pipeline.md §4 (Mover state lighting)
//
// The including shader binds `mover_channel: array<u32>`, the renderer-built
// table of `render/mover_channel/layout.rs`: listed affinity cells, one term
// record per (cell, gated light) with its probe mask, its gating movers, each
// mover's q₀/q₁ bytes per masked probe, the light's term T per masked probe,
// and one t′ per mover (negative while the game hands over no state).
//
// A record's probe mask is storage sparsity only: an unmasked probe composes
// as with no section. Mover state blends visibility; it never flags, zeroes
// or excludes a probe.

const MC_HEADER_CELL_COUNT: u32 = 0u;
const MC_HEADER_CELLS: u32 = 1u;
const MC_HEADER_CELL_RECORDS: u32 = 2u;
const MC_HEADER_RECORDS: u32 = 3u;
const MC_HEADER_MOVER_ENTRIES: u32 = 4u;
const MC_HEADER_MOVER_T: u32 = 5u;
const MC_HEADER_DESCRIPTORS: u32 = 6u;
const MC_HEADER_DESCRIPTOR_COUNT: u32 = 7u;
const MC_RECORD_WORDS: u32 = 8u;
const MC_RECORD_MASK_LOW: u32 = 0u;
const MC_RECORD_MASK_HIGH: u32 = 1u;
const MC_RECORD_LIGHT: u32 = 2u;
const MC_RECORD_MOVERS_START: u32 = 3u;
const MC_RECORD_MOVERS_END: u32 = 4u;
const MC_RECORD_TILE_HALF: u32 = 5u;
const MC_MOVER_ENTRY_WORDS: u32 = 2u;
// One compact 6×6 RGB f16 tile: id 41's stride.
const MC_TILE_HALVES: u32 = 108u;
const MC_NONE: u32 = 0xffffffffu;

/// The listed position of affinity cell `cell`, or `MC_NONE`.
fn mc_listed_position(cell: u32) -> u32 {
    let cells = mover_channel[MC_HEADER_CELLS];
    var low = 0u;
    var high = mover_channel[MC_HEADER_CELL_COUNT];
    loop {
        if (low >= high) {
            break;
        }
        let mid = low + (high - low) / 2u;
        let listed = mover_channel[cells + mid];
        if (listed == cell) {
            return mid;
        }
        if (listed < cell) {
            low = mid + 1u;
        } else {
            high = mid;
        }
    }
    return MC_NONE;
}

/// Records of listed cell `position`: static lights in `[start, static_end)`,
/// animated-baked lights in `[static_end, end)`.
struct McCellRecords {
    start: u32,
    static_end: u32,
    end: u32,
}

fn mc_cell_records(position: u32) -> McCellRecords {
    let at = mover_channel[MC_HEADER_CELL_RECORDS] + position * 2u;
    return McCellRecords(mover_channel[at], mover_channel[at + 1u], mover_channel[at + 2u]);
}

fn mc_record_word(record: u32, field: u32) -> u32 {
    return mover_channel[mover_channel[MC_HEADER_RECORDS] + record * MC_RECORD_WORDS + field];
}

/// The record's masked slot for `local_probe` (its x-fastest rank among the
/// mask's bits), or `MC_NONE` where the record stores nothing.
fn mc_record_slot(record: u32, local_probe: u32) -> u32 {
    let low = mc_record_word(record, MC_RECORD_MASK_LOW);
    let high = mc_record_word(record, MC_RECORD_MASK_HIGH);
    let word = select(low, high, local_probe >= 32u);
    let bit = local_probe % 32u;
    if (((word >> bit) & 1u) == 0u) {
        return MC_NONE;
    }
    let below = countOneBits(word & ((1u << bit) - 1u));
    return select(below, countOneBits(low) + below, local_probe >= 32u);
}

fn mc_byte(index: u32) -> u32 {
    return (mover_channel[index / 4u] >> ((index % 4u) * 8u)) & 0xffu;
}

/// The light's surviving visibility at the record's masked `slot`: the
/// product over its gating movers of lerp(q₀, q₁, t′). A mover the game
/// handed over no state for leaves the light ungated (factor 1).
fn mc_record_visibility(record: u32, slot: u32) -> f32 {
    let entries = mover_channel[MC_HEADER_MOVER_ENTRIES];
    let mover_t = mover_channel[MC_HEADER_MOVER_T];
    var visibility = 1.0;
    let end = mc_record_word(record, MC_RECORD_MOVERS_END);
    for (var entry = mc_record_word(record, MC_RECORD_MOVERS_START); entry < end; entry = entry + 1u) {
        let at = entries + entry * MC_MOVER_ENTRY_WORDS;
        let t = bitcast<f32>(mover_channel[mover_t + mover_channel[at]]);
        // Negative (or non-finite) t: no state handed over this frame.
        if (!(t >= 0.0)) {
            continue;
        }
        let q_byte = mover_channel[at + 1u] + slot * 2u;
        let q0 = f32(mc_byte(q_byte)) / 255.0;
        let q1 = f32(mc_byte(q_byte + 1u)) / 255.0;
        visibility = visibility * mix(q0, q1, min(t, 1.0));
    }
    return visibility;
}

/// Texel `texel_index` of the light's term T at the record's masked `slot`.
fn mc_tile_texel(record: u32, slot: u32, texel_index: u32) -> vec3<f32> {
    let half_base = mc_record_word(record, MC_RECORD_TILE_HALF)
        + slot * MC_TILE_HALVES
        + texel_index * 3u;
    let word_base = half_base / 2u;
    let first = unpack2x16float(mover_channel[word_base]);
    let second = unpack2x16float(mover_channel[word_base + 1u]);
    return select(
        vec3<f32>(first.x, first.y, second.x),
        vec3<f32>(first.y, second.x, second.y),
        (half_base & 1u) != 0u,
    );
}

/// Pass B's raw AnimatedBakedLights index → animation descriptor index,
/// folded into this table so Pass B stays within eight storage buffers.
fn mc_descriptor_index(light_index: u32) -> u32 {
    if (light_index >= mover_channel[MC_HEADER_DESCRIPTOR_COUNT]) {
        return MC_NONE;
    }
    return mover_channel[mover_channel[MC_HEADER_DESCRIPTORS] + light_index];
}
