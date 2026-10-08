// The original id-49 range-table derivation, kept verbatim as the oracle for
// the coverage module. O(active bricks × member cells); tests only.
// See: context/lib/build_pipeline.md §PRL section IDs

use std::collections::{BTreeMap, BTreeSet, VecDeque};

use super::*;

/// The original tail of `validate_resources`: derive, check each cluster's
/// range slice, then compare the whole table.
pub(super) fn oracle_validate_range_table(
    directory: &ClusterDirectorySection,
    inputs: ClusterDirectoryValidationInputs<'_>,
    base: &OctahedralShVolumeSection,
    affinity_dims: [u32; 3],
) -> Result<(), ClusterDirectoryError> {
    let expected_ranges = oracle_derive_expected_ranges(directory, inputs, base, affinity_dims)?;
    if expected_ranges != directory.ranges {
        let first = expected_ranges
            .iter()
            .zip(&directory.ranges)
            .position(|(expected, actual)| expected != actual)
            .unwrap_or(expected_ranges.len().min(directory.ranges.len()));
        return Err(ClusterDirectoryError::ResourceMismatch(format!(
            "range table disagrees with cell/grid coverage at range {first}: expected {} ranges, got {}",
            expected_ranges.len(),
            directory.ranges.len()
        )));
    }
    Ok(())
}

fn oracle_derive_expected_ranges(
    directory: &ClusterDirectorySection,
    inputs: ClusterDirectoryValidationInputs<'_>,
    base: &OctahedralShVolumeSection,
    affinity_dims: [u32; 3],
) -> Result<Vec<ClusterRangeRecord>, ClusterDirectoryError> {
    let (result, counts, _) =
        oracle_derive_ranges_with_counts(directory, inputs, base, affinity_dims)?;
    let mut expected_start = 0usize;
    for (cluster_id, (cluster, count)) in directory.clusters.iter().zip(counts).enumerate() {
        let canonical_start = if count == 0 { 0 } else { expected_start };
        if cluster.range_start as usize != canonical_start || cluster.range_count != count {
            return resource_mismatch(format!(
                "cluster {cluster_id} range slice does not match derived coverage"
            ));
        }
        expected_start += count as usize;
    }
    Ok(result)
}

pub(super) fn oracle_derive_ranges_with_counts(
    directory: &ClusterDirectorySection,
    inputs: ClusterDirectoryValidationInputs<'_>,
    base: &OctahedralShVolumeSection,
    affinity_dims: [u32; 3],
) -> Result<
    (
        Vec<ClusterRangeRecord>,
        Vec<u32>,
        ClusterDirectoryCoverageStats,
    ),
    ClusterDirectoryError,
