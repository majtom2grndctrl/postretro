// Mirror world draws: visible static-world leaves, CPU-culled into direct indexed runs.
// See: context/lib/rendering_pipeline.md §7.3a

use super::*;
use glam::Vec4;
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
}

impl MirrorDraws {
    pub fn clear(&mut self) {
        self.draws.clear();
    }

    pub fn draws(&self) -> &[MirrorDraw] {
        &self.draws
    }

    /// The leaves of `visible`'s spans (every bucket under `DrawAll` or
    /// without a cell draw index) whose bounds reach inside `planes`, merged
    /// into runs where consecutive leaves of one bucket abut in the index
    /// buffer.
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
                    || !aabb_in_planes(
                        glam::Vec3::from(leaf.aabb_min),
                        glam::Vec3::from(leaf.aabb_max),
                        planes,
                    )
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
    }
}
