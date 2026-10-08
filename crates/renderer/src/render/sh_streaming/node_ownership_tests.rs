//! Array-backed node ownership against the tree-keyed code it replaced. The
//! `oracle_*` functions are that code, kept verbatim.

use std::collections::{BTreeMap, BTreeSet};

use postretro_level_format::cluster_directory::{
    ClusterRangeRecord, ClusterRangeRole, ClusterRecord, ClusterResourceDomain,
    ClusterResourceRecord,
};

use super::node_ownership::{derive_node_ownership, derive_owner_dependencies};
use super::ownership::equivalence_tests::{
    GRID_SHAPES, Rng, base_with_probes, key_variants, oracle_derive, synthetic_probes,
};
use super::*;

type OracleOwnership = (BTreeMap<StoredNode, u32>, BTreeMap<u32, Vec<StoredNode>>);

fn oracle_ownership(
    dense_node: &[Option<StoredNode>],
    dense_owner: &[Option<u32>],
) -> Result<OracleOwnership, ShResidencyDrainError> {
    let mut node_owner = BTreeMap::<StoredNode, u32>::new();
    for (dense_index, node) in dense_node.iter().copied().enumerate() {
        if let Some(node) = node {
            let owner =
                dense_owner[dense_index].ok_or(ShResidencyDrainError::MissingDenseOwner {
                    cluster_id: 0,
                    dense_index: dense_index as u32,
                })?;
            node_owner
                .entry(node)
                .and_modify(|prior| *prior = (*prior).min(owner))
                .or_insert(owner);
        }
    }
    let mut nodes_by_owner = BTreeMap::<u32, Vec<StoredNode>>::new();
    for (&node, &owner) in &node_owner {
        nodes_by_owner.entry(owner).or_default().push(node);
    }
    Ok((node_owner, nodes_by_owner))
}