> {
    let probe_count = checked_product(base.grid_dimensions)? as usize;
    if base.probes.len() != probe_count {
        return resource_mismatch(format!(
            "id 34 probe metadata count {} disagrees with grid product {probe_count}",
            base.probes.len()
        ));
    }
    if base.grid_dimensions == [0, 0, 0] {
        if directory
            .clusters
            .iter()
            .any(|cluster| cluster.range_count != 0)
        {
            return resource_mismatch("zero-grid id 34 must have no directory ranges");
        }
        return Ok((
            Vec::new(),
            vec![0; directory.clusters.len()],
            ClusterDirectoryCoverageStats::default(),
        ));
    }
    if base
        .cell_size
        .iter()
        .any(|value| !value.is_finite() || *value <= 0.0)
    {
        return resource_mismatch("id 34 cell_size must be finite and positive");
    }
    let affinity_count = checked_product(affinity_dims)? as usize;
    let mut active = vec![false; affinity_count];
    let mut entry_bearing = vec![false; affinity_count];
    for (probe_index, probe) in base.probes.iter().enumerate() {
        if probe.validity != 0 {
            active[probe_to_brick_index(probe_index as u32, base.grid_dimensions, affinity_dims)?
                as usize] = true;
        }
    }
    for offsets in sparse_offsets(inputs.sh) {
        for (cell, pair) in offsets.windows(2).enumerate() {
            if pair[0] != pair[1] {
                active[cell] = true;
                entry_bearing[cell] = true;
            }
        }
    }

    let cell_to_cluster = cell_to_cluster(directory);
    let mut coverage = vec![BTreeSet::<u32>::new(); directory.clusters.len()];
    for (brick_index, &is_active) in active.iter().enumerate() {
        if !is_active {
            continue;
        }
        let brick_index = brick_index as u32;
        let (min_probe, max_probe) =
            brick_probe_bounds(brick_index, affinity_dims, base.grid_dimensions)?;
        let min_center = probe_position(min_probe, base);
        let max_center = probe_position(max_probe, base);
        let support_min = std::array::from_fn(|axis| min_center[axis] - 0.5 * base.cell_size[axis]);
        let support_max = std::array::from_fn(|axis| max_center[axis] + 0.5 * base.cell_size[axis]);
        let mut covered_any = false;
        for (cluster_id, cluster) in directory.clusters.iter().enumerate() {
            let begin = cluster.member_start as usize;
            let end = begin + cluster.member_count as usize;
            for &cell_id in &directory.members[begin..end] {
                let cell = &inputs.cells.cells[cell_id as usize];
                if cell.is_solid() || cell.is_exterior() {
                    continue;
                }
                let expanded_min =
                    std::array::from_fn(|axis| cell.bounds_min[axis] - base.cell_size[axis]);
                let expanded_max =
                    std::array::from_fn(|axis| cell.bounds_max[axis] + base.cell_size[axis]);
                if aabb_intersects(support_min, support_max, expanded_min, expanded_max) {
                    coverage[cluster_id].insert(brick_index);
                    covered_any = true;
                    break;
                }
            }
        }
        for probe in probes_in_brick(min_probe, max_probe, base.grid_dimensions) {
            if base.probes[probe as usize].validity == 0 {
                continue;
            }
            let cell = oracle_locate_cell(
                inputs.cell_locator,
                directory.runtime_cell_count,
                probe_position(probe_coords(probe, base.grid_dimensions), base),
            )?;
            coverage[cell_to_cluster[cell as usize] as usize].insert(brick_index);
            covered_any = true;
        }
        if entry_bearing[brick_index as usize] && !covered_any {
            let origin = std::array::from_fn(|axis| {
                (brick_coords(brick_index, affinity_dims)[axis] * 4)
                    .min(base.grid_dimensions[axis] - 1)
            });
            let cell = oracle_locate_cell(
                inputs.cell_locator,
                directory.runtime_cell_count,
                probe_position(origin, base),
            )?;
            coverage[cell_to_cluster[cell as usize] as usize].insert(brick_index);
        }
    }

    let mut maximum_visited_nodes_per_cluster = 0usize;
    for cluster_coverage in &mut coverage {
        let mut queue: VecDeque<u32> = cluster_coverage.iter().copied().collect();
        let mut visited_nodes = BTreeSet::new();
        while let Some(brick) = queue.pop_front() {
            let (min_probe, max_probe) =
                brick_probe_bounds(brick, affinity_dims, base.grid_dimensions)?;
            for probe_index in probes_in_brick(min_probe, max_probe, base.grid_dimensions) {
                let scale = base.probes[probe_index as usize].node_scale;
                if scale > 3 {
                    return resource_mismatch(format!(
                        "id 34 probe {probe_index} node_scale {scale} exceeds 3"
                    ));
                }
                let brick_xyz = brick_coords(
                    probe_to_brick_index(probe_index, base.grid_dimensions, affinity_dims)?,
                    affinity_dims,
                );
                let span = 1u32 << scale;
                let origin: [u32; 3] = std::array::from_fn(|axis| (brick_xyz[axis] / span) * span);
                if !visited_nodes.insert((origin, scale)) {
                    continue;
                }
                if scale > 0 {
                    for axis in 0..3 {
                        let max_probe = origin[axis]
                            .checked_add(span)
                            .and_then(|v| v.checked_mul(4))
                            .and_then(|v| v.checked_sub(1))
                            .ok_or(ClusterDirectoryError::SizeOverflow("adaptive node bound"))?;
                        if max_probe >= base.grid_dimensions[axis] {
                            return resource_mismatch(format!(
                                "id 34 adaptive node {origin:?}/scale {scale} exceeds grid {:?}",
                                base.grid_dimensions
                            ));
                        }
                    }
                }
                for z in origin[2]..origin[2] + span {
                    for y in origin[1]..origin[1] + span {
                        for x in origin[0]..origin[0] + span {
                            let member = flatten([x, y, z], affinity_dims)?;
                            if cluster_coverage.insert(member) {
                                queue.push_back(member);
                            }
                        }
                    }
                }
            }
        }
        maximum_visited_nodes_per_cluster =
            maximum_visited_nodes_per_cluster.max(visited_nodes.len());
    }

    let mut owners = BTreeMap::<u32, u32>::new();
    for (cluster, bricks) in coverage.iter().enumerate() {
        for &brick in bricks {
            owners.entry(brick).or_insert(cluster as u32);
        }
    }
    let mut result = Vec::new();
    let mut counts = Vec::with_capacity(directory.clusters.len());
    for (cluster_id, bricks) in coverage.iter().enumerate() {
        let mut cluster_ranges = Vec::new();
        for (resource_index, resource) in directory.resources.iter().enumerate() {
            let indices: Vec<u32> = match resource.domain {
                ClusterResourceDomain::AffinityCell => bricks.iter().copied().collect(),
                ClusterResourceDomain::DenseProbe => bricks
                    .iter()
                    .flat_map(|&brick| {
                        let (min, max) =
                            brick_probe_bounds(brick, affinity_dims, base.grid_dimensions)
                                .expect("validated dimensions");
                        probes_in_brick(min, max, base.grid_dimensions)
                    })
                    .collect(),
            };
            let mut indices = indices;
            indices.sort_unstable();
            indices.dedup();
            let mut start = 0;
            while start < indices.len() {
                let first = indices[start];
                let (role, owner) = if resource.domain == ClusterResourceDomain::DenseProbe {
                    (ClusterRangeRole::Dense, DENSE_OWNER_SENTINEL)
                } else {
                    let owner = owners[&first];
                    (
                        if owner == cluster_id as u32 {
                            ClusterRangeRole::Owned
                        } else {
                            ClusterRangeRole::Halo
                        },
                        owner,
                    )
                };
                let mut end = start + 1;
                while end < indices.len() && indices[end] == indices[end - 1] + 1 {
                    if resource.domain == ClusterResourceDomain::AffinityCell
                        && owners[&indices[end]] != owner
                    {
                        break;
                    }
                    end += 1;
                }
                cluster_ranges.push(ClusterRangeRecord {
                    resource_index: resource_index as u32,
                    start: first,
                    count: (end - start) as u32,
                    owner_cluster_id: owner,
                    role,
                });
                start = end;
            }
        }
        cluster_ranges.sort_by_key(|range| (range.resource_index, range.start));
        counts.push(u32_len(cluster_ranges.len(), "cluster range count")?);
        result.extend(cluster_ranges);
    }
    let stats = ClusterDirectoryCoverageStats {
        affinity_cell_count: affinity_count,
        active_affinity_cell_count: active.iter().filter(|&&value| value).count(),
        covering_references: coverage.iter().map(BTreeSet::len).sum(),
        maximum_visited_nodes_per_cluster,
    };
    Ok((result, counts, stats))
}

