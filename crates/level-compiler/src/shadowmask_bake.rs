// Per-light shadowmask bake for selected static entity-shadow lights.
// Governing context: context/lib/build_pipeline.md

use std::collections::HashMap;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant};

use glam::DVec3;
use rayon::prelude::*;

use postretro_level_format::entity_shadow_lights::EntityShadowLightsSection;
use postretro_level_format::lightmap::LIGHTMAP_POOL_LAYER_EDGE;
use postretro_level_format::shadowmask_atlas::{SHADOWMASK_GROUP_COUNT, ShadowmaskAtlasSection};

use crate::bake_control::BakeControl;
use crate::bvh_build::BvhPrimitive;
use crate::cache::{CacheKey, StageCache};
use crate::geometry::GeometryResult;
use crate::light_namespaces::AlphaLightsNs;
use crate::lightmap_layer::{self, LightmapLayer, SharedAtlas};
use crate::map_data::{LightType, MapLight};
use crate::{affinity_grid, lightmap_bake};

mod assignment;
mod encode;
mod fill;
mod memo;
mod partitions;
mod section;

#[cfg(test)]
pub(crate) use encode::decode_blocks_to_layers;

use assignment::*;
use fill::*;
pub use memo::shadowmask_atlas_input_hash;
use memo::{
    cache_shadowmask_section_then_complete, invalid_selected_light_hash, read_shadowmask_memo,
};
use partitions::{fill_cached_shadowmask_partitions, fill_uncached_shadowmask_partitions};
pub use section::bake_shadowmask_atlas_from_layers;
use section::{empty_section_for_selection, layer_count_from_shared};

pub const SHADOWMASK_ATLAS_STAGE_ID: &str = "shadowmask_atlas";

/// Bump when the cached `ShadowmaskAtlas` bytes can change without a layer input
/// hash change: channel assignment/drop policy, raw-visibility quantization,
/// payload encoding (BC5 groups, per-block BC4 mode choice), memo entry
/// layout (the peak-overlap prefix), empty-section behavior, or
/// `ShadowmaskAtlasSection::to_bytes` payload semantics.
///
/// v5: `SMB6`, one pair of BC5 group planes per lightmap cell block.
///
/// v6: raw visibility follows the chart-local soft-visibility seeds. The key
/// also folds `LAYER_FORMAT_VERSION`, whose bump already misses old memos;
/// this one marks the stage's own output change.
pub const SHADOWMASK_ATLAS_STAGE_VERSION: u32 = 6;

/// A pool layer holds the two mask groups side by side, so it is
/// `SHADOWMASK_GROUP_COUNT` pool edges wide and must fit the pinned device
/// texture dimension. The old whole-layer omission rule (id 42 dropped when
/// the lightmap layer could not double) cannot trigger any more: the
/// oversize-face cut and cell packing bound every block by the pool layer
/// edge, and the doubled pool layer fits by construction.
pub(crate) const MAX_SHADOWMASK_TEXTURE_WIDTH: u32 = lightmap_bake::MAX_ATLAS_DIMENSION;

const _: () =
    assert!(LIGHTMAP_POOL_LAYER_EDGE * SHADOWMASK_GROUP_COUNT <= MAX_SHADOWMASK_TEXTURE_WIDTH);

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub(crate) enum ShadowmaskBakeError {
    #[error(
        "shadowmask atlas dimensions {width}x{height} are not multiples of 4; \
         BC5 encodes 4x4 blocks and would truncate the payload"
    )]
    MisalignedAtlas { width: u32, height: u32 },
}

/// Maximum selected-light count in one governed chart batch. Each light's raw
/// chart buffers collectively carry one full layer payload, so the batch must
/// never widen beyond this residency window.
const SHADOWMASK_RESIDENT_LAYER_WINDOW: usize = 4;

/// Fill checks the cooperative pause gate at this cadence without consuming a
/// governor permit; chart work remains the only governed parallel level.
const SHADOWMASK_FILL_CHECKPOINT_TEXELS: usize = 1024;

/// Shadowmask state prepared before the fused lightmap walk. A section-cache
/// hit carries the finished section and requests no partitions. A miss owns an
/// already-colored fill buffer, so each partition can be consumed directly
/// while the lightmap fold still has it resident.
pub(crate) struct FusedShadowmaskPlan<'a> {
    section: Option<ShadowmaskAtlasSection>,
    overlap: ShadowmaskOverlapReport,
    fill: Option<ShadowmaskFill<'a>>,
    /// The miss's graph peak, stored beside the section in the memo entry.
    peak_texel_overlap: u32,
    compact_index_by_source: HashMap<usize, usize>,
    cache_write: Option<(&'a StageCache, CacheKey)>,
    control: &'a BakeControl,
    work_elapsed: Duration,
}

