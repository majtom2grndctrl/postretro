// Animated lightmap atlas stage: weight-map bake or cache load, compact layout, block stamping.
// See: context/lib/build_pipeline.md §Build Cache

use std::time::Duration;

use postretro_level_format::animated_light_weight_maps::AnimatedLightWeightMapsSection;

use crate::animated_light_weight_maps::{WeightMapBake, WeightMapInputs};
use crate::bake_control::BakeControl;
use crate::light_namespaces::AnimatedBakedLights;
use crate::lightmap_bake::{BlockLayout, cut_map_key_bytes};
use crate::lightmap_layer::{SharedAtlas, atlas_layout_fingerprint};
use crate::texel_footprint::EmbeddedEmitters;
use crate::texel_step_detector::{CutSites, TexelStepReport};
use crate::{
    animated_atlas_layout, animated_block_ids, animated_light_weight_maps, cache, geometry,
};

/// Load the animated-light weight maps from the stage cache, or bake them and
/// store the result with the texel-step report the bake found. Either way the
/// report logs once, here, on the caller's thread, naming each light by its
/// source label: `animated_lights` is the list `wm_inputs.lights` was taken
/// from, in order. `probe_embedded_emitters` are the probe-point bakes'
/// emitter flags, keyed alike ([`crate::animated_direct_sh_bake::embedded_emitters`]);
/// each light either set flags warns once, here. Fails when an animated
/// face's chart leaves its lightmap cell block.
pub(super) fn bake_or_load_weight_maps(
    wm_inputs: &WeightMapInputs<'_>,
    final_lightmap_density: f32,
    stage_cache: Option<&cache::StageCache>,
    animated_weight_control: &BakeControl,
    animated_lights: &AnimatedBakedLights<'_>,
    light_source_labels: &[String],
    probe_embedded_emitters: &EmbeddedEmitters,
) -> anyhow::Result<WeightMapBake> {
    debug_assert_eq!(animated_lights.len(), wm_inputs.lights.len());
    let animated_light_chunks_section = wm_inputs.chunk_section;
    let wm_key = weight_map_cache_key(wm_inputs, final_lightmap_density);
    // Tested every compile, warm or cold, on the cut geometry the stage
    // bakes, and warned here once per light: the flags read only the keyed
    // lights and geometry, so a cached entry was baked under the same ones.
    let embedded_emitters =
        animated_light_weight_maps::embedded_emitters(wm_inputs, animated_weight_control);
    warn_embedded_emitters(
        &embedded_emitters,
        probe_embedded_emitters,
        animated_lights,
        light_source_labels,
    );

    let cached = stage_cache.and_then(|c| c.get(&wm_key)).and_then(|bytes| {
        let sites = CutSites::new(wm_inputs.cut_parents, wm_inputs.face_charts);
        let Some((report, section_bytes)) =
            TexelStepReport::parse(&bytes, wm_inputs.lights.len(), &sites)
        else {
            log::warn!(
                "[cache] corrupt animated_lm_weight_maps entry, re-baking: malformed texel-step report"
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
        let bake = animated_light_weight_maps::bake_animated_light_weight_maps_with(
            wm_inputs,
            &embedded_emitters,
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
        let entry = animated_lights.entries().get(light as usize)?;
        light_source_labels
            .get(entry.source_index)
            .map(String::as_str)
    });
    Ok(bake)
}

/// Warn each animated light flagged by the weight-map stage's emitter test
/// (`weight_maps`) or by the probe-point bakes' (`probes`), once, on the
/// caller thread; both are keyed by position in `animated_lights`. The
/// pipeline calls this with no weight-map flags when the stage has no chunk
/// to bake, so a light only the probe stages flag still warns.
pub(super) fn warn_embedded_emitters(
    weight_maps: &EmbeddedEmitters,
    probes: &EmbeddedEmitters,
    animated_lights: &AnimatedBakedLights<'_>,
    light_source_labels: &[String],
) {
    let entries = animated_lights.entries();
    EmbeddedEmitters::warn(
        "animated",
        weight_maps,
        probes,
        |light| entries.get(light).map(|entry| entry.light),
        |light| {
            let entry = entries.get(light)?;
            light_source_labels
                .get(entry.source_index)
                .map(String::as_str)
        },
    );
}

/// The `animated_lm_weight_maps` cache key.
///
/// Build the input hash from owned/serializable data. Charts, placements,
/// and the chunk section don't derive `Serialize`, so the hash folds
/// `chunk_section.to_bytes()` as a proxy. That proxy is a valid fingerprint
/// for charts AND placements because `build_animated_light_chunks` (and the
/// upstream chart/placement construction) are deterministic given geometry +
/// lights + density — the section bytes faithfully capture those derived
/// inputs.
///
/// Weight maps run after atlas preparation and consume the geometry with
/// split vertices and assigned atlas UVs. Hash that same prepared geometry so
/// the cache key matches the bake inputs. The chunk-section proxy does not
/// see where charts landed, and the cached blocks carry cell-block keys and
/// block-local texels, so the layout fingerprint (placements and the
/// cell-block table) is folded too. The stored report names a cut face by its
/// pre-cut index and owns pairs by cut segment, so the cut map is folded as
/// well.
fn weight_map_cache_key(
    wm_inputs: &WeightMapInputs<'_>,
    final_lightmap_density: f32,
) -> cache::CacheKey {
    let layout_fingerprint = atlas_layout_fingerprint(&SharedAtlas {
        charts: wm_inputs.face_charts,
        placements: wm_inputs.face_placements,
        atlas_width: wm_inputs.atlas_width,
        atlas_height: wm_inputs.atlas_height,
        layout: wm_inputs.layout,
    });
    let mut buf =
        postcard::to_allocvec(&wm_inputs.lights).expect("postcard serialize animated_chunk_lights");
    buf.extend_from_slice(
        &postcard::to_allocvec(&wm_inputs.geometry).expect("postcard serialize geo_result"),
    );
    buf.extend_from_slice(&final_lightmap_density.to_le_bytes());
    buf.extend_from_slice(&wm_inputs.atlas_width.to_le_bytes());
    buf.extend_from_slice(&wm_inputs.atlas_height.to_le_bytes());
    buf.extend_from_slice(&wm_inputs.static_atlas_layer_count.to_le_bytes());
    buf.extend_from_slice(&wm_inputs.chunk_section.to_bytes());
    buf.extend_from_slice(&wm_inputs.area_sample_count.to_le_bytes());
    buf.extend_from_slice(&layout_fingerprint);
    buf.push(u8::from(wm_inputs.penumbra_floor));
    buf.extend_from_slice(&wm_inputs.step_threshold.to_bits().to_le_bytes());
    buf.extend_from_slice(&cut_map_key_bytes(wm_inputs.cut_parents));
    cache::CacheKey::new(
        "animated_lm_weight_maps",
        animated_light_weight_maps::STAGE_VERSION,
        blake3::hash(&buf).as_bytes(),
    )
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
