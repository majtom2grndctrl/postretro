// Mover probe channel bounce reads, for the indirect SH compose.
// See: context/lib/rendering_pipeline.md §4 (Mover state lighting)
//
// The including shader binds `mover_bounce: array<u32>`, the renderer-built
// table of `render/mover_channel/bounce.rs`: id 27's light → descriptor map,
// the listed cells, each cell's bounce entries (mover slot, affected-probe
// mask, first tile half in the cell's payload), one t′ per mover (negative
// while the game hands over no state), and each cell's payload run of
// S^b₀/S^b₁ tiles its base word points at. A streamed level places that run
// per resident cell; while it is not resident the base reads `MB_NONE` and the
// cell composes id 34 as with no section. That is residency, not a
// visibility switch: a row composes only with its cluster resident, and every
// cluster covering a cell carries its payload.
//
// The affected mask is storage sparsity only: an unmasked probe composes id
// 34 exactly as with no section. Mover state blends absolute static bounce
// states; it never flags, zeroes or excludes a probe, and no light's state
// drives any of it.

const MB_HEADER_DESCRIPTOR_COUNT: u32 = 0u;
const MB_HEADER_CELL_COUNT: u32 = 1u;
const MB_HEADER_CELLS: u32 = 2u;
const MB_HEADER_CELL_ENTRIES: u32 = 3u;
const MB_HEADER_CELL_BASES: u32 = 4u;
const MB_HEADER_ENTRIES: u32 = 5u;
const MB_HEADER_MOVER_T: u32 = 6u;
// Id 27's descriptor map starts right after the header.
const MB_DESCRIPTORS: u32 = 8u;
const MB_ENTRY_WORDS: u32 = 4u;
// One compact 6×6 RGB f16 tile: id 27's stride.
const MB_TILE_HALVES: u32 = 108u;
const MB_NONE: u32 = 0xffffffffu;

/// Id 27's delta light → animation descriptor slot, or `MB_NONE`.
fn mb_descriptor_index(light_index: u32) -> u32 {
    if (light_index >= mover_bounce[MB_HEADER_DESCRIPTOR_COUNT]) {
        return MB_NONE;
    }
    return mover_bounce[MB_DESCRIPTORS + light_index];
}

/// The listed position of affinity cell `cell`, or `MB_NONE` when the table
/// does not list it or its payload is not resident.
fn mb_listed_position(cell: u32) -> u32 {
    let cells = mover_bounce[MB_HEADER_CELLS];
    var low = 0u;
    var high = mover_bounce[MB_HEADER_CELL_COUNT];
    loop {
        if (low >= high) {
            break;
        }
        let mid = low + (high - low) / 2u;
        let listed = mover_bounce[cells + mid];
        if (listed == cell) {
            if (mover_bounce[mover_bounce[MB_HEADER_CELL_BASES] + mid] == MB_NONE) {
                return MB_NONE;
            }
            return mid;
        }
        if (listed < cell) {
            low = mid + 1u;
        } else {
            high = mid;
        }
    }
    return MB_NONE;
}

/// Listed cell `position`'s entries `[start, end)` and its payload's first
/// f16 half.
struct MbCellEntries {
    start: u32,
    end: u32,
    base_half: u32,
}

fn mb_cell_entries(position: u32) -> MbCellEntries {
    let at = mover_bounce[MB_HEADER_CELL_ENTRIES] + position;
    return MbCellEntries(
        mover_bounce[at],
        mover_bounce[at + 1u],
        mover_bounce[mover_bounce[MB_HEADER_CELL_BASES] + position] * 2u,
    );
}

fn mb_entry_word(entry: u32, field: u32) -> u32 {
    return mover_bounce[mover_bounce[MB_HEADER_ENTRIES] + entry * MB_ENTRY_WORDS + field];
}

/// The entry's mover t′; negative (or non-finite) while it has no state.
fn mb_entry_t(entry: u32) -> f32 {
    return bitcast<f32>(mover_bounce[mover_bounce[MB_HEADER_MOVER_T] + mb_entry_word(entry, 0u)]);
}

/// The first half of the entry's S^b₀ tile at `local_probe`, or `MB_NONE`
/// where its affected mask stores nothing; S^b₁ follows it.
fn mb_entry_tile(cell: MbCellEntries, entry: u32, local_probe: u32) -> u32 {
    let low = mb_entry_word(entry, 1u);
    let high = mb_entry_word(entry, 2u);
    let word = select(low, high, local_probe >= 32u);
    let bit = local_probe % 32u;
    if (((word >> bit) & 1u) == 0u) {
        return MB_NONE;
    }
    let below = countOneBits(word & ((1u << bit) - 1u));
    let rank = select(below, countOneBits(low) + below, local_probe >= 32u);
    return cell.base_half + mb_entry_word(entry, 3u) + rank * 2u * MB_TILE_HALVES;
}

/// Texel `texel_index` (RGB) of the tile whose first half is `tile_half`.
fn mb_tile_texel(tile_half: u32, texel_index: u32) -> vec3<f32> {
    let half_base = tile_half + texel_index * 3u;
    let word_base = half_base / 2u;
    let first = unpack2x16float(mover_bounce[word_base]);
    let second = unpack2x16float(mover_bounce[word_base + 1u]);
    return select(
        vec3<f32>(first.x, first.y, second.x),
        vec3<f32>(first.y, second.x, second.y),
        (half_base & 1u) != 0u,
    );
}
