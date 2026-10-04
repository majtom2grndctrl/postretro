// Shadow world reach: the cells a shadow region's frustum reaches through the
// baked BVH, and the merged index ranges that draw them.
// See: context/lib/rendering_pipeline.md §7.1 step 6

use std::ops::Range;

use glam::{Vec3, Vec4};
use postretro_render_data::cone_frustum::{Aabb, aabb_intersects_frustum};
use postretro_render_data::geometry::{BVH_NODE_FLAG_LEAF, BvhLeaf, BvhNode};

#[derive(Debug, Clone, Copy)]
struct ReachNode {
    aabb: Aabb,
    skip_index: u32,
    is_leaf: bool,
    /// Leaf slot, when the node is a leaf whose slot is in range. A leaf node
    /// pointing past the leaf array reaches nothing.
    leaf: Option<u32>,
}

#[derive(Debug, Clone, Copy)]
struct ReachLeaf {
    aabb: Aabb,
    cell_id: u32,
}

/// Install-time reach data for one level: the BVH nodes, each leaf's box and
/// cell, and each cell's leaf index ranges merged where they abut.
///
/// Built from the loaded BVH leaves alone, so a level without a per-cell draw
/// index still has reach. Ranges come from leaves, never from baked cell
/// bounds, which a leaf can overhang.
#[derive(Debug, Clone)]
pub struct ShadowReachIndex {
    nodes: Vec<ReachNode>,
    leaves: Vec<ReachLeaf>,
    /// CSR row offsets into `cell_ranges`; `cell_count + 1` entries.
    cell_range_offsets: Vec<u32>,
    cell_ranges: Vec<Range<u32>>,
}

/// Per-walk counters, read by bound and ordering proofs.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ReachStats {
    /// BVH nodes whose box the walk tested.
    pub visited_nodes: u32,
    /// Distinct cells the walk collected.
    pub collected_cells: u32,
    /// Dedupe bits cleared after the walk.
    pub reset_cells: u32,
}

/// Reusable walk state. One scratch serves every region in a frame: each walk
/// clears only the dedupe bits it set, and the caller issues a region's ranges
/// before the next walk overwrites them. Capacity is reserved for a walk that
/// reaches every cell, so no later walk grows it.
#[derive(Debug, Clone, Default)]
pub struct ShadowReachScratch {
    seen: Vec<u64>,
    cells: Vec<u32>,
    ranges: Vec<Range<u32>>,
    stats: ReachStats,
}

impl ShadowReachScratch {
    /// Cells the last walk collected, in walk order.
    pub fn cells(&self) -> &[u32] {
        &self.cells
    }

    /// Merged index ranges from the last walk, ascending.
    pub fn ranges(&self) -> &[Range<u32>] {
        &self.ranges
    }

    pub fn stats(&self) -> ReachStats {
        self.stats
    }
}

impl ShadowReachIndex {
    pub fn new(nodes: &[BvhNode], leaves: &[BvhLeaf]) -> Self {
        let reach_leaves: Vec<ReachLeaf> = leaves
            .iter()
            .map(|leaf| ReachLeaf {
                aabb: aabb(leaf.aabb_min, leaf.aabb_max),
                cell_id: leaf.cell_id,
            })
            .collect();
        let reach_nodes = nodes
            .iter()
            .map(|node| ReachNode {
                aabb: aabb(node.aabb_min, node.aabb_max),
                skip_index: node.skip_index,
                is_leaf: node.flags & BVH_NODE_FLAG_LEAF != 0,
                leaf: (node.flags & BVH_NODE_FLAG_LEAF != 0
                    && (node.left_child_or_leaf_index as usize) < leaves.len())
                .then_some(node.left_child_or_leaf_index),
            })
            .collect();

        let cell_count = leaves
            .iter()
            .map(|leaf| leaf.cell_id as usize + 1)
            .max()
            .unwrap_or(0);
        // Group each cell's leaf ranges, then sort and merge abutting ones.
        // Cell-major geometry collapses a cell to one range; a cell that is not
        // contiguous keeps one range per run and never borrows another cell's
        // indices.
        let mut by_cell: Vec<(u32, Range<u32>)> = leaves
            .iter()
            .filter(|leaf| leaf.index_count > 0)
            .map(|leaf| {
                (
                    leaf.cell_id,
                    leaf.index_offset..leaf.index_offset + leaf.index_count,
                )
            })
            .collect();
        by_cell.sort_unstable_by_key(|(cell, range)| (*cell, range.start));
        let mut cell_range_offsets = Vec::with_capacity(cell_count + 1);
        let mut cell_ranges: Vec<Range<u32>> = Vec::with_capacity(by_cell.len());
        let mut next = by_cell.iter().peekable();
        for cell in 0..cell_count as u32 {
            cell_range_offsets.push(cell_ranges.len() as u32);
            let row_start = cell_ranges.len();
            while let Some((_, range)) = next.next_if(|(owner, _)| *owner == cell) {
                match cell_ranges[row_start..].last_mut() {
                    Some(last) if last.end == range.start => last.end = range.end,
                    _ => cell_ranges.push(range.clone()),
                }
            }
        }
        cell_range_offsets.push(cell_ranges.len() as u32);

        Self {
            nodes: reach_nodes,
            leaves: reach_leaves,
            cell_range_offsets,
            cell_ranges,
        }
    }

