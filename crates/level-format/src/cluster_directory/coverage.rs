//! Id-49 range-table derivation from cells, probes and the cell locator.
//! Shared by compiler population and load-time validation.
//! See: context/lib/build_pipeline.md §PRL section IDs

use crate::{cells::CellsSection, sh_volume::OctahedralShVolumeSection};

use super::{
    ClusterDirectoryCoverageStats, ClusterDirectoryError, ClusterDirectorySection,
    ClusterDirectoryValidationInputs, ClusterRangeRecord, ClusterRangeRole, ClusterResourceDomain,
    DENSE_OWNER_SENTINEL, brick_coords, brick_probe_bounds, cell_to_cluster, checked_product,
    flatten, locate_cell, probe_coords, probe_locate::ProbeLocator, probe_position,
    resource_mismatch, sparse_offsets, u32_len,
};

/// Unset slot in the per-brick tag arrays below. Cluster ids index a `Vec`
/// of 48-byte records, so no real id reaches it.
const NO_CLUSTER: u32 = u32::MAX;

/// Derive every cluster's canonical ranges, its range count, and coverage
/// stats.
///
/// Exact restatement of the original brick-by-member scan, which the tests
/// keep verbatim as an oracle: the same output, and the same first error, for
/// every input. Cost is linear in probes, in the cell-to-brick overlap
/// volume, in one pass over each axis's bricks per member cell, and in the
/// locator boxes a probe-grid descent splits into (`ProbeLocator`), where the
/// original was O(active bricks × member cells) plus one locator walk per
/// valid probe.
///
/// Error order matches the original because errors come from two places
/// only: locating probes (per active brick ascending, probes in brick order,
/// then the entry-bearing fallback) and node closure (per cluster, in queue
/// order). The cell-bounds pass that replaced the per-brick member scan
/// raises none, so it runs ahead of the brick loop.
///
/// Requires every member to index `inputs.cells` (validate_cells and the
/// structure check establish it on the load path).
pub(super) fn derive_ranges_with_counts(
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
    let grid = base.grid_dimensions;
    let affinity_count = checked_product(affinity_dims)? as usize;
    let mut brick_has_valid = vec![false; affinity_count];
    let mut probe_index = 0usize;
    for z in 0..grid[2] {
        for y in 0..grid[1] {
            let row = (affinity_dims[0] * (y / 4 + affinity_dims[1] * (z / 4))) as usize;
            for x in 0..grid[0] {
                if base.probes[probe_index].validity != 0 {
                    brick_has_valid[row + (x / 4) as usize] = true;
                }
                probe_index += 1;
            }
        }
    }
    let mut active = brick_has_valid.clone();
    let mut entry_bearing = vec![false; affinity_count];
    for offsets in sparse_offsets(inputs.sh) {
        for (cell, pair) in offsets.windows(2).enumerate() {
            if pair[0] != pair[1] {
                active[cell] = true;
                entry_bearing[cell] = true;
            }
        }
    }

    let cell_to_cluster = cell_to_cluster(directory);
    let mut coverage = vec![Vec::<u32>::new(); directory.clusters.len()];
    let mut cell_covered = vec![false; affinity_count];
    // The original read member cells only while visiting an active brick.
    if active.contains(&true) {
        cover_from_cell_bounds(
            directory,
            inputs.cells,
            base,
            affinity_dims,
            &active,
            &mut coverage,
            &mut cell_covered,
        );
    }

    // Locate every valid probe in one batched descent. On a locator fault,
    // fall back to the per-probe walk in brick order, which returns the
    // first fault the original returned.
    let mut located = Vec::new();
    let batched = ProbeLocator::new(inputs.cell_locator, directory.runtime_cell_count, base)
        .locate_grid(affinity_dims, &mut located)
        .is_ok();
    if batched {
        for &(brick, cell) in &located {
            coverage[cell_to_cluster[cell as usize] as usize].push(brick);
        }
    }
    for (brick_index, &is_active) in active.iter().enumerate() {
        if !is_active {
            continue;
        }
        let brick = brick_index as u32;
        let mut covered_any = cell_covered[brick_index];
        if batched {
            covered_any |= brick_has_valid[brick_index];
        } else {
            let (min_probe, max_probe) = brick_probe_bounds(brick, affinity_dims, grid)?;
            for z in min_probe[2]..=max_probe[2] {
                for y in min_probe[1]..=max_probe[1] {
                    for x in min_probe[0]..=max_probe[0] {
                        let probe = x + grid[0] * (y + grid[1] * z);
                        if base.probes[probe as usize].validity == 0 {
                            continue;
                        }
                        let cell = locate_cell(
                            inputs.cell_locator,
                            directory.runtime_cell_count,
                            probe_position(probe_coords(probe, grid), base),
                        )?;
                        coverage[cell_to_cluster[cell as usize] as usize].push(brick);
                        covered_any = true;
                    }
                }
            }
        }
        if entry_bearing[brick_index] && !covered_any {
            let origin = std::array::from_fn(|axis| {
                (brick_coords(brick, affinity_dims)[axis] * 4).min(grid[axis] - 1)
            });
            let cell = locate_cell(
                inputs.cell_locator,
                directory.runtime_cell_count,
                probe_position(origin, base),
            )?;
            coverage[cell_to_cluster[cell as usize] as usize].push(brick);
        }
    }

    let maximum_visited_nodes_per_cluster =
        close_over_adaptive_nodes(&mut coverage, base, affinity_dims, affinity_count)?;

    let mut owners = vec![NO_CLUSTER; affinity_count];
    for (cluster, bricks) in coverage.iter().enumerate() {
        for &brick in bricks {
            let owner = &mut owners[brick as usize];
            if *owner == NO_CLUSTER {
                *owner = cluster as u32;
            }
        }
    }
    let mut result = Vec::new();
    let mut counts = Vec::with_capacity(directory.clusters.len());
    let mut dense_runs = Vec::new();
    let mut cluster_ranges = Vec::new();
    let has_dense = directory
        .resources
        .iter()
        .any(|resource| resource.domain == ClusterResourceDomain::DenseProbe);
    for (cluster_id, bricks) in coverage.iter().enumerate() {
        cluster_ranges.clear();
        dense_runs.clear();
        if has_dense {
            dense_probe_runs(bricks, affinity_dims, grid, &mut dense_runs);
        }
        for (resource_index, resource) in directory.resources.iter().enumerate() {
            let resource_index = resource_index as u32;
            match resource.domain {
                ClusterResourceDomain::DenseProbe => {
                    cluster_ranges.extend(dense_runs.iter().map(|&(start, end)| {
                        ClusterRangeRecord {
                            resource_index,
                            start,
                            count: end - start,
                            owner_cluster_id: DENSE_OWNER_SENTINEL,
                            role: ClusterRangeRole::Dense,
                        }
                    }));
                }
                ClusterResourceDomain::AffinityCell => {
                    push_affinity_runs(
                        bricks,
                        &owners,
                        cluster_id as u32,
                        resource_index,
                        &mut cluster_ranges,
                    );
                }
            }
        }
        // Sorted by (resource, start) by construction: resources in table
        // order, each one's runs ascending.
        counts.push(u32_len(cluster_ranges.len(), "cluster range count")?);
        result.extend_from_slice(&cluster_ranges);
    }
    let stats = ClusterDirectoryCoverageStats {
        affinity_cell_count: affinity_count,
        active_affinity_cell_count: active.iter().filter(|&&value| value).count(),
        covering_references: coverage.iter().map(Vec::len).sum(),
        maximum_visited_nodes_per_cluster,
    };
    Ok((result, counts, stats))
}

