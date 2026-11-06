//! What a level's blocks ask of the lightmap and mask pools, counted with the
//! runtime's own placement.
//! See: context/lib/rendering_pipeline.md §4 (Lightmap cell-block residency)

use postretro_level_format::lightmap::{LIGHTMAP_POOL_LAYER_EDGE, LightmapSection};
use postretro_level_format::lightmap_pool::{
    MASK_POOL_LAYER_BYTES, all_resident_pool_bytes, lightmap_pool_layer_bytes, place_all_resident,
    streamed_pool_bytes,
};
use postretro_level_format::shadowmask_atlas::ShadowmaskAtlasSection;

/// Layers of the lightmap pool and of the mask pool.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct PoolLayers {
    pub(crate) lightmap: u32,
    pub(crate) mask: u32,
}

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

/// Each block's lightmap extent and how many mask planes it stores. An extent
/// is one lightmap placement, and one mask placement per present plane: an
/// absent plane demands nothing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct PoolDemand {
    alignment: u32,
    extents: Vec<(u32, u32)>,
    plane_counts: Vec<u32>,
}

/// Reused across cells, so packing one set allocates only inside the placement.
#[derive(Debug, Default)]
pub(super) struct PackScratch {
    lightmap: Vec<(u32, u32)>,
    mask: Vec<(u32, u32)>,
}

impl PoolDemand {
    pub(crate) fn new(
        lightmap: &LightmapSection,
        shadowmask: Option<&ShadowmaskAtlasSection>,
    ) -> anyhow::Result<Self> {
        let block_count = lightmap.blocks.len();
        let plane_counts = match shadowmask {
            Some(section) => {
                anyhow::ensure!(
                    section.blocks.len() == block_count,
                    "Lightmap pool bound: id 42 has {} blocks, id 22 has {block_count}",
                    section.blocks.len(),
                );
                section
                    .blocks
                    .iter()
                    .map(|planes| planes.iter().filter(|plane| !plane.is_empty()).count() as u32)
                    .collect()
            }
            None => vec![0; block_count],
        };
        Ok(Self {
            alignment: lightmap.header().block_alignment(),
            extents: lightmap
                .blocks
                .iter()
                .map(|block| (u32::from(block.width), u32::from(block.height)))
                .collect(),
            plane_counts,
        })
    }

    pub(super) fn block_count(&self) -> usize {
        self.extents.len()
    }

    /// Every mask plane the level stores.
    pub(crate) fn plane_count(&self) -> u32 {
        self.plane_counts.iter().sum()
    }

    /// The layers holding every block at once.
    pub(crate) fn all_resident(&self) -> anyhow::Result<PoolLayers> {
        let blocks: Vec<u32> = (0..self.extents.len() as u32).collect();
        self.layers_of(&blocks, &mut PackScratch::default())
    }

    /// The layers holding `blocks`, each once.
    pub(super) fn layers_of(
        &self,
        blocks: &[u32],
        scratch: &mut PackScratch,
    ) -> anyhow::Result<PoolLayers> {
        scratch.lightmap.clear();
        scratch.mask.clear();
        for &block in blocks {
            let extent = self.extents[block as usize];
            scratch.lightmap.push(extent);
            for _ in 0..self.plane_counts[block as usize] {
                scratch.mask.push(extent);
            }
        }
        Ok(PoolLayers {
            lightmap: self.pool_layers(&scratch.lightmap)?,
            mask: self.pool_layers(&scratch.mask)?,
        })
    }

    /// The mask pool places by the block's alignment too: a plane's extent is
    /// its block's, so the two pools round alike.
    fn pool_layers(&self, extents: &[(u32, u32)]) -> anyhow::Result<u32> {
        place_all_resident(extents, self.alignment, LIGHTMAP_POOL_LAYER_EDGE)
            .map(|pool| pool.layer_count)
            .ok_or_else(|| {
                anyhow::anyhow!(
                    "Lightmap pool bound: a block does not fit a \
                     {LIGHTMAP_POOL_LAYER_EDGE}² pool layer"
                )
            })
    }
}
