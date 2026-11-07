// Id 42's plane records: presence, length and order.
// Governing context: context/lib/build_pipeline.md §PRL section IDs

use super::ShadowmaskBlockRecord;
use crate::lightmap::{
    SectionByteRange, check_adjacent, check_after_previous_block, check_blob, invalid,
};

/// What a block's plane records are checked against.
pub(super) struct PlaneBounds {
    /// Bytes of one plane at the block's extent.
    pub expected_len: u64,
    pub index_len: u64,
    pub section_len: u64,
    /// Where the previous block's planes end, or the index when none has.
    pub previous_end: u64,
}

/// The range a plane takes at `cursor`, moving it past a present plane.
pub(super) fn plane_range(cursor: &mut u64, plane: &[u8]) -> SectionByteRange {
    if plane.is_empty() {
        return SectionByteRange { offset: 0, len: 0 };
    }
    let range = SectionByteRange {
        offset: *cursor,
        len: plane.len() as u32,
    };
    *cursor += plane.len() as u64;
    range
}

/// Refuses an absent plane that keeps an offset, a present plane whose length
/// is not the block's or whose range leaves the section blobs, and present
/// planes that do not run on from the previous block's in record order.
/// Returns where this block's planes end: `previous_end` for a block with none.
pub(super) fn check_block_planes(
    block: usize,
    record: &ShadowmaskBlockRecord,
    bounds: PlaneBounds,
) -> crate::Result<u64> {
    let group_a = ("group A", record.group_a);
    let group_b = ("group B", record.group_b);
    for (what, range) in [group_a, group_b] {
        if range.len != 0 {
            check_blob(
                "shadowmask",
                block,
                what,
                range,
                bounds.expected_len,
                bounds.index_len,
                bounds.section_len,
            )?;
        } else if range.offset != 0 {
            return Err(invalid(format!(
                "shadowmask block {block} absent {what} plane keeps offset {}; an absent plane's \
                 offset is 0",
                range.offset
            )));
        }
    }
    let (first, last) = match (record.group_a.len != 0, record.group_b.len != 0) {
        (true, true) => {
            check_adjacent("shadowmask", block, group_a, group_b)?;
            (group_a, record.group_b)
        }
        (true, false) => (group_a, record.group_a),
        (false, true) => (group_b, record.group_b),
        (false, false) => return Ok(bounds.previous_end),
    };
    let end = check_after_previous_block("shadowmask", block, first, last, bounds.previous_end)?;
    if first.1.offset != bounds.previous_end {
        return Err(invalid(format!(
            "shadowmask block {block} {} blob at {} leaves a gap after the previous block's \
             blobs end at {}; present planes must be adjacent in record order",
            first.0, first.1.offset, bounds.previous_end
        )));
    }
    Ok(end)
}
