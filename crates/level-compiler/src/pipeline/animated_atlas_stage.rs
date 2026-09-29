// Animated lightmap atlas stage: weight-map bake or cache load, compact layout, block stamping.
// See: context/lib/build_pipeline.md §Build Cache

use postretro_level_format::animated_light_weight_maps::AnimatedLightWeightMapsSection;

use crate::bake_control::BakeControl;
use crate::{
    animated_atlas_layout, animated_block_ids, animated_light_weight_maps, cache, geometry,
};

/// Load the animated-light weight maps from the stage cache, or bake them and
/// store the result. Returns `Some` on both paths.
pub(super) fn bake_or_load_weight_maps(
    wm_inputs: &animated_light_weight_maps::WeightMapInputs<'_>,
    final_lightmap_density: f32,
    stage_cache: Option<&cache::StageCache>,
    animated_weight_control: &BakeControl,
) -> Option<AnimatedLightWeightMapsSection> {
    let animated_chunk_lights = wm_inputs.lights;
    let geo_result = wm_inputs.geometry;
    let atlas_width = wm_inputs.atlas_width;
    let atlas_height = wm_inputs.atlas_height;
    let static_atlas_layer_count = wm_inputs.static_atlas_layer_count;
    let animated_light_chunks_section = wm_inputs.chunk_section;
    let soft_shadow_samples = wm_inputs.area_sample_count;

    // Build the input hash from owned/serializable data. Charts, placements,
    // and the chunk section don't derive `Serialize`, so the hash folds
    // `animated_light_chunks_section.to_bytes()` as a proxy. That proxy is a
    // valid fingerprint for charts AND placements because
    // `build_animated_light_chunks` (and the upstream chart/placement
    // construction) are deterministic given geometry + lights + density — the
    // section bytes faithfully capture those derived inputs.
    //
    // Weight maps run after atlas preparation and consume `geo_result` with
    // split vertices and assigned atlas UVs. Hash that same prepared geometry
    // so the cache key matches the bake inputs.
    let wm_input_hash = {
        let mut buf = postcard::to_allocvec(&animated_chunk_lights)
            .expect("postcard serialize animated_chunk_lights");
        buf.extend_from_slice(
            &postcard::to_allocvec(&geo_result).expect("postcard serialize geo_result"),
        );
        buf.extend_from_slice(&final_lightmap_density.to_le_bytes());
        buf.extend_from_slice(&atlas_width.to_le_bytes());
        buf.extend_from_slice(&atlas_height.to_le_bytes());
        buf.extend_from_slice(&static_atlas_layer_count.to_le_bytes());
        buf.extend_from_slice(&animated_light_chunks_section.to_bytes());
        buf.extend_from_slice(&soft_shadow_samples.to_le_bytes());
        *blake3::hash(&buf).as_bytes()
    };
    let wm_key = cache::CacheKey::new(
        "animated_lm_weight_maps",
        animated_light_weight_maps::STAGE_VERSION,
        &wm_input_hash,
    );

    let cached = stage_cache.and_then(|c| c.get(&wm_key));
    let cached_wm_section = cached.and_then(|bytes| {
        postretro_level_format::animated_light_weight_maps::AnimatedLightWeightMapsSection::from_bytes(&bytes)
            .map_err(|e| e.to_string())
            .and_then(|section| match section.consistency_error() {
                None => Ok(section),
                Some(error) => Err(error),
            })
            .map_err(|e| {
                log::warn!("[cache] corrupt animated_lm_weight_maps entry, re-baking: {e}")
            })
            .ok()
    });

    if let Some(section) = cached_wm_section {
        log::info!("[cache] animated_lm_weight_maps hit");
        animated_weight_control.publish_total(animated_light_chunks_section.chunks.len());
        // Cache-hit fast-advance on the orchestrator thread: honor pause only,
        // no permit (the parallel bake path is what needs a permit).
        animated_weight_control.governor().checkpoint();
        animated_weight_control.advance(animated_light_chunks_section.chunks.len());
        Some(section)
    } else {
        log::info!("[cache] animated_lm_weight_maps miss");
        let section = animated_light_weight_maps::bake_animated_light_weight_maps_controlled(
            wm_inputs,
            animated_weight_control,
        );
        if let Some(c) = stage_cache {
            c.put(&wm_key, &section.to_bytes());
        }
        Some(section)
    }
}

/// Repack the culled animated atlas into its compact layout and enforce what
/// the layout relies on. Returns each face's block for the post-SDF vertex
/// stamp, or `None` when the static lightmap is the placeholder: its vertices
/// were never split or given lightmap UVs, and the runtime never samples the
/// animated atlas without a static one, so there is nothing to guard or stamp.
pub(super) fn layout_animated_atlas(
    weight_maps: &mut postretro_level_format::animated_light_weight_maps::AnimatedLightWeightMapsSection,
    chunk_section: &postretro_level_format::animated_light_chunks::AnimatedLightChunksSection,
    geo_result: &geometry::GeometryResult,
    static_layer_size: u32,
    static_lightmap_is_placeholder: bool,
) -> anyhow::Result<Option<Vec<Option<u32>>>> {
    let identity_pages = weight_maps.compact_layers;
    let chosen = animated_atlas_layout::choose_compact_layout(weight_maps, static_layer_size);
    if let Some(error) = weight_maps.consistency_error() {
        anyhow::bail!("Animated lightmap atlas layout is inconsistent: {error}");
    }
    animated_light_weight_maps::validate_animated_atlas_budget(
        weight_maps.page_size,
        weight_maps.compact_layers,
    )
    .map_err(|e| anyhow::anyhow!("Animated weight-map bake failed: {e}"))?;
    log::info!(
        "[AnimatedLightWeightMaps] {chosen:?} layout: {} blocks on {} pages of {}² \
         ({} bytes; identity layout {} pages of {}², {} bytes)",
        weight_maps.blocks.len(),
        weight_maps.compact_layers,
        weight_maps.page_size,
        postretro_level_format::animated_lightmap_atlas::animated_atlas_byte_estimate(
            weight_maps.page_size,
            weight_maps.page_size,
            weight_maps.compact_layers,
        ),
        identity_pages,
        static_layer_size,
        postretro_level_format::animated_lightmap_atlas::animated_atlas_byte_estimate(
            static_layer_size,
            static_layer_size,
            identity_pages,
        ),
    );

    // One face, one block holds on every path — it is a compiler invariant,
    // not a vertex guard — so this runs before the placeholder early return.
    let face_blocks = animated_block_ids::face_blocks(
        chunk_section,
        weight_maps,
        geo_result.face_index_ranges.len(),
    )
    .map_err(|e| anyhow::anyhow!("Animated lightmap block guard failed: {e}"))?;
    if static_lightmap_is_placeholder {
        return Ok(None);
    }
    animated_block_ids::validate_block_guards(
        &geo_result.geometry,
        &geo_result.face_index_ranges,
        weight_maps,
        &face_blocks,
        static_layer_size,
    )
    .map_err(|e| anyhow::anyhow!("Animated lightmap block guard failed: {e}"))?;
    Ok(Some(face_blocks))
}

/// Stamp each face's animated block id onto its vertices.
pub(super) fn stamp_animated_blocks(
    geo_result: &mut geometry::GeometryResult,
    animated_face_blocks: Option<&Vec<Option<u32>>>,
) {
    if let Some(face_blocks) = animated_face_blocks {
        animated_block_ids::stamp_animated_block_ids(
            &mut geo_result.geometry,
            &geo_result.face_index_ranges,
            face_blocks,
        );
    }
}
