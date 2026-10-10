// Standalone shadowmask bake entry points and resident-layer accounting.
// Tests use them as the fused walk's reference.
// See: context/lib/build_pipeline.md §PRL (ShadowmaskAtlas)

use super::*;

/// Test-only instrumentation counts every full-layer-equivalent payload:
/// cached or assembled layers and each cold light's aggregate raw chart output.
#[cfg(test)]
#[derive(Default)]
pub(super) struct ResidentLayerTracker {
    current: std::sync::atomic::AtomicUsize,
    high_water: std::sync::atomic::AtomicUsize,
}

#[cfg(not(test))]
#[derive(Default)]
pub(super) struct ResidentLayerTracker;

pub(super) struct ResidentLayerGuard<'a> {
    #[cfg(test)]
    tracker: &'a ResidentLayerTracker,
    #[cfg(not(test))]
    _tracker: std::marker::PhantomData<&'a ResidentLayerTracker>,
}

impl ResidentLayerTracker {
    pub(super) fn new() -> Self {
        #[cfg(test)]
        {
            Self::default()
        }
        #[cfg(not(test))]
        {
            Self
        }
    }

    pub(super) fn acquire(&self) -> ResidentLayerGuard<'_> {
        #[cfg(test)]
        {
            use std::sync::atomic::Ordering;

            let current = self.current.fetch_add(1, Ordering::Relaxed) + 1;
            self.high_water.fetch_max(current, Ordering::Relaxed);
            ResidentLayerGuard { tracker: self }
        }
        #[cfg(not(test))]
        {
            ResidentLayerGuard {
                _tracker: std::marker::PhantomData,
            }
        }
    }

    #[cfg(test)]
    pub(super) fn high_water(&self) -> usize {
        use std::sync::atomic::Ordering;

        self.high_water.load(Ordering::Relaxed)
    }
}

impl Drop for ResidentLayerGuard<'_> {
    fn drop(&mut self) {
        #[cfg(test)]
        {
            use std::sync::atomic::Ordering;

            let previous = self.tracker.current.fetch_sub(1, Ordering::Relaxed);
            assert!(
                previous > 0,
                "resident shadowmask layer count must not underflow"
            );
        }
    }
}

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
    let resident_layers = ResidentLayerTracker::new();
    bake_shadowmask_atlas_with_window(
        selection,
        alpha_lights,
        shared,
        bvh,
        primitives,
        geometry,
        area_sample_count,
        control,
        SHADOWMASK_RESIDENT_LAYER_WINDOW,
        &resident_layers,
    )
}

#[allow(clippy::too_many_arguments)]
pub(super) fn bake_shadowmask_atlas_with_window(
    selection: Option<&EntityShadowLightsSection>,
    alpha_lights: &AlphaLightsNs<'_>,
    shared: &SharedAtlas<'_>,
    bvh: &bvh::bvh::Bvh<f32, 3>,
    primitives: &[BvhPrimitive],
    geometry: &GeometryResult,
    area_sample_count: u32,
    control: &BakeControl,
    resident_layer_window: usize,
    resident_layers: &ResidentLayerTracker,
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

    publish_shadowmask_total(control, selected.len(), shared);
    let graph = build_analytic_overlap_graph(&selected, shared, geometry, control);
    let mut fill = ShadowmaskFill::new(
        shared.atlas_width,
        shared.atlas_height,
        layer_count_from_shared(shared),
        &shared.layout.blocks,
        selection.light_indices.len(),
        &selected,
        &graph,
        Some(control),
        Some(resident_layers),
    );
    fill_uncached_shadowmask_partitions(
        &mut fill,
        &selected,
        shared,
        bvh,
        primitives,
        geometry,
        area_sample_count,
        control,
        resident_layer_window,
        resident_layers,
    );
    let section = fill.finish();
    // The first light's fill unit is held until all atlas finalization has
    // completed, so the stage cannot display 100% while still working.
    control.advance(1);
    Some(section)
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
    let resident_layers = ResidentLayerTracker::new();
    bake_shadowmask_atlas_cached_with_window(
        selection,
        alpha_lights,
        shared,
        bvh,
        primitives,
        geometry,
        lightmap_density,
        area_sample_count,
        stage_cache,
        control,
        SHADOWMASK_RESIDENT_LAYER_WINDOW,
        &resident_layers,
    )
}

