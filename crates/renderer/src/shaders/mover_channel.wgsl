// Mover probe channel reads, shared by both direct-SH compose passes.
// See: context/lib/rendering_pipeline.md §4 (Mover state lighting)
//
// The including shader binds `mover_channel: array<u32>`, the renderer-built
// table of `render/mover_channel/layout.rs`: listed affinity cells, one term
// record per (cell, gated light) with its probe mask, its gating movers, each
// mover's knots through its ramp, and one t′ per mover (negative while the
// game hands over no state) with its knot bracket. Each listed cell's payload
// (the light's term T per masked probe, then each mover's q at each of its N
// poses) is one run its base word points at. A streamed level places that run per resident cell; while it is
// not resident the base reads `MC_NONE` and the cell composes as with no
// section. That is residency, not a visibility switch: a row composes only
// with its cluster resident, and every cluster covering a cell carries its
// payload.
//
// A record's probe mask is storage sparsity only: an unmasked probe composes
// as with no section. Every pose stores q, a mid pose that encloses the probe
// included (its traced q or, at the first and last pose of an enclosure, its
// clear neighbour's), so compose blends the two poses around t′. Mover
// state blends visibility; it never flags, zeroes or excludes a probe.

const MC_HEADER_CELL_COUNT: u32 = 0u;
const MC_HEADER_CELLS: u32 = 1u;
const MC_HEADER_CELL_RECORDS: u32 = 2u;
const MC_HEADER_RECORDS: u32 = 3u;
const MC_HEADER_MOVER_ENTRIES: u32 = 4u;
// `MC_T_SLOT_WORDS` words per mover slot: its t′ bits, then its knot
// bracket, the knot the direct pair and the bounce walk start from
// (`MoverKnots::bracket`), found once per mover per frame on the CPU so no
// probe scans the knots.
const MC_HEADER_MOVER_T: u32 = 5u;
const MC_HEADER_DESCRIPTORS: u32 = 6u;
const MC_HEADER_DESCRIPTOR_COUNT: u32 = 7u;
const MC_HEADER_CELL_BASES: u32 = 8u;
// Word per mover slot: the absolute word of its knot run, `[N, E, (t′ bits,
// pose) × E]`, E effective knots strictly ascending from (0, pose 0) to
// (1, pose N − 1) (`render/mover_channel/knots.rs`).
const MC_HEADER_KNOTS: u32 = 9u;
// Rust's `state::T_SLOT_WORDS`.
const MC_T_SLOT_WORDS: u32 = 2u;
const MC_RECORD_WORDS: u32 = 8u;
const MC_RECORD_MASK_LOW: u32 = 0u;
const MC_RECORD_MASK_HIGH: u32 = 1u;
const MC_RECORD_LIGHT: u32 = 2u;
const MC_RECORD_MOVERS_START: u32 = 3u;
const MC_RECORD_MOVERS_END: u32 = 4u;
// First term half within the cell's payload run.
const MC_RECORD_TILE_HALF: u32 = 5u;
// The record's listed cell, whose base word locates its payload.
const MC_RECORD_POSITION: u32 = 6u;
// Mover entry: slot, and first q byte within the cell's payload.
const MC_MOVER_ENTRY_WORDS: u32 = 2u;
// One compact 6×6 RGB f16 tile: id 41's stride.
const MC_TILE_HALVES: u32 = 108u;
const MC_NONE: u32 = 0xffffffffu;

/// The listed position of affinity cell `cell`, or `MC_NONE` when the table
/// does not list it or its payload is not resident.
fn mc_listed_position(cell: u32) -> u32 {
    let position = mc_find_cell(cell);
    if (position == MC_NONE) {
        return MC_NONE;
    }
    if (mover_channel[mover_channel[MC_HEADER_CELL_BASES] + position] == MC_NONE) {
        return MC_NONE;
    }
    return position;
}

/// The record's cell payload base, in words.
fn mc_record_base(record: u32) -> u32 {
    return mover_channel[mover_channel[MC_HEADER_CELL_BASES] + mc_record_word(record, MC_RECORD_POSITION)];
}