/// Record every active brick whose probe support overlaps a member cell's
/// bounds expanded by one probe spacing, per cluster; skips solid and
/// exterior cells.
///
/// The overlap test is separable: a brick passes iff it passes on each axis,
/// and each axis term depends only on the brick's coordinate on that axis.
/// So each cell scans each axis's brick coordinates once and visits only the
/// passing product. The per-axis support values and comparisons are the
/// original expressions verbatim, so boundary contact, zero-volume and
/// non-finite bounds resolve exactly as before; nothing assumes monotone
/// support values.
fn cover_from_cell_bounds(
    directory: &ClusterDirectorySection,
    cells: &CellsSection,
    base: &OctahedralShVolumeSection,
    affinity_dims: [u32; 3],
    active: &[bool],
    coverage: &mut [Vec<u32>],
    cell_covered: &mut [bool],
) {
    let grid = base.grid_dimensions;
    let support: [Vec<(f32, f32)>; 3] = std::array::from_fn(|axis| {
        (0..affinity_dims[axis])
            .map(|brick| {
                let min_probe = brick * 4;
                let max_probe = (min_probe + 3).min(grid[axis] - 1);
                let min_center = base.grid_origin[axis] + min_probe as f32 * base.cell_size[axis];
                let max_center = base.grid_origin[axis] + max_probe as f32 * base.cell_size[axis];
                (
                    min_center - 0.5 * base.cell_size[axis],
                    max_center + 0.5 * base.cell_size[axis],
                )
            })
            .collect()
    });
    let mut recorded_for = vec![NO_CLUSTER; active.len()];
    let mut passing: [Vec<u32>; 3] = Default::default();
    for (cluster_id, cluster) in directory.clusters.iter().enumerate() {
        let begin = cluster.member_start as usize;
        let end = begin + cluster.member_count as usize;
        for &cell_id in &directory.members[begin..end] {
            let cell = &cells.cells[cell_id as usize];
            if cell.is_solid() || cell.is_exterior() {
                continue;
            }
            for axis in 0..3 {
                let expanded_min = cell.bounds_min[axis] - base.cell_size[axis];
                let expanded_max = cell.bounds_max[axis] + base.cell_size[axis];
                passing[axis].clear();
                passing[axis].extend(
                    support[axis]
                        .iter()
                        .enumerate()
                        .filter(|(_, (support_min, support_max))| {
                            *support_min <= expanded_max && expanded_min <= *support_max
                        })
                        .map(|(brick, _)| brick as u32),
                );
            }
            for &z in &passing[2] {
                for &y in &passing[1] {
                    let row = affinity_dims[0] * (y + affinity_dims[1] * z);
                    for &x in &passing[0] {
                        let brick = (row + x) as usize;
                        if !active[brick] {
                            continue;
                        }
                        cell_covered[brick] = true;
                        if recorded_for[brick] != cluster_id as u32 {
                            recorded_for[brick] = cluster_id as u32;
                            coverage[cluster_id].push(brick as u32);
                        }
                    }
                }
            }
        }
    }
}

