// MoverProbeChannel (id 57) read views: the direct block walked cell → term
// record → mover visibility, bounce entries with their payload, a listed
// cell's payload whichever placement holds it, and the counts a renderer
// sizes its buffers by.
// See: context/lib/build_pipeline.md §PRL section IDs

use super::{
    MOVER_IRRADIANCE_TILE_F16_COUNT, MOVER_VISIBILITY_BYTES_PER_PROBE, MoverBounceEntry,
    MoverCellSlices, MoverChannelBlock, MoverDirectRecord, MoverProbeChannelError,
    MoverProbeChannelSection, MoverStreamedRow, bounce_entry_f16_count, entry_positions,
};

/// A run of f16 halves, as the section holds them or as id 50 carries them
/// (little-endian bytes). Equal runs compare equal whichever form holds them.
#[derive(Debug, Clone, Copy)]
pub enum MoverHalves<'a> {
    Decoded(&'a [u16]),
    /// Little-endian, two bytes per half.
    Encoded(&'a [u8]),
}

impl<'a> MoverHalves<'a> {
    pub fn len(&self) -> usize {
        match self {
            Self::Decoded(halves) => halves.len(),
            Self::Encoded(bytes) => bytes.len() / 2,
        }
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Half `index`, if in range.
    pub fn get(&self, index: usize) -> Option<u16> {
        match self {
            Self::Decoded(halves) => halves.get(index).copied(),
            Self::Encoded(bytes) => bytes
                .get(index * 2..index * 2 + 2)
                .map(|half| u16::from_le_bytes([half[0], half[1]])),
        }
    }

    /// Halves `range` of the run.
    pub fn slice(&self, range: std::ops::Range<usize>) -> Option<MoverHalves<'a>> {
        match *self {
            Self::Decoded(halves) => halves.get(range).map(Self::Decoded),
            Self::Encoded(bytes) => bytes.get(range.start * 2..range.end * 2).map(Self::Encoded),
        }
    }

    pub fn iter(&self) -> impl Iterator<Item = u16> + 'a {
        let this = *self;
        (0..this.len()).map(move |index| this.get(index).expect("index below len"))
    }

    pub fn to_vec(&self) -> Vec<u16> {
        self.iter().collect()
    }
}

impl PartialEq for MoverHalves<'_> {
    fn eq(&self, other: &Self) -> bool {
        self.len() == other.len() && self.iter().eq(other.iter())
    }
}

impl PartialEq<[u16]> for MoverHalves<'_> {
    fn eq(&self, other: &[u16]) -> bool {
        self.len() == other.len() && self.iter().eq(other.iter().copied())
    }
}

impl PartialEq<&[u16]> for MoverHalves<'_> {
    fn eq(&self, other: &&[u16]) -> bool {
        *self == **other
    }
}

impl PartialEq<Vec<u16>> for MoverHalves<'_> {
    fn eq(&self, other: &Vec<u16>) -> bool {
        *self == other[..]
    }
}

/// One listed cell's payload, whichever placement holds each block: the
/// section's own vectors for a held block, the cell's id-50 row for a
/// streamed one. A renderer reads a cell through this and the
/// `*_at` views below, so it never asks where the payload lives.
#[derive(Debug, Clone, Copy)]
pub struct MoverCellPayload<'a> {
    /// `q₀, q₁` per masked probe, record after record, mover after mover.
    pub visibility: &'a [u8],
    /// One tile per masked probe, flagged record after flagged record.
    pub own_tiles: MoverHalves<'a>,
    /// Per affected probe, its stored `S^b₀` then its stored `S^b₁`, entry
    /// after entry.
    pub bounce_tiles: MoverHalves<'a>,
}

/// One listed cell's term records.
#[derive(Debug, Clone, Copy)]
pub struct DirectCellView<'a> {
    section: &'a MoverProbeChannelSection,
    /// Position in [`MoverProbeChannelSection::cells`].
    position: usize,
    payload: MoverCellPayload<'a>,
}

impl<'a> DirectCellView<'a> {
    /// Dense affinity cell index.
    pub fn cell(&self) -> u32 {
        self.section.cells[self.position].cell
    }

    pub fn valid_probe_mask(&self) -> u64 {
        self.section.cells[self.position].valid_probe_mask
    }

