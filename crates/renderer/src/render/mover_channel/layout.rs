// Mover probe channel table: id 57's direct block laid out for the two
// direct-SH compose passes, each gated light's term T resolved once per
// masked probe, and the per-mover row lists the streamed planner recomposes.
// Each listed cell's payload is one run of words its base word points at:
// fixed in a whole table, placed per resident cell in a pooled one.
// wgpu-free; `gpu.rs` owns the buffer this lays out.
// See: context/lib/rendering_pipeline.md §4 (Mover state lighting)

use std::collections::HashMap;
use std::ops::Range;

use postretro_level_format::mover_probe_channel::{
    DirectRecordView, MOVER_IRRADIANCE_TILE_F16_COUNT, MOVER_VISIBILITY_BYTES_PER_PROBE,
    MoverCellPayload, MoverCellSlices, MoverChannelBlock, MoverHalves, MoverLightNamespace,
    MoverProbeChannelSection, MoverStreamedRow,
};
use postretro_level_format::sh_reconstruct::{Level, kept_mask, stored_delta_tiles};

/// Header words, in `mover_channel.wgsl`'s `MC_HEADER_*` order.
const HEADER_WORDS: usize = 9;
/// Words per term record (`MC_RECORD_WORDS`): probe mask (two words), light
/// key, mover entry range, first term half within the cell's payload, and
/// the cell's listed position.
const RECORD_WORDS: usize = 8;
/// Words per (record, mover) entry: mover slot, then its first q byte
/// within the cell's payload.
const MOVER_ENTRY_WORDS: usize = 2;
/// A cell base word while its payload is not resident: the cell composes as
/// with no section.
pub(super) const NO_BASE: u32 = u32::MAX;
/// A record's light key when the light never promotes (no selection slot).
pub(super) const NO_LIGHT: u32 = u32::MAX;
/// A mover slot's t′ while the game hands over no state: ungated.
pub(super) const NO_STATE: f32 = -1.0;
const TILE_HALVES: usize = MOVER_IRRADIANCE_TILE_F16_COUNT;
/// An f16 whose exponent bits are all set is infinite or NaN.
const F16_EXPONENT: u16 = 0x7c00;

/// The first infinite or NaN half in `halves`, if any: every id-57 payload
/// and every borrowed term is checked with this as it loads or arrives.
pub(super) fn first_non_finite(halves: impl IntoIterator<Item = u16>) -> Option<u16> {
    halves
        .into_iter()
        .find(|&half| half & F16_EXPONENT == F16_EXPONENT)
}

/// Why the channel cannot be laid out or a cell cannot be written: the
/// section and the delta family it borrows T from disagree.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(in crate::render) struct MoverChannelError(pub(in crate::render) String);

impl std::fmt::Display for MoverChannelError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "MoverProbeChannel (id 57): {}", self.0)
    }
}

fn fail<T>(message: String) -> Result<T, MoverChannelError> {
    Err(MoverChannelError(message))
}

/// One sparse delta family (id 41 or id 45) as resident metadata: the same
/// in a whole load and a streamed one, where only its tiles stream.
#[derive(Clone, Copy, Debug)]
pub(in crate::render) struct TermFamily<'a> {
    pub(in crate::render) name: &'static str,
    pub(in crate::render) valid_probe_masks: &'a [u64],
    pub(in crate::render) cell_levels: &'a [u8],
    /// CSR, one offset per affinity cell plus a trailing total.
    pub(in crate::render) affinity_offsets: &'a [u32],
    pub(in crate::render) affinity_lights: &'a [u32],
}

impl TermFamily<'_> {
    /// The cell-local entry holding `light`'s per-probe term. The channel
    /// never rebuilds T from corners, so the cell must sit at L0 and keep
    /// every probe `mask` names.
    fn entry_in_cell(
        &self,
        cell: usize,
        light: u32,
        mask: u64,
    ) -> Result<usize, MoverChannelError> {
        let name = self.name;
        let (Some(&level), Some(&valid), Some(range)) = (
            self.cell_levels.get(cell),
            self.valid_probe_masks.get(cell),
            self.affinity_offsets.get(cell..cell + 2),
        ) else {
            return fail(format!("affinity cell {cell} is outside {name}"));
        };
        if Level::from_u8(level) != Some(Level::L0) {
            return fail(format!(
                "{name} stores affinity cell {cell} at level {level}, not L0, under gated probes"
            ));
        }
        if mask & !kept_mask(Level::L0, valid) != 0 {
            return fail(format!(
                "a gated probe of affinity cell {cell} is not a valid {name} probe"
            ));
        }
        let lights = self
            .affinity_lights
            .get(range[0] as usize..range[1] as usize)
            .unwrap_or(&[]);
        match lights.iter().position(|&entry| entry == light) {
            Some(entry) => Ok(entry),
            None => fail(format!(
                "{name} holds no entry for light {light} at affinity cell {cell}, and the record carries no tile of its own"
            )),
        }
    }
}

