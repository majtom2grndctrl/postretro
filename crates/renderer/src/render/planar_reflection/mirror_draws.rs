// Mirror world draws: visible static-world leaves, CPU-culled into direct indexed runs.
// See: context/lib/rendering_pipeline.md §7.3a

use super::*;
use glam::{Vec3, Vec4};
use postretro_render_cpu::planar_reflection::aabb_in_planes;
use postretro_render_data::geometry::{BucketRange, BvhLeaf};

/// One contiguous run of world indices in one material bucket.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct MirrorDraw {
    pub bucket: u32,
    pub first_index: u32,
    pub end_index: u32,
}

/// The mirror's world draw list, rebuilt each frame it records into reused
/// storage. Direct draws keep the release indirect-call contract to the main
/// camera cull alone (`indirect_contract_tests.rs`); the mirror walk and its
/// crop bound the leaves it visits.
#[derive(Default)]
pub(super) struct MirrorDraws {
    ranges: crate::VisibleSpanRanges,
    draws: Vec<MirrorDraw>,
    /// The same leaves as `draws`, in index order and merged wherever they
    /// abut whatever their bucket: the depth prepass binds no material, and
    /// the index buffer is cell-major, so one cell's buckets often sit end
    /// to end.
    depth_runs: Vec<std::ops::Range<u32>>,
}

impl MirrorDraws {
    pub fn clear(&mut self) {
        self.draws.clear();
        self.depth_runs.clear();
    }

    pub fn draws(&self) -> &[MirrorDraw] {
        &self.draws
    }

    pub fn depth_runs(&self) -> &[std::ops::Range<u32>] {
        &self.depth_runs
    }

    /// The leaves of `visible`'s spans (every bucket under `DrawAll` or
    /// without a cell draw index) whose bounds reach inside `planes`, merged
    /// into runs where consecutive leaves of one bucket abut in the index
    /// buffer, and the depth runs over the same leaves. Allocation-free once
    /// warm.
    pub fn rebuild(
        &mut self,
        leaves: &[BvhLeaf],
        buckets: &[BucketRange],
        index: Option<&postretro_level_loader::CellDrawIndex>,
        visible: &VisibleCells,
        planes: &[Vec4; 6],
    ) {
        self.draws.clear();
        self.ranges.rebuild(index, visible, buckets);
        for range in self.ranges.ranges() {
            let first = range.first_leaf as usize;
            let end = (first + range.leaf_count as usize).min(leaves.len());
            for leaf in &leaves[first.min(end)..end] {
                if leaf.index_count == 0
                    || !aabb_in_planes(Vec3::from(leaf.aabb_min), Vec3::from(leaf.aabb_max), planes)
                {
                    continue;
                }
                let leaf_end = leaf.index_offset + leaf.index_count;
                match self.draws.last_mut() {
                    Some(last)
                        if last.bucket == range.material_bucket_id
                            && last.end_index == leaf.index_offset =>
                    {
                        last.end_index = leaf_end;
                    }
                    _ => self.draws.push(MirrorDraw {
                        bucket: range.material_bucket_id,
                        first_index: leaf.index_offset,
                        end_index: leaf_end,
                    }),
                }
            }
        }
        self.rebuild_depth_runs();
    }

    fn rebuild_depth_runs(&mut self) {
        self.depth_runs.clear();
        self.depth_runs.extend(
            self.draws
                .iter()
                .map(|draw| draw.first_index..draw.end_index),
        );
        self.depth_runs.sort_unstable_by_key(|run| run.start);
        let mut write = 0;
        for read in 0..self.depth_runs.len() {
            if write > 0 && self.depth_runs[write - 1].end == self.depth_runs[read].start {
                self.depth_runs[write - 1].end = self.depth_runs[read].end;
            } else {
                self.depth_runs[write] = self.depth_runs[read].clone();
                write += 1;
            }
        }
        self.depth_runs.truncate(write);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// One triangle of `bucket` at `index_offset`.
    fn leaf(bucket: u32, index_offset: u32) -> BvhLeaf {
        BvhLeaf {
            aabb_min: [0.0; 3],
            material_bucket_id: bucket,
            aabb_max: [1.0; 3],
            index_offset,
            index_count: 3,
            cell_id: 0,
            chunk_range_start: 0,
            chunk_range_count: 0,
        }
    }

    // The depth prepass binds no material, so leaves of different buckets
    // that abut in the cell-major index buffer merge into one run there,
    // while the forward list keeps one run per bucket. Same indices either
    // way, so coverage is unchanged.
    #[test]
    fn mirror_depth_runs_merge_abutting_leaves_across_buckets() {
        // Leaves sorted by bucket; one cell's indices interleave the buckets.
        let leaves = [leaf(0, 0), leaf(0, 6), leaf(1, 3), leaf(1, 12)];
        let buckets = [
            BucketRange {
                material_bucket_id: 0,
                first_leaf: 0,
                leaf_count: 2,
            },
            BucketRange {
                material_bucket_id: 1,
                first_leaf: 2,
                leaf_count: 2,
            },
        ];
        // Every plane passes every point.
        let planes = [Vec4::new(0.0, 0.0, 0.0, 1.0); 6];
        let mut draws = MirrorDraws::default();
        draws.rebuild(&leaves, &buckets, None, &VisibleCells::DrawAll, &planes);
        assert_eq!(draws.draws().len(), 4, "one run per leaf");
        assert_eq!(draws.depth_runs(), &[0..9, 12..15]);

        let forward: u32 = draws
            .draws()
            .iter()
            .map(|draw| draw.end_index - draw.first_index)
            .sum();
        let depth: u32 = draws.depth_runs().iter().map(|run| run.len() as u32).sum();
        assert_eq!(forward, depth);

        draws.clear();
        assert!(draws.depth_runs().is_empty());
    }
}
