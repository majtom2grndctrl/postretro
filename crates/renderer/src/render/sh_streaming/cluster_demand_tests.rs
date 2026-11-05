//! Each cluster's own demand, read from id 49, against a tree-keyed oracle.
//! See: context/lib/rendering_pipeline.md §4 (Cluster SH residency)

use std::collections::{BTreeMap, BTreeSet};

use postretro_level_format::cluster_directory::{
    ClusterRangeRecord, ClusterRecord, ClusterResourceDomain, ClusterResourceRecord,
};
use postretro_level_loader::{
    ShStreamBaseMetadata, ShStreamSourceMetadata, ShStreamSparseMetadata,
};

use super::stored_nodes::equivalence_tests::{
    GRID_SHAPES, Rng, base_with_probes, oracle_derive, synthetic_probes,
};
use super::*;

/// The tiles of the largest single cluster's distinct stored nodes, at least
/// one, or the failure the plan owes this directory.
fn oracle_minimum_slots(
    directory: &ClusterDirectorySection,
    dense_node: &[Option<StoredNode>],
    tile_counts: &BTreeMap<StoredNode, u32>,
) -> Result<u32, ShResidencyDrainError> {
    let mut covered = vec![false; dense_node.len()];
    let mut minimum = 1;
    for (cluster_id, cluster) in directory.clusters.iter().enumerate() {
        let span =
            cluster.range_start as usize..(cluster.range_start + cluster.range_count) as usize;
        let mut nodes = BTreeSet::new();
        for range in &directory.ranges[span] {
            if range.resource_index != 0 {
                continue;
            }
            for dense in range.start..range.start + range.count {
                let Some(node) = dense_node.get(dense as usize) else {
                    return Err(ShResidencyDrainError::MalformedChunk {
                        cluster_id: cluster_id as u32,
                        reason: "dense range exceeds id-34 metadata",
                    });
                };
                covered[dense as usize] = true;
                nodes.extend(node.iter().copied());
            }
        }
        minimum = minimum.max(nodes.iter().map(|node| tile_counts[node]).sum::<u32>());
    }
    match (0..dense_node.len()).find(|&dense| dense_node[dense].is_some() && !covered[dense]) {
        Some(dense) => Err(ShResidencyDrainError::UncoveredDenseProbe {
            dense_index: dense as u32,
        }),
        None => Ok(minimum),
    }
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

fn dense_range(start: u32, count: u32) -> ClusterRangeRecord {
    ClusterRangeRecord {
        resource_index: 0,
        start,
        count,
    }
}

/// `cluster_count` clusters whose dense ranges cover random overlapping spans
/// of `probe_count` probes, plus sparse ranges over the one affinity row.
fn synthetic_directory(
    rng: &mut Rng,
    cluster_count: u32,
    probe_count: u32,
    dims: [u32; 3],
    sparse: bool,
) -> ClusterDirectorySection {
    let mut ranges = Vec::new();
    let mut clusters = Vec::new();
    for _ in 0..cluster_count {
        let first = ranges.len() as u32;
        for _ in 0..1 + rng.below(3) {
            let start = rng.below(probe_count);
            ranges.push(dense_range(start, 1 + rng.below(probe_count - start)));
        }
        if sparse {
            for _ in 0..rng.below(3) {
                ranges.push(ClusterRangeRecord {
                    resource_index: 1,
                    start: 0,
                    count: 1,
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
        mandatory_bound: Vec::new(),
    }
}

/// Have the last cluster also name every probe, so each valid probe is
/// covered and the others overlap it.
fn cover_every_probe(directory: &mut ClusterDirectorySection, probe_count: u32) {
    directory.ranges.push(dense_range(0, probe_count));
    let last = directory.clusters.last_mut().unwrap();
    last.range_count += 1;
    // The appended range is the last cluster's final range, so it must also
    // sit at the end of the shared range table.
    assert_eq!(
        last.range_start + last.range_count,
        directory.ranges.len() as u32
    );
}

fn one_row_sources() -> ShStreamSourceMetadata {
    ShStreamSourceMetadata {
        indirect_delta: Some(ShStreamSparseMetadata {
            section_id: INDIRECT_DELTA_ID,
            internal_version: 1,
            affinity_dims: [1, 1, 1],
            tile_dimension: 4,
            tile_border: 1,
            valid_probe_masks: vec![1],
            cell_levels: vec![0],
            affinity_offsets: vec![0, 1],
            affinity_lights: vec![0],
            animation_descriptor_indices: vec![0],
        }),
        ..Default::default()
    }
}

fn tile_counts(base: &ShStreamBaseMetadata) -> BTreeMap<StoredNode, u32> {
    oracle_derive(base)
        .unwrap()
        .layouts
        .iter()
        .map(|(&node, layout)| (node, layout.tile_count))
        .collect()
}

#[test]
fn cluster_demand_matches_the_oracle_on_synthetic_manifests() {
    let mut rng = Rng(0x1234_5678_9abc_def1);
    let mut compared = 0;
    let mut overlapping = 0;
    for round in 0..8 {
        for dims in GRID_SHAPES {
            for (scaled, invalid_percent) in [(false, 0), (false, 40), (true, 0), (true, 60)] {
                let probes = synthetic_probes(&mut rng, dims, scaled, invalid_percent);
                let base = base_with_probes(dims, probes);
                let layout = oracle_derive(&base).expect("synthetic grids are well formed");
                let tiles = tile_counts(&base);
                let probe_count = base.probes.len() as u32;
                for cluster_count in [1, 3, 7] {
                    let label = format!("round {round} dims {dims:?} clusters {cluster_count}");
                    let mut directory =
                        synthetic_directory(&mut rng, cluster_count, probe_count, dims, true);
                    cover_every_probe(&mut directory, probe_count);
                    let expected = oracle_minimum_slots(&directory, &layout.nodes, &tiles)
                        .expect("every probe is covered");
                    let plan = plan_sh_residency_parts(
                        [2; 32],
                        &directory,
                        &base,
                        &one_row_sources(),
                        &[],
                    )
                    .unwrap_or_else(|error| panic!("{label}: {error}"));
                    assert_eq!(plan.initial_dense_slots.minimum, expected, "{label}");
                    // Overlap is common here, so a sum over clusters or a
                    // repeat within one would both show up as a larger floor.
                    let names_the_row = directory
                        .ranges
                        .iter()
                        .any(|range| range.resource_index == 1);
                    assert_eq!(
                        plan.sparse_capacity_floors
                            .get(&INDIRECT_DELTA_ID)
                            .map(|floor| floor.0),
                        names_the_row.then_some(2),
                        "{label}"
                    );
                    compared += 1;
                    overlapping += usize::from(cluster_count > 1);
                }
            }
        }
    }
    assert!(compared > 100, "only {compared} manifests compared");
    assert!(overlapping > 0);
}

#[test]
fn a_valid_probe_no_cluster_names_fails_the_plan_by_index() {
    let mut rng = Rng(99);
    let dims = [8, 8, 8];
    let base = base_with_probes(dims, synthetic_probes(&mut rng, dims, false, 0));
    let layout = oracle_derive(&base).unwrap();
    let tiles = tile_counts(&base);
    // Clusters that stop short of the last probes leave them uncovered.
    let mut directory = synthetic_directory(&mut rng, 2, 512, dims, false);
    directory.ranges.clear();
    directory.ranges.push(dense_range(0, 300));
    directory.ranges.push(dense_range(200, 200));
    directory.clusters[0] = cluster_record(0, 1);
    directory.clusters[1] = cluster_record(1, 1);

    let expected = oracle_minimum_slots(&directory, &layout.nodes, &tiles).unwrap_err();
    assert_eq!(
        expected,
        ShResidencyDrainError::UncoveredDenseProbe { dense_index: 400 }
    );
    assert_eq!(
        plan_sh_residency_parts(
            [2; 32],
            &directory,
            &base,
            &ShStreamSourceMetadata::default(),
            &[]
        )
        .unwrap_err(),
        expected
    );
}

#[test]
fn a_dense_range_past_the_last_probe_fails_the_plan() {
    let mut rng = Rng(7);
    let dims = [8, 8, 8];
    let base = base_with_probes(dims, synthetic_probes(&mut rng, dims, false, 0));
    let mut directory = synthetic_directory(&mut rng, 1, 512, dims, false);
    directory.ranges.clear();
    directory.ranges.push(dense_range(0, 513));
    directory.clusters[0] = cluster_record(0, 1);
    assert_eq!(
        plan_sh_residency_parts(
            [2; 32],
            &directory,
            &base,
            &ShStreamSourceMetadata::default(),
            &[]
        )
        .unwrap_err(),
        ShResidencyDrainError::MalformedChunk {
            cluster_id: 0,
            reason: "dense range exceeds id-34 metadata",
        }
    );
}

#[test]
fn from_parts_builds_the_oracle_node_tables() {
    let mut rng = Rng(0xfeed_beef);
    let mut built = 0;
    for dims in GRID_SHAPES {
        for (scaled, invalid_percent) in [(false, 30), (true, 30)] {
            let base = base_with_probes(
                dims,
                synthetic_probes(&mut rng, dims, scaled, invalid_percent),
            );
            let probe_count = base.probes.len() as u32;
            let mut directory = synthetic_directory(&mut rng, 4, probe_count, dims, false);
            cover_every_probe(&mut directory, probe_count);
            let state = ShResidencyState::from_parts(
                [1; 32],
                &directory,
                &base,
                &ShStreamSourceMetadata::default(),
                &[],
            )
            .expect("synthetic manifest builds a residency state");
            let layout = oracle_derive(&base).unwrap();
            assert_eq!(state.dense_node, layout.nodes);
            assert_eq!(state.dense_node_local_slot, layout.local_slots);
            assert_eq!(state.node_layouts.len(), layout.layouts.len());
            for (node, expected) in &layout.layouts {
                assert_eq!(
                    state.node_layouts[node].global_base_slot,
                    expected.global_base_slot
                );
                assert_eq!(state.node_layouts[node].tile_count, expected.tile_count);
            }
            built += 1;
        }
    }
    assert!(built >= 18);
}
