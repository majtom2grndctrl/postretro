// Static lightmap pool plan: whether a level's id-22/42 cell blocks can bind
// as an all-resident or streamed pool on this device, and where each
// all-resident block goes. Pure data logic; the renderer's `pool.rs` turns a
// plan into textures.
// See: context/lib/rendering_pipeline.md §4 (Lightmap cell-block residency)

use super::payload::{payload_mismatch, planes_mismatch};
use super::{
    AllResidentMasks, AllResidentPool, BlockAddresses, MaskAddresses, PerPool, PoolDemand,
    PoolKind, block_table_bytes,
};
use postretro_level_format::lightmap::{
    LIGHTMAP_POOL_LAYER_EDGE, LightmapBlockIndex, LightmapBlockPayload, LightmapHeader,
};
use postretro_level_format::shadowmask_atlas::ShadowmaskBlockIndex;

/// Every block of a level placed in the lightmap pool, and every plane placed
/// in the mask pool when the level has one.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StaticPoolPlan {
    pub header: LightmapHeader,
    /// `(width, height)` per block id.
    pub extents: Vec<(u32, u32)>,
    pub pool: AllResidentPool,
    /// `Some` when id 42 is present, every block carries well-formed planes,
    /// and the device holds the mask pool.
    pub masks: Option<AllResidentMasks>,
}

impl StaticPoolPlan {
    pub fn with_shadowmask(&self) -> bool {
        self.masks.is_some()
    }

    /// The block table of the all-resident pools.
    pub fn table_bytes(&self) -> Vec<u8> {
        let addresses: Vec<Option<BlockAddresses>> = self
            .pool
            .placements
            .iter()
            .enumerate()
            .map(|(block, &lightmap)| {
                Some(BlockAddresses {
                    lightmap,
                    masks: match &self.masks {
                        Some(masks) => MaskAddresses::Planes(masks.planes[block]),
                        None => MaskAddresses::NoPool,
                    },
                })
            })
            .collect();
        block_table_bytes(&self.extents, &addresses)
    }
}

/// What the static lightmap binds for a level.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StaticPool {
    /// No id 22: placeholder mode.
    Absent,
    /// Id 22 with zero blocks: placeholder mode.
    Empty,
    /// Id 22 present but unusable on this device or with these payloads:
    /// placeholder mode, with a `[Renderer]` error logged.
    Rejected,
    /// The all-resident block pool.
    Blocks(StaticPoolPlan),
    /// A streamed pool: blocks install through drains.
    Streaming(StreamingPoolPlan),
}

/// A level whose blocks stream: what install needs to build the pool model
/// and its first texture set. Payloads arrive later, through drains.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StreamingPoolPlan {
    pub header: LightmapHeader,
    /// Each block's extent and stored planes. A level with no id 42 has no
    /// mask pool here.
    pub demand: PoolDemand,
    /// The level's content identity, which every drain batch must carry.
    pub content_tag: [u8; 32],
    /// Each pool's first generation in layers, already bounded so a pool plus
    /// its spare layer fit the device's array-layer limit.
    pub first_generation_layers: PerPool<u32>,
    /// Device `maxTextureArrayLayers`, the spare layer included: the pool
    /// model defers a pair rather than grow past it.
    pub max_array_layers: u32,
}

/// How a streamed level's pools take their first generation.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct StreamedPoolSizing {
    /// Id 58: the worst mandatory layers of any camera cell, per pool.
    pub bound: PerPool<u32>,
    /// A harness's first generation for a pool, in place of the bound's.
    pub first_generation_override: PerPool<Option<u32>>,
}

impl StreamedPoolSizing {
    /// A level's own sizing, from its stored bound.
    pub fn from_bound(bound: PerPool<u32>) -> Self {
        Self {
            bound,
            first_generation_override: PerPool::default(),
        }
    }

    /// Each pool's first generation for `demand`, in `edge`² layers. `None`
    /// when a block does not fit a layer.
    fn first_generation(&self, demand: &PoolDemand, edge: u32) -> Option<PerPool<u32>> {
        let sized = demand.first_generation(self.bound, edge)?;
        Some(PerPool::from_fn(|kind| {
            self.first_generation_override[kind].unwrap_or(sized[kind])
        }))
    }
}

impl StreamingPoolPlan {
    pub fn with_shadowmask(&self) -> bool {
        self.demand.has_mask_pool()
    }
}

impl StaticPool {
    /// Whether static world specular has a mask to read: id 42's pool is
    /// planned for real. False without id 42, for a refused mask pool, and in
    /// placeholder mode, whose all-visible placeholder must never read as an
    /// owner's mask. Loaded into the frame uniforms' mask-present bit.
    pub fn static_specular_mask_present(&self) -> bool {
        match self {
            Self::Blocks(plan) => plan.with_shadowmask(),
            Self::Streaming(plan) => plan.with_shadowmask(),
            Self::Absent | Self::Empty | Self::Rejected => false,
        }
    }

