// Per-light shadowmask bake for selected static entity-shadow lights.
// Governing context: context/lib/build_pipeline.md

use std::collections::HashMap;

use postretro_level_format::entity_shadow_lights::EntityShadowLightsSection;
use postretro_level_format::shadowmask_atlas::{
    SHADOWMASK_CHANNEL_DROPPED, ShadowmaskAtlasSection,
};

use crate::bake_control::BakeControl;
use crate::bvh_build::BvhPrimitive;
use crate::cache::{CacheKey, StageCache};
use crate::geometry::GeometryResult;
use crate::light_namespaces::AlphaLightsNs;
use crate::lightmap_layer::{self, LightmapLayer, SharedAtlas, bake_light_layer};
use crate::map_data::MapLight;

pub const SHADOWMASK_ATLAS_STAGE_ID: &str = "shadowmask_atlas";

/// Bump when the cached `ShadowmaskAtlas` bytes can change without a layer input
/// hash change: channel assignment/drop policy, raw-visibility quantization to
/// `Rgba8Unorm`, empty-section behavior, or `ShadowmaskAtlasSection::to_bytes`
/// payload semantics.
pub const SHADOWMASK_ATLAS_STAGE_VERSION: u32 = 1;

#[allow(clippy::too_many_arguments)]
pub fn bake_shadowmask_atlas(
    selection: Option<&EntityShadowLightsSection>,
    alpha_lights: &AlphaLightsNs<'_>,
    shared: &SharedAtlas<'_>,
    bvh: &bvh::bvh::Bvh<f32, 3>,
    primitives: &[BvhPrimitive],
    geometry: &GeometryResult,
    area_sample_count: u32,
    control: &BakeControl,
) -> Option<ShadowmaskAtlasSection> {
    let selection = selection?;
    if selection.light_indices.is_empty() {
        return None;
    }

    let mut selected = Vec::with_capacity(selection.light_indices.len());
    for (selection_index, &alpha_index) in selection.light_indices.iter().enumerate() {
        let Some(entry) = alpha_lights.entries().get(alpha_index as usize) else {
            log::warn!(
                "[ShadowmaskAtlas] selected AlphaLights index {alpha_index} is out of range; marking dropped"
            );
            continue;
        };
        selected.push((selection_index, alpha_index, entry.light));
    }
    if selected.is_empty() {
        return Some(empty_section_for_selection(
            shared,
            selection.light_indices.len(),
        ));
    }

    let layers: Vec<LightmapLayer> = selected
        .iter()
        .map(|(_, _, light)| {
            bake_light_layer(
                light,
                shared,
                bvh,
                primitives,
                geometry,
                area_sample_count,
                control,
            )
        })
        .collect();
    Some(build_shadowmask_from_layers(
        shared.atlas_width,
        shared.atlas_height,
        layer_count_from_shared(shared) as usize,
        selection.light_indices.len(),
        &selected,
        &layers,
    ))
}

#[allow(clippy::too_many_arguments)]
pub fn bake_shadowmask_atlas_cached(
    selection: Option<&EntityShadowLightsSection>,
    alpha_lights: &AlphaLightsNs<'_>,
    shared: &SharedAtlas<'_>,
    bvh: &bvh::bvh::Bvh<f32, 3>,
    primitives: &[BvhPrimitive],
    geometry: &GeometryResult,
    lightmap_density: f32,
    area_sample_count: u32,
    stage_cache: Option<&StageCache>,
    control: &BakeControl,
) -> Option<ShadowmaskAtlasSection> {
    let Some(cache) = stage_cache else {
        return bake_shadowmask_atlas(
            selection,
            alpha_lights,
            shared,
            bvh,
            primitives,
            geometry,
            area_sample_count,
            control,
        );
    };

    let selection = selection?;
    if selection.light_indices.is_empty() {
        return None;
    }

    let mut selected = Vec::with_capacity(selection.light_indices.len());
    let mut layer_input_hashes = Vec::with_capacity(selection.light_indices.len());
    for (selection_index, &alpha_index) in selection.light_indices.iter().enumerate() {
        let Some(entry) = alpha_lights.entries().get(alpha_index as usize) else {
            log::warn!(
                "[ShadowmaskAtlas] selected AlphaLights index {alpha_index} is out of range; marking dropped"
            );
            layer_input_hashes.push(invalid_selected_light_hash(alpha_index));
            continue;
        };

        let input_hash = lightmap_layer::layer_input_hash(
            entry.light,
            shared,
            primitives,
            geometry,
            lightmap_density,
            area_sample_count,
        );
        layer_input_hashes.push(input_hash);
        selected.push((selection_index, alpha_index, entry.light, input_hash));
    }

    let layer_count = layer_count_from_shared(shared);
    let section_input_hash = shadowmask_atlas_input_hash(
        selection,
        &layer_input_hashes,
        shared.atlas_width,
        shared.atlas_height,
        layer_count,
    );
    let section_key = CacheKey::new(
        SHADOWMASK_ATLAS_STAGE_ID,
        SHADOWMASK_ATLAS_STAGE_VERSION,
        &section_input_hash,
    );

    let cached_section = cache.get(&section_key).and_then(|bytes| {
        match ShadowmaskAtlasSection::from_bytes(&bytes) {
            Ok(section) => match validate_cached_shadowmask_section(
                &section, selection, shared, layer_count,
            ) {
                Ok(()) => Some(section),
                Err(reason) => {
                    log::warn!(
                        "[Compiler] shadowmask_atlas cache entry does not match current atlas ({reason}), rebuilding"
                    );
                    None
                }
            },
            Err(err) => {
                log::warn!("[Compiler] corrupt shadowmask_atlas section, rebuilding: {err}");
                None
            }
        }
    });
    if let Some(section) = cached_section {
        log::info!("[cache] shadowmask_atlas hit");
        return Some(section);
    }

    log::info!("[cache] shadowmask_atlas miss");
    let mut selected_refs = Vec::with_capacity(selected.len());
    let mut layers = Vec::with_capacity(selected.len());
    for (selection_index, alpha_index, light, input_hash) in selected {
        selected_refs.push((selection_index, alpha_index, light));
        let layer_key = CacheKey::new(
            "lightmap_layer",
            lightmap_layer::LAYER_FORMAT_VERSION,
            &input_hash,
        );
        let layer = match cache
            .get(&layer_key)
            .and_then(|bytes| lightmap_layer::LightmapLayer::from_bytes(&bytes))
            .and_then(|layer| match validate_cached_lightmap_layer(&layer, shared, layer_count) {
                Ok(()) => Some(layer),
                Err(reason) => {
                    log::warn!(
                        "[Compiler] corrupt lightmap_layer cache entry for shadowmask selected AlphaLights index {alpha_index} ({reason}), re-baking"
                    );
                    None
                }
            })
        {
            Some(layer) => {
                log::info!("[cache] lightmap_layer hit");
                layer
            }
            None => {
                log::info!("[cache] lightmap_layer miss");
                let layer = bake_light_layer(
                    light,
                    shared,
                    bvh,
                    primitives,
                    geometry,
                    area_sample_count,
                    control,
                );
                cache.put(&layer_key, &layer.to_bytes());
                layer
            }
        };
        layers.push(layer);
    }

    let section = bake_shadowmask_atlas_from_layers(
        selection,
        shared.atlas_width,
        shared.atlas_height,
        layer_count,
        &selected_refs,
        &layers,
    );
    if let Some(ref section) = section {
        cache.put(&section_key, &section.to_bytes());
    }
    section
}

