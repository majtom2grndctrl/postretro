// Dynamic spot light shadow-map pool and slot allocation.
//
// See: context/plans/in-progress/lighting-spot-shadows/index.md § Task A
//      context/lib/rendering_pipeline.md §4

use crate::prl::{LightType, MapLight};
use glam::Vec3;

/// Number of shadow-map slots in the pool (retunable constant).
pub const SHADOW_POOL_SIZE: usize = 8;

/// Depth format for shadow maps.
pub const SHADOW_DEPTH_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Depth32Float;

/// Resolution of each shadow map in the pool (1024×1024).
pub const SHADOW_MAP_RESOLUTION: u32 = 1024;

/// Sentinel value written to the slot index field when no slot is allocated.
pub const NO_SHADOW_SLOT: u32 = 0xFFFFFFFF;

/// Pool of shadow-map texture slots, one per dynamic spot light that
/// passes visibility culling. Ranked by projected influence area each frame.
#[allow(dead_code)]
pub struct SpotShadowPool {
    /// Array texture with SHADOW_POOL_SIZE layers, each SHADOW_MAP_RESOLUTION×SHADOW_MAP_RESOLUTION
    pub array_texture: wgpu::Texture,
    /// Texture views for each slot (2D views for render attachments).
    pub views: Vec<wgpu::TextureView>,
    /// Per-frame slot assignment: slot_assignment[light_index] = slot (0..8) or NO_SHADOW_SLOT.
    pub slot_assignment: Vec<u32>,
}

impl SpotShadowPool {
    /// Allocate the shadow-map pool at renderer init.
    ///
    /// Creates a single array texture with `SHADOW_POOL_SIZE` layers,
    /// each `SHADOW_MAP_RESOLUTION × SHADOW_MAP_RESOLUTION` Depth32Float.
    pub fn new(device: &wgpu::Device) -> Self {
        // Create a single array texture.
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

        // Create individual 2D views for each layer (for render attachments).
        let views = (0..SHADOW_POOL_SIZE)
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

        Self {
            array_texture,
            views,
            slot_assignment: Vec::new(),
        }
    }

    /// Get a view of the entire array texture as D2Array (for sampling).
    #[allow(dead_code)]
    pub fn array_view(&self) -> wgpu::TextureView {
        self.array_texture
            .create_view(&wgpu::TextureViewDescriptor {
                label: Some("Spot Shadow Array View"),
                dimension: Some(wgpu::TextureViewDimension::D2Array),
                base_array_layer: 0,
                array_layer_count: Some(SHADOW_POOL_SIZE as u32),
                ..Default::default()
            })
    }

    /// Compute the slot-assignment ranking for visible dynamic spot lights.
    ///
    /// Takes the full light list and camera position. Identifies dynamic spot lights
    /// that pass frustum culling, ranks by influence-area heuristic, and assigns the
    /// top 8 to slots.
    ///
    /// Returns a Vec indexed by light index: entry is the slot index (0..8) or NO_SHADOW_SLOT.
    #[allow(dead_code)]
    pub fn rank_lights(
        lights: &[MapLight],
        camera_position: Vec3,
        camera_near_clip: f32,
        _visible_cell_bitmask: &[bool],
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
    fn nine_lights_eight_assigned_one_unshadowed() {
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
        assert_eq!(assigned_count, SHADOW_POOL_SIZE);

        let unshadowed_count = assignment.iter().filter(|&&s| s == NO_SHADOW_SLOT).count();
        assert_eq!(unshadowed_count, 1);
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