/// One affinity cell's tiles in a delta family: each entry's first f16 half
/// in `tiles`, and the cell's valid probes, which an L0 cell keeps in order.
#[derive(Clone, Copy, Debug)]
pub(in crate::render) struct TermRow<'a> {
    pub(in crate::render) entry_offsets: &'a [u32],
    pub(in crate::render) tiles: &'a [u16],
    pub(in crate::render) valid_probe_mask: u64,
}

impl<'a> TermRow<'a> {
    fn tile(&self, entry: usize, local_probe: u32) -> Option<&'a [u16]> {
        let below = (1u64 << local_probe) - 1;
        let rank = (self.valid_probe_mask & below).count_ones() as usize;
        let start = *self.entry_offsets.get(entry)? as usize + rank * TILE_HALVES;
        self.tiles.get(start..start + TILE_HALVES)
    }
}

/// A whole-loaded delta family's rows: each CSR entry's first f16 half in
/// the family's compact payload, entries in CSR order.
pub(in crate::render) struct WholeTermRows<'a> {
    family: TermFamily<'a>,
    tiles: &'a [u16],
    entry_offsets: Vec<u32>,
}

impl<'a> WholeTermRows<'a> {
    pub(in crate::render) fn new(family: TermFamily<'a>, tiles: &'a [u16]) -> Self {
        let cells = family.affinity_offsets.len().saturating_sub(1);
        let mut entry_offsets = Vec::with_capacity(family.affinity_lights.len());
        let mut next = 0u32;
        for cell in 0..cells {
            let level = Level::from_u8(family.cell_levels[cell]).unwrap_or(Level::L0);
            let kept = stored_delta_tiles(level, family.valid_probe_masks[cell]) as u32;
            for _ in family.affinity_offsets[cell]..family.affinity_offsets[cell + 1] {
                entry_offsets.push(next);
                next = next.saturating_add(kept * TILE_HALVES as u32);
            }
        }
        Self {
            family,
            tiles,
            entry_offsets,
        }
    }

    pub(in crate::render) fn row(&self, cell: u32) -> Option<TermRow<'_>> {
        let cell = cell as usize;
        let range = self.family.affinity_offsets.get(cell..cell + 2)?;
        Some(TermRow {
            entry_offsets: self
                .entry_offsets
                .get(range[0] as usize..range[1] as usize)?,
            tiles: self.tiles,
            valid_probe_mask: *self.family.valid_probe_masks.get(cell)?,
        })
    }
}

/// Where a GPU record's term T comes from.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum TermSource {
    /// The record's own tile, from id 57's payload.
    Own,
    /// Cell-local entry of id 41 (the light's selection slot).
    Static(usize),
    /// Cell-local entry of id 45.
    Animated(usize),
}

#[derive(Clone, Debug)]
struct RecordPlan {
    /// Index into the section's `direct_records`.
    section_record: usize,
    term: TermSource,
    probe_mask: u64,
}

/// One listed cell the table carries records for. Its payload is one run of
/// words: its records' terms (f16 halves), then their q bytes, so the whole
/// cell sits wherever its base word points.
#[derive(Clone, Debug)]
struct CellPlan {
    cell: u32,
    /// Position in the section's `cells`.
    position: usize,
    /// Its records in `MoverChannelLayout::records`: static lights first.
    records: Range<usize>,
    /// f16 halves of the term run (even, so word aligned).
    tile_halves: usize,
    /// Bytes of the visibility run, padded to a word.
    q_bytes: usize,
    needs_static_terms: bool,
    needs_animated_terms: bool,
    /// The cell's valid probes in ids 41 and 45, which an L0 cell keeps in
    /// order: a streamed row's tile rank.
    static_valid: u64,
    animated_valid: u64,
}

impl CellPlan {
    fn payload_words(&self) -> usize {
        self.tile_halves / 2 + self.q_bytes / 4
    }
}

/// Rows per mover slot, CSR: which affinity rows carry a mover's entries in
/// one pass.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(in crate::render) struct MoverRows {
    offsets: Vec<u32>,
    rows: Vec<u32>,
}

