// MoverProbeChannel (id 57) bytes: encode, and the structural decode that
// pins every count, order, reserved field and length of the layout.
// See: context/lib/build_pipeline.md §PRL section IDs

use super::{
    MOVER_ENTRY_FLAG_OWN_TILE, MOVER_IRRADIANCE_TILE_F16_COUNT, MOVER_PROBE_CHANNEL_EPOCH,
    MOVER_VISIBILITY_BYTES_PER_PROBE, MoverBounceEntry, MoverBouncePlacement, MoverDepthRecord,
    MoverDirectEntry, MoverLightNamespace, MoverLightRampRecord, MoverProbeChannelError,
    MoverProbeChannelSection, bounce_entry_f16_count, entry_cells,
};
use crate::delta_sh_volumes::AFFINITY_FACTOR;
use crate::probe_depth_tiles::{PROBE_DEPTH_TILE_TEXEL_BYTES, PROBE_DEPTH_TILE_TEXELS};

type Error = MoverProbeChannelError;

/// epoch, affinity factor, affinity dims, bounce placement.
const HEADER_BYTES: usize = 1 + 1 + 12 + 1;
const DIRECT_ENTRY_BYTES: usize = 12;
const BOUNCE_ENTRY_BYTES: usize = 16;
const DEPTH_RECORD_BYTES: usize = 8;
const RAMP_RECORD_BYTES: usize = 12;
const DEPTH_TILE_PAIR_BYTES: usize =
    2 * PROBE_DEPTH_TILE_TEXELS * PROBE_DEPTH_TILE_TEXEL_BYTES as usize;

pub(super) fn encoded_len(section: &MoverProbeChannelSection) -> usize {
    HEADER_BYTES
        + section.valid_probe_masks.len() * 8
        + section.direct_offsets.len() * 4
        + section.direct_entries.len() * DIRECT_ENTRY_BYTES
        + section.visibility.len()
        + section.own_tiles.len() * 2
        + section.bounce_offsets.len() * 4
        + section.bounce_entries.len() * BOUNCE_ENTRY_BYTES
        + section.bounce_tiles.len() * 2
        + 4
        + section.depth_records.len() * DEPTH_RECORD_BYTES
        + section.depth_tiles.len() * PROBE_DEPTH_TILE_TEXEL_BYTES as usize
        + section.ramps.len() * RAMP_RECORD_BYTES
}

