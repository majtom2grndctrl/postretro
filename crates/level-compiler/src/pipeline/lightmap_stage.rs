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
use crate::light_namespaces::{AlphaLightsNs, StaticBakedLights};
use crate::lightmap_bake::{self, CutParent, LightmapBakeOutput, LightmapConfig, PreparedAtlas};
use crate::lightmap_layer::{self, SharedAtlas};
use crate::map_data::ShadowType;
use crate::shadowmask_bake;
use crate::texel_step_detector::TEXEL_STEP_THRESHOLD;

mod cache;
mod metadata;
mod partitions;
mod peak;
mod texel_steps;
mod window;

#[cfg(test)]
use crate::lightmap_layer::chart_texel_bounds;
#[cfg(test)]
use crate::texel_step_detector::StepSite;
use crate::texel_step_detector::TexelStepReport;
pub(super) use metadata::MetadataCounts;
use partitions::{LayerItems, LayerPartitions, layer_hashes, layer_key_prefixes};
pub(super) use peak::log_predicted_peak;
#[cfg(test)]
pub(super) use peak::predicted_peak;
use texel_steps::StepDetector;
use window::LIGHTMAP_PARTITION_WINDOW;
#[cfg(test)]
pub(crate) use window::WindowProbe;

pub(crate) struct FusedLightingOutput {
    pub lightmap: LightmapBakeOutput,
    pub shadowmask: Option<ShadowmaskAtlasSection>,
    pub shadowmask_elapsed: Duration,
    /// Every stepped (light, face) the texel-step detector found, already
    /// logged; replayed from the `lightmap_section` memo on a hit.
    #[cfg(test)]
    pub texel_steps: TexelStepReport,
    /// Detector time summed over workers; zero on a memo hit. Never keyed.
    pub texel_step_elapsed: Duration,
    /// The atlas's cut faces, for the weight-map bake's texel-step detector.
    pub cut_parents: Vec<CutParent>,
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
    #[cfg(test)]
    pub(crate) raw_fill_probe: Option<shadowmask_bake::RawFillProbe>,
    /// Replaces [`TEXEL_STEP_THRESHOLD`] in the detector and the section key.
    #[cfg(test)]
    pub(crate) step_threshold: Option<f32>,
}

