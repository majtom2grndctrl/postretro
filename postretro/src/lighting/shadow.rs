// Shadow map slot pool: allocation, caching, and per-frame assignment.
// See: context/plans/in-progress/lighting-foundation/5-shadow-maps.md

use std::collections::HashMap;

use glam::{Mat4, Vec3};

use crate::prl::{LightType, MapLight};

// --- Pool sizing constants ---

/// Maximum number of directional lights with active CSM slots.
pub const MAX_CSM_LIGHTS: usize = 2;
/// Cascades per directional light.
pub const CSM_CASCADE_COUNT: usize = 3;
/// Total CSM array layers.
pub const CSM_TOTAL_LAYERS: usize = MAX_CSM_LIGHTS * CSM_CASCADE_COUNT;
/// Resolution of each CSM cascade layer.
pub const CSM_RESOLUTION: u32 = 1024;

/// Maximum number of point lights with active cube shadow map slots.
pub const MAX_POINT_SHADOW_LIGHTS: usize = 16;
/// Resolution of each cube shadow map face.
pub const POINT_SHADOW_RESOLUTION: u32 = 512;

/// Maximum number of spot lights with active shadow map slots.
pub const MAX_SPOT_SHADOW_LIGHTS: usize = 16;
/// Resolution of each spot shadow map layer.
pub const SPOT_SHADOW_RESOLUTION: u32 = 1024;

/// Number of faces per cube map.
pub const CUBE_FACES: usize = 6;

// --- Shadow kind discriminant ---

/// Shader-side discriminant stored in `GpuLight.shadow_info.z`.
/// Matches the `switch` in the forward shader's shadow sampling.
#[allow(dead_code)] // Part of the shadow kind enum; used implicitly as the zero-default.
pub const SHADOW_KIND_NONE: u32 = 0;
pub const SHADOW_KIND_CSM: u32 = 1;
pub const SHADOW_KIND_CUBE: u32 = 2;
pub const SHADOW_KIND_SPOT_2D: u32 = 3;

// --- Cascade split calculation ---

/// Practical split scheme (log/linear blend) for CSM cascades.
/// `lambda` = 0.5 blends equally between logarithmic and uniform splits.
/// Returns cascade far distances in view-space depth.
pub fn compute_cascade_splits(near: f32, far: f32, lambda: f32) -> [f32; CSM_CASCADE_COUNT] {
    let mut splits = [0.0f32; CSM_CASCADE_COUNT];
    for (i, split) in splits.iter_mut().enumerate() {
        let p = (i + 1) as f32 / CSM_CASCADE_COUNT as f32;
        let log_split = near * (far / near).powf(p);
        let lin_split = near + (far - near) * p;
        *split = lambda * log_split + (1.0 - lambda) * lin_split;
    }
    splits
}

