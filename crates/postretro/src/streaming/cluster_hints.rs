//! Id-49 cluster hints decoded once for every streamed resource.
//! See: context/lib/rendering_pipeline.md §"Cluster SH residency"

use std::collections::BTreeSet;

use postretro_level_format::cluster_directory::{
    CLUSTER_HINT_FLAG_PINNED, ClusterDirectorySection,
};
use thiserror::Error;

/// Pins, prefetch priorities, and the cell-to-cluster map, from id 49 alone:
/// no id 50 is needed, so a resource streams its hints on a level whose SH
/// loads whole. Resource-specific policy, such as SH's owner closure, stays
/// with the resource.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ClusterHints {
    /// Owning cluster of each runtime cell.
    pub(crate) cell_to_cluster: Vec<u32>,
    /// Clusters an authored `stream_resident_volume` pins resident.
    pub(crate) pinned: BTreeSet<u32>,
    /// Authored prefetch priority per cluster, 0..=3. Zero is a no-op, so a
    /// map without hints keeps the unhinted order.
    pub(crate) priority: Vec<u8>,
}

#[derive(Debug, Clone, PartialEq, Eq, Error)]
#[error("{0}")]
pub(crate) struct ClusterHintsError(String);

fn invalid<T>(message: &str) -> Result<T, ClusterHintsError> {
    Err(ClusterHintsError(message.into()))
}

impl ClusterHints {
    /// Decodes a directory the loader has validated. Hint flags, priority
    /// range, and hint order are level-format's to reject; the checks here
    /// only keep indexing in bounds and the cell map total.
    pub(crate) fn decode(directory: &ClusterDirectorySection) -> Result<Self, ClusterHintsError> {
        let cluster_count = directory.clusters.len();
        let mut pinned = BTreeSet::new();
        let mut priority = vec![0; cluster_count];
        for hint in &directory.cluster_hints {
            let Some(slot) = usize::try_from(hint.cluster_id)
                .ok()
                .and_then(|cluster_id| priority.get_mut(cluster_id))
            else {
                return invalid("cluster hint names an out-of-range cluster");
            };
            let Ok(hint_priority) = u8::try_from(hint.priority) else {
                return invalid("cluster hint priority exceeds u8");
            };
            *slot = hint_priority;
            if hint.flags & CLUSTER_HINT_FLAG_PINNED != 0 {
                pinned.insert(hint.cluster_id);
            }
        }
        Ok(Self {
            cell_to_cluster: cell_to_cluster(directory)?,
            pinned,
            priority,
        })
    }
}

fn cell_to_cluster(directory: &ClusterDirectorySection) -> Result<Vec<u32>, ClusterHintsError> {
    let Ok(cell_count) = usize::try_from(directory.runtime_cell_count) else {
        return invalid("runtime cell count exceeds usize");
    };
    let mut cell_to_cluster = vec![u32::MAX; cell_count];
    for (cluster_id, cluster) in directory.clusters.iter().enumerate() {
        let Ok(cluster_id) = u32::try_from(cluster_id) else {
            return invalid("cluster id exceeds u32");
        };
        let (Ok(start), Ok(count)) = (
            usize::try_from(cluster.member_start),
            usize::try_from(cluster.member_count),
        ) else {
            return invalid("member range exceeds usize");
        };
        let Some(end) = start.checked_add(count) else {
            return invalid("member range overflows");
        };
        let Some(members) = directory.members.get(start..end) else {
            return invalid("member range exceeds id-49");
        };
        for &cell_id in members {
            let Some(entry) = usize::try_from(cell_id)
                .ok()
                .and_then(|cell| cell_to_cluster.get_mut(cell))
            else {
                return invalid("id-49 cell is out of range");
            };
            if *entry != u32::MAX {
                return invalid("id-49 assigns a runtime cell more than once");
            }
            *entry = cluster_id;
        }
    }
    if cell_to_cluster.contains(&u32::MAX) {
        return invalid("id-49 leaves a runtime cell unassigned");
    }
    Ok(cell_to_cluster)
}

#[cfg(test)]
mod tests {
    use super::*;
    use postretro_level_format::cluster_directory::{ClusterHintRecord, ClusterRecord};

    fn cluster(member_start: u32, member_count: u32) -> ClusterRecord {
        ClusterRecord {
            bounds_min: [0.0; 3],
            bounds_max: [1.0; 3],
            member_start,
            member_count,
            range_start: 0,
            range_count: 0,
            primitive_count: 0,
            flags: 0,
        }
    }

    /// Three clusters over five cells, with no id-50 anywhere.
    fn directory(cluster_hints: Vec<ClusterHintRecord>) -> ClusterDirectorySection {
        ClusterDirectorySection {
            runtime_cell_count: 5,
            primitive_limit: 64,
            cell_limit: 32,
            clusters: vec![cluster(0, 2), cluster(2, 1), cluster(3, 2)],
            resources: Vec::new(),
            members: vec![1, 4, 0, 2, 3],
            ranges: Vec::new(),
            seam_portal_ids: Vec::new(),
            cluster_hints,
        }
    }

    #[test]
    fn hint_decode_reads_pins_priorities_and_cells_from_id_49_alone() {
        let hints = ClusterHints::decode(&directory(vec![
            ClusterHintRecord {
                cluster_id: 0,
                flags: 0,
                priority: 2,
            },
            ClusterHintRecord {
                cluster_id: 2,
                flags: CLUSTER_HINT_FLAG_PINNED,
                priority: 3,
            },
        ]))
        .unwrap();

        assert_eq!(hints.pinned, BTreeSet::from([2]));
        assert_eq!(hints.priority, vec![2, 0, 3]);
        assert_eq!(hints.cell_to_cluster, vec![1, 0, 2, 2, 0]);
    }

    #[test]
    fn hint_decode_without_hints_pins_nothing_and_ranks_nothing() {
        let hints = ClusterHints::decode(&directory(Vec::new())).unwrap();
        assert!(hints.pinned.is_empty());
        assert_eq!(hints.priority, vec![0; 3]);
    }

    #[test]
    fn hint_decode_rejects_a_hint_past_the_cluster_table() {
        let error = ClusterHints::decode(&directory(vec![ClusterHintRecord {
            cluster_id: 3,
            flags: CLUSTER_HINT_FLAG_PINNED,
            priority: 0,
        }]))
        .unwrap_err();
        assert_eq!(
            error.to_string(),
            "cluster hint names an out-of-range cluster"
        );
    }
}