/// Close each cluster's coverage over the adaptive nodes its probes name, and
/// leave it sorted and unique. Returns the most nodes any one cluster visits.
///
/// Walks the original breadth-first order (ascending seeds, then discovery
/// order, probes in brick order) so the first scale or node-bound error is
/// the original's. Per-cluster sets become tag arrays over the brick grid.
fn close_over_adaptive_nodes(
    coverage: &mut [Vec<u32>],
    base: &OctahedralShVolumeSection,
    affinity_dims: [u32; 3],
    affinity_count: usize,
) -> Result<usize, ClusterDirectoryError> {
    let grid = base.grid_dimensions;
    let mut in_coverage = vec![NO_CLUSTER; affinity_count];
    // One slot per (scale 0..=3, node origin brick).
    let mut visited = vec![NO_CLUSTER; affinity_count * 4];
    let mut maximum_visited_nodes = 0usize;
    for (cluster_id, bricks) in coverage.iter_mut().enumerate() {
        let tag = cluster_id as u32;
        bricks.sort_unstable();
        bricks.dedup();
        for &brick in bricks.iter() {
            in_coverage[brick as usize] = tag;
        }
        let mut visited_nodes = 0usize;
        let mut head = 0;
        while head < bricks.len() {
            let brick = bricks[head];
            head += 1;
            let (min_probe, max_probe) = brick_probe_bounds(brick, affinity_dims, grid)?;
            // Every probe of this brick maps back to it, so its node origin
            // and slot depend on its scale alone.
            let brick_xyz = brick_coords(brick, affinity_dims);
            let origins: [[u32; 3]; 4] =
                std::array::from_fn(|scale| brick_xyz.map(|coord| (coord >> scale) << scale));
            let mut nodes = [0usize; 4];
            for (scale, origin) in origins.iter().enumerate() {
                nodes[scale] = scale * affinity_count + flatten(*origin, affinity_dims)? as usize;
            }
            for z in min_probe[2]..=max_probe[2] {
                for y in min_probe[1]..=max_probe[1] {
                    for x in min_probe[0]..=max_probe[0] {
                        let probe_index = x + grid[0] * (y + grid[1] * z);
                        let scale = base.probes[probe_index as usize].node_scale;
                        if scale > 3 {
                            return resource_mismatch(format!(
                                "id 34 probe {probe_index} node_scale {scale} exceeds 3"
                            ));
                        }
                        let span = 1u32 << scale;
                        let origin = origins[scale as usize];
                        let node = nodes[scale as usize];
                        if visited[node] == tag {
                            continue;
                        }
                        visited[node] = tag;
                        visited_nodes += 1;
                        if scale > 0 {
                            for axis in 0..3 {
                                let max_probe = origin[axis]
                                    .checked_add(span)
                                    .and_then(|v| v.checked_mul(4))
                                    .and_then(|v| v.checked_sub(1))
                                    .ok_or(ClusterDirectoryError::SizeOverflow(
                                        "adaptive node bound",
                                    ))?;
                                if max_probe >= grid[axis] {
                                    return resource_mismatch(format!(
                                        "id 34 adaptive node {origin:?}/scale {scale} exceeds grid {:?}",
                                        grid
                                    ));
                                }
                            }
                        }
                        for member_z in origin[2]..origin[2] + span {
                            for member_y in origin[1]..origin[1] + span {
                                for member_x in origin[0]..origin[0] + span {
                                    let member =
                                        flatten([member_x, member_y, member_z], affinity_dims)?;
                                    if in_coverage[member as usize] != tag {
                                        in_coverage[member as usize] = tag;
                                        bricks.push(member);
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
        maximum_visited_nodes = maximum_visited_nodes.max(visited_nodes);
        bricks.sort_unstable();
    }
    Ok(maximum_visited_nodes)
}

/// Maximal runs `[start, end)` of linear probe indices inside `bricks`
/// (sorted, unique), in ascending order.
///
/// Linear probe order is (z, y, x). Bricks sort by (brick z, brick y, brick
/// x), so walking each brick plane's probe planes, then each brick row's
/// probe rows, then the row's bricks, emits probe spans in ascending order;
/// adjacent spans merge, including across row and plane wrap-around.
fn dense_probe_runs(
    bricks: &[u32],
    affinity_dims: [u32; 3],
    grid: [u32; 3],
    runs: &mut Vec<(u32, u32)>,
) {
    let plane_bricks = affinity_dims[0] * affinity_dims[1];
    let mut plane_begin = 0;
    while plane_begin < bricks.len() {
        let brick_z = bricks[plane_begin] / plane_bricks;
        let plane_end = plane_begin
            + bricks[plane_begin..]
                .iter()
                .take_while(|&&brick| brick / plane_bricks == brick_z)
                .count();
        let plane = &bricks[plane_begin..plane_end];
        let z_min = brick_z * 4;
        for z in z_min..=(z_min + 3).min(grid[2] - 1) {
            let mut row_begin = 0;
            while row_begin < plane.len() {
                let brick_y = (plane[row_begin] / affinity_dims[0]) % affinity_dims[1];
                let row_end = row_begin
                    + plane[row_begin..]
                        .iter()
                        .take_while(|&&brick| {
                            (brick / affinity_dims[0]) % affinity_dims[1] == brick_y
                        })
                        .count();
                let y_min = brick_y * 4;
                for y in y_min..=(y_min + 3).min(grid[1] - 1) {
                    let row_base = grid[0] * (y + grid[1] * z);
                    for &brick in &plane[row_begin..row_end] {
                        let x_min = (brick % affinity_dims[0]) * 4;
                        let x_max = (x_min + 3).min(grid[0] - 1);
                        let start = row_base + x_min;
                        let end = row_base + x_max + 1;
                        match runs.last_mut() {
                            Some(last) if last.1 == start => last.1 = end,
                            _ => runs.push((start, end)),
                        }
                    }
                }
                row_begin = row_end;
            }
        }
        plane_begin = plane_end;
    }
}

/// Maximal runs of consecutive bricks with one owner, as `Owned` when the
/// owner is this cluster and `Halo` otherwise.
fn push_affinity_runs(
    bricks: &[u32],
    owners: &[u32],
    cluster_id: u32,
    resource_index: u32,
    ranges: &mut Vec<ClusterRangeRecord>,
) {
    let mut start = 0;
    while start < bricks.len() {
        let first = bricks[start];
        let owner = owners[first as usize];
        let mut end = start + 1;
        while end < bricks.len()
            && bricks[end] == bricks[end - 1] + 1
            && owners[bricks[end] as usize] == owner
        {
            end += 1;
        }
        ranges.push(ClusterRangeRecord {
            resource_index,
            start: first,
            count: (end - start) as u32,
            owner_cluster_id: owner,
            role: if owner == cluster_id {
                ClusterRangeRole::Owned
            } else {
                ClusterRangeRole::Halo
            },
        });
        start = end;
    }
}