/// Build the orthographic projection matrix for one CSM cascade that tightly
/// fits the camera frustum slice between `split_near` and `split_far`.
///
/// `inv_view_proj` is the inverse of the camera's view-projection matrix.
/// `light_dir` is the normalized direction the light shines (from light toward scene).
pub fn cascade_ortho_matrix(
    inv_view_proj: Mat4,
    split_near: f32,
    split_far: f32,
    near: f32,
    far: f32,
    light_dir: Vec3,
) -> Mat4 {
    // Map split distances to NDC Z range [0, 1] (standard depth).
    let ndc_near = split_near_to_ndc(split_near, near, far);
    let ndc_far = split_near_to_ndc(split_far, near, far);

    // 8 corners of the frustum slice in NDC, then unproject to world space.
    let corners_ndc = [
        // Near face
        Vec3::new(-1.0, -1.0, ndc_near),
        Vec3::new(1.0, -1.0, ndc_near),
        Vec3::new(1.0, 1.0, ndc_near),
        Vec3::new(-1.0, 1.0, ndc_near),
        // Far face
        Vec3::new(-1.0, -1.0, ndc_far),
        Vec3::new(1.0, -1.0, ndc_far),
        Vec3::new(1.0, 1.0, ndc_far),
        Vec3::new(-1.0, 1.0, ndc_far),
    ];

    // Build the light's view matrix. Look from an arbitrary position along the
    // light direction. We'll use the centroid of the frustum slice as the
    // target and back the eye out.
    let up = if light_dir.y.abs() > 0.99 {
        Vec3::Z
    } else {
        Vec3::Y
    };
    let light_view = Mat4::look_to_rh(Vec3::ZERO, light_dir, up);

    // Unproject corners to world space, then transform into light view space.
    let mut light_mins = Vec3::splat(f32::MAX);
    let mut light_maxs = Vec3::splat(f32::MIN);
    for ndc in &corners_ndc {
        let world = unproject_ndc(inv_view_proj, *ndc);
        let lv = light_view.transform_point3(world);
        light_mins = light_mins.min(lv);
        light_maxs = light_maxs.max(lv);
    }

    // Orthographic projection that tightly wraps the light-space AABB.
    // Push the near plane back to capture shadow casters behind the frustum.
    let ortho = Mat4::orthographic_rh(
        light_mins.x,
        light_maxs.x,
        light_mins.y,
        light_maxs.y,
        // Near is negative Z in RH — push far back to catch casters.
        -light_maxs.z - 500.0,
        -light_mins.z + 10.0,
    );

    ortho * light_view
}

/// Convert a view-space depth to NDC Z for a perspective-rh projection.
fn split_near_to_ndc(depth: f32, near: f32, far: f32) -> f32 {
    // glam's perspective_rh maps Z to [0, 1] in wgpu's convention.
    // For perspective_rh: ndc_z = far * (depth - near) / (depth * (far - near))
    far * (depth - near) / (depth * (far - near))
}

/// Unproject an NDC point (x, y, z in [-1,1] x [-1,1] x [0,1]) to world space.
fn unproject_ndc(inv_view_proj: Mat4, ndc: Vec3) -> Vec3 {
    let clip = glam::Vec4::new(ndc.x, ndc.y, ndc.z, 1.0);
    let world = inv_view_proj * clip;
    world.truncate() / world.w
}

// --- Cube shadow map view matrices ---

/// Build the 6 view-projection matrices for a point light's cube shadow map.
/// Each face uses a 90-degree FOV perspective projection.
pub fn point_light_cube_matrices(light_pos: Vec3, light_range: f32) -> [Mat4; 6] {
    // Cube face directions and up vectors (OpenGL convention, matches wgpu's
    // texture_cube expectation: +X, -X, +Y, -Y, +Z, -Z).
    let directions: [(Vec3, Vec3); 6] = [
        (Vec3::X, Vec3::NEG_Y),   // +X
        (Vec3::NEG_X, Vec3::NEG_Y), // -X
        (Vec3::Y, Vec3::Z),       // +Y
        (Vec3::NEG_Y, Vec3::NEG_Z), // -Y
        (Vec3::Z, Vec3::NEG_Y),   // +Z
        (Vec3::NEG_Z, Vec3::NEG_Y), // -Z
    ];

    let proj = Mat4::perspective_rh(
        std::f32::consts::FRAC_PI_2, // 90 degrees
        1.0,
        0.1,
        light_range,
    );

    // WebGPU cube-sampling convention (inherited from D3D/Vulkan) expects
    // texture V to increase toward the +t direction of each face's (s, t)
    // parameterization. `perspective_rh` combined with our `look_to_rh` face
    // views emits the framebuffer Y flipped relative to that convention — so
    // without this correction, the depth written at screen-top lands where
    // the hardware sampler expects screen-bottom content, and vice versa.
    // That mismatch causes geometry on one side of each face (e.g. pillars
    // below the light) to be sampled when the shader asks for the opposite
    // direction (e.g. toward the ceiling above the light).
    //
    // Pre-multiplying by a Y-flip in NDC realigns the rendered content with
    // the cube-sampling UV convention. Note: this inverts triangle winding in
    // screen space, so the point-shadow pipeline culls front faces instead of
    // back faces (see shadow_pass.rs).
    let flip_y = Mat4::from_scale(Vec3::new(1.0, -1.0, 1.0));

    let mut matrices = [Mat4::IDENTITY; 6];
    for (i, (dir, up)) in directions.iter().enumerate() {
        let view = Mat4::look_to_rh(light_pos, *dir, *up);
        matrices[i] = flip_y * proj * view;
    }
    matrices
}

