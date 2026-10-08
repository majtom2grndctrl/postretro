//! Lightmap stage orchestration, isolated from the top-level compiler pipeline.
//! See: context/lib/build_pipeline.md §Compiler pipeline.

use std::time::Duration;

use bvh::bvh::Bvh;
use postretro_level_format::entity_shadow_lights::EntityShadowLightsSection;
use postretro_level_format::shadowmask_atlas::ShadowmaskAtlasSection;

use crate::Args;
use crate::bake_control::BakeControl;
use crate::bvh_build::BvhPrimitive;
use crate::cache::{CacheKey, StageCache};
use crate::geometry::GeometryResult;
use crate::light_namespaces::{AlphaLightsNs, StaticBakedEntry, StaticBakedLights};
use crate::lightmap_bake::{self, LightmapBakeOutput, LightmapConfig, PreparedAtlas};
use crate::lightmap_layer::{self, SharedAtlas};
use crate::map_data::{LightType, MapLight, ShadowType};
use crate::shadowmask_bake;
use glam::Vec3;

mod window;

#[cfg(test)]
pub(crate) use window::WindowProbe;
use window::{LIGHTMAP_PARTITION_WINDOW, PartitionSource};

pub(crate) struct FusedLightingOutput {
    pub lightmap: LightmapBakeOutput,
    pub shadowmask: Option<ShadowmaskAtlasSection>,
    pub shadowmask_elapsed: Duration,
    pub shadowmask_overlap: shadowmask_bake::ShadowmaskOverlapReport,
}

/// Bytes per bake-layer texel of the warm per-layer accumulator
/// (`lightmap_layer::IncrementalLayerAccumulator`): its `CompositedAtlas`
/// plane (RGBA f32 irradiance, `Vec3` direction, `bool` coverage = 29, the
/// whole cold plane) plus `weighted_dir` and `fallback_normal` (`Vec3` each)
/// and `chart_index` (`u32`). One layer is live at a time.
const LAYER_PLANE_BYTES_PER_TEXEL: u64 = (4 * 4 + 12 + 1) + 12 + 12 + 4;
/// Shadowmask raw fill (`shadowmask_bake::allocate_shadowmask_raw_fill`):
/// four mask slots per texel of every bake layer, empty layer area included,
/// live for the whole layer loop.
const SHADOWMASK_FILL_BYTES_PER_TEXEL: u64 = 4;
/// One resident light partition: at most one 8-byte record per bake-layer
/// texel (charts never overlap), held twice at its peak — the per-chart
/// buffers beside the assembled records, or the records beside their
/// serialized cache copy.
const PARTITION_BYTES_PER_TEXEL: u64 = 2 * std::mem::size_of::<lightmap_layer::LayerTexel>() as u64;

/// The lightmap stage's predicted working-set peak, from the prepared layout
/// alone: the shadowmask fill, one layer plane, the light window's resident
/// partitions, and the encoded sections.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct PredictedLightmapPeak {
    pub(super) shadowmask_fill: u64,
    pub(super) layer_plane: u64,
    /// Every partition the light window may hold at once.
    pub(super) partitions: u64,
    /// Encoded ids 22 and 42, twice: the section and its cache copy.
    pub(super) sections: u64,
}

impl PredictedLightmapPeak {
    pub(super) fn total(&self) -> u64 {
        self.shadowmask_fill + self.layer_plane + self.partitions + self.sections
    }
}

pub(super) fn predicted_peak(
    prepared: &PreparedAtlas,
    uncompressed_irradiance: bool,
    partition_window: usize,
) -> PredictedLightmapPeak {
    let layer_texels = u64::from(prepared.atlas_width) * u64::from(prepared.atlas_height);
    let scale = u64::from(prepared.layout.direction_texel_scale.max(1));
    let irradiance_bytes_per_texel = if uncompressed_irradiance { 8 } else { 1 };
    let encoded: u64 = prepared
        .layout
        .blocks
        .iter()
        .map(|block| {
            let (w, h) = (u64::from(block.width), u64::from(block.height));
            // Irradiance, direction (Rg8 at the reduced scale), and both BC5
            // shadowmask groups.
            w * h * irradiance_bytes_per_texel + (w / scale) * (h / scale) * 2 + 2 * w * h
        })
        .sum();
    PredictedLightmapPeak {
        shadowmask_fill: SHADOWMASK_FILL_BYTES_PER_TEXEL
            * layer_texels
            * u64::from(prepared.layer_count),
        layer_plane: LAYER_PLANE_BYTES_PER_TEXEL * layer_texels,
        partitions: PARTITION_BYTES_PER_TEXEL * layer_texels * partition_window as u64,
        sections: 2 * encoded,
    }
}

/// `--verbose`: the predicted peak, for comparison with a measured RSS.
pub(super) fn log_predicted_peak(prepared: &PreparedAtlas, uncompressed_irradiance: bool) {
    const MIB: f64 = 1024.0 * 1024.0;
    let peak = predicted_peak(prepared, uncompressed_irradiance, LIGHTMAP_PARTITION_WINDOW);
    log::info!(
        "[Compiler] lightmap stage predicted peak {:.0} MiB: shadowmask fill {:.0} + layer plane {:.0} + {} partitions {:.0} + sections {:.0} ({} blocks on {} bake layers of {}²)",
        peak.total() as f64 / MIB,
        peak.shadowmask_fill as f64 / MIB,
        peak.layer_plane as f64 / MIB,
        LIGHTMAP_PARTITION_WINDOW,
        peak.partitions as f64 / MIB,
        peak.sections as f64 / MIB,
        prepared.layout.blocks.len(),
        prepared.layer_count,
        prepared.atlas_width,
    );
}

/// How the fused walk windows its (layer, light) partitions. Production uses
/// [`LIGHTMAP_PARTITION_WINDOW`]; tests pick a size and attach probes.
#[derive(Clone)]
pub(crate) struct PartitionWindow {
    pub(crate) size: usize,
    #[cfg(test)]
    pub(crate) probe: Option<WindowProbe>,
    #[cfg(test)]
    pub(crate) hooks: PartitionHooks,
}

impl Default for PartitionWindow {
    fn default() -> Self {
        Self {
            size: LIGHTMAP_PARTITION_WINDOW,
            #[cfg(test)]
            probe: None,
            #[cfg(test)]
            hooks: PartitionHooks::default(),
        }
    }
}

/// A test hold at one chart bake: (light position, layer, chart index).
#[cfg(test)]
pub(crate) type ChartHook = std::sync::Arc<dyn Fn(usize, u32, usize) + Send + Sync>;

/// A test hold at one partition cache get or put: (light position, layer).
#[cfg(test)]
pub(crate) type PartitionIoHook = std::sync::Arc<dyn Fn(usize, u32) + Send + Sync>;

/// A test hold inside the consumer's permit, before it folds: (item index).
#[cfg(test)]
pub(crate) type ConsumeHook = std::sync::Arc<dyn Fn(usize) + Send + Sync>;

