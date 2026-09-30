// Static lightmap pool plan: whether a level's id-22/42 cell blocks can bind
// as an all-resident or streamed pool on this device, and where each
// all-resident block goes. Pure data logic; `pool.rs` turns a plan into
// textures.
// See: context/lib/rendering_pipeline.md §4 (Lightmap cell-block residency)

use postretro_level_format::lightmap::{
    LIGHTMAP_POOL_LAYER_EDGE, LightmapBlockIndex, LightmapBlockPayload, LightmapHeader,
};
use postretro_level_format::shadowmask_atlas::{
    SHADOWMASK_GROUP_COUNT, ShadowmaskBlockIndex, group_plane_len,
};
use postretro_render_cpu::lightmap_pool::{AllResidentPool, place_all_resident};

/// Every block of a level placed in the pool, and whether the shadowmask
/// groups install beside them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct StaticPoolPlan {
    pub(crate) header: LightmapHeader,
    /// `(width, height)` per block id.
    pub(crate) extents: Vec<(u32, u32)>,
    pub(crate) pool: AllResidentPool,
    /// Id 42 is present and every block carries well-formed groups.
    pub(crate) with_shadowmask: bool,
}

/// What the static lightmap binds for a level.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum StaticPool {
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
pub(crate) struct StreamingPoolPlan {
    pub(crate) header: LightmapHeader,
    /// `(width, height)` per block id.
    pub(crate) extents: Vec<(u32, u32)>,
    /// Id 42 is present and the device holds its two-group pool width.
    pub(crate) with_shadowmask: bool,
    /// The level's content identity, which every drain batch must carry.
    pub(crate) content_tag: [u8; 32],
    /// The first generation's layer cap, already bounded so the pool plus
    /// its spare layer fit the device's array-layer limit.
    pub(crate) pool_cap_layers: u32,
    /// Device `maxTextureArrayLayers`, the spare layer included: the pool
    /// model defers a pair rather than grow past it.
    pub(crate) max_array_layers: u32,
}

impl StaticPool {
    /// The installed cell blocks' extents, which the animated atlas keys on.
    /// `None` in placeholder mode: the animated atlas has no block frame.
    pub(crate) fn static_block_extents(&self) -> Option<&[(u32, u32)]> {
        match self {
            Self::Blocks(plan) => Some(&plan.extents),
            Self::Streaming(plan) => Some(&plan.extents),
            _ => None,
        }
    }
}

/// Plan the all-resident pool for `index` and the payloads install moved in.
/// Degrades to placeholder mode, logging once, when the payloads disagree with
/// the index (hand-built geometry), or when the device cannot hold a pool
/// layer or the layer count the shelf allocator needs. The shadowmask drops
/// to its all-visible placeholder alone when only its groups are unusable.
pub(crate) fn plan_static_pool(
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
    let with_shadowmask = shadowmask.is_some_and(|_| {
        usable_shadowmask(&extents, payloads, max_texture_dimension_2d)
            .inspect_err(|reason| {
                log::error!(
                    "[Renderer] ShadowmaskAtlas rejected: {reason}; disabling entity-to-world \
                     static-light shadowmask; static world specular falls back to fully lit for \
                     this level"
                )
            })
            .is_ok()
    });
    StaticPool::Blocks(StaticPoolPlan {
        header,
        extents,
        pool,
        with_shadowmask,
    })
}

/// Plan a streamed pool for `index`: the level's blocks install later, per
/// drain, so only the pool shape is checked here. Degrades to placeholder
/// mode, logging once, when the device cannot hold a pool layer or one layer
/// plus the spare. A level whose every block at once would not fit the device
/// still streams, since streaming holds only a subset: the pool model defers
/// a pair rather than grow past the device's layers. The shadowmask drops to
/// its all-visible placeholder alone when the device cannot hold its
/// two-group width.
pub(crate) fn plan_streaming_pool(
    index: Option<&LightmapBlockIndex>,
    shadowmask: Option<&ShadowmaskBlockIndex>,
    content_tag: [u8; 32],
    pool_cap_layers: u32,
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
    if let Ok(pool) = &shape
        && pool.layer_count + 1 > max_texture_array_layers
    {
        log::warn!(
            "[Renderer] Streamed lightmap: all {} cell block(s) need {} array layer(s) with the \
             spare, past device maxTextureArrayLayers {max_texture_array_layers}; mandatory \
             growth past the device limit defers instead",
            extents.len(),
            pool.layer_count + 1,
        );
    }
    if let Err(reason) = shape {
        log::error!(
            "[Renderer] Streamed lightmap pool rejected: {reason}; degrading to the neutral \
             placeholder for this level"
        );
        return StaticPool::Rejected;
    }
    let with_shadowmask = shadowmask.is_some_and(|_| {
        shadowmask_width_fits(max_texture_dimension_2d)
            .inspect_err(|reason| {
                log::error!(
                    "[Renderer] ShadowmaskAtlas rejected: {reason}; static world specular falls \
                     back to fully lit for this level"
                )
            })
            .is_ok()
    });
    StaticPool::Streaming(StreamingPoolPlan {
        header,
        extents,
        with_shadowmask,
        content_tag,
        pool_cap_layers: pool_cap_layers.min(max_texture_array_layers - 1),
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
    if LIGHTMAP_POOL_LAYER_EDGE % scale != 0 {
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
    place_all_resident(extents, header.block_alignment(), LIGHTMAP_POOL_LAYER_EDGE)
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

/// The shadowmask pool holds both mask groups side by side, at twice the
/// pool layer's width.
fn shadowmask_width_fits(max_texture_dimension_2d: u32) -> Result<(), String> {
    let width = u64::from(LIGHTMAP_POOL_LAYER_EDGE) * u64::from(SHADOWMASK_GROUP_COUNT);
    if width > u64::from(max_texture_dimension_2d) {
        return Err(format!(
            "pool texture {width}x{LIGHTMAP_POOL_LAYER_EDGE} (two mask groups) exceeds device \
             maxTextureDimension2D {max_texture_dimension_2d}"
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
    for (block, (&(w, h), payload)) in extents.iter().zip(payloads).enumerate() {
        let irradiance = header.irradiance_len(w, h);
        let direction = header.direction_len(w, h);
        if irradiance != Some(payload.irradiance.len() as u64)
            || direction != Some(payload.direction.len() as u64)
        {
            return Err(format!(
                "block {block} ({w}x{h}) carries {} irradiance and {} direction byte(s), \
                 expected {irradiance:?} and {direction:?}",
                payload.irradiance.len(),
                payload.direction.len(),
            ));
        }
    }
    Ok(())
}

fn usable_shadowmask(
    extents: &[(u32, u32)],
    payloads: &[LightmapBlockPayload],
    max_texture_dimension_2d: u32,
) -> Result<(), String> {
    shadowmask_width_fits(max_texture_dimension_2d)?;
    for (block, (&(w, h), payload)) in extents.iter().zip(payloads).enumerate() {
        let expected = group_plane_len(w, h);
        match &payload.shadowmask {
            Some([a, b])
                if expected == Some(a.len() as u64) && expected == Some(b.len() as u64) => {}
            Some([a, b]) => {
                return Err(format!(
                    "block {block} ({w}x{h}) groups are {} and {} byte(s), expected {expected:?}",
                    a.len(),
                    b.len(),
                ));
            }
            None => return Err(format!("block {block} arrived without its groups")),
        }
    }
    Ok(())
}
