// MoverProbeChannel (id 57) payload placed with id 50's cells: each listed
// cell's slice of the direct and bounce payloads, the row block an id-50
// chunk carries for the cells it covers, and the attach that gathers a
// streamed payload back into the section for a whole-load.
// See: context/lib/build_pipeline.md §PRL section IDs

use std::collections::BTreeMap;
use std::ops::Range;

use super::views::record_payload_len;
use super::wire::{check_masks_are_tight, put_halves};
use super::{
    MoverChannelBlock, MoverPayloadPlacement, MoverProbeChannelError, MoverProbeChannelSection,
    bounce_entry_f16_count,
};

type Error = MoverProbeChannelError;

/// Row block header: `u32` row count, `u32` reserved 0.
pub const MOVER_STREAMED_BLOCK_HEADER_BYTES: usize = 8;
/// Per row: `u32` dense affinity cell, `u32` direct bytes, `u32` bounce
/// bytes, `u32` reserved 0.
pub const MOVER_STREAMED_ROW_BYTES: usize = 16;

/// Where one listed cell's payload sits in the section's payload vectors.
/// The ranges come from the section's structure, so they hold whichever
/// placement each block has.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MoverCellSlices {
    /// Bytes of [`MoverProbeChannelSection::visibility`].
    pub visibility: Range<usize>,
    /// Halves of [`MoverProbeChannelSection::own_tiles`].
    pub own_tiles: Range<usize>,
    /// Halves of [`MoverProbeChannelSection::bounce_tiles`].
    pub bounce_tiles: Range<usize>,
}

/// Bytes one listed cell's id-50 row carries per block: the block's slice
/// when the block streams, else 0.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct MoverStreamedRowLens {
    pub direct: u32,
    pub bounce: u32,
}

impl MoverStreamedRowLens {
    pub fn total(&self) -> u64 {
        u64::from(self.direct) + u64::from(self.bounce)
    }
}

impl MoverCellSlices {
    /// The cell's row lengths for `section`'s placements.
    pub fn row_lens(&self, section: &MoverProbeChannelSection) -> MoverStreamedRowLens {
        let streamed = |block| section.placement(block) == MoverPayloadPlacement::Streamed;
        let bytes = |len: usize| u32::try_from(len).expect("one cell's payload fits u32");
        MoverStreamedRowLens {
            direct: if streamed(MoverChannelBlock::Direct) {
                bytes(self.visibility.len() + self.own_tiles.len() * 2)
            } else {
                0
            },
            bounce: if streamed(MoverChannelBlock::Bounce) {
                bytes(self.bounce_tiles.len() * 2)
            } else {
                0
            },
        }
    }
}

/// One listed cell's streamed payload as an id-50 chunk carries it: the
/// direct payload (the cell's visibility, then its own tiles as
/// little-endian halves) and the bounce payload (its tiles as little-endian
/// halves). A block that stays resident carries no bytes here.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MoverStreamedRow<'a> {
    /// Dense affinity cell index.
    pub cell: u32,
    pub direct: &'a [u8],
    pub bounce: &'a [u8],
}

impl MoverProbeChannelSection {
    /// Each listed cell's payload slices, in listed order.
    pub fn cell_slices(&self) -> Vec<MoverCellSlices> {
        let (mut visibility, mut tiles, mut bounce) = (0usize, 0usize, 0usize);
        (0..self.cells.len())
            .map(|position| {
                let start = (visibility, tiles, bounce);
                for index in self.direct_offsets[position] as usize
                    ..self.direct_offsets[position + 1] as usize
                {
                    let (visibility_len, tile_len) = record_payload_len(self, index);
                    visibility += visibility_len;
                    tiles += tile_len;
                }
                bounce += self.bounce_entries[self.bounce_offsets[position] as usize
                    ..self.bounce_offsets[position + 1] as usize]
                    .iter()
                    .map(bounce_entry_f16_count)
                    .sum::<usize>();
                MoverCellSlices {
                    visibility: start.0..visibility,
                    own_tiles: start.1..tiles,
                    bounce_tiles: start.2..bounce,
                }
            })
            .collect()
    }