    /// The cell's term records in wire order, with their payload.
    pub fn records(self) -> impl Iterator<Item = DirectRecordView<'a>> + 'a {
        let section = self.section;
        let (cell, valid_probe_mask) = (self.cell(), self.valid_probe_mask());
        let start = section.direct_offsets[self.position] as usize;
        let end = section.direct_offsets[self.position + 1] as usize;
        let payload = self.payload;
        let (mut visibility_at, mut tile_at) = (0usize, 0usize);
        (start..end).map(move |index| {
            let (visibility_len, tile_len) = record_payload_len(section, index);
            let record = &section.direct_records[index];
            let view = DirectRecordView {
                cell,
                valid_probe_mask,
                record,
                mover_ids: section.record_movers(index),
                visibility: &payload.visibility[visibility_at..visibility_at + visibility_len],
                own_tiles: record.carries_own_tile.then(|| {
                    payload
                        .own_tiles
                        .slice(tile_at..tile_at + tile_len)
                        .expect("a cell's payload covers its records")
                }),
            };
            visibility_at += visibility_len;
            tile_at += tile_len;
            view
        })
    }
}

/// One term record with its payload.
#[derive(Debug, Clone, Copy)]
pub struct DirectRecordView<'a> {
    /// Dense affinity cell index.
    pub cell: u32,
    pub valid_probe_mask: u64,
    pub record: &'a MoverDirectRecord,
    /// Gating movers, ascending by id.
    pub mover_ids: &'a [u32],
    /// Every mover's `q₀, q₁` per masked probe, mover after mover.
    pub visibility: &'a [u8],
    /// One tile per masked probe, x-fastest; `None` unless the record
    /// carries its own (else id 41 or id 45 holds the term per probe).
    pub own_tiles: Option<MoverHalves<'a>>,
}

impl<'a> DirectRecordView<'a> {
    /// Each gating mover with its `q` pairs, ascending by mover id.
    pub fn movers(&self) -> impl Iterator<Item = MoverVisibilityView<'a>> + 'a {
        let per_mover = self.record.masked_probe_count() * MOVER_VISIBILITY_BYTES_PER_PROBE;
        let visibility = self.visibility;
        self.mover_ids
            .iter()
            .enumerate()
            .map(move |(at, &mover_id)| MoverVisibilityView {
                mover_id,
                visibility: &visibility[at * per_mover..(at + 1) * per_mover],
            })
    }

    /// The own tile at masked slot `slot` (x-fastest rank among the mask's
    /// bits), if the record carries tiles.
    pub fn own_tile(&self, slot: usize) -> Option<MoverHalves<'a>> {
        self.own_tiles?.slice(
            slot * MOVER_IRRADIANCE_TILE_F16_COUNT..(slot + 1) * MOVER_IRRADIANCE_TILE_F16_COUNT,
        )
    }
}

/// One mover's visibility under one term record.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MoverVisibilityView<'a> {
    pub mover_id: u32,
    /// `q₀, q₁` per masked probe, x-fastest; `q = byte / 255`.
    pub visibility: &'a [u8],
}

impl MoverVisibilityView<'_> {
    /// `[q₀, q₁]` bytes at masked slot `slot`.
    pub fn q(&self, slot: usize) -> [u8; 2] {
        let at = slot * MOVER_VISIBILITY_BYTES_PER_PROBE;
        [self.visibility[at], self.visibility[at + 1]]
    }
}

/// A bounce entry with its payload.
#[derive(Debug, Clone, Copy)]
pub struct BounceEntryView<'a> {
    /// Dense affinity cell index.
    pub cell: u32,
    pub entry: &'a MoverBounceEntry,
    /// Per affected probe, x-fastest: its stored `S^b₀` then its stored
    /// `S^b₁`. `None` when the payload streams with id 50 and is not held.
    pub tiles: Option<MoverHalves<'a>>,
}

impl<'a> BounceEntryView<'a> {
    /// Local probe `local`'s `[S^b₀, S^b₁]`: each stored tile, `None` at a
    /// terminus that stores none (the probe composes id 34 there). `None`
    /// overall when the probe is not affected or the payload is not held.
    pub fn probe_tiles(&self, local: u32) -> Option<[Option<MoverHalves<'a>>; 2]> {
        let tiles = self.tiles?;
        let slots = self.entry.probe_tiles(local)?;
        Some(slots.map(|slot| {
            slot.map(|tile| {
                tiles
                    .slice(
                        tile * MOVER_IRRADIANCE_TILE_F16_COUNT
                            ..(tile + 1) * MOVER_IRRADIANCE_TILE_F16_COUNT,
                    )
                    .expect("an entry's payload covers its stored tiles")
            })
        }))
    }
}

/// What the direct block holds, for GPU buffer sizing.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct MoverDirectCounts {
    /// Listed cells holding a term record.
    pub cells: usize,
    pub records: usize,
    /// (record, mover) visibility entries.
    pub mover_entries: usize,
    /// Σ masked probes over records: one gated term slot each.
    pub masked_probes: usize,
    /// Σ masked probes over flagged records: one own tile each.
    pub own_tile_probes: usize,
    /// Σ masked probes over mover entries: one `[q₀, q₁]` pair each.
    pub visibility_pairs: usize,
}

