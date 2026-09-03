// Dynamic-tier world-depth cache state and GPU resources.
// See: context/lib/rendering_pipeline.md §4.
use glam::Mat4;

use crate::lighting::cube_shadow::{CUBE_FACE_RESOLUTION, CUBE_FACES};
use crate::lighting::spot_shadow::{SHADOW_DEPTH_FORMAT, SHADOW_MAP_RESOLUTION};

pub(super) const DYNAMIC_SPOT_CACHE_LAYERS: usize = 3;
pub(super) const DYNAMIC_CUBE_CACHE_SLOTS: usize = 4;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct CacheKey {
    source_index: usize,
    // A spot uses the first 16 words. A cube light owns all six face matrices;
    // retaining every face prevents a stale cache when only a non-zero face
    // changes (for example after a projection or origin update).
    matrix_bits: [u32; 16 * CUBE_FACES],
}

impl CacheKey {
    fn spot(source_index: usize, matrix: Mat4) -> Self {
        let mut matrix_bits = [0; 16 * CUBE_FACES];
        matrix_bits[..16].copy_from_slice(&matrix.to_cols_array().map(f32::to_bits));
        Self {
            source_index,
            matrix_bits,
        }
    }

    fn cube(source_index: usize, matrices: [Mat4; CUBE_FACES]) -> Self {
        let mut matrix_bits = [0; 16 * CUBE_FACES];
        for (face, matrix) in matrices.into_iter().enumerate() {
            matrix_bits[face * 16..(face + 1) * 16]
                .copy_from_slice(&matrix.to_cols_array().map(f32::to_bits));
        }
        Self {
            source_index,
            matrix_bits,
        }
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
struct LayerState {
    key: Option<CacheKey>,
    warm: bool,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(super) struct DynamicSpotPlan {
    pub slot: u32,
    pub cache_layer: i32,
    pub needs_world_render: bool,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(super) struct DynamicCubePlan {
    pub slot: u32,
    pub cache_slot: i32,
    pub needs_world_render: bool,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(super) struct DynamicDepthCachePlan {
    spot: [DynamicSpotPlan; DYNAMIC_SPOT_CACHE_LAYERS],
    cube: [DynamicCubePlan; DYNAMIC_CUBE_CACHE_SLOTS],
    spot_count: usize,
    cube_count: usize,
}

impl DynamicDepthCachePlan {
    pub fn spot(&self) -> &[DynamicSpotPlan] {
        &self.spot[..self.spot_count]
    }

    pub fn cube(&self) -> &[DynamicCubePlan] {
        &self.cube[..self.cube_count]
    }

    pub fn spot_for_slot(&self, slot: u32) -> Option<DynamicSpotPlan> {
        self.spot().iter().copied().find(|plan| plan.slot == slot)
    }

    pub fn should_dispatch_spot_cull(&self, slot: usize) -> bool {
        !self
            .spot()
            .iter()
            .any(|plan| plan.slot as usize == slot && !plan.needs_world_render)
    }

    pub fn cube_for_slot(&self, slot: u32) -> Option<DynamicCubePlan> {
        self.cube().iter().copied().find(|plan| plan.slot == slot)
    }

    pub fn should_dispatch_cube_cull(&self, layer: usize) -> bool {
        let slot = layer / CUBE_FACES;
        !self
            .cube()
            .iter()
            .any(|plan| plan.slot as usize == slot && !plan.needs_world_render)
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(super) struct DynamicCacheCounters {
    pub cached_spots: u32,
    pub cached_cubes: u32,
    pub world_pass_skips: u32,
    pub cull_dispatch_skips: u32,
}

#[derive(Default)]
pub(super) struct DynamicCacheDiagnostics {
    pub frame: DynamicCacheCounters,
    accumulated: DynamicCacheCounters,
    frames: u32,
}

impl DynamicCacheDiagnostics {
    pub fn finish_frame(&mut self, enabled: bool) {
        if !enabled {
            return;
        }
        self.accumulated.cached_spots += self.frame.cached_spots;
        self.accumulated.cached_cubes += self.frame.cached_cubes;
        self.accumulated.world_pass_skips += self.frame.world_pass_skips;
        self.accumulated.cull_dispatch_skips += self.frame.cull_dispatch_skips;
        self.frames += 1;
        if self.frames == 120 {
            log::info!(
                "[Renderer] Dynamic depth cache (avg over {} rendered frames): cached spots {:.2}, cached cubes {:.2}, world-pass skips {:.2}, cull-dispatch skips {:.2}",
                self.frames,
                self.accumulated.cached_spots as f32 / self.frames as f32,
                self.accumulated.cached_cubes as f32 / self.frames as f32,
                self.accumulated.world_pass_skips as f32 / self.frames as f32,
                self.accumulated.cull_dispatch_skips as f32 / self.frames as f32,
            );
            self.accumulated = DynamicCacheCounters::default();
            self.frames = 0;
        }
    }
}

pub(super) struct DynamicDepthCache {
    spot: [LayerState; DYNAMIC_SPOT_CACHE_LAYERS],
    cube: [LayerState; DYNAMIC_CUBE_CACHE_SLOTS],
}

/// Allocate each cache only when the level contains a dynamic light of that
/// type. No placeholder textures are needed: caches are copy sources, not
/// shader bindings, so pipeline layouts never depend on level contents.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(super) struct DynamicCacheAllocation {
    pub spot: bool,
    pub cube: bool,
}

impl DynamicCacheAllocation {
    pub fn for_lights(
        lights: &[postretro_level_loader::MapLight],
        cube_array_supported: bool,
    ) -> Self {
        use postretro_level_loader::LightType;
        Self {
            spot: lights
                .iter()
                .any(|light| light.is_dynamic && light.light_type == LightType::Spot),
            cube: cube_array_supported
                && lights
                    .iter()
                    .any(|light| light.is_dynamic && light.light_type == LightType::Point),
        }
    }
}

pub(super) struct DynamicDepthCacheGpu {
    pub state: DynamicDepthCache,
    allocation: DynamicCacheAllocation,
    pub spot_texture: Option<wgpu::Texture>,
    spot_views: Vec<wgpu::TextureView>,
    pub cube_texture: Option<wgpu::Texture>,
    cube_views: Vec<wgpu::TextureView>,
}

impl DynamicDepthCacheGpu {
    pub fn new(device: &wgpu::Device, allocation: DynamicCacheAllocation) -> Self {
        let spot_texture = allocation.spot.then(|| {
            cache_texture(
                device,
                "Dynamic Spot World Depth Cache",
                SHADOW_MAP_RESOLUTION,
                DYNAMIC_SPOT_CACHE_LAYERS,
            )
        });
        let cube_texture = allocation.cube.then(|| {
            cache_texture(
                device,
                "Dynamic Cube World Depth Cache",
                CUBE_FACE_RESOLUTION,
                DYNAMIC_CUBE_CACHE_SLOTS * CUBE_FACES,
            )
        });
        let spot_views = cache_views(spot_texture.as_ref(), DYNAMIC_SPOT_CACHE_LAYERS);
        let cube_views = cache_views(cube_texture.as_ref(), DYNAMIC_CUBE_CACHE_SLOTS * CUBE_FACES);
        Self {
            state: DynamicDepthCache::default(),
            allocation,
            spot_texture,
            spot_views,
            cube_texture,
            cube_views,
        }
    }

    pub fn reset_level(&mut self, device: &wgpu::Device, allocation: DynamicCacheAllocation) {
        if self.allocation != allocation {
            *self = Self::new(device, allocation);
        } else {
            self.state.reset_level();
        }
    }

    pub fn spot_view(&self, plan: DynamicSpotPlan) -> &wgpu::TextureView {
        &self.spot_views[plan.cache_layer as usize]
    }

    pub fn cube_view(&self, plan: DynamicCubePlan, face: usize) -> &wgpu::TextureView {
        &self.cube_views[DynamicDepthCache::cube_face_layer(plan, face) as usize]
    }
}

fn cache_texture(
    device: &wgpu::Device,
    label: &str,
    resolution: u32,
    layers: usize,
) -> wgpu::Texture {
    device.create_texture(&wgpu::TextureDescriptor {
        label: Some(label),
        size: wgpu::Extent3d {
            width: resolution,
            height: resolution,
            depth_or_array_layers: layers as u32,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: SHADOW_DEPTH_FORMAT,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
        view_formats: &[],
    })
}

fn cache_views(texture: Option<&wgpu::Texture>, layers: usize) -> Vec<wgpu::TextureView> {
    let Some(texture) = texture else {
        return Vec::new();
    };
    (0..layers)
        .map(|layer| {
            texture.create_view(&wgpu::TextureViewDescriptor {
                label: Some("Dynamic World Depth Cache Attachment"),
                dimension: Some(wgpu::TextureViewDimension::D2),
                base_array_layer: layer as u32,
                array_layer_count: Some(1),
                ..Default::default()
            })
        })
        .collect()
}

/// Restore immutable world depth before entity draws. The copy overwrites the
/// prior frame's entity depth, then a Load pass adds only current occluders.
/// With nearest per-tap comparisons, sampling this minimum-depth pool is
/// equivalent to taking min(world comparison, entity comparison) per tap.
pub(super) fn copy_cached_world_depth(
    encoder: &mut wgpu::CommandEncoder,
    source: &wgpu::Texture,
    source_layer: u32,
    destination: &wgpu::Texture,
    destination_layer: u32,
    resolution: u32,
) {
    encoder.copy_texture_to_texture(
        wgpu::TexelCopyTextureInfo {
            texture: source,
            mip_level: 0,
            origin: wgpu::Origin3d {
                x: 0,
                y: 0,
                z: source_layer,
            },
            aspect: wgpu::TextureAspect::DepthOnly,
        },
        wgpu::TexelCopyTextureInfo {
            texture: destination,
            mip_level: 0,
            origin: wgpu::Origin3d {
                x: 0,
                y: 0,
                z: destination_layer,
            },
            aspect: wgpu::TextureAspect::DepthOnly,
        },
        wgpu::Extent3d {
            width: resolution,
            height: resolution,
            depth_or_array_layers: 1,
        },
    );
}

impl Default for DynamicDepthCache {
    fn default() -> Self {
        Self {
            spot: [LayerState::default(); DYNAMIC_SPOT_CACHE_LAYERS],
            cube: [LayerState::default(); DYNAMIC_CUBE_CACHE_SLOTS],
        }
    }
}

impl DynamicDepthCache {
    pub fn reset_level(&mut self) {
        self.spot.fill(LayerState::default());
        self.cube.fill(LayerState::default());
    }

    pub fn plan_frame(
        &mut self,
        spots: &[(u32, usize, Mat4)],
        cubes: &[(u32, usize, [Mat4; CUBE_FACES])],
    ) -> DynamicDepthCachePlan {
        // Rescan the bounded slot inputs for each cache layer. This avoids
        // allocating temporary key vectors every frame (at most 3*96 + 4*6).
        retain_active(&mut self.spot, |key| {
            spots
                .iter()
                .any(|(_, source, matrix)| CacheKey::spot(*source, *matrix) == key)
        });
        retain_active(&mut self.cube, |key| {
            cubes
                .iter()
                .any(|(_, source, matrices)| CacheKey::cube(*source, *matrices) == key)
        });
        let mut plan = DynamicDepthCachePlan::default();
        for &(slot, source, matrix) in spots {
            if let Some(layer) = assign(&mut self.spot, CacheKey::spot(source, matrix)) {
                plan.spot[plan.spot_count] = DynamicSpotPlan {
                    slot,
                    cache_layer: layer as i32,
                    needs_world_render: !self.spot[layer].warm,
                };
                plan.spot_count += 1;
            }
        }
        for &(slot, source, matrices) in cubes {
            if let Some(layer) = assign(&mut self.cube, CacheKey::cube(source, matrices)) {
                plan.cube[plan.cube_count] = DynamicCubePlan {
                    slot,
                    cache_slot: layer as i32,
                    needs_world_render: !self.cube[layer].warm,
                };
                plan.cube_count += 1;
            }
        }
        plan
    }

    pub fn mark_spot_world_rendered(&mut self, plan: DynamicSpotPlan) {
        if plan.cache_layer >= 0 {
            self.spot[plan.cache_layer as usize].warm = true;
        }
    }
    pub fn mark_cube_world_rendered(&mut self, plan: DynamicCubePlan) {
        if plan.cache_slot >= 0 {
            self.cube[plan.cache_slot as usize].warm = true;
        }
    }
    pub fn cube_face_layer(plan: DynamicCubePlan, face: usize) -> i32 {
        plan.cache_slot * CUBE_FACES as i32 + face as i32
    }
}

fn retain_active(layers: &mut [LayerState], active: impl Fn(CacheKey) -> bool) {
    for layer in layers {
        if layer.key.is_some_and(|key| !active(key)) {
            *layer = LayerState::default();
        }
    }
}
fn assign(layers: &mut [LayerState], key: CacheKey) -> Option<usize> {
    if let Some((index, _)) = layers
        .iter()
        .enumerate()
        .find(|(_, layer)| layer.key == Some(key))
    {
        return Some(index);
    }
    let (index, layer) = layers
        .iter_mut()
        .enumerate()
        .find(|(_, layer)| layer.key.is_none())?;
    layer.key = Some(key);
    layer.warm = false;
    Some(index)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn matrix(seed: f32) -> Mat4 {
        Mat4::from_scale(glam::Vec3::splat(seed))
    }

    #[test]
    fn warm_dynamic_spot_skips_world_render_and_retains_its_layer() {
        let mut cache = DynamicDepthCache::default();
        let first = cache.plan_frame(&[(3, 10, matrix(1.0))], &[]).spot()[0];
        assert!(first.needs_world_render);
        cache.mark_spot_world_rendered(first);
        let second = cache.plan_frame(&[(3, 10, matrix(1.0))], &[]).spot()[0];
        assert_eq!(second.cache_layer, first.cache_layer);
        assert!(!second.needs_world_render);
    }

    #[test]
    fn slot_reassignment_retains_cache_layer() {
        let mut cache = DynamicDepthCache::default();
        let first = cache.plan_frame(&[(3, 10, matrix(1.0))], &[]).spot()[0];
        cache.mark_spot_world_rendered(first);
        let moved = cache.plan_frame(&[(5, 10, matrix(1.0))], &[]).spot()[0];
        assert_eq!(moved.cache_layer, first.cache_layer);
        assert!(!moved.needs_world_render);
    }

    #[test]
    fn occupant_or_projection_change_invalidates_dynamic_layer() {
        let mut cache = DynamicDepthCache::default();
        let first = cache.plan_frame(&[(3, 10, matrix(1.0))], &[]).spot()[0];
        cache.mark_spot_world_rendered(first);
        assert!(cache.plan_frame(&[(3, 11, matrix(1.0))], &[]).spot()[0].needs_world_render);
        let restored = cache.plan_frame(&[(3, 10, matrix(1.0))], &[]).spot()[0];
        cache.mark_spot_world_rendered(restored);
        assert!(cache.plan_frame(&[(3, 10, matrix(2.0))], &[]).spot()[0].needs_world_render);
    }

    #[test]
    fn overflow_falls_back_without_displacing_stable_winners() {
        let mut cache = DynamicDepthCache::default();
        let candidates: Vec<_> = (0..DYNAMIC_SPOT_CACHE_LAYERS + 1)
            .map(|i| (i as u32, i, matrix(1.0)))
            .collect();
        let first = cache.plan_frame(&candidates, &[]);
        assert_eq!(first.spot().len(), DYNAMIC_SPOT_CACHE_LAYERS);
        for plan in first.spot().iter().copied() {
            cache.mark_spot_world_rendered(plan);
        }
        let second = cache.plan_frame(&candidates, &[]);
        assert_eq!(second.spot().len(), DYNAMIC_SPOT_CACHE_LAYERS);
        assert!(second.spot().iter().all(|plan| !plan.needs_world_render));
    }

    #[test]
    fn cube_cache_uses_whole_six_face_units_and_resets_on_level_change() {
        let mut cache = DynamicDepthCache::default();
        let cube = [matrix(1.0); CUBE_FACES];
        let first = cache.plan_frame(&[], &[(2, 7, cube)]).cube()[0];
        assert_eq!(DynamicDepthCache::cube_face_layer(first, 0), 0);
        assert_eq!(
            DynamicDepthCache::cube_face_layer(first, CUBE_FACES - 1),
            (CUBE_FACES - 1) as i32
        );
        cache.mark_cube_world_rendered(first);
        assert!(!cache.plan_frame(&[], &[(2, 7, cube)]).cube()[0].needs_world_render);
        cache.reset_level();
        assert!(cache.plan_frame(&[], &[(2, 7, cube)]).cube()[0].needs_world_render);
    }

    #[test]
    fn dynamic_cache_budget_matches_campaign_wave_peak() {
        // The scripted campaign waves peak at two dynamic spots and four
        // dynamic points; retain one spare spot while reserving exactly four
        // whole cube units. These are cache budgets, not active-light caps.
        assert_eq!(DYNAMIC_SPOT_CACHE_LAYERS, 3);
        assert_eq!(DYNAMIC_CUBE_CACHE_SLOTS, 4);
    }

    #[test]
    fn moved_light_does_not_leave_cache_layer_on_old_slot() {
        let mut cache = DynamicDepthCache::default();
        let first = cache.plan_frame(&[(3, 10, matrix(1.0))], &[]);
        assert!(first.spot_for_slot(3).is_some());
        cache.mark_spot_world_rendered(first.spot()[0]);
        let moved = cache.plan_frame(&[(5, 10, matrix(1.0))], &[]);
        assert!(moved.spot_for_slot(3).is_none());
        assert!(moved.spot_for_slot(5).is_some());
    }

    #[test]
    fn warm_dynamic_spot_skips_world_render_and_cull() {
        let mut cache = DynamicDepthCache::default();
        let cold = cache.plan_frame(&[(7, 10, matrix(1.0))], &[]);
        cache.mark_spot_world_rendered(cold.spot()[0]);
        let warm = cache.plan_frame(&[(7, 10, matrix(1.0))], &[]);
        assert!(!warm.spot()[0].needs_world_render);
        assert!(!warm.should_dispatch_spot_cull(7));
    }

    #[test]
    fn dynamic_cache_reset_makes_recycled_source_cold() {
        let mut cache = DynamicDepthCache::default();
        let first = cache.plan_frame(&[(7, 10, matrix(1.0))], &[]).spot()[0];
        cache.mark_spot_world_rendered(first);
        cache.reset_level();
        assert!(cache.plan_frame(&[(7, 10, matrix(1.0))], &[]).spot()[0].needs_world_render);
    }

    #[test]
    fn warm_dynamic_cube_skips_all_six_faces() {
        let mut cache = DynamicDepthCache::default();
        let faces = [matrix(1.0); CUBE_FACES];
        let cold = cache.plan_frame(&[], &[(2, 7, faces)]);
        cache.mark_cube_world_rendered(cold.cube()[0]);
        let warm = cache.plan_frame(&[], &[(2, 7, faces)]);
        assert!(!warm.cube()[0].needs_world_render);
        for face in 0..CUBE_FACES {
            assert!(!warm.should_dispatch_cube_cull(2 * CUBE_FACES + face));
        }
    }

    #[test]
    fn cube_budget_overflow_never_claims_partial_faces() {
        let mut cache = DynamicDepthCache::default();
        let candidates: Vec<_> = (0..DYNAMIC_CUBE_CACHE_SLOTS + 1)
            .map(|slot| (slot as u32, slot, [matrix(1.0); CUBE_FACES]))
            .collect();
        let plan = cache.plan_frame(&[], &candidates);
        assert_eq!(plan.cube().len(), DYNAMIC_CUBE_CACHE_SLOTS);
        for entry in plan.cube().iter().copied() {
            for face in 0..CUBE_FACES {
                assert_eq!(
                    DynamicDepthCache::cube_face_layer(entry, face),
                    entry.cache_slot * CUBE_FACES as i32 + face as i32,
                );
            }
        }
    }

    #[test]
    fn empty_geometry_cache_fill_clears_and_warms_layer() {
        // The depth recorder clears before its world draw gate, so a no-geometry
        // cold fill still becomes a valid far-depth cache layer for the next frame.
        let mut cache = DynamicDepthCache::default();
        let cold = cache.plan_frame(&[(0, 1, matrix(1.0))], &[]).spot()[0];
        assert!(cold.needs_world_render);
        cache.mark_spot_world_rendered(cold);
        assert!(!cache.plan_frame(&[(0, 1, matrix(1.0))], &[]).spot()[0].needs_world_render);
    }

    #[test]
    fn dynamic_cache_does_not_spend_shader_texture_bindings() {
        for shader in [
            include_str!("../shaders/forward.wgsl"),
            include_str!("../shaders/skinned_mesh.wgsl"),
            include_str!("../shaders/kinematic_brush.wgsl"),
        ] {
            assert!(!shader.contains("dynamic_spot_depth_cache"));
            assert!(!shader.contains("dynamic_cube_depth_cache"));
            assert!(shader.contains("sample_spot_shadow("));
            assert!(shader.contains("sample_point_shadow("));
        }
    }

    #[test]
    fn cached_depth_is_copied_before_entity_pass_loads_it() {
        let recorder = include_str!("renderer_dynamic_shadow_passes.rs");
        for label in [
            "Dynamic Spot Entity Shadow Depth Pass",
            "Dynamic Cube Entity Shadow Depth Pass",
        ] {
            let (before, after) = recorder.split_once(label).unwrap();
            assert!(before.rsplit_once("copy_cached_world_depth(").is_some());
            let attachment = after.split_once("timestamp_writes:").unwrap().0;
            assert!(attachment.contains("load: wgpu::LoadOp::Load"));
            assert!(!attachment.contains("LoadOp::Clear"));
        }
        assert!(
            include_str!("../lighting/spot_shadow.rs").contains("wgpu::TextureUsages::COPY_DST")
        );
        assert!(
            include_str!("../lighting/cube_shadow.rs").contains("wgpu::TextureUsages::COPY_DST")
        );
    }
}
