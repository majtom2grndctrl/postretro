// Shadow world draws: which shadow regions draw static world this frame, and
// the CPU reach each one draws as direct indexed draws.
// See: context/lib/rendering_pipeline.md §7.1 steps 6–8

use std::ops::Range;

use glam::Mat4;
use postretro_render_cpu::shadow_reach::{ShadowReachIndex, ShadowReachScratch};
use postretro_render_data::cone_frustum::cone_frustum_planes;
use postretro_render_data::geometry::BvhTree;

use super::cpu_stages::RenderStage;
use super::dynamic_depth_cache::{DynamicCubePlan, DynamicDepthCachePlan, DynamicSpotPlan};
use super::promoted_depth_cache::{
    PromotedCubeCachePlan, PromotedDepthCacheFramePlan, PromotedSpotCachePlan,
};

/// What one spot slot or cube slot does with static world this frame.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum RegionWorld<P, D> {
    /// A promoted light holding a promoted-cache layer: world only on a cold fill.
    Promoted(P),
    /// A dynamic light holding a dynamic-cache layer: world only on a cold fill.
    Dynamic(D),
    /// A dynamic light past cache capacity: world into the live layer every
    /// frame. A promoted light the cache drops has zero weight and holds no
    /// slot (`clear_zero_weight_promoted_assignments`), so it never lands here.
    Live,
}

pub(super) type SpotRegionWorld = RegionWorld<PromotedSpotCachePlan, DynamicSpotPlan>;
pub(super) type CubeRegionWorld = RegionWorld<PromotedCubeCachePlan, DynamicCubePlan>;

/// A cache plan that may need its layer filled with world depth.
pub(super) trait WorldFill: Copy {
    fn needs_world_render(self) -> bool;
}

impl WorldFill for PromotedSpotCachePlan {
    fn needs_world_render(self) -> bool {
        self.needs_world_render
    }
}
impl WorldFill for PromotedCubeCachePlan {
    fn needs_world_render(self) -> bool {
        self.needs_world_render
    }
}
impl WorldFill for DynamicSpotPlan {
    fn needs_world_render(self) -> bool {
        self.needs_world_render
    }
}
impl WorldFill for DynamicCubePlan {
    fn needs_world_render(self) -> bool {
        self.needs_world_render
    }
}

impl<P: WorldFill, D: WorldFill> RegionWorld<P, D> {
    /// Whether this region draws static world (and so walks its reach) this frame.
    pub fn draws_world(self) -> bool {
        match self {
            Self::Promoted(plan) => plan.needs_world_render(),
            Self::Dynamic(plan) => plan.needs_world_render(),
            Self::Live => true,
        }
    }
}

pub(super) fn classify_spot(
    slot: u32,
    promoted: &PromotedDepthCacheFramePlan,
    dynamic: &DynamicDepthCachePlan,
) -> SpotRegionWorld {
    if let Some(plan) = promoted.spot_for_slot(slot) {
        RegionWorld::Promoted(plan)
    } else if let Some(plan) = dynamic.spot_for_slot(slot) {
        RegionWorld::Dynamic(plan)
    } else {
        RegionWorld::Live
    }
}

pub(super) fn classify_cube(
    slot: u32,
    promoted: &PromotedDepthCacheFramePlan,
    dynamic: &DynamicDepthCachePlan,
) -> CubeRegionWorld {
    if let Some(plan) = promoted.cube_for_slot(slot) {
        RegionWorld::Promoted(plan)
    } else if let Some(plan) = dynamic.cube_for_slot(slot) {
        RegionWorld::Dynamic(plan)
    } else {
        RegionWorld::Live
    }
}

/// A shadow region: one spot slot or one cube face layer (`slot * 6 + face`).
/// Spot slot 3 and cube layer 3 are different regions.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum ShadowRegion {
    Spot(u32),
    CubeFace(u32),
}

/// One recorded world draw list, kept by tests.
#[cfg(test)]
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct WorldDrawTrace {
    pub region: ShadowRegion,
    pub ranges: Vec<Range<u32>>,
}