/// Test holds at a partition's chart bake and cache I/O, and in the consumer.
/// The chart and I/O hooks receive the light's position in the stage's
/// layer-light list and the target layer.
#[cfg(test)]
#[derive(Clone, Default)]
pub(crate) struct PartitionHooks {
    pub(crate) before_chart: Option<ChartHook>,
    pub(crate) before_get: Option<PartitionIoHook>,
    pub(crate) before_put: Option<PartitionIoHook>,
    pub(crate) in_consume: Option<ConsumeHook>,
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn bake_fused_prepared(
    args: &Args,
    stage_cache: Option<&StageCache>,
    lightmap_control: &BakeControl,
    shadowmask_control: &BakeControl,
    geometry: &mut GeometryResult,
    static_lights: &StaticBakedLights<'_>,
    alpha_lights: &AlphaLightsNs<'_>,
    shadow_selection: Option<&EntityShadowLightsSection>,
    bvh: &Bvh<f32, 3>,
    primitives: &[BvhPrimitive],
    config: &LightmapConfig,
    prepared: PreparedAtlas,
) -> anyhow::Result<FusedLightingOutput> {
    bake_fused_windowed(
        args,
        stage_cache,
        lightmap_control,
        shadowmask_control,
        geometry,
        static_lights,
        alpha_lights,
        shadow_selection,
        bvh,
        primitives,
        config,
        prepared,
        &PartitionWindow::default(),
    )
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn bake_fused_windowed(
    args: &Args,
    stage_cache: Option<&StageCache>,
    lightmap_control: &BakeControl,
    shadowmask_control: &BakeControl,
    geometry: &mut GeometryResult,
    static_lights: &StaticBakedLights<'_>,
    alpha_lights: &AlphaLightsNs<'_>,
    shadow_selection: Option<&EntityShadowLightsSection>,
    bvh: &Bvh<f32, 3>,
    primitives: &[BvhPrimitive],
    config: &LightmapConfig,
    prepared: PreparedAtlas,
    window: &PartitionWindow,
) -> anyhow::Result<FusedLightingOutput> {
    let density = config.lightmap_density;
    let shared = SharedAtlas {
        charts: &prepared.charts,
        placements: &prepared.placements,
        atlas_width: prepared.atlas_width,
        atlas_height: prepared.atlas_height,
        layout: &prepared.layout,
    };
    // P1: the shadowmask whole-section memo and channel assignment are both
    // resolved before the lightmap memo can choose to skip the walk.
    let mut shadowmask = shadowmask_bake::prepare_fused_shadowmask(
        shadow_selection,
        alpha_lights,
        &shared,
        primitives,
        geometry,
        density,
        args.soft_shadow_samples,
        stage_cache,
        shadowmask_control,
    )?;

    if static_lights.is_empty() || prepared.placements.is_empty() {
        let shadowmask_bake::FusedShadowmaskOutput {
            section: shadowmask,
            elapsed: shadowmask_elapsed,
            overlap: shadowmask_overlap,
        } = shadowmask.finish();
        return Ok(FusedLightingOutput {
            lightmap: LightmapBakeOutput {
                section: postretro_level_format::lightmap::LightmapSection::empty(
                    prepared.layout.direction_texel_scale,
                ),
                charts: prepared.charts,
                placements: prepared.placements,
                atlas_width: prepared.atlas_width,
                atlas_height: prepared.atlas_height,
                layer_count: prepared.layer_count,
                layout: prepared.layout,
            },
            shadowmask,
            shadowmask_elapsed,
            shadowmask_overlap,
        });
    }

    let layer_lights: Vec<_> = static_lights
        .entries()
        .iter()
        .filter(|entry| entry.light.shadow_type != ShadowType::Sdf)
        .collect();
    let total = prepared.placements.len().saturating_mul(layer_lights.len());
    lightmap_control.publish_total(total);

    // Partition keys feed only the cache, so an uncached bake hashes nothing.
    let layer_input_hashes = if stage_cache.is_some() {
        layer_key_hashes(
            &layer_lights,
            &shared,
            primitives,
            geometry,
            density,
            args.soft_shadow_samples,
            prepared.layer_count,
            lightmap_control,
        )
    } else {
        Vec::new()
    };
    let section_key = stage_cache.map(|_| {
        let input_hash = lightmap_layer::section_input_hash(
            &layer_input_hashes,
            &shared,
            density,
            config.uncompressed_irradiance,
            config.direction_texel_scale,
        );
        CacheKey::new(
            "lightmap_section",
            lightmap_layer::LIGHTMAP_SECTION_VERSION,
            &input_hash,
        )
    });
    let cached_section = stage_cache
        .zip(section_key.as_ref())
        .and_then(|(cache, key)| cache.get(key))
        .and_then(|bytes| {
            match postretro_level_format::lightmap::LightmapSection::from_bytes(&bytes) {
                Ok(section) => match lightmap_layer::validate_cached_lightmap_section(
                    &section,
                    &shared,
                    config.uncompressed_irradiance,
                ) {
                    Ok(()) => Some(section),
                    Err(reason) => {
                        log::warn!(
                            "[Compiler] lightmap_section cache entry does not match current atlas ({reason}), recomposing"
                        );
                        None
                    }
                },
                Err(err) => {
                    log::warn!("[Compiler] corrupt lightmap section, recomposing: {err}");
                    None
                }
            }
        });
    let compose_lightmap = cached_section.is_none();
    if let Some(cache) = stage_cache {
        log::info!(
            "[cache] lightmap_section {}",
            if compose_lightmap { "miss" } else { "hit" }
        );
        if !compose_lightmap {
            // The memo stands in for every partition it summarizes; the next
            // prune must keep them as the recompose fallback for a light edit.
            for hash in &layer_input_hashes {
                cache.mark_used(&CacheKey::new(
                    "lightmap_layer",
                    lightmap_layer::LAYER_FORMAT_VERSION,
                    hash,
                ));
            }
        }
    }

    let section = if let Some(section) = cached_section {
        section
    } else {
        // Every bake layer is folded, dilated, encoded, and sliced into its
        // blocks before the next layer's first partition is folded (that
        // layer's partitions may already be baking). With no direct
        // layer-bearing light (all static lights resolve through the SDF) the
        // fold is empty and every block encodes zero irradiance and neutral
        // direction.
        let mut builder = lightmap_bake::BlockSectionBuilder::new(
            &prepared.layout,
            config.uncompressed_irradiance,
        );
        if layer_lights.is_empty() {
            for target_layer in 0..prepared.layer_count {
                // Dilate and whole-layer encode are CPU work bounded by `-j`,
                // like every other layer close.
                let _permit = lightmap_control.governor().enter();
                let mut plane =
                    lightmap_layer::empty_composite(prepared.atlas_width, prepared.atlas_height);
                plane.dilate();
                builder.push_layer(target_layer, &plane);
            }
        } else {
            // Items run layer-major in global light order, the fold order
            // `IncrementalLayerAccumulator::fold_partition` requires. One layer
            // plane is live: the consumer closes a layer when the first item of
            // the next one arrives.
            let items: Vec<(u32, usize)> = (0..prepared.layer_count)
                .flat_map(|layer| (0..layer_lights.len()).map(move |light| (layer, light)))
                .collect();
            let source = LayerPartitions::new(
                args,
                stage_cache,
                lightmap_control,
                geometry,
                bvh,
                primitives,
                &shared,
                &layer_lights,
                &layer_input_hashes,
                &items,
                window,
            );
            let mut open: Option<(u32, lightmap_layer::IncrementalLayerAccumulator)> = None;
            let mut close_layer =
                |(layer, accumulator): (u32, lightmap_layer::IncrementalLayerAccumulator)| {
                    let mut plane = accumulator.finish();
                    plane.dilate();
                    builder.push_layer(layer, &plane);
                };
            window::consume_in_order(
                &source,
                lightmap_control.governor(),
                items.len(),
                window.size,
                |item, partition| {
                    // The consumer runs on whichever worker holds its lock, beside
                    // up to `permits` chart tasks. Its fold, dilate, and
                    // whole-layer encode are CPU work bounded by `-j`, not ray
                    // work; without a permit, `-j 1` would run two busy cores.
                    // It waits only for a free permit, never on a chart task, and
                    // no chart task waits on it; the permit drops on return,
                    // before the window admits new items.
                    let _permit = lightmap_control.governor().enter();
                    #[cfg(test)]
                    if let Some(hook) = &window.hooks.in_consume {
                        hook(item);
                    }
                    let (layer, light) = items[item];
                    if open
                        .as_ref()
                        .is_none_or(|(open_layer, _)| *open_layer != layer)
                    {
                        if let Some(done) = open.take() {
                            close_layer(done);
                        }
                        open = Some((
                            layer,
                            lightmap_layer::IncrementalLayerAccumulator::for_atlas_layer(
                                &shared, layer,
                            ),
                        ));
                    }
                    let entry = layer_lights[light];
                    let (_, accumulator) = open.as_mut().expect("a layer accumulator is open");
                    accumulator.fold_partition(entry.light, &partition, &shared);
                    shadowmask.consume_partition(entry.source_index, &partition);
                },
                #[cfg(test)]
                window.probe.clone(),
            );
            if let Some(done) = open.take() {
                // The last layer closes after the window drains: one permit,
                // as for every close the consumer runs.
                let _permit = lightmap_control.governor().enter();
                close_layer(done);
            }
        }
        builder.finish()
    };

    // If the lightmap memo hit but the shadowmask memo missed, consume only the
    // selected partitions. They are read/baked once here and never by a later
    // pipeline stage.
    if !compose_lightmap {
        let items: Vec<(u32, usize)> = (0..prepared.layer_count)
            .flat_map(|layer| (0..layer_lights.len()).map(move |light| (layer, light)))
            .filter(|&(_, light)| shadowmask.needs_source(layer_lights[light].source_index))
            .collect();
        let source = LayerPartitions::new(
            args,
            stage_cache,
            lightmap_control,
            geometry,
            bvh,
            primitives,
            &shared,
            &layer_lights,
            &layer_input_hashes,
            &items,
            window,
        );
        let completed_work: usize = items
            .iter()
            .map(|&(layer, _)| source.layer_charts[layer as usize].len())
            .sum();
        window::consume_in_order(
            &source,
            lightmap_control.governor(),
            items.len(),
            window.size,
            |item, partition| {
                // Permitted for the same reason as the compose pass's consumer.
                let _permit = lightmap_control.governor().enter();
                #[cfg(test)]
                if let Some(hook) = &window.hooks.in_consume {
                    hook(item);
                }
                let (_, light) = items[item];
                shadowmask.consume_partition(layer_lights[light].source_index, &partition);
            },
            #[cfg(test)]
            window.probe.clone(),
        );
        // This pass consumes only shadowmask-selected partitions, so the rest
        // of the published `total` is completed here to leave progress at 100%.
        if completed_work < total {
            lightmap_control.advance(total - completed_work);
        }
    }

    if compose_lightmap && let (Some(cache), Some(key)) = (stage_cache, section_key.as_ref()) {
        cache.put(key, &section.to_bytes());
    }
    let shadowmask_bake::FusedShadowmaskOutput {
        section: shadowmask,
        elapsed: shadowmask_elapsed,
        overlap: shadowmask_overlap,
    } = shadowmask.finish();
    Ok(FusedLightingOutput {
        lightmap: LightmapBakeOutput {
            section,
            charts: prepared.charts,
            placements: prepared.placements,
            atlas_width: prepared.atlas_width,
            atlas_height: prepared.atlas_height,
            layer_count: prepared.layer_count,
            layout: prepared.layout,
        },
        shadowmask,
        shadowmask_elapsed,
        shadowmask_overlap,
    })
}

/// Every (layer, light) partition key, layer-major in global light order. The
/// atlas fingerprint is hashed once and each light's prefix once; lights hash
/// in parallel, each as one governed item.
#[allow(clippy::too_many_arguments)]
fn layer_key_hashes(
    layer_lights: &[&StaticBakedEntry<'_>],
    shared: &SharedAtlas<'_>,
    primitives: &[BvhPrimitive],
    geometry: &GeometryResult,
    density: f32,
    area_sample_count: u32,
    layer_count: u32,
    control: &BakeControl,
) -> Vec<[u8; 32]> {
    use rayon::prelude::*;
    let context =
        lightmap_layer::LayerKeyContext::new(shared, geometry, density, area_sample_count);
    let prefixes: Vec<blake3::Hasher> = layer_lights
        .par_iter()
        .map(|entry| {
            let _permit = control.governor().enter();
            context.light_prefix(entry.light, primitives, geometry)
        })
        .collect();
    (0..layer_count)
        .flat_map(|layer| {
            prefixes
                .iter()
                .map(move |prefix| lightmap_layer::LayerKeyContext::layer_hash(prefix, layer))
        })
        .collect()
}

/// World bounds of a chart's texel centres, padded by
/// `AABB_PADDING_METERS` to absorb `f32` rounding in the texel walk. The pad
/// (0.5 m, borrowed from `affinity_grid`) is deliberately generous and only
/// makes the reach cull more conservative; a smaller pad would cull more, and
/// must still exceed the `f32` rounding error of the texel positions.
fn chart_texel_bounds(chart: &lightmap_bake::Chart) -> Option<(Vec3, Vec3)> {
    if chart.uv_extent[0] <= 0.0 || chart.uv_extent[1] <= 0.0 {
        return None;
    }
    // Texel centres are affine in (tx, ty), so the corner texels bound them.
    let (width, height) = crate::chart_raster::chart_interior_dims(chart);
    let corners = [
        (0, 0),
        (width - 1, 0),
        (0, height - 1),
        (width - 1, height - 1),
    ]
    .map(|(tx, ty)| crate::chart_raster::chart_texel_world_position(chart, tx, ty));
    let pad = Vec3::splat(crate::affinity_grid::AABB_PADDING_METERS);
    let min = corners
        .iter()
        .copied()
        .fold(Vec3::splat(f32::INFINITY), Vec3::min);
    let max = corners
        .iter()
        .copied()
        .fold(Vec3::splat(f32::NEG_INFINITY), Vec3::max);
    Some((min - pad, max + pad))
}

/// Whether any texel inside `bounds` may receive a direct term from `light`.
/// Every falloff model is zero beyond `falloff_range`, so a point or spot
/// light reaches no texel farther than that; a directional light reaches all.
/// A chart with no texels is never reached.
fn light_may_reach(light: &MapLight, bounds: Option<(Vec3, Vec3)>) -> bool {
    let Some((min, max)) = bounds else {
        return false;
    };
    match light.light_type {
        LightType::Directional => true,
        LightType::Point | LightType::Spot => {
            let origin = light.origin.as_vec3();
            let nearest = origin.clamp(min, max);
            origin.distance(nearest) <= light.falloff_range.max(1.0e-4)
        }
    }
}

/// The fused walk's partitions: one (layer, light) per item, loaded from or
/// written to the per-light layer cache.
struct LayerPartitions<'a> {
    args: &'a Args,
    cache: Option<&'a StageCache>,
    control: &'a BakeControl,
    geometry: &'a GeometryResult,
    bvh: &'a Bvh<f32, 3>,
    primitives: &'a [BvhPrimitive],
    shared: &'a SharedAtlas<'a>,
    layer_lights: &'a [&'a StaticBakedEntry<'a>],
    layer_input_hashes: &'a [[u8; 32]],
    items: &'a [(u32, usize)],
    /// Each bake layer's charts in placement order, built once.
    layer_charts: Vec<Vec<usize>>,
    /// Padded world bounds of each chart's texels; `None` for a chart with no
    /// texels.
    chart_bounds: Vec<Option<(Vec3, Vec3)>>,
    #[cfg(test)]
    hooks: PartitionHooks,
}

impl<'a> LayerPartitions<'a> {
    #[allow(clippy::too_many_arguments)]
    fn new(
        args: &'a Args,
        cache: Option<&'a StageCache>,
        control: &'a BakeControl,
        geometry: &'a GeometryResult,
        bvh: &'a Bvh<f32, 3>,
        primitives: &'a [BvhPrimitive],
        shared: &'a SharedAtlas<'a>,
        layer_lights: &'a [&'a StaticBakedEntry<'a>],
        layer_input_hashes: &'a [[u8; 32]],
        items: &'a [(u32, usize)],
        #[cfg_attr(not(test), allow(unused_variables))] window: &PartitionWindow,
    ) -> Self {
        let mut layer_charts = vec![Vec::new(); lightmap_layer::atlas_layer_count(shared) as usize];
        for (face_idx, placement) in shared.placements.iter().enumerate() {
            layer_charts[placement.layer as usize].push(face_idx);
        }
        let chart_bounds = shared.charts.iter().map(chart_texel_bounds).collect();
        Self {
            args,
            cache,
            control,
            geometry,
            bvh,
            primitives,
            shared,
            layer_lights,
            layer_input_hashes,
            items,
            layer_charts,
            chart_bounds,
            #[cfg(test)]
            hooks: window.hooks.clone(),
        }
    }

    fn key(&self, item: usize) -> CacheKey {
        let (layer, light) = self.items[item];
        let hash = &self.layer_input_hashes[layer as usize * self.layer_lights.len() + light];
        CacheKey::new("lightmap_layer", lightmap_layer::LAYER_FORMAT_VERSION, hash)
    }
}

impl PartitionSource for LayerPartitions<'_> {
    fn load(&self, item: usize) -> Option<lightmap_layer::LightmapLayer> {
        let cache = self.cache?;
        let (target_layer, _light) = self.items[item];
        #[cfg(test)]
        if let Some(hook) = &self.hooks.before_get {
            hook(_light, target_layer);
        }
        let partition = cache
            .get(&self.key(item))
            .and_then(|bytes| lightmap_layer::LightmapLayer::from_bytes(&bytes))
            .and_then(|partition| {
                match lightmap_layer::validate_layer_partition(
                    &partition,
                    self.shared,
                    target_layer,
                ) {
                    Ok(()) => Some(partition),
                    Err(reason) => {
                        log::warn!(
                            "[Compiler] lightmap_layer cache entry does not match target layer {target_layer} ({reason}), re-baking"
                        );
                        None
                    }
                }
            });
        if self.args.verbose {
            log::info!(
                "[cache] lightmap_layer {}",
                if partition.is_some() { "hit" } else { "miss" }
            );
        }
        if partition.is_some() {
            self.control
                .advance(self.layer_charts[target_layer as usize].len());
        }
        partition
    }

    fn plan_charts(&self, item: usize) -> Vec<usize> {
        let (layer, light) = self.items[item];
        let layer_charts = &self.layer_charts[layer as usize];
        let light = self.layer_lights[light].light;
        let reached: Vec<usize> = layer_charts
            .iter()
            .copied()
            .filter(|&chart| light_may_reach(light, self.chart_bounds[chart]))
            .collect();
        // A culled chart bakes no texel, so it completes here.
        self.control.advance(layer_charts.len() - reached.len());
        reached
    }

    fn bake_chart(&self, item: usize, chart: usize) -> Vec<lightmap_layer::LayerTexel> {
        let (_layer, light) = self.items[item];
        #[cfg(test)]
        if let Some(hook) = &self.hooks.before_chart {
            hook(light, _layer, chart);
        }
        lightmap_layer::bake_light_layer_chart_controlled(
            self.layer_lights[light].light,
            self.shared,
            chart,
            self.bvh,
            self.primitives,
            self.geometry,
            self.args.soft_shadow_samples,
            self.control,
        )
    }

    fn finish(
        &self,
        item: usize,
        mut texels: Vec<lightmap_layer::LayerTexel>,
    ) -> lightmap_layer::LightmapLayer {
        let (target_layer, _light) = self.items[item];
        texels.sort_unstable_by_key(|texel| texel.idx);
        let partition = lightmap_layer::LightmapLayer {
            atlas_width: self.shared.atlas_width,
            atlas_height: self.shared.atlas_height,
            layer_count: lightmap_layer::atlas_layer_count(self.shared),
            target_layer,
            texels,
        };
        if let Some(cache) = self.cache {
            #[cfg(test)]
            if let Some(hook) = &self.hooks.before_put {
                hook(_light, target_layer);
            }
            cache.put(&self.key(item), &partition.to_bytes());
        }
        partition
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use glam::DVec3;
    use log::Level;
    use postretro_level_format::geometry::{FaceMeta, GeometrySection, Vertex};
    use postretro_level_format::shadowmask_atlas::SHADOWMASK_CHANNEL_DROPPED;
    use postretro_level_format::texture_names::TextureNamesSection;
    use postretro_test_log_capture::LogCapture;
    use rayon::ThreadPoolBuilder;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicU64, Ordering};

    use crate::bvh_build::build_bvh;
    use crate::geometry::FaceIndexRange;
    use crate::governor::Governor;
    use crate::map_data::FalloffModel;
    use crate::reporter::StageProgress;

    fn fresh_cache_dir(label: &str) -> std::path::PathBuf {
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        let dir = std::env::temp_dir().join(format!(
            "postretro_fused_lighting_cache_{label}_{}_{}",
            std::process::id(),
            COUNTER.fetch_add(1, Ordering::Relaxed),
        ));
        let _ = std::fs::remove_dir_all(&dir);
        dir
    }

    fn quad_geometry() -> GeometryResult {
        let normal = [0.0, 1.0, 0.0];
        let tangent = [1.0, 0.0, 0.0];
        GeometryResult {
            geometry: GeometrySection {
                vertices: vec![
                    Vertex::new(
                        [0.0, 0.0, 0.0],
                        [0.0, 0.0],
                        normal,
                        tangent,
                        true,
                        [0.0, 0.0],
                        0,
                    ),
                    Vertex::new(
                        [1.0, 0.0, 0.0],
                        [1.0, 0.0],
                        normal,
                        tangent,
                        true,
                        [0.0, 0.0],
                        0,
                    ),
                    Vertex::new(
                        [1.0, 0.0, 1.0],
                        [1.0, 1.0],
                        normal,
                        tangent,
                        true,
                        [0.0, 0.0],
                        0,
                    ),
                    Vertex::new(
                        [0.0, 0.0, 1.0],
                        [0.0, 1.0],
                        normal,
                        tangent,
                        true,
                        [0.0, 0.0],
                        0,
                    ),
                ],
                indices: vec![0, 1, 2, 0, 2, 3],
                faces: vec![FaceMeta {
                    leaf_index: 0,
                    texture_index: 0,
                }],
            },
            texture_names: TextureNamesSection { names: Vec::new() },
            face_index_ranges: vec![FaceIndexRange {
                index_offset: 0,
                index_count: 6,
            }],
        }
    }

    /// Two large, separate leaves that each fit one 64² atlas layer but do
    /// not fit together. The fused-path matrix relies on production atlas
    /// preparation assigning the second face to layer 1.
    fn two_layer_geometry() -> GeometryResult {
        let mut first = quad_geometry();
        for vertex in &mut first.geometry.vertices {
            vertex.position[0] *= 12.0;
            vertex.position[2] *= 12.0;
        }

        let mut second = quad_geometry();
        for vertex in &mut second.geometry.vertices {
            vertex.position[0] = vertex.position[0] * 12.0 + 16.0;
            vertex.position[2] *= 12.0;
        }
        second.geometry.faces[0].leaf_index = 1;

        let vertex_offset = first.geometry.vertices.len() as u32;
        let index_offset = first.geometry.indices.len() as u32;
        first.geometry.vertices.extend(second.geometry.vertices);
        first.geometry.indices.extend(
            second
                .geometry
                .indices
                .into_iter()
                .map(|index| index + vertex_offset),
        );
        first.geometry.faces.extend(second.geometry.faces);
        first
            .face_index_ranges
            .extend(second.face_index_ranges.into_iter().map(|mut range| {
                range.index_offset += index_offset;
                range
            }));
        first
    }

    fn point_light(origin: DVec3, color: [f32; 3]) -> MapLight {
        MapLight {
            origin,
            carrier: String::new(),
            light_type: LightType::Point,
            intensity: 1.0,
            color,
            falloff_model: FalloffModel::Linear,
            falloff_range: 5.0,
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

    fn test_args() -> Args {
        crate::parse_args_from(
            ["fixture.map", "--verbose", "--soft-shadow-samples", "4"]
                .into_iter()
                .map(str::to_owned),
        )
        .expect("test arguments must parse")
    }

    fn config(uncompressed_irradiance: bool) -> LightmapConfig {
        LightmapConfig {
            lightmap_density: 0.25,
            area_sample_count: 4,
            uncompressed_irradiance,
            direction_texel_scale: lightmap_bake::DIRECTION_TEXEL_SCALE,
        }
    }

    fn run_fused(
        args: &Args,
        cache: Option<&StageCache>,
        lights: &[MapLight],
        selection: Option<&EntityShadowLightsSection>,
        config: &LightmapConfig,
        lightmap_control: &BakeControl,
        shadowmask_control: &BakeControl,
    ) -> FusedLightingOutput {
        run_fused_with_geometry(
            args,
            cache,
            lights,
            selection,
            config,
            lightmap_control,
            shadowmask_control,
            quad_geometry(),
        )
    }

    #[allow(clippy::too_many_arguments)]
    fn run_fused_with_geometry(
        args: &Args,
        cache: Option<&StageCache>,
        lights: &[MapLight],
        selection: Option<&EntityShadowLightsSection>,
        config: &LightmapConfig,
        lightmap_control: &BakeControl,
        shadowmask_control: &BakeControl,
        mut geometry: GeometryResult,
    ) -> FusedLightingOutput {
        let (bvh, primitives, _) = build_bvh(&geometry).expect("fixture BVH must build");
        let static_lights = StaticBakedLights::from_lights(lights);
        let alpha_lights = AlphaLightsNs::from_lights(lights);
        let prepared = lightmap_bake::prepare_atlas(
            &mut geometry,
            &static_lights,
            config.lightmap_density,
            &[],
        )
        .expect("fixture atlas must prepare");
        bake_fused_prepared(
            args,
            cache,
            lightmap_control,
            shadowmask_control,
            &mut geometry,
            &static_lights,
            &alpha_lights,
            selection,
            &bvh,
            &primitives,
            config,
            prepared,
        )
        .expect("fused fixture bake must succeed")
    }

    fn fused_outputs(
        args: &Args,
        cache: Option<&StageCache>,
        lights: &[MapLight],
        selection: &EntityShadowLightsSection,
        config: &LightmapConfig,
    ) -> (Vec<u8>, Vec<u8>, u32) {
        let output = run_fused_with_geometry(
            args,
            cache,
            lights,
            Some(selection),
            config,
            &BakeControl::unrestricted(),
            &BakeControl::unrestricted(),
            two_layer_geometry(),
        );
        let shadowmask = output
            .shadowmask
            .expect("selected fixture light must emit a shadowmask");
        (
            output.lightmap.section.to_bytes(),
            shadowmask.to_bytes(),
            output.lightmap.layer_count,
        )
    }

    /// (lightmap bytes, shadowmask bytes, layer count), chart total, checkpoint count.
    type FusedOutputsWithWorkers = ((Vec<u8>, Vec<u8>, u32), Option<usize>, usize);

    fn fused_outputs_with_workers(
        workers: usize,
        args: &Args,
        cache: Option<&StageCache>,
        lights: &[MapLight],
        selection: &EntityShadowLightsSection,
        config: &LightmapConfig,
    ) -> FusedOutputsWithWorkers {
        let progress = StageProgress::indeterminate();
        let governor = Arc::new(Governor::new(workers, false));
        let output = ThreadPoolBuilder::new()
            .num_threads(workers)
            .build()
            .expect("build fused fixture pool")
            .install(|| {
                run_fused_with_geometry(
                    args,
                    cache,
                    lights,
                    Some(selection),
                    config,
                    &BakeControl::new(governor, &progress),
                    &BakeControl::unrestricted(),
                    two_layer_geometry(),
                )
            });
        let shadowmask = output
            .shadowmask
            .expect("selected fixture lights must emit a shadowmask");
        (
            (
                output.lightmap.section.to_bytes(),
                shadowmask.to_bytes(),
                output.lightmap.layer_count,
            ),
            progress.total(),
            progress.completed(),
        )
    }

    fn reference_outputs(
        lights: &[MapLight],
        selection: &EntityShadowLightsSection,
        config: &LightmapConfig,
    ) -> (Vec<u8>, Vec<u8>) {
        let mut geometry = two_layer_geometry();
        let (bvh, primitives, _) = build_bvh(&geometry).expect("fixture BVH must build");
        let static_lights = StaticBakedLights::from_lights(lights);
        let alpha_lights = AlphaLightsNs::from_lights(lights);
        let prepared = lightmap_bake::prepare_atlas(
            &mut geometry,
            &static_lights,
            config.lightmap_density,
            &[],
        )
        .expect("fixture atlas must prepare");
        let mut ctx = lightmap_bake::LightmapBakeCtx {
            bvh: &bvh,
            primitives: &primitives,
            geometry: &mut geometry,
            lights: &static_lights,
            scale_regions: &[],
        };
        let lightmap = lightmap_bake::bake_prepared_lightmap_controlled(
            &mut ctx,
            config,
            prepared,
            &BakeControl::unrestricted(),
        )
        .expect("reference lightmap must bake");
        let shared = SharedAtlas {
            charts: &lightmap.charts,
            placements: &lightmap.placements,
            atlas_width: lightmap.atlas_width,
            atlas_height: lightmap.atlas_height,
            layout: &lightmap.layout,
        };
        let shadowmask = shadowmask_bake::bake_shadowmask_atlas(
            Some(selection),
            &alpha_lights,
            &shared,
            &bvh,
            &primitives,
            &geometry,
            config.area_sample_count,
            &BakeControl::unrestricted(),
        )
        .expect("reference shadowmask must bake");
        (lightmap.section.to_bytes(), shadowmask.to_bytes())
    }

    /// Per-texel mask slots `[g0.r, g0.g, g1.r, g1.g]` of one block.
    fn decode_block_slots(groups: &[Vec<u8>; 2], width: u32, height: u32) -> Vec<[u8; 4]> {
        let a = crate::bc5::decode_bc5_rg(&groups[0], width, height);
        let b = crate::bc5::decode_bc5_rg(&groups[1], width, height);
        a.as_chunks::<2>()
            .0
            .iter()
            .zip(b.as_chunks::<2>().0.iter())
            .map(|(a, b)| [a[0], a[1], b[0], b[1]])
            .collect()
    }

    fn assert_two_selected_channels_overlap_in_block_one(lightmap: &[u8], bytes: &[u8]) {
        let lightmap = postretro_level_format::lightmap::LightmapSection::from_bytes(lightmap)
            .expect("fused lightmap bytes must decode");
        let shadowmask = ShadowmaskAtlasSection::from_bytes(bytes, &lightmap.index())
            .expect("fused shadowmask bytes must decode");
        assert_eq!(shadowmask.blocks.len(), 2);
        assert_ne!(shadowmask.channels[0], SHADOWMASK_CHANNEL_DROPPED);
        assert_ne!(shadowmask.channels[1], SHADOWMASK_CHANNEL_DROPPED);
        assert_ne!(
            shadowmask.channels[0], shadowmask.channels[1],
            "overlapping selected lights must occupy distinct channels"
        );
        let block = &lightmap.blocks[1];
        let texels = decode_block_slots(
            &shadowmask.blocks[1],
            u32::from(block.width),
            u32::from(block.height),
        );
        assert!(
            texels.iter().any(|texel| {
                texel[shadowmask.channels[0] as usize] != 0
                    && texel[shadowmask.channels[1] as usize] != 0
            }),
            "both selected lights must write their assigned channels at one overlapping block-1 texel"
        );
    }

    #[test]
    fn fused_cold_warm_and_selection_only_paths_match_reference_bytes() {
        let args = test_args();
        let lights = vec![
            point_light(DVec3::new(13.0, 8.0, 5.0), [1.0, 0.25, 0.1]),
            point_light(DVec3::new(15.0, 9.0, 7.0), [0.1, 0.35, 1.0]),
            point_light(DVec3::new(14.0, 7.0, 6.0), [0.2, 1.0, 0.3]),
        ];
        let mut lights = lights;
        for light in &mut lights {
            light.falloff_range = 40.0;
        }
        let initial_selection = EntityShadowLightsSection {
            light_indices: vec![0, 1],
        };
        let edited_selection = EntityShadowLightsSection {
            light_indices: vec![1, 0],
        };

        for uncompressed in [false, true] {
            let config = config(uncompressed);
            let reference = reference_outputs(&lights, &initial_selection, &config);
            let (cold, cold_total, cold_completed) =
                fused_outputs_with_workers(1, &args, None, &lights, &initial_selection, &config);
            let (parallel_cold, parallel_total, parallel_completed) =
                fused_outputs_with_workers(4, &args, None, &lights, &initial_selection, &config);
            assert_eq!(cold.2, 2, "fixture must exercise atlas layer 1");
            let expected_partitions = lights.len() * cold.2 as usize;
            assert_eq!(cold_total, Some(expected_partitions));
            assert_eq!(cold_completed, expected_partitions);
            assert_eq!(parallel_total, Some(expected_partitions));
            assert_eq!(parallel_completed, expected_partitions);
            assert_eq!(cold.0, reference.0, "cold fused lightmap bytes changed");
            assert_eq!(cold.1, reference.1, "cold fused shadowmask bytes changed");
            assert_eq!(parallel_cold, cold);

            assert_two_selected_channels_overlap_in_block_one(&cold.0, &cold.1);

            let dir = fresh_cache_dir(if uncompressed { "rgba16f" } else { "bc6h" });
            let cache = StageCache::new(&dir).expect("create fused test cache");
            let warm_miss_logs = LogCapture::start();
            let warm_miss =
                fused_outputs(&args, Some(&cache), &lights, &initial_selection, &config);
            assert_eq!(warm_miss.0, reference.0, "warm miss lightmap mismatch");
            assert_eq!(warm_miss.1, reference.1, "warm miss shadowmask mismatch");
            warm_miss_logs.assert_logged_once(Level::Info, "[cache] shadowmask_atlas miss");
            warm_miss_logs.assert_logged_once(Level::Info, "[cache] lightmap_section miss");
            // Partition loads run on Rayon workers, where log capture does not
            // reach, so partition counts come from the cache itself.
            let layers = cache.test_access("lightmap_layer");
            assert_eq!(
                layers.read_attempts - layers.read_hits,
                expected_partitions,
                "an empty warm cache must bake each light/layer partition exactly once"
            );
            assert_eq!(
                layers.read_hits, 0,
                "an empty warm cache cannot load a lightmap partition"
            );
            drop(warm_miss_logs);
            cache.clear_test_accesses();

            let no_edit_logs = LogCapture::start();
            let warm_hit = fused_outputs(&args, Some(&cache), &lights, &initial_selection, &config);
            assert_eq!(warm_hit, warm_miss, "warm hit must preserve exact bytes");
            no_edit_logs.assert_logged_once(Level::Info, "[cache] shadowmask_atlas hit");
            no_edit_logs.assert_logged_once(Level::Info, "[cache] lightmap_section hit");
            assert_eq!(
                cache.test_access("lightmap_layer").read_attempts,
                0,
                "both memo hits must read no lightmap partition"
            );
            drop(no_edit_logs);
            cache.clear_test_accesses();

            let mut one_light_edit = lights.clone();
            one_light_edit[2].intensity = 0.875;
            let edit_reference = reference_outputs(&one_light_edit, &initial_selection, &config);
            let edit_logs = LogCapture::start();
            let edited = fused_outputs(
                &args,
                Some(&cache),
                &one_light_edit,
                &initial_selection,
                &config,
            );
            assert_eq!(edited.0, edit_reference.0, "one-light edit mismatch");
            assert_eq!(edited.1, edit_reference.1, "warm partition-miss mismatch");
            edit_logs.assert_logged_once(Level::Info, "[cache] shadowmask_atlas hit");
            edit_logs.assert_logged_once(Level::Info, "[cache] lightmap_section miss");
            let layers = cache.test_access("lightmap_layer");
            let layer_hits = layers.read_hits;
            let layer_misses = layers.read_attempts - layers.read_hits;
            assert_eq!(
                layer_hits,
                (lights.len() - 1) * edited.2 as usize,
                "the unedited partitions must each load exactly once"
            );
            assert_eq!(layer_misses, edited.2 as usize);
            drop(edit_logs);
            cache.clear_test_accesses();

            let edited_reference = reference_outputs(&lights, &edited_selection, &config);
            let selection_logs = LogCapture::start();
            let selection_only =
                fused_outputs(&args, Some(&cache), &lights, &edited_selection, &config);
            assert_eq!(
                selection_only.0, edited_reference.0,
                "selection-only edit must retain the lightmap section memo"
            );
            assert_eq!(
                selection_only.1, edited_reference.1,
                "selection-only fused shadowmask must match cold reference bytes"
            );
            assert_two_selected_channels_overlap_in_block_one(&selection_only.0, &selection_only.1);
            selection_logs.assert_logged_once(Level::Info, "[cache] shadowmask_atlas miss");
            selection_logs.assert_logged_once(Level::Info, "[cache] lightmap_section hit");
            let layers = cache.test_access("lightmap_layer");
            assert_eq!(
                layers.read_attempts, layers.read_hits,
                "a selection-only edit must miss no lightmap partition"
            );
            let layer_hits = layers.read_hits;
            assert_eq!(
                layer_hits,
                initial_selection.light_indices.len() * selection_only.2 as usize,
                "selection-only shadow rebuild must read exactly one selected partition per layer"
            );
            drop(selection_logs);
            drop(cache);
            let _ = std::fs::remove_dir_all(&dir);
        }
    }

    #[test]
    fn fused_stage_progress_is_exact_and_cleared_selection_emits_no_shadowmask() {
        let args = test_args();
        let config = config(false);
        let lights = vec![
            point_light(DVec3::new(0.2, 1.0, 0.35), [1.0, 0.25, 0.1]),
            point_light(DVec3::new(0.85, 1.5, 0.75), [0.1, 0.35, 1.0]),
        ];
        let selection = EntityShadowLightsSection {
            light_indices: vec![0],
        };
        let lightmap_progress = StageProgress::indeterminate();
        let shadowmask_progress = StageProgress::indeterminate();
        let governor = Arc::new(Governor::new(4, false));
        let output = run_fused(
            &args,
            None,
            &lights,
            Some(&selection),
            &config,
            &BakeControl::new(Arc::clone(&governor), &lightmap_progress),
            &BakeControl::new(governor, &shadowmask_progress),
        );
        let expected_lightmap = output
            .lightmap
            .placements
            .len()
            .saturating_mul(lights.len());
        let expected_shadowmask = output.lightmap.placements.len().saturating_add(2);
        assert_eq!(lightmap_progress.total(), Some(expected_lightmap));
        assert_eq!(lightmap_progress.completed(), expected_lightmap);
        assert_eq!(shadowmask_progress.total(), Some(expected_shadowmask));
        assert_eq!(shadowmask_progress.completed(), expected_shadowmask);

        let cleared = run_fused(
            &args,
            None,
            &lights,
            None,
            &config,
            &BakeControl::unrestricted(),
            &BakeControl::unrestricted(),
        );
        assert!(
            cleared.shadowmask.is_none(),
            "a selection cleared by the finalized direct-delta seam must fill no channel"
        );
        assert_eq!(
            cleared.lightmap.section.to_bytes(),
            output.lightmap.section.to_bytes(),
            "clearing entity-shadow selection must not change lightmap bytes"
        );
    }

    #[test]
    fn fused_stage_preserves_all_sdf_one_plane_fallback_on_cache_hit() {
        let args = test_args();
        let config = config(false);
        let mut sdf_light = point_light(DVec3::new(0.5, 1.0, 0.5), [1.0; 3]);
        sdf_light.shadow_type = ShadowType::Sdf;
        let lights = vec![sdf_light];
        let dir = fresh_cache_dir("all_sdf");
        let cache = StageCache::new(&dir).expect("create all-SDF test cache");

        let first_progress = StageProgress::indeterminate();
        let first = run_fused(
            &args,
            Some(&cache),
            &lights,
            None,
            &config,
            &BakeControl::new(Arc::new(Governor::new(1, false)), &first_progress),
            &BakeControl::unrestricted(),
        );
        let second_progress = StageProgress::indeterminate();
        let second = run_fused(
            &args,
            Some(&cache),
            &lights,
            None,
            &config,
            &BakeControl::new(Arc::new(Governor::new(1, false)), &second_progress),
            &BakeControl::unrestricted(),
        );
        assert_eq!(first.lightmap.section.blocks.len(), 1);
        assert!(
            first.lightmap.section.blocks[0]
                .direction
                .as_chunks::<2>()
                .0
                .iter()
                .all(|texel| *texel == [128, 255]),
            "an all-SDF bake keeps its block with neutral direction"
        );
        assert_eq!(
            first.lightmap.section.to_bytes(),
            second.lightmap.section.to_bytes()
        );
        assert!(first.shadowmask.is_none());
        assert!(second.shadowmask.is_none());
        assert_eq!(first_progress.total(), Some(0));
        assert_eq!(first_progress.completed(), 0);
        assert_eq!(second_progress.total(), Some(0));
        assert_eq!(second_progress.completed(), 0);

        drop(cache);
        let _ = std::fs::remove_dir_all(dir);
    }

    /// `count` 12 m quads in one cell. At the fixture's 0.25 m/texel each
    /// charts at 52², so under a 64-texel test pool edge the cell takes one
    /// block per quad.
    fn quads_in_one_cell(count: usize) -> GeometryResult {
        let mut geometry = quad_geometry();
        geometry.geometry.vertices.clear();
        geometry.geometry.indices.clear();
        geometry.geometry.faces.clear();
        geometry.face_index_ranges.clear();
        for quad in 0..count {
            let mut next = quad_geometry();
            for vertex in &mut next.geometry.vertices {
                vertex.position[0] = vertex.position[0] * 12.0 + 14.0 * quad as f32;
                vertex.position[2] *= 12.0;
            }
            let vertex_offset = geometry.geometry.vertices.len() as u32;
            let index_offset = geometry.geometry.indices.len() as u32;
            geometry.geometry.vertices.extend(next.geometry.vertices);
            geometry.geometry.indices.extend(
                next.geometry
                    .indices
                    .into_iter()
                    .map(|index| index + vertex_offset),
            );
            geometry.geometry.faces.extend(next.geometry.faces);
            geometry
                .face_index_ranges
                .extend(next.face_index_ranges.into_iter().map(|mut range| {
                    range.index_offset += index_offset;
                    range
                }));
        }
        geometry
    }

    /// (lightmap bytes, shadowmask bytes, block count) of a fused cold bake of
    /// one multi-block cell on `workers` threads.
    fn multi_block_fused_outputs(workers: usize, uncompressed: bool) -> (Vec<u8>, Vec<u8>, usize) {
        let args = test_args();
        let config = config(uncompressed);
        let mut light = point_light(DVec3::new(20.0, 6.0, 6.0), [1.0, 0.5, 0.2]);
        light.falloff_range = 40.0;
        let lights = vec![light];
        let selection = EntityShadowLightsSection {
            light_indices: vec![0],
        };
        let mut geometry = quads_in_one_cell(3);
        let (bvh, primitives, _) = build_bvh(&geometry).expect("fixture BVH must build");
        let static_lights = StaticBakedLights::from_lights(&lights);
        let alpha_lights = AlphaLightsNs::from_lights(&lights);
        let progress = StageProgress::indeterminate();
        let control = BakeControl::new(Arc::new(Governor::new(workers, false)), &progress);
        ThreadPoolBuilder::new()
            .num_threads(workers)
            .build()
            .expect("build fused fixture pool")
            .install(|| {
                let prepared = lightmap_bake::prepare_atlas_within(
                    &mut geometry,
                    &static_lights,
                    config.lightmap_density,
                    &[],
                    lightmap_bake::BlockOrdering::by_cell_id(config.direction_texel_scale),
                    64,
                    &control,
                )
                .expect("multi-block fixture must prepare");
                let blocks = prepared.layout.blocks.len();
                let output = bake_fused_prepared(
                    &args,
                    None,
                    &control,
                    &BakeControl::unrestricted(),
                    &mut geometry,
                    &static_lights,
                    &alpha_lights,
                    Some(&selection),
                    &bvh,
                    &primitives,
                    &config,
                    prepared,
                )
                .expect("multi-block fused bake must succeed");
                (
                    output.lightmap.section.to_bytes(),
                    output
                        .shadowmask
                        .expect("selected light must emit a shadowmask")
                        .to_bytes(),
                    blocks,
                )
            })
    }

    #[test]
    fn multi_block_cell_section_bytes_are_identical_with_one_worker_and_many() {
        for uncompressed in [false, true] {
            let one = multi_block_fused_outputs(1, uncompressed);
            let many = multi_block_fused_outputs(4, uncompressed);
            assert_eq!(one.2, 3, "the fixture cell splits into three blocks");
            let section = postretro_level_format::lightmap::LightmapSection::from_bytes(&one.0)
                .expect("multi-block lightmap decodes");
            assert!(section.blocks.iter().all(|block| block.cell_id == 0));
            assert_eq!(one, many, "uncompressed = {uncompressed}");
        }
    }

    #[test]
    fn predicted_peak_charges_every_bake_layer_one_plane_and_both_sections_twice() {
        let mut geometry = quads_in_one_cell(3);
        let lights = vec![point_light(DVec3::new(20.0, 6.0, 6.0), [1.0; 3])];
        let static_lights = StaticBakedLights::from_lights(&lights);
        let prepared = lightmap_bake::prepare_atlas_within(
            &mut geometry,
            &static_lights,
            0.25,
            &[],
            lightmap_bake::BlockOrdering::by_cell_id(2),
            64,
            &BakeControl::unrestricted(),
        )
        .expect("multi-block fixture must prepare");
        let layer_texels = u64::from(prepared.atlas_width) * u64::from(prepared.atlas_height);
        let peak = predicted_peak(&prepared, false, 1);
        assert_eq!(
            peak.shadowmask_fill,
            4 * layer_texels * u64::from(prepared.layer_count)
        );
        assert_eq!(peak.layer_plane, 57 * layer_texels);
        let block_texels: u64 = prepared
            .layout
            .blocks
            .iter()
            .map(|b| u64::from(b.width) * u64::from(b.height))
            .sum();
        // BC6H 1 B + direction 2 B / scale² + shadowmask 2 B per texel, twice.
        assert_eq!(peak.sections, 2 * (block_texels * 3 + block_texels / 4 * 2));
        assert_eq!(
            peak.total(),
            peak.shadowmask_fill + peak.layer_plane + peak.partitions + peak.sections
        );
    }

    // The window's resident partitions are charged, growing with the window;
    // a window of one adds exactly one partition to the old terms.
    #[test]
    fn predicted_peak_charges_window_partitions() {
        let mut geometry = quads_in_one_cell(3);
        let lights = vec![point_light(DVec3::new(20.0, 6.0, 6.0), [1.0; 3])];
        let static_lights = StaticBakedLights::from_lights(&lights);
        let prepared =
            lightmap_bake::prepare_atlas(&mut geometry, &static_lights, 0.25, &[]).unwrap();
        let layer_texels = u64::from(prepared.atlas_width) * u64::from(prepared.atlas_height);
        let one_partition = 2 * 8 * layer_texels;

        let one = predicted_peak(&prepared, false, 1);
        let without_window = one.shadowmask_fill + one.layer_plane + one.sections;
        assert_eq!(one.total(), without_window + one_partition);
        let mut previous = one.total();
        for window in [2, LIGHTMAP_PARTITION_WINDOW, 4 * LIGHTMAP_PARTITION_WINDOW] {
            let peak = predicted_peak(&prepared, false, window);
            assert!(peak.total() > previous, "window {window}");
            assert_eq!(peak.partitions, window as u64 * one_partition);
            previous = peak.total();
        }
    }

    /// Append a horizontal quad `[x, x + size] × [z, z + size]` at height
    /// `y` as one face of `leaf`.
    fn push_quad(geometry: &mut GeometryResult, x: f32, z: f32, size: f32, y: f32, leaf: u32) {
        let mut quad = quad_geometry();
        for vertex in &mut quad.geometry.vertices {
            vertex.position = [
                x + vertex.position[0] * size,
                y,
                z + vertex.position[2] * size,
            ];
        }
        quad.geometry.faces[0].leaf_index = leaf;
        let vertex_offset = geometry.geometry.vertices.len() as u32;
        let index_offset = geometry.geometry.indices.len() as u32;
        geometry.geometry.vertices.extend(quad.geometry.vertices);
        geometry.geometry.indices.extend(
            quad.geometry
                .indices
                .into_iter()
                .map(|index| index + vertex_offset),
        );
        geometry.geometry.faces.extend(quad.geometry.faces);
        geometry
            .face_index_ranges
            .extend(quad.face_index_ranges.into_iter().map(|mut range| {
                range.index_offset += index_offset;
                range
            }));
    }

    /// A 6 m floor under a 2 m occluder in cell 1, so an area light casts a
    /// penumbra. With `far_quad`, an unlit 3 m quad in cell 0 precedes them:
    /// it renumbers both faces and takes the bake layer's corner, so cell 1's
    /// block lands beside it at other bake-layer coordinates.
    fn penumbra_cell_geometry(far_quad: bool) -> GeometryResult {
        let mut geometry = quad_geometry();
        geometry.geometry.vertices.clear();
        geometry.geometry.indices.clear();
        geometry.geometry.faces.clear();
        geometry.face_index_ranges.clear();
        if far_quad {
            push_quad(&mut geometry, 500.0, 500.0, 3.0, 0.0, 0);
        }
        push_quad(&mut geometry, 0.0, 0.0, 6.0, 0.0, 1);
        push_quad(&mut geometry, 2.0, 2.0, 2.0, 1.0, 1);
        geometry
    }

    /// Soft-visibility seeds key on the chart through the shipping per-light
    /// layer walk too: the fused stage bakes a moved, renumbered floor's
    /// penumbra into identical lightmap and shadowmask block bytes.
    #[test]
    fn fused_bake_of_a_moved_and_renumbered_chart_is_byte_identical() {
        let args = test_args();
        let config = config(true);
        let mut light = point_light(DVec3::new(3.0, 3.0, 3.0), [1.0, 0.9, 0.8]);
        light.falloff_range = 8.0;
        light.light_size = 1.0;
        let lights = vec![light];
        let selection = EntityShadowLightsSection {
            light_indices: vec![0],
        };
        let bake = |far_quad: bool| {
            let output = run_fused_with_geometry(
                &args,
                None,
                &lights,
                Some(&selection),
                &config,
                &BakeControl::unrestricted(),
                &BakeControl::unrestricted(),
                penumbra_cell_geometry(far_quad),
            );
            let floor = output.lightmap.placements[usize::from(far_quad)];
            let block = output
                .lightmap
                .section
                .blocks
                .iter()
                .position(|block| block.cell_id == 1)
                .expect("cell 1 bakes a block");
            let shadowmask = output
                .shadowmask
                .expect("the selected light emits a shadowmask");
            (
                (floor.x, floor.y),
                output.lightmap.section.blocks[block].clone(),
                shadowmask.blocks[block].clone(),
            )
        };
        let (alone_at, alone, alone_mask) = bake(false);
        let (shifted_at, shifted, shifted_mask) = bake(true);
        assert_ne!(alone_at, shifted_at, "the far quad must move the floor");

        // The penumbra must reach the floor, or seeds never matter.
        let red: Vec<u16> = alone
            .irradiance
            .as_chunks::<8>()
            .0
            .iter()
            .map(|texel| u16::from_le_bytes([texel[0], texel[1]]))
            .collect();
        let (lo, hi) = (*red.iter().min().unwrap(), *red.iter().max().unwrap());
        let penumbra = red
            .iter()
            .filter(|&&r| r > lo + (hi - lo) / 20 && r < hi - (hi - lo) / 20)
            .count();
        assert!(
            penumbra > 10,
            "fixture must bake a penumbra, got {penumbra}"
        );

        assert_eq!(alone.irradiance, shifted.irradiance);
        assert_eq!(alone.direction, shifted.direction);
        assert_eq!(alone_mask, shifted_mask, "shadowmask moved with the chart");
    }

    mod window_tests;
}
