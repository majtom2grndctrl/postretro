//! Affinity-row contributor counts and the resident-row unions they feed.
//!
//! Each ref table counts, per affinity row, the installed probes or sparse
//! rows that contribute to it. The resident sets are unions of table keys:
//! indirect is base ∪ id-27; Pass A is id-35 ∪ id-41; Pass B is Pass A ∪
//! id-45. Install and eviction both update these unions per touched row, so
//! neither costs time proportional to the resident map.

use super::*;

/// Affinity-row sets an install or eviction can touch.
#[derive(Debug, Clone, Copy)]
pub(super) enum RowSet {
    IndirectDirty,
    IndirectResident,
    DirectPromotionDirty,
    DirectPromotionResident,
    DirectAnimatedDirty,
    DirectAnimatedResident,
}

/// Per-row contributor counts.
#[derive(Debug, Clone, Copy)]
pub(super) enum RowRefTable {
    IndirectBase,
    IndirectDelta,
    DirectBase,
    DirectPromotion,
    DirectAnimated,
}

impl RowRefTable {
    /// Resident sets that hold every row this table references.
    pub(super) const fn resident_sets(self) -> &'static [RowSet] {
        match self {
            Self::IndirectBase | Self::IndirectDelta => &[RowSet::IndirectResident],
            Self::DirectBase | Self::DirectPromotion => &[
                RowSet::DirectPromotionResident,
                RowSet::DirectAnimatedResident,
            ],
            Self::DirectAnimated => &[RowSet::DirectAnimatedResident],
        }
    }
}

impl RowSet {
    /// Ref tables whose keys this resident set is the union of. Dirty sets
    /// have no sources.
    const fn sources(self) -> &'static [RowRefTable] {
        match self {
            Self::IndirectResident => &[RowRefTable::IndirectBase, RowRefTable::IndirectDelta],
            Self::DirectPromotionResident => {
                &[RowRefTable::DirectBase, RowRefTable::DirectPromotion]
            }
            Self::DirectAnimatedResident => &[
                RowRefTable::DirectBase,
                RowRefTable::DirectPromotion,
                RowRefTable::DirectAnimated,
            ],
            Self::IndirectDirty | Self::DirectPromotionDirty | Self::DirectAnimatedDirty => &[],
        }
    }
}

/// `(row, count)` for every distinct row in `rows`, ascending. A cluster's
/// probes share rows (up to 64 per 4×4×4 affinity cell), so row bookkeeping
/// runs once per row rather than once per probe.
pub(super) fn row_counts(mut rows: Vec<u32>) -> Vec<(u32, u32)> {
    rows.sort_unstable();
    let mut counts: Vec<(u32, u32)> = Vec::new();
    for row in rows {
        match counts.last_mut() {
            Some((last, count)) if *last == row => *count += 1,
            _ => counts.push((row, 1)),
        }
    }
    counts
}

pub(super) fn increment_row_ref(
    refs: &mut BTreeMap<u32, u32>,
    row: u32,
    count: u32,
) -> Result<(), ShResidencyDrainError> {
    let current = refs.entry(row).or_insert(0);
    *current = current
        .checked_add(count)
        .ok_or(ShResidencyDrainError::SlotOverflow)?;
    Ok(())
}

pub(super) fn decrement_row_ref(
    refs: &mut BTreeMap<u32, u32>,
    row: u32,
    count: u32,
) -> Result<(), ShResidencyDrainError> {
    let current = refs
        .get_mut(&row)
        .ok_or(ShResidencyDrainError::SlotOverflow)?;
    *current = current
        .checked_sub(count)
        .ok_or(ShResidencyDrainError::SlotOverflow)?;
    if *current == 0 {
        refs.remove(&row);
    }
    Ok(())
}

