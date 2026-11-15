// Mover probe channel bounce table: id 57's bounce block laid out for the
// indirect SH compose, which binds it at binding 25 in place of a separate
// light → descriptor map (id 27's map folds in, as id 45's folds into the
// direct table). Per listed cell: its bounce entries (mover slot, its per-pose
// masks, the probes any read pose stores, first tile half), each mover's
// knots through its ramp, and one payload run of the stored tiles its base
// word points at: fixed in a whole table, placed per resident cell in a
// pooled one. Tiles keep id 57's order, pose-major then probe by stored rank,
// and each pose's first tile rank is expanded at install, so the compose
// finds a probe's two states once per (entry, probe), never per texel.
// wgpu-free; `gpu.rs` owns the buffer.
// See: context/lib/rendering_pipeline.md §4 (Mover state lighting)
//
// The masks are storage sparsity only: an unmasked probe composes id 34
// exactly as with no section, and a pose an affected probe stores no state
// for composes id 34's texel there (a terminus, or a mid pose marked base)
// or blends across (an unmarked mid pose), so the blend runs between
// absolute states by the mover's t′ alone. Mover state blends absolute
// bounce states; it never flags, zeroes or excludes a probe.

use postretro_level_format::mover_probe_channel::{
    MOVER_IRRADIANCE_TILE_F16_COUNT, MoverBounceEntry, MoverCellPayload, MoverProbeChannelSection,
};

use super::knots::MoverKnots;
use super::layout::{CellWrite, MoverChannelError, MoverRows, NO_BASE, first_non_finite};
use super::state::{T_SLOT_WORDS, write_no_state};

/// Header words, in `mover_bounce.wgsl`'s `MB_HEADER_*` order. Id 27's
/// descriptor map starts right after it, so the compose reads a light's
/// descriptor at a fixed offset.
pub(super) const BOUNCE_HEADER_WORDS: usize = 8;
/// Words per bounce entry (`MB_ENTRY_WORDS`): mover slot, the word of its
/// pose block, the probes some pose compose reads stores a state for (two
/// words), first tile half within the cell's payload.
const ENTRY_WORDS: usize = 5;
/// Words per (entry, pose) (`MB_POSE_WORDS`): the stored mask (two words),
/// the base mask (two words), the pose's first tile rank in the entry.
const POSE_WORDS: usize = 5;
/// f16 halves per stored (probe, pose): one tile.
const TILE_HALVES: usize = MOVER_IRRADIANCE_TILE_F16_COUNT;

#[derive(Clone, Debug)]
struct BounceCellPlan {
    cell: u32,
    /// Position in the section's `cells`.
    position: usize,
    /// f16 halves of the cell's tiles, every pose's (a multiple of
    /// `TILE_HALVES`, so even).
    halves: usize,
}

/// Probes `entry` stores a state for at a pose compose reads: its slot's
/// effective knots. A pose the ramp squeezes onto t′ 0 or 1 is never read.
fn read_stored_mask(entry: &MoverBounceEntry, knots: &MoverKnots, slot: usize) -> u64 {
    knots
        .slot(slot)
        .iter()
        .filter_map(|knot| entry.poses.get(knot.pose as usize))
        .fold(0, |mask, pose| mask | pose.stored)
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
    /// (`mover_ids`, ascending, with their `knots`), after id 27's
    /// `descriptor_indices`.
    pub(in crate::render) fn build(
        section: Option<&MoverProbeChannelSection>,
        descriptor_indices: &[u32],
        mover_ids: &[u32],
        knots: &MoverKnots,
        pooled: bool,
    ) -> Result<Self, MoverChannelError> {
        let mut cells = Vec::new();
        let mut entry_count = 0usize;
        let mut pose_count = 0usize;
        if let Some(section) = section {
            for (position, listed) in section.cells.iter().enumerate() {
                let entries = section.bounce_offsets[position] as usize
                    ..section.bounce_offsets[position + 1] as usize;
                if entries.is_empty() {
                    continue;
                }
                entry_count += entries.len();
                let entries = &section.bounce_entries[entries];
                pose_count += entries
                    .iter()
                    .map(MoverBounceEntry::pose_count)
                    .sum::<usize>();
                cells.push(BounceCellPlan {
                    cell: listed.cell,
                    position,
                    halves: entries
                        .iter()
                        .map(|entry| entry.stored_tile_count() * TILE_HALVES)
                        .sum(),
                });
            }
        }

        let descriptors_word = BOUNCE_HEADER_WORDS;
        let cells_word = descriptors_word + descriptor_indices.len();
        let cell_entries_word = cells_word + cells.len();
        let bases_word = cell_entries_word + cells.len() + 1;
        let entries_word = bases_word + cells.len();
        let poses_word = entries_word + entry_count * ENTRY_WORDS;
        let knots_word = poses_word + pose_count * POSE_WORDS;
        let t_word = knots_word + knots.table_word_count();
        let payload_word = t_word + T_SLOT_WORDS * mover_ids.len();
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
        words[7] = knots_word as u32;
        words[descriptors_word..cells_word].copy_from_slice(descriptor_indices);
        knots.write_table(&mut words, knots_word);
        let mut whole_bases = (!pooled).then(|| Vec::with_capacity(cells.len()));
        let mut rows = vec![Vec::new(); mover_ids.len()];
        let mut entry = 0usize;
        let mut pose_at = poses_word;
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
                    let read = read_stored_mask(bounce, knots, slot);
                    words[at] = slot as u32;
                    words[at + 1] = pose_at as u32;
                    words[at + 2] = read as u32;
                    words[at + 3] = (read >> 32) as u32;
                    words[at + 4] = half as u32;
                    let mut rank = 0u32;
                    for pose in &bounce.poses {
                        words[pose_at] = pose.stored as u32;
                        words[pose_at + 1] = (pose.stored >> 32) as u32;
                        words[pose_at + 2] = pose.base as u32;
                        words[pose_at + 3] = (pose.base >> 32) as u32;
                        words[pose_at + 4] = rank;
                        rank += pose.stored.count_ones();
                        pose_at += POSE_WORDS;
                    }
                    half += bounce.stored_tile_count() * TILE_HALVES;
                    rows[slot].push(plan.cell);
                    entry += 1;
                }
            }
        }
        words[cell_entries_word + cells.len()] = entry as u32;
        write_no_state(&mut words[t_word..payload_word]);
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

    /// No level: the header alone.
    pub(in crate::render) fn empty() -> Self {
        Self::build(None, &[], &[], &MoverKnots::default(), false)
            .expect("an empty bounce table lays out")
    }

    pub(in crate::render) fn initial_words(&self) -> &[u32] {
        &self.initial_words
    }

    /// Byte offset of the t′ block: per mover slot, its t′ then its knot
    /// bracket ([`T_SLOT_WORDS`]).
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
    /// tiles in id 57's order, checked as they arrive: the run must cover
    /// every entry and hold only finite radiance at every pose.
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
            if let Some(bad) = first_non_finite(tiles.iter()) {
                return Err(MoverChannelError(format!(
                    "affinity cell {}'s bounce state for mover {} holds a non-finite value ({bad:#06x})",
                    cell.cell, entry.entry.mover_id
                )));
            }
            // Whole tiles of 108 halves: always an even count.
            let mut halves = tiles.iter();
            while let (Some(low), Some(high)) = (halves.next(), halves.next()) {
                words.push(u32::from(low) | (u32::from(high) << 16));
            }
        }
        debug_assert_eq!(words.len(), cell.halves / 2);
        Ok(CellWrite { plan, words })
    }
}