/// Build the shadowmask section from preloaded per-light layers.
///
/// `selected` and `layers` must be in the same order as
/// `selection.light_indices`, after dropping any out-of-range selected
/// `AlphaLights` entries the same way the uncached path does. Each `selected`
/// entry carries its original selection index so invalid earlier selections do
/// not shift the channel table.
pub fn bake_shadowmask_atlas_from_layers(
    selection: &EntityShadowLightsSection,
    atlas_width: u32,
    atlas_height: u32,
    layer_count: u32,
    selected: &[(usize, u32, &MapLight)],
    layers: &[LightmapLayer],
) -> Option<ShadowmaskAtlasSection> {
    if selection.light_indices.is_empty() {
        return None;
    }

    debug_assert_eq!(
        selected.len(),
        layers.len(),
        "shadowmask selected light/layer slices must align"
    );

    if selected.is_empty() {
        return Some(empty_section_for_dimensions(
            atlas_width,
            atlas_height,
            layer_count,
            selection.light_indices.len(),
        ));
    }

    Some(build_shadowmask_from_layers(
        atlas_width,
        atlas_height,
        layer_count as usize,
        selection.light_indices.len(),
        selected,
        layers,
    ))
}

/// Whole-section input hash for the `"shadowmask_atlas"` memo entry.
///
/// The byte layout is fixed and order-sensitive:
/// `LAYER_FORMAT_VERSION`, selected-light count, selected `AlphaLights` indices,
/// selected per-light `lightmap_layer::layer_input_hash` values, then atlas
/// width/height/layer-count. The caller supplies the hashes in exactly the same
/// order as `selection.light_indices`; the helper does no sorting.
pub fn shadowmask_atlas_input_hash(
    selection: &EntityShadowLightsSection,
    layer_input_hashes: &[[u8; 32]],
    atlas_width: u32,
    atlas_height: u32,
    layer_count: u32,
) -> [u8; 32] {
    debug_assert_eq!(
        selection.light_indices.len(),
        layer_input_hashes.len(),
        "shadowmask selected light/hash slices must align"
    );

    let mut hasher = blake3::Hasher::new();
    hasher.update(&lightmap_layer::LAYER_FORMAT_VERSION.to_le_bytes());
    hasher.update(&(selection.light_indices.len() as u32).to_le_bytes());
    for &alpha_index in &selection.light_indices {
        hasher.update(&alpha_index.to_le_bytes());
    }
    for hash in layer_input_hashes {
        hasher.update(hash);
    }
    hasher.update(&atlas_width.to_le_bytes());
    hasher.update(&atlas_height.to_le_bytes());
    hasher.update(&layer_count.to_le_bytes());
    *hasher.finalize().as_bytes()
}

fn empty_section_for_selection(
    shared: &SharedAtlas<'_>,
    selected_light_count: usize,
) -> ShadowmaskAtlasSection {
    let layer_count = layer_count_from_shared(shared);
    empty_section_for_dimensions(
        shared.atlas_width,
        shared.atlas_height,
        layer_count,
        selected_light_count,
    )
}

fn empty_section_for_dimensions(
    width: u32,
    height: u32,
    layer_count: u32,
    selected_light_count: usize,
) -> ShadowmaskAtlasSection {
    let texel_count = width as usize * height as usize * layer_count as usize;
    ShadowmaskAtlasSection {
        width,
        height,
        layer_count,
        channels: vec![SHADOWMASK_CHANNEL_DROPPED; selected_light_count],
        data: vec![255; texel_count * 4],
    }
}

fn layer_count_from_shared(shared: &SharedAtlas<'_>) -> u32 {
    shared
        .placements
        .iter()
        .map(|placement| placement.layer + 1)
        .max()
        .unwrap_or(1)
}

fn invalid_selected_light_hash(alpha_index: u32) -> [u8; 32] {
    let mut hasher = blake3::Hasher::new();
    hasher.update(b"shadowmask_atlas_invalid_selected_alpha_light");
    hasher.update(&alpha_index.to_le_bytes());
    *hasher.finalize().as_bytes()
}

fn validate_cached_shadowmask_section(
    section: &ShadowmaskAtlasSection,
    selection: &EntityShadowLightsSection,
    shared: &SharedAtlas<'_>,
    layer_count: u32,
) -> Result<(), String> {
    if section.width != shared.atlas_width || section.height != shared.atlas_height {
        return Err(format!(
            "dimensions {}x{} != {}x{}",
            section.width, section.height, shared.atlas_width, shared.atlas_height
        ));
    }
    if section.layer_count != layer_count {
        return Err(format!(
            "layer_count {} != {}",
            section.layer_count, layer_count
        ));
    }
    if section.channels.len() != selection.light_indices.len() {
        return Err(format!(
            "channel count {} != selected light count {}",
            section.channels.len(),
            selection.light_indices.len()
        ));
    }
    Ok(())
}

fn validate_cached_lightmap_layer(
    layer: &LightmapLayer,
    shared: &SharedAtlas<'_>,
    layer_count: u32,
) -> Result<(), String> {
    if layer.atlas_width != shared.atlas_width || layer.atlas_height != shared.atlas_height {
        return Err(format!(
            "dimensions {}x{} != {}x{}",
            layer.atlas_width, layer.atlas_height, shared.atlas_width, shared.atlas_height
        ));
    }
    if layer.layer_count != layer_count {
        return Err(format!(
            "layer_count {} != {}",
            layer.layer_count, layer_count
        ));
    }

    let Some(plane) = (shared.atlas_width as usize).checked_mul(shared.atlas_height as usize)
    else {
        return Err("atlas dimensions overflow texel plane size".to_string());
    };
    for (texel_index, texel) in layer.texels.iter().enumerate() {
        if texel.layer >= layer_count {
            return Err(format!(
                "texel {texel_index} layer {} out of bounds for {} layers",
                texel.layer, layer_count
            ));
        }
        if texel.idx as usize >= plane {
            return Err(format!(
                "texel {texel_index} idx {} out of bounds for {} texels",
                texel.idx, plane
            ));
        }
    }

    Ok(())
}

