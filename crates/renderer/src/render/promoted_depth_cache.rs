use super::renderer_types::{
    MAX_PROMOTED_CUBE, MAX_PROMOTED_SPOT, PromotedShadowPoolKind, PromotedStaticLightRecord,
};

use crate::lighting::cube_shadow::{CUBE_FACE_RESOLUTION, CUBE_FACES};
use crate::lighting::spot_shadow::{SHADOW_DEPTH_FORMAT, SHADOW_MAP_RESOLUTION};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct CacheKey {
    global_light_index: u32,
    selection_index: u32,
    slot: u32,
}

impl CacheKey {
    fn from_record(record: &PromotedStaticLightRecord) -> Self {
        Self {
            global_light_index: record.global_light_index,
            selection_index: record.selection_index,
            slot: record.slot,
        }
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
struct LayerState {
    key: Option<CacheKey>,
    warm: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) struct PromotedSpotCachePlan {
    pub slot: u32,
    pub cache_layer: u32,
    pub needs_world_render: bool,
}

impl PromotedSpotCachePlan {
    pub fn is_warm(self) -> bool {
        !self.needs_world_render
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) struct PromotedCubeCachePlan {
    pub slot: u32,
    pub cache_layer_base: u32,
    pub needs_world_render: bool,
}

impl PromotedCubeCachePlan {
    pub fn is_warm(self) -> bool {
        !self.needs_world_render
    }

    pub fn cache_layer(self, face: usize) -> u32 {
        self.cache_layer_base + face as u32
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(super) struct PromotedDepthCacheCounters {
    pub promoted_count: u32,
    pub cached_world_render_skips: u32,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub(super) struct PromotedDepthCacheFramePlan {
    pub spot: Vec<PromotedSpotCachePlan>,
    pub cube: Vec<PromotedCubeCachePlan>,
    pub counters: PromotedDepthCacheCounters,
}

impl PromotedDepthCacheFramePlan {
    pub fn spot_for_slot(&self, slot: u32) -> Option<PromotedSpotCachePlan> {
        self.spot.iter().copied().find(|plan| plan.slot == slot)
    }

    pub fn cube_for_slot(&self, slot: u32) -> Option<PromotedCubeCachePlan> {
        self.cube.iter().copied().find(|plan| plan.slot == slot)
    }

    pub fn should_dispatch_spot_cull(&self, slot: usize) -> bool {
        !self
            .spot
            .iter()
            .any(|plan| plan.slot as usize == slot && plan.is_warm())
    }

    pub fn should_dispatch_cube_cull(&self, layer: usize) -> bool {
        let slot = layer / CUBE_FACES;
        !self
            .cube
            .iter()
            .any(|plan| plan.slot as usize == slot && plan.is_warm())
    }

    pub fn skipped_spot_cull_dispatches(&self, occupied_slots: &[bool]) -> u32 {
        occupied_slots
            .iter()
            .enumerate()
            .filter(|(slot, occupied)| **occupied && !self.should_dispatch_spot_cull(*slot))
            .count() as u32
    }

    pub fn skipped_cube_cull_dispatches(&self, occupied_layers: &[bool]) -> u32 {
        occupied_layers
            .iter()
            .enumerate()
            .filter(|(layer, occupied)| **occupied && !self.should_dispatch_cube_cull(*layer))
            .count() as u32
    }
}

pub(super) struct PromotedDepthCache {
    spot_views: Vec<wgpu::TextureView>,
    spot_sampled_view: wgpu::TextureView,
    cube_face_views: Vec<wgpu::TextureView>,
    cube_sampled_view: Option<wgpu::TextureView>,
    spot_layers: Vec<LayerState>,
    cube_layers: Vec<LayerState>,
}

impl PromotedDepthCache {
    pub fn new(device: &wgpu::Device, cube_array_supported: bool) -> Self {
        let spot_texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("Promoted Spot World Depth Cache"),
            size: wgpu::Extent3d {
                width: SHADOW_MAP_RESOLUTION,
                height: SHADOW_MAP_RESOLUTION,
                depth_or_array_layers: MAX_PROMOTED_SPOT as u32,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: SHADOW_DEPTH_FORMAT,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING,
            view_formats: &[],
        });
        let spot_views = (0..MAX_PROMOTED_SPOT)
            .map(|layer| {
                spot_texture.create_view(&wgpu::TextureViewDescriptor {
                    label: Some(&format!("Promoted Spot Cache View {layer}")),
                    dimension: Some(wgpu::TextureViewDimension::D2),
                    base_array_layer: layer as u32,
                    array_layer_count: Some(1),
                    ..Default::default()
                })
            })
            .collect();
        let spot_sampled_view = spot_texture.create_view(&wgpu::TextureViewDescriptor {
            label: Some("Promoted Spot Cache Sampled Array View"),
            dimension: Some(wgpu::TextureViewDimension::D2Array),
            base_array_layer: 0,
            array_layer_count: Some(MAX_PROMOTED_SPOT as u32),
            ..Default::default()
        });

        let cube_layer_count = (MAX_PROMOTED_CUBE * CUBE_FACES) as u32;
        let cube_texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("Promoted Cube World Depth Cache"),
            size: wgpu::Extent3d {
                width: CUBE_FACE_RESOLUTION,
                height: CUBE_FACE_RESOLUTION,
                depth_or_array_layers: cube_layer_count,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: SHADOW_DEPTH_FORMAT,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING,
            view_formats: &[],
        });
        let cube_face_views = (0..cube_layer_count)
            .map(|layer| {
                cube_texture.create_view(&wgpu::TextureViewDescriptor {
                    label: Some(&format!("Promoted Cube Cache Face View {layer}")),
                    dimension: Some(wgpu::TextureViewDimension::D2Array),
                    base_array_layer: layer,
                    array_layer_count: Some(1),
                    ..Default::default()
                })
            })
            .collect();
        let cube_sampled_view = cube_array_supported.then(|| {
            cube_texture.create_view(&wgpu::TextureViewDescriptor {
                label: Some("Promoted Cube Cache Sampled Cube Array View"),
                dimension: Some(wgpu::TextureViewDimension::CubeArray),
                base_array_layer: 0,
                array_layer_count: Some(cube_layer_count),
                ..Default::default()
            })
        });

        Self {
            spot_views,
            spot_sampled_view,
            cube_face_views,
            cube_sampled_view,
            spot_layers: vec![LayerState::default(); MAX_PROMOTED_SPOT],
            cube_layers: vec![LayerState::default(); MAX_PROMOTED_CUBE],
        }
    }

    pub fn reset_level(&mut self) {
        clear_layers(&mut self.spot_layers);
        clear_layers(&mut self.cube_layers);
    }

    pub fn plan_frame(
        &mut self,
        records: &[PromotedStaticLightRecord],
    ) -> PromotedDepthCacheFramePlan {
        plan_frame_with_layers(&mut self.spot_layers, &mut self.cube_layers, records)
    }

    pub fn mark_spot_world_rendered(&mut self, plan: PromotedSpotCachePlan) {
        if let Some(layer) = self.spot_layers.get_mut(plan.cache_layer as usize) {
            layer.warm = true;
        }
    }

    pub fn mark_cube_world_rendered(&mut self, plan: PromotedCubeCachePlan) {
        let layer = plan.cache_layer_base as usize / CUBE_FACES;
        if let Some(layer) = self.cube_layers.get_mut(layer) {
            layer.warm = true;
        }
    }

    pub fn spot_view(&self, plan: PromotedSpotCachePlan) -> &wgpu::TextureView {
        &self.spot_views[plan.cache_layer as usize]
    }

    pub fn spot_sampled_view(&self) -> &wgpu::TextureView {
        &self.spot_sampled_view
    }

    pub fn cube_face_view(&self, plan: PromotedCubeCachePlan, face: usize) -> &wgpu::TextureView {
        &self.cube_face_views[plan.cache_layer(face) as usize]
    }

    pub fn cube_sampled_view(&self) -> Option<&wgpu::TextureView> {
        self.cube_sampled_view.as_ref()
    }
}

fn clear_layers(layers: &mut [LayerState]) {
    for layer in layers {
        *layer = LayerState::default();
    }
}

fn retain_active_layers(layers: &mut [LayerState], active_keys: &[CacheKey]) {
    for layer in layers {
        if let Some(key) = layer.key {
            if !active_keys.contains(&key) {
                *layer = LayerState::default();
            }
        }
    }
}

fn assign_layer(layers: &mut [LayerState], key: CacheKey) -> Option<usize> {
    if let Some((idx, _)) = layers
        .iter()
        .enumerate()
        .find(|(_, layer)| layer.key == Some(key))
    {
        return Some(idx);
    }
    let (idx, layer) = layers
        .iter_mut()
        .enumerate()
        .find(|(_, layer)| layer.key.is_none())?;
    layer.key = Some(key);
    layer.warm = false;
    Some(idx)
}

fn plan_frame_with_layers(
    spot_layers: &mut [LayerState],
    cube_layers: &mut [LayerState],
    records: &[PromotedStaticLightRecord],
) -> PromotedDepthCacheFramePlan {
    let spot_keys: Vec<CacheKey> = records
        .iter()
        .filter(|record| record.pool_kind == PromotedShadowPoolKind::Spot)
        .map(CacheKey::from_record)
        .collect();
    let cube_keys: Vec<CacheKey> = records
        .iter()
        .filter(|record| record.pool_kind == PromotedShadowPoolKind::Cube)
        .map(CacheKey::from_record)
        .collect();
    retain_active_layers(spot_layers, &spot_keys);
    retain_active_layers(cube_layers, &cube_keys);

    let mut plan = PromotedDepthCacheFramePlan::default();
    plan.counters.promoted_count = records.len() as u32;

    for record in records {
        let key = CacheKey::from_record(record);
        match record.pool_kind {
            PromotedShadowPoolKind::Spot => {
                if let Some(layer) = assign_layer(spot_layers, key) {
                    let warm = spot_layers[layer].warm;
                    if warm {
                        plan.counters.cached_world_render_skips += 1;
                    }
                    plan.spot.push(PromotedSpotCachePlan {
                        slot: record.slot,
                        cache_layer: layer as u32,
                        needs_world_render: !warm,
                    });
                }
            }
            PromotedShadowPoolKind::Cube => {
                if let Some(layer) = assign_layer(cube_layers, key) {
                    let warm = cube_layers[layer].warm;
                    if warm {
                        plan.counters.cached_world_render_skips += CUBE_FACES as u32;
                    }
                    plan.cube.push(PromotedCubeCachePlan {
                        slot: record.slot,
                        cache_layer_base: (layer * CUBE_FACES) as u32,
                        needs_world_render: !warm,
                    });
                }
            }
        }
    }

    plan
}

#[cfg(test)]
mod tests {
    use super::*;

    fn record(
        selection_index: u32,
        pool_kind: PromotedShadowPoolKind,
        slot: u32,
    ) -> PromotedStaticLightRecord {
        PromotedStaticLightRecord {
            global_light_index: selection_index + 100,
            selection_index,
            pool_kind,
            slot,
            weight: 1.0,
        }
    }

    fn cache_without_gpu() -> (Vec<LayerState>, Vec<LayerState>) {
        (
            vec![LayerState::default(); MAX_PROMOTED_SPOT],
            vec![LayerState::default(); MAX_PROMOTED_CUBE],
        )
    }

    #[test]
    fn cache_budget_matches_promoted_budget_not_pool_size() {
        assert_eq!(MAX_PROMOTED_SPOT, 8);
        assert_eq!(MAX_PROMOTED_CUBE * CUBE_FACES, 12);
    }

    #[test]
    fn cache_source_does_not_request_copy_source_usage() {
        let src = include_str!("promoted_depth_cache.rs");
        assert!(
            !src.contains(concat!("COPY", "_SRC")),
            "the promoted world-depth cache is sampled directly, never copied into a pool slot"
        );
    }

    #[test]
    fn warm_promoted_spot_skips_world_render_and_cull_dispatch() {
        let (mut spot_layers, mut cube_layers) = cache_without_gpu();
        let records = [record(0, PromotedShadowPoolKind::Spot, 7)];

        let mut first = plan_with_layers(&mut spot_layers, &mut cube_layers, &records);
        assert!(first.spot[0].needs_world_render);
        spot_layers[first.spot[0].cache_layer as usize].warm = true;

        let second = plan_with_layers(&mut spot_layers, &mut cube_layers, &records);
        assert!(second.spot[0].is_warm());
        assert!(!second.should_dispatch_spot_cull(7));
        assert_eq!(second.counters.cached_world_render_skips, 1);
        let mut occupied = vec![false; crate::lighting::spot_shadow::SHADOW_POOL_SIZE];
        occupied[7] = true;
        assert_eq!(second.skipped_spot_cull_dispatches(&occupied), 1);

        first.spot.clear();
    }

    #[test]
    fn cube_warm_skip_counts_each_face_sub_region() {
        let (mut spot_layers, mut cube_layers) = cache_without_gpu();
        let records = [record(1, PromotedShadowPoolKind::Cube, 2)];

        let first = plan_with_layers(&mut spot_layers, &mut cube_layers, &records);
        cube_layers[first.cube[0].cache_layer_base as usize / CUBE_FACES].warm = true;

        let second = plan_with_layers(&mut spot_layers, &mut cube_layers, &records);
        assert!(second.cube[0].is_warm());
        let mut occupied = vec![false; crate::lighting::cube_shadow::CUBE_COUNT * CUBE_FACES];
        for face in 0..CUBE_FACES {
            let layer = 2 * CUBE_FACES + face;
            assert!(!second.should_dispatch_cube_cull(layer));
            occupied[layer] = true;
        }
        assert_eq!(second.counters.cached_world_render_skips, CUBE_FACES as u32);
        assert_eq!(
            second.skipped_cube_cull_dispatches(&occupied),
            CUBE_FACES as u32,
        );
    }

    #[test]
    fn warm_cache_does_not_count_cull_skip_without_occupied_cull_work() {
        let (mut spot_layers, mut cube_layers) = cache_without_gpu();
        let records = [record(0, PromotedShadowPoolKind::Spot, 7)];
        let first = plan_with_layers(&mut spot_layers, &mut cube_layers, &records);
        spot_layers[first.spot[0].cache_layer as usize].warm = true;

        let second = plan_with_layers(&mut spot_layers, &mut cube_layers, &records);

        assert_eq!(second.counters.cached_world_render_skips, 1);
        assert_eq!(second.skipped_spot_cull_dispatches(&[]), 0);
    }

    #[test]
    fn slot_reassignment_invalidates_cache_layer() {
        let (mut spot_layers, mut cube_layers) = cache_without_gpu();
        let records = [record(0, PromotedShadowPoolKind::Spot, 3)];
        let first = plan_with_layers(&mut spot_layers, &mut cube_layers, &records);
        spot_layers[first.spot[0].cache_layer as usize].warm = true;

        let reassigned = [record(0, PromotedShadowPoolKind::Spot, 4)];
        let second = plan_with_layers(&mut spot_layers, &mut cube_layers, &reassigned);
        assert!(second.spot[0].needs_world_render);
        assert!(second.should_dispatch_spot_cull(4));
    }

    fn plan_with_layers(
        spot_layers: &mut [LayerState],
        cube_layers: &mut [LayerState],
        records: &[PromotedStaticLightRecord],
    ) -> PromotedDepthCacheFramePlan {
        plan_frame_with_layers(spot_layers, cube_layers, records)
    }
}