impl FusedShadowmaskPlan<'_> {
    pub(crate) fn needs_source(&self, source_index: usize) -> bool {
        self.fill.is_some() && self.compact_index_by_source.contains_key(&source_index)
    }

    pub(crate) fn consume_partition(&mut self, source_index: usize, partition: &LightmapLayer) {
        let Some(fill) = self.fill.as_mut() else {
            return;
        };
        let Some(&compact_index) = self.compact_index_by_source.get(&source_index) else {
            return;
        };
        let started = Instant::now();
        fill.write_partition(compact_index, partition);
        self.work_elapsed += started.elapsed();
    }

    pub(crate) fn finish(mut self) -> FusedShadowmaskOutput {
        let started = Instant::now();
        if let Some(fill) = self.fill.take() {
            let section = fill.finish();
            if let Some((cache, key)) = self.cache_write.take() {
                cache_shadowmask_section_then_complete(
                    cache,
                    &key,
                    &section,
                    self.peak_texel_overlap,
                    self.control,
                    true,
                    || {},
                );
            } else {
                self.control.advance(1);
            }
            self.section = Some(section);
        }
        self.work_elapsed += started.elapsed();
        FusedShadowmaskOutput {
            section: self.section,
            elapsed: self.work_elapsed,
            overlap: self.overlap,
        }
    }
}

pub(crate) struct FusedShadowmaskOutput {
    pub(crate) section: Option<ShadowmaskAtlasSection>,
    pub(crate) elapsed: Duration,
    pub(crate) overlap: ShadowmaskOverlapReport,
}

/// What `--verbose` reports about per-texel mask overlap for one bake.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ShadowmaskOverlapReport {
    /// No selected static light: no section and nothing measured.
    NoSelection,
    /// Most selected lights covering one texel, from this bake's graph or the
    /// memo entry that graph produced.
    Peak(u32),
}

/// The `--verbose` overlap line. The mask-capacity decision reads this number;
/// nothing in the bake acts on it.
pub(crate) fn log_overlap_report(
    report: ShadowmaskOverlapReport,
    section: Option<&ShadowmaskAtlasSection>,
) {
    if let (ShadowmaskOverlapReport::Peak(peak), Some(section)) = (report, section) {
        log::info!(
            "[ShadowmaskAtlas] peak per-texel overlap: {peak} selected light(s) at one texel; \
             {} cell block(s), BC5 .rg in {} groups ({} slots)",
            section.blocks.len(),
            SHADOWMASK_GROUP_COUNT,
            SHADOWMASK_GROUP_COUNT * 2,
        );
    }
}

