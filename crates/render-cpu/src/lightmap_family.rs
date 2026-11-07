// CPU plan of a level's lightmap family: the static cell-block pool and the
// animated atlas that keys its blocks on that pool, both on device limits.
// See: context/lib/rendering_pipeline.md §4 (Lightmap cell-block residency), §7.1

use postretro_level_format::animated_light_chunks::AnimatedLightChunksSection;
use postretro_level_format::animated_light_weight_maps::AnimatedLightWeightMapsSection;
use postretro_level_format::lightmap::{LightmapBlockIndex, LightmapBlockPayload};
use postretro_level_format::shadowmask_atlas::ShadowmaskBlockIndex;
use postretro_render_data::geometry::BvhLeaf;

use crate::animated_lightmap::plan::{AnimatedLightmapPlan, plan_animated_lightmap};
use crate::device_limits::DeviceLimits;
use crate::lightmap_pool::{StaticPool, StreamedPoolSizing, plan_static_pool, plan_streaming_pool};

/// A streamed lightmap pool's level identity and how its pools are sized.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LightmapStreamingInputs {
    pub content_tag: [u8; 32],
    pub pool_sizing: StreamedPoolSizing,
}

/// The level data the lightmap family plans from.
#[derive(Debug, Clone, Copy)]
pub struct LightmapFamilyInputs<'a> {
    /// Id 22's block index; `None` is placeholder mode.
    pub lightmap: Option<&'a LightmapBlockIndex>,
    /// Id 42's block index.
    pub shadowmask: Option<&'a ShadowmaskBlockIndex>,
    /// `Some` streams the pool; `None` installs every block at once.
    pub streaming: Option<LightmapStreamingInputs>,
    /// The all-resident pool's payloads (ignored when streaming).
    pub payload_blocks: &'a [LightmapBlockPayload],
    pub weight_maps: Option<&'a AnimatedLightWeightMapsSection>,
    pub animated_chunks: Option<&'a AnimatedLightChunksSection>,
    pub bvh_leaves: &'a [BvhLeaf],
    /// The SH volume's animated descriptor count
    /// (`sh_volume::animated_light_count`).
    pub animated_light_count: u32,
}

/// One plan decides the static pool for both consumers: the animated atlas
/// keys its blocks on the cell blocks the pool installs, and a placeholder or
/// rejected pool leaves it no block frame. A rejected animated plan carries
/// its reason; the renderer logs it once and binds the dummy.
#[derive(Debug, Clone, PartialEq)]
pub struct LightmapFamilyPlan {
    pub static_pool: StaticPool,
    pub animated: Result<AnimatedLightmapPlan, String>,
}

// Compile-time guard: the plan is built on a load-pool thread.
const _: fn() = || {
    fn assert_send<T: Send>() {}
    assert_send::<LightmapFamilyPlan>();
};

pub fn plan_lightmap_family(
    inputs: &LightmapFamilyInputs<'_>,
    limits: &DeviceLimits,
) -> LightmapFamilyPlan {
    let static_pool = match inputs.streaming {
        Some(streaming) => plan_streaming_pool(
            inputs.lightmap,
            inputs.shadowmask,
            streaming.content_tag,
            streaming.pool_sizing,
            limits.max_texture_dimension_2d,
            limits.max_texture_array_layers,
        ),
        None => plan_static_pool(
            inputs.lightmap,
            inputs.shadowmask,
            inputs.payload_blocks,
            limits.max_texture_dimension_2d,
            limits.max_texture_array_layers,
        ),
    };
    let animated = plan_animated_lightmap(
        inputs.weight_maps,
        inputs.animated_chunks,
        inputs.bvh_leaves,
        inputs.animated_light_count,
        static_pool.static_block_extents(),
        limits,
    );
    LightmapFamilyPlan {
        static_pool,
        animated,
    }
}
