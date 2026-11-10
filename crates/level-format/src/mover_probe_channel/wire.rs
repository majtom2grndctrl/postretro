// MoverProbeChannel (id 57) bytes: encode, and the structural decode that
// pins every count, order, mask, reserved field and length of the layout.
// See: context/lib/build_pipeline.md §PRL section IDs

use super::{
    MOVER_IRRADIANCE_TILE_F16_COUNT, MOVER_PROBE_CHANNEL_EPOCH, MOVER_RECORD_FLAG_OWN_TILE,
    MOVER_VISIBILITY_BYTES_PER_PROBE, MoverBounceEntry, MoverChannelBlock, MoverChannelBytes,
    MoverChannelCell, MoverDepthRecord, MoverDirectRecord, MoverLightNamespace,
    MoverLightRampRecord, MoverPayloadPlacement, MoverProbeChannelError, MoverProbeChannelSection,
    bounce_entry_f16_count, entry_positions,
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
/// u32 mover, u32 reserved, u64 terminus-0 mask, u64 terminus-1 mask.
const BOUNCE_ENTRY_BYTES: usize = 24;
/// u32 mover, u32 probe, u32 stored-termini bits.
const DEPTH_RECORD_BYTES: usize = 12;
/// A depth record's stored-termini bits: bit k stores `D_k`.
const DEPTH_TERMINI_BITS: u32 = 0b11;
const RAMP_RECORD_BYTES: usize = 12;
const DEPTH_TILE_BYTES: usize = PROBE_DEPTH_TILE_TEXELS * PROBE_DEPTH_TILE_TEXEL_BYTES as usize;

impl MoverProbeChannelSection {
    /// [`Self::to_bytes`]'s bytes, part by part, from the section's
    /// structure, so a streamed payload counts the same whether or not it is
    /// held: a resident payload in its own parts, a streamed one in
    /// `streamed` and outside the total.
    pub fn byte_breakdown(&self) -> MoverChannelBytes {
        let counts = self.direct_counts();
        let visibility = counts.visibility_pairs * MOVER_VISIBILITY_BYTES_PER_PROBE;
        let own_tiles = counts.own_tile_probes * MOVER_IRRADIANCE_TILE_F16_COUNT * 2;
        let bounce_payload = self.bounce_payload_bytes() as usize;
        let resident = |block| self.placement(block) == MoverPayloadPlacement::Resident;
        let direct_resident = resident(MoverChannelBlock::Direct);
        let bounce_resident = resident(MoverChannelBlock::Bounce);
        MoverChannelBytes {
            header: HEADER_BYTES,
            cell_index: 4 + self.cells.len() * CELL_RECORD_BYTES,
            direct_index: (self.direct_offsets.len() + self.mover_offsets.len()) * 4,
            direct_records: self.direct_records.len() * DIRECT_RECORD_BYTES
                + self.direct_movers.len() * MOVER_ID_BYTES,
            visibility: if direct_resident { visibility } else { 0 },
            own_tiles: if direct_resident { own_tiles } else { 0 },
            bounce_index: self.bounce_offsets.len() * 4
                + self.bounce_entries.len() * BOUNCE_ENTRY_BYTES,
            bounce_payload: if bounce_resident { bounce_payload } else { 0 },
            depth: 4
                + self.depth_records.len() * DEPTH_RECORD_BYTES
                + self.depth_tiles.len() * PROBE_DEPTH_TILE_TEXEL_BYTES as usize,
            ramps: self.ramps.len() * RAMP_RECORD_BYTES,
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
    debug_assert_eq!(section.ramps.len(), section.mover_ids().len());
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
    if direct_resident {
        out.extend_from_slice(&section.visibility);
        put_halves(&mut out, &section.own_tiles);
    }

    put_u32s(&mut out, &section.bounce_offsets);
    for entry in &section.bounce_entries {
        out.extend_from_slice(&entry.mover_id.to_le_bytes());
        out.extend_from_slice(&0u32.to_le_bytes());
        for mask in entry.stored_probe_masks {
            out.extend_from_slice(&mask.to_le_bytes());
        }
    }
    if bounce_resident {
        put_halves(&mut out, &section.bounce_tiles);
    }

    out.extend_from_slice(&(section.depth_records.len() as u32).to_le_bytes());
    for record in &section.depth_records {
        out.extend_from_slice(&record.mover_id.to_le_bytes());
        out.extend_from_slice(&record.probe_index.to_le_bytes());
        out.extend_from_slice(&termini_bits(record.stored_termini).to_le_bytes());
    }
    for [mean, mean_sq] in &section.depth_tiles {
        out.extend_from_slice(&mean.to_le_bytes());
        out.extend_from_slice(&mean_sq.to_le_bytes());
    }
    for ramp in &section.ramps {
        out.extend_from_slice(&ramp.mover_id.to_le_bytes());
        out.extend_from_slice(&ramp.start.to_le_bytes());
        out.extend_from_slice(&ramp.end.to_le_bytes());
    }
    debug_assert_eq!(out.len(), section.encoded_len());
    out
}

/// A depth record's stored termini as wire bits: bit k stores `D_k`.
fn termini_bits(stored: [bool; 2]) -> u32 {
    u32::from(stored[0]) | u32::from(stored[1]) << 1
}

fn put_u32s(out: &mut Vec<u8>, values: &[u32]) {
    for value in values {
        out.extend_from_slice(&value.to_le_bytes());
    }
}

pub(super) fn put_halves(out: &mut Vec<u8>, halves: &[u16]) {
    for half in halves {
        out.extend_from_slice(&half.to_le_bytes());
    }
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
        visibility_len = checked_add(
            visibility_len,
            movers.len() * probes * MOVER_VISIBILITY_BYTES_PER_PROBE,
            "visibility payload",
        )?;
        if record.carries_own_tile {
            own_tile_halves = checked_add(
                own_tile_halves,
                probes * MOVER_IRRADIANCE_TILE_F16_COUNT,
                "own-tile payload",
            )?;
        }
    }
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
    let mut bounce_entries = Vec::new();
    for (index, bytes) in reader
        .records(bounce_count, BOUNCE_ENTRY_BYTES, "bounce entries")?
        .enumerate()
    {
        let reserved = le_u32(&bytes[4..8]);
        if reserved != 0 {
            return Err(Error::ReservedBits {
                what: "bounce entry reserved u32",
                index,
                value: reserved,
            });
        }
        bounce_entries.push(MoverBounceEntry {
            mover_id: le_u32(&bytes[0..4]),
            stored_probe_masks: [le_u64(&bytes[8..16]), le_u64(&bytes[16..24])],
        });
    }
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
        check_mask(
            "bounce",
            cells[position],
            index,
            entry.affected_probe_mask(),
        )?;
    }
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
        MoverPayloadPlacement::Streamed => Vec::new(),
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
    let mut depth_records: Vec<MoverDepthRecord> = Vec::new();
    for (record, bytes) in reader
        .records(depth_count, DEPTH_RECORD_BYTES, "depth records")?
        .enumerate()
    {
        let termini = le_u32(&bytes[8..12]);
        if termini & !DEPTH_TERMINI_BITS != 0 {
            return Err(Error::ReservedBits {
                what: "depth record stored-termini bits",
                index: record,
                value: termini,
            });
        }
        if termini == 0 {
            return Err(Error::DepthRecordStoresNothing { record });
        }
        depth_records.push(MoverDepthRecord {
            mover_id: le_u32(&bytes[0..4]),
            probe_index: le_u32(&bytes[4..8]),
            stored_termini: [termini & 1 != 0, termini & 2 != 0],
        });
    }
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

    let mut section = MoverProbeChannelSection {
        affinity_factor,
        affinity_dims,
        direct_placement,
        bounce_placement,
        cells,
        direct_offsets,
        direct_records,
        mover_offsets,
        direct_movers,
        visibility,
        own_tiles,
        bounce_offsets,
        bounce_entries,
        bounce_tiles,
        depth_records,
        depth_tiles,
        ramps: Vec::new(),
    };

    // Ramp records: exactly the movers named above, ascending.
    let movers = section.mover_ids();
    let mut ramps = Vec::with_capacity(movers.len());
    for (record, (bytes, &expected)) in reader
        .records(movers.len(), RAMP_RECORD_BYTES, "ramp records")?
        .zip(&movers)
        .enumerate()
    {
        let mover_id = le_u32(&bytes[0..4]);
        if mover_id != expected {
            return Err(Error::RampMover {
                record,
                found: mover_id,
                expected,
            });
        }
        let start = f32::from_le_bytes(bytes[4..8].try_into().unwrap());
        let end = f32::from_le_bytes(bytes[8..12].try_into().unwrap());
        // Every comparison is false for NaN, and infinities fail the bounds.
        if !(start >= 0.0 && start < end && end <= 1.0) {
            return Err(Error::RampRange {
                mover_id,
                start,
                end,
            });
        }
        ramps.push(MoverLightRampRecord {
            mover_id,
            start,
            end,
        });
    }
    section.ramps = ramps;

    let trailing = data.len() - reader.at;
    if trailing != 0 {
        return Err(Error::TrailingBytes { count: trailing });
    }
    Ok(section)
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

/// A record's or bounce entry's mask is a non-empty subset of its cell's
/// valid probes.
fn check_mask(
    block: &'static str,
    cell: MoverChannelCell,
    entry: usize,
    mask: u64,
) -> Result<(), Error> {
    if mask == 0 {
        return Err(Error::ProbeMaskEmpty {
            block,
            cell: cell.cell as usize,
            entry,
        });
    }
    if mask & !cell.valid_probe_mask != 0 {
        return Err(Error::ProbeMaskNotValid {
            block,
            cell: cell.cell as usize,
            entry,
            mask,
            valid: cell.valid_probe_mask,
        });
    }
    Ok(())
}

/// Each record's mask is exactly its movers' `q < 255` probes: every masked
/// probe has a mover below 255 at either terminus, and every mover is below
/// 255 somewhere.
pub(super) fn check_masks_are_tight(
    cells: &[MoverChannelCell],
    direct_offsets: &[u32],
    records: &[MoverDirectRecord],
    mover_offsets: &[u32],
    movers: &[u32],
    visibility: &[u8],
) -> Result<(), Error> {
    let mut at = 0;
    for (index, (position, record)) in entry_positions(direct_offsets).zip(records).enumerate() {
        let cell = cells[position].cell as usize;
        let per_mover = record.masked_probe_count() * MOVER_VISIBILITY_BYTES_PER_PROBE;
        let first = mover_offsets[index] as usize;
        let count = mover_offsets[index + 1] as usize - first;
        let payload = &visibility[at..at + count * per_mover];
        at += count * per_mover;
        for (entry, q) in payload.chunks_exact(per_mover).enumerate() {
            if q.iter().all(|&byte| byte == u8::MAX) {
                return Err(Error::MoverGatesNothing {
                    cell,
                    record: index,
                    entry: first + entry,
                    mover_id: movers[first + entry],
                });
            }
        }
        let mut bits = record.probe_mask;
        let mut slot = 0;
        while bits != 0 {
            let local = bits.trailing_zeros();
            bits &= bits - 1;
            let gated = payload.chunks_exact(per_mover).any(|q| {
                q[slot * MOVER_VISIBILITY_BYTES_PER_PROBE..][..MOVER_VISIBILITY_BYTES_PER_PROBE]
                    .iter()
                    .any(|&byte| byte < u8::MAX)
            });
            if !gated {
                return Err(Error::ProbeMaskNotTight {
                    cell,
                    record: index,
                    probe: local,
                });
            }
            slot += 1;
        }
    }
    Ok(())
}

/// Entries of each listed cell ascend strictly by `key`: ordered, no
/// duplicates.
fn check_cell_order<T, K: Ord>(
    block: &'static str,
    offsets: &[u32],
    cells: &[MoverChannelCell],
    entries: &[T],
    key: impl Fn(&T) -> K,
) -> Result<(), Error> {
    for (position, range) in offsets.windows(2).enumerate() {
        let (start, end) = (range[0] as usize, range[1] as usize);
        if let Some(at) = entries[start..end]
            .windows(2)
            .position(|pair| key(&pair[0]) >= key(&pair[1]))
        {
            return Err(Error::EntryOrder {
                block,
                cell: cells[position].cell as usize,
                entry: start + at + 1,
            });
        }
    }
    Ok(())
}

fn checked_add(total: usize, add: usize, what: &'static str) -> Result<usize, Error> {
    total.checked_add(add).ok_or(Error::SizeOverflow { what })
}

fn le_u32(bytes: &[u8]) -> u32 {
    u32::from_le_bytes(bytes.try_into().unwrap())
}

fn le_u64(bytes: &[u8]) -> u64 {
    u64::from_le_bytes(bytes.try_into().unwrap())
}

/// A cursor that refuses to read past the end, so no count read from the
/// file allocates before its bytes are known to exist.
struct Reader<'a> {
    data: &'a [u8],
    at: usize,
}

impl<'a> Reader<'a> {
    fn take(&mut self, len: usize, what: &'static str) -> Result<&'a [u8], Error> {
        let remaining = self.data.len() - self.at;
        if len > remaining {
            return Err(Error::Truncated {
                what,
                needed: len,
                remaining,
            });
        }
        let bytes = &self.data[self.at..self.at + len];
        self.at += len;
        Ok(bytes)
    }

    fn u8(&mut self, what: &'static str) -> Result<u8, Error> {
        Ok(self.take(1, what)?[0])
    }

    fn u32(&mut self, what: &'static str) -> Result<u32, Error> {
        Ok(le_u32(self.take(4, what)?))
    }

    /// `count` fixed-size records, length-checked as one read.
    fn records(
        &mut self,
        count: usize,
        size: usize,
        what: &'static str,
    ) -> Result<std::slice::ChunksExact<'a, u8>, Error> {
        let len = count
            .checked_mul(size)
            .ok_or(Error::SizeOverflow { what })?;
        Ok(self.take(len, what)?.chunks_exact(size))
    }

    fn halves(&mut self, count: usize, what: &'static str) -> Result<Vec<u16>, Error> {
        Ok(self
            .records(count, 2, what)?
            .map(|half| u16::from_le_bytes([half[0], half[1]]))
            .collect())
    }

    /// A CSR offset table over `rows`: starts at 0, never decreases.
    fn offsets(
        &mut self,
        rows: usize,
        block: &'static str,
        what: &'static str,
    ) -> Result<Vec<u32>, Error> {
        let count = rows.checked_add(1).ok_or(Error::SizeOverflow { what })?;
        let offsets: Vec<u32> = self.records(count, 4, what)?.map(le_u32).collect();
        if offsets[0] != 0 {
            return Err(Error::CsrOffsets {
                block,
                index: 0,
                offset: offsets[0],
                previous: 0,
            });
        }
        if let Some(index) = offsets.windows(2).position(|pair| pair[0] > pair[1]) {
            return Err(Error::CsrOffsets {
                block,
                index: index + 1,
                offset: offsets[index + 1],
                previous: offsets[index],
            });
        }
        Ok(offsets)
    }
}
