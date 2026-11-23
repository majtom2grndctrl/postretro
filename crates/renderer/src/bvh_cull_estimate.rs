// CPU estimate of the BVH tree-walk cull's work over the CPU BVH mirrors, for
// the dev-tools Spatial diagnostics; read-only, whichever GPU cull ran.
// See: context/lib/rendering_pipeline.md §7.1

use postretro_render_data::geometry::{BVH_NODE_FLAG_LEAF, BucketRange, BvhLeaf, BvhNode};

use crate::compute_cull::{MAX_VISIBLE_CELLS, VISIBLE_CELLS_WORDS};

const FRONTIER_TARGET_SUBTREES: usize = 64;

#[derive(Debug, Clone, Copy, Default, PartialEq)]
#[cfg_attr(not(feature = "dev-tools"), allow(dead_code))]
pub struct BvhFrontierDiagnostics {
    pub frontier_subtrees: u32,
    pub total_estimated_work: u32,
    pub max_subtree_work: u32,
    pub imbalance_ratio: f32,
}

#[derive(Debug, Clone, Copy, Default, PartialEq)]
#[cfg_attr(not(feature = "dev-tools"), allow(dead_code))]
pub struct BvhCullDiagnostics {
    pub estimated_node_visits: u32,
    pub leaf_tests: u32,
    pub frustum_rejects: u32,
    pub visible_cell_rejects: u32,
    pub submitted_leaves: u32,
    pub submitted_index_count: u32,
    pub submitted_bucket_spans: u32,
    pub frontier: BvhFrontierDiagnostics,
}


fn build_visible_cell_bitmask(
    visible: &postretro_visibility::VisibleCells,
) -> [u32; VISIBLE_CELLS_WORDS] {
    let mut bitmask = [0u32; VISIBLE_CELLS_WORDS];
    match visible {
        postretro_visibility::VisibleCells::Culled(cells) => {
            for &cell in cells {
                if cell >= MAX_VISIBLE_CELLS {
                    continue;
                }
                let word = (cell >> 5) as usize;
                let bit = 1u32 << (cell & 31);
                bitmask[word] |= bit;
            }
        }
        postretro_visibility::VisibleCells::DrawAll => bitmask.fill(u32::MAX),
    }
    bitmask
}

fn bitmask_cell_is_visible(bitmask: &[u32; VISIBLE_CELLS_WORDS], cell_id: u32) -> bool {
    let word = (cell_id >> 5) as usize;
    if word >= bitmask.len() {
        return false;
    }
    let bit = 1u32 << (cell_id & 31);
    (bitmask[word] & bit) != 0
}

fn is_aabb_outside_gpu_planes(
    aabb_min: [f32; 3],
    aabb_max: [f32; 3],
    planes: &[[f32; 4]; 6],
) -> bool {
    for plane in planes {
        let px = if plane[0] >= 0.0 {
            aabb_max[0]
        } else {
            aabb_min[0]
        };
        let py = if plane[1] >= 0.0 {
            aabb_max[1]
        } else {
            aabb_min[1]
        };
        let pz = if plane[2] >= 0.0 {
            aabb_max[2]
        } else {
            aabb_min[2]
        };
        if plane[0] * px + plane[1] * py + plane[2] * pz + plane[3] < 0.0 {
            return true;
        }
    }
    false
}

fn next_skip_index(current: usize, skip_index: u32, limit: usize) -> usize {
    let next = skip_index as usize;
    debug_assert!(
        next > current || next >= limit,
        "flat BVH skip_index must advance traversal"
    );
    if next > current {
        next.min(limit)
    } else {
        limit
    }
}

fn estimate_subtree_work(
    nodes: &[BvhNode],
    leaves: &[BvhLeaf],
    planes: &[[f32; 4]; 6],
    visible_bitmask: &[u32; VISIBLE_CELLS_WORDS],
    root: usize,
    end: usize,
) -> BvhCullDiagnostics {
    let mut diagnostics = BvhCullDiagnostics::default();
    let mut i = root;
    let end = end.min(nodes.len());

    while i < end {
        let node = nodes[i];
        diagnostics.estimated_node_visits += 1;
        if is_aabb_outside_gpu_planes(node.aabb_min, node.aabb_max, planes) {
            diagnostics.frustum_rejects += 1;
            i = next_skip_index(i, node.skip_index, end);
            continue;
        }

        if (node.flags & BVH_NODE_FLAG_LEAF) != 0 {
            let leaf_idx = node.left_child_or_leaf_index as usize;
            if let Some(leaf) = leaves.get(leaf_idx) {
                diagnostics.leaf_tests += 1;
                if is_aabb_outside_gpu_planes(leaf.aabb_min, leaf.aabb_max, planes) {
                    diagnostics.frustum_rejects += 1;
                } else if !bitmask_cell_is_visible(visible_bitmask, leaf.cell_id) {
                    diagnostics.visible_cell_rejects += 1;
                } else {
                    diagnostics.submitted_leaves += 1;
                    diagnostics.submitted_index_count = diagnostics
                        .submitted_index_count
                        .saturating_add(leaf.index_count);
                }
            }
            i = next_skip_index(i, node.skip_index, end);
        } else {
            i += 1;
        }
    }

    diagnostics
}

