//! Manifest-derived topology used by the app-side residency planner.
//! See: context/lib/rendering_pipeline.md §4

use std::collections::{BTreeMap, BTreeSet};

use postretro_level_format::SectionId;
use postretro_level_format::cluster_directory::{
    CLUSTER_HINT_FLAG_PINNED, ClusterDirectorySection, ClusterRangeRole, ClusterResourceDomain,
    DENSE_OWNER_SENTINEL,
};
use postretro_level_format::cluster_sh_payloads::ClusterShPayloadsSection;
use postretro_level_loader::{ShStreamBaseMetadata, ShStreamManifest, ShStreamSeamPortal};

use super::controller::ShResidencyControllerError;

#[derive(Debug)]
pub(super) struct PlannerTopology {
    pub(super) cell_to_cluster: Vec<u32>,
    pub(super) adjacency: Vec<Vec<u32>>,
    /// Authored seam portal endpoints resolved by the validated loader. This
    /// supplements normal adjacency for warm-up only; it is never fed back to
    /// the visibility traversal.
    pub(super) seam_portals: Vec<SeamPortalEndpoint>,
    /// Canonical cluster IDs with a non-optional resident policy record.
    pub(super) pinned_clusters: BTreeSet<u32>,
    /// Canonical authored priority for each cluster. Zero is intentionally a
    /// no-op so old maps retain the exact old ordering.
    pub(super) authored_priorities: Vec<u32>,
    /// Every dependency which must be sampleable before this cluster's halo
    /// can become sampleable. Dense node/probe writers and sparse row owners
    /// are both represented here.
    pub(super) owners: Vec<Vec<u32>>,
    pub(super) requested_resident_bytes: Vec<u64>,
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

    pub(super) fn from_manifest(
        manifest: &ShStreamManifest,
    ) -> Result<Self, ShResidencyControllerError> {
        Self::from_manifest_view(ManifestTopologyView {
            directory: manifest.cluster_directory(),
            payloads: manifest.payloads(),
            base: manifest.base(),
            adjacency: manifest.cluster_adjacency(),
            seam_portals: manifest.seam_portals(),
        })
    }

