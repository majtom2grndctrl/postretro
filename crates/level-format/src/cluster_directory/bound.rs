//! The id-49 bound tail: worst mandatory SH demand of any camera cell.
//! See: context/lib/build_pipeline.md §PRL section IDs

use super::wire::{push_u32, read_u32};
use super::{
    ClusterDirectoryError, ClusterDirectorySection, ClusterResourceDomain, try_vec, u32_len,
    usize_count,
};

/// Default lead and baked maximum. A later memory tier varies lead between
/// them without a rebake.
pub const MAX_BOUND_ENTRIES: u32 = 2;
/// `worst_cell` of the entry holding the whole map: a level with no usable
/// id 51 has no camera-cell sets to take a worst cell from.
pub const WHOLE_MAP_WORST_CELL: u32 = u32::MAX;
/// lead, worst_dense_slots, worst_cell, family_count.
pub const BOUND_ENTRY_HEADER_SIZE: usize = 16;
/// resource_index, worst_entries, worst_tile_f16.
pub const BOUND_FAMILY_RECORD_SIZE: usize = 12;
const BOUND_COUNT_SIZE: usize = 4;

/// Worst demand of one sparse family. Sparse capacity is two-dimensional, so
/// one number cannot stand for it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ShMandatoryBoundFamily {
    pub resource_index: u32,
    pub worst_entries: u32,
    pub worst_tile_f16: u32,
}

/// Worst mandatory demand at one lead, in physical slots and sparse units
/// rather than source bytes. `worst_cell` is
/// [`WHOLE_MAP_WORST_CELL`] when the entry holds the whole map.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ShMandatoryBoundEntry {
    /// Id-51 fixed-point units.
    pub lead: u32,
    pub worst_dense_slots: u32,
    pub worst_cell: u32,
    /// Ascending by `resource_index`, affinity-domain resources only.
    pub families: Vec<ShMandatoryBoundFamily>,
}

pub(super) fn tail_byte_len(
    entries: &[ShMandatoryBoundEntry],
) -> Result<usize, ClusterDirectoryError> {
    entries
        .iter()
        .try_fold(BOUND_COUNT_SIZE, |total, entry| {
            entry
                .families
                .len()
                .checked_mul(BOUND_FAMILY_RECORD_SIZE)?
                .checked_add(BOUND_ENTRY_HEADER_SIZE)?
                .checked_add(total)
        })
        .ok_or(ClusterDirectoryError::SizeOverflow("bound tail bytes"))
}

pub(super) fn push_tail(
    bytes: &mut Vec<u8>,
    entries: &[ShMandatoryBoundEntry],
) -> Result<(), ClusterDirectoryError> {
    push_u32(bytes, u32_len(entries.len(), "bound count")?);
    for entry in entries {
        push_u32(bytes, entry.lead);
        push_u32(bytes, entry.worst_dense_slots);
        push_u32(bytes, entry.worst_cell);
        push_u32(bytes, u32_len(entry.families.len(), "bound family count")?);
        for family in &entry.families {
            push_u32(bytes, family.resource_index);
            push_u32(bytes, family.worst_entries);
            push_u32(bytes, family.worst_tile_f16);
        }
    }
    Ok(())
}

/// Parse the bytes past the header-counted tables. Every read is checked
/// against what remains, and counts are proven before anything allocates, so
/// a forged count fails by name instead of reading or reserving past the
/// section. Ordering and range rules live in [`validate_bound_entries`], which
/// the encoder shares.
pub(super) fn parse_tail(tail: &[u8]) -> Result<Vec<ShMandatoryBoundEntry>, ClusterDirectoryError> {
    let mut cursor = TailCursor { tail, offset: 0 };
    let bound_count = cursor.take_u32()?;
    if bound_count > MAX_BOUND_ENTRIES {
        return Err(ClusterDirectoryError::BoundCount {
            count: bound_count,
            limit: MAX_BOUND_ENTRIES,
        });
    }
    let mut entries = try_vec(bound_count, "bound entries")?;
    for _ in 0..bound_count {
        cursor.require(BOUND_ENTRY_HEADER_SIZE)?;
        let lead = cursor.take_u32()?;
        let worst_dense_slots = cursor.take_u32()?;
        let worst_cell = cursor.take_u32()?;
        let family_count = cursor.take_u32()?;
        cursor.require(
            usize_count(family_count)?
                .checked_mul(BOUND_FAMILY_RECORD_SIZE)
                .ok_or(ClusterDirectoryError::SizeOverflow("bound family bytes"))?,
        )?;
        let mut families = try_vec(family_count, "bound families")?;
        for _ in 0..family_count {
            families.push(ShMandatoryBoundFamily {
                resource_index: cursor.take_u32()?,
                worst_entries: cursor.take_u32()?,
                worst_tile_f16: cursor.take_u32()?,
            });
        }
        entries.push(ShMandatoryBoundEntry {
            lead,
            worst_dense_slots,
            worst_cell,
            families,
        });
    }
    let extra = tail.len() - cursor.offset;
    if extra != 0 {
        return Err(ClusterDirectoryError::BoundTrailingBytes { extra });
    }
    Ok(entries)
}

struct TailCursor<'a> {
    tail: &'a [u8],
    offset: usize,
}

impl TailCursor<'_> {
    fn require(&self, needed: usize) -> Result<(), ClusterDirectoryError> {
        let available = self.tail.len() - self.offset;
        if needed > available {
            return Err(ClusterDirectoryError::BoundTruncated { needed, available });
        }
        Ok(())
    }

    fn take_u32(&mut self) -> Result<u32, ClusterDirectoryError> {
        self.require(4)?;
        let value = read_u32(self.tail, self.offset);
        self.offset += 4;
        Ok(value)
    }
}

/// Ordering and range rules of the tail, shared by encode and decode through
/// `validate_structure`.
pub(super) fn validate_bound_entries(
    directory: &ClusterDirectorySection,
) -> Result<(), ClusterDirectoryError> {
    let entries = &directory.mandatory_bound;
    if entries.len() > MAX_BOUND_ENTRIES as usize {
        return Err(ClusterDirectoryError::BoundCount {
            count: u32_len(entries.len(), "bound count")?,
            limit: MAX_BOUND_ENTRIES,
        });
    }
    if entries.windows(2).any(|pair| pair[0].lead >= pair[1].lead) {
        return Err(ClusterDirectoryError::BoundLeadOrder);
    }
    for (index, entry) in entries.iter().enumerate() {
        if entry.worst_cell == WHOLE_MAP_WORST_CELL {
            if entries.len() != 1 || entry.lead != 0 {
                return Err(ClusterDirectoryError::BoundWholeMapShape {
                    entry: index,
                    lead: entry.lead,
                    entries: entries.len(),
                });
            }
        } else if entry.worst_cell >= directory.runtime_cell_count {
            return Err(ClusterDirectoryError::BoundCellOutOfRange {
                entry: index,
                cell: entry.worst_cell,
                count: directory.runtime_cell_count,
            });
        }
        if entry
            .families
            .windows(2)
            .any(|pair| pair[0].resource_index >= pair[1].resource_index)
        {
            return Err(ClusterDirectoryError::BoundFamilyOrder { entry: index });
        }
        for family in &entry.families {
            let is_affinity = directory
                .resources
                .get(family.resource_index as usize)
                .is_some_and(|resource| resource.domain == ClusterResourceDomain::AffinityCell);
            if !is_affinity {
                return Err(ClusterDirectoryError::BoundFamilyResource {
                    entry: index,
                    resource: family.resource_index,
                    count: directory.resources.len(),
                });
            }
        }
    }
    Ok(())
}
