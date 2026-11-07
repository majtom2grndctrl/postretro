//! What a level's blocks ask of the lightmap and mask pools, through the demand
//! the runtime counts with, and what those pools cost.
//! See: context/lib/rendering_pipeline.md §4 (Lightmap cell-block residency)

use postretro_level_format::lightmap::{LIGHTMAP_POOL_LAYER_EDGE, LightmapSection};
use postretro_level_format::lightmap_pool::{
    MASK_POOL_LAYER_BYTES, PackScratch, PerPool, PoolDemand, all_resident_pool_bytes,
    lightmap_pool_layer_bytes, streamed_pool_bytes,
};
use postretro_level_format::shadowmask_atlas::ShadowmaskAtlasSection;

/// Layers of the lightmap pool and of the mask pool.
pub(crate) type PoolLayers = PerPool<u32>;

/// What a layer of each pool costs in this level's formats.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct LayerPrices {
    lightmap: u64,
}

impl LayerPrices {
    pub(super) fn of(lightmap: &LightmapSection) -> anyhow::Result<Self> {
        let header = lightmap.header();
        let lightmap = lightmap_pool_layer_bytes(&header).ok_or_else(|| {
            anyhow::anyhow!(
                "Lightmap pool bound: no pool can allocate irradiance format {}",
                header.irradiance_format
            )
        })?;
        Ok(Self { lightmap })
    }

    /// `layers` exactly, with no spare.
    pub(super) fn all_resident_bytes(self, layers: PoolLayers) -> u64 {
        all_resident_pool_bytes(layers.lightmap, self.lightmap)
            + all_resident_pool_bytes(layers.mask, MASK_POOL_LAYER_BYTES)
    }

    /// `layers` as streamed pools, each with its spare.
    pub(super) fn streamed_bytes(self, layers: PoolLayers) -> u64 {
        streamed_pool_bytes(layers.lightmap, self.lightmap)
            + streamed_pool_bytes(layers.mask, MASK_POOL_LAYER_BYTES)
    }
}

/// The shared demand of a level's sections. A level with no id 42 has no mask
/// pool, so every block demands its lightmap placement alone.
pub(crate) fn pool_demand(
    lightmap: &LightmapSection,
    shadowmask: Option<&ShadowmaskAtlasSection>,
) -> anyhow::Result<PoolDemand> {
    let block_count = lightmap.blocks.len();
    if let Some(section) = shadowmask {
        anyhow::ensure!(
            section.blocks.len() == block_count,
            "Lightmap pool bound: id 42 has {} blocks, id 22 has {block_count}",
            section.blocks.len(),
        );
    }
    Ok(PoolDemand::new(
        lightmap.header().block_alignment(),
        lightmap
            .blocks
            .iter()
            .map(|block| (u32::from(block.width), u32::from(block.height)))
            .collect(),
        shadowmask.map(|section| {
            section
                .blocks
                .iter()
                .map(|planes| planes.each_ref().map(|plane| !plane.is_empty()))
                .collect()
        }),
    ))
}

/// The layers holding every block at once.
pub(crate) fn all_resident_layers(demand: &PoolDemand) -> anyhow::Result<PoolLayers> {
    demand
        .all_resident(LIGHTMAP_POOL_LAYER_EDGE)
        .ok_or_else(block_past_a_layer)
}

/// The layers holding `blocks`, each once.
pub(super) fn layers_of(
    demand: &PoolDemand,
    blocks: &[u32],
    scratch: &mut PackScratch,
) -> anyhow::Result<PoolLayers> {
    demand
        .layers_of(blocks, LIGHTMAP_POOL_LAYER_EDGE, scratch)
        .ok_or_else(block_past_a_layer)
}

fn block_past_a_layer() -> anyhow::Error {
    anyhow::anyhow!(
        "Lightmap pool bound: a block does not fit a {LIGHTMAP_POOL_LAYER_EDGE}² pool layer"
    )
}
