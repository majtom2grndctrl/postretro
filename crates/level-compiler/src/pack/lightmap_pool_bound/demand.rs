//! What a level's blocks ask of the lightmap and mask pools, through the demand
//! the runtime counts with, and what those pools cost.
//! See: context/lib/rendering_pipeline.md §4 (Lightmap cell-block residency)

use postretro_level_format::lightmap::{LIGHTMAP_POOL_LAYER_EDGE, LightmapSection};
use postretro_level_format::lightmap_pool::{
    BlockMasks, PackScratch, PerPool, PoolDemand, all_resident_pool_bytes, first_generation_layers,
    pool_layer_bytes, streamed_pool_bytes,
};
use postretro_level_format::shadowmask_atlas::ShadowmaskAtlasSection;

/// Layers of the lightmap pool and of the mask pool.
pub(crate) type PoolLayers = PerPool<u32>;

/// What a layer of each pool costs in this level's formats.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct LayerPrices {
    layer_bytes: PerPool<u64>,
}

impl LayerPrices {
    pub(super) fn of(lightmap: &LightmapSection) -> anyhow::Result<Self> {
        let header = lightmap.header();
        let layer_bytes = pool_layer_bytes(&header).ok_or_else(|| {
            anyhow::anyhow!(
                "Lightmap pool bound: no pool can allocate irradiance format {}",
                header.irradiance_format
            )
        })?;
        Ok(Self { layer_bytes })
    }

    /// `layers` exactly, with no spare.
    pub(super) fn all_resident_bytes(self, layers: PoolLayers) -> u64 {
        PerPool::from_fn(|kind| all_resident_pool_bytes(layers[kind], self.layer_bytes[kind]))
            .total()
    }

    /// `layers` as streamed pools, each with its spare.
    pub(super) fn streamed_bytes(self, layers: PoolLayers) -> u64 {
        PerPool::from_fn(|kind| streamed_pool_bytes(layers[kind], self.layer_bytes[kind])).total()
    }

    /// Each pool's first generation from its `bound`, as the runtime sizes it.
    pub(super) fn first_generation(
        self,
        bound: PoolLayers,
        all_resident: PoolLayers,
    ) -> PoolLayers {
        PerPool::from_fn(|kind| {
            first_generation_layers(bound[kind], all_resident[kind], self.layer_bytes[kind])
        })
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
            section.blocks.len() == block_count && section.extents.len() == block_count,
            "Lightmap pool bound: id 42 has {} blocks and {} mask extents, id 22 has \
             {block_count} blocks",
            section.blocks.len(),
            section.extents.len(),
        );
    }
    // Each present plane at its own mask extent, as `PoolDemand::of_indices`
    // reads them back from the written section.
    Ok(PoolDemand::with_masks(
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
                .zip(&section.extents)
                .map(|(planes, extents)| BlockMasks {
                    planes: planes.each_ref().map(|plane| !plane.is_empty()),
                    extents: [extents.plane(0), extents.plane(1)],
                })
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
