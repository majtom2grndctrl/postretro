// MoverProbeChannel (id 57) bytes: encode, and the structural decode that
// pins every count, order, mask, reserved field and length of the layout.
// See: context/lib/build_pipeline.md §PRL section IDs

mod across;
mod checks;
mod posed;
mod reader;

pub(super) use across::across_entry;
use across::{
    NamingBlocks, across_block_bytes, check_across_named, put_across_entries, read_across_entries,
};
pub(super) use checks::check_masks_are_tight;
use checks::{check_cell_order, check_mask};
use posed::{
    bounce_entry_bytes, depth_record_bytes, mover_record_bytes, put_bounce_entries,
    put_depth_records, put_mover_records, read_bounce_entries, read_depth_records,
    read_mover_records,
};
pub(super) use reader::put_halves;
use reader::{Reader, checked_add, le_u32, le_u64, put_u32s};

use super::{
    MOVER_IRRADIANCE_TILE_F16_COUNT, MOVER_PROBE_CHANNEL_EPOCH, MOVER_RECORD_FLAG_OWN_TILE,
    MoverBounceEntry, MoverChannelBlock, MoverChannelBytes, MoverChannelCell, MoverDepthRecord,
    MoverDirectRecord, MoverLightNamespace, MoverPayloadPlacement, MoverProbeChannelError,
    MoverProbeChannelSection, MoverRecord, bounce_entry_f16_count, entry_positions,
};
use crate::delta_sh_volumes::AFFINITY_FACTOR;
use crate::probe_depth_tiles::{PROBE_DEPTH_TILE_TEXEL_BYTES, PROBE_DEPTH_TILE_TEXELS};

type Error = MoverProbeChannelError;

/// epoch, affinity factor, affinity dims, direct placement, bounce placement.
const HEADER_BYTES: usize = 1 + 1 + 12 + 1 + 1;
/// u32 cell, u64 valid-probe mask.
const CELL_RECORD_BYTES: usize = 12;
/// u8 namespace, u8 flags, u16 reserved, u32 light, u64 probe mask.
const DIRECT_RECORD_BYTES: usize = 16;
const MOVER_ID_BYTES: usize = 4;
const DEPTH_TILE_BYTES: usize = PROBE_DEPTH_TILE_TEXELS * PROBE_DEPTH_TILE_TEXEL_BYTES as usize;

impl MoverProbeChannelSection {
    /// [`Self::to_bytes`]'s bytes, part by part, from the section's
    /// structure, so a streamed payload counts the same whether or not it is
    /// held: a resident payload in its own parts, a streamed one in
    /// `streamed` and outside the total.
    pub fn byte_breakdown(&self) -> MoverChannelBytes {
        let counts = self.direct_counts();
        let visibility = counts.visibility_bytes;
        let own_tiles = counts.own_tile_probes * MOVER_IRRADIANCE_TILE_F16_COUNT * 2;
        let bounce_payload = self.bounce_payload_bytes() as usize;
        let resident = |block| self.placement(block) == MoverPayloadPlacement::Resident;
        let direct_resident = resident(MoverChannelBlock::Direct);
        let bounce_resident = resident(MoverChannelBlock::Bounce);
        MoverChannelBytes {
            header: HEADER_BYTES,
            cell_index: 4 + self.cells.len() * CELL_RECORD_BYTES,
            mover_records: 4 + self
                .mover_records
                .iter()
                .map(mover_record_bytes)
                .sum::<usize>(),
            direct_index: (self.direct_offsets.len() + self.mover_offsets.len()) * 4,
            direct_records: self.direct_records.len() * DIRECT_RECORD_BYTES
                + self.direct_movers.len() * MOVER_ID_BYTES,
            across: across_block_bytes(&self.mover_records, &self.across_entries),
            visibility: if direct_resident { visibility } else { 0 },
            own_tiles: if direct_resident { own_tiles } else { 0 },
            bounce_index: self.bounce_offsets.len() * 4
                + (self.bounce_entries.len() + 1) * 4
                + self
                    .bounce_entries
                    .iter()
                    .map(|entry| bounce_entry_bytes(entry.pose_count()))
                    .sum::<usize>(),
            bounce_payload: if bounce_resident { bounce_payload } else { 0 },
            depth: 4
                + self
                    .depth_records
                    .iter()
                    .map(|record| depth_record_bytes(record.pose_count()))
                    .sum::<usize>()
                + self.depth_tiles.len() * PROBE_DEPTH_TILE_TEXEL_BYTES as usize,
            streamed_direct: if direct_resident {
                0
            } else {
                visibility + own_tiles
            },
            streamed_bounce: if bounce_resident { 0 } else { bounce_payload },
        }
    }
}