    pub(super) fn from_manifest_view(
        manifest: ManifestTopologyView<'_>,
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

        let mut pinned_clusters = BTreeSet::new();
        let mut authored_priorities = vec![0; cluster_count];
        for hint in &directory.cluster_hints {
            let cluster_id = usize::try_from(hint.cluster_id).map_err(|_| {
                ShResidencyControllerError::InvalidTopology("cluster hint id exceeds usize".into())
            })?;
            if cluster_id >= cluster_count {
                return Err(ShResidencyControllerError::InvalidTopology(
                    "cluster hint names an out-of-range cluster".into(),
                ));
            }
            if hint.flags & !CLUSTER_HINT_FLAG_PINNED != 0 || hint.priority > 3 {
                return Err(ShResidencyControllerError::InvalidTopology(
                    "cluster hint has invalid flags or priority".into(),
                ));
            }
            if hint.flags & CLUSTER_HINT_FLAG_PINNED != 0 {
                pinned_clusters.insert(hint.cluster_id);
            }
            authored_priorities[cluster_id] = hint.priority;
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

        let cell_count = usize::try_from(directory.runtime_cell_count).map_err(|_| {
            ShResidencyControllerError::InvalidTopology("runtime cell count exceeds usize".into())
        })?;
        let mut cell_to_cluster = vec![u32::MAX; cell_count];
        for (cluster_id, cluster) in directory.clusters.iter().enumerate() {
            let start = usize::try_from(cluster.member_start).map_err(|_| {
                ShResidencyControllerError::InvalidTopology("member start exceeds usize".into())
            })?;
            let end = start
                .checked_add(usize::try_from(cluster.member_count).map_err(|_| {
                    ShResidencyControllerError::InvalidTopology("member count exceeds usize".into())
                })?)
                .ok_or_else(|| {
                    ShResidencyControllerError::InvalidTopology("member range overflows".into())
                })?;
            let members = directory.members.get(start..end).ok_or_else(|| {
                ShResidencyControllerError::InvalidTopology("member range exceeds id-49".into())
            })?;
            for &cell_id in members {
                let cell = usize::try_from(cell_id).map_err(|_| {
                    ShResidencyControllerError::InvalidTopology("cell id exceeds usize".into())
                })?;
                let entry = cell_to_cluster.get_mut(cell).ok_or_else(|| {
                    ShResidencyControllerError::InvalidTopology("id-49 cell is out of range".into())
                })?;
                if *entry != u32::MAX {
                    return Err(ShResidencyControllerError::InvalidTopology(
                        "id-49 assigns a runtime cell more than once".into(),
                    ));
                }
                *entry = u32::try_from(cluster_id).map_err(|_| {
                    ShResidencyControllerError::InvalidTopology("cluster id exceeds u32".into())
                })?;
            }
        }
        if cell_to_cluster.contains(&u32::MAX) {
            return Err(ShResidencyControllerError::InvalidTopology(
                "id-49 leaves a runtime cell unassigned".into(),
            ));
        }

        let mut owners = vec![BTreeSet::new(); cluster_count];
        let streamed_sparse: BTreeSet<u32> = manifest
            .payloads
            .sources
            .iter()
            .filter(|source| {
                matches!(
                    source.kind,
                    postretro_level_format::cluster_sh_payloads::ClusterShPayloadsSourceKind::SparseAffinity
                )
            })
            .map(|source| source.section_id)
            .collect();
        let base_resource_index = directory
            .resources
            .iter()
            .position(|resource| resource.section_id == SectionId::OctahedralShVolume as u32)
            .ok_or_else(|| {
                ShResidencyControllerError::InvalidTopology(
                    "id-49 does not contain the required id-34 resource".into(),
                )
            })?;

        let mut node_owner = BTreeMap::<DenseNode, u32>::new();
        let mut patch_owner = vec![u32::MAX; manifest.base.probes.len()];
        for (cluster_id, _) in directory.clusters.iter().enumerate() {
            for range in cluster_ranges(directory, cluster_id)? {
                let resource = directory
                    .resources
                    .get(range.resource_index as usize)
                    .ok_or_else(|| {
                        ShResidencyControllerError::InvalidTopology(
                            "range names an absent resource".into(),
                        )
                    })?;
                if range.resource_index as usize != base_resource_index
                    || resource.domain != ClusterResourceDomain::DenseProbe
                {
                    continue;
                }
                if range.role != ClusterRangeRole::Dense
                    || range.owner_cluster_id != DENSE_OWNER_SENTINEL
                {
                    return Err(ShResidencyControllerError::InvalidTopology(
                        "id-34 range has non-dense ownership fields".into(),
                    ));
                }
                for dense_index in checked_range(range.start, range.count, "dense range")? {
                    let dense_index = usize::try_from(dense_index).map_err(|_| {
                        ShResidencyControllerError::InvalidTopology(
                            "dense index exceeds usize".into(),
                        )
                    })?;
                    if manifest
                        .base
                        .probes
                        .get(dense_index)
                        .ok_or_else(|| {
                            ShResidencyControllerError::InvalidTopology(
                                "dense index exceeds id-34 metadata".into(),
                            )
                        })?
                        .validity
                        == 0
                    {
                        continue;
                    }
                    let patch = patch_owner.get_mut(dense_index).ok_or_else(|| {
                        ShResidencyControllerError::InvalidTopology(
                            "dense index exceeds id-34 metadata".into(),
                        )
                    })?;
                    *patch = (*patch).min(cluster_id as u32);
                    let node = dense_node(manifest.base, dense_index)?;
                    node_owner
                        .entry(node)
                        .and_modify(|owner| *owner = (*owner).min(cluster_id as u32))
                        .or_insert(cluster_id as u32);
                }
            }
        }

        for (cluster_id, _) in directory.clusters.iter().enumerate() {
            for range in cluster_ranges(directory, cluster_id)? {
                let resource = directory
                    .resources
                    .get(range.resource_index as usize)
                    .ok_or_else(|| {
                        ShResidencyControllerError::InvalidTopology(
                            "range names an absent resource".into(),
                        )
                    })?;
                if range.resource_index as usize == base_resource_index {
                    for dense_index in checked_range(range.start, range.count, "dense range")? {
                        let dense_index = usize::try_from(dense_index).map_err(|_| {
                            ShResidencyControllerError::InvalidTopology(
                                "dense index exceeds usize".into(),
                            )
                        })?;
                        if manifest
                            .base
                            .probes
                            .get(dense_index)
                            .ok_or_else(|| {
                                ShResidencyControllerError::InvalidTopology(
                                    "dense index exceeds id-34 metadata".into(),
                                )
                            })?
                            .validity
                            == 0
                        {
                            continue;
                        }
                        let patch = *patch_owner.get(dense_index).ok_or_else(|| {
                            ShResidencyControllerError::InvalidTopology(
                                "dense index exceeds id-34 metadata".into(),
                            )
                        })?;
                        if patch == u32::MAX {
                            return Err(ShResidencyControllerError::InvalidTopology(
                                "dense patch has no canonical writer".into(),
                            ));
                        }
                        owners[cluster_id].insert(patch);
                        let node_owner = *node_owner
                            .get(&dense_node(manifest.base, dense_index)?)
                            .ok_or_else(|| {
                                ShResidencyControllerError::InvalidTopology(
                                    "dense node has no canonical writer".into(),
                                )
                            })?;
                        owners[cluster_id].insert(node_owner);
                    }
                    continue;
                }
                if resource.domain == ClusterResourceDomain::AffinityCell
                    && streamed_sparse.contains(&resource.section_id)
                    && range.role == ClusterRangeRole::Halo
                {
                    owners[cluster_id].insert(range.owner_cluster_id);
                }
            }
            owners[cluster_id].remove(&(cluster_id as u32));
        }
        validate_owner_graph(&owners)?;

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
        let chunk_hashes = manifest
            .payloads
            .index
            .iter()
            .map(|entry| entry.hash)
            .collect();
        Ok(Self {
            cell_to_cluster,
            adjacency,
            seam_portals,
            pinned_clusters,
            authored_priorities,
            owners: owners
                .into_iter()
                .map(|owners| owners.into_iter().collect())
                .collect(),
            requested_resident_bytes,
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

fn validate_owner_graph(owners: &[BTreeSet<u32>]) -> Result<(), ShResidencyControllerError> {
    let mut marks = vec![0u8; owners.len()];
    for cluster_id in 0..owners.len() {
        visit_owner_graph(cluster_id, owners, &mut marks)?;
    }
    Ok(())
}

fn visit_owner_graph(
    cluster_id: usize,
    owners: &[BTreeSet<u32>],
    marks: &mut [u8],
) -> Result<(), ShResidencyControllerError> {
    match marks.get(cluster_id).copied() {
        Some(2) => return Ok(()),
        Some(1) => {
            return Err(ShResidencyControllerError::OwnerCycle(
                u32::try_from(cluster_id).map_err(|_| {
                    ShResidencyControllerError::InvalidTopology("cluster id exceeds u32".into())
                })?,
            ));
        }
        Some(0) => {}
        None => {
            return Err(ShResidencyControllerError::InvalidTopology(
                "owner graph index exceeds cluster count".into(),
            ));
        }
        Some(_) => unreachable!("owner graph marks only use 0, 1, and 2"),
    }
    marks[cluster_id] = 1;
    for &owner in &owners[cluster_id] {
        let owner = usize::try_from(owner).map_err(|_| {
            ShResidencyControllerError::InvalidTopology("owner id exceeds usize".into())
        })?;
        if owner >= owners.len() {
            return Err(ShResidencyControllerError::InvalidTopology(
                "owner map names an out-of-range cluster".into(),
            ));
        }
        visit_owner_graph(owner, owners, marks)?;
    }
    marks[cluster_id] = 2;
    Ok(())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
struct DenseNode {
    origin: [u32; 3],
    scale: u8,
}

fn dense_node(
    base: &ShStreamBaseMetadata,
    dense_index: usize,
) -> Result<DenseNode, ShResidencyControllerError> {
    let probe = base.probes.get(dense_index).ok_or_else(|| {
        ShResidencyControllerError::InvalidTopology("dense index exceeds id-34 probes".into())
    })?;
    let width = usize::try_from(base.grid_dimensions[0]).map_err(|_| {
        ShResidencyControllerError::InvalidTopology("grid width exceeds usize".into())
    })?;
    let height = usize::try_from(base.grid_dimensions[1]).map_err(|_| {
        ShResidencyControllerError::InvalidTopology("grid height exceeds usize".into())
    })?;
    let xy = width.checked_mul(height).ok_or_else(|| {
        ShResidencyControllerError::InvalidTopology("grid xy dimensions overflow".into())
    })?;
    if xy == 0
        || dense_index
            >= xy
                .checked_mul(usize::try_from(base.grid_dimensions[2]).map_err(|_| {
                    ShResidencyControllerError::InvalidTopology("grid depth exceeds usize".into())
                })?)
                .ok_or_else(|| {
                    ShResidencyControllerError::InvalidTopology("grid dimensions overflow".into())
                })?
    {
        return Err(ShResidencyControllerError::InvalidTopology(
            "dense index is outside id-34 grid".into(),
        ));
    }
    let coords = [
        u32::try_from(dense_index % width).map_err(|_| {
            ShResidencyControllerError::InvalidTopology("probe x exceeds u32".into())
        })?,
        u32::try_from((dense_index / width) % height).map_err(|_| {
            ShResidencyControllerError::InvalidTopology("probe y exceeds u32".into())
        })?,
        u32::try_from(dense_index / xy).map_err(|_| {
            ShResidencyControllerError::InvalidTopology("probe z exceeds u32".into())
        })?,
    ];
    let scale_edge = 1u32
        .checked_shl(u32::from(probe.node_scale))
        .ok_or_else(|| {
            ShResidencyControllerError::InvalidTopology("id-34 node scale overflows".into())
        })?;
    let brick = coords.map(|coordinate| coordinate / 4);
    Ok(DenseNode {
        origin: brick.map(|coordinate| coordinate / scale_edge * scale_edge),
        scale: probe.node_scale,
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
mod tests {
    use super::*;
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
    fn manifest_malformed_dense_ownership_is_rejected_before_planner_construction() {
        let mut fixture = ManifestFixture::two_clusters();
        fixture.malformed_dense_ownership();

        assert_invalid_topology(
            fixture.build_topology(),
            "id-34 range has non-dense ownership fields",
        );
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
    fn manifest_sparse_halo_owner_is_rejected_before_planner_construction() {
        let mut fixture = ManifestFixture::two_clusters();
        fixture.add_sparse_halo_with_owner(2);

        assert_invalid_topology(
            fixture.build_topology(),
            "owner map names an out-of-range cluster",
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

    #[test]
    fn disconnected_owner_cycle_is_rejected_during_topology_validation() {
        let owners = vec![BTreeSet::new(), BTreeSet::from([2]), BTreeSet::from([1])];
        assert!(matches!(
            validate_owner_graph(&owners),
            Err(ShResidencyControllerError::OwnerCycle(1 | 2))
        ));
    }
}