impl Default for PartitionWindow {
    fn default() -> Self {
        Self {
            size: LIGHTMAP_PARTITION_WINDOW,
            #[cfg(test)]
            probe: None,
            #[cfg(test)]
            hooks: PartitionHooks::default(),
            #[cfg(test)]
            raw_fill_probe: None,
            #[cfg(test)]
            step_threshold: None,
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
    light_source_labels: &[String],
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
        light_source_labels,
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
    light_source_labels: &[String],
    bvh: &Bvh<f32, 3>,
    primitives: &[BvhPrimitive],
    config: &LightmapConfig,
    prepared: PreparedAtlas,
    window: &PartitionWindow,
) -> anyhow::Result<FusedLightingOutput> {
    peak::gate_working_set(
        &prepared,
        MetadataCounts::new(static_lights, alpha_lights, primitives.len()),
        config.uncompressed_irradiance,
        window.size,
        args.working_set_max_bytes,
    )?;
    // An owner index past u16 fails before any lightmap work. The pipeline
    // checks first, before any bake stage; this guards direct callers.
    let static_spec_light_count = shadowmask_bake::static_spec_light_count(alpha_lights);
    shadowmask_bake::check_owner_index_capacity(static_spec_light_count)?;
    // No consumer can use these charts, so zero admission also means no
    // layer/owner/key metadata allocation, even with many authored lights.
    if static_lights.is_empty() || prepared.placements.is_empty() {
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
            shadowmask: None,
            shadowmask_elapsed: Duration::ZERO,
            #[cfg(test)]
            texel_steps: TexelStepReport::default(),
            texel_step_elapsed: Duration::ZERO,
            cut_parents: prepared.cut_parents,
        });
    }
    let density = config.lightmap_density;
    let shared = SharedAtlas {
        charts: &prepared.charts,
        placements: &prepared.placements,
        atlas_width: prepared.atlas_width,
        atlas_height: prepared.atlas_height,
        layout: &prepared.layout,
    };
    let layer_lights: Vec<_> = static_lights
        .entries()
        .iter()
        .filter(|entry| entry.light.shadow_type != ShadowType::Sdf)
        .collect();
    // Partition keys feed only the cache, so an uncached bake hashes nothing.
    let layer_prefixes = if stage_cache.is_some() && !prepared.placements.is_empty() {
        layer_key_prefixes(
            &layer_lights,
            &shared,
            primitives,
            geometry,
            density,
            args.soft_shadow_samples,
            config.penumbra_floor,
            window.size,
            lightmap_control,
        )
    } else {
        Vec::new()
    };
    let eligible =
        shadowmask_bake::owner_eligible_lights(static_lights, alpha_lights, shadow_selection);
    let label_source: std::collections::HashMap<u16, usize> = eligible
        .iter()
        .map(|light| (light.spec_index, light.source_index))
        .collect();
    // Source indices are ascending in the static namespace. Derive eligible
    // hashes from the same prefixes without copying or reordering a matrix.
    let eligible_layer_hash = |source, layer| {
        let position = layer_lights
            .binary_search_by_key(&source, |entry| entry.source_index)
            .expect("an owner-eligible light is a layer light");
        lightmap_layer::LayerKeyContext::layer_hash(&layer_prefixes[position], layer)
    };
    // The shadowmask memo resolves before the lightmap memo can choose to skip
    // the walk; on a miss, ownership is decided inside the walk.
    let mut shadowmask = shadowmask_bake::prepare_fused_shadowmask(
        eligible,
        &eligible_layer_hash,
        static_spec_light_count,
        prepared.layer_count,
        &shared,
        &shadowmask_bake::EstimateScene {
            cut_parents: &prepared.cut_parents,
            bvh,
            primitives,
            geometry,
            area_sample_count: args.soft_shadow_samples,
            penumbra_floor: config.penumbra_floor,
        },
        stage_cache,
        shadowmask_control,
    )?;
    #[cfg(test)]
    shadowmask.set_raw_fill_probe(window.raw_fill_probe.clone());
    let log_report = |report: &shadowmask_bake::OwnerReport,
                      estimate: &shadowmask_bake::EstimateStats| {
        if args.verbose && estimate.cut_faces > 0 {
            log::info!(
                "[ShadowmaskAtlas] cut-face estimate: {} cut face(s), {} sample(s), {} light \
                 trace(s) in {:.1} ms",
                estimate.cut_faces,
                estimate.samples,
                estimate.traces,
                estimate.elapsed.as_secs_f64() * 1000.0,
            );
        }
        shadowmask_bake::log_owner_report(
            report,
            |spec_index| {
                label_source
                    .get(&spec_index)
                    .and_then(|&source| light_source_labels.get(source))
                    .cloned()
                    .unwrap_or_else(|| format!("static light {spec_index}"))
            },
            args.verbose,
        )
    };

    let total = prepared.placements.len().saturating_mul(layer_lights.len());
    lightmap_control.publish_total(total);

