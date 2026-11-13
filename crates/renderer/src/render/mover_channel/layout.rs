// Mover probe channel table: id 57's direct block laid out for the two
// direct-SH compose passes, each gated light's term T resolved once per
// masked probe, and the per-mover row lists the streamed planner recomposes
// (`rows.rs`).
// Each listed cell's payload is one run of words its base word points at:
// fixed in a whole table, placed per resident cell in a pooled one.
// wgpu-free; `gpu.rs` owns the buffer this lays out.
// See: context/lib/rendering_pipeline.md §4 (Mover state lighting)

use std::collections::HashMap;
use std::ops::Range;

pub(in crate::render) use super::cell_writes::CellWrite;
pub(in crate::render) use super::direct_checks::check_direct_cell;
use super::knots::MoverKnots;
#[cfg(test)]
pub(in crate::render) use super::rows::NO_MOVER_ROWS;
pub(in crate::render) use super::rows::{MoverChannelRows, MoverRows};
use super::state::{T_SLOT_WORDS, write_no_state};
pub(in crate::render) use super::terms::{TermFamily, TermRow, WholeTermRows};
use postretro_level_format::mover_probe_channel::{
    MOVER_IRRADIANCE_TILE_F16_COUNT, MoverLightNamespace, MoverProbeChannelSection,
};

/// Header words, in `mover_channel.wgsl`'s `MC_HEADER_*` order.
const HEADER_WORDS: usize = 10;
/// Words per term record (`MC_RECORD_WORDS`): probe mask (two words), light
/// key, mover entry range, first term half within the cell's payload, and
/// the cell's listed position.
const RECORD_WORDS: usize = 8;
/// Words per (record, mover) entry: mover slot and its first q byte within
/// the cell's payload. The mover's N q bytes per masked probe follow
/// probe-major, as id 57 stores them; its knot run gives N.
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

pub(super) fn fail<T>(message: String) -> Result<T, MoverChannelError> {
    Err(MoverChannelError(message))
}

/// Where a GPU record's term T comes from.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum TermSource {
    /// The record's own tile, from id 57's payload.
    Own,
    /// Cell-local entry of id 41 (the light's selection slot).
    Static(usize),
    /// Cell-local entry of id 45.
    Animated(usize),
}

#[derive(Clone, Debug)]
pub(super) struct RecordPlan {
    /// Index into the section's `direct_records`.
    pub(super) section_record: usize,
    pub(super) term: TermSource,
    pub(super) probe_mask: u64,
}

/// One listed cell the table carries records for. Its payload is one run of
/// words: its records' terms (f16 halves), then their q bytes, so the whole
/// cell sits wherever its base word points.
#[derive(Clone, Debug)]
pub(super) struct CellPlan {
    pub(super) cell: u32,
    /// Position in the section's `cells`.
    pub(super) position: usize,
    /// Its records in `MoverChannelLayout::records`: static lights first.
    pub(super) records: Range<usize>,
    /// f16 halves of the term run (even, so word aligned).
    pub(super) tile_halves: usize,
    /// Bytes of the visibility run, padded to a word.
    pub(super) q_bytes: usize,
    pub(super) needs_static_terms: bool,
    pub(super) needs_animated_terms: bool,
    /// The cell's valid probes in ids 41 and 45, which an L0 cell keeps in
    /// order: a streamed row's tile rank.
    static_valid: u64,
    animated_valid: u64,
}

impl CellPlan {
    pub(super) fn payload_words(&self) -> usize {
        self.tile_halves / 2 + self.q_bytes / 4
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

impl<'a> MoverChannelInputs<'a> {
    /// Id 41 as the table reads it. The loader disables id 41 by clearing
    /// the id-40 selection. A whole load then drops id 41 as well, but a
    /// streamed load still lists its metadata; either way a record borrowing
    /// from id 41 composes ungated and the table reads no id-41 term, so the
    /// same PRL composes, and is ledgered, alike in every mode.
    pub(in crate::render) fn live_static_terms(&self) -> Option<TermFamily<'a>> {
        self.static_terms
            .filter(|_| !self.entity_shadow_lights.is_empty())
    }
}

/// The laid-out table and what writes and plans against it.
#[derive(Debug, Clone)]
pub(in crate::render) struct MoverChannelLayout {
    initial_words: Vec<u32>,
    /// Slot → id 43 `mover_id`, ascending.
    mover_ids: Vec<u32>,
    /// Each slot's knots through its ramp, in the table's knot block too.
    knots: MoverKnots,
    t_word: usize,
    /// First word of each cell's base word.
    bases_word: usize,
    /// First word of the payload region: every cell's payload in a whole
    /// table, the pool in a pooled one.
    payload_word: usize,
    /// Each cell's payload at a fixed base (`None`: pooled per resident cell).
    whole_bases: Option<Vec<u32>>,
    pub(super) cells: Vec<CellPlan>,
    pub(super) records: Vec<RecordPlan>,
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
        let static_terms = inputs.live_static_terms();
        let selection: HashMap<u32, u32> = inputs
            .entity_shadow_lights
            .iter()
            .enumerate()
            .map(|(slot, &light)| (light, slot as u32))
            .collect();
        let mover_ids = section.mover_ids();
        let knots = MoverKnots::new(Some(section), &mover_ids)?;
        let slot_of = |mover: u32| mover_ids.binary_search(&mover).ok().map(|slot| slot as u32);
        let poses_of =
            |mover: u32| slot_of(mover).map_or(0, |slot| knots.pose_count(slot as usize));

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
                    q_bytes += section
                        .record_movers(index)
                        .iter()
                        .map(|&mover| probes * poses_of(mover))
                        .sum::<usize>();
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
        let knots_word = descriptors_word + inputs.animation_descriptor_indices.len();
        let t_word = knots_word + knots.table_word_count();
        let payload_word = t_word + T_SLOT_WORDS * mover_ids.len();
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
        words[9] = knots_word as u32;
        knots.write_table(&mut words, knots_word);
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
                    q_at += probes * knots.pose_count(slot as usize);
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
        words[descriptors_word..knots_word].copy_from_slice(inputs.animation_descriptor_indices);
        write_no_state(&mut words[t_word..payload_word]);
        promotable.dedup();
        animated_cells.dedup();
        Ok(Self {
            initial_words: words,
            mover_ids,
            knots,
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
        words[9] = words.len() as u32;
        words[descriptors_word..].copy_from_slice(animation_descriptor_indices);
        Self {
            knots: MoverKnots::default(),
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

    /// Each slot's knots through its ramp: the bounce table and the depth
    /// live tiles read the same.
    pub(super) fn knots(&self) -> &MoverKnots {
        &self.knots
    }

    /// Byte offset of the t′ block: per mover slot, its t′ then its knot
    /// bracket ([`T_SLOT_WORDS`]).
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
}

fn as_u32(value: usize, what: &str) -> Result<u32, MoverChannelError> {
    u32::try_from(value)
        .map_err(|_| MoverChannelError(format!("the table's {what} ({value}) overflows u32")))
}
