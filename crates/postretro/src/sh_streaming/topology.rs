//! Manifest-derived topology used by the app-side residency planner.
//! See: context/lib/rendering_pipeline.md §4

use std::sync::Arc;

use postretro_level_format::SectionId;
use postretro_level_format::cluster_directory::ClusterDirectorySection;
use postretro_level_format::cluster_sh_payloads::ClusterShPayloadsSection;
use postretro_level_loader::{ShStreamBaseMetadata, ShStreamManifest, ShStreamSeamPortal};

use super::controller::ShResidencyControllerError;
use crate::streaming::cluster_hints::ClusterHints;

#[derive(Debug)]
pub(super) struct PlannerTopology {
    /// Pins, authored priorities, and the cell-to-cluster map, decoded by the
    /// shared streaming layer.
    pub(super) hints: Arc<ClusterHints>,
    pub(super) adjacency: Vec<Vec<u32>>,
    /// Authored seam portal endpoints resolved by the validated loader. This
    /// supplements normal adjacency for warm-up only; it is never fed back to
    /// the visibility traversal.
    pub(super) seam_portals: Vec<SeamPortalEndpoint>,
    pub(super) requested_resident_bytes: Vec<u64>,
    /// Encoded id-50 chunk length per cluster: the bytes a read of it moves.
    pub(super) encoded_chunk_bytes: Vec<u64>,
    pub(super) chunk_hashes: Vec<[u8; 32]>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct SeamPortalEndpoint {
    pub(super) portal_id: u32,
    pub(super) front_cluster_id: u32,
    pub(super) back_cluster_id: u32,
}

impl PlannerTopology {
    pub(super) fn cluster_count(&self) -> usize {
        self.adjacency.len()
    }

    /// `hints` is the level's id 49 decoded once at level scope; it must be
    /// the same directory `manifest` carries.
    pub(super) fn from_manifest(
        manifest: &ShStreamManifest,
        hints: Arc<ClusterHints>,
    ) -> Result<Self, ShResidencyControllerError> {
        Self::from_manifest_view(
            ManifestTopologyView {
                directory: manifest.cluster_directory(),
                payloads: manifest.payloads(),
                base: manifest.base(),
                adjacency: manifest.cluster_adjacency(),
                seam_portals: manifest.seam_portals(),
            },
            hints,
        )
    }

    pub(super) fn from_manifest_view(
        manifest: ManifestTopologyView<'_>,
        hints: Arc<ClusterHints>,
    ) -> Result<Self, ShResidencyControllerError> {
        let directory = manifest.directory;
        let cluster_count =
            usize::try_from(manifest.payloads.header.cluster_count).map_err(|_| {
                ShResidencyControllerError::InvalidTopology("cluster count exceeds usize".into())
            })?;
        if directory.clusters.len() != cluster_count {
            return Err(ShResidencyControllerError::InvalidTopology(
                "id-49 and id-50 disagree on cluster count".into(),
            ));
        }
        if manifest.adjacency.len() != cluster_count {
            return Err(ShResidencyControllerError::InvalidTopology(
                "validated cluster adjacency disagrees with cluster count".into(),
            ));
        }

        if hints.priority.len() != cluster_count
            || hints.cell_to_cluster.len() != directory.runtime_cell_count as usize
        {
            return Err(ShResidencyControllerError::InvalidTopology(
                "id-49 hints disagree with the manifest's directory".into(),
            ));
        }

        let mut seam_portals = Vec::with_capacity(manifest.seam_portals.len());
        for seam in manifest.seam_portals {
            seam_portals.push(seam_portal_endpoint(*seam, cluster_count)?);
        }
        if seam_portals
            .windows(2)
            .any(|pair| pair[0].portal_id >= pair[1].portal_id)
        {
            return Err(ShResidencyControllerError::InvalidTopology(
                "seam portal IDs are not canonical".into(),
            ));
        }

        let base_resource_index = directory
            .resources
            .iter()
            .position(|resource| resource.section_id == SectionId::OctahedralShVolume as u32)
            .ok_or_else(|| {
                ShResidencyControllerError::InvalidTopology(
                    "id-49 does not contain the required id-34 resource".into(),
                )
            })?;
        for cluster_id in 0..cluster_count {
            for range in cluster_ranges(directory, cluster_id)? {
                if range.resource_index as usize >= directory.resources.len() {
                    return Err(ShResidencyControllerError::InvalidTopology(
                        "range names an absent resource".into(),
                    ));
                }
                if range.resource_index as usize == base_resource_index {
                    let dense_range = checked_range(range.start, range.count, "dense range")?;
                    if dense_range.end as usize > manifest.base.probes.len() {
                        return Err(ShResidencyControllerError::InvalidTopology(
                            "dense index exceeds id-34 metadata".into(),
                        ));
                    }
                }
            }
        }

        let adjacency = manifest.adjacency.to_vec();
        for (cluster_id, neighbors) in adjacency.iter().enumerate() {
            if neighbors
                .iter()
                .any(|&neighbor| neighbor as usize >= cluster_count)
            {
                return Err(ShResidencyControllerError::InvalidTopology(format!(
                    "cluster {cluster_id} adjacency names an out-of-range neighbor"
                )));
            }
        }
        let requested_resident_bytes = manifest
            .payloads
            .index
            .iter()
            .map(|entry| entry.requested_resident_bytes)
            .collect();
        let encoded_chunk_bytes = manifest
            .payloads
            .index
            .iter()
            .map(|entry| entry.payload_len)
            .collect();
        let chunk_hashes = manifest
            .payloads
            .index
            .iter()
            .map(|entry| entry.hash)
            .collect();
        Ok(Self {
            hints,
            adjacency,
            seam_portals,
            requested_resident_bytes,
            encoded_chunk_bytes,
            chunk_hashes,
        })
    }
}

#[derive(Clone, Copy)]
pub(super) struct ManifestTopologyView<'a> {
    pub(super) directory: &'a ClusterDirectorySection,
    pub(super) payloads: &'a ClusterShPayloadsSection,
    pub(super) base: &'a ShStreamBaseMetadata,
    pub(super) adjacency: &'a [Vec<u32>],
    pub(super) seam_portals: &'a [ShStreamSeamPortal],
}

