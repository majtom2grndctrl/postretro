// Per-(layer, light) partition keys, chart reach culls, and the cached or
// baked partition source the fused walk windows over.
// See: context/lib/build_pipeline.md §Build Cache

use bvh::bvh::Bvh;
use glam::Vec3;

use crate::Args;
use crate::bake_control::BakeControl;
use crate::bvh_build::BvhPrimitive;
use crate::cache::{CacheKey, StageCache};
use crate::geometry::GeometryResult;
use crate::light_namespaces::StaticBakedEntry;
use crate::lightmap_bake;
use crate::lightmap_layer::{self, SharedAtlas};
use crate::map_data::{LightType, MapLight};

#[cfg(test)]
use super::PartitionHooks;
use super::PartitionWindow;
use super::window::PartitionSource;

/// Layer-major Cartesian work, indexed without a layer-by-light allocation.
/// The shadowmask-only pass borrows an ascending subset of light positions.
#[derive(Clone, Copy)]
pub(super) struct LayerItems<'a> {
    layers: u32,
    lights: usize,
    selected: Option<&'a [usize]>,
}

impl<'a> LayerItems<'a> {
    pub(super) fn all(layers: u32, lights: usize) -> Self {
        Self {
            layers,
            lights,
            selected: None,
        }
    }

    pub(super) fn selected(layers: u32, selected: &'a [usize]) -> Self {
        Self {
            layers,
            lights: selected.len(),
            selected: Some(selected),
        }
    }

    pub(super) fn len(self) -> usize {
        (self.layers as usize)
            .checked_mul(self.lights)
            .expect("admitted partition count fits address space")
    }

    pub(super) fn get(self, item: usize) -> (u32, usize) {
        assert!(item < self.len());
        let light = item % self.lights;
        (
            (item / self.lights) as u32,
            self.selected.map_or(light, |selected| selected[light]),
        )
    }
}

/// One prefix per light; layer hashes are finalized only as each consumer
/// needs them. Prefix construction also bounds concurrent geometry-hash scratch
/// by the partition window, independently of the Rayon pool's worker count.
#[allow(clippy::too_many_arguments)]
pub(super) fn layer_key_prefixes(
    layer_lights: &[&StaticBakedEntry<'_>],
    shared: &SharedAtlas<'_>,
    primitives: &[BvhPrimitive],
    geometry: &GeometryResult,
    density: f32,
    area_sample_count: u32,
    window: usize,
    control: &BakeControl,
) -> Vec<blake3::Hasher> {
    use rayon::prelude::*;
    let context =
        lightmap_layer::LayerKeyContext::new(shared, geometry, density, area_sample_count);
    let mut prefixes = Vec::with_capacity(layer_lights.len());
    for lights in layer_lights.chunks(window.max(1)) {
        let mut batch: Vec<_> = lights
            .par_iter()
            .map(|entry| {
                let _permit = control.governor().enter();
                context.light_prefix(entry.light, primitives, geometry)
            })
            .collect();
        prefixes.append(&mut batch);
    }
    prefixes
}

/// Exact legacy layer-major hash order, with no hash matrix.
pub(super) fn layer_hashes(
    prefixes: &[blake3::Hasher],
    layer_count: u32,
) -> impl ExactSizeIterator<Item = [u8; 32]> + '_ {
    let items = LayerItems::all(layer_count, prefixes.len());
    (0..items.len()).map(move |item| {
        let (layer, light) = items.get(item);
        lightmap_layer::LayerKeyContext::layer_hash(&prefixes[light], layer)
    })
}

/// World bounds of a chart's texel centres, padded by
/// `AABB_PADDING_METERS` to absorb `f32` rounding in the texel walk. The pad
/// (0.5 m, borrowed from `affinity_grid`) is deliberately generous and only
/// makes the reach cull more conservative; a smaller pad would cull more, and
/// must still exceed the `f32` rounding error of the texel positions.
pub(super) fn chart_texel_bounds(chart: &lightmap_bake::Chart) -> Option<(Vec3, Vec3)> {
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
pub(super) fn light_may_reach(light: &MapLight, bounds: Option<(Vec3, Vec3)>) -> bool {
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
pub(super) struct LayerPartitions<'a> {
    args: &'a Args,
    cache: Option<&'a StageCache>,
    control: &'a BakeControl,
    geometry: &'a GeometryResult,
    bvh: &'a Bvh<f32, 3>,
    primitives: &'a [BvhPrimitive],
    shared: &'a SharedAtlas<'a>,
    layer_lights: &'a [&'a StaticBakedEntry<'a>],
    layer_prefixes: &'a [blake3::Hasher],
    items: LayerItems<'a>,
    /// The bake's receiver-footprint switch (`LightmapConfig::penumbra_floor`).
    penumbra_floor: bool,
    /// Each bake layer's charts in placement order, built once.
    pub(super) layer_charts: Vec<Vec<usize>>,
    /// Padded world bounds of each chart's texels; `None` for a chart with no
    /// texels.
    chart_bounds: Vec<Option<(Vec3, Vec3)>>,
    #[cfg(test)]
    hooks: PartitionHooks,
}

impl<'a> LayerPartitions<'a> {
    #[allow(clippy::too_many_arguments)]
    pub(super) fn new(
        args: &'a Args,
        cache: Option<&'a StageCache>,
        control: &'a BakeControl,
        geometry: &'a GeometryResult,
        bvh: &'a Bvh<f32, 3>,
        primitives: &'a [BvhPrimitive],
        shared: &'a SharedAtlas<'a>,
        layer_lights: &'a [&'a StaticBakedEntry<'a>],
        layer_prefixes: &'a [blake3::Hasher],
        items: LayerItems<'a>,
        penumbra_floor: bool,
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
            layer_prefixes,
            items,
            penumbra_floor,
            layer_charts,
            chart_bounds,
            #[cfg(test)]
            hooks: window.hooks.clone(),
        }
    }

    fn key(&self, item: usize) -> CacheKey {
        let (layer, light) = self.items.get(item);
        let hash = lightmap_layer::LayerKeyContext::layer_hash(&self.layer_prefixes[light], layer);
        CacheKey::new(
            "lightmap_layer",
            lightmap_layer::LAYER_FORMAT_VERSION,
            &hash,
        )
    }
}

impl PartitionSource for LayerPartitions<'_> {
    fn load(&self, item: usize) -> Option<lightmap_layer::LightmapLayer> {
        let cache = self.cache?;
        let (target_layer, _light) = self.items.get(item);
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
        let (layer, light) = self.items.get(item);
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
        let (_layer, light) = self.items.get(item);
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
            self.penumbra_floor,
            self.control,
        )
    }

    fn finish(
        &self,
        item: usize,
        mut texels: Vec<lightmap_layer::LayerTexel>,
    ) -> lightmap_layer::LightmapLayer {
        let (target_layer, _light) = self.items.get(item);
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