    /// The installed cell blocks' extents, which the animated atlas keys on.
    /// `None` in placeholder mode: the animated atlas has no block frame.
    pub fn static_block_extents(&self) -> Option<&[(u32, u32)]> {
        match self {
            Self::Blocks(plan) => Some(&plan.extents),
            Self::Streaming(plan) => Some(plan.demand.extents()),
            _ => None,
        }
    }
}

/// Plan the all-resident pool for `index` and the payloads install moved in.
/// Degrades to placeholder mode, logging once, when the payloads disagree with
/// the index (hand-built geometry), or when the device cannot hold a pool
/// layer or the layer count the shelf allocator needs. The mask pool drops to
/// its placeholder alone when only its planes are unusable, or the device
/// cannot hold its layers; the level then draws no static non-SDF world
/// specular.
pub fn plan_static_pool(
    index: Option<&LightmapBlockIndex>,
    shadowmask: Option<&ShadowmaskBlockIndex>,
    payloads: &[LightmapBlockPayload],
    max_texture_dimension_2d: u32,
    max_texture_array_layers: u32,
) -> StaticPool {
    let Some(index) = index else {
        return StaticPool::Absent;
    };
    if index.records.is_empty() {
        return StaticPool::Empty;
    }
    let header = index.header;
    let extents = block_extents(index);
    let pool = payloads_match_index(&header, &extents, payloads)
        .and_then(|()| place_pool(&header, &extents, max_texture_dimension_2d))
        .and_then(|pool| {
            array_layers_fit(pool.layer_count, extents.len(), max_texture_array_layers)
                .map(|()| pool)
        });
    let pool = match pool {
        Ok(pool) => pool,
        Err(reason) => {
            log::error!(
                "[Renderer] Lightmap cell blocks rejected: {reason}; degrading to the neutral \
                 placeholder for this level"
            );
            return StaticPool::Rejected;
        }
    };
    let masks = shadowmask.and_then(|shadowmask| {
        place_masks(index, shadowmask, payloads, max_texture_array_layers)
            .inspect_err(|reason| {
                log::error!(
                    "[Renderer] ShadowmaskAtlas rejected: {reason}; disabling entity-to-world \
                 static-light shadowmask; this level gets no static non-SDF world specular"
                )
            })
            .ok()
    });
    StaticPool::Blocks(StaticPoolPlan {
        header,
        extents,
        pool,
        masks,
    })
}

/// Plan a streamed pool for `index`: the level's blocks install later, per
/// drain, so only the pool shape and each pool's first generation are settled
/// here. Degrades to placeholder mode, logging once, when the device cannot
/// hold a pool layer or one layer plus the spare, or when the mask planes do
/// not fit the pool layer edge. A level whose every block at
/// once would not fit the device still streams, since streaming holds only a
/// subset: the pool model defers a pair rather than grow past the device's
/// layers, in either pool.
pub fn plan_streaming_pool(
    index: Option<&LightmapBlockIndex>,
    shadowmask: Option<&ShadowmaskBlockIndex>,
    content_tag: [u8; 32],
    sizing: StreamedPoolSizing,
    max_texture_dimension_2d: u32,
    max_texture_array_layers: u32,
) -> StaticPool {
    let Some(index) = index else {
        return StaticPool::Absent;
    };
    if index.records.is_empty() {
        return StaticPool::Empty;
    }
    let header = index.header;
    let extents = block_extents(index);
    let shape = place_pool(&header, &extents, max_texture_dimension_2d).and_then(|pool| {
        array_layers_fit(
            MIN_STREAMED_ARRAY_LAYERS,
            extents.len(),
            max_texture_array_layers,
        )
        .map(|()| pool)
    });
    let demand = PoolDemand::of_indices(index, shadowmask);
    if shape.is_ok()
        && let Some(all_resident) = demand.all_resident(LIGHTMAP_POOL_LAYER_EDGE)
    {
        for kind in PoolKind::ALL {
            let layers = all_resident[kind];
            if layers > 0 && layers + 1 > max_texture_array_layers {
                log::warn!(
                    "[Renderer] Streamed lightmap: the {} pool needs {} array layer(s) with the \
                     spare for all {} block(s), past device maxTextureArrayLayers \
                     {max_texture_array_layers}; mandatory growth past the device limit defers \
                     instead",
                    kind.name(),
                    layers + 1,
                    extents.len(),
                );
            }
        }
    }
    let first_generation = shape.and_then(|_| {
        sizing
            .first_generation(&demand, LIGHTMAP_POOL_LAYER_EDGE)
            .ok_or_else(|| {
                format!("mask planes do not fit {LIGHTMAP_POOL_LAYER_EDGE}² pool layers")
            })
    });
    let first_generation = match first_generation {
        Ok(layers) => layers,
        Err(reason) => {
            log::error!(
                "[Renderer] Streamed lightmap pool rejected: {reason}; degrading to the neutral \
                 placeholder for this level"
            );
            return StaticPool::Rejected;
        }
    };
    StaticPool::Streaming(StreamingPoolPlan {
        header,
        demand,
        content_tag,
        first_generation_layers: first_generation
            .map(|layers| layers.min(max_texture_array_layers - 1)),
        max_array_layers: max_texture_array_layers,
    })
}