fn build_shadowmask_from_layers(
    width: u32,
    height: u32,
    layer_count: usize,
    selected_light_count: usize,
    selected: &[(usize, u32, &MapLight)],
    layers: &[LightmapLayer],
) -> ShadowmaskAtlasSection {
    let plane = (width * height) as usize;
    let texel_count = plane * layer_count;
    let masks: Vec<Vec<(usize, u8)>> = layers
        .iter()
        .map(|layer| {
            layer
                .texels
                .iter()
                .filter_map(|texel| {
                    if texel.raw_visibility < 0.0 {
                        return None;
                    }
                    let global_idx = texel.layer as usize * plane + texel.idx as usize;
                    let value = (texel.raw_visibility.clamp(0.0, 1.0) * 255.0 + 0.5) as u8;
                    Some((global_idx, value))
                })
                .collect()
        })
        .collect();

    let graph = overlap_graph(&masks);
    let compact_channels = assign_channels_with_drops(&graph, selected);
    let mut channels = vec![SHADOWMASK_CHANNEL_DROPPED; selected_light_count];
    for (compact_index, &(selection_index, _, _)) in selected.iter().enumerate() {
        if selection_index < channels.len() {
            channels[selection_index] = compact_channels
                .get(compact_index)
                .copied()
                .unwrap_or(SHADOWMASK_CHANNEL_DROPPED);
        }
    }

    let mut data = vec![255u8; texel_count * 4];
    for (compact_index, mask) in masks.iter().enumerate() {
        let channel = compact_channels
            .get(compact_index)
            .copied()
            .unwrap_or(SHADOWMASK_CHANNEL_DROPPED);
        if channel == SHADOWMASK_CHANNEL_DROPPED {
            continue;
        }
        for &(texel_idx, visibility) in mask {
            data[texel_idx * 4 + channel as usize] = visibility;
        }
    }

    ShadowmaskAtlasSection {
        width,
        height,
        layer_count: layer_count as u32,
        channels,
        data,
    }
}

fn overlap_graph(masks: &[Vec<(usize, u8)>]) -> Vec<Vec<bool>> {
    let mut graph = vec![vec![false; masks.len()]; masks.len()];
    let mut texel_lights: HashMap<usize, Vec<usize>> = HashMap::new();
    for (light_index, mask) in masks.iter().enumerate() {
        for &(texel_idx, _) in mask {
            texel_lights.entry(texel_idx).or_default().push(light_index);
        }
    }
    for lights in texel_lights.values() {
        for (pos, &a) in lights.iter().enumerate() {
            for &b in &lights[pos + 1..] {
                graph[a][b] = true;
                graph[b][a] = true;
            }
        }
    }
    graph
}

fn assign_channels_with_drops(
    graph: &[Vec<bool>],
    selected: &[(usize, u32, &MapLight)],
) -> Vec<u8> {
    let mut active = vec![true; graph.len()];
    loop {
        if let Some(channels) = color_graph(graph, &active) {
            let dropped: Vec<usize> = active
                .iter()
                .enumerate()
                .filter_map(|(i, &is_active)| (!is_active).then_some(i))
                .collect();
            if !dropped.is_empty() {
                log::warn!(
                    "[ShadowmaskAtlas] dropped {} selected light mask(s) after >4-way overlap: {:?}",
                    dropped.len(),
                    dropped
                );
            }
            return channels;
        }

        let Some(drop_index) = lowest_intensity_active(selected, &active) else {
            return vec![SHADOWMASK_CHANNEL_DROPPED; graph.len()];
        };
        active[drop_index] = false;
    }
}

fn lowest_intensity_active(selected: &[(usize, u32, &MapLight)], active: &[bool]) -> Option<usize> {
    active
        .iter()
        .enumerate()
        .filter(|&(_, is_active)| *is_active)
        .min_by(|&(a, _), &(b, _)| {
            light_intensity_score(selected[a].2)
                .total_cmp(&light_intensity_score(selected[b].2))
                .then(selected[a].0.cmp(&selected[b].0))
                .then(selected[a].1.cmp(&selected[b].1))
        })
        .map(|(i, _)| i)
}

fn light_intensity_score(light: &MapLight) -> f32 {
    light.intensity * light.color[0].max(light.color[1]).max(light.color[2])
}

fn color_graph(graph: &[Vec<bool>], active: &[bool]) -> Option<Vec<u8>> {
    let mut order: Vec<usize> = active
        .iter()
        .enumerate()
        .filter_map(|(i, &is_active)| is_active.then_some(i))
        .collect();
    order.sort_by(|&a, &b| {
        degree(graph, active, b)
            .cmp(&degree(graph, active, a))
            .then(a.cmp(&b))
    });

    let mut channels = vec![SHADOWMASK_CHANNEL_DROPPED; graph.len()];
    if color_order(graph, active, &order, 0, &mut channels) {
        Some(channels)
    } else {
        None
    }
}

fn color_order(
    graph: &[Vec<bool>],
    active: &[bool],
    order: &[usize],
    cursor: usize,
    channels: &mut [u8],
) -> bool {
    if cursor == order.len() {
        return true;
    }
    let light = order[cursor];
    for channel in 0..4u8 {
        let used_by_neighbor = (0..graph.len())
            .any(|other| active[other] && graph[light][other] && channels[other] == channel);
        if used_by_neighbor {
            continue;
        }
        channels[light] = channel;
        if color_order(graph, active, order, cursor + 1, channels) {
            return true;
        }
        channels[light] = SHADOWMASK_CHANNEL_DROPPED;
    }
    false
}