#[allow(clippy::too_many_arguments)]
pub(super) fn bake_shadowmask_atlas_cached_with_window(
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
    resident_layer_window: usize,
    resident_layers: &ResidentLayerTracker,
) -> Option<ShadowmaskAtlasSection> {
    let Some(cache) = stage_cache else {
        return bake_shadowmask_atlas_with_window(
            selection,
            alpha_lights,
            shared,
            bvh,
            primitives,
            geometry,
            area_sample_count,
            control,
            resident_layer_window,
            resident_layers,
        );
    };

    let selection = selection?;
    if selection.light_indices.is_empty() {
        return None;
    }

    let layer_count = layer_count_from_shared(shared);
    let mut selected = Vec::with_capacity(selection.light_indices.len());
    let mut selected_layer_input_hashes = Vec::with_capacity(selection.light_indices.len());
    let mut layer_input_hashes =
        Vec::with_capacity(selection.light_indices.len() * layer_count as usize);
    for (selection_index, &alpha_index) in selection.light_indices.iter().enumerate() {
        let Some(entry) = alpha_lights.entries().get(alpha_index as usize) else {
            log::warn!(
                "[ShadowmaskAtlas] selected AlphaLights index {alpha_index} is out of range; marking dropped"
            );
            for target_layer in 0..layer_count {
                layer_input_hashes.push(invalid_selected_light_hash(alpha_index, target_layer));
            }
            continue;
        };

        let hashes: Vec<[u8; 32]> = (0..layer_count)
            .map(|target_layer| {
                lightmap_layer::layer_input_hash(
                    entry.light,
                    shared,
                    primitives,
                    geometry,
                    lightmap_density,
                    area_sample_count,
                    target_layer,
                )
            })
            .collect();
        layer_input_hashes.extend_from_slice(&hashes);
        selected.push((selection_index, alpha_index, entry.light));
        selected_layer_input_hashes.push(hashes);
    }

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

    publish_shadowmask_total(control, selected.len(), shared);
    if let Some(memo) = read_shadowmask_memo(cache, &section_key, selection, shared) {
        log::info!("[cache] shadowmask_atlas hit");
        advance_shadowmask_total(control, selected.len(), shared);
        return Some(memo.section);
    }

    log::info!("[cache] shadowmask_atlas miss");
    if selected.is_empty() {
        let section = empty_section_for_selection(shared, selection.light_indices.len());
        cache_shadowmask_section_then_complete(
            cache,
            &section_key,
            &section,
            0,
            control,
            false,
            || {},
        );
        return Some(section);
    }
    let graph = build_analytic_overlap_graph(&selected, shared, geometry, control);
    let mut fill = ShadowmaskFill::new(
        shared.atlas_width,
        shared.atlas_height,
        layer_count,
        &shared.layout.blocks,
        selection.light_indices.len(),
        &selected,
        &graph,
        Some(control),
        Some(resident_layers),
    );
    fill_cached_shadowmask_partitions(
        &mut fill,
        &selected,
        shared,
        bvh,
        primitives,
        geometry,
        area_sample_count,
        cache,
        &selected_layer_input_hashes,
        control,
    );
    let section = fill.finish();
    cache_shadowmask_section_then_complete(
        cache,
        &section_key,
        &section,
        graph.peak_texel_overlap(),
        control,
        !selected.is_empty(),
        || {},
    );
    Some(section)
}

#[cfg(test)]
#[allow(clippy::too_many_arguments)]
pub(super) fn bake_shadowmask_atlas_with_test_window(
    selection: Option<&EntityShadowLightsSection>,
    alpha_lights: &AlphaLightsNs<'_>,
    shared: &SharedAtlas<'_>,
    bvh: &bvh::bvh::Bvh<f32, 3>,
    primitives: &[BvhPrimitive],
    geometry: &GeometryResult,
    area_sample_count: u32,
    control: &BakeControl,
    resident_layer_window: usize,
) -> (Option<ShadowmaskAtlasSection>, usize) {
    let resident_layers = ResidentLayerTracker::default();
    let section = bake_shadowmask_atlas_with_window(
        selection,
        alpha_lights,
        shared,
        bvh,
        primitives,
        geometry,
        area_sample_count,
        control,
        resident_layer_window,
        &resident_layers,
    );
    (section, resident_layers.high_water())
}

#[cfg(test)]
#[allow(clippy::too_many_arguments)]
pub(super) fn bake_shadowmask_atlas_cached_with_test_window(
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
    resident_layer_window: usize,
) -> (Option<ShadowmaskAtlasSection>, usize) {
    let resident_layers = ResidentLayerTracker::default();
    let section = bake_shadowmask_atlas_cached_with_window(
        selection,
        alpha_lights,
        shared,
        bvh,
        primitives,
        geometry,
        lightmap_density,
        area_sample_count,
        stage_cache,
        control,
        resident_layer_window,
        &resident_layers,
    );
    (section, resident_layers.high_water())
}

/// The valid selected set becomes known only after `AlphaLights` bounds
/// filtering. Keep zero-work stages indeterminate rather than publishing 0/0.
pub(super) fn publish_shadowmask_total(
    control: &BakeControl,
    valid_selected_light_count: usize,
    shared: &SharedAtlas<'_>,
) {
    let total = shadowmask_progress_total(valid_selected_light_count, shared);
    if total != 0 {
        control.publish_total(total);
    }
}

pub(super) fn shadowmask_progress_total(
    valid_selected_light_count: usize,
    shared: &SharedAtlas<'_>,
) -> usize {
    if valid_selected_light_count == 0 {
        return 0;
    }
    valid_selected_light_count
        .saturating_mul(shared.placements.len())
        .saturating_add(shared.placements.len())
        .saturating_add(1)
        .saturating_add(valid_selected_light_count)
}

/// A whole-section cache hit performs no chart, assignment, or fill work, but
/// still represents the complete stage to the progress handle.
pub(super) fn advance_shadowmask_total(
    control: &BakeControl,
    valid_selected_light_count: usize,
    shared: &SharedAtlas<'_>,
) {
    let total = shadowmask_progress_total(valid_selected_light_count, shared);
    if total != 0 {
        control.governor().checkpoint();
        control.advance(total);
    }
}