pub(super) fn to_bytes(section: &MoverProbeChannelSection) -> Vec<u8> {
    let listed = section.cells.len();
    debug_assert_eq!(section.direct_offsets.len(), listed + 1);
    debug_assert_eq!(section.bounce_offsets.len(), listed + 1);
    debug_assert_eq!(
        section.mover_offsets.len(),
        section.direct_records.len() + 1
    );
    debug_assert_eq!(
        section.direct_offsets.last().copied(),
        Some(section.direct_records.len() as u32)
    );
    debug_assert_eq!(
        section.mover_offsets.last().copied(),
        Some(section.direct_movers.len() as u32)
    );
    debug_assert_eq!(
        section.bounce_offsets.last().copied(),
        Some(section.bounce_entries.len() as u32)
    );
    debug_assert_eq!(
        section.depth_tiles.len(),
        section
            .depth_records
            .iter()
            .map(MoverDepthRecord::stored_tile_count)
            .sum::<usize>()
            * PROBE_DEPTH_TILE_TEXELS
    );
    debug_assert_eq!(
        section
            .mover_records
            .iter()
            .map(|record| record.mover_id)
            .collect::<Vec<_>>(),
        section.mover_ids(),
        "mover records cover exactly the movers the blocks name"
    );
    let direct_resident = section.direct_placement == MoverPayloadPlacement::Resident;
    let bounce_resident = section.bounce_placement == MoverPayloadPlacement::Resident;
    debug_assert!(
        section.payload_held(MoverChannelBlock::Direct)
            && section.payload_held(MoverChannelBlock::Bounce),
        "a resident payload is held; a streamed one need not be, and is not written"
    );

    let mut out = Vec::with_capacity(section.encoded_len());
    out.push(MOVER_PROBE_CHANNEL_EPOCH);
    out.push(section.affinity_factor);
    for dim in section.affinity_dims {
        out.extend_from_slice(&dim.to_le_bytes());
    }
    out.push(section.direct_placement as u8);
    out.push(section.bounce_placement as u8);

    out.extend_from_slice(&(listed as u32).to_le_bytes());
    for cell in &section.cells {
        out.extend_from_slice(&cell.cell.to_le_bytes());
        out.extend_from_slice(&cell.valid_probe_mask.to_le_bytes());
    }
    put_mover_records(&mut out, &section.mover_records);

    put_u32s(&mut out, &section.direct_offsets);
    for record in &section.direct_records {
        out.push(record.namespace as u8);
        out.push(if record.carries_own_tile {
            MOVER_RECORD_FLAG_OWN_TILE
        } else {
            0
        });
        out.extend_from_slice(&0u16.to_le_bytes());
        out.extend_from_slice(&record.light_index.to_le_bytes());
        out.extend_from_slice(&record.probe_mask.to_le_bytes());
    }
    put_u32s(&mut out, &section.mover_offsets);
    put_u32s(&mut out, &section.direct_movers);
    put_across_entries(&mut out, &section.mover_records, &section.across_entries);
    if direct_resident {
        out.extend_from_slice(&section.visibility);
        put_halves(&mut out, &section.own_tiles);
    }

    put_u32s(&mut out, &section.bounce_offsets);
    put_bounce_entries(&mut out, &section.bounce_entries);
    if bounce_resident {
        put_halves(&mut out, &section.bounce_tiles);
    }

    put_depth_records(&mut out, &section.depth_records);
    for [mean, mean_sq] in &section.depth_tiles {
        out.extend_from_slice(&mean.to_le_bytes());
        out.extend_from_slice(&mean_sq.to_le_bytes());
    }
    debug_assert_eq!(out.len(), section.encoded_len());
    out
}