impl MoverRows {
    pub(super) fn from_sets(sets: Vec<Vec<u32>>) -> Self {
        let mut offsets = Vec::with_capacity(sets.len() + 1);
        let mut rows = Vec::new();
        offsets.push(0);
        for mut set in sets {
            set.sort_unstable();
            set.dedup();
            rows.extend(set);
            offsets.push(rows.len() as u32);
        }
        Self { offsets, rows }
    }

    /// Rows carrying mover slot `slot`'s entries, ascending.
    pub(in crate::render) fn rows(&self, slot: u32) -> &[u32] {
        let slot = slot as usize;
        match self.offsets.get(slot..slot + 2) {
            Some(range) => &self.rows[range[0] as usize..range[1] as usize],
            None => &[],
        }
    }

    /// Whether slot `slot` carries any entry in this pass.
    pub(super) fn has_rows(&self, slot: u32) -> bool {
        !self.rows(slot).is_empty()
    }
}

/// What the streamed planner reads of the channel: per-mover row lists for
/// each pass, and the rows whose membership the channel widens.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(in crate::render) struct MoverChannelRows {
    /// Pass A: rows carrying a mover's gated static lights. Pass B follows
    /// Pass A's planned rows on its own.
    pub(in crate::render) static_by_mover: MoverRows,
    /// Pass B: rows carrying a mover's gated animated-baked lights.
    pub(in crate::render) animated_by_mover: MoverRows,
    /// The indirect pass: rows carrying a mover's bounce entries.
    pub(in crate::render) bounce_by_mover: MoverRows,
    /// Rows with a gated static light that can promote: a promotion weight
    /// change moves their handoff even with no id-41 entry there.
    pub(in crate::render) promotable: Vec<u32>,
    /// Rows with a gated animated-baked light: its curve moves them even
    /// with no id-45 entry there.
    pub(in crate::render) animated: Vec<u32>,
}

/// A level with no channel, borrowable for any lifetime: planner tests that
/// hand the frame no mover rows.
#[cfg(test)]
pub(in crate::render) static NO_MOVER_ROWS: MoverChannelRows = MoverChannelRows {
    static_by_mover: MoverRows {
        offsets: Vec::new(),
        rows: Vec::new(),
    },
    animated_by_mover: MoverRows {
        offsets: Vec::new(),
        rows: Vec::new(),
    },
    bounce_by_mover: MoverRows {
        offsets: Vec::new(),
        rows: Vec::new(),
    },
    promotable: Vec::new(),
    animated: Vec::new(),
};

impl MoverChannelRows {
    pub(in crate::render) fn has_promotable(&self, row: u32) -> bool {
        self.promotable.binary_search(&row).is_ok()
    }

    pub(in crate::render) fn has_animated(&self, row: u32) -> bool {
        self.animated.binary_search(&row).is_ok()
    }
}

/// Everything the table is laid out from.
pub(in crate::render) struct MoverChannelInputs<'a> {
    pub(in crate::render) section: Option<&'a MoverProbeChannelSection>,
    /// Selection order: entry `s` is the AlphaLights index of selection slot
    /// `s` (id 40).
    pub(in crate::render) entity_shadow_lights: &'a [u32],
    /// Id 41, when the level has it.
    pub(in crate::render) static_terms: Option<TermFamily<'a>>,
    /// Id 45, when the level has it. Without it there is no Pass B, and an
    /// animated-baked light has no direct-SH term to gate.
    pub(in crate::render) animated_terms: Option<TermFamily<'a>>,
    /// Pass B's AnimatedBakedLights → descriptor map (id 45's).
    pub(in crate::render) animation_descriptor_indices: &'a [u32],
    /// The indirect compose's light → descriptor map (id 27's), folded into
    /// the bounce table the way id 45's folds into this one.
    pub(in crate::render) indirect_descriptor_indices: &'a [u32],
}

/// One listed cell's payload words, ready to place: its records' terms, then
/// their q bytes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(in crate::render) struct CellWrite {
    pub(in crate::render) plan: usize,
    pub(in crate::render) words: Vec<u32>,
}