/// Probe the whole shadowmask memo and, on a miss, complete analytic overlap
/// graph construction plus deterministic channel assignment before the fused
/// lightmap walk begins. No visibility ray is traced here.
#[allow(clippy::too_many_arguments)]
pub(crate) fn prepare_fused_shadowmask<'a>(
    selection: Option<&EntityShadowLightsSection>,
    alpha_lights: &'a AlphaLightsNs<'a>,
    shared: &SharedAtlas<'_>,
    primitives: &[BvhPrimitive],
    geometry: &GeometryResult,
    lightmap_density: f32,
    area_sample_count: u32,
    cache: Option<&'a StageCache>,
    control: &'a BakeControl,
) -> Result<FusedShadowmaskPlan<'a>, ShadowmaskBakeError> {
    let started = Instant::now();
    let no_section = |started: Instant, overlap| FusedShadowmaskPlan {
        section: None,
        overlap,
        fill: None,
        peak_texel_overlap: 0,
        compact_index_by_source: HashMap::new(),
        cache_write: None,
        control,
        work_elapsed: started.elapsed(),
    };
    let Some(selection) = selection.filter(|selection| !selection.light_indices.is_empty()) else {
        return Ok(no_section(started, ShadowmaskOverlapReport::NoSelection));
    };
    // No placed chart means no lightmap texel for a mask to cover. The empty-
    // geometry atlas is 1×1 and would otherwise fail the alignment check.
    if shared.placements.is_empty() {
        return Ok(no_section(started, ShadowmaskOverlapReport::NoSelection));
    }
    if shared.atlas_width % 4 != 0 || shared.atlas_height % 4 != 0 {
        return Err(ShadowmaskBakeError::MisalignedAtlas {
            width: shared.atlas_width,
            height: shared.atlas_height,
        });
    }
    let layer_count = layer_count_from_shared(shared);
    let mut selected = Vec::with_capacity(selection.light_indices.len());
    let mut compact_index_by_source = HashMap::new();
    // The layer keys feed only the memo key, so an uncached bake hashes nothing.
    let key_context = cache.map(|_| {
        lightmap_layer::LayerKeyContext::new(shared, geometry, lightmap_density, area_sample_count)
    });
    let mut layer_input_hashes = Vec::new();
    for (selection_index, &alpha_index) in selection.light_indices.iter().enumerate() {
        let Some(entry) = alpha_lights.entries().get(alpha_index as usize) else {
            log::warn!(
                "[ShadowmaskAtlas] selected AlphaLights index {alpha_index} is out of range; marking dropped"
            );
            if key_context.is_some() {
                for target_layer in 0..layer_count {
                    layer_input_hashes.push(invalid_selected_light_hash(alpha_index, target_layer));
                }
            }
            continue;
        };
        compact_index_by_source.insert(entry.source_index, selected.len());
        selected.push((selection_index, alpha_index, entry.light));
        if let Some(context) = &key_context {
            let prefix = context.light_prefix(entry.light, primitives, geometry);
            for target_layer in 0..layer_count {
                layer_input_hashes.push(lightmap_layer::LayerKeyContext::layer_hash(
                    &prefix,
                    target_layer,
                ));
            }
        }
    }

    let section_key = cache.map(|_| {
        let input_hash = shadowmask_atlas_input_hash(
            selection,
            &layer_input_hashes,
            shared.atlas_width,
            shared.atlas_height,
            layer_count,
        );
        CacheKey::new(
            SHADOWMASK_ATLAS_STAGE_ID,
            SHADOWMASK_ATLAS_STAGE_VERSION,
            &input_hash,
        )
    });

    let fused_total = if selected.is_empty() {
        0
    } else {
        shared.placements.len().saturating_add(2)
    };
    if fused_total != 0 {
        control.publish_total(fused_total);
    }

    if let (Some(cache), Some(key)) = (cache, section_key.as_ref()) {
        if let Some(memo) = read_shadowmask_memo(cache, key, selection, shared) {
            log::info!("[cache] shadowmask_atlas hit");
            control.governor().checkpoint();
            control.advance(fused_total);
            return Ok(FusedShadowmaskPlan {
                section: Some(memo.section),
                overlap: ShadowmaskOverlapReport::Peak(memo.peak_texel_overlap),
                fill: None,
                peak_texel_overlap: 0,
                compact_index_by_source,
                cache_write: None,
                control,
                work_elapsed: started.elapsed(),
            });
        }
        log::info!("[cache] shadowmask_atlas miss");
    }

    if selected.is_empty() {
        // No valid selected light covers anything: the peak is zero.
        let section = empty_section_for_selection(shared, selection.light_indices.len());
        if let (Some(cache), Some(key)) = (cache, section_key.as_ref()) {
            cache_shadowmask_section_then_complete(cache, key, &section, 0, control, false, || {});
        }
        return Ok(FusedShadowmaskPlan {
            section: Some(section),
            overlap: ShadowmaskOverlapReport::Peak(0),
            fill: None,
            peak_texel_overlap: 0,
            compact_index_by_source,
            cache_write: None,
            control,
            work_elapsed: started.elapsed(),
        });
    }

    let graph = build_analytic_overlap_graph(&selected, shared, geometry, control);
    let fill = ShadowmaskFill::new(
        shared.atlas_width,
        shared.atlas_height,
        layer_count,
        &shared.layout.blocks,
        selection.light_indices.len(),
        &selected,
        &graph,
        Some(control),
        None,
    );
    let peak_texel_overlap = graph.peak_texel_overlap();
    Ok(FusedShadowmaskPlan {
        section: None,
        overlap: ShadowmaskOverlapReport::Peak(peak_texel_overlap),
        fill: Some(fill),
        peak_texel_overlap,
        compact_index_by_source,
        cache_write: cache.zip(section_key),
        control,
        work_elapsed: started.elapsed(),
    })
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

