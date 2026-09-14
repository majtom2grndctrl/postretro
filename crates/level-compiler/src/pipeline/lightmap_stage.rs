//! Lightmap stage orchestration, isolated from the top-level compiler pipeline.

use bvh::bvh::Bvh;

use crate::Args;
use crate::bake_control::BakeControl;
use crate::bvh_build::BvhPrimitive;
use crate::cache::{CacheKey, StageCache};
use crate::geometry::GeometryResult;
use crate::light_namespaces::StaticBakedLights;
use crate::lightmap_bake::{self, LightmapBakeOutput, LightmapConfig};
use crate::lightmap_layer::{self, SharedAtlas};
use crate::map_data::{MapData, MapLight, ShadowType};

#[allow(clippy::too_many_arguments)]
pub(super) fn bake(
    args: &Args,
    map_data: &MapData,
    stage_cache: Option<&StageCache>,
    control: &BakeControl,
    geometry: &mut GeometryResult,
    static_lights: &StaticBakedLights<'_>,
    bvh: &Bvh<f32, 3>,
    primitives: &[BvhPrimitive],
    config: &LightmapConfig,
) -> anyhow::Result<(LightmapBakeOutput, f32)> {
    let density = config.lightmap_density;
    let output = if let Some(cache) = stage_cache {
        bake_cached(
            args,
            map_data,
            cache,
            control,
            geometry,
            static_lights,
            bvh,
            primitives,
            config,
        )?
    } else {
        let mut ctx = lightmap_bake::LightmapBakeCtx {
            bvh,
            primitives,
            geometry,
            lights: static_lights,
            scale_regions: &map_data.lightmap_scale_regions,
        };
        lightmap_bake::bake_lightmap_controlled(&mut ctx, config, control)
            .map_err(|e| anyhow::anyhow!("Lightmap bake failed: {e}"))?
    };
    Ok((output, density))
}

#[allow(clippy::too_many_arguments)]
fn bake_cached(
    args: &Args,
    map_data: &MapData,
    cache: &StageCache,
    control: &BakeControl,
    geometry: &mut GeometryResult,
    static_lights: &StaticBakedLights<'_>,
    bvh: &Bvh<f32, 3>,
    primitives: &[BvhPrimitive],
    config: &LightmapConfig,
) -> anyhow::Result<LightmapBakeOutput> {
    let density = config.lightmap_density;
    let prepared = lightmap_bake::prepare_atlas(
        geometry,
        static_lights,
        density,
        &map_data.lightmap_scale_regions,
    )
    .map_err(|e| anyhow::anyhow!("Lightmap atlas prepare failed: {e}"))?;

    if static_lights.is_empty() || prepared.placements.is_empty() {
        return Ok(LightmapBakeOutput {
            section: postretro_level_format::lightmap::LightmapSection::placeholder(),
            charts: prepared.charts,
            placements: prepared.placements,
            atlas_width: prepared.atlas_width,
            atlas_height: prepared.atlas_height,
            layer_count: prepared.layer_count,
        });
    }

    let shared = SharedAtlas {
        charts: &prepared.charts,
        placements: &prepared.placements,
        atlas_width: prepared.atlas_width,
        atlas_height: prepared.atlas_height,
    };
    let layer_lights: Vec<&MapLight> = static_lights
        .entries()
        .iter()
        .map(|entry| entry.light)
        .filter(|light| light.shadow_type != ShadowType::Sdf)
        .collect();
    let total = prepared.placements.len().saturating_mul(layer_lights.len());
    control.publish_total(total);

    let mut layer_input_hashes =
        Vec::with_capacity(prepared.layer_count as usize * layer_lights.len());
    for target_layer in 0..prepared.layer_count {
        for light in &layer_lights {
            layer_input_hashes.push(lightmap_layer::layer_input_hash(
                light,
                &shared,
                primitives,
                geometry,
                density,
                args.soft_shadow_samples,
                target_layer,
            ));
        }
    }

    let section_input_hash = lightmap_layer::section_input_hash(
        &layer_input_hashes,
        &shared,
        density,
        config.uncompressed_irradiance,
        config.direction_texel_scale,
    );
    let section_key = CacheKey::new(
        "lightmap_section",
        lightmap_layer::LIGHTMAP_SECTION_VERSION,
        &section_input_hash,
    );
    let expected_layer_count = if layer_lights.is_empty() {
        1
    } else {
        prepared.layer_count
    };
    let cached_section = cache.get(&section_key).and_then(|bytes| {
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

    let section = match cached_section {
        Some(section) => {
            log::info!("[cache] lightmap_section hit");
            control.governor().checkpoint();
            control.advance(total);
            section
        }
        None => {
            log::info!("[cache] lightmap_section miss");
            let section = compose_cached_layers(
                args,
                cache,
                control,
                geometry,
                bvh,
                primitives,
                config,
                &prepared,
                &shared,
                &layer_lights,
                &layer_input_hashes,
            );
            cache.put(&section_key, &section.to_bytes());
            section
        }
    };

    Ok(LightmapBakeOutput {
        section,
        charts: prepared.charts,
        placements: prepared.placements,
        atlas_width: prepared.atlas_width,
        atlas_height: prepared.atlas_height,
        layer_count: prepared.layer_count,
    })
}

#[allow(clippy::too_many_arguments)]
fn compose_cached_layers(
    args: &Args,
    cache: &StageCache,
    control: &BakeControl,
    geometry: &GeometryResult,
    bvh: &Bvh<f32, 3>,
    primitives: &[BvhPrimitive],
    config: &LightmapConfig,
    prepared: &lightmap_bake::PreparedAtlas,
    shared: &SharedAtlas<'_>,
    layer_lights: &[&MapLight],
    layer_input_hashes: &[[u8; 32]],
) -> postretro_level_format::lightmap::LightmapSection {
    let density = config.lightmap_density;
    if layer_lights.is_empty() {
        let mut fallback =
            lightmap_layer::empty_composite(prepared.atlas_width, prepared.atlas_height);
        fallback.dilate();
        return fallback.encode_section(
            density,
            config.uncompressed_irradiance,
            config.direction_texel_scale,
        );
    }

    let mut irradiance = Vec::new();
    let mut direction = Vec::new();
    for target_layer in 0..prepared.layer_count {
        let target_chart_count = prepared
            .placements
            .iter()
            .filter(|placement| placement.layer == target_layer)
            .count();
        let mut accumulator =
            lightmap_layer::IncrementalLayerAccumulator::for_atlas_layer(shared, target_layer);
        let hash_offset = target_layer as usize * layer_lights.len();
        for (light, input_hash) in layer_lights
            .iter()
            .zip(&layer_input_hashes[hash_offset..hash_offset + layer_lights.len()])
        {
            let layer_key = CacheKey::new(
                "lightmap_layer",
                lightmap_layer::LAYER_FORMAT_VERSION,
                input_hash,
            );
            let cached_partition = cache
                .get(&layer_key)
                .and_then(|bytes| lightmap_layer::LightmapLayer::from_bytes(&bytes))
                .and_then(|partition| {
                    match lightmap_layer::validate_layer_partition(
                        &partition,
                        shared,
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
            let partition = match cached_partition {
                Some(partition) => {
                    if args.verbose {
                        log::info!("[cache] lightmap_layer hit");
                    }
                    control.governor().checkpoint();
                    control.advance(target_chart_count);
                    partition
                }
                None => {
                    if args.verbose {
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
                    cache.put(&layer_key, &partition.to_bytes());
                    partition
                }
            };
            accumulator.fold_partition(light, &partition, shared);
            drop(partition);
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
}
