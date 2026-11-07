//! The one definition of a camera cell's set for the lightmap pool bound.
//! See: context/lib/build_pipeline.md §PRL section IDs (Lightmap pool bound)

use postretro_level_format::cell_residency_set::{CellResidencySetSection, DEFAULT_LEAD};
use postretro_level_format::cluster_directory::ClusterDirectorySection;

use crate::pack::sh_mandatory_bound::pinned_clusters;

/// What the bound charges a camera cell for: its id-51 entries within the
/// lead, plus pins. Any later source of cells for the bound joins here, and
/// id 58's layout does not change for it.
pub(super) struct BoundCellInput<'a> {
    residency: &'a CellResidencySetSection,
    lead: u32,
    /// Pins are mandatory from every camera cell, so they resolve once.
    pinned_cells: Vec<u32>,
}

impl<'a> BoundCellInput<'a> {
    pub(super) fn new(
        residency: &'a CellResidencySetSection,
        directory: &ClusterDirectorySection,
    ) -> anyhow::Result<Self> {
        Ok(Self {
            residency,
            lead: DEFAULT_LEAD.min(residency.max_lead),
            pinned_cells: pinned_cells(directory)?,
        })
    }

    /// The lead the bound holds for: the default, or the baked maximum where
    /// that is lower.
    pub(super) fn lead(&self) -> u32 {
        self.lead
    }

    /// Append `camera_cell`'s set to `cells`. A cell both pinned and within
    /// lead appears twice; the bound counts blocks, each once.
    pub(super) fn cells_of(&self, camera_cell: u32, cells: &mut Vec<u32>) {
        // Entries ascend by lead, so the set is a prefix: the rest is band.
        let within_lead = self
            .residency
            .entries_for(camera_cell as usize)
            .iter()
            .take_while(|entry| entry.lead <= self.lead);
        cells.extend(within_lead.map(|entry| entry.cell_id));
        cells.extend_from_slice(&self.pinned_cells);
    }
}

/// The member cells of every pinned cluster.
fn pinned_cells(directory: &ClusterDirectorySection) -> anyhow::Result<Vec<u32>> {
    let mut cells = Vec::new();
    for cluster_id in pinned_clusters(directory) {
        let cluster = directory.clusters.get(cluster_id as usize).ok_or_else(|| {
            anyhow::anyhow!("Lightmap pool bound: pin names cluster {cluster_id} outside id 49")
        })?;
        let start = cluster.member_start as usize;
        let members = directory
            .members
            .get(start..start + cluster.member_count as usize)
            .ok_or_else(|| {
                anyhow::anyhow!("Lightmap pool bound: cluster {cluster_id} members exceed id 49")
            })?;
        cells.extend_from_slice(members);
    }
    Ok(cells)
}