/// Build the view-projection matrix for a spot light's single shadow map.
pub fn spot_light_matrix(light_pos: Vec3, light_dir: Vec3, outer_angle: f32, light_range: f32) -> Mat4 {
    let fov = outer_angle * 2.0; // outer_angle is half-angle
    let fov = fov.min(std::f32::consts::PI - 0.01); // clamp to < 180 degrees
    let up = if light_dir.y.abs() > 0.99 {
        Vec3::Z
    } else {
        Vec3::Y
    };
    let view = Mat4::look_to_rh(light_pos, light_dir, up);
    let proj = Mat4::perspective_rh(fov, 1.0, 0.1, light_range.max(1.0));
    proj * view
}

// --- Slot pool and cache ---

/// Per-frame assignment of a shadow-casting light to a slot in the pool.
#[derive(Debug, Clone, Copy)]
pub struct ShadowSlot {
    /// Index of the light in the global light array.
    pub light_index: u32,
    /// Shadow kind (CSM, cube, spot-2D).
    pub shadow_kind: u32,
    /// Index into the type-specific pool (cascade base for CSM, cube slot for
    /// point, array layer for spot).
    pub pool_slot: u32,
    /// Whether this slot's shadow map content is still valid from a previous
    /// frame (the light held this slot last frame and nothing changed).
    pub cached: bool,
}

/// Manages the fixed shadow map slot pool and per-frame assignment.
pub struct ShadowSlotPool {
    /// light_index -> pool_slot mapping from the previous frame.
    /// CSM, point, and spot each have independent namespaces — the key includes
    /// the shadow kind to disambiguate.
    prev_csm: HashMap<u32, u32>,
    prev_point: HashMap<u32, u32>,
    prev_spot: HashMap<u32, u32>,
}

impl ShadowSlotPool {
    pub fn new() -> Self {
        Self {
            prev_csm: HashMap::new(),
            prev_point: HashMap::new(),
            prev_spot: HashMap::new(),
        }
    }