impl ShResidencyState {
    pub(super) fn row_set_mut(&mut self, set: RowSet) -> &mut BTreeSet<u32> {
        match set {
            RowSet::IndirectDirty => &mut self.indirect_dirty_rows,
            RowSet::IndirectResident => &mut self.indirect_resident_rows,
            RowSet::DirectPromotionDirty => &mut self.direct_promotion_dirty_rows,
            RowSet::DirectPromotionResident => &mut self.direct_promotion_resident_rows,
            RowSet::DirectAnimatedDirty => &mut self.direct_animated_dirty_rows,
            RowSet::DirectAnimatedResident => &mut self.direct_animated_resident_rows,
        }
    }

    fn row_ref_table(&self, table: RowRefTable) -> &BTreeMap<u32, u32> {
        match table {
            RowRefTable::IndirectBase => &self.indirect_base_row_refs,
            RowRefTable::IndirectDelta => &self.indirect_delta_row_refs,
            RowRefTable::DirectBase => &self.direct_base_row_refs,
            RowRefTable::DirectPromotion => &self.direct_promotion_row_refs,
            RowRefTable::DirectAnimated => &self.direct_animated_row_refs,
        }
    }

    pub(super) fn row_ref_table_mut(&mut self, table: RowRefTable) -> &mut BTreeMap<u32, u32> {
        match table {
            RowRefTable::IndirectBase => &mut self.indirect_base_row_refs,
            RowRefTable::IndirectDelta => &mut self.indirect_delta_row_refs,
            RowRefTable::DirectBase => &mut self.direct_base_row_refs,
            RowRefTable::DirectPromotion => &mut self.direct_promotion_row_refs,
            RowRefTable::DirectAnimated => &mut self.direct_animated_row_refs,
        }
    }

    /// Release `count` contributors of `row` from `table`. When the table's
    /// last contributor leaves, drop the row from each resident union that
    /// no other source table still feeds.
    pub(super) fn release_row_refs(
        &mut self,
        table: RowRefTable,
        row: u32,
        count: u32,
    ) -> Result<(), ShResidencyDrainError> {
        decrement_row_ref(self.row_ref_table_mut(table), row, count)?;
        if self.row_ref_table(table).contains_key(&row) {
            return Ok(());
        }
        for &set in table.resident_sets() {
            if !set
                .sources()
                .iter()
                .any(|&source| self.row_ref_table(source).contains_key(&row))
            {
                self.row_set_mut(set).remove(&row);
            }
        }
        Ok(())
    }

    /// Rebuild every resident union from the ref tables. A test oracle for
    /// the incremental install and eviction paths.
    #[cfg(test)]
    pub(super) fn rebuilt_resident_rows(&self) -> [BTreeSet<u32>; 3] {
        let keys = |sources: &[RowRefTable]| -> BTreeSet<u32> {
            sources
                .iter()
                .flat_map(|&source| self.row_ref_table(source).keys().copied())
                .collect()
        };
        [
            keys(RowSet::IndirectResident.sources()),
            keys(RowSet::DirectPromotionResident.sources()),
            keys(RowSet::DirectAnimatedResident.sources()),
        ]
    }

    #[cfg(test)]
    pub(super) fn resident_rows(&self) -> [BTreeSet<u32>; 3] {
        [
            self.indirect_resident_rows.clone(),
            self.direct_promotion_resident_rows.clone(),
            self.direct_animated_resident_rows.clone(),
        ]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn row_counts_group_repeated_rows_in_ascending_order() {
        assert_eq!(row_counts(vec![7, 2, 7, 7, 3, 2]), [(2, 2), (3, 1), (7, 3)]);
        assert!(row_counts(Vec::new()).is_empty());
    }

    #[test]
    fn a_row_ref_count_cannot_underflow() {
        let mut refs = BTreeMap::from([(4, 2)]);
        assert_eq!(
            decrement_row_ref(&mut refs, 4, 3),
            Err(ShResidencyDrainError::SlotOverflow)
        );
        decrement_row_ref(&mut refs, 4, 2).unwrap();
        assert!(refs.is_empty());
    }
}