    pub fn cell_count(&self) -> usize {
        self.cell_range_offsets.len().saturating_sub(1)
    }

    /// One cell's merged leaf index ranges; empty for an unknown cell.
    pub fn cell_ranges(&self, cell: u32) -> &[Range<u32>] {
        let cell = cell as usize;
        if cell >= self.cell_count() {
            return &[];
        }
        let start = self.cell_range_offsets[cell] as usize;
        let end = self.cell_range_offsets[cell + 1] as usize;
        &self.cell_ranges[start..end]
    }

    /// Scratch sized for this index: a walk that reaches every cell needs no
    /// further capacity.
    pub fn scratch(&self) -> ShadowReachScratch {
        let mut scratch = ShadowReachScratch::default();
        self.fit_scratch(&mut scratch);
        scratch
    }

    fn fit_scratch(&self, scratch: &mut ShadowReachScratch) {
        let words = self.cell_count().div_ceil(64);
        if scratch.seen.len() < words {
            scratch.seen.resize(words, 0);
        }
        scratch
            .cells
            .reserve(self.cell_count().saturating_sub(scratch.cells.len()));
        scratch
            .ranges
            .reserve(self.cell_ranges.len().saturating_sub(scratch.ranges.len()));
    }

    /// Walk the BVH against `planes` and return the merged index ranges of
    /// every reached cell. A cell is reached when any of its leaves' boxes
    /// passes [`aabb_intersects_frustum`]; a node whose box misses is skipped
    /// with its whole subtree, mirroring `bvh_cull.wgsl` `cull_main`.
    pub fn reach<'s>(
        &self,
        planes: &[Vec4; 6],
        scratch: &'s mut ShadowReachScratch,
    ) -> &'s [Range<u32>] {
        // A scratch from another level is refitted once; this level's own
        // scratch already holds full capacity and allocates nothing here.
        self.fit_scratch(scratch);
        scratch.cells.clear();
        scratch.ranges.clear();
        scratch.stats = ReachStats::default();

        let limit = self.nodes.len();
        let mut i = 0;
        while i < limit {
            let node = &self.nodes[i];
            scratch.stats.visited_nodes += 1;
            if !aabb_intersects_frustum(&node.aabb, planes) {
                i = next_skip_index(i, node.skip_index, limit);
                continue;
            }
            if node.is_leaf {
                if let Some(leaf) = node.leaf.map(|slot| self.leaves[slot as usize])
                    && aabb_intersects_frustum(&leaf.aabb, planes)
                {
                    collect_cell(scratch, leaf.cell_id);
                }
                i = next_skip_index(i, node.skip_index, limit);
                continue;
            }
            i += 1;
        }

        for &cell in &scratch.cells {
            let start = self.cell_range_offsets[cell as usize] as usize;
            let end = self.cell_range_offsets[cell as usize + 1] as usize;
            scratch
                .ranges
                .extend(self.cell_ranges[start..end].iter().cloned());
            let word = &mut scratch.seen[cell as usize / 64];
            *word &= !(1u64 << (cell % 64));
            scratch.stats.reset_cells += 1;
        }
        scratch.stats.collected_cells = scratch.cells.len() as u32;

        // Cells are cell-major, so sorted ranges from consecutive reached
        // cells abut and merge into one draw.
        scratch.ranges.sort_unstable_by_key(|range| range.start);
        merge_abutting(&mut scratch.ranges);
        &scratch.ranges
    }

    /// Oracle: every cell owning a leaf whose box passes the frustum, by testing
    /// every leaf. Ascending, deduplicated. Allocates; proofs only.
    pub fn brute_force_reached_cells(&self, planes: &[Vec4; 6]) -> Vec<u32> {
        let mut cells: Vec<u32> = self
            .leaves
            .iter()
            .filter(|leaf| aabb_intersects_frustum(&leaf.aabb, planes))
            .map(|leaf| leaf.cell_id)
            .collect();
        cells.sort_unstable();
        cells.dedup();
        cells
    }
}

fn aabb(min: [f32; 3], max: [f32; 3]) -> Aabb {
    Aabb {
        min: Vec3::from(min),
        max: Vec3::from(max),
    }
}

fn collect_cell(scratch: &mut ShadowReachScratch, cell: u32) {
    let word = &mut scratch.seen[cell as usize / 64];
    let bit = 1u64 << (cell % 64);
    if *word & bit == 0 {
        *word |= bit;
        scratch.cells.push(cell);
    }
}

/// Merge sorted ranges in place where one ends exactly where the next starts.
fn merge_abutting(ranges: &mut Vec<Range<u32>>) {
    let mut write = 0;
    for read in 0..ranges.len() {
        if write > 0 && ranges[write - 1].end == ranges[read].start {
            ranges[write - 1].end = ranges[read].end;
        } else {
            ranges[write] = ranges[read].clone();
            write += 1;
        }
    }
    ranges.truncate(write);
}

/// A skip index must move the walk forward; a malformed one ends it rather
/// than looping. Mirrors the camera cull's CPU estimate.
fn next_skip_index(current: usize, skip_index: u32, limit: usize) -> usize {
    let next = skip_index as usize;
    if next > current {
        next.min(limit)
    } else {
        limit
    }
}

#[cfg(test)]
#[path = "shadow_reach_tests.rs"]
mod tests;
