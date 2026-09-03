// Dynamic-tier world-depth cache planning. GPU resources and recording stay in
// the renderer passes; this module keeps the cross-frame cache state pure.
use glam::Mat4;

use crate::lighting::cube_shadow::CUBE_FACE_RESOLUTION;
use crate::lighting::cube_shadow::CUBE_FACES;
use crate::lighting::spot_shadow::{SHADOW_DEPTH_FORMAT, SHADOW_MAP_RESOLUTION, SHADOW_POOL_SIZE};

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

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) struct DynamicSpotPlan {
    pub slot: u32,
    pub cache_layer: i32,
    pub needs_world_render: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) struct DynamicCubePlan {
    pub slot: u32,
    pub cache_slot: i32,
    pub needs_world_render: bool,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub(super) struct DynamicDepthCachePlan {
    pub spot: Vec<DynamicSpotPlan>,
    pub cube: Vec<DynamicCubePlan>,
}

impl DynamicDepthCachePlan {
    pub fn spot_for_slot(&self, slot: u32) -> Option<DynamicSpotPlan> {
        self.spot.iter().copied().find(|plan| plan.slot == slot)
    }

    pub fn should_dispatch_spot_cull(&self, slot: usize) -> bool {
        !self
            .spot
            .iter()
            .any(|plan| plan.slot as usize == slot && !plan.needs_world_render)
    }

    pub fn cube_for_slot(&self, slot: u32) -> Option<DynamicCubePlan> {
        self.cube.iter().copied().find(|plan| plan.slot == slot)
    }

    pub fn should_dispatch_cube_cull(&self, layer: usize) -> bool {
        let slot = layer / CUBE_FACES;
        !self
            .cube
            .iter()
            .any(|plan| plan.slot as usize == slot && !plan.needs_world_render)
    }
}

/// Build the slot-indexed cache-layer channel afresh. `-1` means pool-only;
/// callers never retain a previous frame's assignment for a vacated slot.
pub(super) fn spot_layer_channel(plan: &DynamicDepthCachePlan) -> [i32; SHADOW_POOL_SIZE] {
    let mut layers = [-1; SHADOW_POOL_SIZE];
    for entry in &plan.spot {
        layers[entry.slot as usize] = entry.cache_layer;
    }
    layers
}

pub(super) struct DynamicDepthCache {
    spot: Vec<LayerState>,
    cube: Vec<LayerState>,
}

/// GPU ownership for the dynamic-tier cache. This is deliberately separate
/// from the shared group-5 pool bindings: forward, meshes, and movers each bind
/// this cache through their own dedicated layout.
pub(super) struct DynamicDepthCacheGpu {
    pub state: DynamicDepthCache,
    spot_views: Vec<wgpu::TextureView>,
    #[allow(dead_code)] // retained by the bind group; wgpu ownership is indirect.
    pub spot_sampled_view: wgpu::TextureView,
    cube_views: Vec<wgpu::TextureView>,
    #[allow(dead_code)] // retained by the bind group; wgpu ownership is indirect.
    pub cube_sampled_view: Option<wgpu::TextureView>,
    pub spot_layers_buffer: wgpu::Buffer,
    pub cube_layers_buffer: wgpu::Buffer,
    pub bind_group: wgpu::BindGroup,
}

impl DynamicDepthCacheGpu {
    pub fn bind_group_layout(
        device: &wgpu::Device,
        cube_array_supported: bool,
    ) -> wgpu::BindGroupLayout {
        let mut entries = vec![
            wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Texture {
                    sample_type: wgpu::TextureSampleType::Depth,
                    view_dimension: wgpu::TextureViewDimension::D2Array,
                    multisampled: false,
                },
                count: None,
            },
            wgpu::BindGroupLayoutEntry {
                binding: 1,
                visibility: wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Comparison),
                count: None,
            },
            wgpu::BindGroupLayoutEntry {
                binding: 2,
                visibility: wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: false,
                    min_binding_size: std::num::NonZeroU64::new((SHADOW_POOL_SIZE * 4) as u64),
                },
                count: None,
            },
            wgpu::BindGroupLayoutEntry {
                binding: 3,
                visibility: wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: false,
                    min_binding_size: std::num::NonZeroU64::new(
                        (crate::lighting::cube_shadow::CUBE_COUNT * 4) as u64,
                    ),
                },
                count: None,
            },
        ];
        if cube_array_supported {
            entries.push(wgpu::BindGroupLayoutEntry {
                binding: 4,
                visibility: wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Texture {
                    sample_type: wgpu::TextureSampleType::Depth,
                    view_dimension: wgpu::TextureViewDimension::CubeArray,
                    multisampled: false,
                },
                count: None,
            });
        }
        device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("Dynamic World Depth Cache BGL"),
            entries: &entries,
        })
    }

    pub fn new(
        device: &wgpu::Device,
        bind_group_layout: &wgpu::BindGroupLayout,
        cube_array_supported: bool,
    ) -> Self {
        let spot_texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("Dynamic Spot World Depth Cache"),
            size: wgpu::Extent3d {
                width: SHADOW_MAP_RESOLUTION,
                height: SHADOW_MAP_RESOLUTION,
                depth_or_array_layers: DYNAMIC_SPOT_CACHE_LAYERS as u32,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: SHADOW_DEPTH_FORMAT,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING,
            view_formats: &[],
        });
        let spot_views = (0..DYNAMIC_SPOT_CACHE_LAYERS)
            .map(|layer| {
                spot_texture.create_view(&wgpu::TextureViewDescriptor {
                    label: Some(&format!("Dynamic Spot World Depth Cache View {layer}")),
                    dimension: Some(wgpu::TextureViewDimension::D2),
                    base_array_layer: layer as u32,
                    array_layer_count: Some(1),
                    ..Default::default()
                })
            })
            .collect();
        let spot_sampled_view = spot_texture.create_view(&wgpu::TextureViewDescriptor {
            label: Some("Dynamic Spot World Depth Cache Array"),
            dimension: Some(wgpu::TextureViewDimension::D2Array),
            base_array_layer: 0,
            array_layer_count: Some(DYNAMIC_SPOT_CACHE_LAYERS as u32),
            ..Default::default()
        });
        let cube_layers = (DYNAMIC_CUBE_CACHE_SLOTS * CUBE_FACES) as u32;
        let cube_texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("Dynamic Cube World Depth Cache"),
            size: wgpu::Extent3d {
                width: CUBE_FACE_RESOLUTION,
                height: CUBE_FACE_RESOLUTION,
                depth_or_array_layers: cube_layers,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: SHADOW_DEPTH_FORMAT,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING,
            view_formats: &[],
        });
        let cube_views = (0..cube_layers)
            .map(|layer| {
                cube_texture.create_view(&wgpu::TextureViewDescriptor {
                    label: Some(&format!("Dynamic Cube World Depth Cache Face {layer}")),
                    dimension: Some(wgpu::TextureViewDimension::D2Array),
                    base_array_layer: layer,
                    array_layer_count: Some(1),
                    ..Default::default()
                })
            })
            .collect();
        let cube_sampled_view = cube_array_supported.then(|| {
            cube_texture.create_view(&wgpu::TextureViewDescriptor {
                label: Some("Dynamic Cube World Depth Cache Array"),
                dimension: Some(wgpu::TextureViewDimension::CubeArray),
                base_array_layer: 0,
                array_layer_count: Some(cube_layers),
                ..Default::default()
            })
        });
        let spot_layers_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("Dynamic Spot Cache Layers"),
            size: (SHADOW_POOL_SIZE * 4) as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let cube_layers_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("Dynamic Cube Cache Layers"),
            size: (crate::lighting::cube_shadow::CUBE_COUNT * 4) as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let compare_sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("Dynamic World Depth Cache Comparison Sampler"),
            compare: Some(wgpu::CompareFunction::Less),
            mag_filter: wgpu::FilterMode::Nearest,
            min_filter: wgpu::FilterMode::Nearest,
            mipmap_filter: wgpu::MipmapFilterMode::Nearest,
            ..Default::default()
        });
        let mut bind_entries = vec![
            wgpu::BindGroupEntry {
                binding: 0,
                resource: wgpu::BindingResource::TextureView(&spot_sampled_view),
            },
            wgpu::BindGroupEntry {
                binding: 1,
                resource: wgpu::BindingResource::Sampler(&compare_sampler),
            },
            wgpu::BindGroupEntry {
                binding: 2,
                resource: spot_layers_buffer.as_entire_binding(),
            },
            wgpu::BindGroupEntry {
                binding: 3,
                resource: cube_layers_buffer.as_entire_binding(),
            },
        ];
        if let Some(view) = &cube_sampled_view {
            bind_entries.push(wgpu::BindGroupEntry {
                binding: 4,
                resource: wgpu::BindingResource::TextureView(view),
            });
        }
        let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("Dynamic World Depth Cache Bind Group"),
            layout: bind_group_layout,
            entries: &bind_entries,
        });
        Self {
            state: DynamicDepthCache::default(),
            spot_views,
            spot_sampled_view,
            cube_views,
            cube_sampled_view,
            spot_layers_buffer,
            cube_layers_buffer,
            bind_group,
        }
    }

    pub fn spot_view(&self, plan: DynamicSpotPlan) -> &wgpu::TextureView {
        &self.spot_views[plan.cache_layer as usize]
    }
    pub fn cube_view(&self, plan: DynamicCubePlan, face: usize) -> &wgpu::TextureView {
        &self.cube_views[DynamicDepthCache::cube_face_layer(plan, face) as usize]
    }
}