    /// The id-50 rows for `cells` (dense affinity indices, ascending): each
    /// listed cell among them whose streamed blocks carry a byte, in
    /// ascending order, with its lengths.
    pub fn streamed_rows_for(
        &self,
        slices: &[MoverCellSlices],
        cells: impl IntoIterator<Item = u32>,
    ) -> Vec<(u32, MoverStreamedRowLens)> {
        cells
            .into_iter()
            .filter_map(|cell| {
                let position = self.listed_position(cell as usize)?;
                let lens = slices[position].row_lens(self);
                (lens.total() > 0).then_some((cell, lens))
            })
            .collect()
    }

    /// The row block an id-50 chunk carries for `rows` ([`Self::streamed_rows_for`]),
    /// from the payload the section holds (as the compiler builds it).
    pub fn encode_streamed_rows(
        &self,
        slices: &[MoverCellSlices],
        rows: &[(u32, MoverStreamedRowLens)],
    ) -> Vec<u8> {
        debug_assert!(
            MoverChannelBlock::BOTH
                .iter()
                .all(|&block| self.payload_held(block)),
            "a streamed payload is encoded from the section that holds it"
        );
        let payload: u64 = rows.iter().map(|(_, lens)| lens.total()).sum();
        let mut out = Vec::with_capacity(
            MOVER_STREAMED_BLOCK_HEADER_BYTES
                + rows.len() * MOVER_STREAMED_ROW_BYTES
                + payload as usize,
        );
        out.extend_from_slice(&(rows.len() as u32).to_le_bytes());
        out.extend_from_slice(&0u32.to_le_bytes());
        for (cell, lens) in rows {
            out.extend_from_slice(&cell.to_le_bytes());
            out.extend_from_slice(&lens.direct.to_le_bytes());
            out.extend_from_slice(&lens.bounce.to_le_bytes());
            out.extend_from_slice(&0u32.to_le_bytes());
        }
        for (cell, lens) in rows {
            let position = self
                .listed_position(*cell as usize)
                .expect("a streamed row names a listed cell");
            let slices = &slices[position];
            if lens.direct > 0 {
                out.extend_from_slice(&self.visibility[slices.visibility.clone()]);
                put_halves(&mut out, &self.own_tiles[slices.own_tiles.clone()]);
            }
            if lens.bounce > 0 {
                put_halves(&mut out, &self.bounce_tiles[slices.bounce_tiles.clone()]);
            }
        }
        debug_assert_eq!(out.len(), streamed_block_len(rows));
        out
    }

    /// Gather a streamed payload back into the section from the rows of
    /// every id-50 chunk (the whole-load's read): a cell several clusters
    /// cover may repeat, and its copies must agree. Every listed cell whose
    /// streamed blocks carry a byte needs exactly its row's lengths, and no
    /// row may name another cell. The direct payload's masks are then
    /// checked tight, as a resident one's are at decode. Placement stays
    /// streamed: the payload is the same bytes, now held.
    pub fn attach_streamed_payload<'a>(
        &mut self,
        rows: impl IntoIterator<Item = MoverStreamedRow<'a>>,
    ) -> Result<(), Error> {
        let slices = self.cell_slices();
        let mut by_cell: BTreeMap<u32, MoverStreamedRow<'a>> = BTreeMap::new();
        for row in rows {
            let Some(position) = self.listed_position(row.cell as usize) else {
                return Err(Error::StreamedRows {
                    detail: format!(
                        "names affinity cell {}, which the section does not list",
                        row.cell
                    ),
                });
            };
            let lens = slices[position].row_lens(self);
            for (block, bytes, expected) in [
                (MoverChannelBlock::Direct, row.direct, lens.direct),
                (MoverChannelBlock::Bounce, row.bounce, lens.bounce),
            ] {
                if bytes.len() != expected as usize {
                    return Err(Error::StreamedPayloadMismatch {
                        block,
                        cell: row.cell,
                        detail: format!("holds {} byte(s), expected {expected}", bytes.len()),
                    });
                }
            }
            if let Some(previous) = by_cell.insert(row.cell, row)
                && previous != row
            {
                let block = if previous.direct != row.direct {
                    MoverChannelBlock::Direct
                } else {
                    MoverChannelBlock::Bounce
                };
                return Err(Error::StreamedPayloadMismatch {
                    block,
                    cell: row.cell,
                    detail: "differs between two clusters that carry it".into(),
                });
            }
        }

        let direct = !self.payload_held(MoverChannelBlock::Direct);
        let bounce = !self.payload_held(MoverChannelBlock::Bounce);
        let (mut visibility, mut own_tiles, mut bounce_tiles) =
            (Vec::new(), Vec::new(), Vec::new());
        for (position, slices) in slices.iter().enumerate() {
            let cell = self.cells[position].cell;
            let lens = slices.row_lens(self);
            if lens.total() == 0 {
                continue;
            }
            let Some(row) = by_cell.get(&cell) else {
                let block = if lens.direct > 0 {
                    MoverChannelBlock::Direct
                } else {
                    MoverChannelBlock::Bounce
                };
                return Err(Error::StreamedPayloadMismatch {
                    block,
                    cell,
                    detail: "is in no id-50 chunk".into(),
                });
            };
            if direct {
                let (cell_visibility, tiles) = row.direct.split_at(slices.visibility.len());
                visibility.extend_from_slice(cell_visibility);
                own_tiles.extend(halves(tiles));
            }
            if bounce {
                bounce_tiles.extend(halves(row.bounce));
            }
        }
        if direct {
            check_masks_are_tight(
                &self.cells,
                &self.direct_offsets,
                &self.direct_records,
                &self.mover_offsets,
                &self.direct_movers,
                &visibility,
            )?;
            self.visibility = visibility;
            self.own_tiles = own_tiles;
        }
        if bounce {
            self.bounce_tiles = bounce_tiles;
        }
        Ok(())
    }
}

