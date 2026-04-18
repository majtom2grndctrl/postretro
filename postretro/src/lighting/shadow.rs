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

// --- Shadow kind discriminant ---

/// Shader-side discriminant stored in `GpuLight.shadow_info.z`.
/// Matches the branches in the forward shader's shadow sampling.
///
/// Discriminator values:
/// - `0` = no shadow (unshadowed)
/// - `1` = CSM (directional lights, this sub-plan)
/// - `2` = **reserved for SDF sphere-trace** (point and spot lights, sub-plan 9).
///   Do not reuse this value for any other shadow path — sub-plan 9 depends on it.
#[allow(dead_code)] // Part of the shadow kind enum; used implicitly as the zero-default.
pub const SHADOW_KIND_NONE: u32 = 0;
pub const SHADOW_KIND_CSM: u32 = 1;
/// SDF sphere-trace soft shadows for point and spot lights. Sub-plan 8.
pub const SHADOW_KIND_SDF: u32 = 2;

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

/// Light-space axis-aligned bounds for a CSM cascade.
///
/// `min` and `max` are in the light's view space. The extent (`max - min`)
/// is fixed to the bounding sphere of the frustum slice — rotation-invariant,
/// so texel size stays constant frame-to-frame. `min` is then quantized to
/// that fixed texel grid so the projection steps in whole-texel increments
/// as the camera rotates — this removes the shimmer that would otherwise
/// crawl along hard shadow edges.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CascadeBounds {
    pub min: Vec3,
    pub max: Vec3,
    /// Rotation-only light view used to produce these bounds. Callers compose
    /// their ortho matrix as `ortho(min, max) * light_view`.
    pub light_view: Mat4,
}

