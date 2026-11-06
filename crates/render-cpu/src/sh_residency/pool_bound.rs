// The stored mandatory bound a level's pools size from: which id-49 entry
// matches the runtime's default lead, in the pools' own units.
// See: context/lib/rendering_pipeline.md §4 (Bounded SH pool)

use std::collections::BTreeMap;

use postretro_level_format::cell_residency_set::DEFAULT_LEAD;
use postretro_level_format::cluster_directory::{ClusterDirectorySection, WHOLE_MAP_WORST_CELL};

use super::ShResidencyDrainError;

/// Worst mandatory demand at the default lead. Sparse capacity is
/// two-dimensional, so each family keeps both of its figures.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ShPoolBound {
    pub dense_slots: u32,
    /// `(worst_entries, worst_tile_f16)`, keyed by section id.
    pub sparse: BTreeMap<u32, (u32, u32)>,
}

/// The entry at `min(DEFAULT_LEAD, baked maximum)`. With no id 51, or an id 51
/// with no camera cell, the one whole-map entry instead: lead 0, `worst_cell`
/// [`WHOLE_MAP_WORST_CELL`]. Compiler and runtime share the constant, so a
/// level without that entry breaks the contract and is refused rather than
/// sized from a guess.
pub fn stored_pool_bound(
    directory: &ClusterDirectorySection,
    baked_max_lead: Option<u32>,
) -> Result<ShPoolBound, ShResidencyDrainError> {
    let lead = DEFAULT_LEAD.min(baked_max_lead.unwrap_or(0));
    let entries = &directory.mandatory_bound;
    let entry = match (entries.as_slice(), baked_max_lead) {
        // A level with id 51 but no camera cell has no set to take a worst
        // cell from either, so the whole map stands at any lead.
        ([whole], _) if whole.worst_cell == WHOLE_MAP_WORST_CELL => Some(whole),
        // Reach is gone, so a camera-cell figure would undersize the pool.
        (_, None) => None,
        (_, Some(_)) => entries.iter().find(|entry| entry.lead == lead),
    }
    .ok_or_else(|| ShResidencyDrainError::MandatoryBoundMismatch {
        expected_lead: lead,
        stored_leads: entries.iter().map(|entry| entry.lead).collect(),
    })?;
    let sparse = entry
        .families
        .iter()
        .map(|family| {
            let resource = directory
                .resources
                .get(family.resource_index as usize)
                .ok_or(ShResidencyDrainError::MalformedChunk {
                    cluster_id: 0,
                    reason: "id-49 bound family exceeds the resource table",
                })?;
            Ok((
                resource.section_id,
                (family.worst_entries, family.worst_tile_f16),
            ))
        })
        .collect::<Result<_, ShResidencyDrainError>>()?;
    Ok(ShPoolBound {
        dense_slots: entry.worst_dense_slots,
        sparse,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use postretro_level_format::cluster_directory::{
        ClusterResourceDomain, ClusterResourceRecord, ShMandatoryBoundEntry, ShMandatoryBoundFamily,
    };

    const SPARSE_SECTION: u32 = 27;

    fn directory(mandatory_bound: Vec<ShMandatoryBoundEntry>) -> ClusterDirectorySection {
        ClusterDirectorySection {
            runtime_cell_count: 4,
            primitive_limit: 0,
            cell_limit: 0,
            clusters: Vec::new(),
            resources: vec![ClusterResourceRecord {
                section_id: SPARSE_SECTION,
                domain: ClusterResourceDomain::AffinityCell,
                dimensions: [1, 1, 1],
            }],
            members: Vec::new(),
            ranges: Vec::new(),
            seam_portal_ids: Vec::new(),
            cluster_hints: Vec::new(),
            mandatory_bound,
        }
    }

    fn entry(lead: u32, worst_dense_slots: u32, worst_cell: u32) -> ShMandatoryBoundEntry {
        ShMandatoryBoundEntry {
            lead,
            worst_dense_slots,
            worst_cell,
            families: vec![ShMandatoryBoundFamily {
                resource_index: 0,
                worst_entries: worst_dense_slots + 1,
                worst_tile_f16: worst_dense_slots + 2,
            }],
        }
    }

    #[test]
    fn dense_and_sparse_read_the_same_default_lead_entry() {
        let stored = directory(vec![
            entry(DEFAULT_LEAD, 10, 0),
            entry(DEFAULT_LEAD * 2, 90, 1),
        ]);
        assert_eq!(
            stored_pool_bound(&stored, Some(DEFAULT_LEAD * 2)).unwrap(),
            ShPoolBound {
                dense_slots: 10,
                sparse: BTreeMap::from([(SPARSE_SECTION, (11, 12))]),
            }
        );
    }

    #[test]
    fn a_baked_maximum_below_the_default_lead_selects_its_single_entry() {
        let lead = DEFAULT_LEAD / 2;
        let stored = directory(vec![entry(lead, 7, 0)]);
        assert_eq!(
            stored_pool_bound(&stored, Some(lead)).unwrap().dense_slots,
            7
        );
    }

    #[test]
    fn no_id51_reads_the_whole_map_entry() {
        let stored = directory(vec![entry(0, 42, WHOLE_MAP_WORST_CELL)]);
        assert_eq!(stored_pool_bound(&stored, None).unwrap().dense_slots, 42);
    }

    // Id 51 exists on any level with usable portals, even one whose cells are
    // all solid or exterior, and the compiler stores the whole map for those.
    #[test]
    fn a_whole_map_entry_stands_when_id51_has_no_camera_cell() {
        let stored = directory(vec![entry(0, 42, WHOLE_MAP_WORST_CELL)]);
        assert_eq!(
            stored_pool_bound(&stored, Some(DEFAULT_LEAD * 2))
                .unwrap()
                .dense_slots,
            42
        );
    }

    #[test]
    fn a_stored_lead_off_the_default_is_refused_by_name() {
        let stored = directory(vec![entry(DEFAULT_LEAD + 1, 10, 0)]);
        let error = stored_pool_bound(&stored, Some(DEFAULT_LEAD * 2)).unwrap_err();
        assert_eq!(
            error,
            ShResidencyDrainError::MandatoryBoundMismatch {
                expected_lead: DEFAULT_LEAD,
                stored_leads: vec![DEFAULT_LEAD + 1],
            }
        );
        assert!(error.to_string().contains("re-bake the level"));
    }

    #[test]
    fn a_streamed_level_with_no_stored_bound_is_refused() {
        assert_eq!(
            stored_pool_bound(&directory(Vec::new()), None),
            Err(ShResidencyDrainError::MandatoryBoundMismatch {
                expected_lead: 0,
                stored_leads: Vec::new(),
            })
        );
    }

    // Without id 51 only a whole-map entry can hold the level: reach is gone,
    // so a camera-cell figure would undersize the pool. Lead 0 is no exception:
    // it matches the lead a level with no id 51 looks for.
    #[test]
    fn a_camera_cell_entry_is_refused_with_no_id51() {
        for lead in [DEFAULT_LEAD, 0] {
            let stored = directory(vec![entry(lead, 10, 0)]);
            assert_eq!(
                stored_pool_bound(&stored, None),
                Err(ShResidencyDrainError::MandatoryBoundMismatch {
                    expected_lead: 0,
                    stored_leads: vec![lead],
                })
            );
        }
    }
}
