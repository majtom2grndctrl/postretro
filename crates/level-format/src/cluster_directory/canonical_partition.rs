//! Deterministic runtime-cell partitioning for the cluster directory.
//! See: context/lib/build_pipeline.md §PRL section IDs

use std::collections::BTreeSet;

use crate::{bvh::BvhSection, cells::CellsSection, portals::PortalsSection};

use super::{
    CLUSTER_FLAG_INDIVISIBLE_OVERSIZE, ClusterDirectoryError, ClusterRecord, canonical_zero,
    invalid, try_vec, u32_len, usize_count,
};

/// Canonical resource-independent partition of runtime cells.
/// Cluster range fields remain zero until resource ranges are populated.
#[derive(Debug, Clone, PartialEq)]
pub struct CanonicalCellPartition {
    pub clusters: Vec<ClusterRecord>,
    pub members: Vec<u32>,
}

/// Reconstruct the deterministic cell partition used by compiler output.
///
/// This is shared by compiler construction and runtime semantic validation so
/// accepted section-49 membership cannot drift from the greedy frontier rule.
pub fn canonical_cell_partition(
    cells: &CellsSection,
    portals: &PortalsSection,
    bvh: &BvhSection,
    primitive_limit: u32,
    cell_limit: u32,
) -> Result<CanonicalCellPartition, ClusterDirectoryError> {
    if primitive_limit == 0 || cell_limit == 0 {
        return invalid("primitive_limit and cell_limit must be positive");
    }
    let cell_count = u32_len(cells.cells.len(), "runtime cell count")?;
    let cell_count_usize = usize_count(cell_count)?;
    let cell_limit_usize = usize_count(cell_limit)?;

    let mut primitive_counts = try_vec(cell_count, "cell primitive counts")?;
    primitive_counts.resize(cell_count_usize, 0u32);
    for (leaf_index, leaf) in bvh.leaves.iter().enumerate() {
        if leaf.cell_id >= cell_count {
            return invalid(format!(
                "BVH leaf {leaf_index} names cell {} outside {cell_count}",
                leaf.cell_id
            ));
        }
        if leaf.index_count != 0 {
            primitive_counts[leaf.cell_id as usize] = primitive_counts[leaf.cell_id as usize]
                .checked_add(1)
                .ok_or(ClusterDirectoryError::SizeOverflow("cell primitive count"))?;
        }
    }

    let mut adjacency = try_vec(cell_count, "cell adjacency")?;
    adjacency.resize_with(cell_count_usize, BTreeSet::new);
    for (portal_index, portal) in portals.portals.iter().enumerate() {
        if portal.front_leaf >= cell_count || portal.back_leaf >= cell_count {
            return invalid(format!(
                "portal {portal_index} endpoint ({}, {}) outside {cell_count} cells",
                portal.front_leaf, portal.back_leaf
            ));
        }
        adjacency[portal.front_leaf as usize].insert(portal.back_leaf);
        adjacency[portal.back_leaf as usize].insert(portal.front_leaf);
    }

    let mut unassigned: BTreeSet<u32> = (0..cell_count).collect();
    let mut clusters = try_vec(cell_count, "canonical clusters")?;
    let mut members = try_vec(cell_count, "canonical members")?;
    while !unassigned.is_empty() {
        let seed = *unassigned
            .iter()
            .min_by(|&&left, &&right| compare_cell_keys(left, right, &cells.cells))
            .expect("nonempty set has a seed");
        unassigned.remove(&seed);
        let mut cluster_members = vec![seed];
        let mut primitive_count = primitive_counts[seed as usize];
        let mut frontier: BTreeSet<u32> = adjacency[seed as usize]
            .iter()
            .copied()
            .filter(|cell| unassigned.contains(cell))
            .collect();

        loop {
            if cluster_members.len() >= cell_limit_usize {
                break;
            }
            let mut candidate = None;
            for &cell in &frontier {
                let Some(candidate_primitive_count) =
                    primitive_count.checked_add(primitive_counts[cell as usize])
                else {
                    continue;
                };
                if candidate_primitive_count > primitive_limit {
                    continue;
                }
                if candidate.is_none_or(|(current, _)| {
                    compare_cell_keys(cell, current, &cells.cells).is_lt()
                }) {
                    candidate = Some((cell, candidate_primitive_count));
                }
            }
            let Some((candidate, candidate_primitive_count)) = candidate else {
                break;
            };
            frontier.remove(&candidate);
            if !unassigned.remove(&candidate) {
                continue;
            }
            cluster_members.push(candidate);
            primitive_count = candidate_primitive_count;
            frontier.extend(
                adjacency[candidate as usize]
                    .iter()
                    .copied()
                    .filter(|cell| unassigned.contains(cell)),
            );
        }

        cluster_members.sort_unstable();
        let member_start = u32_len(members.len(), "canonical member start")?;
        let member_count = u32_len(cluster_members.len(), "canonical cluster member count")?;
        let mut bounds_min = [f32::INFINITY; 3];
        let mut bounds_max = [f32::NEG_INFINITY; 3];
        for &cell_id in &cluster_members {
            let cell = &cells.cells[cell_id as usize];
            for axis in 0..3 {
                bounds_min[axis] = canonical_zero(bounds_min[axis].min(cell.bounds_min[axis]));
                bounds_max[axis] = canonical_zero(bounds_max[axis].max(cell.bounds_max[axis]));
            }
        }
        let flags = if member_count == 1 && primitive_count > primitive_limit {
            CLUSTER_FLAG_INDIVISIBLE_OVERSIZE
        } else {
            0
        };
        members.extend(cluster_members);
        clusters.push(ClusterRecord {
            bounds_min,
            bounds_max,
            member_start,
            member_count,
            range_start: 0,
            range_count: 0,
            primitive_count,
            flags,
        });
    }

    Ok(CanonicalCellPartition { clusters, members })
}

fn compare_cell_keys(
    left: u32,
    right: u32,
    cells: &[crate::cells::CellRecord],
) -> std::cmp::Ordering {
    let left_bounds = cells[left as usize].bounds_min.map(canonical_zero);
    let right_bounds = cells[right as usize].bounds_min.map(canonical_zero);
    left_bounds[2]
        .total_cmp(&right_bounds[2])
        .then_with(|| left_bounds[1].total_cmp(&right_bounds[1]))
        .then_with(|| left_bounds[0].total_cmp(&right_bounds[0]))
        .then_with(|| left.cmp(&right))
}