pub(super) fn to_bytes(section: &MoverProbeChannelSection) -> Vec<u8> {
    let cells = section.affinity_cell_count();
    debug_assert_eq!(section.valid_probe_masks.len(), cells);
    debug_assert_eq!(section.direct_offsets.len(), cells + 1);
    debug_assert_eq!(section.bounce_offsets.len(), cells + 1);
    debug_assert_eq!(
        section.direct_offsets.last().copied(),
        Some(section.direct_entries.len() as u32)
    );
    debug_assert_eq!(
        section.bounce_offsets.last().copied(),
        Some(section.bounce_entries.len() as u32)
    );
    debug_assert_eq!(
        section.depth_tiles.len(),
        section.depth_records.len() * 2 * PROBE_DEPTH_TILE_TEXELS
    );
    debug_assert_eq!(section.ramps.len(), section.mover_ids().len());

    let mut out = Vec::with_capacity(encoded_len(section));
    out.push(MOVER_PROBE_CHANNEL_EPOCH);
    out.push(section.affinity_factor);
    for dim in section.affinity_dims {
        out.extend_from_slice(&dim.to_le_bytes());
    }
    out.push(section.bounce_placement as u8);
    for mask in &section.valid_probe_masks {
        out.extend_from_slice(&mask.to_le_bytes());
    }
    for offset in &section.direct_offsets {
        out.extend_from_slice(&offset.to_le_bytes());
    }
    for entry in &section.direct_entries {
        out.extend_from_slice(&entry.mover_id.to_le_bytes());
        out.push(entry.namespace as u8);
        out.push(if entry.carries_own_tile {
            MOVER_ENTRY_FLAG_OWN_TILE
        } else {
            0
        });
        out.extend_from_slice(&0u16.to_le_bytes());
        out.extend_from_slice(&entry.light_index.to_le_bytes());
    }
    out.extend_from_slice(&section.visibility);
    for half in &section.own_tiles {
        out.extend_from_slice(&half.to_le_bytes());
    }
    for offset in &section.bounce_offsets {
        out.extend_from_slice(&offset.to_le_bytes());
    }
    for entry in &section.bounce_entries {
        out.extend_from_slice(&entry.mover_id.to_le_bytes());
        out.extend_from_slice(&0u32.to_le_bytes());
        out.extend_from_slice(&entry.affected_probe_mask.to_le_bytes());
    }
    for half in &section.bounce_tiles {
        out.extend_from_slice(&half.to_le_bytes());
    }
    out.extend_from_slice(&(section.depth_records.len() as u32).to_le_bytes());
    for record in &section.depth_records {
        out.extend_from_slice(&record.mover_id.to_le_bytes());
        out.extend_from_slice(&record.probe_index.to_le_bytes());
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
    debug_assert_eq!(out.len(), encoded_len(section));
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
    let cells = affinity_dims.iter().try_fold(1usize, |count, &dim| {
        count.checked_mul(dim as usize).ok_or(Error::SizeOverflow {
            what: "affinity cell count",
        })
    })?;
    let placement_byte = reader.u8("bounce placement")?;
    let bounce_placement =
        MoverBouncePlacement::from_u8(placement_byte).ok_or(Error::UnknownBouncePlacement {
            value: placement_byte,
        })?;

    let valid_probe_masks: Vec<u64> = reader
        .records(cells, 8, "valid-probe masks")?
        .map(|bytes| u64::from_le_bytes(bytes.try_into().unwrap()))
        .collect();

    // Direct block.
    let direct_offsets = reader.offsets(cells, "direct", "direct offsets")?;
    let direct_count = *direct_offsets.last().expect("cells + 1 offsets") as usize;
    let mut direct_entries = Vec::new();
    for (index, bytes) in reader
        .records(direct_count, DIRECT_ENTRY_BYTES, "direct entries")?
        .enumerate()
    {
        let namespace_byte = bytes[4];
        let namespace =
            MoverLightNamespace::from_u8(namespace_byte).ok_or(Error::UnknownLightNamespace {
                entry: index,
                value: namespace_byte,
            })?;
        let flags = bytes[5];
        if flags & !MOVER_ENTRY_FLAG_OWN_TILE != 0 {
            return Err(Error::ReservedBits {
                what: "direct entry flags",
                index,
                value: u32::from(flags),
            });
        }
        if namespace.requires_own_tile() && flags & MOVER_ENTRY_FLAG_OWN_TILE == 0 {
            return Err(Error::OwnTileRequired {
                entry: index,
                namespace,
            });
        }
        let reserved = u16::from_le_bytes([bytes[6], bytes[7]]);
        if reserved != 0 {
            return Err(Error::ReservedBits {
                what: "direct entry reserved u16",
                index,
                value: u32::from(reserved),
            });
        }
        direct_entries.push(MoverDirectEntry {
            mover_id: le_u32(&bytes[0..4]),
            namespace,
            carries_own_tile: flags & MOVER_ENTRY_FLAG_OWN_TILE != 0,
            light_index: le_u32(&bytes[8..12]),
        });
    }
    check_cell_order("direct", &direct_offsets, &direct_entries, |entry| {
        entry.order_key()
    })?;

    let mut visibility_len = 0usize;
    let mut own_tile_halves = 0usize;
    for (cell, entry) in entry_cells(&direct_offsets).zip(&direct_entries) {
        let probes = valid_probe_masks[cell].count_ones() as usize;
        visibility_len = checked_add(
            visibility_len,
            probes * MOVER_VISIBILITY_BYTES_PER_PROBE,
            "visibility payload",
        )?;
        if entry.carries_own_tile {
            own_tile_halves = checked_add(
                own_tile_halves,
                probes * MOVER_IRRADIANCE_TILE_F16_COUNT,
                "own-tile payload",
            )?;
        }
    }
    let visibility = reader.take(visibility_len, "visibility payload")?.to_vec();
    let own_tiles = reader.halves(own_tile_halves, "own-tile payload")?;

    // Bounce block.
    let bounce_offsets = reader.offsets(cells, "bounce", "bounce offsets")?;
    let bounce_count = *bounce_offsets.last().expect("cells + 1 offsets") as usize;
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
            affected_probe_mask: u64::from_le_bytes(bytes[8..16].try_into().unwrap()),
        });
    }
    check_cell_order("bounce", &bounce_offsets, &bounce_entries, |entry| {
        entry.mover_id
    })?;
    for (index, (cell, entry)) in entry_cells(&bounce_offsets)
        .zip(&bounce_entries)
        .enumerate()
    {
        let valid = valid_probe_masks[cell];
        let mask = entry.affected_probe_mask;
        if mask == 0 || mask & !valid != 0 {
            return Err(Error::BounceProbeNotValid {
                cell,
                entry: index,
                mask,
                valid,
            });
        }
    }
    let bounce_tiles = match bounce_placement {
        MoverBouncePlacement::Resident => {
            let halves = bounce_entries.iter().try_fold(0usize, |total, entry| {
                checked_add(total, bounce_entry_f16_count(entry), "bounce payload")
            })?;
            reader.halves(halves, "bounce payload")?
        }
        MoverBouncePlacement::Streamed if bounce_entries.is_empty() => {
            return Err(Error::StreamedBounceWithoutEntries);
        }
        MoverBouncePlacement::Streamed => Vec::new(),
    };

    // Depth block.
    let depth_count = reader.u32("depth record count")? as usize;
    let depth_records: Vec<MoverDepthRecord> = reader
        .records(depth_count, DEPTH_RECORD_BYTES, "depth records")?
        .map(|bytes| MoverDepthRecord {
            mover_id: le_u32(&bytes[0..4]),
            probe_index: le_u32(&bytes[4..8]),
        })
        .collect();
    if let Some(record) = depth_records.windows(2).position(|pair| pair[0] >= pair[1]) {
        return Err(Error::DepthRecordOrder { record: record + 1 });
    }
    let depth_tiles: Vec<[u16; 2]> = reader
        .records(depth_count, DEPTH_TILE_PAIR_BYTES, "depth tiles")?
        .flat_map(|pair| pair.as_chunks::<4>().0)
        .map(|&[mean_lo, mean_hi, sq_lo, sq_hi]| {
            [
                u16::from_le_bytes([mean_lo, mean_hi]),
                u16::from_le_bytes([sq_lo, sq_hi]),
            ]
        })
        .collect();

    if direct_entries.is_empty() && bounce_entries.is_empty() && depth_records.is_empty() {
        return Err(Error::Empty);
    }

    let mut section = MoverProbeChannelSection {
        affinity_factor,
        affinity_dims,
        bounce_placement,
        valid_probe_masks,
        direct_offsets,
        direct_entries,
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

/// Entries of each cell ascend strictly by `key`: ordered, no duplicates.
fn check_cell_order<T, K: Ord>(
    block: &'static str,
    offsets: &[u32],
    entries: &[T],
    key: impl Fn(&T) -> K,
) -> Result<(), Error> {
    for (cell, range) in offsets.windows(2).enumerate() {
        let (start, end) = (range[0] as usize, range[1] as usize);
        if let Some(position) = entries[start..end]
            .windows(2)
            .position(|pair| key(&pair[0]) >= key(&pair[1]))
        {
            return Err(Error::EntryOrder {
                block,
                cell,
                entry: start + position + 1,
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

    /// A CSR offset table over `cells`: starts at 0, never decreases.
    fn offsets(
        &mut self,
        cells: usize,
        block: &'static str,
        what: &'static str,
    ) -> Result<Vec<u32>, Error> {
        let count = cells.checked_add(1).ok_or(Error::SizeOverflow { what })?;
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
