//! Runtime all-resident pool floors: the lightmap pool's before the fused bake
//! or memo, the mask pool's after it, once plane presence is known.
//! See: context/lib/build_pipeline.md §Compiler pipeline.

use postretro_level_format::lightmap::{
    LIGHTMAP_POOL_LAYER_EDGE, LightmapSection, RUNTIME_MAX_TEXTURE_ARRAY_LAYERS,
};
use postretro_level_format::lightmap_pool::place_all_resident;
use postretro_level_format::shadowmask_atlas::ShadowmaskAtlasSection;

use crate::lightmap_bake::{BlockLayout, PreparedAtlas};
use crate::pack::{all_resident_layers, pool_demand};

/// Count the runtime pool from finalized compiler blocks, independently of
/// their internal bake-layer placement. The renderer reaches the same
/// `place_all_resident` in level-format, via `PoolDemand::place_blocks`.
pub(crate) fn runtime_pool_layers(layout: &BlockLayout) -> anyhow::Result<u32> {
    let extents: Vec<_> = layout.blocks.iter().map(|b| (b.width, b.height)).collect();
    place_all_resident(&extents, layout.alignment(), LIGHTMAP_POOL_LAYER_EDGE)
        .map(|pool| pool.layer_count)
        .ok_or_else(|| {
            anyhow::anyhow!(
                "Lightmap runtime all-resident pool: cell blocks do not fit \
                 {LIGHTMAP_POOL_LAYER_EDGE}² pool layers"
            )
        })
}

/// Id 51 is emitted exactly when the compiler's loader-backed portal test
/// passes. Its presence therefore decides whether the shipped level can
/// stream, including on a warm bake. No static lights means no emitted blocks.
pub(crate) fn gate_runtime_pool(
    prepared: &PreparedAtlas,
    has_static_lights: bool,
    emitted_id51: bool,
) -> anyhow::Result<u32> {
    gate_runtime_pool_with_floor(
        prepared,
        has_static_lights,
        emitted_id51,
        RUNTIME_MAX_TEXTURE_ARRAY_LAYERS,
    )
}

/// The production check with a smaller runtime floor for cheap pipeline tests.
/// Pool placement always uses the real runtime layer edge.
pub(crate) fn gate_runtime_pool_with_floor(
    prepared: &PreparedAtlas,
    has_static_lights: bool,
    emitted_id51: bool,
    floor: u32,
) -> anyhow::Result<u32> {
    if !has_static_lights || prepared.layout.blocks.is_empty() {
        return Ok(0);
    }
    let layers = runtime_pool_layers(&prepared.layout)?;
    if layers <= floor {
        return Ok(layers);
    }
    if !emitted_id51 {
        anyhow::bail!(
            "Lightmap runtime all-resident pool: {} cell block(s) need {layers} pool layers, \
             exceeding the {floor}-layer runtime array-layer floor; this level cannot stream \
             because it has no usable CellResidencySet (id 51) and portals. Raise \
             --lightmap-density, lower a region's _lightmap_scale, or split the map",
            prepared.layout.blocks.len(),
        );
    }
    log::warn!(
        "[Compiler] Lightmap runtime all-resident pool: {} cell block(s) need {layers} pool \
         layers, exceeding the {floor}-layer runtime array-layer floor; this level can stream, \
         but POSTRETRO_LIGHTMAP_STREAMING=all-resident would exceed that floor",
        prepared.layout.blocks.len(),
    );
    Ok(layers)
}

/// The mask pool's floor. An all-resident level's mask pool can need up to
/// twice the lightmap pool's layers, so a level past the pre-bake check's reach
/// can still fail here. Runs on every build, because the memo holds no verdict.
pub(crate) fn gate_mask_pool(
    lightmap: &LightmapSection,
    shadowmask: Option<&ShadowmaskAtlasSection>,
    emitted_id51: bool,
) -> anyhow::Result<u32> {
    gate_mask_pool_with_floor(
        lightmap,
        shadowmask,
        emitted_id51,
        RUNTIME_MAX_TEXTURE_ARRAY_LAYERS,
    )
}

/// The production check with a smaller runtime floor for cheap pipeline tests.
pub(crate) fn gate_mask_pool_with_floor(
    lightmap: &LightmapSection,
    shadowmask: Option<&ShadowmaskAtlasSection>,
    emitted_id51: bool,
    floor: u32,
) -> anyhow::Result<u32> {
    if shadowmask.is_none() {
        return Ok(0);
    }
    let demand = pool_demand(lightmap, shadowmask)?;
    let layers = all_resident_layers(&demand)?.mask;
    if layers <= floor {
        return Ok(layers);
    }
    let planes = demand.plane_count();
    if !emitted_id51 {
        anyhow::bail!(
            "Shadowmask runtime all-resident pool: {planes} mask plane(s) need {layers} pool \
             layers, exceeding the {floor}-layer runtime array-layer floor; this level cannot \
             stream because it has no usable CellResidencySet (id 51) and portals. Raise \
             --lightmap-density, lower a region's _lightmap_scale, or split the map",
        );
    }
    log::warn!(
        "[Compiler] Shadowmask runtime all-resident pool: {planes} mask plane(s) need {layers} \
         pool layers, exceeding the {floor}-layer runtime array-layer floor; this level can \
         stream, but POSTRETRO_LIGHTMAP_STREAMING=all-resident would exceed that floor",
    );
    Ok(layers)
}

#[cfg(test)]
mod tests;