/// Bindings a world depth draw sets on its pass.
pub(super) struct WorldDepthBindings<'a> {
    pub pipeline: &'a wgpu::RenderPipeline,
    pub bind_group: &'a wgpu::BindGroup,
    pub dynamic_offset: u32,
    pub vertex_buffer: &'a wgpu::Buffer,
    pub index_buffer: &'a wgpu::Buffer,
}

/// The installed level's reach index and the one scratch every region's walk
/// reuses. A level with no BVH has no index and draws its whole index buffer.
#[derive(Default)]
pub(super) struct ShadowWorldDraws {
    reach: Option<ShadowReachIndex>,
    scratch: ShadowReachScratch,
    /// `0..index_count`, the no-BVH draw.
    all: Range<u32>,
    /// BVH walks since the frame began; a no-BVH draw is not a walk.
    pub walks: u32,
    #[cfg(test)]
    pub trace: Vec<WorldDrawTrace>,
}

impl ShadowWorldDraws {
    /// Reach data for a newly installed level. Replaces the previous level's
    /// index and scratch, so no range from it survives the install.
    pub fn install(bvh: Option<&BvhTree>, index_count: u32) -> Self {
        // Nothing draws without indices, so an empty level builds no reach.
        let bvh = bvh.filter(|bvh| !bvh.leaves.is_empty() && index_count > 0);
        // Leaf ranges were checked against the index array before install
        // (§5 Indirect-args invariant 2), so every reach range lies inside it.
        debug_assert!(bvh.is_none_or(|bvh| bvh.leaves.iter().all(|leaf| {
            u64::from(leaf.index_offset) + u64::from(leaf.index_count) <= u64::from(index_count)
        })));
        let reach = bvh.map(|bvh| ShadowReachIndex::new(&bvh.nodes, &bvh.leaves));
        let scratch = reach
            .as_ref()
            .map(ShadowReachIndex::scratch)
            .unwrap_or_default();
        Self {
            reach,
            scratch,
            all: 0..index_count,
            walks: 0,
            #[cfg(test)]
            trace: Vec::new(),
        }
    }

    /// Start a frame's shadow recording: walk counts (and test traces) cover
    /// one frame.
    pub fn begin_frame(&mut self) {
        self.walks = 0;
        #[cfg(test)]
        self.trace.clear();
    }

    /// Index ranges a region whose light-space matrix is `matrix` draws: its
    /// merged reach, or the whole index buffer without a BVH. Only valid until
    /// the next call.
    pub fn ranges(&mut self, region: ShadowRegion, matrix: &Mat4) -> &[Range<u32>] {
        #[cfg(not(test))]
        let _ = region;
        let ranges: &[Range<u32>] = match &self.reach {
            Some(reach) => {
                self.walks += 1;
                reach.reach(&cone_frustum_planes(matrix), &mut self.scratch)
            }
            None if self.all.is_empty() => &[],
            None => std::slice::from_ref(&self.all),
        };
        #[cfg(test)]
        self.trace.push(WorldDrawTrace {
            region,
            ranges: ranges.to_vec(),
        });
        ranges
    }

    /// Draw a region's reach into an open depth pass. An empty reach sets
    /// nothing and draws nothing; the pass's clear still applies.
    pub fn record(
        &mut self,
        pass: &mut wgpu::RenderPass<'_>,
        cpu: &postretro_stage_timing::StageFrame<RenderStage>,
        region: ShadowRegion,
        matrix: &Mat4,
        bindings: WorldDepthBindings<'_>,
    ) {
        let ranges = {
            // Only a BVH walk is reach work; the no-BVH whole-buffer draw is not.
            let _reach_scope = self
                .reach
                .is_some()
                .then(|| cpu.scope(RenderStage::ShadowReach));
            self.ranges(region, matrix)
        };
        if ranges.is_empty() {
            return;
        }
        pass.set_pipeline(bindings.pipeline);
        pass.set_bind_group(0, bindings.bind_group, &[bindings.dynamic_offset]);
        pass.set_vertex_buffer(0, bindings.vertex_buffer.slice(..));
        pass.set_index_buffer(bindings.index_buffer.slice(..), wgpu::IndexFormat::Uint32);
        for range in ranges {
            pass.draw_indexed(range.clone(), 0, 0..1);
        }
    }
}

#[cfg(test)]
#[path = "shadow_world_draws_tests.rs"]
mod tests;