/// [`MoverProbeChannelSection::to_bytes`]'s bytes, part by part, and the
/// payload placed with id 50 instead.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct MoverChannelBytes {
    /// Epoch, affinity grid, payload placements.
    pub header: usize,
    /// Listed cell count and the listed cell records (index and valid mask).
    pub cell_index: usize,
    /// Direct offsets per listed cell and mover offsets per record.
    pub direct_index: usize,
    /// Term records and mover ids.
    pub direct_records: usize,
    /// Resident visibility; 0 when the direct payload streams.
    pub visibility: usize,
    /// Resident own tiles; 0 when the direct payload streams.
    pub own_tiles: usize,
    /// Bounce offsets per listed cell and bounce entries.
    pub bounce_index: usize,
    /// Resident bounce tiles; 0 when the bounce payload streams.
    pub bounce_payload: usize,
    /// Depth count, records and tiles.
    pub depth: usize,
    pub ramps: usize,
    /// The direct payload placed with id 50's cells, outside [`Self::total`].
    pub streamed_direct: usize,
    /// The bounce payload placed with id 50's cells, outside [`Self::total`].
    pub streamed_bounce: usize,
}

impl MoverChannelBytes {
    /// Bytes the section itself writes.
    pub fn total(&self) -> usize {
        self.header
            + self.cell_index
            + self.direct_index
            + self.direct_records
            + self.visibility
            + self.own_tiles
            + self.bounce_index
            + self.bounce_payload
            + self.depth
            + self.ramps
    }

    /// The direct block's own bytes in the section: its index, records and
    /// resident payload; the shared cell index is apart.
    pub fn direct_block(&self) -> usize {
        self.direct_index + self.direct_records + self.visibility + self.own_tiles
    }
}

