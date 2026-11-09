// MoverProbeChannel (id 57) read views: the direct block walked cell → term
// record → mover visibility, bounce entries with their payload, and the
// counts a renderer sizes its buffers by.
// See: context/lib/build_pipeline.md §PRL section IDs

use super::{
    MOVER_IRRADIANCE_TILE_F16_COUNT, MOVER_VISIBILITY_BYTES_PER_PROBE, MoverBounceEntry,
    MoverBouncePlacement, MoverDirectRecord, MoverProbeChannelSection, bounce_entry_f16_count,
    entry_positions,
};

/// One listed cell's term records.
#[derive(Debug, Clone, Copy)]
pub struct DirectCellView<'a> {
    section: &'a MoverProbeChannelSection,
    /// Position in [`MoverProbeChannelSection::cells`].
    position: usize,
    /// Payload cursors at the cell's first record.
    visibility_at: usize,
    tile_at: usize,
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
        let (mut visibility_at, mut tile_at) = (self.visibility_at, self.tile_at);
        (start..end).map(move |index| {
            let view = record_view(
                section,
                index,
                cell,
                valid_probe_mask,
                visibility_at,
                tile_at,
            );
            visibility_at += view.visibility.len();
            tile_at += view.own_tiles.map_or(0, <[u16]>::len);
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
    pub own_tiles: Option<&'a [u16]>,
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
    pub fn own_tile(&self, slot: usize) -> Option<&'a [u16]> {
        let tiles = self.own_tiles?;
        tiles.get(
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
    /// `S^b₀, S^b₁` per affected probe, x-fastest; `None` when the payload
    /// streams with id 50.
    pub tiles: Option<&'a [u16]>,
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

/// [`MoverProbeChannelSection::to_bytes`]'s bytes, part by part.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct MoverChannelBytes {
    /// Epoch, affinity grid, bounce placement.
    pub header: usize,
    /// Listed cell count and the listed cell records (index and valid mask).
    pub cell_index: usize,
    /// Direct offsets per listed cell and mover offsets per record.
    pub direct_index: usize,
    /// Term records and mover ids.
    pub direct_records: usize,
    pub visibility: usize,
    pub own_tiles: usize,
    /// Bounce offsets per listed cell and bounce entries.
    pub bounce_index: usize,
    pub bounce_payload: usize,
    /// Depth count, records and tiles.
    pub depth: usize,
    pub ramps: usize,
}

impl MoverChannelBytes {
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

    /// The direct block's own bytes: its index, records, visibility and
    /// tiles; the shared cell index is apart.
    pub fn direct_block(&self) -> usize {
        self.direct_index + self.direct_records + self.visibility + self.own_tiles
    }
}

impl MoverProbeChannelSection {
    /// The direct block walked cell → term record → mover, in wire order.
    /// Listed cells holding only bounce entries are skipped.
    pub fn direct_cells(&self) -> impl Iterator<Item = DirectCellView<'_>> {
        let (mut visibility_at, mut tile_at) = (0usize, 0usize);
        (0..self.cells.len()).filter_map(move |position| {
            let view = DirectCellView {
                section: self,
                position,
                visibility_at,
                tile_at,
            };
            let start = self.direct_offsets[position] as usize;
            let end = self.direct_offsets[position + 1] as usize;
            for index in start..end {
                let (visibility, tiles) = record_payload_len(self, index);
                visibility_at += visibility;
                tile_at += tiles;
            }
            (start != end).then_some(view)
        })
    }

    /// Every term record with its cell and payload, in wire order.
    pub fn direct_record_views(&self) -> impl Iterator<Item = DirectRecordView<'_>> {
        self.direct_cells().flat_map(|cell| cell.records())
    }

    /// Every bounce entry with its cell and payload, in wire order.
    pub fn bounce_entry_views(&self) -> impl Iterator<Item = BounceEntryView<'_>> {
        let resident = self.bounce_placement == MoverBouncePlacement::Resident;
        let mut tile_at = 0;
        entry_positions(&self.bounce_offsets)
            .zip(&self.bounce_entries)
            .map(move |(position, entry)| {
                let tiles = resident.then(|| {
                    let len = bounce_entry_f16_count(entry);
                    let tiles = &self.bounce_tiles[tile_at..tile_at + len];
                    tile_at += len;
                    tiles
                });
                BounceEntryView {
                    cell: self.cells[position].cell,
                    entry,
                    tiles,
                }
            })
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

fn record_view<'a>(
    section: &'a MoverProbeChannelSection,
    index: usize,
    cell: u32,
    valid_probe_mask: u64,
    visibility_at: usize,
    tile_at: usize,
) -> DirectRecordView<'a> {
    let record = &section.direct_records[index];
    let (visibility_len, tile_len) = record_payload_len(section, index);
    DirectRecordView {
        cell,
        valid_probe_mask,
        record,
        mover_ids: section.record_movers(index),
        visibility: &section.visibility[visibility_at..visibility_at + visibility_len],
        own_tiles: record
            .carries_own_tile
            .then(|| &section.own_tiles[tile_at..tile_at + tile_len]),
    }
}

/// Visibility bytes and own-tile halves record `index` holds.
fn record_payload_len(section: &MoverProbeChannelSection, index: usize) -> (usize, usize) {
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
