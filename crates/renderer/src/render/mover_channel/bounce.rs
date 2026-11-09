// Mover probe channel bounce table: id 57's bounce block laid out for the
// indirect SH compose, which binds it at binding 25 in place of a separate
// light → descriptor map (id 27's map folds in, as id 45's folds into the
// direct table). Per listed cell: its bounce entries (mover slot, one stored
// mask per terminus, first tile half), and one payload run of the stored
// S^b₀/S^b₁ tiles its base word points at: fixed in a whole table, placed
// per resident cell in a pooled one. wgpu-free; `gpu.rs` owns the buffer.
// See: context/lib/rendering_pipeline.md §4 (Mover state lighting)
//
// The masks are storage sparsity only: an unmasked probe composes id 34
// exactly as with no section, and an affected probe's unstored terminus
// composes id 34's texel there, so the lerp runs toward id 34 by the mover's
// t′ alone. Mover state blends absolute bounce states; it never flags,
// zeroes or excludes a probe.

use postretro_level_format::mover_probe_channel::{
    MOVER_IRRADIANCE_TILE_F16_COUNT, MoverCellPayload, MoverProbeChannelSection,
};

use super::layout::{CellWrite, MoverChannelError, MoverRows, NO_BASE, NO_STATE, first_non_finite};

/// Header words, in `mover_bounce.wgsl`'s `MB_HEADER_*` order. Id 27's
/// descriptor map starts right after it, so the compose reads a light's
/// descriptor at a fixed offset.
pub(super) const BOUNCE_HEADER_WORDS: usize = 8;
/// Words per bounce entry: mover slot, the S^b₀ mask (two words), the S^b₁
/// mask (two words), first tile half within the cell's payload.
const ENTRY_WORDS: usize = 6;
/// f16 halves per stored (probe, terminus): one tile.
const TILE_HALVES: usize = MOVER_IRRADIANCE_TILE_F16_COUNT;

#[derive(Clone, Debug)]
struct BounceCellPlan {
    cell: u32,
    /// Position in the section's `cells`.
    position: usize,
    /// f16 halves of the cell's tiles (a multiple of `TILE_HALVES`, so even).
    halves: usize,
}

/// The laid-out bounce table.
#[derive(Debug, Clone)]
pub(in crate::render) struct BounceLayout {
    initial_words: Vec<u32>,
    t_word: usize,
    bases_word: usize,
    payload_word: usize,
    /// Each cell's payload at a fixed base (`None`: pooled per resident cell).
    whole_bases: Option<Vec<u32>>,
    cells: Vec<BounceCellPlan>,
    rows_by_mover: MoverRows,
}

impl BounceLayout {
    /// Lay out `section`'s bounce block over the shared mover slots
    /// (`mover_ids`, ascending), after id 27's `descriptor_indices`.
    pub(in crate::render) fn build(
        section: Option<&MoverProbeChannelSection>,
        descriptor_indices: &[u32],
        mover_ids: &[u32],
        pooled: bool,
    ) -> Result<Self, MoverChannelError> {
        let mut cells = Vec::new();
        let mut entry_count = 0usize;
        if let Some(section) = section {
            for (position, listed) in section.cells.iter().enumerate() {
                let entries = section.bounce_offsets[position] as usize
                    ..section.bounce_offsets[position + 1] as usize;
                if entries.is_empty() {
                    continue;
                }
                entry_count += entries.len();
                let halves = section.bounce_entries[entries]
                    .iter()
                    .map(|entry| entry.stored_tile_count() * TILE_HALVES)
                    .sum();
                cells.push(BounceCellPlan {
                    cell: listed.cell,
                    position,
                    halves,
                });
            }
        }

        let descriptors_word = BOUNCE_HEADER_WORDS;
        let cells_word = descriptors_word + descriptor_indices.len();
        let cell_entries_word = cells_word + cells.len();
        let bases_word = cell_entries_word + cells.len() + 1;
        let entries_word = bases_word + cells.len();
        let t_word = entries_word + entry_count * ENTRY_WORDS;
        let payload_word = t_word + mover_ids.len();
        let whole_payload: usize = cells.iter().map(|cell| cell.halves / 2).sum();
        let total_words = payload_word + if pooled { 0 } else { whole_payload };
        u32::try_from((payload_word + whole_payload) * 4).map_err(|_| {
            MoverChannelError(format!(
                "the bounce table's byte size with every cell resident ({}) overflows u32",
                (payload_word + whole_payload) * 4
            ))
        })?;

        let mut words = vec![0u32; total_words];
        words[0] = descriptor_indices.len() as u32;
        words[1] = cells.len() as u32;
        words[2] = cells_word as u32;
        words[3] = cell_entries_word as u32;
        words[4] = bases_word as u32;
        words[5] = entries_word as u32;
        words[6] = t_word as u32;
        words[descriptors_word..cells_word].copy_from_slice(descriptor_indices);
        let mut whole_bases = (!pooled).then(|| Vec::with_capacity(cells.len()));
        let mut rows = vec![Vec::new(); mover_ids.len()];
        let mut entry = 0usize;
        let mut next_base = payload_word;
        if let Some(section) = section {
            for (index, plan) in cells.iter().enumerate() {
                words[cells_word + index] = plan.cell;
                words[cell_entries_word + index] = entry as u32;
                words[bases_word + index] = match whole_bases.as_mut() {
                    Some(bases) => {
                        let base = next_base as u32;
                        bases.push(base);
                        next_base += plan.halves / 2;
                        base
                    }
                    None => NO_BASE,
                };
                let mut half = 0usize;
                for bounce in &section.bounce_entries[section.bounce_offsets[plan.position] as usize
                    ..section.bounce_offsets[plan.position + 1] as usize]
                {
                    let Ok(slot) = mover_ids.binary_search(&bounce.mover_id) else {
                        return Err(MoverChannelError(format!(
                            "bounce entry mover {} has no slot",
                            bounce.mover_id
                        )));
                    };
                    let at = entries_word + entry * ENTRY_WORDS;
                    let [m0, m1] = bounce.stored_probe_masks;
                    words[at] = slot as u32;
                    words[at + 1] = m0 as u32;
                    words[at + 2] = (m0 >> 32) as u32;
                    words[at + 3] = m1 as u32;
                    words[at + 4] = (m1 >> 32) as u32;
                    words[at + 5] = half as u32;
                    half += bounce.stored_tile_count() * TILE_HALVES;
                    rows[slot].push(plan.cell);
                    entry += 1;
                }
            }
        }
        words[cell_entries_word + cells.len()] = entry as u32;
        words[t_word..payload_word].fill(NO_STATE.to_bits());
        Ok(Self {
            initial_words: words,
            t_word,
            bases_word,
            payload_word,
            whole_bases,
            cells,
            rows_by_mover: MoverRows::from_sets(rows),
        })
    }

