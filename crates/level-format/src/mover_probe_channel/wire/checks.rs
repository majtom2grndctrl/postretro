// MoverProbeChannel (id 57) cross-entry checks: masks within their cell's
// valid probes, direct masks tight against every pose's q (255 at every
// across pose), and entries in strict order within each listed cell.
// See: context/lib/build_pipeline.md §PRL section IDs

use super::Error;
use super::across::across_entry;
use crate::mover_probe_channel::{
    MoverAcrossEntry, MoverChannelCell, MoverDirectRecord, entry_positions,
};

/// A record's or bounce entry's mask is a non-empty subset of its cell's
/// valid probes.
pub(super) fn check_mask(
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
/// probe has a mover below 255 at some pose, and every mover is below 255
/// somewhere. A pose `across` marks for a mover's probe stores 255 there,
/// so the bytes are canonical and the mask counts only poses compose reads.
/// `pose_count` gives each mover's poses.
#[allow(clippy::too_many_arguments)]
pub(in crate::mover_probe_channel) fn check_masks_are_tight(
    cells: &[MoverChannelCell],
    direct_offsets: &[u32],
    records: &[MoverDirectRecord],
    mover_offsets: &[u32],
    movers: &[u32],
    visibility: &[u8],
    across: &[MoverAcrossEntry],
    pose_count: impl Fn(u32) -> usize,
) -> Result<(), Error> {
    let mut at = 0;
    // Each mover's q bytes under one record, with its pose count (N per
    // masked probe) and its across marks at the cell.
    let mut payload: Vec<(u32, &[u8], usize, Option<&MoverAcrossEntry>)> = Vec::new();
    for (index, (position, record)) in entry_positions(direct_offsets).zip(records).enumerate() {
        let cell = cells[position].cell as usize;
        let probes = record.masked_probe_count();
        let first = mover_offsets[index] as usize;
        payload.clear();
        for (entry, &mover_id) in movers[first..mover_offsets[index + 1] as usize]
            .iter()
            .enumerate()
        {
            let poses = pose_count(mover_id);
            let q = &visibility[at..at + probes * poses];
            at += probes * poses;
            if q.iter().all(|&byte| byte == u8::MAX) {
                return Err(Error::MoverGatesNothing {
                    cell,
                    record: index,
                    entry: first + entry,
                    mover_id,
                });
            }
            payload.push((mover_id, q, poses, across_entry(across, position, mover_id)));
        }
        let mut bits = record.probe_mask;
        let mut slot = 0;
        while bits != 0 {
            let local = bits.trailing_zeros();
            bits &= bits - 1;
            for &(mover_id, q, poses, marks) in &payload {
                let Some(marks) = marks else {
                    continue;
                };
                let probe_q = &q[slot * poses..(slot + 1) * poses];
                if let Some(pose) = (1..poses - 1)
                    .find(|&pose| marks.is_across(pose, local) && probe_q[pose] != u8::MAX)
                {
                    return Err(Error::AcrossVisibility {
                        cell,
                        record: index,
                        mover_id,
                        probe: local,
                        pose,
                        q: probe_q[pose],
                    });
                }
            }
            let gated = payload.iter().any(|&(_, q, poses, _)| {
                q[slot * poses..(slot + 1) * poses]
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
pub(super) fn check_cell_order<T, K: Ord>(
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