    /// Assign shadow slots for the current frame. Returns the assignments and
    /// a per-light shadow info array (indexed by global light index) for GPU upload.
    ///
    /// `visible_light_indices` comes from sub-plan 4's frustum test; if empty,
    /// all shadow-casting lights are candidates. `lights` is the full light list.
    /// `camera_pos` is used for distance-priority sorting.
    pub fn assign(
        &mut self,
        lights: &[MapLight],
        visible_light_indices: &[u32],
        camera_pos: Vec3,
    ) -> ShadowAssignment {
        // Collect shadow-casting lights that are visible this frame, separated by type.
        let candidates: Vec<u32> = if visible_light_indices.is_empty() {
            // No influence data — all shadow-casting lights are candidates.
            (0..lights.len() as u32)
                .filter(|&i| lights[i as usize].cast_shadows)
                .collect()
        } else {
            visible_light_indices
                .iter()
                .copied()
                .filter(|&i| (i as usize) < lights.len() && lights[i as usize].cast_shadows)
                .collect()
        };

        let mut directional_candidates: Vec<u32> = Vec::new();
        let mut point_candidates: Vec<(u32, f32)> = Vec::new(); // (index, distance²)
        let mut spot_candidates: Vec<(u32, f32)> = Vec::new();

        for &li in &candidates {
            let light = &lights[li as usize];
            match light.light_type {
                LightType::Directional => directional_candidates.push(li),
                LightType::Point => {
                    let pos = Vec3::new(
                        light.origin[0] as f32,
                        light.origin[1] as f32,
                        light.origin[2] as f32,
                    );
                    let dist_sq = pos.distance_squared(camera_pos);
                    point_candidates.push((li, dist_sq));
                }
                LightType::Spot => {
                    let pos = Vec3::new(
                        light.origin[0] as f32,
                        light.origin[1] as f32,
                        light.origin[2] as f32,
                    );
                    let dist_sq = pos.distance_squared(camera_pos);
                    spot_candidates.push((li, dist_sq));
                }
            }
        }

        // Sort by distance (nearest first).
        point_candidates.sort_by(|a, b| a.1.partial_cmp(&b.1).unwrap_or(std::cmp::Ordering::Equal));
        spot_candidates.sort_by(|a, b| a.1.partial_cmp(&b.1).unwrap_or(std::cmp::Ordering::Equal));

        // Assign slots.
        let mut slots = Vec::new();
        let mut new_csm: HashMap<u32, u32> = HashMap::new();
        let mut new_point: HashMap<u32, u32> = HashMap::new();
        let mut new_spot: HashMap<u32, u32> = HashMap::new();

        // Directional lights always get a CSM slot (up to MAX_CSM_LIGHTS).
        for (slot_idx, &li) in directional_candidates.iter().take(MAX_CSM_LIGHTS).enumerate() {
            let pool_slot = slot_idx as u32;
            // CSM always re-renders (camera-dependent).
            let cached = false;
            slots.push(ShadowSlot {
                light_index: li,
                shadow_kind: SHADOW_KIND_CSM,
                pool_slot,
                cached,
            });
            new_csm.insert(li, pool_slot);
        }

        // Point lights — prefer keeping the same slot from last frame.
        let mut used_point_slots = [false; MAX_POINT_SHADOW_LIGHTS];
        let mut point_assignments: Vec<(u32, u32, bool)> = Vec::new(); // (li, slot, cached)

        // First pass: lights that still hold a slot from last frame.
        for &(li, _) in point_candidates.iter().take(MAX_POINT_SHADOW_LIGHTS) {
            if let Some(&prev_slot) = self.prev_point.get(&li) {
                if (prev_slot as usize) < MAX_POINT_SHADOW_LIGHTS && !used_point_slots[prev_slot as usize] {
                    used_point_slots[prev_slot as usize] = true;
                    point_assignments.push((li, prev_slot, true));
                }
            }
        }
        // Second pass: assign free slots to new lights.
        let assigned_point: std::collections::HashSet<u32> =
            point_assignments.iter().map(|&(li, _, _)| li).collect();
        let mut free_point_slots: Vec<u32> = (0..MAX_POINT_SHADOW_LIGHTS as u32)
            .filter(|&s| !used_point_slots[s as usize])
            .collect();
        for &(li, _) in point_candidates.iter().take(MAX_POINT_SHADOW_LIGHTS) {
            if !assigned_point.contains(&li) {
                if let Some(slot) = free_point_slots.pop() {
                    point_assignments.push((li, slot, false));
                }
            }
        }
        for (li, slot, cached) in point_assignments {
            slots.push(ShadowSlot {
                light_index: li,
                shadow_kind: SHADOW_KIND_CUBE,
                pool_slot: slot,
                cached,
            });
            new_point.insert(li, slot);
        }

        // Spot lights — same caching strategy as point lights.
        let mut used_spot_slots = [false; MAX_SPOT_SHADOW_LIGHTS];
        let mut spot_assignments: Vec<(u32, u32, bool)> = Vec::new();

        for &(li, _) in spot_candidates.iter().take(MAX_SPOT_SHADOW_LIGHTS) {
            if let Some(&prev_slot) = self.prev_spot.get(&li) {
                if (prev_slot as usize) < MAX_SPOT_SHADOW_LIGHTS && !used_spot_slots[prev_slot as usize] {
                    used_spot_slots[prev_slot as usize] = true;
                    spot_assignments.push((li, prev_slot, true));
                }
            }
        }
        let assigned_spot: std::collections::HashSet<u32> =
            spot_assignments.iter().map(|&(li, _, _)| li).collect();
        let mut free_spot_slots: Vec<u32> = (0..MAX_SPOT_SHADOW_LIGHTS as u32)
            .filter(|&s| !used_spot_slots[s as usize])
            .collect();
        for &(li, _) in spot_candidates.iter().take(MAX_SPOT_SHADOW_LIGHTS) {
            if !assigned_spot.contains(&li) {
                if let Some(slot) = free_spot_slots.pop() {
                    spot_assignments.push((li, slot, false));
                }
            }
        }
        for (li, slot, cached) in spot_assignments {
            slots.push(ShadowSlot {
                light_index: li,
                shadow_kind: SHADOW_KIND_SPOT_2D,
                pool_slot: slot,
                cached,
            });
            new_spot.insert(li, slot);
        }

        // Update cache for next frame.
        self.prev_csm = new_csm;
        self.prev_point = new_point;
        self.prev_spot = new_spot;

        // Build per-light shadow info (indexed by global light index).
        let mut per_light_info = vec![[0u32; 4]; lights.len()];
        for slot in &slots {
            let li = slot.light_index as usize;
            if li < per_light_info.len() {
                per_light_info[li] = [1, slot.pool_slot, slot.shadow_kind, 0];
            }
        }

        ShadowAssignment {
            slots,
            per_light_info,
        }
    }
}