    pub(in crate::render) fn initial_words(&self) -> &[u32] {
        &self.initial_words
    }

    pub(super) fn t_byte_offset(&self) -> u64 {
        self.t_word as u64 * 4
    }

    pub(super) fn rows_by_mover(&self) -> &MoverRows {
        &self.rows_by_mover
    }

    pub(in crate::render) fn cell_count(&self) -> usize {
        self.cells.len()
    }

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

    pub(super) fn cell_payload_words(&self) -> Vec<u32> {
        self.cells
            .iter()
            .map(|cell| (cell.halves / 2) as u32)
            .collect()
    }

    pub(super) fn payload_word(&self) -> usize {
        self.payload_word
    }

    pub(super) fn base_byte_offset(&self, plan: usize) -> u64 {
        (self.bases_word + plan) as u64 * 4
    }

    pub(super) fn whole_base(&self, plan: usize) -> Option<u32> {
        self.whole_bases.as_ref()?.get(plan).copied()
    }

    pub(super) fn position(&self, plan: usize) -> usize {
        self.cells[plan].position
    }

    /// Cell `plan`'s payload words from `payload`. The bytes are id 57's
    /// tiles, checked as they arrive: the run must cover every entry (the
    /// payload view checked its length) and hold only finite radiance.
    pub(in crate::render) fn cell_write(
        &self,
        section: &MoverProbeChannelSection,
        plan: usize,
        payload: MoverCellPayload<'_>,
    ) -> Result<CellWrite, MoverChannelError> {
        let cell = &self.cells[plan];
        if payload.bounce_tiles.len() != cell.halves {
            return Err(MoverChannelError(format!(
                "affinity cell {}'s bounce payload holds {} half(s), expected {}",
                cell.cell,
                payload.bounce_tiles.len(),
                cell.halves
            )));
        }
        let mut words = Vec::with_capacity(cell.halves / 2);
        for entry in section.bounce_entries_at(cell.position, payload) {
            let tiles = entry.tiles.ok_or_else(|| {
                MoverChannelError(format!(
                    "affinity cell {}'s bounce payload is not resident",
                    cell.cell
                ))
            })?;
            let halves = tiles.to_vec();
            if let Some(bad) = first_non_finite(halves.iter().copied()) {
                return Err(MoverChannelError(format!(
                    "affinity cell {}'s bounce state for mover {} holds a non-finite value ({bad:#06x})",
                    cell.cell, entry.entry.mover_id
                )));
            }
            words.extend(
                halves
                    .as_chunks::<2>()
                    .0
                    .iter()
                    .map(|pair| u32::from(pair[0]) | (u32::from(pair[1]) << 16)),
            );
        }
        Ok(CellWrite { plan, words })
    }
}
