// Dynamic spot light shadow-map pool and slot allocation.
//
// See: context/plans/in-progress/lighting-spot-shadows/index.md § Task A
//      context/lib/rendering_pipeline.md §4

use crate::prl::{LightType, MapLight};
use glam::{Mat4, Vec3};

/// Near-clip distance used when building a spot light's projection matrix.
/// Matches the camera near-clip policy — close enough that self-shadowing
/// acne is controlled by the depth bias, far enough to keep precision.
pub const SHADOW_NEAR_CLIP: f32 = 0.1;

/// Build a light-space view-projection matrix for a spot light, producing
/// NDC that the forward shader converts to `[0, 1]` UVs for sampling.
///
/// `far` clamps to `falloff_range` but we enforce a minimum so zero-range
/// or degenerate lights don't produce a zero-extent frustum.
pub fn light_space_matrix(light: &MapLight) -> Mat4 {
    let eye = Vec3::new(
        light.origin[0] as f32,
        light.origin[1] as f32,
        light.origin[2] as f32,
    );
    let mut dir = Vec3::new(
        light.cone_direction[0],
        light.cone_direction[1],
        light.cone_direction[2],
    );
    if dir.length_squared() < 1e-8 {
        dir = Vec3::new(0.0, 0.0, -1.0);
    } else {
        dir = dir.normalize();
    }
    // Pick an up vector not colinear with `dir`.
    let world_up = if dir.y.abs() > 0.99 {
        Vec3::new(0.0, 0.0, 1.0)
    } else {
        Vec3::new(0.0, 1.0, 0.0)
    };
    let target = eye + dir;
    let view = Mat4::look_at_rh(eye, target, world_up);

    let fov_y = (2.0 * light.cone_angle_outer).max(0.05);
    let far = light.falloff_range.max(0.5);
    // `perspective_rh` in glam targets Vulkan/D3D/Metal depth range [0, 1].
    let proj = Mat4::perspective_rh(fov_y, 1.0, SHADOW_NEAR_CLIP, far);
    proj * view
}

/// Number of shadow-map slots in the pool (retunable constant).
pub const SHADOW_POOL_SIZE: usize = 12;

/// Depth format for shadow maps.
pub const SHADOW_DEPTH_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Depth32Float;

/// Resolution of each shadow map in the pool (1024×1024).
pub const SHADOW_MAP_RESOLUTION: u32 = 1024;

/// Sentinel value written to the slot index field when no slot is allocated.
pub const NO_SHADOW_SLOT: u32 = 0xFFFFFFFF;

/// Size of the `array<mat4x4<f32>, 8>` storage buffer consumed by the
/// forward shader at `@group(5) @binding(2)`. Eight 4×4 f32 matrices.
pub const LIGHT_SPACE_MATRICES_SIZE: u64 = (SHADOW_POOL_SIZE * 16 * 4) as u64;

/// Pool of shadow-map texture slots, one per dynamic spot light that
/// passes visibility culling. Ranked by projected influence area each frame.
///
/// Owns the group 5 resources the forward shader binds: the shadow depth
/// array (as a D2Array view), the comparison sampler, and the light-space
/// matrix storage buffer. `matrices` is sized for all 8 slots; slots that
/// aren't assigned in a given frame are left at whatever was last written
/// (the fragment shader gates on the per-light slot sentinel so those
/// stale entries are never sampled).
pub struct SpotShadowPool {
    /// Array texture with SHADOW_POOL_SIZE layers, each SHADOW_MAP_RESOLUTION×SHADOW_MAP_RESOLUTION.
    /// Held for ownership — actual access goes through `views` and `bind_group`.
    #[allow(dead_code)]
    pub array_texture: wgpu::Texture,
    /// Texture views for each slot (2D views for render attachments).
    pub views: Vec<wgpu::TextureView>,
    /// D2Array view of `array_texture`, bound at `@group(5) @binding(0)` for sampling.
    /// Held for ownership — `bind_group` references it.
    #[allow(dead_code)]
    pub array_view: wgpu::TextureView,
    /// Comparison sampler bound at `@group(5) @binding(1)`.
    /// Held for ownership — `bind_group` references it.
    #[allow(dead_code)]
    pub compare_sampler: wgpu::Sampler,
    /// Storage buffer of 8 `mat4x4<f32>` bound at `@group(5) @binding(2)`.
    /// Contains light-space view-projection matrices per slot.
    pub matrices_buffer: wgpu::Buffer,
    /// Bind group for group 5 — lives alongside the resources above.
    pub bind_group: wgpu::BindGroup,
    /// Per-frame slot assignment: slot_assignment[light_index] = slot (0..8) or NO_SHADOW_SLOT.
    pub slot_assignment: Vec<u32>,
}