#[cfg(test)]
thread_local! {
    static LAST_RAW_FILL: std::cell::RefCell<Option<Vec<u8>>> = const {
        std::cell::RefCell::new(None)
    };
    static RAW_FILL_LIVE_BYTES: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
    static RAW_FILL_LIVE_BYTES_AT_CACHE_WRITE: std::cell::Cell<Option<usize>> = const {
        std::cell::Cell::new(None)
    };
    static SHADOWMASK_OUTPUT_ALLOCATION_COUNT: std::cell::Cell<usize> = const {
        std::cell::Cell::new(0)
    };
    static SHADOWMASK_STREAMED_CACHE_WRITE_COUNT: std::cell::Cell<usize> = const {
        std::cell::Cell::new(0)
    };
}

#[cfg(test)]
fn take_raw_fill_live_bytes_at_cache_write() -> Option<usize> {
    RAW_FILL_LIVE_BYTES_AT_CACHE_WRITE.with(std::cell::Cell::take)
}

/// Tests assert exact fill values on the raw masks the encoder consumed; BC4
/// reproduces only block endpoints exactly. The copy lives outside
/// `RawFillBuffer`, so the raw-fill residency counters exclude it.
#[cfg(test)]
fn record_raw_fill(raw: &[u8]) {
    LAST_RAW_FILL.with(|last| *last.borrow_mut() = Some(raw.to_vec()));
}

#[cfg(test)]
fn take_last_raw_fill() -> Vec<u8> {
    LAST_RAW_FILL
        .with(|last| last.borrow_mut().take())
        .expect("a shadowmask fill must have finished on this thread")
}

/// Allocates the pre-encode raw fill, initialized fully visible.
fn allocate_shadowmask_raw_fill(data_len: usize) -> RawFillBuffer {
    #[cfg(test)]
    SHADOWMASK_OUTPUT_ALLOCATION_COUNT.with(|count| count.set(count.get() + 1));
    RawFillBuffer::new(vec![255; data_len])
}

#[cfg(test)]
fn raw_fill_live_bytes_add(bytes: usize) {
    RAW_FILL_LIVE_BYTES.with(|live| live.set(live.get() + bytes));
}

#[cfg(test)]
fn raw_fill_live_bytes_sub(bytes: usize) {
    RAW_FILL_LIVE_BYTES.with(|live| live.set(live.get() - bytes));
}

#[cfg(test)]
fn raw_fill_live_bytes() -> usize {
    RAW_FILL_LIVE_BYTES.with(std::cell::Cell::get)
}

#[cfg(test)]
fn reset_shadowmask_output_allocation_count() {
    SHADOWMASK_OUTPUT_ALLOCATION_COUNT.with(|count| count.set(0));
}

/// Section payload allocations on this thread: a raw fill, or the encoded
/// payload of an empty section, which has no raw fill.
#[cfg(test)]
fn shadowmask_output_allocation_count() -> usize {
    SHADOWMASK_OUTPUT_ALLOCATION_COUNT.with(std::cell::Cell::get)
}

#[cfg(test)]
fn reset_shadowmask_streamed_cache_write_count() {
    SHADOWMASK_STREAMED_CACHE_WRITE_COUNT.with(|count| count.set(0));
}

#[cfg(test)]
fn shadowmask_streamed_cache_write_count() -> usize {
    SHADOWMASK_STREAMED_CACHE_WRITE_COUNT.with(std::cell::Cell::get)
}

/// The valid selected set becomes known only after `AlphaLights` bounds
/// filtering. Keep zero-work stages indeterminate rather than publishing 0/0.
fn publish_shadowmask_total(
    control: &BakeControl,
    valid_selected_light_count: usize,
    shared: &SharedAtlas<'_>,
) {
    let total = shadowmask_progress_total(valid_selected_light_count, shared);
    if total != 0 {
        control.publish_total(total);
    }
}

