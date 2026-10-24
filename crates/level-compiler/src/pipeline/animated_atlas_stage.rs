// Animated lightmap atlas stage: weight-map bake or cache load, compact layout, block stamping.
// See: context/lib/build_pipeline.md §Build Cache

use std::time::Duration;

use postretro_level_format::animated_light_weight_maps::AnimatedLightWeightMapsSection;

use crate::animated_light_weight_maps::WeightMapBake;
use crate::bake_control::BakeControl;
use crate::lightmap_bake::BlockLayout;
use crate::lightmap_layer::{SharedAtlas, atlas_layout_fingerprint};
use crate::texel_step_detector::TexelStepReport;
use crate::{
    animated_atlas_layout, animated_block_ids, animated_light_weight_maps, cache, geometry,
};

/// Load the animated-light weight maps from the stage cache, or bake them and
/// store the result with the texel-step report the bake found. Either way the
/// report logs once, here, on the caller's thread. Fails when an animated
/// face's chart leaves its lightmap cell block.
pub(super) fn bake_or_load_weight_maps(
    wm_inputs: &animated_light_weight_maps::WeightMapInputs<'_>,
    final_lightmap_density: f32,
    stage_cache: Option<&cache::StageCache>,
    animated_weight_control: &BakeControl,
) -> anyhow::Result<WeightMapBake> {
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
    // so the cache key matches the bake inputs. The chunk-section proxy does
    // not see where charts landed, and the cached blocks carry cell-block
    // keys and block-local texels, so the layout fingerprint (placements and
    // the cell-block table) is folded too. The stored report names a cut face
    // by its pre-cut index and owns pairs by cut segment, which the cut
    // geometry alone does not pin, so the cut map is folded as well.
    let layout_fingerprint = atlas_layout_fingerprint(&SharedAtlas {
        charts: wm_inputs.face_charts,
        placements: wm_inputs.face_placements,
        atlas_width,
        atlas_height,
        layout: wm_inputs.layout,
    });
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
        buf.extend_from_slice(&layout_fingerprint);
        buf.push(u8::from(wm_inputs.penumbra_floor));
        buf.extend_from_slice(&wm_inputs.step_threshold.to_bits().to_le_bytes());
        buf.extend_from_slice(&(wm_inputs.cut_parents.len() as u32).to_le_bytes());
        for cut in wm_inputs.cut_parents {
            buf.extend_from_slice(&cut.parent.to_le_bytes());
            for bound in [cut.faces.start, cut.faces.end] {
                buf.extend_from_slice(&(bound as u32).to_le_bytes());
            }
            for [u, v] in &cut.own {
                for bound in [u.start, u.end, v.start, v.end] {
                    buf.extend_from_slice(&bound.to_le_bytes());
                }
            }
        }
        *blake3::hash(&buf).as_bytes()
    };
    let wm_key = cache::CacheKey::new(
        "animated_lm_weight_maps",
        animated_light_weight_maps::STAGE_VERSION,
        &wm_input_hash,
    );

    let cached = stage_cache.and_then(|c| c.get(&wm_key)).and_then(|bytes| {
        let Some((report, section_bytes)) = TexelStepReport::parse(
            &bytes,
            animated_chunk_lights.len(),
            wm_inputs.face_charts.len(),
        ) else {
            log::warn!(
                "[cache] corrupt animated_lm_weight_maps entry, re-baking: no texel-step report"
            );
            return None;
        };
        AnimatedLightWeightMapsSection::from_bytes(section_bytes)
            .map_err(|e| e.to_string())
            .and_then(|section| match section.consistency_error() {
                None => Ok(section),
                Some(error) => Err(error),
            })
            .map_err(|e| {
                log::warn!("[cache] corrupt animated_lm_weight_maps entry, re-baking: {e}")
            })
            .ok()
            .map(|section| (section, report))
    });

    let bake = if let Some((section, texel_steps)) = cached {
        log::info!("[cache] animated_lm_weight_maps hit");
        animated_weight_control.publish_total(animated_light_chunks_section.chunks.len());
        // Cache-hit fast-advance on the orchestrator thread: honor pause only,
        // no permit (the parallel bake path is what needs a permit).
        animated_weight_control.governor().checkpoint();
        animated_weight_control.advance(animated_light_chunks_section.chunks.len());
        WeightMapBake {
            section,
            texel_steps,
            texel_step_elapsed: Duration::ZERO,
        }
    } else {
        log::info!("[cache] animated_lm_weight_maps miss");
        let bake = animated_light_weight_maps::bake_animated_light_weight_maps_controlled(
            wm_inputs,
            animated_weight_control,
        )
        .map_err(|e| anyhow::anyhow!("Animated weight-map bake failed: {e}"))?;
        if let Some(c) = stage_cache {
            // The report leads the entry, so a hit never has the section
            // without the warnings its bake found.
            c.put(
                &wm_key,
                &[bake.texel_steps.to_bytes(), bake.section.to_bytes()].concat(),
            );
        }
        bake
    };
    bake.texel_steps.log("animated", |light| {
        animated_chunk_lights[light as usize].origin
    });
    Ok(bake)
}

/// Repack the culled animated atlas into its compact layout and enforce what
/// the layout relies on. Returns each face's block for the post-SDF vertex
/// stamp, or `None` when the static lightmap has no blocks: its vertices were
/// never split or given block ids, and the runtime never samples the animated
/// atlas without a static one, so there is nothing to guard or stamp.
pub(super) fn layout_animated_atlas(
    weight_maps: &mut postretro_level_format::animated_light_weight_maps::AnimatedLightWeightMapsSection,
    chunk_section: &postretro_level_format::animated_light_chunks::AnimatedLightChunksSection,
    geo_result: &geometry::GeometryResult,
    layout: &BlockLayout,
    bake_layer_size: u32,
    static_lightmap_has_no_blocks: bool,
) -> anyhow::Result<Option<Vec<Option<u32>>>> {
    let identity_pages = weight_maps.compact_layers;
    let chosen = animated_atlas_layout::choose_compact_layout(weight_maps, layout, bake_layer_size);
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
        bake_layer_size,
        postretro_level_format::animated_lightmap_atlas::animated_atlas_byte_estimate(
            bake_layer_size,
            bake_layer_size,
            identity_pages,
        ),
    );

    // One face, one block holds on every path — it is a compiler invariant,
    // not a vertex guard — so this runs before the no-block early return.
    let face_blocks = animated_block_ids::face_blocks(
        chunk_section,
        weight_maps,
        geo_result.face_index_ranges.len(),
    )
    .map_err(|e| anyhow::anyhow!("Animated lightmap block guard failed: {e}"))?;
    if static_lightmap_has_no_blocks {
        return Ok(None);
    }
    animated_block_ids::validate_block_guards(
        &geo_result.geometry,
        &geo_result.face_index_ranges,
        weight_maps,
        &face_blocks,
        layout,
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

#[cfg(test)]
mod tests;