impl SpotShadowPool {
    /// Build the bind group layout for `@group(5)` of the forward shader.
    pub fn bind_group_layout(device: &wgpu::Device) -> wgpu::BindGroupLayout {
        device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("Spot Shadow BGL"),
            entries: &[
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
                        min_binding_size: std::num::NonZeroU64::new(LIGHT_SPACE_MATRICES_SIZE),
                    },
                    count: None,
                },
            ],
        })
    }

    /// Allocate the shadow-map pool at renderer init.
    ///
    /// Creates a single array texture with `SHADOW_POOL_SIZE` layers,
    /// each `SHADOW_MAP_RESOLUTION × SHADOW_MAP_RESOLUTION` Depth32Float,
    /// along with the sampler, matrix buffer, and bind group that the
    /// forward shader's `@group(5)` layout expects.
    pub fn new(device: &wgpu::Device, layout: &wgpu::BindGroupLayout) -> Self {
        let array_texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("Spot Shadow Map Array"),
            size: wgpu::Extent3d {
                width: SHADOW_MAP_RESOLUTION,
                height: SHADOW_MAP_RESOLUTION,
                depth_or_array_layers: SHADOW_POOL_SIZE as u32,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: SHADOW_DEPTH_FORMAT,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING,
            view_formats: &[],
        });

        // Per-layer 2D views used as render attachments in the shadow pass.
        let views: Vec<wgpu::TextureView> = (0..SHADOW_POOL_SIZE)
            .map(|i| {
                array_texture.create_view(&wgpu::TextureViewDescriptor {
                    label: Some(&format!("Spot Shadow Map View {}", i)),
                    dimension: Some(wgpu::TextureViewDimension::D2),
                    base_array_layer: i as u32,
                    array_layer_count: Some(1u32),
                    ..Default::default()
                })
            })
            .collect();

        // D2Array view used by the forward shader for sampling.
        let array_view = array_texture.create_view(&wgpu::TextureViewDescriptor {
            label: Some("Spot Shadow Array View"),
            dimension: Some(wgpu::TextureViewDimension::D2Array),
            base_array_layer: 0,
            array_layer_count: Some(SHADOW_POOL_SIZE as u32),
            ..Default::default()
        });

        // `CompareFunction::Less`: textureSampleCompare returns 1.0 (lit)
        // when the fragment's depth is less than the stored (light-nearest)
        // depth — i.e. the fragment is closer than the shadow caster, so
        // it's not occluded.
        let compare_sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("Spot Shadow Compare Sampler"),
            address_mode_u: wgpu::AddressMode::ClampToEdge,
            address_mode_v: wgpu::AddressMode::ClampToEdge,
            address_mode_w: wgpu::AddressMode::ClampToEdge,
            mag_filter: wgpu::FilterMode::Nearest,
            min_filter: wgpu::FilterMode::Nearest,
            mipmap_filter: wgpu::MipmapFilterMode::Nearest,
            compare: Some(wgpu::CompareFunction::Less),
            ..Default::default()
        });

        let matrices_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("Spot Shadow Light-Space Matrices"),
            size: LIGHT_SPACE_MATRICES_SIZE,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("Spot Shadow Bind Group"),
            layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(&array_view),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::Sampler(&compare_sampler),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: matrices_buffer.as_entire_binding(),
                },
            ],
        });

        Self {
            array_texture,
            views,
            array_view,
            compare_sampler,
            matrices_buffer,
            bind_group,
            slot_assignment: Vec::new(),
        }
    }

    /// Compute the slot-assignment ranking for visible dynamic spot lights.
    ///
    /// Takes the full light list and camera position. Identifies dynamic spot lights
    /// that pass frustum culling, ranks by influence-area heuristic, and assigns the
    /// top 8 to slots.
    ///
    /// Returns a Vec indexed by light index: entry is the slot index (0..8) or NO_SHADOW_SLOT.
    pub fn rank_lights(
        lights: &[MapLight],
        camera_position: Vec3,
        camera_near_clip: f32,
        eligible_lights: &[bool],
        influence_volumes: &[crate::lighting::influence::LightInfluence],
    ) -> Vec<u32> {
        let mut slot_assignment = vec![NO_SHADOW_SLOT; lights.len()];

        // Collect visible dynamic spot lights with their scores.
        let mut candidates: Vec<(usize, f32)> = lights
            .iter()
            .enumerate()
            .filter_map(|(idx, light)| {
                // Only consider dynamic spot lights.
                if !light.is_dynamic || light.light_type != LightType::Spot {
                    return None;
                }

                // Per-light eligibility gate. The caller folds visibility and
                // animated-brightness suppression into this slice; an empty
                // (or short) slice is treated as all-eligible so existing
                // tests and the first-frame pre-bridge call keep working.
                if idx < eligible_lights.len() && !eligible_lights[idx] {
                    return None;
                }

                // Apply frustum-cull pre-filter via influence volume. If no influence
                // volumes are available, treat the light as visible.
                let in_frustum = if idx < influence_volumes.len() {
                    let inf = &influence_volumes[idx];
                    inf.is_in_frustum_approx(camera_position)
                } else {
                    // No influence volume data; assume visible.
                    true
                };

                if !in_frustum {
                    return None;
                }

                // Compute heuristic score: (falloff_range / max(distance, near_clip))^2
                let light_pos = Vec3::new(
                    light.origin[0] as f32,
                    light.origin[1] as f32,
                    light.origin[2] as f32,
                );
                let dist = (light_pos - camera_position).length();
                let denom = dist.max(camera_near_clip);
                let score = (light.falloff_range / denom).powi(2);

                Some((idx, score))
            })
            .collect();

        // Sort by score (descending), then by index (ascending) for determinism.
        candidates.sort_by(|a, b| {
            b.1.partial_cmp(&a.1)
                .unwrap_or(std::cmp::Ordering::Equal)
                .then(a.0.cmp(&b.0))
        });

        // Assign top 8 to slots.
        for (slot, (light_idx, _score)) in candidates.iter().take(SHADOW_POOL_SIZE).enumerate() {
            slot_assignment[*light_idx] = slot as u32;
            log::debug!("[ShadowPool] light {} → slot {}", light_idx, slot);
        }

        if candidates.len() > SHADOW_POOL_SIZE {
            log::debug!(
                "[ShadowPool] {} dynamic spot lights visible; {} assigned to slots, {} unshadowed",
                candidates.len(),
                SHADOW_POOL_SIZE,
                candidates.len() - SHADOW_POOL_SIZE
            );
        }

        slot_assignment
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_light(_idx: u32, origin: [f64; 3], falloff_range: f32, is_dynamic: bool) -> MapLight {
        MapLight {
            origin,
            light_type: LightType::Spot,
            intensity: 1.0,
            color: [1.0, 1.0, 1.0],
            falloff_model: crate::prl::FalloffModel::Linear,
            falloff_range,
            cone_angle_inner: 0.3,
            cone_angle_outer: 0.6,
            cone_direction: [0.0, 0.0, -1.0],
            cast_shadows: true,
            is_dynamic,
            tag: None,
            leaf_index: 0,
        }
    }

    #[test]
    fn empty_light_list_produces_empty_assignment() {
        let assignment = SpotShadowPool::rank_lights(&[], Vec3::ZERO, 0.1, &[], &[]);
        assert!(assignment.is_empty());
    }

    #[test]
    fn non_dynamic_lights_are_not_assigned() {
        let lights = vec![
            test_light(0, [0.0, 0.0, 0.0], 10.0, false),
            test_light(1, [10.0, 0.0, 0.0], 10.0, false),
        ];
        let assignment = SpotShadowPool::rank_lights(&lights, Vec3::ZERO, 0.1, &[], &[]);
        assert_eq!(assignment[0], NO_SHADOW_SLOT);
        assert_eq!(assignment[1], NO_SHADOW_SLOT);
    }

    #[test]
    fn point_lights_are_not_assigned() {
        let mut light = test_light(0, [0.0, 0.0, 0.0], 10.0, true);
        light.light_type = LightType::Point;
        let lights = vec![light];
        let assignment = SpotShadowPool::rank_lights(&lights, Vec3::ZERO, 0.1, &[], &[]);
        assert_eq!(assignment[0], NO_SHADOW_SLOT);
    }

    #[test]
    fn two_dynamic_spots_both_assigned() {
        let lights = vec![
            test_light(0, [0.0, 0.0, 0.0], 10.0, true),
            test_light(1, [10.0, 0.0, 0.0], 10.0, true),
        ];
        let assignment = SpotShadowPool::rank_lights(&lights, Vec3::ZERO, 0.1, &[], &[]);
        assert_ne!(assignment[0], NO_SHADOW_SLOT);
        assert_ne!(assignment[1], NO_SHADOW_SLOT);
        // Should be different slots.
        assert_ne!(assignment[0], assignment[1]);
    }

    #[test]
    fn nine_lights_all_assigned_when_pool_has_capacity() {
        let mut lights = Vec::new();
        for i in 0..9 {
            lights.push(test_light(
                i as u32,
                [i as f64 * 10.0, 0.0, 0.0],
                10.0,
                true,
            ));
        }
        let assignment = SpotShadowPool::rank_lights(&lights, Vec3::ZERO, 0.1, &[], &[]);

        let assigned_count = assignment.iter().filter(|&&s| s != NO_SHADOW_SLOT).count();
        assert_eq!(assigned_count, 9, "all 9 lights fit within pool capacity");

        let unshadowed_count = assignment.iter().filter(|&&s| s == NO_SHADOW_SLOT).count();
        assert_eq!(unshadowed_count, 0, "no lights left unshadowed");
    }

    #[test]
    fn closer_light_ranks_higher() {
        // Light 0 at origin is much closer than light 1 at distance 100.
        let lights = vec![
            test_light(0, [0.0, 0.0, 0.0], 10.0, true),
            test_light(1, [100.0, 0.0, 0.0], 10.0, true),
        ];
        let assignment = SpotShadowPool::rank_lights(&lights, Vec3::ZERO, 0.1, &[], &[]);
        // Light 0 should get slot 0 (lower index = higher rank).
        assert_eq!(assignment[0], 0);
        assert_eq!(assignment[1], 1);
    }

    #[test]
    fn larger_falloff_ranks_higher() {
        // Both at same distance; light 0 has larger falloff_range.
        let lights = vec![
            test_light(0, [0.0, 0.0, -10.0], 20.0, true),
            test_light(1, [0.0, 0.0, -10.0], 10.0, true),
        ];
        let assignment = SpotShadowPool::rank_lights(&lights, Vec3::ZERO, 0.1, &[], &[]);
        // Light 0 (larger range) should get slot 0.
        assert_eq!(assignment[0], 0);
        assert_eq!(assignment[1], 1);
    }

    #[test]
    fn ties_broken_by_light_index() {
        // Two lights with identical distance and falloff_range.
        let lights = vec![
            test_light(0, [10.0, 0.0, 0.0], 10.0, true),
            test_light(1, [10.0, 0.0, 0.0], 10.0, true),
        ];
        let assignment = SpotShadowPool::rank_lights(&lights, Vec3::ZERO, 0.1, &[], &[]);
        // Light 0 (lower index) should get slot 0; light 1 gets slot 1.
        assert_eq!(assignment[0], 0);
        assert_eq!(assignment[1], 1);
    }

    #[test]
    fn lights_in_invisible_cells_are_culled() {
        let lights = vec![
            test_light(0, [0.0, 0.0, -10.0], 10.0, true),
            test_light(1, [10.0, 0.0, -10.0], 10.0, true),
            test_light(2, [20.0, 0.0, -10.0], 10.0, true),
        ];
        let bitmask = [true, false, true];
        let assignment = SpotShadowPool::rank_lights(&lights, Vec3::ZERO, 0.1, &bitmask, &[]);
        assert_ne!(assignment[0], NO_SHADOW_SLOT);
        assert_eq!(assignment[1], NO_SHADOW_SLOT);
        assert_ne!(assignment[2], NO_SHADOW_SLOT);
    }

    #[test]
    fn nine_lights_with_eight_visible_assigns_eight() {
        // The invisible light (index 0) is placed closest to the camera so it
        // would otherwise rank #1 by heuristic — proving the bitmask filter
        // takes precedence over the score.
        let mut lights = Vec::new();
        lights.push(test_light(0, [0.0, 0.0, -1.0], 10.0, true));
        for i in 1..9 {
            lights.push(test_light(
                i as u32,
                [i as f64 * 50.0, 0.0, -10.0],
                10.0,
                true,
            ));
        }
        let mut bitmask = vec![true; 9];
        bitmask[0] = false;
        let assignment = SpotShadowPool::rank_lights(&lights, Vec3::ZERO, 0.1, &bitmask, &[]);

        assert_eq!(assignment[0], NO_SHADOW_SLOT);
        let assigned_count = assignment[1..]
            .iter()
            .filter(|&&s| s != NO_SHADOW_SLOT)
            .count();
        assert_eq!(assigned_count, 8, "all 8 visible lights get slots");
    }

    #[test]
    fn empty_bitmask_treated_as_all_visible() {
        let lights = vec![
            test_light(0, [0.0, 0.0, -10.0], 10.0, true),
            test_light(1, [10.0, 0.0, -10.0], 10.0, true),
            test_light(2, [20.0, 0.0, -10.0], 10.0, true),
        ];
        let assignment = SpotShadowPool::rank_lights(&lights, Vec3::ZERO, 0.1, &[], &[]);
        assert_ne!(assignment[0], NO_SHADOW_SLOT);
        assert_ne!(assignment[1], NO_SHADOW_SLOT);
        assert_ne!(assignment[2], NO_SHADOW_SLOT);
    }

    #[test]
    fn camera_near_clip_clamps_denominator() {
        // Light very close to camera (distance < near_clip). Heuristic should clamp.
        let lights = vec![test_light(0, [0.001, 0.0, 0.0], 10.0, true)];
        let camera_near_clip = 0.1;
        let assignment =
            SpotShadowPool::rank_lights(&lights, Vec3::ZERO, camera_near_clip, &[], &[]);
        // Should still be assigned.
        assert_eq!(assignment[0], 0);
    }
}