fn mc_find_cell(cell: u32) -> u32 {
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

fn mc_knot_t(run: u32, knot: u32) -> f32 {
    return bitcast<f32>(mover_channel[run + 2u + 2u * knot]);
}

fn mc_knot_pose(run: u32, knot: u32) -> u32 {
    return mover_channel[run + 3u + 2u * knot];
}

/// Slot `mover`'s t′: negative (or non-finite) while it has no state.
fn mc_mover_t(mover: u32) -> f32 {
    return bitcast<f32>(mover_channel[mover_channel[MC_HEADER_MOVER_T] + MC_T_SLOT_WORDS * mover]);
}

/// Slot `mover`'s knot bracket at its t′: below t′ 1, the last knot at or
/// below it.
fn mc_mover_bracket(mover: u32) -> u32 {
    return mover_channel[mover_channel[MC_HEADER_MOVER_T] + MC_T_SLOT_WORDS * mover + 1u];
}

struct McInterval {
    lo: u32,
    hi: u32,
    weight: f32,
}

/// Poses `lo` and `hi` around a stated `t` (≥ 0) in the knot run at `run`,
/// and the weight of `hi`: zero reads `lo` exactly. `at` is the mover's
/// knot bracket at `t` (`mc_mover_bracket`), so the cost is flat in knot
/// count. Direct stores q at every pose, so the interval is the bracket's
/// knot and the next. Mirrors `knots::interval` with every pose defined.
fn mc_interval(run: u32, t: f32, at: u32) -> McInterval {
    let last = mover_channel[run + 1u] - 1u;
    if (t >= 1.0) {
        let pose = mc_knot_pose(run, last);
        return McInterval(pose, pose, 0.0);
    }
    // Below t′ 1 the bracket is below the last knot, so `at + 1` exists.
    let lo_pose = mc_knot_pose(run, at);
    let t_lo = mc_knot_t(run, at);
    if (t <= t_lo) {
        return McInterval(lo_pose, lo_pose, 0.0);
    }
    let hi = at + 1u;
    return McInterval(
        lo_pose,
        mc_knot_pose(run, hi),
        (t - t_lo) / (mc_knot_t(run, hi) - t_lo),
    );
}

/// The light's surviving visibility at the record's masked `slot`: the
/// product over its gating movers of each one's q,
/// blended between the two poses around its t′ (exact at a knot). A
/// mover the game handed over no state for leaves the light ungated (factor
/// 1). Runs once per (record, probe), never per texel.
fn mc_record_visibility(record: u32, slot: u32) -> f32 {
    let entries = mover_channel[MC_HEADER_MOVER_ENTRIES];
    let base_byte = mc_record_base(record) * 4u;
    var visibility = 1.0;
    let end = mc_record_word(record, MC_RECORD_MOVERS_END);
    for (var entry = mc_record_word(record, MC_RECORD_MOVERS_START); entry < end; entry = entry + 1u) {
        let at = entries + entry * MC_MOVER_ENTRY_WORDS;
        let mover = mover_channel[at];
        let t = mc_mover_t(mover);
        // Negative (or non-finite) t: no state handed over this frame.
        if (!(t >= 0.0)) {
            continue;
        }
        let run = mover_channel[mover_channel[MC_HEADER_KNOTS] + mover];
        let span = mc_interval(run, t, mc_mover_bracket(mover));
        // N q bytes per masked probe, probe-major.
        let q_byte = base_byte + mover_channel[at + 1u] + slot * mover_channel[run];
        var q = f32(mc_byte(q_byte + span.lo)) / 255.0;
        if (span.weight > 0.0) {
            q = mix(q, f32(mc_byte(q_byte + span.hi)) / 255.0, span.weight);
        }
        visibility = visibility * q;
    }
    return visibility;
}

/// Texel `texel_index` of the light's term T at the record's masked `slot`.
fn mc_tile_texel(record: u32, slot: u32, texel_index: u32) -> vec3<f32> {
    let half_base = mc_record_base(record) * 2u
        + mc_record_word(record, MC_RECORD_TILE_HALF)
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
