//! SDF atlas stage: the whole-section cache lookup and bake.
//! See: context/lib/build_pipeline.md §Build Cache (SDF atlas)

use postretro_level_format::sdf_atlas::SdfAtlasSection;

use crate::cache::StageCache;
use crate::geometry::GeometryResult;
use crate::partition::BspTree;
use crate::sdf_bake;

/// The SDF atlas for `geometry`, from the cache when its key hits.
pub(super) fn bake_or_load_sdf_atlas(
    geometry: &GeometryResult,
    tree: &BspTree,
    config: &sdf_bake::SdfConfig,
    stage_cache: Option<&StageCache>,
) -> SdfAtlasSection {
    // Positions, indices and BSP solidity only: the key captures triangle
    // order, and the lightmap attributes atlas preparation wrote stay out of it.
    let sdf_key = sdf_bake::cache_key(geometry, tree, config);

    let cached = stage_cache.and_then(|c| c.get(&sdf_key));
    let cached_section = cached.and_then(|bytes| {
        SdfAtlasSection::from_bytes(&bytes)
            .map_err(|e| log::warn!("[cache] corrupt sdf_atlas entry, re-baking: {e}"))
            .ok()
    });

    if let Some(section) = cached_section {
        log::info!("[cache] sdf_atlas hit");
        return section;
    }
    log::info!("[cache] sdf_atlas miss");
    let ctx = sdf_bake::SdfBakeCtx { geometry, tree };
    let section = sdf_bake::bake_sdf_atlas(&ctx, config);
    if let Some(c) = stage_cache {
        c.put(&sdf_key, &section.to_bytes());
    }
    section
}