/// The laid-out table and what writes and plans against it.
#[derive(Debug, Clone)]
pub(in crate::render) struct MoverChannelLayout {
    initial_words: Vec<u32>,
    /// Slot → id 43 `mover_id`, ascending.
    mover_ids: Vec<u32>,
    t_word: usize,
    /// First word of each cell's base word.
    bases_word: usize,
    /// First word of the payload region: every cell's payload in a whole
    /// table, the pool in a pooled one.
    payload_word: usize,
    /// Each cell's payload at a fixed base (`None`: pooled per resident cell).
    whole_bases: Option<Vec<u32>>,
    cells: Vec<CellPlan>,
    records: Vec<RecordPlan>,
    rows: MoverChannelRows,
    /// Records the table leaves ungated: their delta family is absent or
    /// disabled at load (id 41 is disabled by a cleared id-40 selection,
    /// whether or not its metadata is listed). An animated-baked record is
    /// ungated without id 45 even when it carries its own tile: Pass B gates
    /// by subtracting from the term id 45 added, and with no id 45 that term
    /// was never added.
    ungated_records: usize,
}

impl MoverChannelLayout {
    /// Lay out `inputs`. A record whose term family is disabled at load
    /// composes ungated; a family present but holding no L0 entry for a
    /// record that borrows from it is refused. `pooled` places each cell's
    /// payload per resident cell instead of at a fixed base.
    pub(in crate::render) fn build(
        inputs: &MoverChannelInputs<'_>,
        pooled: bool,
    ) -> Result<Self, MoverChannelError> {
        let Some(section) = inputs.section else {
            return Ok(Self::empty(inputs.animation_descriptor_indices));
        };
        // The loader disables id 41 by clearing the id-40 selection. A whole
        // load then drops id 41 as well, but a streamed load still lists its
        // metadata; either way a record borrowing from id 41 composes
        // ungated, so the same PRL composes alike in every mode.
        let static_terms = inputs
            .static_terms
            .filter(|_| !inputs.entity_shadow_lights.is_empty());
        let selection: HashMap<u32, u32> = inputs
            .entity_shadow_lights
            .iter()
            .enumerate()
            .map(|(slot, &light)| (light, slot as u32))
            .collect();
        let mover_ids = section.mover_ids();
        let slot_of = |mover: u32| mover_ids.binary_search(&mover).ok().map(|slot| slot as u32);

        let mut records = Vec::new();
        let mut keys = Vec::new();
        let mut static_ends = Vec::new();
        let mut cells = Vec::new();
        let mut ungated_records = 0usize;
        for (position, listed) in section.cells.iter().enumerate() {
            let cell = listed.cell as usize;
            let wire = section.direct_offsets[position] as usize
                ..section.direct_offsets[position + 1] as usize;
            let start = records.len();
            let (mut needs_static, mut needs_animated) = (false, false);
            let (mut tile_halves, mut q_bytes) = (0usize, 0usize);
            // Static lights first, then animated-baked ones, each in wire order.
            for animated_pass in [false, true] {
                for index in wire.clone() {
                    let record = &section.direct_records[index];
                    let animated = record.namespace == MoverLightNamespace::AnimatedBaked;
                    if animated != animated_pass {
                        continue;
                    }
                    let (term, key) = match record.namespace {
                        MoverLightNamespace::StaticLight => {
                            let slot = selection.get(&record.light_index).copied();
                            let term = if record.carries_own_tile {
                                TermSource::Own
                            } else if let Some(family) = static_terms {
                                let Some(slot) = slot else {
                                    return fail(format!(
                                        "direct record {index} borrows its term from {} but light {} holds no selection slot",
                                        family.name, record.light_index
                                    ));
                                };
                                needs_static = true;
                                TermSource::Static(family.entry_in_cell(
                                    cell,
                                    slot,
                                    record.probe_mask,
                                )?)
                            } else {
                                ungated_records += 1;
                                continue;
                            };
                            (term, slot.unwrap_or(NO_LIGHT))
                        }
                        MoverLightNamespace::BakeOnlyStaticLight => {
                            if !record.carries_own_tile {
                                return fail(format!(
                                    "bake-only direct record {index} carries no tile of its own"
                                ));
                            }
                            (TermSource::Own, NO_LIGHT)
                        }
                        MoverLightNamespace::AnimatedBaked => {
                            let Some(family) = inputs.animated_terms else {
                                ungated_records += 1;
                                continue;
                            };
                            let term = if record.carries_own_tile {
                                TermSource::Own
                            } else {
                                needs_animated = true;
                                TermSource::Animated(family.entry_in_cell(
                                    cell,
                                    record.light_index,
                                    record.probe_mask,
                                )?)
                            };
                            (term, record.light_index)
                        }
                    };
                    let probes = record.masked_probe_count();
                    tile_halves += probes * TILE_HALVES;
                    q_bytes += probes
                        * MOVER_VISIBILITY_BYTES_PER_PROBE
                        * section.record_movers(index).len();
                    records.push(RecordPlan {
                        section_record: index,
                        term,
                        probe_mask: record.probe_mask,
                    });
                    keys.push(key);
                }
                if !animated_pass {
                    static_ends.push(records.len());
                }
            }
            if records.len() == start {
                static_ends.pop();
                continue;
            }
            cells.push(CellPlan {
                cell: listed.cell,
                position,
                records: start..records.len(),
                tile_halves,
                q_bytes: q_bytes.next_multiple_of(4),
                needs_static_terms: needs_static,
                needs_animated_terms: needs_animated,
                static_valid: static_terms
                    .and_then(|family| family.valid_probe_masks.get(cell).copied())
                    .unwrap_or(0),
                animated_valid: inputs
                    .animated_terms
                    .and_then(|family| family.valid_probe_masks.get(cell).copied())
                    .unwrap_or(0),
            });
        }

        let mover_entries: usize = records
            .iter()
            .map(|record| section.record_movers(record.section_record).len())
            .sum();
        let cells_word = HEADER_WORDS;
        let cell_records_word = cells_word + cells.len();
        let bases_word = cell_records_word + 2 * cells.len() + 1;
        let records_word = bases_word + cells.len();
        let entries_word = records_word + records.len() * RECORD_WORDS;
        let descriptors_word = entries_word + mover_entries * MOVER_ENTRY_WORDS;
        let t_word = descriptors_word + inputs.animation_descriptor_indices.len();
        let payload_word = t_word + mover_ids.len();
        let whole_payload: usize = cells.iter().map(CellPlan::payload_words).sum();
        let total_words = payload_word + if pooled { 0 } else { whole_payload };
        // The shader addresses q bytes and tile halves within the table, a
        // pool included: its words are bounded by the whole payload's.
        as_u32(
            (payload_word + whole_payload) * 4,
            "byte size with every cell resident",
        )?;

        let mut words = vec![0u32; total_words];
        words[0] = cells.len() as u32;
        words[1] = cells_word as u32;
        words[2] = cell_records_word as u32;
        words[3] = records_word as u32;
        words[4] = entries_word as u32;
        words[5] = t_word as u32;
        words[6] = descriptors_word as u32;
        words[7] = inputs.animation_descriptor_indices.len() as u32;
        words[8] = bases_word as u32;
        let mut whole_bases = (!pooled).then(|| Vec::with_capacity(cells.len()));
        let mut static_rows = vec![Vec::new(); mover_ids.len()];
        let mut animated_rows = vec![Vec::new(); mover_ids.len()];
        let mut promotable = Vec::new();
        let mut animated_cells = Vec::new();
        let mut entry = 0usize;
        let mut next_base = payload_word;
        for (index, cell) in cells.iter().enumerate() {
            words[cells_word + index] = cell.cell;
            words[cell_records_word + 2 * index] = cell.records.start as u32;
            words[cell_records_word + 2 * index + 1] = static_ends[index] as u32;
            words[bases_word + index] = match whole_bases.as_mut() {
                Some(bases) => {
                    let base = next_base as u32;
                    bases.push(base);
                    next_base += cell.payload_words();
                    base
                }
                None => NO_BASE,
            };
            // Cell-local cursors: terms first, then q bytes after them.
            let (mut tile_at, mut q_at) = (0usize, cell.tile_halves * 2);
            for at in cell.records.clone() {
                let plan = &records[at];
                let movers = section.record_movers(plan.section_record);
                let probes = plan.probe_mask.count_ones() as usize;
                let base = records_word + at * RECORD_WORDS;
                words[base] = plan.probe_mask as u32;
                words[base + 1] = (plan.probe_mask >> 32) as u32;
                words[base + 2] = keys[at];
                words[base + 3] = entry as u32;
                words[base + 4] = (entry + movers.len()) as u32;
                words[base + 5] = tile_at as u32;
                words[base + 6] = index as u32;
                for &mover in movers {
                    let Some(slot) = slot_of(mover) else {
                        return fail(format!("mover {mover} has no slot"));
                    };
                    let at_entry = entries_word + entry * MOVER_ENTRY_WORDS;
                    words[at_entry] = slot;
                    words[at_entry + 1] = q_at as u32;
                    q_at += probes * MOVER_VISIBILITY_BYTES_PER_PROBE;
                    entry += 1;
                    if at < static_ends[index] {
                        static_rows[slot as usize].push(cell.cell);
                    } else {
                        animated_rows[slot as usize].push(cell.cell);
                    }
                }
                tile_at += probes * TILE_HALVES;
                if at < static_ends[index] && keys[at] != NO_LIGHT {
                    promotable.push(cell.cell);
                }
                if at >= static_ends[index] {
                    animated_cells.push(cell.cell);
                }
            }
        }
        words[cell_records_word + 2 * cells.len()] = records.len() as u32;
        words[descriptors_word..t_word].copy_from_slice(inputs.animation_descriptor_indices);
        words[t_word..payload_word].fill(NO_STATE.to_bits());
        promotable.dedup();
        animated_cells.dedup();
        Ok(Self {
            initial_words: words,
            mover_ids,
            t_word,
            bases_word,
            payload_word,
            whole_bases,
            cells,
            records,
            rows: MoverChannelRows {
                static_by_mover: MoverRows::from_sets(static_rows),
                animated_by_mover: MoverRows::from_sets(animated_rows),
                bounce_by_mover: MoverRows::default(),
                promotable,
                animated: animated_cells,
            },
            ungated_records,
        })
    }