/// Result of per-frame shadow slot assignment.
pub struct ShadowAssignment {
    /// Active shadow slots this frame.
    pub slots: Vec<ShadowSlot>,
    /// Per-light shadow info for GPU upload. Index = global light index.
    /// Each entry: [cast_shadows (0/1), shadow_map_index, shadow_kind, reserved].
    pub per_light_info: Vec<[u32; 4]>,
}

/// Pack shadow info into the GpuLight byte layout (slot 4, bytes 64..80).
/// `info` is [cast_shadows, shadow_map_index, shadow_kind, reserved].
pub fn write_shadow_info(dst: &mut [u8], info: [u32; 4]) {
    for (i, &val) in info.iter().enumerate() {
        let offset = 64 + i * 4;
        dst[offset..offset + 4].copy_from_slice(&val.to_ne_bytes());
    }
}

/// Pack the CSM view-projection matrices into a contiguous byte buffer.
/// Layout: `MAX_CSM_LIGHTS * CSM_CASCADE_COUNT` mat4x4<f32> entries.
pub fn pack_csm_view_proj_buffer(matrices: &[Mat4]) -> Vec<u8> {
    let mut bytes = Vec::with_capacity(matrices.len() * 64);
    for m in matrices {
        for &val in &m.to_cols_array() {
            bytes.extend_from_slice(&val.to_ne_bytes());
        }
    }
    bytes
}