fn shadowmask_progress_total(valid_selected_light_count: usize, shared: &SharedAtlas<'_>) -> usize {
    if valid_selected_light_count == 0 {
        return 0;
    }
    valid_selected_light_count
        .saturating_mul(shared.placements.len())
        .saturating_add(shared.placements.len())
        .saturating_add(1)
        .saturating_add(valid_selected_light_count)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct AnalyticGraphStats {
    light_chart_pairs: usize,
    candidate_light_chart_pairs: usize,
}

fn build_analytic_overlap_graph(
    selected: &[(usize, u32, &MapLight)],
    shared: &SharedAtlas<'_>,
    geometry: &GeometryResult,
    control: &BakeControl,
) -> OverlapGraph {
    let chart_order: Vec<usize> = (0..shared.placements.len()).collect();
    let (graph, stats) = build_analytic_overlap_graph_in_order(
        selected,
        shared,
        geometry,
        control,
        &chart_order,
        true,
    );
    log::debug!(
        "[ShadowmaskAtlas] analytic graph prune kept {}/{} light-chart pairs; adjacency {} bytes",
        stats.candidate_light_chart_pairs,
        stats.light_chart_pairs,
        graph.storage_bytes()
    );
    graph
}

fn build_analytic_overlap_graph_in_order(
    selected: &[(usize, u32, &MapLight)],
    shared: &SharedAtlas<'_>,
    geometry: &GeometryResult,
    control: &BakeControl,
    chart_order: &[usize],
    prune: bool,
) -> (OverlapGraph, AnalyticGraphStats) {
    let graph = OverlapGraph::new(selected.len());
    let world_aabb = lightmap_layer::geometry_world_aabb(geometry);
    let candidate_pairs = AtomicUsize::new(0);

    chart_order.par_iter().for_each(|&chart_index| {
        // The prune, texel walk, and edge union are one governed chart item.
        let _permit = control.governor().enter();
        let chart = &shared.charts[chart_index];
        let chart_aabb = chart_world_aabb(chart);
        let candidates: Vec<usize> = selected
            .iter()
            .enumerate()
            .filter_map(|(compact_index, &(_, _, light))| {
                (!prune || chart_may_receive_light(light, chart_aabb, world_aabb))
                    .then_some(compact_index)
            })
            .collect();
        candidate_pairs.fetch_add(candidates.len(), Ordering::Relaxed);

        let mut covered_lights = Vec::with_capacity(candidates.len());
        lightmap_layer::for_each_light_layer_chart_texel(shared, chart_index, |sample| {
            covered_lights.clear();
            for &compact_index in &candidates {
                let light = selected[compact_index].2;
                if lightmap_bake::light_texel_is_covered(
                    light,
                    sample.world_p,
                    sample.surface_normal,
                ) {
                    covered_lights.push(compact_index);
                }
            }
            graph.record_texel_overlap(covered_lights.len());
            for (position, &a) in covered_lights.iter().enumerate() {
                for &b in &covered_lights[position + 1..] {
                    graph.mark_overlap(a, b);
                }
            }
        });
        control.advance(1);
    });

    let stats = AnalyticGraphStats {
        light_chart_pairs: selected.len().saturating_mul(chart_order.len()),
        candidate_light_chart_pairs: candidate_pairs.load(Ordering::Relaxed),
    };
    (graph, stats)
}

fn chart_world_aabb(chart: &lightmap_bake::Chart) -> Option<(DVec3, DVec3)> {
    if chart.uv_extent[0] <= 0.0 || chart.uv_extent[1] <= 0.0 {
        return None;
    }

    let mut min = DVec3::splat(f64::INFINITY);
    let mut max = DVec3::splat(f64::NEG_INFINITY);
    for u in [chart.uv_min[0], chart.uv_min[0] + chart.uv_extent[0]] {
        for v in [chart.uv_min[1], chart.uv_min[1] + chart.uv_extent[1]] {
            let point = chart.origin + chart.u_axis * u + chart.v_axis * v;
            let point = DVec3::new(point.x as f64, point.y as f64, point.z as f64);
            min = min.min(point);
            max = max.max(point);
        }
    }
    Some((min, max))
}

fn chart_may_receive_light(
    light: &MapLight,
    chart_aabb: Option<(DVec3, DVec3)>,
    world_aabb: (DVec3, DVec3),
) -> bool {
    let Some((chart_min, chart_max)) = chart_aabb else {
        return false;
    };
    if matches!(light.light_type, LightType::Directional) {
        return true;
    }
    let (mut light_min, mut light_max) = affinity_grid::light_aabb(light, world_aabb);
    // Coverage casts the light origin to f32 before subtracting the f32 chart
    // sample. Pruning must use that same coordinate domain or it can reject a
    // chart that coverage retains at large finite coordinates.
    let coverage_origin = DVec3::new(
        light.origin.x as f32 as f64,
        light.origin.y as f32 as f64,
        light.origin.z as f32 as f64,
    );
    let origin_rounding = coverage_origin - light.origin;
    light_min += origin_rounding;
    light_max += origin_rounding;
    chart_min.x <= light_max.x
        && chart_max.x >= light_min.x
        && chart_min.y <= light_max.y
        && chart_max.y >= light_min.y
        && chart_min.z <= light_max.z
        && chart_max.z >= light_min.z
}

/// A whole-section cache hit performs no chart, assignment, or fill work, but
/// still represents the complete stage to the progress handle.
fn advance_shadowmask_total(
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

#[cfg(test)]
mod tests;