/// The smallest streamed pool texture: one pool layer plus the repack spare.
const MIN_STREAMED_ARRAY_LAYERS: u32 = 2;

fn block_extents(index: &LightmapBlockIndex) -> Vec<(u32, u32)> {
    index
        .records
        .iter()
        .map(|r| (u32::from(r.width), u32::from(r.height)))
        .collect()
}

/// Places every block all-resident, after the checks every pool shares
/// whatever its layer count: the direction scale divides a layer, a layer
/// fits the device, and every block fits a layer.
fn place_pool(
    header: &LightmapHeader,
    extents: &[(u32, u32)],
    max_texture_dimension_2d: u32,
) -> Result<AllResidentPool, String> {
    let scale = header.direction_texel_scale.max(1);
    if !LIGHTMAP_POOL_LAYER_EDGE.is_multiple_of(scale) {
        return Err(format!(
            "direction texel scale {scale} does not divide the {LIGHTMAP_POOL_LAYER_EDGE}-texel \
             pool layer"
        ));
    }
    if LIGHTMAP_POOL_LAYER_EDGE > max_texture_dimension_2d {
        return Err(format!(
            "pool layer {LIGHTMAP_POOL_LAYER_EDGE}² exceeds device maxTextureDimension2D \
             {max_texture_dimension_2d}"
        ));
    }
    PoolDemand::lightmap_only(header.block_alignment(), extents.to_vec())
        .place_blocks(LIGHTMAP_POOL_LAYER_EDGE)
        .ok_or_else(|| format!("cell blocks do not fit {LIGHTMAP_POOL_LAYER_EDGE}² pool layers"))
}

fn array_layers_fit(
    required: u32,
    blocks: usize,
    max_texture_array_layers: u32,
) -> Result<(), String> {
    if required > max_texture_array_layers {
        return Err(format!(
            "the pool needs {required} array layer(s) for {blocks} cell block(s), exceeding \
             device maxTextureArrayLayers {max_texture_array_layers}"
        ));
    }
    Ok(())
}

/// The loader validated every blob against the index; hand-built geometry can
/// still pair an index with payloads of another shape, which would fail the
/// region uploads.
fn payloads_match_index(
    header: &LightmapHeader,
    extents: &[(u32, u32)],
    payloads: &[LightmapBlockPayload],
) -> Result<(), String> {
    if payloads.len() != extents.len() {
        return Err(format!(
            "{} payload(s) arrived for {} block(s)",
            payloads.len(),
            extents.len()
        ));
    }
    for (block, (&extent, payload)) in extents.iter().zip(payloads).enumerate() {
        if let Some(reason) = payload_mismatch(header, extent, None, payload) {
            return Err(format!("block {block}: {reason}"));
        }
    }
    Ok(())
}

/// The all-resident mask pool: every present plane placed, in the same pool
/// shape a streamed level takes. Its layers are the lightmap pool's edge, which
/// `place_pool` already held to the device. A level with id 42 and no present
/// plane places nothing and still has masks, which read zero.
fn place_masks(
    index: &LightmapBlockIndex,
    shadowmask: &ShadowmaskBlockIndex,
    payloads: &[LightmapBlockPayload],
    max_texture_array_layers: u32,
) -> Result<AllResidentMasks, String> {
    let demand = PoolDemand::of_indices(index, Some(shadowmask));
    for (block, (&(w, h), payload)) in demand.extents().iter().zip(payloads).enumerate() {
        let planes = demand.planes(block as u32);
        if let Some(reason) = planes_mismatch((w, h), planes, payload) {
            return Err(format!("block {block} {reason}"));
        }
    }
    let masks = demand
        .place_planes(LIGHTMAP_POOL_LAYER_EDGE)
        .ok_or_else(|| format!("mask planes do not fit {LIGHTMAP_POOL_LAYER_EDGE}² pool layers"))?;
    if masks.layer_count > max_texture_array_layers {
        return Err(format!(
            "the mask pool needs {} array layer(s) for {} plane(s), exceeding device \
             maxTextureArrayLayers {max_texture_array_layers}",
            masks.layer_count,
            demand.plane_count(),
        ));
    }
    Ok(masks)
}