    /// No section: the header and Pass B's descriptor map only.
    pub(in crate::render) fn empty(animation_descriptor_indices: &[u32]) -> Self {
        let cell_records_word = HEADER_WORDS;
        let descriptors_word = cell_records_word + 1;
        let mut words = vec![0u32; descriptors_word + animation_descriptor_indices.len()];
        words[1] = cell_records_word as u32;
        words[2] = cell_records_word as u32;
        words[3] = descriptors_word as u32;
        words[4] = descriptors_word as u32;
        words[5] = words.len() as u32;
        words[6] = descriptors_word as u32;
        words[7] = animation_descriptor_indices.len() as u32;
        words[8] = descriptors_word as u32;
        words[descriptors_word..].copy_from_slice(animation_descriptor_indices);
        Self {
            t_word: words.len(),
            bases_word: descriptors_word,
            payload_word: words.len(),
            whole_bases: Some(Vec::new()),
            initial_words: words,
            mover_ids: Vec::new(),
            cells: Vec::new(),
            records: Vec::new(),
            rows: MoverChannelRows::default(),
            ungated_records: 0,
        }
    }

    /// The table's words before any cell payload is written: a whole
    /// table's payload region zeroed, a pooled table's fixed words only.
    pub(in crate::render) fn initial_words(&self) -> &[u32] {
        &self.initial_words
    }