pub(super) fn from_bytes(data: &[u8]) -> Result<MoverProbeChannelSection, Error> {
    let mut reader = Reader { data, at: 0 };

    let epoch = reader.u8("epoch")?;
    if epoch != MOVER_PROBE_CHANNEL_EPOCH {
        return Err(Error::EpochMismatch {
            found: epoch,
            expected: MOVER_PROBE_CHANNEL_EPOCH,
        });
    }
    let affinity_factor = reader.u8("affinity factor")?;
    if affinity_factor != AFFINITY_FACTOR {
        return Err(Error::AffinityFactor {
            found: affinity_factor,
            expected: AFFINITY_FACTOR,
        });
    }
    let affinity_dims = [
        reader.u32("affinity dims")?,
        reader.u32("affinity dims")?,
        reader.u32("affinity dims")?,
    ];
    let grid_cells = affinity_dims.iter().try_fold(1usize, |count, &dim| {
        count.checked_mul(dim as usize).ok_or(Error::SizeOverflow {
            what: "affinity cell count",
        })
    })?;
    let mut placement = |block: MoverChannelBlock| {
        let value = reader.u8("payload placement")?;
        MoverPayloadPlacement::from_u8(value).ok_or(Error::UnknownPlacement { block, value })
    };
    let direct_placement = placement(MoverChannelBlock::Direct)?;
    let bounce_placement = placement(MoverChannelBlock::Bounce)?;

    let cells = read_cells(&mut reader, grid_cells)?;
    let listed = cells.len();
    let cell_of = |position: usize| cells[position].cell as usize;

    // Mover records come first: every block's per-pose lengths follow them.
    let mover_records = read_mover_records(&mut reader)?;
    let pose_count = |block: &'static str, mover_id: u32| {
        mover_records
            .binary_search_by_key(&mover_id, |record| record.mover_id)
            .map(|at| mover_records[at].pose_count())
            .map_err(|_| Error::MoverRecordMissing { block, mover_id })
    };

    // Direct block.
    let direct_offsets = reader.offsets(listed, "direct", "direct offsets")?;
    let record_count = *direct_offsets.last().expect("listed + 1 offsets") as usize;
    let direct_records = read_direct_records(&mut reader, record_count)?;
    for (index, (position, record)) in entry_positions(&direct_offsets)
        .zip(&direct_records)
        .enumerate()
    {
        check_mask("direct", cells[position], index, record.probe_mask)?;
    }
    check_cell_order(
        "direct",
        &direct_offsets,
        &cells,
        &direct_records,
        |record| record.order_key(),
    )?;

    let mover_offsets = reader.offsets(record_count, "direct movers", "mover offsets")?;
    let mover_count = *mover_offsets.last().expect("records + 1 offsets") as usize;
    let direct_movers: Vec<u32> = reader
        .records(mover_count, MOVER_ID_BYTES, "direct movers")?
        .map(le_u32)
        .collect();
    let mut visibility_len = 0usize;
    let mut own_tile_halves = 0usize;
    // Each listed cell's direct payload bytes: its id-50 row when the block
    // streams.
    let mut direct_row_bytes = vec![0u64; listed];
    for (index, (position, record)) in entry_positions(&direct_offsets)
        .zip(&direct_records)
        .enumerate()
    {
        let movers =
            &direct_movers[mover_offsets[index] as usize..mover_offsets[index + 1] as usize];
        if movers.is_empty() {
            return Err(Error::RecordWithoutMovers {
                cell: cell_of(position),
                record: index,
            });
        }
        if let Some(at) = movers.windows(2).position(|pair| pair[0] >= pair[1]) {
            return Err(Error::MoverOrder {
                cell: cell_of(position),
                record: index,
                entry: mover_offsets[index] as usize + at + 1,
            });
        }
        let probes = record.masked_probe_count();
        for &mover in movers {
            let len =
                probes
                    .checked_mul(pose_count("direct", mover)?)
                    .ok_or(Error::SizeOverflow {
                        what: "visibility payload",
                    })?;
            visibility_len = checked_add(visibility_len, len, "visibility payload")?;
            direct_row_bytes[position] = direct_row_bytes[position].saturating_add(len as u64);
        }
        if record.carries_own_tile {
            let halves = probes * MOVER_IRRADIANCE_TILE_F16_COUNT;
            own_tile_halves = checked_add(own_tile_halves, halves, "own-tile payload")?;
            direct_row_bytes[position] =
                direct_row_bytes[position].saturating_add(2 * halves as u64);
        }
    }
    if direct_placement == MoverPayloadPlacement::Streamed {
        check_streamed_row_lens(MoverChannelBlock::Direct, &cells, direct_row_bytes)?;
    }
    // Across marks stay resident: the q payload they qualify may stream.
    let across_entries = read_across_entries(&mut reader, &cells, &mover_records, pose_count)?;
    let (visibility, own_tiles) = match direct_placement {
        MoverPayloadPlacement::Resident => {
            let visibility = reader.take(visibility_len, "visibility payload")?.to_vec();
            check_masks_are_tight(
                &cells,
                &direct_offsets,
                &direct_records,
                &mover_offsets,
                &direct_movers,
                &visibility,
                &across_entries,
                // The visibility-length pass resolved every mover a
                // record names, or returned its missing-record error.
                |mover| pose_count("direct", mover).expect("checked above"),
            )?;
            let own_tiles = reader.halves(own_tile_halves, "own-tile payload")?;
            (visibility, own_tiles)
        }
        MoverPayloadPlacement::Streamed if direct_records.is_empty() => {
            return Err(Error::StreamedBlockWithoutEntries {
                block: MoverChannelBlock::Direct,
            });
        }
        // Mask tightness reads q, so it is checked when the payload attaches.
        MoverPayloadPlacement::Streamed => (Vec::new(), Vec::new()),
    };

    // Bounce block.
    let bounce_offsets = reader.offsets(listed, "bounce", "bounce offsets")?;
    let bounce_count = *bounce_offsets.last().expect("listed + 1 offsets") as usize;
    let bounce_entries = read_bounce_entries(&mut reader, bounce_count, pose_count)?;
    check_cell_order(
        "bounce",
        &bounce_offsets,
        &cells,
        &bounce_entries,
        |entry| entry.mover_id,
    )?;
    for (index, (position, entry)) in entry_positions(&bounce_offsets)
        .zip(&bounce_entries)
        .enumerate()
    {
        let affected = entry.affected_probe_mask();
        check_mask("bounce", cells[position], index, affected)?;
        if let Some((pose, masks)) = entry
            .poses
            .iter()
            .enumerate()
            .find(|(_, masks)| masks.base & !affected != 0)
        {
            return Err(Error::BaseOutsideAffected {
                cell: cell_of(position),
                entry: index,
                pose,
                mask: masks.base & !affected,
            });
        }
    }
    check_across_named(
        &NamingBlocks {
            cells: &cells,
            direct_offsets: &direct_offsets,
            direct_records: &direct_records,
            mover_offsets: &mover_offsets,
            direct_movers: &direct_movers,
            bounce_offsets: &bounce_offsets,
            bounce_entries: &bounce_entries,
        },
        &across_entries,
    )?;
    let bounce_tiles = match bounce_placement {
        MoverPayloadPlacement::Resident => {
            let halves = bounce_entries.iter().try_fold(0usize, |total, entry| {
                checked_add(total, bounce_entry_f16_count(entry), "bounce payload")
            })?;
            reader.halves(halves, "bounce payload")?
        }
        MoverPayloadPlacement::Streamed if bounce_entries.is_empty() => {
            return Err(Error::StreamedBlockWithoutEntries {
                block: MoverChannelBlock::Bounce,
            });
        }
        MoverPayloadPlacement::Streamed => {
            let rows = (0..listed).map(|position| {
                bounce_entries
                    [bounce_offsets[position] as usize..bounce_offsets[position + 1] as usize]
                    .iter()
                    .map(|entry| 2 * bounce_entry_f16_count(entry) as u64)
                    .sum::<u64>()
            });
            check_streamed_row_lens(MoverChannelBlock::Bounce, &cells, rows)?;
            Vec::new()
        }
    };

    // Every listed cell holds a direct record or a bounce entry.
    if let Some(position) = (0..listed).find(|&at| {
        direct_offsets[at] == direct_offsets[at + 1] && bounce_offsets[at] == bounce_offsets[at + 1]
    }) {
        return Err(Error::CellWithoutEntries {
            cell: cell_of(position),
        });
    }

    // Depth block.
    let depth_count = reader.u32("depth record count")? as usize;
    let depth_records = read_depth_records(&mut reader, depth_count, pose_count)?;
    if let Some(record) = depth_records
        .windows(2)
        .position(|pair| pair[0].order_key() >= pair[1].order_key())
    {
        return Err(Error::DepthRecordOrder { record: record + 1 });
    }
    let depth_tile_count: usize = depth_records
        .iter()
        .map(MoverDepthRecord::stored_tile_count)
        .sum();
    let depth_tiles: Vec<[u16; 2]> = reader
        .records(depth_tile_count, DEPTH_TILE_BYTES, "depth tiles")?
        .flat_map(|pair| pair.as_chunks::<4>().0)
        .map(|&[mean_lo, mean_hi, sq_lo, sq_hi]| {
            [
                u16::from_le_bytes([mean_lo, mean_hi]),
                u16::from_le_bytes([sq_lo, sq_hi]),
            ]
        })
        .collect();

    if direct_records.is_empty() && bounce_entries.is_empty() && depth_records.is_empty() {
        return Err(Error::Empty);
    }

    let section = MoverProbeChannelSection {
        affinity_factor,
        affinity_dims,
        direct_placement,
        bounce_placement,
        cells,
        mover_records,
        direct_offsets,
        direct_records,
        mover_offsets,
        direct_movers,
        across_entries,
        visibility,
        own_tiles,
        bounce_offsets,
        bounce_entries,
        bounce_tiles,
        depth_records,
        depth_tiles,
    };

    // Every mover a block names has a record (checked as each was read);
    // no record names a mover without entries.
    let named = section.mover_ids();
    if let Some(unused) = section
        .mover_records
        .iter()
        .find(|record| named.binary_search(&record.mover_id).is_err())
    {
        return Err(Error::MoverRecordUnused {
            mover_id: unused.mover_id,
        });
    }

    let trailing = data.len() - reader.at;
    if trailing != 0 {
        return Err(Error::TrailingBytes { count: trailing });
    }
    Ok(section)
}

