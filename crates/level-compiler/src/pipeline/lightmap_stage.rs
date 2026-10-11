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
use crate::lightmap_bake::{self, LightmapBakeOutput, LightmapConfig, PreparedAtlas};
use crate::lightmap_layer::{self, SharedAtlas};
use crate::map_data::ShadowType;
use crate::shadowmask_bake;

mod partitions;
mod peak;
mod window;

#[cfg(test)]
use partitions::chart_texel_bounds;
use partitions::{LayerPartitions, layer_key_hashes};
pub(super) use peak::log_predicted_peak;
#[cfg(test)]
use peak::predicted_peak;
use window::LIGHTMAP_PARTITION_WINDOW;
#[cfg(test)]
pub(crate) use window::WindowProbe;

pub(crate) struct FusedLightingOutput {
    pub lightmap: LightmapBakeOutput,
    pub shadowmask: Option<ShadowmaskAtlasSection>,
    pub shadowmask_elapsed: Duration,
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
    // An owner index past u16 fails before any lightmap work. The pipeline
    // checks first, before any bake stage; this guards direct callers.
    let static_spec_light_count = shadowmask_bake::static_spec_light_count(alpha_lights);
    shadowmask_bake::check_owner_index_capacity(static_spec_light_count)?;
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
    let layer_input_hashes = if stage_cache.is_some() && !prepared.placements.is_empty() {
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
    let eligible =
        shadowmask_bake::owner_eligible_lights(static_lights, alpha_lights, shadow_selection);
    let label_source: std::collections::HashMap<u16, usize> = eligible
        .iter()
        .map(|light| (light.spec_index, light.source_index))
        .collect();
    // Each eligible light's layer hashes, light-major, out of the layer-major
    // partition keys. Every eligible light is a layer light.
    let eligible_layer_hashes: Vec<[u8; 32]> = if layer_input_hashes.is_empty() {
        Vec::new()
    } else {
        let layer_position: std::collections::HashMap<usize, usize> = layer_lights
            .iter()
            .enumerate()
            .map(|(position, entry)| (entry.source_index, position))
            .collect();
        eligible
            .iter()
            .flat_map(|light| {
                let position = *layer_position
                    .get(&light.source_index)
                    .expect("an owner-eligible light is a layer light");
                let lights = layer_lights.len();
                let hashes = &layer_input_hashes;
                (0..prepared.layer_count as usize)
                    .map(move |layer| hashes[layer * lights + position])
            })
            .collect()
    };
    // The shadowmask memo resolves before the lightmap memo can choose to skip
    // the walk; on a miss, ownership is decided inside the walk.
    let mut shadowmask = shadowmask_bake::prepare_fused_shadowmask(
        eligible,
        &eligible_layer_hashes,
        static_spec_light_count,
        prepared.layer_count,
        &shared,
        &shadowmask_bake::EstimateScene {
            cut_parents: &prepared.cut_parents,
            bvh,
            primitives,
            geometry,
            area_sample_count: args.soft_shadow_samples,
        },
        stage_cache,
        shadowmask_control,
    )?;
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

    if static_lights.is_empty() || prepared.placements.is_empty() {
        let shadowmask_bake::FusedShadowmaskOutput {
            section: shadowmask,
            report: shadowmask_report,
            estimate: shadowmask_estimate,
            elapsed: shadowmask_elapsed,
        } = shadowmask.finish(&shared);
        log_report(&shadowmask_report, &shadowmask_estimate);
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
        });
    }

    let total = prepared.placements.len().saturating_mul(layer_lights.len());
    lightmap_control.publish_total(total);

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
        }
        builder.finish()
    };

    // If the lightmap memo hit but the shadowmask memo missed, consume only the
    // owner-eligible partitions. They are read/baked once here and never by a later
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
        cache.put(key, &section.to_bytes());
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
    })
}

#[cfg(test)]
mod tests;
