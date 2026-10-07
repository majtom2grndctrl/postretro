//! Affinity-row contributor counts and the resident-row unions they feed.
//!
//! Each ref table counts, per affinity row, the installed probes or sparse
//! rows that contribute to it. The resident sets are dense bitsets over the
//! row space, each the union of its tables' keys:
//! indirect is base ∪ id-27; Pass A is id-35 ∪ id-41; Pass B is Pass A ∪
//! id-45. Install and eviction both update these unions per touched row, so
//! neither costs time proportional to the resident map. Compose membership
//! narrows further: only entry-carrying sparse rows contribute.

use super::compose_plan::PassMembership;
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
    /// Insert `row` into `set`; returns whether it was absent.
    pub(super) fn row_set_insert(&mut self, set: RowSet, row: u32) -> bool {
        match set {
            RowSet::IndirectDirty => self.indirect_dirty_rows.insert(row),
            RowSet::DirectPromotionDirty => self.direct_promotion_dirty_rows.insert(row),
            RowSet::DirectAnimatedDirty => self.direct_animated_dirty_rows.insert(row),
            RowSet::IndirectResident => self.indirect_resident_rows.insert(row),
            RowSet::DirectPromotionResident => self.direct_promotion_resident_rows.insert(row),
            RowSet::DirectAnimatedResident => self.direct_animated_resident_rows.insert(row),
        }
    }

    pub(super) fn row_set_remove(&mut self, set: RowSet, row: u32) {
        match set {
            RowSet::IndirectDirty => {
                self.indirect_dirty_rows.remove(&row);
            }
            RowSet::DirectPromotionDirty => {
                self.direct_promotion_dirty_rows.remove(&row);
            }
            RowSet::DirectAnimatedDirty => {
                self.direct_animated_dirty_rows.remove(&row);
            }
            RowSet::IndirectResident => {
                self.indirect_resident_rows.remove(&row);
            }
            RowSet::DirectPromotionResident => {
                self.direct_promotion_resident_rows.remove(&row);
            }
            RowSet::DirectAnimatedResident => {
                self.direct_animated_resident_rows.remove(&row);
            }
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
        self.compose_membership_touched.push(row);
        for &set in table.resident_sets() {
            if !set
                .sources()
                .iter()
                .any(|&source| self.row_ref_table(source).contains_key(&row))
            {
                self.row_set_remove(set, row);
            }
        }
        Ok(())
    }

    /// Whether `row` is resident for Pass A. With id-45 present, Pass A also
    /// covers id-45-only rows so Pass B always reads a written intermediate.
    /// A level with an id-35 base but neither id-41 nor id-45 samples that
    /// base directly and never dispatches Pass A, so no row is resident for it.
    pub(super) fn direct_compose_resident(&self, row: u32) -> bool {
        self.direct_compose_required
            && (self.direct_promotion_resident_rows.contains(&row)
                || (self.animated_direct_compose_required
                    && self.direct_animated_resident_rows.contains(&row)))
    }

    /// Whether `row`'s installed sparse row in `section_id` carries at least
    /// one CSR entry. This reads the pool's row pair, the value compose
    /// reads; zero-entry and evicted rows both hold the `[0, 0]` sentinel.
    pub(super) fn sparse_row_has_entries(&self, section_id: u32, row: u32) -> bool {
        self.sparse_pools
            .get(&section_id)
            .and_then(|pool| pool.row_pairs.get(row as usize))
            .is_some_and(|&[start, end]| end > start)
    }

    /// Whether `row` contributes to the pass fed by `table`'s section. The
    /// compiler emits a sparse row for every covered brick, but a row with
    /// no entries composes to an animation-independent value, so it is
    /// resident and dirty-tracked without joining the per-source trigger.
    /// Entry counts are static per level, so this flips only with the ref,
    /// and every ref flip already queues the row as membership-touched.
    fn sparse_row_contributes(&self, table: RowRefTable, section_id: u32, row: u32) -> bool {
        self.row_ref_table(table).contains_key(&row) && self.sparse_row_has_entries(section_id, row)
    }

    /// A row's current compose-planner membership, from the resident unions
    /// and entry-carrying sparse rows. A pass the level never dispatches
    /// reports no membership, so the planner neither tracks nor retries its
    /// rows.
    pub(super) fn compose_row_membership(&self, row: u32) -> RowMembership {
        let direct_resident = self.direct_compose_resident(row);
        let static_contributing =
            self.sparse_row_contributes(RowRefTable::DirectPromotion, DIRECT_DELTA_ID, row);
        let animated_direct = if self.animated_direct_compose_required {
            PassMembership {
                resident: direct_resident,
                contributing: self.sparse_row_contributes(
                    RowRefTable::DirectAnimated,
                    ANIMATED_DIRECT_DELTA_ID,
                    row,
                ),
                upstream: static_contributing,
            }
        } else {
            PassMembership::default()
        };
        RowMembership {
            indirect: PassMembership {
                resident: self.indirect_resident_rows.contains(&row),
                contributing: self.sparse_row_contributes(
                    RowRefTable::IndirectDelta,
                    INDIRECT_DELTA_ID,
                    row,
                ),
                upstream: false,
            },
            static_direct: PassMembership {
                resident: direct_resident,
                contributing: static_contributing,
                upstream: false,
            },
            animated_direct,
        }
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
            self.indirect_resident_rows.iter().collect(),
            self.direct_promotion_resident_rows.iter().collect(),
            self.direct_animated_resident_rows.iter().collect(),
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