fn child_roots(nodes: &[BvhNode], root: usize) -> Option<(usize, usize)> {
    let node = nodes.get(root)?;
    if (node.flags & BVH_NODE_FLAG_LEAF) != 0 {
        return None;
    }
    let left = root + 1;
    let left_node = nodes.get(left)?;
    let right = left_node.skip_index as usize;
    if right <= left || right >= node.skip_index as usize || right >= nodes.len() {
        return None;
    }
    Some((left, right))
}

fn fixed_frontier_roots(nodes: &[BvhNode]) -> Vec<usize> {
    if nodes.is_empty() {
        return Vec::new();
    }

    let mut frontier = vec![0usize];
    while frontier.len() < FRONTIER_TARGET_SUBTREES {
        let Some(expand_at) = frontier
            .iter()
            .position(|&root| child_roots(nodes, root).is_some())
        else {
            break;
        };
        let root = frontier.remove(expand_at);
        let Some((left, right)) = child_roots(nodes, root) else {
            frontier.insert(expand_at, root);
            break;
        };
        frontier.insert(expand_at, right);
        frontier.insert(expand_at, left);
    }

    frontier
}

fn estimate_fixed_frontier(
    nodes: &[BvhNode],
    leaves: &[BvhLeaf],
    planes: &[[f32; 4]; 6],
    visible_bitmask: &[u32; VISIBLE_CELLS_WORDS],
) -> BvhFrontierDiagnostics {
    let roots = fixed_frontier_roots(nodes);
    if roots.is_empty() {
        return BvhFrontierDiagnostics::default();
    }

    let mut total_work = 0u32;
    let mut max_work = 0u32;
    for &root in &roots {
        let end = nodes
            .get(root)
            .map(|node| node.skip_index as usize)
            .unwrap_or(nodes.len());
        let subtree = estimate_subtree_work(nodes, leaves, planes, visible_bitmask, root, end);
        let work = subtree
            .estimated_node_visits
            .saturating_add(subtree.leaf_tests);
        total_work = total_work.saturating_add(work);
        max_work = max_work.max(work);
    }

    let avg_work = total_work as f32 / roots.len() as f32;
    BvhFrontierDiagnostics {
        frontier_subtrees: roots.len() as u32,
        total_estimated_work: total_work,
        max_subtree_work: max_work,
        imbalance_ratio: if avg_work > 0.0 {
            max_work as f32 / avg_work
        } else {
            0.0
        },
    }
}

pub(crate) fn estimate_bvh_cull_with_planes(
    nodes: &[BvhNode],
    leaves: &[BvhLeaf],
    bucket_ranges: &[BucketRange],
    visible: &postretro_visibility::VisibleCells,
    planes: &[[f32; 4]; 6],
    submitted_bucket_scratch: &mut Vec<bool>,
) -> BvhCullDiagnostics {
    let visible_bitmask = build_visible_cell_bitmask(visible);
    let mut diagnostics =
        estimate_subtree_work(nodes, leaves, planes, &visible_bitmask, 0, nodes.len());

    if submitted_bucket_scratch.len() != bucket_ranges.len() {
        submitted_bucket_scratch.resize(bucket_ranges.len(), false);
    }
    submitted_bucket_scratch.fill(false);

    if diagnostics.submitted_leaves > 0 {
        let mut i = 0usize;
        while i < nodes.len() {
            let node = nodes[i];
            if is_aabb_outside_gpu_planes(node.aabb_min, node.aabb_max, planes) {
                i = next_skip_index(i, node.skip_index, nodes.len());
                continue;
            }
            if (node.flags & BVH_NODE_FLAG_LEAF) != 0 {
                let leaf_idx = node.left_child_or_leaf_index as usize;
                if let Some(leaf) = leaves.get(leaf_idx) {
                    let submitted =
                        !is_aabb_outside_gpu_planes(leaf.aabb_min, leaf.aabb_max, planes)
                            && bitmask_cell_is_visible(&visible_bitmask, leaf.cell_id);
                    if submitted
                        && let Some(bucket_index) = bucket_ranges.iter().position(|range| {
                            let start = range.first_leaf as usize;
                            let end = start + range.leaf_count as usize;
                            (start..end).contains(&leaf_idx)
                        })
                    {
                        submitted_bucket_scratch[bucket_index] = true;
                    }
                }
                i = next_skip_index(i, node.skip_index, nodes.len());
            } else {
                i += 1;
            }
        }
    }

    diagnostics.submitted_bucket_spans = submitted_bucket_scratch
        .iter()
        .filter(|&&submitted| submitted)
        .count() as u32;
    diagnostics.frontier = estimate_fixed_frontier(nodes, leaves, planes, &visible_bitmask);
    diagnostics
}
