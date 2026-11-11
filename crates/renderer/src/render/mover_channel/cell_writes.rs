// Mover probe channel direct table cell writes: one listed cell's payload
// words (its records' terms, then their q bytes) from the section's held
// payload or the cell's streamed id-50 row, ready to place.
// See: context/lib/rendering_pipeline.md §4 (Mover state lighting)

use postretro_level_format::mover_probe_channel::{
    DirectRecordView, MOVER_IRRADIANCE_TILE_F16_COUNT as TILE_HALVES, MoverCellPayload,
    MoverCellSlices, MoverChannelBlock, MoverHalves, MoverProbeChannelSection, MoverStreamedRow,
};

use super::direct_checks::check_direct_cell;
use super::layout::{MoverChannelError, MoverChannelLayout, TermSource, fail, first_non_finite};
use super::terms::TermRow;

/// One listed cell's payload words, ready to place: its records' terms, then
/// their q bytes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(in crate::render) struct CellWrite {
    pub(in crate::render) plan: usize,
    pub(in crate::render) words: Vec<u32>,
}

impl MoverChannelLayout {
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
            // Every pose's q, probe-major, as the section stores them.
            for mover in view.movers() {
                q_bytes.extend_from_slice(mover.visibility);
            }
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

    /// Cell `plan`'s direct payload for a write: held in `section`, or from
    /// the cell's streamed id-50 row. A held direct payload reads nothing
    /// else, so a bounce block streaming beside it, its tiles absent from
    /// `streamed` or never brought, never stops the write.
    pub(in crate::render) fn plan_direct_payload<'a>(
        &self,
        section: &'a MoverProbeChannelSection,
        slices: &[MoverCellSlices],
        plan: usize,
        streamed: Option<MoverStreamedRow<'a>>,
    ) -> Result<MoverCellPayload<'a>, MoverChannelError> {
        let position = self.cells[plan].position;
        if section.payload_held(MoverChannelBlock::Direct) {
            return Ok(held_direct_payload(section, &slices[position]));
        }
        section
            .cell_payload(position, &slices[position], streamed)
            .map_err(|error| MoverChannelError(error.to_string()))
    }
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
