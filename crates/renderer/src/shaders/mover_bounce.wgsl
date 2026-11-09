// Mover probe channel bounce reads, for the indirect SH compose.
// See: context/lib/rendering_pipeline.md §4 (Mover state lighting)
//
// The including shader binds `mover_bounce: array<u32>`, the renderer-built
// table of `render/mover_channel/bounce.rs`: id 27's light → descriptor map,
// the listed cells, each cell's bounce entries (mover slot, one stored-probe
// mask per terminus, first tile half in the cell's payload), one t′ per mover
// (negative while the game hands over no state), and each cell's payload run
// of stored S^b₀/S^b₁ tiles its base word points at. A streamed level places
// that run per resident cell; while it is not resident the base reads
// `MB_NONE` and the cell composes id 34 as with no section. That is
// residency, not a visibility switch: a row composes only with its cluster
// resident, and every cluster covering a cell carries its payload.
//
// The masks are storage sparsity only: an unmasked probe composes id 34
// exactly as with no section, and a terminus an affected probe stores no
// state for composes id 34's texel there, so the lerp runs toward id 34 by
// the mover's t′ alone. Mover state blends absolute static bounce states; it
// never flags, zeroes or excludes a probe, and no light's state drives any
// of it.

const MB_HEADER_DESCRIPTOR_COUNT: u32 = 0u;
const MB_HEADER_CELL_COUNT: u32 = 1u;
const MB_HEADER_CELLS: u32 = 2u;
const MB_HEADER_CELL_ENTRIES: u32 = 3u;
const MB_HEADER_CELL_BASES: u32 = 4u;
const MB_HEADER_ENTRIES: u32 = 5u;
const MB_HEADER_MOVER_T: u32 = 6u;
// Id 27's descriptor map starts right after the header.
const MB_DESCRIPTORS: u32 = 8u;
const MB_ENTRY_WORDS: u32 = 6u;
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

/// Whether `local_probe`'s bit is set in the 64-bit mask at entry words
/// `field` (low) and `field + 1` (high), and how many set bits precede it.
struct MbMaskBit {
    stored: bool,
    below: u32,
}

fn mb_mask_bit(entry: u32, field: u32, local_probe: u32) -> MbMaskBit {
    let low = mb_entry_word(entry, field);
    let high = mb_entry_word(entry, field + 1u);
    let word = select(low, high, local_probe >= 32u);
    let bit = local_probe % 32u;
    let below = countOneBits(word & ((1u << bit) - 1u));
    return MbMaskBit(
        ((word >> bit) & 1u) != 0u,
        select(below, countOneBits(low) + below, local_probe >= 32u),
    );
}

/// The first half of the entry's stored S^b₀ (`x`) and S^b₁ (`y`) tiles at
/// `local_probe`, each `MB_NONE` where that terminus stores no state and the
/// probe composes id 34 there. Both are `MB_NONE` where the probe is not
/// affected. Tiles run probe after probe, S^b₀ before S^b₁.
fn mb_entry_tiles(cell: MbCellEntries, entry: u32, local_probe: u32) -> vec2<u32> {
    let s0 = mb_mask_bit(entry, 1u, local_probe);
    let s1 = mb_mask_bit(entry, 3u, local_probe);
    let first = cell.base_half + mb_entry_word(entry, 5u)
        + (s0.below + s1.below) * MB_TILE_HALVES;
    let tile0 = select(MB_NONE, first, s0.stored);
    let tile1 = select(MB_NONE, first + select(0u, MB_TILE_HALVES, s0.stored), s1.stored);
    return vec2<u32>(tile0, tile1);
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