fn seam_portal_endpoint(
    seam: ShStreamSeamPortal,
    cluster_count: usize,
) -> Result<SeamPortalEndpoint, ShResidencyControllerError> {
    let front = usize::try_from(seam.front_cluster_id).map_err(|_| {
        ShResidencyControllerError::InvalidTopology("seam front cluster exceeds usize".into())
    })?;
    let back = usize::try_from(seam.back_cluster_id).map_err(|_| {
        ShResidencyControllerError::InvalidTopology("seam back cluster exceeds usize".into())
    })?;
    if front >= cluster_count || back >= cluster_count || front == back {
        return Err(ShResidencyControllerError::InvalidTopology(
            "seam portal names invalid cluster endpoints".into(),
        ));
    }
    Ok(SeamPortalEndpoint {
        portal_id: seam.portal_id,
        front_cluster_id: seam.front_cluster_id,
        back_cluster_id: seam.back_cluster_id,
    })
}

fn cluster_ranges(
    directory: &postretro_level_format::cluster_directory::ClusterDirectorySection,
    cluster_id: usize,
) -> Result<
    &[postretro_level_format::cluster_directory::ClusterRangeRecord],
    ShResidencyControllerError,
> {
    let cluster = directory.clusters.get(cluster_id).ok_or_else(|| {
        ShResidencyControllerError::InvalidTopology("cluster index exceeds id-49".into())
    })?;
    let start = usize::try_from(cluster.range_start).map_err(|_| {
        ShResidencyControllerError::InvalidTopology("range start exceeds usize".into())
    })?;
    let end = start
        .checked_add(usize::try_from(cluster.range_count).map_err(|_| {
            ShResidencyControllerError::InvalidTopology("range count exceeds usize".into())
        })?)
        .ok_or_else(|| ShResidencyControllerError::InvalidTopology("range end overflows".into()))?;
    directory.ranges.get(start..end).ok_or_else(|| {
        ShResidencyControllerError::InvalidTopology("cluster range exceeds id-49".into())
    })
}

fn checked_range(
    start: u32,
    count: u32,
    label: &'static str,
) -> Result<std::ops::Range<u32>, ShResidencyControllerError> {
    let end = start.checked_add(count).ok_or_else(|| {
        ShResidencyControllerError::InvalidTopology(format!("{label} overflows u32"))
    })?;
    Ok(start..end)
}

#[cfg(test)]
mod independence_tests;

#[cfg(test)]
mod tests {
    use crate::sh_streaming::topology_test_fixtures::{ManifestFixture, assert_invalid_topology};

    #[test]
    fn manifest_duplicate_cell_assignment_is_rejected_before_planner_construction() {
        let mut fixture = ManifestFixture::two_clusters();
        fixture.duplicate_cell_assignment();

        assert_invalid_topology(
            fixture.build_topology(),
            "assigns a runtime cell more than once",
        );
    }

    #[test]
    fn manifest_unassigned_cell_is_rejected_before_planner_construction() {
        let mut fixture = ManifestFixture::two_clusters();
        fixture.leave_cell_unassigned();

        assert_invalid_topology(fixture.build_topology(), "leaves a runtime cell unassigned");
    }

    #[test]
    fn manifest_out_of_range_dense_index_is_rejected_before_planner_construction() {
        let mut fixture = ManifestFixture::two_clusters();
        fixture.out_of_range_dense_index();

        assert_invalid_topology(
            fixture.build_topology(),
            "dense index exceeds id-34 metadata",
        );
    }

    #[test]
    fn manifest_out_of_range_adjacency_is_rejected_before_planner_construction() {
        let mut fixture = ManifestFixture::two_clusters();
        fixture.out_of_range_adjacency();

        assert_invalid_topology(
            fixture.build_topology(),
            "cluster 0 adjacency names an out-of-range neighbor",
        );
    }
}