    pub(in crate::render) fn mover_ids(&self) -> &[u32] {
        &self.mover_ids
    }

    /// Byte offset of the t′ block: one f32 per mover slot.
    pub(in crate::render) fn t_byte_offset(&self) -> u64 {
        self.t_word as u64 * 4
    }

    pub(in crate::render) fn rows(&self) -> &MoverChannelRows {
        &self.rows
    }

    pub(in crate::render) fn record_count(&self) -> usize {
        self.records.len()
    }

    pub(in crate::render) fn ungated_records(&self) -> usize {
        self.ungated_records
    }

    pub(in crate::render) fn cell_count(&self) -> usize {
        self.cells.len()
    }

    /// The plan index of affinity cell `cell`, if the table carries it.
    pub(in crate::render) fn plan_of_cell(&self, cell: u32) -> Option<usize> {
        self.cells
            .binary_search_by_key(&cell, |plan| plan.cell)
            .ok()
    }

    /// Each plan's affinity cell, in plan order.
    #[cfg(test)]
    pub(super) fn cell_numbers(&self) -> Vec<u32> {
        self.cells.iter().map(|cell| cell.cell).collect()
    }

    /// Which delta families plan `plan`'s write needs: (id 41, id 45).
    pub(in crate::render) fn plan_needs(&self, plan: usize) -> (bool, bool) {
        let cell = &self.cells[plan];
        (cell.needs_static_terms, cell.needs_animated_terms)
    }

    /// Cell `plan`'s valid probes in (id 41, id 45).
    pub(in crate::render) fn plan_valid_masks(&self, plan: usize) -> (u64, u64) {
        let cell = &self.cells[plan];
        (cell.static_valid, cell.animated_valid)
    }

    /// Each cell's payload words, in plan order: what a pool places.
    pub(super) fn cell_payload_words(&self) -> Vec<u32> {
        self.cells
            .iter()
            .map(|cell| cell.payload_words() as u32)
            .collect()
    }

