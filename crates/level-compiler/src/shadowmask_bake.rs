// Per-light shadowmask bake for selected static entity-shadow lights.
// Governing context: context/lib/build_pipeline.md

use std::collections::HashMap;
use std::sync::atomic::AtomicU8;

use rayon::prelude::*;

use postretro_level_format::entity_shadow_lights::EntityShadowLightsSection;
use postretro_level_format::shadowmask_atlas::{
    SHADOWMASK_CHANNEL_DROPPED, ShadowmaskAtlasSection,
};

use crate::bake_control::BakeControl;
use crate::bvh_build::BvhPrimitive;
use crate::cache::{CacheKey, StageCache};
use crate::geometry::GeometryResult;
use crate::light_namespaces::AlphaLightsNs;
use crate::lightmap_layer::{self, LayerTexel, LightmapLayer, SharedAtlas};
use crate::map_data::MapLight;

pub const SHADOWMASK_ATLAS_STAGE_ID: &str = "shadowmask_atlas";

/// Bump when the cached `ShadowmaskAtlas` bytes can change without a layer input
/// hash change: channel assignment/drop policy, raw-visibility quantization to
/// `Rgba8Unorm`, empty-section behavior, or `ShadowmaskAtlasSection::to_bytes`
/// payload semantics.
pub const SHADOWMASK_ATLAS_STAGE_VERSION: u32 = 1;

/// Maximum selected-light count in one governed chart batch. Each light's raw
/// chart buffers collectively carry one full layer payload, so the batch must
/// never widen beyond this residency window.
const SHADOWMASK_RESIDENT_LAYER_WINDOW: usize = 4;

/// Fill checks the cooperative pause gate at this cadence without consuming a
/// governor permit; chart work remains the only governed parallel level.
const SHADOWMASK_FILL_CHECKPOINT_TEXELS: usize = 1024;

/// A shadowmask needs only the atlas position, fixed selected-light index, and
/// unquantized visibility from a lightmap layer. Keeping the visibility as `f32`
/// preserves the established single quantization at fill time.
#[derive(Clone, Copy, Debug)]
struct ShadowmaskMembershipTexel {
    global_texel_index: usize,
    compact_light_index: u32,
    raw_visibility: f32,
}

/// Complete compact coverage needed by the global one-channel-per-light
/// assignment. Buckets are always addressed and consumed in compact selection
/// order, never in bake completion order.
#[derive(Debug)]
struct ShadowmaskMembership {
    by_light: Vec<Vec<ShadowmaskMembershipTexel>>,
}

impl ShadowmaskMembership {
    fn for_light_count(light_count: usize) -> Self {
        Self {
            by_light: (0..light_count).map(|_| Vec::new()).collect(),
        }
    }
}

/// Test-only instrumentation counts every full-layer-equivalent payload:
/// cached or assembled layers and each cold light's aggregate raw chart output.
#[cfg(test)]
#[derive(Default)]
struct ResidentLayerTracker {
    current: std::sync::atomic::AtomicUsize,
    high_water: std::sync::atomic::AtomicUsize,
}

#[cfg(not(test))]
#[derive(Default)]
struct ResidentLayerTracker;

struct ResidentLayerGuard<'a> {
    #[cfg(test)]
    tracker: &'a ResidentLayerTracker,
    #[cfg(not(test))]
    _tracker: std::marker::PhantomData<&'a ResidentLayerTracker>,
}

impl ResidentLayerTracker {
    fn new() -> Self {
        #[cfg(test)]
        {
            Self::default()
        }
        #[cfg(not(test))]
        {
            Self
        }
    }

