// Mover probe channel bounce reads, for the indirect SH compose.
// See: context/lib/rendering_pipeline.md §4 (Mover state lighting)
//
// The including shader binds `mover_bounce: array<u32>`, the renderer-built
// table of `render/mover_channel/bounce.rs`: id 27's light → descriptor map,
// the listed cells, each cell's bounce entries (mover slot, per-pose stored
// and base masks with each pose's first tile rank, the probes a read pose
// stores, first tile half in the cell's payload), each mover's knots through
// its ramp, one t′ per mover (negative while the game hands over no state),
// and each cell's payload run of stored tiles, pose-major then probe by
// stored rank, that its base word points at. A streamed level places that
// run per resident cell; while it is not resident the base reads `MB_NONE`
// and the cell composes id 34 as with no section. That is residency, not a
// visibility switch: a row composes only with its cluster resident, and
// every cluster covering a cell carries its payload.
//
// The masks are storage sparsity only: an unmasked probe composes id 34
// exactly as with no section; a terminus an affected probe stores no state
// for, or a mid pose marking it base, composes id 34's texel there; an
// unmarked mid pose is blended across. The blend runs between absolute
// states by the mover's t′ alone. Mover state blends absolute static bounce
// states; it never flags, zeroes or excludes a probe, and no light's state
// drives any of it.

const MB_HEADER_DESCRIPTOR_COUNT: u32 = 0u;
const MB_HEADER_CELL_COUNT: u32 = 1u;
const MB_HEADER_CELLS: u32 = 2u;
const MB_HEADER_CELL_ENTRIES: u32 = 3u;
const MB_HEADER_CELL_BASES: u32 = 4u;
const MB_HEADER_ENTRIES: u32 = 5u;
const MB_HEADER_MOVER_T: u32 = 6u;
// Word per mover slot: the absolute word of its knot run, `[N, E, (t′ bits,
// pose) × E]` (`render/mover_channel/knots.rs`).
const MB_HEADER_KNOTS: u32 = 7u;
// Id 27's descriptor map starts right after the header.
const MB_DESCRIPTORS: u32 = 8u;
// Mover slot, pose block word, read-stored mask (two words), first tile half.
const MB_ENTRY_WORDS: u32 = 5u;
// Per pose: stored mask (two words), base mask (two words), first tile rank.
const MB_POSE_WORDS: u32 = 5u;
// One compact 6×6 RGB f16 tile: id 27's stride.
const MB_TILE_HALVES: u32 = 108u;
const MB_NONE: u32 = 0xffffffffu;
// A mid pose the probe is unmarked at: blended across.
const MB_ACROSS: u32 = 0xfffffffeu;

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

/// Whether `local_probe`'s bit is set in the 64-bit mask at table words
/// `word` (low) and `word + 1` (high), and how many set bits precede it.
struct MbMaskBit {
    hit: bool,
    below: u32,
}

fn mb_mask_bit(word: u32, local_probe: u32) -> MbMaskBit {
    let low = mover_bounce[word];
    let high = mover_bounce[word + 1u];
    let bits = select(low, high, local_probe >= 32u);
    let bit = local_probe % 32u;
    let below = countOneBits(bits & ((1u << bit) - 1u));
    return MbMaskBit(
        ((bits >> bit) & 1u) != 0u,
        select(below, countOneBits(low) + below, local_probe >= 32u),
    );
}

fn mb_knot_t(run: u32, knot: u32) -> f32 {
    return bitcast<f32>(mover_bounce[run + 2u + 2u * knot]);
}

/// What pose `pose` holds for `local_probe`: its tile's first half (from
/// the entry's `first_half`), `MB_NONE` where it reads id 34 (a terminus
/// storing nothing, or a mid pose marked base), `MB_ACROSS` where an
/// unmarked mid pose is blended across.
fn mb_pose_state(
    poses: u32,
    pose: u32,
    terminus: bool,
    local_probe: u32,
    first_half: u32,
) -> u32 {
    let at = poses + pose * MB_POSE_WORDS;
    let stored = mb_mask_bit(at, local_probe);
    if (stored.hit) {
        return first_half + (mover_bounce[at + 4u] + stored.below) * MB_TILE_HALVES;
    }
    if (terminus || mb_mask_bit(at + 2u, local_probe).hit) {
        return MB_NONE;
    }
    return MB_ACROSS;
}

/// One entry's states at `local_probe` for a stated `t` (≥ 0): the first
/// halves of the two states around t′ (`MB_NONE` reads id 34's texel), and
/// the weight of the second; zero reads the first exactly. `affected` is
/// false where no pose compose reads stores a state for the probe, which
/// then composes id 34 exactly as with no section.
struct MbBlend {
    affected: bool,
    lo: u32,
    hi: u32,
    weight: f32,
}

/// The interval lookup, once per (entry, probe), never per texel: the
/// defined poses around t′ among the mover's knots (the termini always; a
/// mid pose where it stores a state or reads id 34) and the blend weight.
/// Mirrors `knots::interval`.
fn mb_entry_blend(cell: MbCellEntries, entry: u32, local_probe: u32, t: f32) -> MbBlend {
    if (!mb_mask_bit(mover_bounce[MB_HEADER_ENTRIES] + entry * MB_ENTRY_WORDS + 2u, local_probe).hit) {
        return MbBlend(false, MB_NONE, MB_NONE, 0.0);
    }
    let run = mover_bounce[mover_bounce[MB_HEADER_KNOTS] + mb_entry_word(entry, 0u)];
    let last = mover_bounce[run + 1u] - 1u;
    let poses = mb_entry_word(entry, 1u);
    let first_half = cell.base_half + mb_entry_word(entry, 4u);
    if (t >= 1.0) {
        let state = mb_pose_state(poses, mover_bounce[run] - 1u, true, local_probe, first_half);
        return MbBlend(true, state, state, 0.0);
    }
    var at = 0u;
    for (var knot = 1u; knot < last; knot = knot + 1u) {
        if (mb_knot_t(run, knot) > t) {
            break;
        }
        at = knot;
    }
    var lo = at;
    var lo_state = mb_pose_state(poses, mover_bounce[run + 3u + 2u * lo], lo == 0u, local_probe, first_half);
    while (lo > 0u && lo_state == MB_ACROSS) {
        lo = lo - 1u;
        lo_state = mb_pose_state(poses, mover_bounce[run + 3u + 2u * lo], lo == 0u, local_probe, first_half);
    }
    let t_lo = mb_knot_t(run, lo);
    if (t <= t_lo) {
        return MbBlend(true, lo_state, lo_state, 0.0);
    }
    var hi = at + 1u;
    var hi_state = mb_pose_state(poses, mover_bounce[run + 3u + 2u * hi], hi == last, local_probe, first_half);
    while (hi < last && hi_state == MB_ACROSS) {
        hi = hi + 1u;
        hi_state = mb_pose_state(poses, mover_bounce[run + 3u + 2u * hi], hi == last, local_probe, first_half);
    }
    return MbBlend(true, lo_state, hi_state, (t - t_lo) / (mb_knot_t(run, hi) - t_lo));
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