fn oracle_owner_dependencies(
    directory: &ClusterDirectorySection,
    dense_owner: &[Option<u32>],
    dense_node: &[Option<StoredNode>],
    node_owner: &BTreeMap<StoredNode, u32>,
) -> Result<Vec<BTreeSet<u32>>, ShResidencyDrainError> {
    let cluster_count = directory.clusters.len() as u32;
    let mut owner_dependencies = vec![BTreeSet::new(); cluster_count as usize];
    for cluster_id in 0..cluster_count {
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
                for dense in range.start..range_end {
                    if let Some(Some(owner)) = dense_owner.get(dense as usize)
                        && *owner != cluster_id
                    {
                        owner_dependencies[cluster_id as usize].insert(*owner);
                    }
                    if let Some(Some(node)) = dense_node.get(dense as usize)
                        && let Some(&owner) = node_owner.get(node)
                        && owner != cluster_id
                    {
                        owner_dependencies[cluster_id as usize].insert(owner);
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

fn cluster_record(range_start: u32, range_count: u32) -> ClusterRecord {
    ClusterRecord {
        bounds_min: [0.0; 3],
        bounds_max: [1.0; 3],
        member_start: 0,
        member_count: 0,
        range_start,
        range_count,
        primitive_count: 0,
        flags: 0,
    }
}

fn dense_range(cluster_id: u32, start: u32, count: u32) -> ClusterRangeRecord {
    ClusterRangeRecord {
        resource_index: 0,
        start,
        count,
        owner_cluster_id: cluster_id,
        role: ClusterRangeRole::Owned,
    }
}

/// `cluster_count` clusters whose dense ranges cover random spans of
/// `probe_count` probes, plus sparse halo ranges naming random owners.
/// `overshoot` lets a dense range run past the last probe.
fn synthetic_directory(
    rng: &mut Rng,
    cluster_count: u32,
    probe_count: u32,
    dims: [u32; 3],
    overshoot: bool,
    sparse: bool,
) -> ClusterDirectorySection {
    let mut ranges = Vec::new();
    let mut clusters = Vec::new();
    for cluster_id in 0..cluster_count {
        let first = ranges.len() as u32;
        for _ in 0..1 + rng.below(3) {
            let start = rng.below(probe_count);
            let room = probe_count - start + if overshoot { 3 } else { 0 };
            ranges.push(dense_range(cluster_id, start, 1 + rng.below(room)));
        }
        if sparse {
            for _ in 0..rng.below(3) {
                ranges.push(ClusterRangeRecord {
                    resource_index: 1,
                    start: 0,
                    count: 1,
                    owner_cluster_id: rng.below(cluster_count),
                    role: if rng.chance(70) {
                        ClusterRangeRole::Halo
                    } else {
                        ClusterRangeRole::Owned
                    },
                });
            }
        }
        clusters.push(cluster_record(first, ranges.len() as u32 - first));
    }
    ClusterDirectorySection {
        runtime_cell_count: 0,
        primitive_limit: 0,
        cell_limit: 0,
        clusters,
        resources: vec![
            ClusterResourceRecord {
                section_id: INDIRECT_BASE_ID,
                domain: ClusterResourceDomain::DenseProbe,
                dimensions: dims,
            },
            ClusterResourceRecord {
                section_id: INDIRECT_DELTA_ID,
                domain: ClusterResourceDomain::AffinityCell,
                dimensions: [1, 1, 1],
            },
        ],
        members: Vec::new(),
        ranges,
        seam_portal_ids: Vec::new(),
        cluster_hints: Vec::new(),
    }
}

/// Lowest cluster whose dense ranges contain each probe.
fn lowest_dense_owner(directory: &ClusterDirectorySection, probes: usize) -> Vec<Option<u32>> {
    let mut owner = vec![None::<u32>; probes];
    for (cluster_id, cluster) in directory.clusters.iter().enumerate() {
        let span =
            cluster.range_start as usize..(cluster.range_start + cluster.range_count) as usize;
        for range in &directory.ranges[span] {
            if range.resource_index != 0 {
                continue;
            }
            for dense in range.start..range.start + range.count {
                if let Some(slot) = owner.get_mut(dense as usize) {
                    *slot =
                        Some(slot.map_or(cluster_id as u32, |prior| prior.min(cluster_id as u32)));
                }
            }
        }
    }
    owner
}

fn assert_ownership_matches(
    directory: &ClusterDirectorySection,
    dense_node: &[Option<StoredNode>],
    dense_owner: &[Option<u32>],
    dims: [u32; 3],
    label: &str,
) -> bool {
    let new = derive_node_ownership(dense_node, dense_owner, dims);
    let old = oracle_ownership(dense_node, dense_owner);
    let (new_owner, new_by_owner, old_owner, old_by_owner) = match (new, old) {
        (Err(new), Err(old)) => {
            assert_eq!(new, old, "{label}");
            return false;
        }
        (Ok(new), Ok((old_owner, old_by_owner))) => {
            (new.node_owner, new.nodes_by_owner, old_owner, old_by_owner)
        }
        _ => panic!("{label}: one side rejected"),
    };
    assert_eq!(new_owner.len(), old_owner.len(), "{label}: node count");
    for (node, owner) in &old_owner {
        assert_eq!(
            new_owner.get(node),
            Some(owner),
            "{label}: owner of {node:?}"
        );
        for variant in key_variants(*node) {
            assert_eq!(
                new_owner.get(&variant),
                old_owner.get(&variant),
                "{label}: query {variant:?}"
            );
        }
    }
    // Order feeds install order: each owner's nodes must come back in the
    // exact sequence the tree-keyed map yielded.
    assert_eq!(new_by_owner, old_by_owner, "{label}: nodes_by_owner");

    let new_deps = derive_owner_dependencies(directory, dense_owner, dense_node, |node| {
        new_owner.get(node).copied()
    });
    let old_deps = oracle_owner_dependencies(directory, dense_owner, dense_node, &old_owner);
    assert_eq!(new_deps, old_deps, "{label}: owner dependencies");
    true
}

#[test]
fn node_ownership_and_dependencies_match_the_oracle_on_synthetic_manifests() {
    let mut rng = Rng(0x1234_5678_9abc_def1);
    let mut compared = 0;
    let mut multi_owner_nodes = 0;
    for round in 0..8 {
        for dims in GRID_SHAPES {
            for (scaled, invalid_percent) in [(false, 0), (false, 40), (true, 0), (true, 60)] {
                let probes = synthetic_probes(&mut rng, dims, scaled, invalid_percent);
                let base = base_with_probes(dims, probes);
                let layout = oracle_derive(&base).expect("synthetic grids are well formed");
                let probe_count = base.probes.len() as u32;
                for cluster_count in [1, 3, 7] {
                    let directory =
                        synthetic_directory(&mut rng, cluster_count, probe_count, dims, true, true);
                    let dense_owner = lowest_dense_owner(&directory, base.probes.len());
                    let label = format!("round {round} dims {dims:?} clusters {cluster_count}");
                    compared += usize::from(assert_ownership_matches(
                        &directory,
                        &layout.nodes,
                        &dense_owner,
                        dims,
                        &label,
                    ));
                    multi_owner_nodes += usize::from(cluster_count > 1);
                }
            }
        }
    }
    assert!(
        compared > 100,
        "only {compared} manifests reached a full comparison"
    );
    assert!(multi_owner_nodes > 0);
}

#[test]
fn an_unowned_valid_probe_fails_both_derivations_identically() {
    let mut rng = Rng(99);
    let dims = [8, 8, 8];
    let base = base_with_probes(dims, synthetic_probes(&mut rng, dims, false, 0));
    let layout = oracle_derive(&base).unwrap();
    let directory = synthetic_directory(&mut rng, 2, 512, dims, false, false);
    let mut dense_owner = lowest_dense_owner(&directory, 512);
    dense_owner[300] = None;
    dense_owner[7] = None;
    assert!(!assert_ownership_matches(
        &directory,
        &layout.nodes,
        &dense_owner,
        dims,
        "unowned probe"
    ));
}

#[test]
fn from_parts_builds_the_oracle_ownership_tables() {
    let mut rng = Rng(0xfeed_beef);
    let mut built = 0;
    for dims in GRID_SHAPES {
        for (scaled, invalid_percent) in [(false, 30), (true, 30)] {
            let base = base_with_probes(
                dims,
                synthetic_probes(&mut rng, dims, scaled, invalid_percent),
            );
            let probe_count = base.probes.len() as u32;
            // One cluster spans every probe so each valid probe has an owner;
            // the others overlap random spans and so contest the lowest id.
            let mut directory = synthetic_directory(&mut rng, 4, probe_count, dims, false, false);
            let last_cluster = directory.clusters.len() as u32 - 1;
            directory
                .ranges
                .push(dense_range(last_cluster, 0, probe_count));
            let last = directory.clusters.last_mut().unwrap();
            last.range_count += 1;
            // The appended range is the last cluster's final range, so it
            // must also sit at the end of the shared range table.
            assert_eq!(
                last.range_start + last.range_count,
                directory.ranges.len() as u32
            );
            let state = ShResidencyState::from_parts(
                [1; 32],
                &directory,
                &base,
                &postretro_level_loader::ShStreamSourceMetadata::default(),
                &[],
            )
            .expect("synthetic manifest builds a residency state");
            let layout = oracle_derive(&base).unwrap();
            let dense_owner = lowest_dense_owner(&directory, base.probes.len());
            let (owner, by_owner) = oracle_ownership(&layout.nodes, &dense_owner).unwrap();
            assert_eq!(state.dense_node, layout.nodes);
            assert_eq!(state.dense_node_local_slot, layout.local_slots);
            assert_eq!(state.node_owner.len(), owner.len());
            for (node, expected) in &owner {
                assert_eq!(state.node_owner.get(node), Some(expected));
                let layout_entry = &layout.layouts[node];
                assert_eq!(
                    state.node_layouts[node].global_base_slot,
                    layout_entry.global_base_slot
                );
                assert_eq!(state.node_layouts[node].tile_count, layout_entry.tile_count);
            }
            assert_eq!(state.nodes_by_owner, by_owner);
            let deps =
                oracle_owner_dependencies(&directory, &dense_owner, &layout.nodes, &owner).unwrap();
            assert_eq!(state.owner_dependencies, deps);
            built += 1;
        }
    }
    assert!(built >= 18);
}
