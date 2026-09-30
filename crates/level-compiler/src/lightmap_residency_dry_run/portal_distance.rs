//! Checks the untruncated hub-metric recompute against stored id-46 records.
//!
//! The recompute itself (`cell_residency_bake::portal_distance`) keeps every
//! partner within a bound; id 46 keeps only each cell's
//! `CELL_VISIBILITY_FANOUT_K` nearest. Inside the bound the two must agree.

use postretro_level_format::cell_visibility::CoupledPairRecord;

/// Fixed-point slack for f32 section bounds versus the bake's f64 leaves.
pub(crate) const VALIDATION_TOLERANCE_FIXED: u32 = 2;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct DistanceValidation {
    /// Stored id-46 records within the recompute bound.
    pub checked: usize,
    pub matched: usize,
    /// Found by the recompute but more than the tolerance apart.
    pub mismatched: usize,
    /// Stored records the recompute found no path for within the bound.
    pub missing: usize,
    pub max_abs_diff: u32,
}

/// Compare stored id-46 records inside `max_fixed` with the recompute.
pub(crate) fn validate_against_stored(
    stored: &[CoupledPairRecord],
    recomputed: &[CoupledPairRecord],
    max_fixed: u32,
) -> DistanceValidation {
    let mut result = DistanceValidation::default();
    for pair in stored.iter().filter(|pair| pair.distance <= max_fixed) {
        result.checked += 1;
        match recomputed.binary_search_by_key(&(pair.cell_a, pair.cell_b), |r| (r.cell_a, r.cell_b))
        {
            Ok(index) => {
                let diff = recomputed[index].distance.abs_diff(pair.distance);
                result.max_abs_diff = result.max_abs_diff.max(diff);
                if diff <= VALIDATION_TOLERANCE_FIXED {
                    result.matched += 1;
                } else {
                    result.mismatched += 1;
                }
            }
            Err(_) => result.missing += 1,
        }
    }
    result
}

impl DistanceValidation {
    pub(crate) fn all_matched(&self) -> bool {
        self.matched == self.checked
    }
}
