// MoverProbeChannel (id 57) across marks: per (listed cell, mover), the
// probes each mid pose encloses, which direct compose blends across. Read
// against each mover's pose count, and checked tight against the probes the
// mover's direct records and bounce entry name at that cell.
// See: context/lib/build_pipeline.md §PRL section IDs

use super::reader::{Reader, le_u32, le_u64, put_u32s};
use super::{Error, MoverBounceEntry, MoverChannelCell, MoverDirectRecord, MoverRecord};
use crate::mover_probe_channel::MoverAcrossEntry;

/// u32 listed cell, u32 mover, u32 mid-pose count; one u64 mask per mid
/// pose follows.
const ACROSS_ENTRY_HEAD_BYTES: usize = 12;
const MASK_BYTES: usize = 8;

/// Whether the across block is written: exactly when some mover has a mid
/// pose. With none, no probe can be marked across, so a section of two-pose
/// movers packs exactly as it did before across marks existed.
pub(super) fn has_across_block(movers: &[MoverRecord]) -> bool {
    movers.iter().any(|record| record.pose_count() > 2)
}

/// The across block's bytes: its count, then each entry; nothing when the
/// block is absent.
pub(super) fn across_block_bytes(movers: &[MoverRecord], entries: &[MoverAcrossEntry]) -> usize {
    if !has_across_block(movers) {
        debug_assert!(entries.is_empty(), "two-pose movers mark nothing across");
        return 0;
    }
    4 + entries
        .iter()
        .map(|entry| ACROSS_ENTRY_HEAD_BYTES + MASK_BYTES * entry.mid_poses.len())
        .sum::<usize>()
}

pub(super) fn put_across_entries(
    out: &mut Vec<u8>,
    movers: &[MoverRecord],
    entries: &[MoverAcrossEntry],
) {
    if !has_across_block(movers) {
        return;
    }
    out.extend_from_slice(&(entries.len() as u32).to_le_bytes());
    for entry in entries {
        put_u32s(
            out,
            &[entry.listed, entry.mover_id, entry.mid_poses.len() as u32],
        );
        for mask in &entry.mid_poses {
            out.extend_from_slice(&mask.to_le_bytes());
        }
    }
}

/// The across entries, where `movers` has a mid pose (else the block is
/// absent): ascending by (listed cell, mover) with no repeats, each naming
/// a listed cell and a mover with a record, holding exactly one mask per
/// mid pose of that mover, marking some probe, and marking only the cell's
/// valid probes.
pub(super) fn read_across_entries(
    reader: &mut Reader<'_>,
    cells: &[MoverChannelCell],
    movers: &[MoverRecord],
    pose_count: impl Fn(&'static str, u32) -> Result<usize, Error>,
) -> Result<Vec<MoverAcrossEntry>, Error> {
    if !has_across_block(movers) {
        return Ok(Vec::new());
    }
    let count = reader.u32("across entry count")? as usize;
    // Each entry takes at least its head, so a count past the bytes fails on
    // its first short read, never on an allocation.
    let mut entries: Vec<MoverAcrossEntry> = Vec::new();
    for index in 0..count {
        let head = reader.take(ACROSS_ENTRY_HEAD_BYTES, "across entries")?;
        let (listed, mover_id) = (le_u32(&head[0..4]), le_u32(&head[4..8]));
        let found = le_u32(&head[8..12]);
        let Some(cell) = cells.get(listed as usize) else {
            return Err(Error::AcrossCellOutOfRange {
                entry: index,
                listed,
                cells: cells.len(),
            });
        };
        if entries
            .last()
            .is_some_and(|previous| previous.order_key() >= (listed, mover_id))
        {
            return Err(Error::AcrossOrder { entry: index });
        }
        let expected = pose_count("across", mover_id)? - 2;
        if found as usize != expected {
            return Err(Error::AcrossMidPoseCount {
                entry: index,
                mover_id,
                found,
                expected,
            });
        }
        let mid_poses: Vec<u64> = reader
            .records(expected, MASK_BYTES, "across masks")?
            .map(le_u64)
            .collect();
        let entry = MoverAcrossEntry {
            listed,
            mover_id,
            mid_poses,
        };
        if entry.marked_probes() == 0 {
            return Err(Error::AcrossEntryEmpty {
                entry: index,
                mover_id,
            });
        }
        for (mid, &mask) in entry.mid_poses.iter().enumerate() {
            if mask & !cell.valid_probe_mask != 0 {
                return Err(Error::AcrossMarkNotValid {
                    cell: cell.cell as usize,
                    entry: index,
                    pose: mid + 1,
                    mask,
                    valid: cell.valid_probe_mask,
                });
            }
        }
        entries.push(entry);
    }
    Ok(entries)
}

/// The direct and bounce blocks an across check reads, as decoded.
pub(super) struct NamingBlocks<'a> {
    pub(super) cells: &'a [MoverChannelCell],
    pub(super) direct_offsets: &'a [u32],
    pub(super) direct_records: &'a [MoverDirectRecord],
    pub(super) mover_offsets: &'a [u32],
    pub(super) direct_movers: &'a [u32],
    pub(super) bounce_offsets: &'a [u32],
    pub(super) bounce_entries: &'a [MoverBounceEntry],
}