impl Default for DynamicDepthCache {
    fn default() -> Self {
        Self {
            spot: vec![LayerState::default(); DYNAMIC_SPOT_CACHE_LAYERS],
            cube: vec![LayerState::default(); DYNAMIC_CUBE_CACHE_SLOTS],
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
        let spot_keys: Vec<_> = spots
            .iter()
            .map(|(_, source, matrix)| CacheKey::spot(*source, *matrix))
            .collect();
        let cube_keys: Vec<_> = cubes
            .iter()
            .map(|(_, source, matrices)| CacheKey::cube(*source, *matrices))
            .collect();
        retain_active(&mut self.spot, &spot_keys);
        retain_active(&mut self.cube, &cube_keys);
        DynamicDepthCachePlan {
            spot: spots
                .iter()
                .filter_map(|(slot, source, matrix)| {
                    let layer = assign(&mut self.spot, CacheKey::spot(*source, *matrix))?;
                    Some(DynamicSpotPlan {
                        slot: *slot,
                        cache_layer: layer as i32,
                        needs_world_render: !self.spot[layer].warm,
                    })
                })
                .collect(),
            cube: cubes
                .iter()
                .filter_map(|(slot, source, matrices)| {
                    let layer = assign(&mut self.cube, CacheKey::cube(*source, *matrices))?;
                    Some(DynamicCubePlan {
                        slot: *slot,
                        cache_slot: layer as i32,
                        needs_world_render: !self.cube[layer].warm,
                    })
                })
                .collect(),
        }
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

fn retain_active(layers: &mut [LayerState], active: &[CacheKey]) {
    for layer in layers {
        if layer.key.is_some_and(|key| !active.contains(&key)) {
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
        let first = cache.plan_frame(&[(3, 10, matrix(1.0))], &[]).spot[0];
        assert!(first.needs_world_render);
        cache.mark_spot_world_rendered(first);
        let second = cache.plan_frame(&[(3, 10, matrix(1.0))], &[]).spot[0];
        assert_eq!(second.cache_layer, first.cache_layer);
        assert!(!second.needs_world_render);
    }

    #[test]
    fn slot_reassignment_retains_cache_layer() {
        let mut cache = DynamicDepthCache::default();
        let first = cache.plan_frame(&[(3, 10, matrix(1.0))], &[]).spot[0];
        cache.mark_spot_world_rendered(first);
        let moved = cache.plan_frame(&[(5, 10, matrix(1.0))], &[]).spot[0];
        assert_eq!(moved.cache_layer, first.cache_layer);
        assert!(!moved.needs_world_render);
    }

    #[test]
    fn occupant_or_projection_change_invalidates_dynamic_layer() {
        let mut cache = DynamicDepthCache::default();
        let first = cache.plan_frame(&[(3, 10, matrix(1.0))], &[]).spot[0];
        cache.mark_spot_world_rendered(first);
        assert!(cache.plan_frame(&[(3, 11, matrix(1.0))], &[]).spot[0].needs_world_render);
        let restored = cache.plan_frame(&[(3, 10, matrix(1.0))], &[]).spot[0];
        cache.mark_spot_world_rendered(restored);
        assert!(cache.plan_frame(&[(3, 10, matrix(2.0))], &[]).spot[0].needs_world_render);
    }

    #[test]
    fn overflow_falls_back_without_displacing_stable_winners() {
        let mut cache = DynamicDepthCache::default();
        let candidates: Vec<_> = (0..DYNAMIC_SPOT_CACHE_LAYERS + 1)
            .map(|i| (i as u32, i, matrix(1.0)))
            .collect();
        let first = cache.plan_frame(&candidates, &[]);
        assert_eq!(first.spot.len(), DYNAMIC_SPOT_CACHE_LAYERS);
        for plan in first.spot {
            cache.mark_spot_world_rendered(plan);
        }
        let second = cache.plan_frame(&candidates, &[]);
        assert_eq!(second.spot.len(), DYNAMIC_SPOT_CACHE_LAYERS);
        assert!(second.spot.iter().all(|plan| !plan.needs_world_render));
    }

    #[test]
    fn cube_cache_uses_whole_six_face_units_and_resets_on_level_change() {
        let mut cache = DynamicDepthCache::default();
        let cube = [matrix(1.0); CUBE_FACES];
        let first = cache.plan_frame(&[], &[(2, 7, cube)]).cube[0];
        assert_eq!(DynamicDepthCache::cube_face_layer(first, 0), 0);
        assert_eq!(
            DynamicDepthCache::cube_face_layer(first, CUBE_FACES - 1),
            (CUBE_FACES - 1) as i32
        );
        cache.mark_cube_world_rendered(first);
        assert!(!cache.plan_frame(&[], &[(2, 7, cube)]).cube[0].needs_world_render);
        cache.reset_level();
        assert!(cache.plan_frame(&[], &[(2, 7, cube)]).cube[0].needs_world_render);
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
        let first_channel = spot_layer_channel(&first);
        assert!(first_channel[3] >= 0);
        cache.mark_spot_world_rendered(first.spot[0]);
        let moved = cache.plan_frame(&[(5, 10, matrix(1.0))], &[]);
        let moved_channel = spot_layer_channel(&moved);
        assert_eq!(moved_channel[3], -1);
        assert!(moved_channel[5] >= 0);
    }

    #[test]
    fn warm_dynamic_spot_skips_world_render_and_cull() {
        let mut cache = DynamicDepthCache::default();
        let cold = cache.plan_frame(&[(7, 10, matrix(1.0))], &[]);
        cache.mark_spot_world_rendered(cold.spot[0]);
        let warm = cache.plan_frame(&[(7, 10, matrix(1.0))], &[]);
        assert!(!warm.spot[0].needs_world_render);
        assert!(!warm.should_dispatch_spot_cull(7));
    }

    #[test]
    fn dynamic_cache_reset_makes_recycled_source_cold() {
        let mut cache = DynamicDepthCache::default();
        let first = cache.plan_frame(&[(7, 10, matrix(1.0))], &[]).spot[0];
        cache.mark_spot_world_rendered(first);
        cache.reset_level();
        assert!(cache.plan_frame(&[(7, 10, matrix(1.0))], &[]).spot[0].needs_world_render);
    }

    #[test]
    fn warm_dynamic_cube_skips_all_six_faces() {
        let mut cache = DynamicDepthCache::default();
        let faces = [matrix(1.0); CUBE_FACES];
        let cold = cache.plan_frame(&[], &[(2, 7, faces)]);
        cache.mark_cube_world_rendered(cold.cube[0]);
        let warm = cache.plan_frame(&[], &[(2, 7, faces)]);
        assert!(!warm.cube[0].needs_world_render);
        for face in 0..CUBE_FACES {
            assert!(!warm.should_dispatch_cube_cull(2 * CUBE_FACES + face));
        }
    }

    #[test]
    fn dynamic_namespace_uses_its_own_bind_group() {
        let forward = include_str!("../shaders/forward.wgsl");
        let mesh = include_str!("../shaders/skinned_mesh.wgsl");
        let mover = include_str!("../shaders/kinematic_brush.wgsl");
        let pipeline = include_str!("renderer_init_pipelines.rs");
        assert!(forward.contains("@group(6) @binding(0) var dynamic_spot_depth_cache"));
        assert!(mesh.contains("@group(5) @binding(0) var dynamic_spot_depth_cache"));
        assert!(mover.contains("@group(5) @binding(0) var dynamic_spot_depth_cache"));
        assert!(pipeline.contains("Some(dynamic_depth_cache_bgl)"));
    }
}