fn probe_to_brick_index(
    probe: u32,
    grid_dims: [u32; 3],
    affinity_dims: [u32; 3],
) -> Result<u32, ClusterDirectoryError> {
    let coords = probe_coords(probe, grid_dims);
    flatten([coords[0] / 4, coords[1] / 4, coords[2] / 4], affinity_dims)
}

fn probes_in_brick(min: [u32; 3], max: [u32; 3], grid_dims: [u32; 3]) -> impl Iterator<Item = u32> {
    let mut probes = Vec::new();
    for z in min[2]..=max[2] {
        for y in min[1]..=max[1] {
            for x in min[0]..=max[0] {
                probes.push(x + grid_dims[0] * (y + grid_dims[1] * z));
            }
        }
    }
    probes.into_iter()
}

fn aabb_intersects(a_min: [f32; 3], a_max: [f32; 3], b_min: [f32; 3], b_max: [f32; 3]) -> bool {
    (0..3).all(|axis| a_min[axis] <= b_max[axis] && b_min[axis] <= a_max[axis])
}

fn oracle_locate_cell(
    locator: &CellLocatorSection,
    runtime_cell_count: u32,
    point: [f32; 3],
) -> Result<u32, ClusterDirectoryError> {
    let mut child = locator.root;
    let mut remaining = locator.nodes.len() + 1;
    loop {
        if remaining == 0 {
            return invalid("cell locator traversal did not terminate");
        }
        remaining -= 1;
        match child {
            CellLocatorChild::Cell(cell) => {
                if cell >= runtime_cell_count {
                    return invalid(format!(
                        "cell locator terminal cell {cell} outside {runtime_cell_count} runtime cells"
                    ));
                }
                return Ok(cell);
            }
            CellLocatorChild::Node(index) => {
                let node = locator.nodes.get(index as usize).ok_or_else(|| {
                    ClusterDirectoryError::InvalidData(format!(
                        "cell locator node {index} is out of range"
                    ))
                })?;
                let signed = node.plane_normal[0] * point[0]
                    + node.plane_normal[1] * point[1]
                    + node.plane_normal[2] * point[2]
                    - node.plane_distance;
                child = if signed >= 0.0 { node.front } else { node.back };
            }
        }
    }
}