    /// First word of the payload region.
    pub(super) fn payload_word(&self) -> usize {
        self.payload_word
    }

    /// Byte offset of cell `plan`'s base word.
    pub(super) fn base_byte_offset(&self, plan: usize) -> u64 {
        (self.bases_word + plan) as u64 * 4
    }

    /// Cell `plan`'s fixed base word in a whole table.
    pub(super) fn whole_base(&self, plan: usize) -> Option<u32> {
        self.whole_bases.as_ref()?.get(plan).copied()
    }

    /// Cell `plan`'s write: its records' terms then their q bytes, as words.
    /// `payload` is the listed cell's payload
    /// (`MoverProbeChannelSection::cell_payload`), already checked by
    /// [`check_direct_cell`]; each family's row is needed only when a record
    /// borrows from it, and a borrowed term must be finite.
    pub(in crate::render) fn cell_write(
        &self,
        section: &MoverProbeChannelSection,
        plan: usize,
        payload: MoverCellPayload<'_>,
        static_row: Option<TermRow<'_>>,
        animated_row: Option<TermRow<'_>>,
    ) -> Result<CellWrite, MoverChannelError> {
        let cell = &self.cells[plan];
        let Some(view) = section.direct_cell_at(cell.position, payload) else {
            return fail(format!(
                "affinity cell {} holds no direct record",
                cell.cell
            ));
        };
        let wire_start = section.direct_offsets[cell.position] as usize;
        let views: Vec<DirectRecordView<'_>> = view.records().collect();
        let mut tile_halves = Vec::with_capacity(cell.tile_halves);
        let mut q_bytes = Vec::with_capacity(cell.q_bytes);
        for record in &self.records[cell.records.clone()] {
            let view = &views[record.section_record - wire_start];
            q_bytes.extend_from_slice(view.visibility);
            let mut mask = record.probe_mask;
            let mut slot = 0usize;
            while mask != 0 {
                let local = mask.trailing_zeros();
                mask &= mask - 1;
                let term = match record.term {
                    TermSource::Own => view.own_tile(slot).map(|tile| tile.to_vec()),
                    TermSource::Static(entry) => static_row
                        .and_then(|row| row.tile(entry, local))
                        .map(<[u16]>::to_vec),
                    TermSource::Animated(entry) => animated_row
                        .and_then(|row| row.tile(entry, local))
                        .map(<[u16]>::to_vec),
                };
                let Some(term) = term.filter(|term| term.len() == TILE_HALVES) else {
                    return fail(format!(
                        "affinity cell {}'s direct record {} has no term tile at local probe {local}",
                        cell.cell, record.section_record
                    ));
                };
                if let Some(bad) = first_non_finite(term.iter().copied()) {
                    return fail(format!(
                        "affinity cell {}'s direct record {} borrows a non-finite term at local probe {local} ({bad:#06x})",
                        cell.cell, record.section_record
                    ));
                }
                tile_halves.extend_from_slice(&term);
                slot += 1;
            }
        }
        debug_assert_eq!(tile_halves.len(), cell.tile_halves);
        q_bytes.resize(cell.q_bytes, 0);
        let mut words = Vec::with_capacity(cell.payload_words());
        words.extend(
            tile_halves.chunks(2).map(|pair| {
                u32::from(pair[0]) | (u32::from(pair.get(1).copied().unwrap_or(0)) << 16)
            }),
        );
        words.extend(
            q_bytes
                .as_chunks::<4>()
                .0
                .iter()
                .map(|quad| u32::from_le_bytes(*quad)),
        );
        Ok(CellWrite { plan, words })
    }