    #[cfg(not(test))]
    let step_threshold = TEXEL_STEP_THRESHOLD;
    #[cfg(test)]
    let step_threshold = window.step_threshold.unwrap_or(TEXEL_STEP_THRESHOLD);
    let section_key = stage_cache.map(|_| {
        let input_hash = lightmap_layer::section_input_hash(
            layer_hashes(&layer_prefixes, prepared.layer_count),
            &shared,
            density,
            config.uncompressed_irradiance,
            config.direction_texel_scale,
            step_threshold,
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
            let Some((report, section_bytes)) =
                TexelStepReport::parse(&bytes, layer_lights.len(), prepared.charts.len())
            else {
                log::warn!(
                    "[Compiler] corrupt lightmap section, recomposing: memo entry has no texel-step report"
                );
                return None;
            };
            match postretro_level_format::lightmap::LightmapSection::from_bytes(section_bytes) {
                Ok(section) => match lightmap_layer::validate_cached_lightmap_section(
                    &section,
                    &shared,
                    config.uncompressed_irradiance,
                ) {
                    Ok(()) => Some((section, report)),
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
            for hash in layer_hashes(&layer_prefixes, prepared.layer_count) {
                cache.mark_used(&CacheKey::new(
                    "lightmap_layer",
                    lightmap_layer::LAYER_FORMAT_VERSION,
                    &hash,
                ));
            }
        }
    }

    let mut texel_step_elapsed = Duration::ZERO;
    let (section, texel_steps) = if let Some(cached) = cached_section {
        cached
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
        let mut texel_steps = TexelStepReport::default();
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
            let items = LayerItems::all(prepared.layer_count, layer_lights.len());
            // Every partition this pass bakes or loads is checked, so a layer
            // cache hit reports what its bake found.
            let detector = StepDetector::new(
                step_threshold,
                &shared,
                bvh,
                primitives,
                geometry,
                &prepared.cut_parents,
            );
            let source = LayerPartitions::new(
                args,
                stage_cache,
                lightmap_control,
                geometry,
                bvh,
                primitives,
                &shared,
                &layer_lights,
                &layer_prefixes,
                items,
                config.penumbra_floor,
                Some(&detector),
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
                    let (layer, light) = items.get(item);
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
                    shadowmask.consume_partition(entry.source_index, &partition, &shared);
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
            drop(source);
            (texel_steps, texel_step_elapsed) = detector.finish();
        }
        (builder.finish(), texel_steps)
    };
    // Cold, warm or replayed, the same report logs once, here, on the
    // caller's thread.
    texel_steps.log("static", |light| layer_lights[light as usize].light.origin);

    // If the lightmap memo hit but the shadowmask memo missed, consume only the
    // owner-eligible partitions. They are read/baked once here and never by a later
    // pipeline stage.
    if !compose_lightmap {
        let selected: Vec<usize> = layer_lights
            .iter()
            .enumerate()
            .filter_map(|(light, entry)| {
                shadowmask.needs_source(entry.source_index).then_some(light)
            })
            .collect();
        let items = LayerItems::selected(prepared.layer_count, &selected);
        let source = LayerPartitions::new(
            args,
            stage_cache,
            lightmap_control,
            geometry,
            bvh,
            primitives,
            &shared,
            &layer_lights,
            &layer_prefixes,
            items,
            config.penumbra_floor,
            None,
            window,
        );
        let completed_work = prepared.placements.len().saturating_mul(selected.len());
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
                let (_, light) = items.get(item);
                shadowmask.consume_partition(layer_lights[light].source_index, &partition, &shared);
            },
            #[cfg(test)]
            window.probe.clone(),
        );
        // This pass consumes only owner-eligible partitions, so the rest
        // of the published `total` is completed here to leave progress at 100%.
        if completed_work < total {
            lightmap_control.advance(total - completed_work);
        }
    }

    if compose_lightmap && let (Some(cache), Some(key)) = (stage_cache, section_key.as_ref()) {
        #[cfg(test)]
        RAW_FILL_AT_LIGHTMAP_CACHE_WRITE
            .with(|sample| sample.set(Some(shadowmask.raw_fill_live_bytes())));
        cache::cache_lightmap_section(cache, key, &section, &texel_steps);
    }
    let shadowmask_bake::FusedShadowmaskOutput {
        section: shadowmask,
        report: shadowmask_report,
        estimate: shadowmask_estimate,
        elapsed: shadowmask_elapsed,
    } = shadowmask.finish(&shared);
    log_report(&shadowmask_report, &shadowmask_estimate);
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
        #[cfg(test)]
        texel_steps,
        texel_step_elapsed,
        cut_parents: prepared.cut_parents,
    })
}

#[cfg(test)]
mod tests;

#[cfg(test)]
thread_local! {
    pub(crate) static RAW_FILL_AT_LIGHTMAP_CACHE_WRITE: std::cell::Cell<Option<usize>> = const { std::cell::Cell::new(None) };
}