/// Pack spot view-projection matrices into a contiguous byte buffer.
pub fn pack_spot_view_proj_buffer(matrices: &[Mat4]) -> Vec<u8> {
    pack_csm_view_proj_buffer(matrices) // Same layout — array of mat4x4<f32>.
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cascade_splits_are_monotonically_increasing() {
        let splits = compute_cascade_splits(0.1, 4096.0, 0.5);
        assert!(splits[0] > 0.1);
        assert!(splits[1] > splits[0]);
        assert!(splits[2] > splits[1]);
        assert!(splits[2] <= 4096.0);
    }

    #[test]
    fn cascade_splits_with_lambda_zero_are_linear() {
        let splits = compute_cascade_splits(1.0, 100.0, 0.0);
        let expected_step = 99.0 / CSM_CASCADE_COUNT as f32;
        for (i, &s) in splits.iter().enumerate() {
            let expected = 1.0 + expected_step * (i + 1) as f32;
            assert!(
                (s - expected).abs() < 1e-4,
                "split[{i}] = {s}, expected {expected}"
            );
        }
    }

    #[test]
    fn cascade_splits_with_lambda_one_are_logarithmic() {
        let splits = compute_cascade_splits(1.0, 1000.0, 1.0);
        // Log split: near * (far/near)^(i/n)
        let ratio = 1000.0f32;
        for (i, &s) in splits.iter().enumerate() {
            let p = (i + 1) as f32 / CSM_CASCADE_COUNT as f32;
            let expected = 1.0 * ratio.powf(p);
            assert!(
                (s - expected).abs() < 1e-2,
                "split[{i}] = {s}, expected {expected}"
            );
        }
    }

    #[test]
    fn point_light_cube_matrices_are_finite() {
        let matrices = point_light_cube_matrices(Vec3::ZERO, 100.0);
        for (i, m) in matrices.iter().enumerate() {
            for &val in &m.to_cols_array() {
                assert!(val.is_finite(), "cube face {i} matrix has non-finite value");
            }
        }
    }

    #[test]
    fn spot_light_matrix_is_finite() {
        let m = spot_light_matrix(Vec3::ZERO, Vec3::NEG_Z, 0.5, 50.0);
        for &val in &m.to_cols_array() {
            assert!(val.is_finite(), "spot matrix has non-finite value");
        }
    }

    fn sample_lights() -> Vec<MapLight> {
        vec![
            MapLight {
                origin: [0.0, 100.0, 0.0],
                light_type: LightType::Directional,
                intensity: 1.0,
                color: [1.0, 1.0, 1.0],
                falloff_model: crate::prl::FalloffModel::Linear,
                falloff_range: 0.0,
                cone_angle_inner: 0.0,
                cone_angle_outer: 0.0,
                cone_direction: [0.0, -1.0, 0.0],
                cast_shadows: true,
            },
            MapLight {
                origin: [10.0, 5.0, -20.0],
                light_type: LightType::Point,
                intensity: 300.0,
                color: [1.0, 0.8, 0.5],
                falloff_model: crate::prl::FalloffModel::InverseSquared,
                falloff_range: 50.0,
                cone_angle_inner: 0.0,
                cone_angle_outer: 0.0,
                cone_direction: [0.0, 0.0, 0.0],
                cast_shadows: true,
            },
            MapLight {
                origin: [-10.0, 3.0, -15.0],
                light_type: LightType::Spot,
                intensity: 200.0,
                color: [1.0, 1.0, 1.0],
                falloff_model: crate::prl::FalloffModel::Linear,
                falloff_range: 30.0,
                cone_angle_inner: 0.3,
                cone_angle_outer: 0.5,
                cone_direction: [0.0, -1.0, 0.0],
                cast_shadows: true,
            },
            MapLight {
                origin: [50.0, 5.0, -50.0],
                light_type: LightType::Point,
                intensity: 100.0,
                color: [1.0, 1.0, 1.0],
                falloff_model: crate::prl::FalloffModel::Linear,
                falloff_range: 20.0,
                cone_angle_inner: 0.0,
                cone_angle_outer: 0.0,
                cone_direction: [0.0, 0.0, 0.0],
                cast_shadows: false, // does not cast shadows
            },
        ]
    }

    #[test]
    fn slot_pool_assigns_all_shadow_casters() {
        let lights = sample_lights();
        let visible: Vec<u32> = (0..lights.len() as u32).collect();
        let mut pool = ShadowSlotPool::new();
        let assignment = pool.assign(&lights, &visible, Vec3::ZERO);

        // 3 shadow casters: 1 directional, 1 point, 1 spot.
        assert_eq!(assignment.slots.len(), 3);
        assert_eq!(
            assignment.per_light_info[0],
            [1, 0, SHADOW_KIND_CSM, 0],
            "directional light should get CSM slot 0"
        );
        assert_eq!(assignment.per_light_info[1][0], 1, "point should have shadow");
        assert_eq!(assignment.per_light_info[1][2], SHADOW_KIND_CUBE);
        assert_eq!(assignment.per_light_info[2][0], 1, "spot should have shadow");
        assert_eq!(assignment.per_light_info[2][2], SHADOW_KIND_SPOT_2D);
        assert_eq!(
            assignment.per_light_info[3],
            [0, 0, 0, 0],
            "non-shadow-casting light should have no shadow info"
        );
    }

    #[test]
    fn slot_pool_caches_point_light_slots_across_frames() {
        let lights = sample_lights();
        let visible: Vec<u32> = (0..lights.len() as u32).collect();
        let mut pool = ShadowSlotPool::new();

        // First frame — all new.
        let a1 = pool.assign(&lights, &visible, Vec3::ZERO);
        let point_slot_1 = a1
            .slots
            .iter()
            .find(|s| s.shadow_kind == SHADOW_KIND_CUBE)
            .expect("should have a point shadow slot");
        assert!(!point_slot_1.cached, "first frame should not be cached");
        let slot_idx = point_slot_1.pool_slot;

        // Second frame — same visible set, point light should be cached.
        let a2 = pool.assign(&lights, &visible, Vec3::ZERO);
        let point_slot_2 = a2
            .slots
            .iter()
            .find(|s| s.shadow_kind == SHADOW_KIND_CUBE)
            .expect("should still have point shadow slot");
        assert!(point_slot_2.cached, "second frame should be cached");
        assert_eq!(point_slot_2.pool_slot, slot_idx, "should keep same slot");
    }

    #[test]
    fn slot_pool_unshadowed_lights_get_zero_info() {
        let lights = sample_lights();
        let visible: Vec<u32> = (0..lights.len() as u32).collect();
        let mut pool = ShadowSlotPool::new();
        let assignment = pool.assign(&lights, &visible, Vec3::ZERO);

        // Light 3 has cast_shadows=false.
        assert_eq!(assignment.per_light_info[3], [0, 0, 0, 0]);
    }

    #[test]
    fn slot_pool_degrades_when_pool_full() {
        // Create more point lights than MAX_POINT_SHADOW_LIGHTS.
        let mut lights: Vec<MapLight> = (0..MAX_POINT_SHADOW_LIGHTS + 5)
            .map(|i| MapLight {
                origin: [i as f64 * 10.0, 5.0, 0.0],
                light_type: LightType::Point,
                intensity: 100.0,
                color: [1.0, 1.0, 1.0],
                falloff_model: crate::prl::FalloffModel::InverseSquared,
                falloff_range: 50.0,
                cone_angle_inner: 0.0,
                cone_angle_outer: 0.0,
                cone_direction: [0.0, 0.0, 0.0],
                cast_shadows: true,
            })
            .collect();

        let visible: Vec<u32> = (0..lights.len() as u32).collect();
        let mut pool = ShadowSlotPool::new();
        let assignment = pool.assign(&lights, &visible, Vec3::ZERO);

        // Only MAX_POINT_SHADOW_LIGHTS should get slots.
        let point_slots: Vec<_> = assignment
            .slots
            .iter()
            .filter(|s| s.shadow_kind == SHADOW_KIND_CUBE)
            .collect();
        assert_eq!(point_slots.len(), MAX_POINT_SHADOW_LIGHTS);

        // Remaining lights should have shadow_kind 0.
        let unshadowed: Vec<_> = assignment
            .per_light_info
            .iter()
            .filter(|info| info[0] == 0)
            .collect();
        assert_eq!(unshadowed.len(), 5, "5 lights should degrade to unshadowed");
    }

    #[test]
    fn write_shadow_info_encodes_correctly() {
        let mut bytes = [0u8; 80];
        write_shadow_info(&mut bytes, [1, 5, SHADOW_KIND_CUBE, 0]);
        assert_eq!(
            u32::from_ne_bytes(bytes[64..68].try_into().unwrap()),
            1,
            "cast_shadows"
        );
        assert_eq!(
            u32::from_ne_bytes(bytes[68..72].try_into().unwrap()),
            5,
            "shadow_map_index"
        );
        assert_eq!(
            u32::from_ne_bytes(bytes[72..76].try_into().unwrap()),
            SHADOW_KIND_CUBE,
            "shadow_kind"
        );
        assert_eq!(
            u32::from_ne_bytes(bytes[76..80].try_into().unwrap()),
            0,
            "reserved"
        );
    }
}
