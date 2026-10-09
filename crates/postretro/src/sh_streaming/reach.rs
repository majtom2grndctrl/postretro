//! SH's view of the level's reach: the clusters of the camera cell's id-51
//! entries, each at the smallest lead any of its cells carries.
//! See: context/lib/rendering_pipeline.md §4 (Cluster SH residency)

use std::collections::BTreeMap;

use super::controller::ShResidencyControllerError;
use crate::streaming::cell_demand::{DemandFrame, PathDemand};

/// The clusters the camera cell's id-51 set reaches, each at its nearest
/// cell's lead. Rebuilt only when the camera cell or lead L changes, so
/// turning in place or standing still costs no lookup. Lead and band split
/// at the stage's L: within L is mandatory reach, past it (up to the baked
/// maximum) is the optional band.
#[derive(Debug, Default)]
pub(super) struct ShReach {
    /// Camera cell and L the clusters were computed for.
    key: Option<(u32, u32)>,
    lead: u32,
    /// Cluster id → smallest lead of its cells in the camera cell's set.
    clusters: BTreeMap<u32, u32>,
}

impl ShReach {
    /// Applies one frame's cell demand; `None` (no usable id 51) empties the
    /// reach: no lead or band tier. A solid or exterior camera cell with no
    /// baked set, and the empty world, keep the current reach, so a change of
    /// L takes effect once the camera is in a cell with a baked set.
    pub(super) fn update(
        &mut self,
        frame: Option<DemandFrame<'_>>,
        cell_to_cluster: &[u32],
    ) -> Result<(), ShResidencyControllerError> {
        // An id 51 for another cell map is not usable here: the loader keeps
        // ids 49 and 51 on one level, so only a mixed fixture reaches this.
        let Some(frame) =
            frame.filter(|frame| frame.residency_set.camera_cell_count() == cell_to_cluster.len())
        else {
            self.key = None;
            self.clusters.clear();
            return Ok(());
        };
        let key = (frame.camera_cell, frame.lead);
        if self.key == Some(key) {
            return Ok(());
        }
        let entries = match frame.path_demand() {
            PathDemand::Hold => return Ok(()),
            PathDemand::CameraSetOrHold | PathDemand::CameraSet | PathDemand::PortalWalk => {
                frame.residency_set.entries_for(frame.camera_cell as usize)
            }
        };
        if entries.is_empty() && frame.path_demand() == PathDemand::CameraSetOrHold {
            return Ok(());
        }
        self.clusters.clear();
        for entry in entries {
            let cluster = *cell_to_cluster.get(entry.cell_id as usize).ok_or_else(|| {
                ShResidencyControllerError::InvalidTopology(format!(
                    "id-51 cell {} is outside the id-49 cell map",
                    entry.cell_id
                ))
            })?;
            self.clusters
                .entry(cluster)
                .and_modify(|lead| *lead = (*lead).min(entry.lead))
                .or_insert(entry.lead);
        }
        self.key = Some(key);
        self.lead = frame.lead;
        Ok(())
    }

    /// Every reach cluster with its lead, ascending cluster id.
    pub(super) fn clusters(&self) -> impl Iterator<Item = (u32, u32)> + '_ {
        self.clusters
            .iter()
            .map(|(&cluster, &lead)| (cluster, lead))
    }

    /// Whether `lead` is within L: mandatory reach rather than band.
    pub(super) fn is_lead(&self, lead: u32) -> bool {
        lead <= self.lead
    }

    /// The cluster's reach lead; past every reach cluster when not in reach,
    /// so pressure and request order treat it as farthest.
    pub(super) fn lead_of(&self, cluster: u32) -> u32 {
        self.clusters.get(&cluster).copied().unwrap_or(u32::MAX)
    }

    /// Clusters in the mandatory reach.
    pub(super) fn lead_count(&self) -> usize {
        self.clusters
            .values()
            .filter(|&&lead| self.is_lead(lead))
            .count()
    }
}