/// An id-50 row carries each listed cell's streamed payload length as a
/// `u32`, so a cell whose `block` payload passes `u32::MAX` bytes is refused
/// here, before any load path sizes its row from the section's structure.
fn check_streamed_row_lens(
    block: MoverChannelBlock,
    cells: &[MoverChannelCell],
    row_bytes: impl IntoIterator<Item = u64>,
) -> Result<(), Error> {
    match cells
        .iter()
        .zip(row_bytes)
        .find(|&(_, bytes)| bytes > u64::from(u32::MAX))
    {
        Some((cell, bytes)) => Err(Error::StreamedRowOverflow {
            block,
            cell: cell.cell,
            bytes,
        }),
        None => Ok(()),
    }
}

/// The listed cells: strictly ascending, each inside the affinity grid.
fn read_cells(reader: &mut Reader<'_>, grid_cells: usize) -> Result<Vec<MoverChannelCell>, Error> {
    let count = reader.u32("listed cell count")? as usize;
    let mut cells: Vec<MoverChannelCell> = Vec::new();
    for (listed, bytes) in reader
        .records(count, CELL_RECORD_BYTES, "listed cells")?
        .enumerate()
    {
        let cell = MoverChannelCell {
            cell: le_u32(&bytes[0..4]),
            valid_probe_mask: le_u64(&bytes[4..12]),
        };
        if cell.cell as usize >= grid_cells {
            return Err(Error::CellOutOfRange {
                listed,
                cell: cell.cell,
                cells: grid_cells,
            });
        }
        if let Some(previous) = cells.last()
            && previous.cell >= cell.cell
        {
            return Err(Error::CellOrder {
                listed,
                cell: cell.cell,
                previous: previous.cell,
            });
        }
        cells.push(cell);
    }
    Ok(cells)
}

