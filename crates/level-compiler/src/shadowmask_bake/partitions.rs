// Per-atlas-layer light partitions, cold-baked or read from the layer cache, fed into the fill.
// See: context/lib/build_pipeline.md §Build Cache

use rayon::prelude::*;

use super::ResidentLayerTracker;
use super::fill::ShadowmaskFill;
use super::section::layer_count_from_shared;
use crate::bake_control::BakeControl;
use crate::bvh_build::BvhPrimitive;
use crate::cache::{CacheKey, StageCache};
use crate::geometry::GeometryResult;
use crate::lightmap_layer::{self, LayerTexel, LightmapLayer, SharedAtlas};
use crate::map_data::MapLight;

/// Bake one atlas layer at a time, retaining at most W selected-light chart
/// payloads and consuming them directly into the final output. Assembling a
/// full partition here would overlap it with the batch that supplied it.
#[allow(clippy::too_many_arguments)]
pub(super) fn fill_uncached_shadowmask_partitions(
    fill: &mut ShadowmaskFill,
    selected: &[(usize, u32, &MapLight)],
    shared: &SharedAtlas<'_>,
    bvh: &bvh::bvh::Bvh<f32, 3>,
    primitives: &[BvhPrimitive],
    geometry: &GeometryResult,
    area_sample_count: u32,
    control: &BakeControl,
    resident_layer_window: usize,
    resident_layers: &ResidentLayerTracker,
) {
    assert!(
        resident_layer_window > 0,
        "shadowmask resident-layer window must be at least one"
    );
    let layer_count = layer_count_from_shared(shared);
    for target_layer in 0..layer_count {
        let target_charts: Vec<_> = shared
            .placements
            .iter()
            .enumerate()
            .filter_map(|(chart_index, placement)| {
                (placement.layer == target_layer).then_some(chart_index)
            })
            .collect();

        for batch_start in (0..selected.len()).step_by(resident_layer_window) {
            let batch_end = (batch_start + resident_layer_window).min(selected.len());
            let batch = &selected[batch_start..batch_end];
            let _resident_partitions: Vec<_> =
                batch.iter().map(|_| resident_layers.acquire()).collect();
            let work_items: Vec<(usize, &MapLight)> = batch
                .iter()
                .flat_map(|&(_, _, light)| {
                    target_charts
                        .iter()
                        .copied()
                        .map(move |chart_index| (chart_index, light))
                })
                .collect();
            let chart_outputs: Vec<Vec<LayerTexel>> = work_items
                .par_iter()
                .map(|&(chart_index, light)| {
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
            for compact_light_index in batch_start..batch_end {
                for _ in 0..target_charts.len() {
                    let chart_texels = chart_outputs
                        .next()
                        .expect("one ordered output is required for every chart task");
                    fill.write_texels(compact_light_index, target_layer, &chart_texels);
                }
                if target_layer + 1 == layer_count && compact_light_index != 0 {
                    control.advance(1);
                }
            }
            debug_assert!(chart_outputs.next().is_none());
        }
        control.governor().checkpoint();
    }
}

/// Read or bake exactly one `(atlas layer, selected light)` cache partition,
/// write it into the final output, then drop it. Every selected partition is
/// populated in the cache even when global coloring drops the light.
#[allow(clippy::too_many_arguments)]
pub(super) fn fill_cached_shadowmask_partitions(
    fill: &mut ShadowmaskFill,
    selected: &[(usize, u32, &MapLight)],
    shared: &SharedAtlas<'_>,
    bvh: &bvh::bvh::Bvh<f32, 3>,
    primitives: &[BvhPrimitive],
    geometry: &GeometryResult,
    area_sample_count: u32,
    cache: &StageCache,
    layer_input_hashes: &[Vec<[u8; 32]>],
    control: &BakeControl,
) {
    let layer_count = layer_count_from_shared(shared);
    debug_assert_eq!(
        selected.len(),
        layer_input_hashes.len(),
        "selected light/hash partitions must align after invalid selections are filtered"
    );
    for target_layer in 0..layer_count {
        for (compact_light_index, ((_, alpha_index, light), hashes)) in
            selected.iter().zip(layer_input_hashes).enumerate()
        {
            debug_assert_eq!(hashes.len(), layer_count as usize);
            let target_chart_count = shared
                .placements
                .iter()
                .filter(|placement| placement.layer == target_layer)
                .count();
            let hash = &hashes[target_layer as usize];
            let key = CacheKey::new("lightmap_layer", lightmap_layer::LAYER_FORMAT_VERSION, hash);
            let cached_partition = cache
                .get(&key)
                .and_then(|bytes| LightmapLayer::from_bytes(&bytes))
                .and_then(|partition| {
                    match lightmap_layer::validate_layer_partition(
                        &partition,
                        shared,
                        target_layer,
                    ) {
                        Ok(()) => Some(partition),
                        Err(reason) => {
                            log::warn!(
                                "[Compiler] corrupt lightmap_layer cache entry for shadowmask selected AlphaLights index {alpha_index}, target layer {target_layer} ({reason}), re-baking"
                            );
                            None
                        }
                    }
                });
            let partition = match cached_partition {
                Some(partition) => {
                    log::info!("[cache] lightmap_layer hit");
                    control.governor().checkpoint();
                    control.advance(target_chart_count);
                    partition
                }
                None => {
                    log::info!("[cache] lightmap_layer miss");
                    let partition = lightmap_layer::bake_light_layer_controlled(
                        light,
                        shared,
                        bvh,
                        primitives,
                        geometry,
                        target_layer,
                        area_sample_count,
                        control,
                    );
                    cache.put(&key, &partition.to_bytes());
                    partition
                }
            };
            fill.write_partition(compact_light_index, &partition);
            drop(partition);
            if target_layer + 1 == layer_count && compact_light_index != 0 {
                control.advance(1);
            }
        }
        control.governor().checkpoint();
    }
}