    /// Every cell `section`'s held direct payload can write now, with the
    /// whole-load delta rows `rows` supplies. A streamed payload not held, or
    /// a family row `rows` cannot give, leaves the cell for a later install.
    /// Every listed cell's held payload is checked first, gated or not.
    pub(in crate::render) fn held_writes<'a>(
        &self,
        section: &MoverProbeChannelSection,
        slices: &[MoverCellSlices],
        mut rows: impl FnMut(u32, bool) -> Option<TermRow<'a>>,
    ) -> Result<Vec<CellWrite>, MoverChannelError> {
        if !section.payload_held(MoverChannelBlock::Direct) {
            return Ok(Vec::new());
        }
        for (position, cell_slices) in slices.iter().enumerate() {
            check_direct_cell(section, position, held_direct_payload(section, cell_slices))?;
        }
        let mut writes = Vec::new();
        for (plan, cell) in self.cells.iter().enumerate() {
            let static_row = if cell.needs_static_terms {
                match rows(cell.cell, false) {
                    Some(row) => Some(row),
                    None => continue,
                }
            } else {
                None
            };
            let animated_row = if cell.needs_animated_terms {
                match rows(cell.cell, true) {
                    Some(row) => Some(row),
                    None => continue,
                }
            } else {
                None
            };
            let payload = held_direct_payload(section, &slices[cell.position]);
            writes.push(self.cell_write(section, plan, payload, static_row, animated_row)?);
        }
        Ok(writes)
    }

    /// Cell `plan`'s payload for a write: held in `section`, or from the
    /// cell's streamed id-50 row.
    pub(in crate::render) fn plan_payload<'a>(
        &self,
        section: &'a MoverProbeChannelSection,
        slices: &[MoverCellSlices],
        plan: usize,
        streamed: Option<MoverStreamedRow<'a>>,
    ) -> Result<MoverCellPayload<'a>, MoverChannelError> {
        let position = self.cells[plan].position;
        section
            .cell_payload(position, &slices[position], streamed)
            .map_err(|error| MoverChannelError(error.to_string()))
    }
}

fn as_u32(value: usize, what: &str) -> Result<u32, MoverChannelError> {
    u32::try_from(value)
        .map_err(|_| MoverChannelError(format!("the table's {what} ({value}) overflows u32")))
}

/// A listed cell's held direct payload alone. A direct write reads nothing
/// else, so a bounce block streamed beside a resident direct block, whose
/// tiles arrive with the cell's clusters, never stops the cell's write.
fn held_direct_payload<'a>(
    section: &'a MoverProbeChannelSection,
    slices: &MoverCellSlices,
) -> MoverCellPayload<'a> {
    MoverCellPayload {
        visibility: &section.visibility[slices.visibility.clone()],
        own_tiles: MoverHalves::Decoded(&section.own_tiles[slices.own_tiles.clone()]),
        bounce_tiles: MoverHalves::Decoded(&[]),
    }
}

/// Check listed cell `position`'s direct payload as it loads or arrives:
/// every term record's probe mask tight against its q bytes, and every own
/// tile finite. It covers every record, whether the table gates it or leaves
/// it ungated, so a PRL is accepted or refused alike in every mode: the
/// whole load checks every record's tightness at decode or attach.
pub(in crate::render) fn check_direct_cell(
    section: &MoverProbeChannelSection,
    position: usize,
    payload: MoverCellPayload<'_>,
) -> Result<(), MoverChannelError> {
    let Some(view) = section.direct_cell_at(position, payload) else {
        return Ok(());
    };
    let first = section.direct_offsets[position] as usize;
    for (offset, record) in view.records().enumerate() {
        let index = first + offset;
        check_tight(index, record.cell, &record)?;
        if let Some(bad) = record
            .own_tiles
            .and_then(|tiles| first_non_finite(tiles.iter()))
        {
            return fail(format!(
                "affinity cell {}'s direct record {index} holds a non-finite own tile value ({bad:#06x})",
                record.cell
            ));
        }
    }
    Ok(())
}

/// A record's probe mask must be exactly the probes where some gating mover
/// leaves q below 255 at a terminus, and each mover must store one such q.
fn check_tight(
    record: usize,
    cell: u32,
    view: &DirectRecordView<'_>,
) -> Result<(), MoverChannelError> {
    let probes = view.record.masked_probe_count();
    let expected = probes * MOVER_VISIBILITY_BYTES_PER_PROBE * view.mover_ids.len();
    if view.visibility.len() != expected {
        return fail(format!(
            "affinity cell {cell}'s direct record {record} holds {} visibility byte(s), expected {expected}",
            view.visibility.len()
        ));
    }
    let mut touched = 0u64;
    for mover in view.movers() {
        let mut stores = false;
        for slot in 0..probes {
            if mover.q(slot).iter().any(|&q| q < u8::MAX) {
                touched |= 1 << slot;
                stores = true;
            }
        }
        if !stores {
            return fail(format!(
                "affinity cell {cell}'s direct record {record} lists mover {} with no q below 255",
                mover.mover_id
            ));
        }
    }
    let all = if probes == 64 {
        u64::MAX
    } else {
        (1u64 << probes) - 1
    };
    if touched != all {
        return fail(format!(
            "affinity cell {cell}'s direct record {record} has a probe mask that is not tight"
        ));
    }
    Ok(())
}