impl MoverProbeChannelSection {
    /// The direct block walked cell → term record → mover, in wire order,
    /// over the payload the section holds. Listed cells holding only bounce
    /// entries are skipped. When the direct payload streams and is not held,
    /// this yields nothing: read each cell through [`Self::cell_payload`]
    /// with its id-50 row and [`Self::direct_cell_at`].
    pub fn direct_cells(&self) -> impl Iterator<Item = DirectCellView<'_>> {
        let held = self.payload_held(MoverChannelBlock::Direct);
        let slices = if held { self.cell_slices() } else { Vec::new() };
        slices
            .into_iter()
            .enumerate()
            .filter_map(move |(position, slices)| {
                let payload = self.held_payload(&slices);
                self.direct_cell_at(position, payload)
            })
    }

    /// Every term record with its cell and payload, in wire order, over the
    /// payload the section holds ([`Self::direct_cells`]).
    pub fn direct_record_views(&self) -> impl Iterator<Item = DirectRecordView<'_>> {
        self.direct_cells().flat_map(|cell| cell.records())
    }

    /// Listed cell `position`'s term records over `payload`, or `None` when
    /// the cell holds none.
    pub fn direct_cell_at<'a>(
        &'a self,
        position: usize,
        payload: MoverCellPayload<'a>,
    ) -> Option<DirectCellView<'a>> {
        let start = self.direct_offsets[position];
        let end = self.direct_offsets[position + 1];
        (start != end).then_some(DirectCellView {
            section: self,
            position,
            payload,
        })
    }

    /// Listed cell `position`'s bounce entries over `payload`, in wire order.
    pub fn bounce_entries_at<'a>(
        &'a self,
        position: usize,
        payload: MoverCellPayload<'a>,
    ) -> impl Iterator<Item = BounceEntryView<'a>> + 'a {
        let cell = self.cells[position].cell;
        let start = self.bounce_offsets[position] as usize;
        let end = self.bounce_offsets[position + 1] as usize;
        let mut tile_at = 0;
        self.bounce_entries[start..end].iter().map(move |entry| {
            let len = bounce_entry_f16_count(entry);
            let tiles = payload
                .bounce_tiles
                .slice(tile_at..tile_at + len)
                .expect("a cell's payload covers its bounce entries");
            tile_at += len;
            BounceEntryView {
                cell,
                entry,
                tiles: Some(tiles),
            }
        })
    }

    /// Every bounce entry with its cell, in wire order, with the payload the
    /// section holds (`None` while a streamed payload is not held).
    pub fn bounce_entry_views(&self) -> impl Iterator<Item = BounceEntryView<'_>> {
        let held = self.payload_held(MoverChannelBlock::Bounce);
        let mut tile_at = 0;
        entry_positions(&self.bounce_offsets)
            .zip(&self.bounce_entries)
            .map(move |(position, entry)| {
                let tiles = held.then(|| {
                    let len = bounce_entry_f16_count(entry);
                    let tiles = &self.bounce_tiles[tile_at..tile_at + len];
                    tile_at += len;
                    MoverHalves::Decoded(tiles)
                });
                BounceEntryView {
                    cell: self.cells[position].cell,
                    entry,
                    tiles,
                }
            })
    }

    /// Listed cell `position`'s payload: each held block from the section,
    /// each streamed block not held from `streamed`, the cell's id-50 row.
    /// `slices` is the cell's entry of [`Self::cell_slices`]. A streamed
    /// block that is neither held nor in a row of the right length is the
    /// named error, so no reader samples a payload that is not there.
    pub fn cell_payload<'a>(
        &'a self,
        position: usize,
        slices: &MoverCellSlices,
        streamed: Option<MoverStreamedRow<'a>>,
    ) -> Result<MoverCellPayload<'a>, MoverProbeChannelError> {
        let mut payload = self.held_payload(slices);
        let lens = slices.row_lens(self);
        let cell = self.cells[position].cell;
        for block in MoverChannelBlock::BOTH {
            let expected = match block {
                MoverChannelBlock::Direct => lens.direct,
                MoverChannelBlock::Bounce => lens.bounce,
            } as usize;
            // A cell holding nothing of a streamed block has no bytes of it
            // in its row, or no row at all.
            if self.payload_held(block) || expected == 0 {
                continue;
            }
            let row =
                streamed.ok_or(MoverProbeChannelError::StreamedPayloadUnresolved { block })?;
            if row.cell != cell {
                return Err(MoverProbeChannelError::StreamedPayloadMismatch {
                    block,
                    cell,
                    detail: format!("is read from the row of affinity cell {}", row.cell),
                });
            }
            let bytes = match block {
                MoverChannelBlock::Direct => row.direct,
                MoverChannelBlock::Bounce => row.bounce,
            };
            if bytes.len() != expected {
                return Err(MoverProbeChannelError::StreamedPayloadMismatch {
                    block,
                    cell,
                    detail: format!("holds {} byte(s), expected {expected}", bytes.len()),
                });
            }
            match block {
                MoverChannelBlock::Direct => {
                    let (visibility, tiles) = bytes.split_at(slices.visibility.len());
                    payload.visibility = visibility;
                    payload.own_tiles = MoverHalves::Encoded(tiles);
                }
                MoverChannelBlock::Bounce => payload.bounce_tiles = MoverHalves::Encoded(bytes),
            }
        }
        Ok(payload)
    }

    /// The cell's payload from the section's own vectors; a block not held
    /// reads empty.
    fn held_payload(&self, slices: &MoverCellSlices) -> MoverCellPayload<'_> {
        let direct = self.payload_held(MoverChannelBlock::Direct);
        let bounce = self.payload_held(MoverChannelBlock::Bounce);
        MoverCellPayload {
            visibility: if direct {
                &self.visibility[slices.visibility.clone()]
            } else {
                &[]
            },
            own_tiles: MoverHalves::Decoded(if direct {
                &self.own_tiles[slices.own_tiles.clone()]
            } else {
                &[]
            }),
            bounce_tiles: MoverHalves::Decoded(if bounce {
                &self.bounce_tiles[slices.bounce_tiles.clone()]
            } else {
                &[]
            }),
        }
    }

    /// What the direct block holds, for sizing GPU buffers.
    pub fn direct_counts(&self) -> MoverDirectCounts {
        let mut counts = MoverDirectCounts {
            cells: self
                .direct_offsets
                .windows(2)
                .filter(|range| range[0] != range[1])
                .count(),
            records: self.direct_records.len(),
            mover_entries: self.direct_movers.len(),
            ..MoverDirectCounts::default()
        };
        for (index, record) in self.direct_records.iter().enumerate() {
            let probes = record.masked_probe_count();
            counts.masked_probes += probes;
            if record.carries_own_tile {
                counts.own_tile_probes += probes;
            }
            counts.visibility_pairs += probes * self.record_movers(index).len();
        }
        counts
    }
}

/// Visibility bytes and own-tile halves record `index` holds.
pub(super) fn record_payload_len(
    section: &MoverProbeChannelSection,
    index: usize,
) -> (usize, usize) {
    let record = &section.direct_records[index];
    let probes = record.masked_probe_count();
    let visibility = probes * MOVER_VISIBILITY_BYTES_PER_PROBE * section.record_movers(index).len();
    let tiles = if record.carries_own_tile {
        probes * MOVER_IRRADIANCE_TILE_F16_COUNT
    } else {
        0
    };
    (visibility, tiles)
}