fn read_direct_records(
    reader: &mut Reader<'_>,
    count: usize,
) -> Result<Vec<MoverDirectRecord>, Error> {
    let mut records = Vec::new();
    for (index, bytes) in reader
        .records(count, DIRECT_RECORD_BYTES, "direct records")?
        .enumerate()
    {
        let namespace =
            MoverLightNamespace::from_u8(bytes[0]).ok_or(Error::UnknownLightNamespace {
                record: index,
                value: bytes[0],
            })?;
        let flags = bytes[1];
        if flags & !MOVER_RECORD_FLAG_OWN_TILE != 0 {
            return Err(Error::ReservedBits {
                what: "direct record flags",
                index,
                value: u32::from(flags),
            });
        }
        if namespace.requires_own_tile() && flags & MOVER_RECORD_FLAG_OWN_TILE == 0 {
            return Err(Error::OwnTileRequired {
                record: index,
                namespace,
            });
        }
        let reserved = u16::from_le_bytes([bytes[2], bytes[3]]);
        if reserved != 0 {
            return Err(Error::ReservedBits {
                what: "direct record reserved u16",
                index,
                value: u32::from(reserved),
            });
        }
        records.push(MoverDirectRecord {
            namespace,
            carries_own_tile: flags & MOVER_RECORD_FLAG_OWN_TILE != 0,
            light_index: le_u32(&bytes[4..8]),
            probe_mask: le_u64(&bytes[8..16]),
        });
    }
    Ok(records)
}