fn degree(graph: &[Vec<bool>], active: &[bool], light: usize) -> usize {
    graph[light]
        .iter()
        .enumerate()
        .filter(|&(i, &adjacent)| active[i] && adjacent)
        .count()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bake_control::BakeControl;
    use crate::bvh_build::build_bvh;
    use crate::governor::Governor;
    use crate::light_namespaces::{AlphaLightsNs, StaticBakedLights};
    use crate::lightmap_bake::prepare_atlas;
    use crate::lightmap_layer::LayerTexel;
    use crate::map_data::{FalloffModel, LightType, ShadowType};
    use crate::reporter::StageProgress;
    use glam::DVec3;
    use postretro_level_format::geometry::{FaceMeta, GeometrySection, Vertex};
    use postretro_level_format::texture_names::TextureNamesSection;

    fn light(intensity: f32) -> MapLight {
        MapLight {
            origin: DVec3::ZERO,
            carrier: String::new(),
            light_type: LightType::Point,
            intensity,
            color: [1.0, 1.0, 1.0],
            falloff_model: FalloffModel::Linear,
            falloff_range: 4.0,
            light_size: 0.0,
            angular_diameter: 0.0,
            cone_angle_inner: None,
            cone_angle_outer: None,
            cone_direction: None,
            animation: None,
            bake_only: false,
            is_dynamic: false,
            casts_entity_shadows: false,
            is_animated: false,
            tags: Vec::new(),
            shadow_type: ShadowType::StaticLightMap,
        }
    }

    fn quad_geometry() -> GeometryResult {
        let n = [0.0, 1.0, 0.0];
        let t = [1.0, 0.0, 0.0];
        GeometryResult {
            geometry: GeometrySection {
                vertices: vec![
                    Vertex::new([0.0, 0.0, 0.0], [0.0, 0.0], n, t, true, [0.0, 0.0], 0),
                    Vertex::new([1.0, 0.0, 0.0], [1.0, 0.0], n, t, true, [0.0, 0.0], 0),
                    Vertex::new([1.0, 0.0, 1.0], [1.0, 1.0], n, t, true, [0.0, 0.0], 0),
                    Vertex::new([0.0, 0.0, 1.0], [0.0, 1.0], n, t, true, [0.0, 0.0], 0),
                ],
                indices: vec![0, 1, 2, 0, 2, 3],
                faces: vec![FaceMeta {
                    leaf_index: 0,
                    texture_index: 0,
                }],
            },
            texture_names: TextureNamesSection { names: Vec::new() },
            face_index_ranges: vec![crate::geometry::FaceIndexRange {
                index_offset: 0,
                index_count: 6,
            }],
        }
    }

    fn layer(
        width: u32,
        height: u32,
        layer_count: u32,
        texels: &[(u32, u32, f32)],
    ) -> LightmapLayer {
        LightmapLayer {
            atlas_width: width,
            atlas_height: height,
            layer_count,
            texels: texels
                .iter()
                .map(|&(idx, layer, visibility)| LayerTexel {
                    idx,
                    layer,
                    irradiance: [0.0; 3],
                    weighted_dir: [0.0; 3],
                    fallback_normal: [0.0, 1.0, 0.0],
                    raw_visibility: visibility,
                })
                .collect(),
        }
    }

    fn fake_layer_hash(seed: u8) -> [u8; 32] {
        let mut hash = [0u8; 32];
        hash[0] = seed;
        hash[17] = seed.wrapping_mul(31);
        hash[31] = seed.wrapping_add(7);
        hash
    }

    use crate::cache::{CacheKey, StageCache};
    use crate::lightmap_bake::PreparedAtlas;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicU64, Ordering};

    const DENSITY: f32 = 0.25;
    const AREA_SAMPLES: u32 = 4;

    fn test_control() -> BakeControl {
        let progress = StageProgress::indeterminate();
        BakeControl::new(Arc::new(Governor::new(1, false)), &progress)
    }

    fn fresh_cache_dir(label: &str) -> std::path::PathBuf {
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        let nonce = COUNTER.fetch_add(1, Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!(
            "postretro_shadowmask_cache_test_{label}_{nonce}_{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        dir
    }

    fn cache_fixture(
        lights: &[MapLight],
    ) -> (
        GeometryResult,
        PreparedAtlas,
        bvh::bvh::Bvh<f32, 3>,
        Vec<BvhPrimitive>,
    ) {
        let mut geo = quad_geometry();
        let static_lights = StaticBakedLights::from_lights(lights);
        let prepared = prepare_atlas(&mut geo, &static_lights, DENSITY).unwrap();
        let (bvh, primitives, _) = build_bvh(&geo).unwrap();
        (geo, prepared, bvh, primitives)
    }

    fn shared_from_prepared(prepared: &PreparedAtlas) -> SharedAtlas<'_> {
        SharedAtlas {
            charts: &prepared.charts,
            placements: &prepared.placements,
            atlas_width: prepared.atlas_width,
            atlas_height: prepared.atlas_height,
        }
    }

    fn layer_key(
        light: &MapLight,
        shared: &SharedAtlas<'_>,
        primitives: &[BvhPrimitive],
        geo: &GeometryResult,
        area_samples: u32,
    ) -> (CacheKey, [u8; 32]) {
        let input_hash =
            lightmap_layer::layer_input_hash(light, shared, primitives, geo, DENSITY, area_samples);
        (
            CacheKey::new(
                "lightmap_layer",
                lightmap_layer::LAYER_FORMAT_VERSION,
                &input_hash,
            ),
            input_hash,
        )
    }

    fn shadowmask_key(
        selection: &EntityShadowLightsSection,
        shared: &SharedAtlas<'_>,
        layer_input_hashes: &[[u8; 32]],
    ) -> CacheKey {
        let section_hash = shadowmask_atlas_input_hash(
            selection,
            layer_input_hashes,
            shared.atlas_width,
            shared.atlas_height,
            layer_count_from_shared(shared),
        );
        CacheKey::new(
            SHADOWMASK_ATLAS_STAGE_ID,
            SHADOWMASK_ATLAS_STAGE_VERSION,
            &section_hash,
        )
    }

    fn shadowmask_section(
        width: u32,
        height: u32,
        layer_count: u32,
        channels: Vec<u8>,
        value: u8,
    ) -> ShadowmaskAtlasSection {
        ShadowmaskAtlasSection {
            width,
            height,
            layer_count,
            channels,
            data: vec![value; (width * height * layer_count * 4) as usize],
        }
    }

    enum BadCachedSection {
        Dimensions,
        LayerCount,
        ChannelCount,
    }

    fn bad_cached_section(
        shared: &SharedAtlas<'_>,
        kind: BadCachedSection,
    ) -> ShadowmaskAtlasSection {
        match kind {
            BadCachedSection::Dimensions => shadowmask_section(
                shared.atlas_width + 1,
                shared.atlas_height,
                layer_count_from_shared(shared),
                vec![0],
                0,
            ),
            BadCachedSection::LayerCount => shadowmask_section(
                shared.atlas_width,
                shared.atlas_height,
                layer_count_from_shared(shared) + 1,
                vec![0],
                0,
            ),
            BadCachedSection::ChannelCount => shadowmask_section(
                shared.atlas_width,
                shared.atlas_height,
                layer_count_from_shared(shared),
                vec![0, 1],
                0,
            ),
        }
    }

    fn assert_cached_section_rebuilt_from_seeded_layer(label: &str, kind: BadCachedSection) {
        let mut test_light = light(5.0);
        test_light.origin = DVec3::new(0.25, 1.0, 0.25);
        let lights = vec![test_light];
        let (geo, prepared, bvh, primitives) = cache_fixture(&lights);
        let shared = shared_from_prepared(&prepared);
        let alpha_lights = AlphaLightsNs::from_lights(&lights);
        let selection = EntityShadowLightsSection {
            light_indices: vec![0],
        };
        let (layer_key, input_hash) =
            layer_key(&lights[0], &shared, &primitives, &geo, AREA_SAMPLES);
        let section_key = shadowmask_key(&selection, &shared, &[input_hash]);
        let selected = selected_refs(&selection, &alpha_lights);
        let seeded_layer = layer(
            shared.atlas_width,
            shared.atlas_height,
            layer_count_from_shared(&shared),
            &[(0, 0, 0.25)],
        );
        let expected = bake_shadowmask_atlas_from_layers(
            &selection,
            shared.atlas_width,
            shared.atlas_height,
            layer_count_from_shared(&shared),
            &selected,
            std::slice::from_ref(&seeded_layer),
        )
        .unwrap();

        let dir = fresh_cache_dir(label);
        let cache = StageCache::new(&dir).expect("cache dir");
        cache.put(&section_key, &bad_cached_section(&shared, kind).to_bytes());
        cache.put(&layer_key, &seeded_layer.to_bytes());

        let result = bake_shadowmask_atlas_cached(
            Some(&selection),
            &alpha_lights,
            &shared,
            &bvh,
            &primitives,
            &geo,
            DENSITY,
            AREA_SAMPLES,
            Some(&cache),
            &test_control(),
        )
        .expect("rebuilt section");

        assert_eq!(result, expected);
        let overwritten = cache
            .get(&section_key)
            .expect("mismatched section entry overwritten");
        assert_eq!(
            ShadowmaskAtlasSection::from_bytes(&overwritten).unwrap(),
            expected
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    enum BadCachedLayer {
        Metadata,
        TexelBounds,
    }

    fn bad_cached_layer(shared: &SharedAtlas<'_>, kind: BadCachedLayer) -> LightmapLayer {
        match kind {
            BadCachedLayer::Metadata => layer(
                shared.atlas_width + 1,
                shared.atlas_height,
                layer_count_from_shared(shared) + 1,
                &[(0, 0, 0.25)],
            ),
            BadCachedLayer::TexelBounds => layer(
                shared.atlas_width,
                shared.atlas_height,
                layer_count_from_shared(shared),
                &[
                    (shared.atlas_width * shared.atlas_height, 0, 0.25),
                    (0, layer_count_from_shared(shared), 0.5),
                ],
            ),
        }
    }

    fn assert_cached_layer_rejected_and_rebaked(label: &str, kind: BadCachedLayer) {
        let mut test_light = light(5.0);
        test_light.origin = DVec3::new(0.25, 1.0, 0.25);
        let lights = vec![test_light];
        let (geo, prepared, bvh, primitives) = cache_fixture(&lights);
        let shared = shared_from_prepared(&prepared);
        let alpha_lights = AlphaLightsNs::from_lights(&lights);
        let selection = EntityShadowLightsSection {
            light_indices: vec![0],
        };
        let (layer_key, input_hash) =
            layer_key(&lights[0], &shared, &primitives, &geo, AREA_SAMPLES);
        let section_key = shadowmask_key(&selection, &shared, &[input_hash]);
        let invalid = bad_cached_layer(&shared, kind);

        let dir = fresh_cache_dir(label);
        let cache = StageCache::new(&dir).expect("cache dir");
        cache.put(&layer_key, &invalid.to_bytes());

        let result = bake_shadowmask_atlas_cached(
            Some(&selection),
            &alpha_lights,
            &shared,
            &bvh,
            &primitives,
            &geo,
            DENSITY,
            AREA_SAMPLES,
            Some(&cache),
            &test_control(),
        )
        .expect("rebuilt section");

        let stored_layer = cache
            .get(&layer_key)
            .and_then(|bytes| LightmapLayer::from_bytes(&bytes))
            .expect("invalid layer entry overwritten by rebake");
        assert_ne!(
            stored_layer, invalid,
            "invalid decodable lightmap_layer payload must not be reused"
        );
        validate_cached_lightmap_layer(&stored_layer, &shared, layer_count_from_shared(&shared))
            .expect("rebaked layer matches current atlas");

        let stored_section = cache.get(&section_key).expect("shadowmask section stored");
        assert_eq!(
            ShadowmaskAtlasSection::from_bytes(&stored_section).unwrap(),
            result
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    fn selected_refs<'a>(
        selection: &EntityShadowLightsSection,
        alpha_lights: &'a AlphaLightsNs<'a>,
    ) -> Vec<(usize, u32, &'a MapLight)> {
        selection
            .light_indices
            .iter()
            .enumerate()
            .filter_map(|(selection_index, &alpha_index)| {
                alpha_lights
                    .entries()
                    .get(alpha_index as usize)
                    .map(|entry| (selection_index, alpha_index, entry.light))
            })
            .collect()
    }

    #[test]
    fn single_light_mask_writes_raw_visibility_to_assigned_channel() {
        let lights = [(0, light(5.0))];
        let selected: Vec<(usize, u32, &MapLight)> = lights
            .iter()
            .enumerate()
            .map(|(selection_index, (i, l))| (selection_index, *i, l))
            .collect();
        let layers = vec![layer(2, 1, 1, &[(0, 0, 0.25), (1, 0, 1.0)])];

        let section = build_shadowmask_from_layers(2, 1, 1, 1, &selected, &layers);

        assert_eq!(section.channels, vec![0]);
        assert_eq!(section.data[0], 64);
        assert_eq!(section.data[4], 255);
    }

    #[test]
    fn overlapping_lights_use_different_channels() {
        let lights = [(0, light(5.0)), (1, light(4.0))];
        let selected: Vec<(usize, u32, &MapLight)> = lights
            .iter()
            .enumerate()
            .map(|(selection_index, (i, l))| (selection_index, *i, l))
            .collect();
        let layers = vec![
            layer(1, 1, 1, &[(0, 0, 0.25)]),
            layer(1, 1, 1, &[(0, 0, 0.5)]),
        ];

        let section = build_shadowmask_from_layers(1, 1, 1, 2, &selected, &layers);

        assert_ne!(section.channels[0], section.channels[1]);
        assert_eq!(section.data[section.channels[0] as usize], 64);
        assert_eq!(section.data[section.channels[1] as usize], 128);
    }

    #[test]
    fn five_way_overlap_drops_lowest_intensity_globally() {
        let lights = [
            (0, light(5.0)),
            (1, light(4.0)),
            (2, light(1.0)),
            (3, light(3.0)),
            (4, light(2.0)),
        ];
        let selected: Vec<(usize, u32, &MapLight)> = lights
            .iter()
            .enumerate()
            .map(|(selection_index, (i, l))| (selection_index, *i, l))
            .collect();
        let layers: Vec<LightmapLayer> = (0..5).map(|_| layer(1, 1, 1, &[(0, 0, 1.0)])).collect();

        let section = build_shadowmask_from_layers(1, 1, 1, 5, &selected, &layers);

        assert_eq!(section.channels[2], SHADOWMASK_CHANNEL_DROPPED);
        assert_eq!(
            section
                .channels
                .iter()
                .filter(|&&c| c != SHADOWMASK_CHANNEL_DROPPED)
                .count(),
            4
        );
    }

    #[test]
    fn multi_layer_payload_uses_layer_major_texel_indexing() {
        let lights = [(0, light(5.0))];
        let selected: Vec<(usize, u32, &MapLight)> = lights
            .iter()
            .enumerate()
            .map(|(selection_index, (i, l))| (selection_index, *i, l))
            .collect();
        let layers = vec![layer(1, 1, 2, &[(0, 1, 0.5)])];

        let section = build_shadowmask_from_layers(1, 1, 2, 1, &selected, &layers);

        assert_eq!(section.data.len(), 8);
        assert_eq!(section.data[0], 255);
        assert_eq!(section.data[4], 128);
    }

    // Regression: an invalid earlier AlphaLights selection compacted the valid
    // light left, so the channel table no longer matched selection indices.
    #[test]
    fn invalid_selected_alpha_light_preserves_original_channel_slot() {
        let valid_light = light(5.0);
        let selected = vec![(1usize, 0u32, &valid_light)];
        let selection = EntityShadowLightsSection {
            light_indices: vec![99, 0],
        };
        let layers = vec![layer(1, 1, 1, &[(0, 0, 0.25)])];

        let section =
            bake_shadowmask_atlas_from_layers(&selection, 1, 1, 1, &selected, &layers).unwrap();

        assert_eq!(section.channels[0], SHADOWMASK_CHANNEL_DROPPED);
        assert_ne!(section.channels[1], SHADOWMASK_CHANNEL_DROPPED);
        assert_eq!(section.data[section.channels[1] as usize], 64);
    }

    #[test]
    fn preloaded_layer_bake_matches_uncached_shadowmask_section_bytes() {
        let mut geo = quad_geometry();
        let mut light_a = light(5.0);
        light_a.origin = DVec3::new(0.25, 1.0, 0.25);
        let mut light_b = light(3.0);
        light_b.origin = DVec3::new(0.75, 1.0, 0.75);
        let lights = vec![light_a, light_b];
        let static_lights = StaticBakedLights::from_lights(&lights);
        let prepared = prepare_atlas(&mut geo, &static_lights, 0.25).unwrap();
        let (bvh, primitives, _) = build_bvh(&geo).unwrap();
        let shared = SharedAtlas {
            charts: &prepared.charts,
            placements: &prepared.placements,
            atlas_width: prepared.atlas_width,
            atlas_height: prepared.atlas_height,
        };
        let alpha_lights = AlphaLightsNs::from_lights(&lights);
        let selection = EntityShadowLightsSection {
            light_indices: vec![0, 1],
        };

        let uncached = bake_shadowmask_atlas(
            Some(&selection),
            &alpha_lights,
            &shared,
            &bvh,
            &primitives,
            &geo,
            4,
            &test_control(),
        )
        .unwrap();

        let selected: Vec<(usize, u32, &MapLight)> = selection
            .light_indices
            .iter()
            .enumerate()
            .filter_map(|(selection_index, &alpha_index)| {
                alpha_lights
                    .entries()
                    .get(alpha_index as usize)
                    .map(|entry| (selection_index, alpha_index, entry.light))
            })
            .collect();
        let layers: Vec<LightmapLayer> = selected
            .iter()
            .map(|(_, _, light)| {
                bake_light_layer(light, &shared, &bvh, &primitives, &geo, 4, &test_control())
            })
            .collect();

        let preloaded = bake_shadowmask_atlas_from_layers(
            &selection,
            shared.atlas_width,
            shared.atlas_height,
            layer_count_from_shared(&shared),
            &selected,
            &layers,
        )
        .unwrap();

        assert_eq!(
            preloaded.to_bytes(),
            uncached.to_bytes(),
            "cache-facing preloaded layer path must preserve uncached section bytes"
        );
    }

    #[test]
    fn shadowmask_atlas_cache_hit_returns_section_without_layer_entries() {
        let mut test_light = light(5.0);
        test_light.origin = DVec3::new(0.25, 1.0, 0.25);
        let lights = vec![test_light];
        let (geo, prepared, bvh, primitives) = cache_fixture(&lights);
        let shared = shared_from_prepared(&prepared);
        let alpha_lights = AlphaLightsNs::from_lights(&lights);
        let selection = EntityShadowLightsSection {
            light_indices: vec![0],
        };
        let (_, input_hash) = layer_key(&lights[0], &shared, &primitives, &geo, AREA_SAMPLES);
        let section_key = shadowmask_key(&selection, &shared, &[input_hash]);
        let cached = ShadowmaskAtlasSection {
            width: shared.atlas_width,
            height: shared.atlas_height,
            layer_count: layer_count_from_shared(&shared),
            channels: vec![3],
            data: vec![
                0;
                (shared.atlas_width * shared.atlas_height * layer_count_from_shared(&shared) * 4)
                    as usize
            ],
        };

        let dir = fresh_cache_dir("whole_section_hit");
        let cache = StageCache::new(&dir).expect("cache dir");
        cache.put(&section_key, &cached.to_bytes());

        let result = bake_shadowmask_atlas_cached(
            Some(&selection),
            &alpha_lights,
            &shared,
            &bvh,
            &primitives,
            &geo,
            DENSITY,
            AREA_SAMPLES,
            Some(&cache),
            &test_control(),
        )
        .expect("cached section");

        assert_eq!(
            result, cached,
            "whole-section hit must return cached bytes without requiring lightmap_layer entries"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn shadowmask_atlas_cache_hit_with_wrong_dimensions_is_rebuilt() {
        assert_cached_section_rebuilt_from_seeded_layer(
            "wrong_section_dimensions",
            BadCachedSection::Dimensions,
        );
    }

    #[test]
    fn shadowmask_atlas_cache_hit_with_wrong_layer_count_is_rebuilt() {
        assert_cached_section_rebuilt_from_seeded_layer(
            "wrong_section_layer_count",
            BadCachedSection::LayerCount,
        );
    }

    #[test]
    fn shadowmask_atlas_cache_hit_with_wrong_channel_count_is_rebuilt() {
        assert_cached_section_rebuilt_from_seeded_layer(
            "wrong_section_channel_count",
            BadCachedSection::ChannelCount,
        );
    }

    #[test]
    fn shadowmask_atlas_cache_miss_reuses_existing_layers_and_stores_section() {
        let mut test_light = light(5.0);
        test_light.origin = DVec3::new(0.25, 1.0, 0.25);
        let lights = vec![test_light];
        let (geo, prepared, bvh, primitives) = cache_fixture(&lights);
        let shared = shared_from_prepared(&prepared);
        let alpha_lights = AlphaLightsNs::from_lights(&lights);
        let selection = EntityShadowLightsSection {
            light_indices: vec![0],
        };
        let (layer_key, input_hash) =
            layer_key(&lights[0], &shared, &primitives, &geo, AREA_SAMPLES);
        let section_key = shadowmask_key(&selection, &shared, &[input_hash]);
        let selected = selected_refs(&selection, &alpha_lights);
        let seeded_layer = layer(
            shared.atlas_width,
            shared.atlas_height,
            layer_count_from_shared(&shared),
            &[(0, 0, 0.25)],
        );
        let expected = bake_shadowmask_atlas_from_layers(
            &selection,
            shared.atlas_width,
            shared.atlas_height,
            layer_count_from_shared(&shared),
            &selected,
            std::slice::from_ref(&seeded_layer),
        )
        .unwrap();

        let dir = fresh_cache_dir("section_miss_reuse_layer");
        let cache = StageCache::new(&dir).expect("cache dir");
        cache.put(&layer_key, &seeded_layer.to_bytes());

        let result = bake_shadowmask_atlas_cached(
            Some(&selection),
            &alpha_lights,
            &shared,
            &bvh,
            &primitives,
            &geo,
            DENSITY,
            AREA_SAMPLES,
            Some(&cache),
            &test_control(),
        )
        .expect("rebuilt section");

        assert_eq!(
            result, expected,
            "section miss must build from the existing lightmap_layer payload"
        );
        let stored = cache.get(&section_key).expect("shadowmask section stored");
        assert_eq!(
            ShadowmaskAtlasSection::from_bytes(&stored).unwrap(),
            expected
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn shadowmask_atlas_layer_cache_rejects_wrong_atlas_metadata() {
        assert_cached_layer_rejected_and_rebaked("wrong_layer_metadata", BadCachedLayer::Metadata);
    }

    #[test]
    fn shadowmask_atlas_layer_cache_rejects_out_of_bounds_texels() {
        assert_cached_layer_rejected_and_rebaked(
            "out_of_bounds_layer_texel",
            BadCachedLayer::TexelBounds,
        );
    }

    #[test]
    fn shadowmask_atlas_cache_miss_bakes_and_stores_missing_layer() {
        let mut test_light = light(5.0);
        test_light.origin = DVec3::new(0.25, 1.0, 0.25);
        let lights = vec![test_light];
        let (geo, prepared, bvh, primitives) = cache_fixture(&lights);
        let shared = shared_from_prepared(&prepared);
        let alpha_lights = AlphaLightsNs::from_lights(&lights);
        let selection = EntityShadowLightsSection {
            light_indices: vec![0],
        };
        let (layer_key, input_hash) =
            layer_key(&lights[0], &shared, &primitives, &geo, AREA_SAMPLES);
        let section_key = shadowmask_key(&selection, &shared, &[input_hash]);

        let dir = fresh_cache_dir("missing_layer_store");
        let cache = StageCache::new(&dir).expect("cache dir");

        let result = bake_shadowmask_atlas_cached(
            Some(&selection),
            &alpha_lights,
            &shared,
            &bvh,
            &primitives,
            &geo,
            DENSITY,
            AREA_SAMPLES,
            Some(&cache),
            &test_control(),
        )
        .expect("rebuilt section");

        assert!(
            cache
                .get(&layer_key)
                .and_then(|bytes| LightmapLayer::from_bytes(&bytes))
                .is_some(),
            "missing selected lightmap_layer must be baked and stored"
        );
        let stored = cache.get(&section_key).expect("shadowmask section stored");
        assert_eq!(ShadowmaskAtlasSection::from_bytes(&stored).unwrap(), result);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn corrupt_shadowmask_atlas_cache_entry_is_overwritten_from_layers() {
        let mut test_light = light(5.0);
        test_light.origin = DVec3::new(0.25, 1.0, 0.25);
        let lights = vec![test_light];
        let (geo, prepared, bvh, primitives) = cache_fixture(&lights);
        let shared = shared_from_prepared(&prepared);
        let alpha_lights = AlphaLightsNs::from_lights(&lights);
        let selection = EntityShadowLightsSection {
            light_indices: vec![0],
        };
        let (layer_key, input_hash) =
            layer_key(&lights[0], &shared, &primitives, &geo, AREA_SAMPLES);
        let section_key = shadowmask_key(&selection, &shared, &[input_hash]);
        let selected = selected_refs(&selection, &alpha_lights);
        let seeded_layer = layer(
            shared.atlas_width,
            shared.atlas_height,
            layer_count_from_shared(&shared),
            &[(0, 0, 0.75)],
        );
        let expected = bake_shadowmask_atlas_from_layers(
            &selection,
            shared.atlas_width,
            shared.atlas_height,
            layer_count_from_shared(&shared),
            &selected,
            std::slice::from_ref(&seeded_layer),
        )
        .unwrap();

        let dir = fresh_cache_dir("corrupt_section");
        let cache = StageCache::new(&dir).expect("cache dir");
        cache.put(&section_key, b"not a shadowmask section");
        cache.put(&layer_key, &seeded_layer.to_bytes());

        let result = bake_shadowmask_atlas_cached(
            Some(&selection),
            &alpha_lights,
            &shared,
            &bvh,
            &primitives,
            &geo,
            DENSITY,
            AREA_SAMPLES,
            Some(&cache),
            &test_control(),
        )
        .expect("rebuilt section");

        assert_eq!(result, expected);
        let overwritten = cache
            .get(&section_key)
            .expect("corrupt section entry overwritten");
        assert_eq!(
            ShadowmaskAtlasSection::from_bytes(&overwritten).unwrap(),
            expected
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn shadowmask_atlas_no_cache_ignores_and_does_not_overwrite_entry() {
        let mut test_light = light(5.0);
        test_light.origin = DVec3::new(0.25, 1.0, 0.25);
        let lights = vec![test_light];
        let (geo, prepared, bvh, primitives) = cache_fixture(&lights);
        let shared = shared_from_prepared(&prepared);
        let alpha_lights = AlphaLightsNs::from_lights(&lights);
        let selection = EntityShadowLightsSection {
            light_indices: vec![0],
        };
        let (_, input_hash) = layer_key(&lights[0], &shared, &primitives, &geo, AREA_SAMPLES);
        let section_key = shadowmask_key(&selection, &shared, &[input_hash]);
        let cached = ShadowmaskAtlasSection {
            width: shared.atlas_width,
            height: shared.atlas_height,
            layer_count: layer_count_from_shared(&shared),
            channels: vec![3],
            data: vec![
                0;
                (shared.atlas_width * shared.atlas_height * layer_count_from_shared(&shared) * 4)
                    as usize
            ],
        };

        let dir = fresh_cache_dir("no_cache");
        let cache = StageCache::new(&dir).expect("cache dir");
        cache.put(&section_key, &cached.to_bytes());

        let result = bake_shadowmask_atlas_cached(
            Some(&selection),
            &alpha_lights,
            &shared,
            &bvh,
            &primitives,
            &geo,
            DENSITY,
            AREA_SAMPLES,
            None,
            &test_control(),
        )
        .expect("uncached section");
        let uncached = bake_shadowmask_atlas(
            Some(&selection),
            &alpha_lights,
            &shared,
            &bvh,
            &primitives,
            &geo,
            AREA_SAMPLES,
            &test_control(),
        )
        .expect("direct uncached section");

        assert_eq!(
            result, uncached,
            "stage_cache == None must delegate to the current recompute path"
        );
        assert_ne!(
            result, cached,
            "stage_cache == None must ignore an existing shadowmask_atlas entry"
        );
        let still_cached = cache.get(&section_key).expect("seeded cache entry remains");
        assert_eq!(
            ShadowmaskAtlasSection::from_bytes(&still_cached).unwrap(),
            cached,
            "stage_cache == None must not overwrite shadowmask_atlas entries"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn shadowmask_atlas_input_hash_is_membership_and_order_sensitive() {
        let selection = EntityShadowLightsSection {
            light_indices: vec![1, 2],
        };
        let hashes = vec![fake_layer_hash(10), fake_layer_hash(20)];
        let base = shadowmask_atlas_input_hash(&selection, &hashes, 4, 8, 1);

        let reordered_selection = EntityShadowLightsSection {
            light_indices: vec![2, 1],
        };
        let reordered_hashes = vec![fake_layer_hash(20), fake_layer_hash(10)];
        assert_ne!(
            base,
            shadowmask_atlas_input_hash(&reordered_selection, &reordered_hashes, 4, 8, 1),
            "selection order must affect the section cache key"
        );

        let changed_membership = EntityShadowLightsSection {
            light_indices: vec![1, 3],
        };
        assert_ne!(
            base,
            shadowmask_atlas_input_hash(&changed_membership, &hashes, 4, 8, 1),
            "selection membership must affect the section cache key"
        );
    }

    #[test]
    fn shadowmask_atlas_input_hash_includes_atlas_dimensions_and_layer_count() {
        let selection = EntityShadowLightsSection {
            light_indices: vec![0],
        };
        let hashes = vec![fake_layer_hash(7)];
        let base = shadowmask_atlas_input_hash(&selection, &hashes, 4, 8, 1);

        assert_ne!(
            base,
            shadowmask_atlas_input_hash(&selection, &hashes, 5, 8, 1)
        );
        assert_ne!(
            base,
            shadowmask_atlas_input_hash(&selection, &hashes, 4, 9, 1)
        );
        assert_ne!(
            base,
            shadowmask_atlas_input_hash(&selection, &hashes, 4, 8, 2)
        );
    }

    #[test]
    fn shadowmask_atlas_input_hash_includes_selected_layer_hashes() {
        let selection = EntityShadowLightsSection {
            light_indices: vec![0, 1],
        };
        let base_hashes = vec![fake_layer_hash(1), fake_layer_hash(2)];
        let changed_hashes = vec![fake_layer_hash(1), fake_layer_hash(3)];

        assert_ne!(
            shadowmask_atlas_input_hash(&selection, &base_hashes, 4, 8, 1),
            shadowmask_atlas_input_hash(&selection, &changed_hashes, 4, 8, 1),
            "changing a selected lightmap layer input hash must affect the section cache key"
        );
    }

    #[test]
    fn soft_shadow_samples_change_selected_layer_and_shadowmask_keys() {
        let mut test_light = light(5.0);
        test_light.origin = DVec3::new(0.25, 1.0, 0.25);
        let lights = vec![test_light];
        let (geo, prepared, _, primitives) = cache_fixture(&lights);
        let shared = shared_from_prepared(&prepared);
        let selection = EntityShadowLightsSection {
            light_indices: vec![0],
        };

        let layer_hash_4 =
            lightmap_layer::layer_input_hash(&lights[0], &shared, &primitives, &geo, DENSITY, 4);
        let layer_hash_8 =
            lightmap_layer::layer_input_hash(&lights[0], &shared, &primitives, &geo, DENSITY, 8);
        assert_ne!(
            layer_hash_4, layer_hash_8,
            "--soft-shadow-samples must affect the selected layer input hash"
        );

        assert_ne!(
            shadowmask_atlas_input_hash(
                &selection,
                &[layer_hash_4],
                shared.atlas_width,
                shared.atlas_height,
                layer_count_from_shared(&shared),
            ),
            shadowmask_atlas_input_hash(
                &selection,
                &[layer_hash_8],
                shared.atlas_width,
                shared.atlas_height,
                layer_count_from_shared(&shared),
            ),
            "--soft-shadow-samples must flow through to the shadowmask_atlas key"
        );
    }
}
