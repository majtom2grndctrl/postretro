//! Canonical owner of every stored node, and the cross-cluster owner
//! dependencies derived from it.
//!
//! Both are pure functions of the immutable manifest. Per-probe work indexes
//! `NodeMap` arrays; nothing here walks a tree once per probe.

use std::collections::{BTreeMap, BTreeSet};

use postretro_level_format::cluster_directory::{ClusterDirectorySection, ClusterRangeRole};

use super::ShResidencyDrainError;
use super::node_map::NodeMap;
use super::ownership::StoredNode;
use super::{ANIMATED_DIRECT_DELTA_ID, DIRECT_DELTA_ID, INDIRECT_BASE_ID, INDIRECT_DELTA_ID};

pub(super) struct NodeOwnership {
    /// The lowest cluster owning any probe of each stored node.
    pub(super) node_owner: NodeMap<u32>,
    /// The nodes each cluster owns, in `StoredNode` order.
    pub(super) nodes_by_owner: BTreeMap<u32, Vec<StoredNode>>,
}

pub(super) fn derive_node_ownership(
    dense_node: &[Option<StoredNode>],
    dense_owner: &[Option<u32>],
    grid_dimensions: [u32; 3],
) -> Result<NodeOwnership, ShResidencyDrainError> {
    let mut node_owner = NodeMap::for_grid(grid_dimensions, dense_node.len());
    // Consecutive probes mostly repeat the previous probe's node and owner,
    // and lowering a node to the same owner twice changes nothing.
    let mut last: Option<(StoredNode, u32)> = None;
    for (dense_index, node) in dense_node.iter().copied().enumerate() {
        if let Some(node) = node {
            let owner =
                dense_owner[dense_index].ok_or(ShResidencyDrainError::MissingDenseOwner {
                    cluster_id: 0,
                    dense_index: dense_index as u32,
                })?;
            if last != Some((node, owner)) {
                node_owner.insert_or_update(node, owner, |prior| *prior = (*prior).min(owner));
                last = Some((node, owner));
            }
        }
    }

    let mut nodes_by_owner = BTreeMap::<u32, Vec<StoredNode>>::new();
    for (node, &owner) in node_owner.iter() {
        nodes_by_owner.entry(owner).or_default().push(node);
    }
    // The table iterates in array order; every consumer sees the nodes in
    // `StoredNode` order, as a tree-keyed map would have yielded them.
    for nodes in nodes_by_owner.values_mut() {
        nodes.sort_unstable();
    }
    Ok(NodeOwnership {
        node_owner,
        nodes_by_owner,
    })
}

/// For each cluster, the other clusters that must be sampleable before it:
/// the writers and stored-node owners of its dense probes, and the owners of
/// its sparse halo rows. `node_owner` answers a stored node's canonical owner.
pub(super) fn derive_owner_dependencies(
    directory: &ClusterDirectorySection,
    dense_owner: &[Option<u32>],
    dense_node: &[Option<StoredNode>],
    node_owner: impl Fn(&StoredNode) -> Option<u32>,
) -> Result<Vec<BTreeSet<u32>>, ShResidencyDrainError> {
    let cluster_count = directory.clusters.len();
    let mut owner_dependencies = vec![BTreeSet::new(); cluster_count];
    for cluster_id in 0..cluster_count as u32 {
        let cluster = &directory.clusters[cluster_id as usize];
        let start = cluster.range_start as usize;
        let end = start.checked_add(cluster.range_count as usize).ok_or(
            ShResidencyDrainError::MalformedChunk {
                cluster_id,
                reason: "cluster range end overflows",
            },
        )?;
        for range in
            directory
                .ranges
                .get(start..end)
                .ok_or(ShResidencyDrainError::MalformedChunk {
                    cluster_id,
                    reason: "cluster range exceeds id-49",
                })?
        {
            let resource = &directory.resources[range.resource_index as usize];
            let range_end = range.start + range.count;
            if resource.section_id == INDIRECT_BASE_ID {
                // A repeat of the previous probe's writer or node owner is
                // already in this cluster's set.
                let mut last_writer: Option<u32> = None;
                let mut last_node: Option<(StoredNode, Option<u32>)> = None;
                let mut last_node_owner: Option<u32> = None;
                for dense in range.start..range_end {
                    // A chunk can carry the dense writer and its stored
                    // node closure from different canonical clusters. Both
                    // must already be sampleable: the writer owns the
                    // indirection word while the node owner owns the tile
                    // bytes that word addresses.
                    if let Some(Some(owner)) = dense_owner.get(dense as usize)
                        && *owner != cluster_id
                        && last_writer != Some(*owner)
                    {
                        owner_dependencies[cluster_id as usize].insert(*owner);
                        last_writer = Some(*owner);
                    }
                    if let Some(Some(node)) = dense_node.get(dense as usize) {
                        let owner = match last_node {
                            Some((cached, owner)) if cached == *node => owner,
                            _ => {
                                let owner = node_owner(node);
                                last_node = Some((*node, owner));
                                owner
                            }
                        };
                        if let Some(owner) = owner
                            && owner != cluster_id
                            && last_node_owner != Some(owner)
                        {
                            owner_dependencies[cluster_id as usize].insert(owner);
                            last_node_owner = Some(owner);
                        }
                    }
                }
            } else if matches!(
                resource.section_id,
                INDIRECT_DELTA_ID | DIRECT_DELTA_ID | ANIMATED_DIRECT_DELTA_ID
            ) && range.role == ClusterRangeRole::Halo
                && range.owner_cluster_id != cluster_id
            {
                owner_dependencies[cluster_id as usize].insert(range.owner_cluster_id);
            }
        }
    }
    Ok(owner_dependencies)
}
