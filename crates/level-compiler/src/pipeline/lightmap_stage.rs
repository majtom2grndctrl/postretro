//! Lightmap stage orchestration, isolated from the top-level compiler pipeline.

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
use crate::map_data::{MapData, MapLight, ShadowType};
use crate::shadowmask_bake;

pub(super) struct FusedLightingOutput {
    pub lightmap: LightmapBakeOutput,
    pub shadowmask: Option<ShadowmaskAtlasSection>,
    pub shadowmask_elapsed: Duration,
}

pub(super) fn prepare(
    map_data: &MapData,
    geometry: &mut GeometryResult,
    static_lights: &StaticBakedLights<'_>,
    config: &LightmapConfig,
) -> anyhow::Result<PreparedAtlas> {
    lightmap_bake::prepare_atlas(
        geometry,
        static_lights,
        config.lightmap_density,
        &map_data.lightmap_scale_regions,
    )
    .map_err(|e| anyhow::anyhow!("Lightmap atlas prepare failed: {e}"))
}

#[allow(clippy::too_many_arguments)]
pub(super) fn bake_fused_prepared(
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
    let density = config.lightmap_density;
    let shared = SharedAtlas {
        charts: &prepared.charts,
        placements: &prepared.placements,
        atlas_width: prepared.atlas_width,
        atlas_height: prepared.atlas_height,
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
    );

    if static_lights.is_empty() || prepared.placements.is_empty() {
        let (shadowmask, shadowmask_elapsed) = shadowmask.finish();
        return Ok(FusedLightingOutput {
            lightmap: LightmapBakeOutput {
                section: postretro_level_format::lightmap::LightmapSection::placeholder(),
                charts: prepared.charts,
                placements: prepared.placements,
                atlas_width: prepared.atlas_width,
                atlas_height: prepared.atlas_height,
                layer_count: prepared.layer_count,
            },
            shadowmask,
            shadowmask_elapsed,
        });
    }

    let layer_lights: Vec<_> = static_lights
        .entries()
        .iter()
        .filter(|entry| entry.light.shadow_type != ShadowType::Sdf)
        .collect();
    let total = prepared.placements.len().saturating_mul(layer_lights.len());
    if total != 0 {
        lightmap_control.publish_total(total);
    }

    let mut layer_input_hashes =
        Vec::with_capacity(prepared.layer_count as usize * layer_lights.len());
    for target_layer in 0..prepared.layer_count {
        for entry in &layer_lights {
            layer_input_hashes.push(lightmap_layer::layer_input_hash(
                entry.light,
                &shared,
                primitives,
                geometry,
                density,
                args.soft_shadow_samples,
                target_layer,
            ));
        }
    }
    let expected_layer_count = if layer_lights.is_empty() {
        1
    } else {
        prepared.layer_count
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
                    expected_layer_count,
                    density,
                    config.uncompressed_irradiance,
                    config.direction_texel_scale,
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
    if stage_cache.is_some() {
        log::info!(
            "[cache] lightmap_section {}",
            if compose_lightmap { "miss" } else { "hit" }
        );
    }

    let section = if layer_lights.is_empty() && compose_lightmap {
        let mut fallback =
            lightmap_layer::empty_composite(prepared.atlas_width, prepared.atlas_height);
        fallback.dilate();
        fallback.encode_section(
            density,
            config.uncompressed_irradiance,
            config.direction_texel_scale,
        )
    } else if let Some(section) = cached_section {
        section
    } else {
        let mut irradiance = Vec::new();
        let mut direction = Vec::new();
        let mut completed_work = 0usize;
        for target_layer in 0..prepared.layer_count {
            let target_chart_count = prepared
                .placements
                .iter()
                .filter(|placement| placement.layer == target_layer)
                .count();
            let mut accumulator =
                lightmap_layer::IncrementalLayerAccumulator::for_atlas_layer(&shared, target_layer);
            let hash_offset = target_layer as usize * layer_lights.len();
            for (entry, input_hash) in layer_lights
                .iter()
                .zip(&layer_input_hashes[hash_offset..hash_offset + layer_lights.len()])
            {
                let partition = load_or_bake_partition(
                    args,
                    stage_cache,
                    lightmap_control,
                    geometry,
                    bvh,
                    primitives,
                    &shared,
                    entry.light,
                    input_hash,
                    target_layer,
                    target_chart_count,
                );
                completed_work = completed_work.saturating_add(target_chart_count);
                accumulator.fold_partition(entry.light, &partition, &shared);
                shadowmask.consume_partition(entry.source_index, &partition);
            }
            let mut plane = accumulator.finish();
            plane.dilate();
            let (mut layer_irradiance, mut layer_direction) = lightmap_bake::encode_atlas_layer(
                &plane,
                config.uncompressed_irradiance,
                config.direction_texel_scale,
            );
            irradiance.append(&mut layer_irradiance);
            direction.append(&mut layer_direction);
        }
        debug_assert_eq!(completed_work, total);
        lightmap_bake::assemble_layered_section(
            prepared.atlas_width,
            prepared.atlas_height,
            prepared.layer_count,
            density,
            config.uncompressed_irradiance,
            config.direction_texel_scale,
            irradiance,
            direction,
        )
    };

    // If the lightmap memo hit but the shadowmask memo missed, consume only the
    // selected partitions. They are read/baked once here and never by a later
    // pipeline stage.
    if !compose_lightmap {
        let mut completed_work = 0usize;
        for target_layer in 0..prepared.layer_count {
            let target_chart_count = prepared
                .placements
                .iter()
                .filter(|placement| placement.layer == target_layer)
                .count();
            let hash_offset = target_layer as usize * layer_lights.len();
            for (entry, input_hash) in layer_lights
                .iter()
                .zip(&layer_input_hashes[hash_offset..hash_offset + layer_lights.len()])
            {
                if !shadowmask.needs_source(entry.source_index) {
                    continue;
                }
                let partition = load_or_bake_partition(
                    args,
                    stage_cache,
                    lightmap_control,
                    geometry,
                    bvh,
                    primitives,
                    &shared,
                    entry.light,
                    input_hash,
                    target_layer,
                    target_chart_count,
                );
                completed_work = completed_work.saturating_add(target_chart_count);
                shadowmask.consume_partition(entry.source_index, &partition);
            }
        }
        if completed_work < total {
            lightmap_control.advance(total - completed_work);
        }
    }

    if compose_lightmap {
        if let (Some(cache), Some(key)) = (stage_cache, section_key.as_ref()) {
            cache.put(key, &section.to_bytes());
        }
    }
    let (shadowmask, shadowmask_elapsed) = shadowmask.finish();
    Ok(FusedLightingOutput {
        lightmap: LightmapBakeOutput {
            section,
            charts: prepared.charts,
            placements: prepared.placements,
            atlas_width: prepared.atlas_width,
            atlas_height: prepared.atlas_height,
            layer_count: prepared.layer_count,
        },
        shadowmask,
        shadowmask_elapsed,
    })
}

#[allow(clippy::too_many_arguments)]
fn load_or_bake_partition(
    args: &Args,
    cache: Option<&StageCache>,
    control: &BakeControl,
    geometry: &GeometryResult,
    bvh: &Bvh<f32, 3>,
    primitives: &[BvhPrimitive],
    shared: &SharedAtlas<'_>,
    light: &MapLight,
    input_hash: &[u8; 32],
    target_layer: u32,
    target_chart_count: usize,
) -> lightmap_layer::LightmapLayer {
    let layer_key = CacheKey::new(
        "lightmap_layer",
        lightmap_layer::LAYER_FORMAT_VERSION,
        input_hash,
    );
    let cached_partition = cache
        .and_then(|cache| cache.get(&layer_key))
        .and_then(|bytes| lightmap_layer::LightmapLayer::from_bytes(&bytes))
        .and_then(|partition| {
            match lightmap_layer::validate_layer_partition(&partition, shared, target_layer) {
                Ok(()) => Some(partition),
                Err(reason) => {
                    log::warn!(
                        "[Compiler] lightmap_layer cache entry does not match target layer {target_layer} ({reason}), re-baking"
                    );
                    None
                }
            }
        });
    match cached_partition {
        Some(partition) => {
            if args.verbose {
                log::info!("[cache] lightmap_layer hit");
            }
            control.governor().checkpoint();
            control.advance(target_chart_count);
            partition
        }
        None => {
            if args.verbose && cache.is_some() {
                log::info!("[cache] lightmap_layer miss");
            }
            let partition = lightmap_layer::bake_light_layer_controlled(
                light,
                shared,
                bvh,
                primitives,
                geometry,
                target_layer,
                args.soft_shadow_samples,
                control,
            );
            if let Some(cache) = cache {
                cache.put(&layer_key, &partition.to_bytes());
            }
            partition
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use glam::DVec3;
    use log::Level;
    use postretro_level_format::geometry::{FaceMeta, GeometrySection, Vertex};
    use postretro_level_format::texture_names::TextureNamesSection;
    use postretro_test_log_capture::LogCapture;
    use rayon::ThreadPoolBuilder;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicU64, Ordering};

    use crate::bvh_build::build_bvh;
    use crate::geometry::FaceIndexRange;
    use crate::governor::Governor;
    use crate::map_data::{FalloffModel, LightType};
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
        let mut geometry = quad_geometry();
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
        let output = run_fused(
            args,
            cache,
            lights,
            Some(selection),
            config,
            &BakeControl::unrestricted(),
            &BakeControl::unrestricted(),
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

    fn fused_outputs_with_workers(
        workers: usize,
        args: &Args,
        cache: Option<&StageCache>,
        lights: &[MapLight],
        selection: &EntityShadowLightsSection,
        config: &LightmapConfig,
    ) -> (Vec<u8>, Vec<u8>, u32) {
        ThreadPoolBuilder::new()
            .num_threads(workers)
            .build()
            .expect("build fused fixture pool")
            .install(|| fused_outputs(args, cache, lights, selection, config))
    }

    fn reference_outputs(
        lights: &[MapLight],
        selection: &EntityShadowLightsSection,
        config: &LightmapConfig,
    ) -> (Vec<u8>, Vec<u8>) {
        let mut geometry = quad_geometry();
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

    #[test]
    fn fused_cold_warm_and_selection_only_paths_match_reference_bytes() {
        let args = test_args();
        let lights = vec![
            point_light(DVec3::new(0.2, 1.0, 0.35), [1.0, 0.25, 0.1]),
            point_light(DVec3::new(0.85, 1.5, 0.75), [0.1, 0.35, 1.0]),
        ];
        let initial_selection = EntityShadowLightsSection {
            light_indices: vec![0],
        };
        let edited_selection = EntityShadowLightsSection {
            light_indices: vec![1],
        };

        for uncompressed in [false, true] {
            let config = config(uncompressed);
            let reference = reference_outputs(&lights, &initial_selection, &config);
            let cold =
                fused_outputs_with_workers(1, &args, None, &lights, &initial_selection, &config);
            let parallel_cold =
                fused_outputs_with_workers(4, &args, None, &lights, &initial_selection, &config);
            assert_eq!(cold.0, reference.0, "cold fused lightmap bytes changed");
            assert_eq!(cold.1, reference.1, "cold fused shadowmask bytes changed");
            assert_eq!(parallel_cold, cold);

            let dir = fresh_cache_dir(if uncompressed { "rgba16f" } else { "bc6h" });
            let cache = StageCache::new(&dir).expect("create fused test cache");
            let warm_miss =
                fused_outputs(&args, Some(&cache), &lights, &initial_selection, &config);
            assert_eq!(warm_miss.0, reference.0, "warm miss lightmap mismatch");
            assert_eq!(warm_miss.1, reference.1, "warm miss shadowmask mismatch");

            let no_edit_logs = LogCapture::start();
            let warm_hit = fused_outputs(&args, Some(&cache), &lights, &initial_selection, &config);
            assert_eq!(warm_hit, warm_miss, "warm hit must preserve exact bytes");
            no_edit_logs.assert_logged_once(Level::Info, "[cache] shadowmask_atlas hit");
            no_edit_logs.assert_logged_once(Level::Info, "[cache] lightmap_section hit");
            no_edit_logs.assert_not_logged(Level::Info, "[cache] lightmap_layer hit");
            no_edit_logs.assert_not_logged(Level::Info, "[cache] lightmap_layer miss");
            drop(no_edit_logs);

            let mut one_light_edit = lights.clone();
            one_light_edit[1].intensity = 0.875;
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
            let edit_records = edit_logs.records();
            let layer_hits = edit_records
                .iter()
                .filter(|record| record.message.contains("[cache] lightmap_layer hit"))
                .count();
            let layer_misses = edit_records
                .iter()
                .filter(|record| record.message.contains("[cache] lightmap_layer miss"))
                .count();
            assert_eq!(layer_hits, edited.2 as usize);
            assert_eq!(layer_misses, edited.2 as usize);
            drop(edit_logs);

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
            selection_logs.assert_logged_once(Level::Info, "[cache] shadowmask_atlas miss");
            selection_logs.assert_logged_once(Level::Info, "[cache] lightmap_section hit");
            selection_logs.assert_not_logged(Level::Info, "[cache] lightmap_layer miss");
            let layer_hits = selection_logs
                .records()
                .into_iter()
                .filter(|record| {
                    record.level == Level::Info
                        && record.message.contains("[cache] lightmap_layer hit")
                })
                .count();
            assert_eq!(
                layer_hits, selection_only.2 as usize,
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

        let first = run_fused(
            &args,
            Some(&cache),
            &lights,
            None,
            &config,
            &BakeControl::unrestricted(),
            &BakeControl::unrestricted(),
        );
        let second = run_fused(
            &args,
            Some(&cache),
            &lights,
            None,
            &config,
            &BakeControl::unrestricted(),
            &BakeControl::unrestricted(),
        );
        assert_eq!(first.lightmap.section.layer_count, 1);
        assert_eq!(
            first.lightmap.section.to_bytes(),
            second.lightmap.section.to_bytes()
        );
        assert!(first.shadowmask.is_none());
        assert!(second.shadowmask.is_none());

        drop(cache);
        let _ = std::fs::remove_dir_all(dir);
    }
}