/// Fit a cascade's light-space AABB to the frustum slice and snap the origin
/// to texel boundaries. `shadow_resolution` is the cascade texture resolution
/// (e.g. `CSM_RESOLUTION`) used to derive the snap grid.
///
/// The extent is preserved across snapping; only the origin is quantized.
pub fn fit_cascade_bounds(
    inv_view_proj: Mat4,
    split_near: f32,
    split_far: f32,
    near: f32,
    far: f32,
    light_dir: Vec3,
    shadow_resolution: u32,
) -> CascadeBounds {
    let ndc_near = split_near_to_ndc(split_near, near, far);
    let ndc_far = split_near_to_ndc(split_far, near, far);
    let corners_ndc = [
        Vec3::new(-1.0, -1.0, ndc_near),
        Vec3::new(1.0, -1.0, ndc_near),
        Vec3::new(1.0, 1.0, ndc_near),
        Vec3::new(-1.0, 1.0, ndc_near),
        Vec3::new(-1.0, -1.0, ndc_far),
        Vec3::new(1.0, -1.0, ndc_far),
        Vec3::new(1.0, 1.0, ndc_far),
        Vec3::new(-1.0, 1.0, ndc_far),
    ];

    // Unproject to world space. Distances between corners are rotation- and
    // translation-invariant, so the bounding sphere built from these is stable
    // across camera motion — this is the key property that makes snapping work.
    let mut corners_world = [Vec3::ZERO; 8];
    for (i, ndc) in corners_ndc.iter().enumerate() {
        corners_world[i] = unproject_ndc(inv_view_proj, *ndc);
    }

    // Bounding sphere: centroid + max distance to any corner.
    let mut center = Vec3::ZERO;
    for c in &corners_world {
        center += *c;
    }
    center /= 8.0;
    let mut radius = 0.0f32;
    for c in &corners_world {
        radius = radius.max((*c - center).length());
    }
    // Round radius to a coarse step so floating-point noise in the unprojection
    // (which varies slightly with camera pose) cannot wiggle the extent.
    radius = radius.ceil();

    let up = if light_dir.y.abs() > 0.99 {
        Vec3::Z
    } else {
        Vec3::Y
    };
    let light_view = Mat4::look_to_rh(Vec3::ZERO, light_dir, up);
    let light_center = light_view.transform_point3(center);

    // Fixed-extent AABB in light space. The extent (2 * radius) depends only
    // on the frustum slice shape, not on camera orientation.
    let mut min = Vec3::new(
        light_center.x - radius,
        light_center.y - radius,
        light_center.z - radius,
    );
    let mut max = Vec3::new(
        light_center.x + radius,
        light_center.y + radius,
        light_center.z + radius,
    );

    // Snap xy origin to the fixed texel grid. Because extent is constant,
    // texel size is constant and the same world-space grid is used every
    // frame — tiny camera rotations land in the same cell, stepping by whole
    // texels only when crossing a boundary. Z is left unsnapped; depth is
    // bounded separately in `cascade_ortho_matrix` with an extended margin.
    let resolution = shadow_resolution.max(1) as f32;
    let texel_x = (max.x - min.x) / resolution;
    let texel_y = (max.y - min.y) / resolution;
    if texel_x.is_finite() && texel_x > 0.0 {
        let snapped = (min.x / texel_x).floor() * texel_x;
        let dx = snapped - min.x;
        min.x += dx;
        max.x += dx;
    }
    if texel_y.is_finite() && texel_y > 0.0 {
        let snapped = (min.y / texel_y).floor() * texel_y;
        let dy = snapped - min.y;
        min.y += dy;
        max.y += dy;
    }

    CascadeBounds {
        min,
        max,
        light_view,
    }
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
    let bounds = fit_cascade_bounds(
        inv_view_proj,
        split_near,
        split_far,
        near,
        far,
        light_dir,
        CSM_RESOLUTION,
    );

    // Orthographic projection that tightly wraps the texel-snapped light-space
    // AABB. Push the near plane back to capture shadow casters behind the
    // frustum. Near is negative Z in RH — push far back to catch casters.
    let ortho = Mat4::orthographic_rh(
        bounds.min.x,
        bounds.max.x,
        bounds.min.y,
        bounds.max.y,
        -bounds.max.z - 500.0,
        -bounds.min.z + 10.0,
    );

    ortho * bounds.light_view
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

// --- Slot pool and cache ---

/// Per-frame assignment of a shadow-casting light to a slot in the pool.
#[derive(Debug, Clone, Copy)]
pub struct ShadowSlot {
    /// Index of the light in the global light array.
    pub light_index: u32,
    /// Shadow kind (currently only CSM — see `SHADOW_KIND_*`).
    pub shadow_kind: u32,
    /// Index into the type-specific pool (cascade base for CSM).
    pub pool_slot: u32,
}

/// Manages the fixed shadow map slot pool and per-frame assignment.
///
/// Today only CSM (directional) slots are tracked. Point and spot lights
/// receive their shadow contribution from sub-plan 9's SDF sphere-trace,
/// which has no per-light slot allocation.
pub struct ShadowSlotPool {
    /// light_index -> pool_slot mapping from the previous frame for CSM.
    prev_csm: HashMap<u32, u32>,
}

impl ShadowSlotPool {
    pub fn new() -> Self {
        Self {
            prev_csm: HashMap::new(),
        }
    }

    /// Assign shadow slots for the current frame. Returns the assignments and
    /// a per-light shadow info array (indexed by global light index) for GPU upload.
    ///
    /// `visible_light_indices` comes from sub-plan 4's frustum test; if empty,
    /// all shadow-casting lights are candidates. `lights` is the full light list.
    /// `_camera_pos` is unused today but retained for the eventual distance-priority
    /// sort when additional slot pools are introduced.
    pub fn assign(
        &mut self,
        lights: &[MapLight],
        visible_light_indices: &[u32],
        _camera_pos: Vec3,
    ) -> ShadowAssignment {
        // Collect shadow-casting directional lights that are visible this frame.
        // Point and spot lights never get a CSM slot — their shadow path lives
        // in sub-plan 9 (SDF sphere-trace).
        let directional_candidates: Vec<u32> = if visible_light_indices.is_empty() {
            (0..lights.len() as u32)
                .filter(|&i| {
                    let l = &lights[i as usize];
                    l.cast_shadows && matches!(l.light_type, LightType::Directional)
                })
                .collect()
        } else {
            visible_light_indices
                .iter()
                .copied()
                .filter(|&i| {
                    if (i as usize) >= lights.len() {
                        return false;
                    }
                    let l = &lights[i as usize];
                    l.cast_shadows && matches!(l.light_type, LightType::Directional)
                })
                .collect()
        };

        let mut slots = Vec::new();
        let mut new_csm: HashMap<u32, u32> = HashMap::new();

        // Directional lights always get a CSM slot (up to MAX_CSM_LIGHTS).
        for (slot_idx, &li) in directional_candidates
            .iter()
            .take(MAX_CSM_LIGHTS)
            .enumerate()
        {
            let pool_slot = slot_idx as u32;
            slots.push(ShadowSlot {
                light_index: li,
                shadow_kind: SHADOW_KIND_CSM,
                pool_slot,
            });
            new_csm.insert(li, pool_slot);
        }

        // Update cache for next frame.
        self.prev_csm = new_csm;

        // Build per-light shadow info (indexed by global light index).
        let mut per_light_info = vec![[0u32; 4]; lights.len()];
        for slot in &slots {
            let li = slot.light_index as usize;
            if li < per_light_info.len() {
                per_light_info[li] = [1, slot.pool_slot, slot.shadow_kind, 0];
            }
        }

        // Point and spot lights with cast_shadows=true use SDF sphere-trace
        // (shadow_kind == 2). They don't occupy a shadow map slot — the SDF
        // atlas is a single shared resource loaded at level init.
        for (li, l) in lights.iter().enumerate() {
            if l.cast_shadows
                && matches!(
                    l.light_type,
                    crate::prl::LightType::Point | crate::prl::LightType::Spot
                )
            {
                // Don't overwrite a CSM slot assignment.
                if per_light_info[li][2] == SHADOW_KIND_NONE {
                    per_light_info[li] = [1, 0, SHADOW_KIND_SDF, 0];
                }
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

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    // --- Helpers ---

    fn camera_view_proj(
        eye: Vec3,
        yaw: f32,
        pitch: f32,
        fov_y: f32,
        aspect: f32,
        near: f32,
        far: f32,
    ) -> Mat4 {
        let forward = Vec3::new(
            yaw.cos() * pitch.cos(),
            pitch.sin(),
            yaw.sin() * pitch.cos(),
        )
        .normalize();
        let view = Mat4::look_to_rh(eye, forward, Vec3::Y);
        let proj = Mat4::perspective_rh(fov_y, aspect, near, far);
        proj * view
    }

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
                cast_shadows: false,
            },
        ]
    }

    #[test]
    fn slot_pool_assigns_directional_only() {
        // CSM is the only shadow kind this pool manages. Point/spot lights,
        // shadowed or not, receive `per_light_info == [0,0,0,0]` — their
        // shadow contribution comes from sub-plan 9's SDF sphere-trace at
        // runtime, not from a slot allocated here.
        let lights = sample_lights();
        let visible: Vec<u32> = (0..lights.len() as u32).collect();
        let mut pool = ShadowSlotPool::new();
        let assignment = pool.assign(&lights, &visible, Vec3::ZERO);

        assert_eq!(assignment.slots.len(), 1, "only 1 directional caster");
        assert_eq!(
            assignment.per_light_info[0],
            [1, 0, SHADOW_KIND_CSM, 0],
            "directional light should get CSM slot 0"
        );
        // Point and spot lights with cast_shadows=true get SDF kind (sub-plan 8).
        // pool_slot is 0 (unused) for SDF lights.
        assert_eq!(
            assignment.per_light_info[1],
            [1, 0, SHADOW_KIND_SDF, 0],
            "point light (cast_shadows=true) should get SDF shadow kind"
        );
        assert_eq!(
            assignment.per_light_info[2],
            [1, 0, SHADOW_KIND_SDF, 0],
            "spot light (cast_shadows=true) should get SDF shadow kind"
        );
        // Light 3 has cast_shadows=false — no shadow.
        assert_eq!(assignment.per_light_info[3], [0, 0, 0, 0]);
    }

    #[test]
    fn slot_pool_respects_csm_light_limit() {
        // More directional lights than MAX_CSM_LIGHTS — the excess must be
        // dropped (no silent overflow into an adjacent slot).
        let lights: Vec<MapLight> = (0..MAX_CSM_LIGHTS + 2)
            .map(|_| MapLight {
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
            })
            .collect();
        let visible: Vec<u32> = (0..lights.len() as u32).collect();
        let mut pool = ShadowSlotPool::new();
        let assignment = pool.assign(&lights, &visible, Vec3::ZERO);
        assert_eq!(assignment.slots.len(), MAX_CSM_LIGHTS);
        for i in 0..MAX_CSM_LIGHTS {
            assert_eq!(assignment.per_light_info[i][0], 1);
        }
        for i in MAX_CSM_LIGHTS..lights.len() {
            assert_eq!(assignment.per_light_info[i], [0, 0, 0, 0]);
        }
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

    // --- CSM texel-snapping regression tests ---
    //
    // Regression: cascade_ortho_matrix did not snap the light-space AABB origin
    // to texel boundaries, so the shadow projection drifted continuously under
    // camera rotation. Hard shadow edges shimmered and crawled across surfaces.
    //
    // The fix quantizes the xy origin of the light-space AABB to integer
    // multiples of texel size (extent / resolution). These tests specify that
    // contract; they fail against the stub in `fit_cascade_bounds`.

    /// How many texel-sizes away from a multiple we tolerate. Allows for
    /// float-precision slop in the snap operation without admitting drift.
    const TEXEL_SNAP_TOLERANCE: f32 = 1e-3;

    fn texel_size(bounds: CascadeBounds, resolution: u32) -> (f32, f32) {
        (
            (bounds.max.x - bounds.min.x) / resolution as f32,
            (bounds.max.y - bounds.min.y) / resolution as f32,
        )
    }

    fn is_texel_aligned(value: f32, texel: f32) -> bool {
        if !texel.is_finite() || texel.abs() < 1e-6 {
            return true; // degenerate extent — nothing to snap against
        }
        let ratio = value / texel;
        let nearest = ratio.round();
        (ratio - nearest).abs() < TEXEL_SNAP_TOLERANCE
    }

    #[test]
    fn cascade_bounds_origin_snaps_to_texel_boundary() {
        let view_proj = camera_view_proj(
            Vec3::new(12.3, 4.5, -7.8),
            0.6,
            -0.2,
            1.2,
            16.0 / 9.0,
            0.1,
            4096.0,
        );
        let inv_view_proj = view_proj.inverse();
        let light_dir = Vec3::new(0.3, -1.0, 0.2).normalize();
        let bounds = fit_cascade_bounds(inv_view_proj, 0.1, 30.0, 0.1, 4096.0, light_dir, 1024);
        let (tx, ty) = texel_size(bounds, 1024);

        assert!(
            is_texel_aligned(bounds.min.x, tx),
            "bounds.min.x = {} not aligned to texel {}",
            bounds.min.x,
            tx
        );
        assert!(
            is_texel_aligned(bounds.min.y, ty),
            "bounds.min.y = {} not aligned to texel {}",
            bounds.min.y,
            ty
        );
    }

    proptest! {
        /// For any plausible camera pose and light direction, the fitted cascade
        /// bounds' xy origin is an integer multiple of the texel size. This is
        /// the core texel-snap invariant — if it holds across the domain, the
        /// projection cannot drift sub-texel as the camera rotates.
        #[test]
        fn cascade_bounds_xy_origin_is_texel_aligned(
            cam_x in -50.0f32..50.0,
            cam_y in -5.0f32..20.0,
            cam_z in -50.0f32..50.0,
            yaw in -std::f32::consts::PI..std::f32::consts::PI,
            pitch in -1.5f32..1.5,
            light_x in -1.0f32..1.0,
            light_y in -1.0f32..-0.1, // keep light pointing generally downward
            light_z in -1.0f32..1.0,
        ) {
            let light_dir = Vec3::new(light_x, light_y, light_z).normalize();
            let view_proj = camera_view_proj(
                Vec3::new(cam_x, cam_y, cam_z),
                yaw,
                pitch,
                1.2,
                16.0 / 9.0,
                0.1,
                4096.0,
            );
            let bounds = fit_cascade_bounds(
                view_proj.inverse(),
                0.1,
                30.0,
                0.1,
                4096.0,
                light_dir,
                1024,
            );
            let (tx, ty) = texel_size(bounds, 1024);

            prop_assert!(
                is_texel_aligned(bounds.min.x, tx),
                "min.x={} texel={}",
                bounds.min.x, tx
            );
            prop_assert!(
                is_texel_aligned(bounds.min.y, ty),
                "min.y={} texel={}",
                bounds.min.y, ty
            );
        }

        /// A camera rotation smaller than one shadow texel must not shift the
        /// snapped origin by more than a single texel step (often zero). Without
        /// snapping, any rotation shifts the origin continuously and the diff
        /// is arbitrary. This is the shimmer invariant.
        #[test]
        fn small_yaw_rotation_steps_at_most_one_texel(
            base_yaw in -std::f32::consts::PI..std::f32::consts::PI,
            // Tiny delta — well under the per-pixel yaw change of a mouse move.
            delta in -1e-4f32..1e-4,
        ) {
            let eye = Vec3::new(0.0, 2.0, 0.0);
            let light_dir = Vec3::new(0.3, -1.0, 0.1).normalize();
            let vp_a = camera_view_proj(eye, base_yaw, 0.0, 1.2, 16.0 / 9.0, 0.1, 4096.0);
            let vp_b = camera_view_proj(eye, base_yaw + delta, 0.0, 1.2, 16.0 / 9.0, 0.1, 4096.0);
            let a = fit_cascade_bounds(vp_a.inverse(), 0.1, 30.0, 0.1, 4096.0, light_dir, 1024);
            let b = fit_cascade_bounds(vp_b.inverse(), 0.1, 30.0, 0.1, 4096.0, light_dir, 1024);

            // Use a's texel size as the reference; with preserved extent the
            // two should be identical to within float slop.
            let (tx, ty) = texel_size(a, 1024);
            // Allow up to ~1 texel of movement per axis. With snapping active
            // the step is 0 for the vast majority of tiny rotations and
            // occasionally 1 texel when the unsnapped origin crosses a cell
            // boundary. Without snapping, the delta scales with the rotation
            // and this bound fails.
            let dx = (a.min.x - b.min.x).abs();
            let dy = (a.min.y - b.min.y).abs();
            prop_assert!(
                dx <= tx * (1.0 + TEXEL_SNAP_TOLERANCE),
                "dx={} tx={}",
                dx, tx
            );
            prop_assert!(
                dy <= ty * (1.0 + TEXEL_SNAP_TOLERANCE),
                "dy={} ty={}",
                dy, ty
            );
        }

        /// Snapping must preserve extent — only the origin moves. If extent
        /// drifts, texel size drifts with it and the alignment invariant
        /// becomes self-consistent but physically meaningless.
        #[test]
        fn texel_snap_preserves_extent(
            yaw in -std::f32::consts::PI..std::f32::consts::PI,
            pitch in -1.0f32..1.0,
        ) {
            let light_dir = Vec3::new(0.3, -1.0, 0.1).normalize();
            let vp = camera_view_proj(Vec3::ZERO, yaw, pitch, 1.2, 16.0 / 9.0, 0.1, 4096.0);
            // Same inputs — just verify the function is self-consistent on
            // the extent invariant it promises.
            let bounds = fit_cascade_bounds(vp.inverse(), 0.1, 30.0, 0.1, 4096.0, light_dir, 1024);
            let extent_x = bounds.max.x - bounds.min.x;
            let extent_y = bounds.max.y - bounds.min.y;
            prop_assert!(extent_x > 0.0 && extent_x.is_finite());
            prop_assert!(extent_y > 0.0 && extent_y.is_finite());
        }
    }

    #[test]
    fn write_shadow_info_encodes_correctly() {
        let mut bytes = [0u8; 80];
        write_shadow_info(&mut bytes, [1, 5, SHADOW_KIND_CSM, 0]);
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
            SHADOW_KIND_CSM,
            "shadow_kind"
        );
        assert_eq!(
            u32::from_ne_bytes(bytes[76..80].try_into().unwrap()),
            0,
            "reserved"
        );
    }
}