/// Every across mark names a probe some direct record or the bounce entry
/// of its mover names at its cell, and the bounce entry neither stores nor
/// reads the base at a marked pose: an enclosed mid pose holds no value
/// state there.
pub(super) fn check_across_named(
    blocks: &NamingBlocks<'_>,
    entries: &[MoverAcrossEntry],
) -> Result<(), Error> {
    for (index, entry) in entries.iter().enumerate() {
        let position = entry.listed as usize;
        let cell = blocks.cells[position].cell as usize;
        let records =
            blocks.direct_offsets[position] as usize..blocks.direct_offsets[position + 1] as usize;
        let mut named = 0u64;
        for record in records {
            let movers = &blocks.direct_movers
                [blocks.mover_offsets[record] as usize..blocks.mover_offsets[record + 1] as usize];
            if movers.binary_search(&entry.mover_id).is_ok() {
                named |= blocks.direct_records[record].probe_mask;
            }
        }
        let bounce = &blocks.bounce_entries[blocks.bounce_offsets[position] as usize
            ..blocks.bounce_offsets[position + 1] as usize];
        let bounce = bounce
            .binary_search_by_key(&entry.mover_id, |bounce| bounce.mover_id)
            .ok()
            .map(|at| &bounce[at]);
        if let Some(bounce) = bounce {
            named |= bounce.affected_probe_mask();
        }
        for (mid, &mask) in entry.mid_poses.iter().enumerate() {
            let pose = mid + 1;
            if mask & !named != 0 {
                return Err(Error::AcrossMarkNotNamed {
                    cell,
                    entry: index,
                    pose,
                    probe: (mask & !named).trailing_zeros(),
                    mover_id: entry.mover_id,
                });
            }
            let held = bounce.map_or(0, |bounce| {
                let masks = bounce.poses[pose];
                masks.stored | masks.base
            });
            if mask & held != 0 {
                return Err(Error::AcrossBounceState {
                    cell,
                    entry: index,
                    pose,
                    probe: (mask & held).trailing_zeros(),
                    mover_id: entry.mover_id,
                });
            }
        }
    }
    Ok(())
}

/// The across entry of `mover_id` at listed cell `position`, if any.
pub(in crate::mover_probe_channel) fn across_entry(
    entries: &[MoverAcrossEntry],
    position: usize,
    mover_id: u32,
) -> Option<&MoverAcrossEntry> {
    let key = (u32::try_from(position).ok()?, mover_id);
    entries
        .binary_search_by_key(&key, MoverAcrossEntry::order_key)
        .ok()
        .map(|at| &entries[at])
}