/// Body bytes of a row block over `rows`.
pub(crate) fn streamed_block_len(rows: &[(u32, MoverStreamedRowLens)]) -> usize {
    MOVER_STREAMED_BLOCK_HEADER_BYTES
        + rows.len() * MOVER_STREAMED_ROW_BYTES
        + rows
            .iter()
            .map(|(_, lens)| lens.total() as usize)
            .sum::<usize>()
}

/// A row block's rows: the header, each row's reserved word and the payload
/// length checked; what the rows name is checked against the section by
/// [`MoverProbeChannelSection::attach_streamed_payload`] or
/// [`MoverProbeChannelSection::cell_payload`].
pub fn parse_streamed_rows(bytes: &[u8]) -> Result<Vec<MoverStreamedRow<'_>>, Error> {
    let fail = |detail: String| Error::StreamedRows { detail };
    if bytes.len() < MOVER_STREAMED_BLOCK_HEADER_BYTES {
        return Err(fail(format!(
            "of {} byte(s) is shorter than its header",
            bytes.len()
        )));
    }
    let word = |at: usize| u32::from_le_bytes(bytes[at..at + 4].try_into().expect("four bytes"));
    let count = word(0) as usize;
    if word(4) != 0 {
        return Err(fail("sets its reserved header word".into()));
    }
    let table_end = count
        .checked_mul(MOVER_STREAMED_ROW_BYTES)
        .and_then(|table| table.checked_add(MOVER_STREAMED_BLOCK_HEADER_BYTES))
        .filter(|&end| end <= bytes.len())
        .ok_or_else(|| {
            fail(format!(
                "of {} byte(s) cannot hold {count} row(s)",
                bytes.len()
            ))
        })?;
    let mut rows = Vec::with_capacity(count);
    let mut at = table_end;
    let mut previous = None;
    for row in 0..count {
        let base = MOVER_STREAMED_BLOCK_HEADER_BYTES + row * MOVER_STREAMED_ROW_BYTES;
        let (cell, direct, bounce) = (word(base), word(base + 4) as usize, word(base + 8) as usize);
        if word(base + 12) != 0 {
            return Err(fail(format!("row {row} sets its reserved word")));
        }
        if previous.is_some_and(|previous| previous >= cell) {
            return Err(fail(format!(
                "row {row} names cell {cell} out of ascending order"
            )));
        }
        previous = Some(cell);
        let end = at
            .checked_add(direct)
            .and_then(|end| end.checked_add(bounce))
            .filter(|&end| end <= bytes.len())
            .ok_or_else(|| fail(format!("row {row}'s payload runs past the block")))?;
        rows.push(MoverStreamedRow {
            cell,
            direct: &bytes[at..at + direct],
            bounce: &bytes[at + direct..end],
        });
        at = end;
    }
    if at != bytes.len() {
        return Err(fail(format!("has {} trailing byte(s)", bytes.len() - at)));
    }
    Ok(rows)
}

fn halves(bytes: &[u8]) -> impl Iterator<Item = u16> + '_ {
    bytes
        .as_chunks::<2>()
        .0
        .iter()
        .map(|&half| u16::from_le_bytes(half))
}