    fn acquire(&self) -> ResidentLayerGuard<'_> {
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
    fn high_water(&self) -> usize {
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
fn bake_shadowmask_atlas_with_window(
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
    let membership = collect_shadowmask_membership_in_batches(
        &selected,
        shared,
        bvh,
        primitives,
        geometry,
        area_sample_count,
        None,
        None,
        control,
        resident_layer_window,
        resident_layers,
    );
    Some(build_shadowmask_from_membership_controlled(
        shared.atlas_width,
        shared.atlas_height,
        layer_count_from_shared(shared) as usize,
        selection.light_indices.len(),
        &selected,
        &membership,
        Some(control),
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
fn bake_shadowmask_atlas_cached_with_window(
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

    let mut selected = Vec::with_capacity(selection.light_indices.len());
    let mut selected_layer_input_hashes = Vec::with_capacity(selection.light_indices.len());
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
        selected_layer_input_hashes.push(input_hash);
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

    publish_shadowmask_total(control, selected.len(), shared);
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
        advance_shadowmask_total(control, selected.len(), shared);
        return Some(section);
    }

    log::info!("[cache] shadowmask_atlas miss");
    let selected_refs: Vec<(usize, u32, &MapLight)> = selected
        .iter()
        .map(|&(selection_index, alpha_index, light, _)| (selection_index, alpha_index, light))
        .collect();
    let membership = collect_shadowmask_membership_in_batches(
        &selected_refs,
        shared,
        bvh,
        primitives,
        geometry,
        area_sample_count,
        Some(cache),
        Some(&selected_layer_input_hashes),
        control,
        resident_layer_window,
        resident_layers,
    );
    let section = Some(build_shadowmask_from_membership_controlled(
        shared.atlas_width,
        shared.atlas_height,
        layer_count as usize,
        selection.light_indices.len(),
        &selected_refs,
        &membership,
        Some(control),
    ));
    if let Some(ref section) = section {
        cache.put(&section_key, &section.to_bytes());
    }
    section
}

#[cfg(test)]
#[allow(clippy::too_many_arguments)]
fn bake_shadowmask_atlas_with_test_window(
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
fn bake_shadowmask_atlas_cached_with_test_window(
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
    let texel_count = texel_plane_len(width, height)
        .checked_mul(layer_count as usize)
        .expect("shadowmask atlas texel count exceeds addressable memory");
    let data_len = texel_count
        .checked_mul(4)
        .expect("shadowmask atlas byte count exceeds addressable memory");
    ShadowmaskAtlasSection {
        width,
        height,
        layer_count,
        channels: vec![SHADOWMASK_CHANNEL_DROPPED; selected_light_count],
        data: vec![255; data_len],
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

/// The valid selected set becomes known only after `AlphaLights` bounds
/// filtering. Keep zero-work stages indeterminate rather than publishing 0/0.
fn publish_shadowmask_total(
    control: &BakeControl,
    valid_selected_light_count: usize,
    shared: &SharedAtlas<'_>,
) {
    let total = valid_selected_light_count.saturating_mul(shared.placements.len());
    if total != 0 {
        control.publish_total(total);
    }
}

/// Cache hits perform no chart work, but still represent complete shadowmask
/// work to the stage progress handle.
fn advance_shadowmask_total(
    control: &BakeControl,
    valid_selected_light_count: usize,
    shared: &SharedAtlas<'_>,
) {
    let total = valid_selected_light_count.saturating_mul(shared.placements.len());
    if total != 0 {
        control.governor().checkpoint();
        control.advance(total);
    }
}

/// Run the only light-axis Rayon level used by the shadowmask bake.
///
/// Each batch contains at most W selected lights. A cold light's chart buffers
/// collectively equal one full layer payload, so widening a batch to the
/// governor cap would also widen residency. The indexed chart level still uses
/// every permitted worker when W times the chart count exposes enough work.
/// Every chart joins and every payload is compacted before the next batch.
#[allow(clippy::too_many_arguments)]
fn collect_shadowmask_membership_in_batches(
    selected: &[(usize, u32, &MapLight)],
    shared: &SharedAtlas<'_>,
    bvh: &bvh::bvh::Bvh<f32, 3>,
    primitives: &[BvhPrimitive],
    geometry: &GeometryResult,
    area_sample_count: u32,
    cache: Option<&StageCache>,
    layer_input_hashes: Option<&[[u8; 32]]>,
    control: &BakeControl,
    resident_layer_window: usize,
    resident_layers: &ResidentLayerTracker,
) -> ShadowmaskMembership {
    assert!(
        resident_layer_window > 0,
        "shadowmask resident-layer window must be at least one"
    );
    debug_assert_eq!(
        cache.is_some(),
        layer_input_hashes.is_some(),
        "cached shadowmask membership needs per-light hashes"
    );
    if let Some(hashes) = layer_input_hashes {
        debug_assert_eq!(
            hashes.len(),
            selected.len(),
            "selected light/hash slices must align after invalid selections are filtered"
        );
    }

    let plane = texel_plane_len(shared.atlas_width, shared.atlas_height);
    let chart_count = shared.placements.len();
    let layer_count = layer_count_from_shared(shared);
    let mut membership = ShadowmaskMembership::for_light_count(selected.len());

    let mut batch_start = 0;
    while batch_start < selected.len() {
        let mut batch_membership: Vec<Option<Vec<ShadowmaskMembershipTexel>>> = Vec::new();
        let mut missing: Vec<(usize, &MapLight, Option<CacheKey>)> = Vec::new();

        while batch_start + batch_membership.len() < selected.len()
            && batch_membership.len() < resident_layer_window
        {
            let batch_light_index = batch_membership.len();
            let compact_light_index = batch_start + batch_light_index;
            let (_, alpha_index, light) = selected[compact_light_index];
            batch_membership.push(None);
            if let (Some(cache), Some(hashes)) = (cache, layer_input_hashes) {
                let layer_key = CacheKey::new(
                    "lightmap_layer",
                    lightmap_layer::LAYER_FORMAT_VERSION,
                    &hashes[compact_light_index],
                );
                let cached_layer = cache
                    .get(&layer_key)
                    .and_then(|bytes| LightmapLayer::from_bytes(&bytes))
                    .and_then(|layer| match validate_cached_lightmap_layer(
                        &layer,
                        shared,
                        layer_count,
                    ) {
                        Ok(()) => Some(layer),
                        Err(reason) => {
                            log::warn!(
                                "[Compiler] corrupt lightmap_layer cache entry for shadowmask selected AlphaLights index {alpha_index} ({reason}), re-baking"
                            );
                            None
                        }
                    });
                if let Some(layer) = cached_layer {
                    let _resident_layer = resident_layers.acquire();
                    log::info!("[cache] lightmap_layer hit");
                    // A warm layer has no chart task, but it is still a full
                    // light's worth of published shadowmask work.
                    control.governor().checkpoint();
                    control.advance(chart_count);
                    batch_membership[batch_light_index] =
                        Some(collect_layer_membership(compact_light_index, &layer, plane));
                } else {
                    log::info!("[cache] lightmap_layer miss");
                    missing.push((batch_light_index, light, Some(layer_key)));
                }
            } else {
                missing.push((batch_light_index, light, None));
            }
        }
        let batch_len = batch_membership.len();

        // This indexed collection is the sole governed parallel level. It is
        // ordered first by compact selected-light index and then by chart index,
        // so joining cannot inherit Rayon completion order.
        let work_items: Vec<(usize, &MapLight, usize)> = missing
            .iter()
            .flat_map(|(batch_light_index, light, _)| {
                (0..chart_count).map(move |chart_index| (*batch_light_index, *light, chart_index))
            })
            .collect();
        // Charge each cold light before any chart buffer starts growing. One
        // guard covers all of that light's per-chart Vecs and is retained while
        // they are assembled, cached, and compacted. This makes the test hook
        // observe the raw payloads that previously escaped its accounting.
        let raw_output_guards: Vec<_> = missing.iter().map(|_| resident_layers.acquire()).collect();
        let chart_outputs: Vec<Vec<LayerTexel>> = work_items
            .par_iter()
            .map(|&(_, light, chart_index)| {
                lightmap_layer::bake_light_layer_chart_controlled(
                    light,
                    shared,
                    chart_index,
                    bvh,
                    primitives,
                    geometry,
                    area_sample_count,
                    control,
                )
            })
            .collect();

        let mut chart_outputs = chart_outputs.into_iter();
        let mut raw_output_guards = raw_output_guards.into_iter();
        for (batch_light_index, _light, layer_key) in missing {
            let _resident_payload = raw_output_guards
                .next()
                .expect("one residency guard is required for each cold light payload");
            let texels = (0..chart_count)
                .flat_map(|_| {
                    chart_outputs
                        .next()
                        .expect("one ordered output is required for every chart task")
                })
                .collect();
            let layer = LightmapLayer {
                atlas_width: shared.atlas_width,
                atlas_height: shared.atlas_height,
                layer_count,
                texels,
            };
            if let (Some(cache), Some(layer_key)) = (cache, layer_key.as_ref()) {
                cache.put(layer_key, &layer.to_bytes());
            }
            let compact_light_index = batch_start + batch_light_index;
            batch_membership[batch_light_index] =
                Some(collect_layer_membership(compact_light_index, &layer, plane));
        }
        debug_assert!(
            chart_outputs.next().is_none(),
            "every chart output must join exactly one selected light"
        );
        debug_assert!(
            raw_output_guards.next().is_none(),
            "every cold light payload must release exactly one residency guard"
        );

        // Batch concatenation follows the fixed compact selection order rather
        // than work completion order. Coloring runs only after every batch has
        // populated this global membership.
        for (batch_light_index, entries) in batch_membership.into_iter().enumerate() {
            membership.by_light[batch_start + batch_light_index] = entries
                .expect("each selected light must provide cached or freshly baked membership");
        }

        batch_start += batch_len;
        // Chart items and full-layer guards have all released their governed
        // permits or residency accounting by this point.
        control.governor().checkpoint();
    }

    // Keep the assignment barrier cooperative too, including the empty-chart
    // case where no governed chart item had a chance to observe a pause.
    control.governor().checkpoint();
    membership
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
    let Some(texel_count) = plane.checked_mul(layer_count as usize) else {
        return Err("atlas layer count overflows addressable texels".to_string());
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
        let Some(global_texel_index) = (texel.layer as usize)
            .checked_mul(plane)
            .and_then(|offset| offset.checked_add(texel.idx as usize))
        else {
            return Err(format!(
                "texel {texel_index} layer-major index exceeds addressable memory"
            ));
        };
        if global_texel_index >= texel_count {
            return Err(format!(
                "texel {texel_index} layer-major index {global_texel_index} out of bounds for {texel_count} texels"
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
    let plane = texel_plane_len(width, height);
    let mut membership = ShadowmaskMembership::for_light_count(layers.len());
    for (compact_light_index, layer) in layers.iter().enumerate() {
        record_layer_membership(&mut membership, compact_light_index, layer, plane);
    }
    build_shadowmask_from_membership(
        width,
        height,
        layer_count,
        selected_light_count,
        selected,
        &membership,
    )
}

fn record_layer_membership(
    membership: &mut ShadowmaskMembership,
    compact_light_index: usize,
    layer: &LightmapLayer,
    plane: usize,
) {
    membership.by_light[compact_light_index] =
        collect_layer_membership(compact_light_index, layer, plane);
}

fn collect_layer_membership(
    compact_light_index: usize,
    layer: &LightmapLayer,
    plane: usize,
) -> Vec<ShadowmaskMembershipTexel> {
    let mut entries = Vec::with_capacity(layer.texels.len());
    for texel in &layer.texels {
        // This is deliberately the legacy skip predicate. NaN is retained
        // because `NaN < 0.0` is false; cache payloads permit NaN.
        if texel.raw_visibility < 0.0 {
            continue;
        }
        let layer_offset = (texel.layer as usize)
            .checked_mul(plane)
            .expect("shadowmask layer texel index exceeds addressable memory");
        let global_texel_index = layer_offset
            .checked_add(texel.idx as usize)
            .expect("shadowmask global texel index exceeds addressable memory");
        entries.push(ShadowmaskMembershipTexel {
            global_texel_index,
            compact_light_index: compact_light_index as u32,
            raw_visibility: texel.raw_visibility,
        });
    }
    entries
}

fn build_shadowmask_from_membership(
    width: u32,
    height: u32,
    layer_count: usize,
    selected_light_count: usize,
    selected: &[(usize, u32, &MapLight)],
    membership: &ShadowmaskMembership,
) -> ShadowmaskAtlasSection {
    build_shadowmask_from_membership_controlled(
        width,
        height,
        layer_count,
        selected_light_count,
        selected,
        membership,
        None,
    )
}

fn build_shadowmask_from_membership_controlled(
    width: u32,
    height: u32,
    layer_count: usize,
    selected_light_count: usize,
    selected: &[(usize, u32, &MapLight)],
    membership: &ShadowmaskMembership,
    control: Option<&BakeControl>,
) -> ShadowmaskAtlasSection {
    build_shadowmask_from_membership_with_assignment_checkpoint(
        width,
        height,
        layer_count,
        selected_light_count,
        selected,
        membership,
        control,
        || {
            if let Some(control) = control {
                control.governor().checkpoint();
            }
        },
    )
}

#[allow(clippy::too_many_arguments)]
fn build_shadowmask_from_membership_with_assignment_checkpoint(
    width: u32,
    height: u32,
    layer_count: usize,
    selected_light_count: usize,
    selected: &[(usize, u32, &MapLight)],
    membership: &ShadowmaskMembership,
    control: Option<&BakeControl>,
    assignment_checkpoint: impl FnOnce(),
) -> ShadowmaskAtlasSection {
    debug_assert_eq!(
        selected.len(),
        membership.by_light.len(),
        "shadowmask selected light/membership slices must align"
    );

    let texel_count = texel_plane_len(width, height)
        .checked_mul(layer_count)
        .expect("shadowmask atlas texel count exceeds addressable memory");
    // No governed work item is active here: every chart batch completed before
    // this serial global assignment barrier.
    assignment_checkpoint();
    let graph = overlap_graph(membership);
    let compact_channels = assign_channels_with_drops(&graph, selected);
    if let Some(control) = control {
        control.governor().checkpoint();
    }
    let mut channels = vec![SHADOWMASK_CHANNEL_DROPPED; selected_light_count];
    for (compact_index, &(selection_index, _, _)) in selected.iter().enumerate() {
        if selection_index < channels.len() {
            channels[selection_index] = compact_channels
                .get(compact_index)
                .copied()
                .unwrap_or(SHADOWMASK_CHANNEL_DROPPED);
        }
    }

    let data_len = texel_count
        .checked_mul(4)
        .expect("shadowmask atlas byte count exceeds addressable memory");
    let data: Vec<AtomicU8> = (0..data_len).map(|_| AtomicU8::new(255)).collect();
    membership
        .by_light
        .par_iter()
        .enumerate()
        .for_each(|(compact_index, mask)| {
            let channel = compact_channels
                .get(compact_index)
                .copied()
                .unwrap_or(SHADOWMASK_CHANNEL_DROPPED);
            if channel == SHADOWMASK_CHANNEL_DROPPED {
                return;
            }
            for chunk in mask.chunks(SHADOWMASK_FILL_CHECKPOINT_TEXELS) {
                // Fill is intentionally ungoverned, but remains cooperative
                // for long masks. It never enters the governor or waits on a
                // governed chart item.
                if let Some(control) = control {
                    control.governor().checkpoint();
                }
                for texel in chunk {
                    debug_assert_eq!(texel.compact_light_index as usize, compact_index);
                    let visibility = (texel.raw_visibility.clamp(0.0, 1.0) * 255.0 + 0.5) as u8;
                    let offset = shadowmask_data_offset(texel.global_texel_index, channel)
                        .expect("shadowmask texel byte offset exceeds addressable memory");
                    // An overlap is a graph edge, so coloring gives those lights
                    // different channels and therefore different byte offsets.
                    // Same-channel lights have no shared texel, making these
                    // parallel stores disjoint by construction.
                    data[offset].store(visibility, std::sync::atomic::Ordering::Relaxed);
                }
            }
        });
    let data = data.into_iter().map(AtomicU8::into_inner).collect();

    ShadowmaskAtlasSection {
        width,
        height,
        layer_count: layer_count as u32,
        channels,
        data,
    }
}

fn texel_plane_len(width: u32, height: u32) -> usize {
    (width as usize)
        .checked_mul(height as usize)
        .expect("shadowmask atlas plane exceeds addressable memory")
}

fn shadowmask_data_offset(global_texel_index: usize, channel: u8) -> Option<usize> {
    global_texel_index
        .checked_mul(4)
        .and_then(|base| base.checked_add(channel as usize))
}

fn overlap_graph(membership: &ShadowmaskMembership) -> Vec<Vec<bool>> {
    let mut graph = vec![vec![false; membership.by_light.len()]; membership.by_light.len()];
    let mut texel_lights: HashMap<usize, Vec<usize>> = HashMap::new();
    for (compact_light_index, entries) in membership.by_light.iter().enumerate() {
        for entry in entries {
            debug_assert_eq!(entry.compact_light_index as usize, compact_light_index);
            texel_lights
                .entry(entry.global_texel_index)
                .or_default()
                .push(compact_light_index);
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
    use crate::chart_raster::ChartPlacement;
    use crate::governor::Governor;
    use crate::light_namespaces::{AlphaLightsNs, StaticBakedLights};
    use crate::lightmap_bake::{Chart, prepare_atlas};
    use crate::lightmap_layer::{LayerTexel, bake_light_layer};
    use crate::map_data::{FalloffModel, LightType, ShadowType};
    use crate::reporter::StageProgress;
    use glam::{DVec3, Vec3};
    use postretro_level_format::geometry::{FaceMeta, GeometrySection, Vertex};
    use postretro_level_format::texture_names::TextureNamesSection;
    use rayon::ThreadPoolBuilder;

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
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::sync::{Arc, Condvar, Mutex, mpsc};
    use std::thread;
    use std::time::Duration;

    const DENSITY: f32 = 0.25;
    const AREA_SAMPLES: u32 = 4;

    const TOP_LEVEL_MULTILAYER_FIVE_WAY_GOLDEN: [u8; 224] = {
        let mut bytes = [255; 224];
        bytes[0] = 5;
        bytes[1] = 0;
        bytes[2] = 0;
        bytes[3] = 0;
        bytes[4] = 5;
        bytes[5] = 0;
        bytes[6] = 0;
        bytes[7] = 0;
        bytes[8] = 2;
        bytes[9] = 0;
        bytes[10] = 0;
        bytes[11] = 0;
        bytes[12] = 5;
        bytes[13] = 0;
        bytes[14] = 0;
        bytes[15] = 0;
        bytes[16] = 0;
        bytes[17] = 1;
        bytes[18] = SHADOWMASK_CHANNEL_DROPPED;
        bytes[19] = 2;
        bytes[20] = 3;
        bytes[21] = 0;
        bytes[22] = 0;
        bytes[23] = 0;
        bytes
    };

    fn test_control() -> BakeControl {
        let progress = StageProgress::indeterminate();
        BakeControl::new(Arc::new(Governor::new(1, false)), &progress)
    }

    fn one_texel_chart() -> Chart {
        Chart {
            origin: Vec3::ZERO,
            u_axis: Vec3::X,
            v_axis: Vec3::Z,
            uv_min: [0.0, 0.0],
            uv_extent: [1.0, 1.0],
            normal: Vec3::Y,
            width_texels: 5,
            height_texels: 5,
            leaf_index: 0,
        }
    }

    fn top_level_multilayer_five_way_inputs() -> (
        GeometryResult,
        bvh::bvh::Bvh<f32, 3>,
        Vec<BvhPrimitive>,
        Vec<Chart>,
        Vec<ChartPlacement>,
        Vec<MapLight>,
        EntityShadowLightsSection,
    ) {
        let geometry = quad_geometry();
        let bvh = bvh::bvh::Bvh { nodes: Vec::new() };
        let primitives = Vec::new();
        let charts = vec![one_texel_chart(), one_texel_chart()];
        let placements = vec![
            ChartPlacement {
                x: 0,
                y: 0,
                layer: 0,
            },
            ChartPlacement {
                x: 0,
                y: 0,
                layer: 1,
            },
        ];
        let mut lights = vec![light(5.0), light(4.0), light(1.0), light(3.0), light(2.0)];
        for test_light in &mut lights {
            test_light.origin = DVec3::new(0.5, 1.0, 0.5);
        }
        let selection = EntityShadowLightsSection {
            light_indices: (0..lights.len() as u32).collect(),
        };
        (
            geometry, bvh, primitives, charts, placements, lights, selection,
        )
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
    fn nan_raw_visibility_is_retained_with_legacy_inclusion_semantics() {
        let lights = [(0, light(5.0)), (1, light(4.0))];
        let selected: Vec<(usize, u32, &MapLight)> = lights
            .iter()
            .enumerate()
            .map(|(selection_index, (index, light))| (selection_index, *index, light))
            .collect();
        let layers = vec![
            layer(1, 1, 1, &[(0, 0, f32::NAN)]),
            layer(1, 1, 1, &[(0, 0, 1.0)]),
        ];

        let section = build_shadowmask_from_layers(1, 1, 1, 2, &selected, &layers);

        // NaN was historically included because the old code skipped only
        // values satisfying `raw_visibility < 0.0`. It must still create the
        // overlap edge (and quantizes with the established Rust cast behavior).
        assert_ne!(section.channels[0], section.channels[1]);
        assert_eq!(section.data[section.channels[0] as usize], 0);
        assert_eq!(section.data[section.channels[1] as usize], 255);
    }

    #[cfg(target_pointer_width = "64")]
    #[test]
    fn membership_indices_above_u32_do_not_alias_overlap_or_fill_offsets() {
        let first = 0usize;
        let plane = texel_plane_len(8192, 8192);
        let after_u32 = 64usize * plane;
        assert_eq!(after_u32, u32::MAX as usize + 1);
        let membership = ShadowmaskMembership {
            by_light: vec![
                vec![ShadowmaskMembershipTexel {
                    global_texel_index: first,
                    compact_light_index: 0,
                    raw_visibility: 1.0,
                }],
                vec![ShadowmaskMembershipTexel {
                    global_texel_index: after_u32,
                    compact_light_index: 1,
                    raw_visibility: 1.0,
                }],
            ],
        };

        let graph = overlap_graph(&membership);

        assert!(
            !graph[0][1],
            "distinct global texels must not gain a false overlap edge after u32"
        );
        assert_ne!(
            shadowmask_data_offset(first, 0),
            shadowmask_data_offset(after_u32, 0),
            "fill offsets must retain the full global texel identity"
        );
    }

    #[test]
    fn paused_assignment_barrier_waits_before_shadowmask_fill() {
        let governor = Arc::new(Governor::new(1, true));
        let (waiting_tx, waiting_rx) = mpsc::channel();
        let (finished_tx, finished_rx) = mpsc::channel();
        let worker_governor = Arc::clone(&governor);
        let worker = thread::spawn(move || {
            let progress = StageProgress::indeterminate();
            let control = BakeControl::new(worker_governor, &progress);
            let light = light(1.0);
            let selected = vec![(0, 0, &light)];
            let membership = ShadowmaskMembership {
                by_light: vec![vec![ShadowmaskMembershipTexel {
                    global_texel_index: 0,
                    compact_light_index: 0,
                    raw_visibility: 1.0,
                }]],
            };
            let section = build_shadowmask_from_membership_with_assignment_checkpoint(
                1,
                1,
                1,
                1,
                &selected,
                &membership,
                Some(&control),
                || {
                    control.governor().checkpoint_with_wait_observer(|| {
                        waiting_tx.send(()).expect("test coordinator is waiting");
                    });
                },
            );
            finished_tx
                .send(section)
                .expect("test coordinator is waiting");
        });

        waiting_rx
            .recv_timeout(Duration::from_secs(2))
            .expect("assignment checkpoint did not observe the paused governor");
        assert!(
            matches!(finished_rx.try_recv(), Err(mpsc::TryRecvError::Empty)),
            "assignment must remain blocked after its pause checkpoint reports waiting"
        );

        governor.set_paused(false);
        let section = finished_rx
            .recv_timeout(Duration::from_secs(2))
            .expect("assignment did not resume after unpausing");
        worker.join().expect("assignment worker must not panic");
        assert_eq!(section.data, vec![255, 255, 255, 255]);
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
    fn pre_streaming_multilayer_five_way_overlap_golden_bytes_are_preserved() {
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
            .map(|(selection_index, (alpha_index, light))| (selection_index, *alpha_index, light))
            .collect();
        let layers = vec![
            layer(2, 1, 2, &[(0, 0, 0.25), (1, 1, 0.6)]),
            layer(2, 1, 2, &[(0, 0, 0.5)]),
            layer(2, 1, 2, &[(0, 0, 0.75)]),
            layer(2, 1, 2, &[(0, 0, 1.0)]),
            layer(2, 1, 2, &[(0, 0, 0.0)]),
        ];

        let section = build_shadowmask_from_layers(2, 1, 2, 5, &selected, &layers);

        // Captured from the pre-streaming layer composite: two atlas layers,
        // five selected masks sharing a texel (the lowest intensity drops), and
        // an additional layer-1 texel proving layer-major addressing.
        let golden = [
            2, 0, 0, 0, 1, 0, 0, 0, 2, 0, 0, 0, 5, 0, 0, 0, 0, 1, 0xFF, 2, 3, 0, 0, 0, 64, 128,
            255, 0, 255, 255, 255, 255, 255, 255, 255, 255, 153, 255, 255, 255,
        ];
        assert_eq!(section.to_bytes(), golden);
    }

    #[test]
    fn top_level_cached_and_no_cache_paths_match_multilayer_five_way_golden() {
        let (geometry, bvh, primitives, charts, placements, lights, selection) =
            top_level_multilayer_five_way_inputs();
        let shared = SharedAtlas {
            charts: &charts,
            placements: &placements,
            atlas_width: 5,
            atlas_height: 5,
        };
        let alpha_lights = AlphaLightsNs::from_lights(&lights);

        let no_cache = bake_shadowmask_atlas(
            Some(&selection),
            &alpha_lights,
            &shared,
            &bvh,
            &primitives,
            &geometry,
            AREA_SAMPLES,
            &test_control(),
        )
        .expect("no-cache shadowmask section");
        assert_eq!(
            no_cache.to_bytes().as_slice(),
            TOP_LEVEL_MULTILAYER_FIVE_WAY_GOLDEN.as_slice(),
            "no-cache streaming bake must match the pre-streaming golden"
        );

        let input_hashes: Vec<[u8; 32]> = lights
            .iter()
            .map(|test_light| {
                lightmap_layer::layer_input_hash(
                    test_light,
                    &shared,
                    &primitives,
                    &geometry,
                    DENSITY,
                    AREA_SAMPLES,
                )
            })
            .collect();
        let cold_dir = fresh_cache_dir("top_level_golden_cold");
        let cold_cache = StageCache::new(&cold_dir).expect("cold cache dir");
        let cached_miss = bake_shadowmask_atlas_cached(
            Some(&selection),
            &alpha_lights,
            &shared,
            &bvh,
            &primitives,
            &geometry,
            DENSITY,
            AREA_SAMPLES,
            Some(&cold_cache),
            &test_control(),
        )
        .expect("cached shadowmask section after cold per-light misses");
        assert_eq!(
            cached_miss.to_bytes().as_slice(),
            TOP_LEVEL_MULTILAYER_FIVE_WAY_GOLDEN.as_slice(),
            "cache-backed streaming bake must match the pre-streaming golden"
        );

        // Copy only the warm per-light layers to a fresh cache so the next
        // call must take a section miss while reusing every layer entry.
        let warm_dir = fresh_cache_dir("top_level_golden_warm_layers");
        let warm_cache = StageCache::new(&warm_dir).expect("warm cache dir");
        for input_hash in &input_hashes {
            let layer_key = CacheKey::new(
                "lightmap_layer",
                lightmap_layer::LAYER_FORMAT_VERSION,
                input_hash,
            );
            let bytes = cold_cache
                .get(&layer_key)
                .expect("cold cache stores each baked layer");
            warm_cache.put(&layer_key, &bytes);
        }
        let cached_warm_layers = bake_shadowmask_atlas_cached(
            Some(&selection),
            &alpha_lights,
            &shared,
            &bvh,
            &primitives,
            &geometry,
            DENSITY,
            AREA_SAMPLES,
            Some(&warm_cache),
            &test_control(),
        )
        .expect("cached shadowmask section from warm layers");
        assert_eq!(
            cached_warm_layers.to_bytes().as_slice(),
            TOP_LEVEL_MULTILAYER_FIVE_WAY_GOLDEN.as_slice(),
            "per-light warm cache section miss must match the pre-streaming golden"
        );

        let cached_section_hit = bake_shadowmask_atlas_cached(
            Some(&selection),
            &alpha_lights,
            &shared,
            &bvh,
            &primitives,
            &geometry,
            DENSITY,
            AREA_SAMPLES,
            Some(&warm_cache),
            &test_control(),
        )
        .expect("whole shadowmask cache hit");
        assert_eq!(
            cached_section_hit.to_bytes().as_slice(),
            TOP_LEVEL_MULTILAYER_FIVE_WAY_GOLDEN.as_slice(),
            "whole-section cache hit must return the pre-streaming golden"
        );

        let _ = std::fs::remove_dir_all(&cold_dir);
        let _ = std::fs::remove_dir_all(&warm_dir);
    }

    #[test]
    fn window_sizes_bound_resident_layers_and_preserve_shadowmask_bytes() {
        let (geometry, bvh, primitives, charts, placements, lights, selection) =
            top_level_multilayer_five_way_inputs();
        let shared = SharedAtlas {
            charts: &charts,
            placements: &placements,
            atlas_width: 5,
            atlas_height: 5,
        };
        let alpha_lights = AlphaLightsNs::from_lights(&lights);

        for window in [1, 2, 4] {
            let progress = StageProgress::indeterminate();
            let control = BakeControl::new(Arc::new(Governor::new(4, false)), &progress);
            let (section, resident_high_water) = bake_shadowmask_atlas_with_test_window(
                Some(&selection),
                &alpha_lights,
                &shared,
                &bvh,
                &primitives,
                &geometry,
                AREA_SAMPLES,
                &control,
                window,
            );
            let section = section.expect("selected lights produce a shadowmask section");

            assert!(
                resident_high_water <= window,
                "W={window} must bound resident per-light payloads"
            );
            assert_eq!(
                section.to_bytes().as_slice(),
                TOP_LEVEL_MULTILAYER_FIVE_WAY_GOLDEN.as_slice(),
                "W={window} must not change shipped shadowmask bytes"
            );
        }
    }

    #[test]
    fn high_permit_low_chart_bake_saturates_all_permits_when_window_exposes_eight_tasks() {
        let geometry = quad_geometry();
        let bvh = bvh::bvh::Bvh { nodes: Vec::new() };
        let primitives = Vec::new();
        let charts = vec![one_texel_chart()];
        let placements = vec![ChartPlacement {
            x: 0,
            y: 0,
            layer: 0,
        }];
        let mut lights: Vec<MapLight> = (0..8).map(|index| light(8.0 - index as f32)).collect();
        for test_light in &mut lights {
            test_light.origin = DVec3::new(0.5, 1.0, 0.5);
        }
        let selection = EntityShadowLightsSection {
            light_indices: (0..lights.len() as u32).collect(),
        };
        let shared = SharedAtlas {
            charts: &charts,
            placements: &placements,
            atlas_width: 5,
            atlas_height: 5,
        };
        let alpha_lights = AlphaLightsNs::from_lights(&lights);
        let progress = StageProgress::indeterminate();
        let governor = Arc::new(Governor::new(8, false));
        let control = BakeControl::new(Arc::clone(&governor), &progress);
        let pool = ThreadPoolBuilder::new()
            .num_threads(8)
            .build()
            .expect("eight-worker pool");

        let (admitted_tx, admitted_rx) = mpsc::channel();
        let released = Arc::new((Mutex::new(false), Condvar::new()));
        let hook_released = Arc::clone(&released);
        governor.set_enter_hook(Arc::new(move || {
            admitted_tx
                .send(())
                .expect("admission coordinator is waiting");
            let (lock, changed) = &*hook_released;
            let released = lock.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
            drop(
                changed
                    .wait_while(released, |released| !*released)
                    .unwrap_or_else(|poisoned| poisoned.into_inner()),
            );
        }));

        let ((section, resident_high_water), admitted_count) = thread::scope(|scope| {
            let worker = scope.spawn(|| {
                pool.install(|| {
                    bake_shadowmask_atlas_with_test_window(
                        Some(&selection),
                        &alpha_lights,
                        &shared,
                        &bvh,
                        &primitives,
                        &geometry,
                        AREA_SAMPLES,
                        &control,
                        8,
                    )
                })
            });

            let mut admitted_count = 0;
            while admitted_count < 8 && admitted_rx.recv_timeout(Duration::from_secs(2)).is_ok() {
                admitted_count += 1;
            }
            let (lock, changed) = &*released;
            *lock.lock().unwrap_or_else(|poisoned| poisoned.into_inner()) = true;
            changed.notify_all();

            (
                worker
                    .join()
                    .expect("shadowmask saturation worker panicked"),
                admitted_count,
            )
        });

        assert_eq!(
            admitted_count, 8,
            "one-chart selected-light work must expose the full -j 8 permit cap"
        );
        assert!(section.is_some());
        assert!(
            resident_high_water <= 8,
            "the chart batch must stay within its full-layer residency window"
        );
        assert_eq!(progress.total(), Some(8));
        assert_eq!(progress.completed(), 8);
    }

    // Regression: permit-driven batch widening retained eight raw full-layer payloads at W=1.
    #[test]
    fn one_layer_window_bounds_cold_one_chart_payloads_below_eight_permits() {
        let geometry = quad_geometry();
        let bvh = bvh::bvh::Bvh { nodes: Vec::new() };
        let primitives = Vec::new();
        let charts = vec![one_texel_chart()];
        let placements = vec![ChartPlacement {
            x: 0,
            y: 0,
            layer: 0,
        }];
        let mut lights: Vec<MapLight> = (0..8).map(|index| light(8.0 - index as f32)).collect();
        for test_light in &mut lights {
            test_light.origin = DVec3::new(0.5, 1.0, 0.5);
        }
        let selection = EntityShadowLightsSection {
            light_indices: (0..lights.len() as u32).collect(),
        };
        let shared = SharedAtlas {
            charts: &charts,
            placements: &placements,
            atlas_width: 5,
            atlas_height: 5,
        };
        let alpha_lights = AlphaLightsNs::from_lights(&lights);
        let progress = StageProgress::indeterminate();
        let control = BakeControl::new(Arc::new(Governor::new(8, false)), &progress);
        let pool = ThreadPoolBuilder::new()
            .num_threads(8)
            .build()
            .expect("eight-worker pool");

        let (section, resident_high_water) = pool.install(|| {
            bake_shadowmask_atlas_with_test_window(
                Some(&selection),
                &alpha_lights,
                &shared,
                &bvh,
                &primitives,
                &geometry,
                AREA_SAMPLES,
                &control,
                1,
            )
        });

        assert!(section.is_some());
        assert_eq!(resident_high_water, 1);
        assert_eq!(progress.total(), Some(8));
        assert_eq!(progress.completed(), 8);
    }

    #[test]
    fn reversed_completion_membership_keeps_compact_selection_bytes() {
        let lights = [(0, light(5.0)), (1, light(4.0)), (2, light(3.0))];
        let selected: Vec<(usize, u32, &MapLight)> = lights
            .iter()
            .enumerate()
            .map(|(selection_index, (alpha_index, light))| (selection_index, *alpha_index, light))
            .collect();
        let layers = vec![
            layer(1, 1, 1, &[(0, 0, 0.25)]),
            layer(1, 1, 1, &[(0, 0, 0.5)]),
            layer(1, 1, 1, &[(0, 0, 0.75)]),
        ];
        let submission_order = build_shadowmask_from_layers(1, 1, 1, 3, &selected, &layers);

        // Simulate C, B, A finishing after their fixed compact indexes were
        // assigned. Buckets are addressed by compact index, not append order.
        let mut completion_order = ShadowmaskMembership::for_light_count(selected.len());
        for compact_light_index in [2, 1, 0] {
            record_layer_membership(
                &mut completion_order,
                compact_light_index,
                &layers[compact_light_index],
                1,
            );
        }
        let reversed = build_shadowmask_from_membership(1, 1, 1, 3, &selected, &completion_order);

        assert_eq!(reversed.to_bytes(), submission_order.to_bytes());
    }

    #[test]
    fn one_permit_with_window_four_completes_and_reports_all_chart_work() {
        let (geometry, bvh, primitives, charts, placements, lights, selection) =
            top_level_multilayer_five_way_inputs();
        let shared = SharedAtlas {
            charts: &charts,
            placements: &placements,
            atlas_width: 5,
            atlas_height: 5,
        };
        let alpha_lights = AlphaLightsNs::from_lights(&lights);
        let total = selection.light_indices.len() * shared.placements.len();
        let progress = StageProgress::indeterminate();
        let control = BakeControl::new(Arc::new(Governor::new(1, false)), &progress);
        let pool = ThreadPoolBuilder::new()
            .num_threads(2)
            .build()
            .expect("two-worker pool");
        let (section, resident_high_water) = pool.install(|| {
            bake_shadowmask_atlas_with_test_window(
                Some(&selection),
                &alpha_lights,
                &shared,
                &bvh,
                &primitives,
                &geometry,
                AREA_SAMPLES,
                &control,
                4,
            )
        });

        assert!(section.is_some(), "-j 1 windowed bake must complete");
        assert_eq!(progress.total(), Some(total));
        assert_eq!(progress.completed(), total);
        assert!(resident_high_water <= 4);
    }

    #[test]
    fn last_partial_batch_assigns_cross_batch_overlap_once_globally() {
        let (geometry, bvh, primitives, charts, placements, lights, selection) =
            top_level_multilayer_five_way_inputs();
        let shared = SharedAtlas {
            charts: &charts,
            placements: &placements,
            atlas_width: 5,
            atlas_height: 5,
        };
        let alpha_lights = AlphaLightsNs::from_lights(&lights);
        let (section, _) = bake_shadowmask_atlas_with_test_window(
            Some(&selection),
            &alpha_lights,
            &shared,
            &bvh,
            &primitives,
            &geometry,
            AREA_SAMPLES,
            &test_control(),
            2,
        );
        let section = section.expect("last partial batch still emits a section");

        // Five lights cover the fixture texel. The lowest-intensity third light
        // is dropped, while A (first batch) and E (last partial batch) must be
        // colored differently by the one global post-batch assignment.
        assert_ne!(section.channels[0], SHADOWMASK_CHANNEL_DROPPED);
        assert_ne!(section.channels[4], SHADOWMASK_CHANNEL_DROPPED);
        assert_ne!(section.channels[0], section.channels[4]);
    }

    #[test]
    fn zero_selection_keeps_the_none_section_path() {
        let selection = EntityShadowLightsSection {
            light_indices: Vec::new(),
        };
        assert_eq!(
            bake_shadowmask_atlas_from_layers(&selection, 1, 1, 1, &[], &[]),
            None
        );
    }

    #[test]
    fn all_filtered_selection_keeps_empty_bytes_and_indeterminate_progress() {
        let source_lights = vec![light(5.0)];
        let (geo, prepared, bvh, primitives) = cache_fixture(&source_lights);
        let shared = shared_from_prepared(&prepared);
        let no_alpha_lights = AlphaLightsNs::from_lights(&[]);
        let selection = EntityShadowLightsSection {
            light_indices: vec![0],
        };
        let progress = StageProgress::indeterminate();
        let control = BakeControl::new(Arc::new(Governor::new(1, false)), &progress);

        let section = bake_shadowmask_atlas(
            Some(&selection),
            &no_alpha_lights,
            &shared,
            &bvh,
            &primitives,
            &geo,
            AREA_SAMPLES,
            &control,
        )
        .expect("all-filtered selection still emits an empty section");

        assert_eq!(section.channels, vec![SHADOWMASK_CHANNEL_DROPPED]);
        assert!(section.data.iter().all(|&value| value == 255));
        assert_eq!(progress.total(), None);
        assert_eq!(progress.completed(), 0);
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
    fn cached_streaming_layers_stay_bounded_and_warm_progress_completes() {
        let mut lights: Vec<MapLight> = (1..=5).map(|intensity| light(intensity as f32)).collect();
        for test_light in &mut lights {
            test_light.origin = DVec3::new(0.25, 1.0, 0.25);
        }
        let (geo, prepared, bvh, primitives) = cache_fixture(&lights);
        let shared = shared_from_prepared(&prepared);
        let alpha_lights = AlphaLightsNs::from_lights(&lights);
        let selection = EntityShadowLightsSection {
            light_indices: (0..lights.len() as u32).collect(),
        };
        let hashes: Vec<[u8; 32]> = lights
            .iter()
            .map(|test_light| {
                lightmap_layer::layer_input_hash(
                    test_light,
                    &shared,
                    &primitives,
                    &geo,
                    DENSITY,
                    AREA_SAMPLES,
                )
            })
            .collect();
        let dir = fresh_cache_dir("streaming_bound_and_warm_progress");
        let cache = StageCache::new(&dir).expect("cache dir");
        for (index, hash) in hashes.iter().enumerate() {
            let key = CacheKey::new("lightmap_layer", lightmap_layer::LAYER_FORMAT_VERSION, hash);
            cache.put(
                &key,
                &layer(
                    shared.atlas_width,
                    shared.atlas_height,
                    layer_count_from_shared(&shared),
                    &[(0, 0, index as f32 / 4.0)],
                )
                .to_bytes(),
            );
        }

        let total = lights.len() * shared.placements.len();
        assert_ne!(total, 0, "fixture must expose chart work");
        let cold_progress = StageProgress::indeterminate();
        let cold_control = BakeControl::new(Arc::new(Governor::new(1, false)), &cold_progress);
        let (cold, resident_high_water) = bake_shadowmask_atlas_cached_with_test_window(
            Some(&selection),
            &alpha_lights,
            &shared,
            &bvh,
            &primitives,
            &geo,
            DENSITY,
            AREA_SAMPLES,
            Some(&cache),
            &cold_control,
            SHADOWMASK_RESIDENT_LAYER_WINDOW,
        );
        let cold = cold.expect("cached section miss rebuilt from streamed layers");

        assert_eq!(cold_progress.total(), Some(total));
        assert_eq!(cold_progress.completed(), total);
        assert!(
            resident_high_water <= SHADOWMASK_RESIDENT_LAYER_WINDOW,
            "streaming path must retain no more than its fixed layer window"
        );

        let warm_progress = StageProgress::indeterminate();
        let warm_control = BakeControl::new(Arc::new(Governor::new(1, false)), &warm_progress);
        let warm = bake_shadowmask_atlas_cached(
            Some(&selection),
            &alpha_lights,
            &shared,
            &bvh,
            &primitives,
            &geo,
            DENSITY,
            AREA_SAMPLES,
            Some(&cache),
            &warm_control,
        )
        .expect("whole shadowmask memo hit");

        assert_eq!(warm, cold);
        assert_eq!(warm_progress.total(), Some(total));
        assert_eq!(warm_progress.completed(), total);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn mixed_layer_cache_hits_and_misses_complete_shadowmask_progress() {
        let mut lights = vec![light(5.0), light(4.0), light(3.0)];
        for (index, test_light) in lights.iter_mut().enumerate() {
            test_light.origin = DVec3::new(0.25 + index as f64 * 0.1, 1.0, 0.25);
        }
        let (geo, prepared, bvh, primitives) = cache_fixture(&lights);
        let shared = shared_from_prepared(&prepared);
        let alpha_lights = AlphaLightsNs::from_lights(&lights);
        let selection = EntityShadowLightsSection {
            light_indices: vec![0, 1, 2],
        };
        let (first_layer_key, _) = layer_key(&lights[0], &shared, &primitives, &geo, AREA_SAMPLES);
        let dir = fresh_cache_dir("mixed_layer_cache_progress");
        let cache = StageCache::new(&dir).expect("cache dir");
        cache.put(
            &first_layer_key,
            &layer(
                shared.atlas_width,
                shared.atlas_height,
                layer_count_from_shared(&shared),
                &[(0, 0, 0.5)],
            )
            .to_bytes(),
        );

        let total = selection.light_indices.len() * shared.placements.len();
        let progress = StageProgress::indeterminate();
        let control = BakeControl::new(Arc::new(Governor::new(2, false)), &progress);
        let (section, resident_high_water) = bake_shadowmask_atlas_cached_with_test_window(
            Some(&selection),
            &alpha_lights,
            &shared,
            &bvh,
            &primitives,
            &geo,
            DENSITY,
            AREA_SAMPLES,
            Some(&cache),
            &control,
            2,
        );

        assert!(section.is_some());
        assert_eq!(progress.total(), Some(total));
        assert_eq!(progress.completed(), total);
        assert!(resident_high_water <= 2);
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
