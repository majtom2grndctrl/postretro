// Dynamic point-light cube-array shadow pool: per-light 6-face omnidirectional
// depth, ranked into a fixed-capacity cube-array. Entity occluders ONLY in v1
// (no world geometry rendered into cube faces).
//
// See: context/lib/rendering_pipeline.md §7.1 (shadow passes), §4 (lighting)

use crate::lighting::spot_shadow::{SHADOW_DEPTH_FORMAT, assign_ranked_slots};
use crate::prl::{LightType, MapLight};
use glam::{Mat4, Vec3};

/// Near-clip distance for a cube face's perspective projection. Matches the spot
/// path's `SHADOW_NEAR_CLIP` — close enough that depth bias controls acne, far
/// enough to keep precision.
pub const CUBE_NEAR_CLIP: f32 = 0.1;

/// Number of cube slots in the pool, sized to realistic concurrent demand after
/// PVS culling + influence ranking — NOT worst case. Each occupied slot draws
/// entity occluders into 6 faces, so the cost (and the VRAM) scales with this.
///
/// VRAM justification: a `Depth32Float` cube-array is
/// `CUBE_FACE_RESOLUTION² × 4 B × 6 faces × CUBE_COUNT`.
/// At 512² / 6 slots that is `512*512*4*6*6` = 36 MiB — within the budget left
/// after the 96-slot spot pool (96 × 1024² × 4 B = 384 MiB) and the lightmap
/// atlases. Six concurrent entity-shadow-casting point lights is generous for a
/// retro-FPS combat arena once PVS + influence ranking have culled the set, and
/// the lower angular detail of omni faces tolerates the 512² resolution.
pub const CUBE_COUNT: usize = 6;

/// Per-face square resolution. 512 leans on the lower angular detail of omni
/// cube faces (vs. the spot pool's 1024²); see `CUBE_COUNT` for the VRAM
/// trade-off this resolution feeds.
pub const CUBE_FACE_RESOLUTION: u32 = 512;

/// Number of faces per cube. Fixed at 6 (the cube's sides); a slot owns array
/// layers `slot*6 .. slot*6 + 6`.
pub const CUBE_FACES: usize = 6;

/// The 6 cube-face directions in the canonical wgpu/D3D cube-map face order:
/// +X, -X, +Y, -Y, +Z, -Z. A sampling layer index `slot*6 + face` follows this
/// order so the forward sample path (Task 5) can map a world direction to a face
/// with the standard major-axis selection.
const CUBE_FACE_DIRS: [Vec3; CUBE_FACES] = [
    Vec3::new(1.0, 0.0, 0.0),  // +X
    Vec3::new(-1.0, 0.0, 0.0), // -X
    Vec3::new(0.0, 1.0, 0.0),  // +Y
    Vec3::new(0.0, -1.0, 0.0), // -Y
    Vec3::new(0.0, 0.0, 1.0),  // +Z
    Vec3::new(0.0, 0.0, -1.0), // -Z
];

/// Per-face "up" vectors, paired with `CUBE_FACE_DIRS`. The ±Y faces look along
/// the Y axis, so their up is along Z (any vector not colinear with the look
/// direction); the other four use -Y. These match the standard cube-map face
/// orientation convention so all 6 faces tile the sphere with no gap or overlap.
const CUBE_FACE_UPS: [Vec3; CUBE_FACES] = [
    Vec3::new(0.0, -1.0, 0.0), // +X
    Vec3::new(0.0, -1.0, 0.0), // -X
    Vec3::new(0.0, 0.0, 1.0),  // +Y
    Vec3::new(0.0, 0.0, -1.0), // -Y
    Vec3::new(0.0, -1.0, 0.0), // +Z
    Vec3::new(0.0, -1.0, 0.0), // -Z
];

/// Build the 6 light-space view-projection matrices for a point light's cube
/// faces, in `CUBE_FACE_DIRS` order. Each is a 90° perspective (aspect 1.0,
/// near `CUBE_NEAR_CLIP`, far from the light's falloff range) times that face's
/// look-at view. Together the 6 frusta tile the full sphere around the light.
///
/// Pure math — no GPU. Unit-tested for sphere coverage and direction→face
/// mapping. The far plane clamps `falloff_range` to a small minimum so a
/// zero-range or degenerate light still yields a finite frustum.
pub fn cube_face_matrices(light: &MapLight) -> [Mat4; CUBE_FACES] {
    let eye = Vec3::new(
        light.origin[0] as f32,
        light.origin[1] as f32,
        light.origin[2] as f32,
    );
    let far = light.falloff_range.max(0.5);
    // 90° vertical FOV, aspect 1.0 — adjacent faces meet exactly at their shared
    // edge, so the 6 frusta partition all directions.
    let proj = Mat4::perspective_rh(std::f32::consts::FRAC_PI_2, 1.0, CUBE_NEAR_CLIP, far);

    let mut matrices = [Mat4::IDENTITY; CUBE_FACES];
    for face in 0..CUBE_FACES {
        let view = Mat4::look_at_rh(eye, eye + CUBE_FACE_DIRS[face], CUBE_FACE_UPS[face]);
        matrices[face] = proj * view;
    }
    matrices
}

/// Rank dynamic point lights into the cube pool, mirroring the spot ranker's
/// scoring/drop policy via the SHARED [`assign_ranked_slots`] core so the two
/// cannot drift. Pool-slot eligibility is `is_dynamic && Point`; lights beyond
/// `CUBE_COUNT` are dropped lowest-ranked-first (graceful, no panic).
///
/// `eligible_lights` is the caller's per-light visibility/brightness gate
/// (indexed by light); an empty (or short) slice treats every light as eligible,
/// matching the spot ranker's first-frame contract.
///
/// Influence score is the same `(falloff_range / max(distance, near_clip))²`
/// heuristic the spot ranker uses, so spot and point rank consistently.
///
/// Returns a Vec indexed by light index: each entry is a slot (`0..CUBE_COUNT`)
/// or [`crate::lighting::spot_shadow::NO_SHADOW_SLOT`].
pub fn rank_point_lights(
    lights: &[MapLight],
    camera_position: Vec3,
    camera_near_clip: f32,
    eligible_lights: &[bool],
) -> Vec<u32> {
    let candidates: Vec<(usize, f32)> = lights
        .iter()
        .enumerate()
        .filter_map(|(idx, light)| {
            // Pool eligibility: dynamic point lights only. The spot pool owns
            // dynamic spots; baked lights bake their shadow into the lightmap.
            if !light.is_dynamic || light.light_type != LightType::Point {
                return None;
            }
            // Per-light eligibility (visibility + brightness), folded by the
            // caller. Empty slice = all-eligible (first-frame / test contract).
            if idx < eligible_lights.len() && !eligible_lights[idx] {
                return None;
            }
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

    if candidates.len() > CUBE_COUNT {
        log::debug!(
            "[CubeShadowPool] {} pool-eligible point lights; {} assigned to cube slots, {} unshadowed",
            candidates.len(),
            CUBE_COUNT,
            candidates.len() - CUBE_COUNT
        );
    }

    // SHARED scoring/drop core — identical policy to the spot ranker.
    assign_ranked_slots(candidates, CUBE_COUNT, lights.len())
}

/// Renderer-owned cube-array point-shadow pool. A single `Depth32Float`
/// cube-array texture allocated once, with:
///   * one `CubeArray` view for the forward sample path (Task 5 binds it),
///   * per-face `D2Array` render views at `baseArrayLayer = slot*6 + face`.
///
/// Disabled (constructed via [`CubeShadowPool::new`] returning `None`) when the
/// adapter lacks `CUBE_ARRAY_TEXTURES`; point shadows then cleanly off and the
/// spot path is unaffected.
pub struct CubeShadowPool {
    /// Cube-array depth texture (`CUBE_COUNT × 6` layers). Held for ownership —
    /// access goes through the views below.
    #[allow(dead_code)]
    pub array_texture: wgpu::Texture,
    /// `CubeArray` view for sampling in the forward pass. Held for ownership now;
    /// Task 5 binds it into forward group 5. `#[allow(dead_code)]` until then.
    #[allow(dead_code)]
    pub sampling_view: wgpu::TextureView,
    /// Per-face `D2Array` render-attachment views, indexed `slot*6 + face`.
    pub face_views: Vec<wgpu::TextureView>,
    /// Per-(slot,face) light-space matrix for the occupant, written each frame in
    /// the renderer's slot-update step. `None` = unoccupied face (skipped by the
    /// render loop). Indexed `slot*6 + face`.
    pub face_matrices: Vec<Option<Mat4>>,
    /// Per-frame slot assignment: `slot_assignment[light_index]` = slot
    /// (`0..CUBE_COUNT`) or [`crate::lighting::spot_shadow::NO_SHADOW_SLOT`].
    pub slot_assignment: Vec<u32>,
    /// Per-slot entity-occluder gate, written alongside `face_matrices`. `true`
    /// only when the slot's occupant passes
    /// [`crate::lighting::entity_occluder_eligible`]. Cube faces are ENTITY-ONLY
    /// in v1, so an ineligible point light draws NOTHING into its faces.
    pub slot_entity_eligible: Vec<bool>,
}

impl CubeShadowPool {
    /// Allocate the cube-array pool at renderer init, or `None` when the adapter
    /// lacks `CUBE_ARRAY_TEXTURES` (point shadows then disabled, spot unaffected).
    ///
    /// `cube_array_supported` is the renderer's queried
    /// `DownlevelFlags::CUBE_ARRAY_TEXTURES`; passed in (not queried here) so the
    /// GPU-capability decision stays at the renderer's adapter boundary and the
    /// pure disable logic is exercisable via [`cube_pool_enabled`] without a GPU.
    pub fn new(device: &wgpu::Device, cube_array_supported: bool) -> Option<Self> {
        if !cube_pool_enabled(cube_array_supported) {
            return None;
        }

        let layer_count = (CUBE_COUNT * CUBE_FACES) as u32;
        let array_texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("Cube Shadow Depth Array"),
            size: wgpu::Extent3d {
                width: CUBE_FACE_RESOLUTION,
                height: CUBE_FACE_RESOLUTION,
                depth_or_array_layers: layer_count,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: SHADOW_DEPTH_FORMAT,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING,
            view_formats: &[],
        });

        // Per-face D2Array render views: one layer each, at `slot*6 + face`.
        // (`D2Array` rather than `D2` because the cube-array texture's layers are
        // addressed as array layers; a single-layer D2Array view is a valid
        // render attachment.)
        let face_views: Vec<wgpu::TextureView> = (0..layer_count)
            .map(|layer| {
                array_texture.create_view(&wgpu::TextureViewDescriptor {
                    label: Some(&format!("Cube Shadow Face View {layer}")),
                    dimension: Some(wgpu::TextureViewDimension::D2Array),
                    base_array_layer: layer,
                    array_layer_count: Some(1),
                    ..Default::default()
                })
            })
            .collect();

        // CubeArray sampling view spanning all slots, for the forward pass.
        let sampling_view = array_texture.create_view(&wgpu::TextureViewDescriptor {
            label: Some("Cube Shadow Sampling View"),
            dimension: Some(wgpu::TextureViewDimension::CubeArray),
            base_array_layer: 0,
            array_layer_count: Some(layer_count),
            ..Default::default()
        });

        Some(Self {
            array_texture,
            sampling_view,
            face_views,
            face_matrices: vec![None; CUBE_COUNT * CUBE_FACES],
            slot_assignment: Vec::new(),
            slot_entity_eligible: vec![false; CUBE_COUNT],
        })
    }

    /// Flat index into `face_views` / `face_matrices` for a `(slot, face)` pair.
    pub fn face_layer(slot: u32, face: usize) -> usize {
        slot as usize * CUBE_FACES + face
    }
}

/// Pure disable decision for the cube pool, factored out so the
/// adapter-capability gate is unit-testable without a GPU. The pool allocates
/// iff the adapter reports `CUBE_ARRAY_TEXTURES`.
pub fn cube_pool_enabled(cube_array_supported: bool) -> bool {
    cube_array_supported
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::lighting::spot_shadow::NO_SHADOW_SLOT;
    use crate::prl::{FalloffModel, ShadowType};

    fn point_light(origin: [f64; 3], falloff_range: f32, is_dynamic: bool) -> MapLight {
        MapLight {
            origin,
            light_type: LightType::Point,
            intensity: 1.0,
            color: [1.0, 1.0, 1.0],
            falloff_model: FalloffModel::InverseSquared,
            falloff_range,
            cone_angle_inner: 0.0,
            cone_angle_outer: 0.0,
            cone_direction: [0.0, 0.0, 0.0],
            is_dynamic,
            casts_entity_shadows: false,
            animated_slot: None,
            tags: vec![],
            leaf_index: 0,
            shadow_type: ShadowType::StaticLightMap,
        }
    }

    // --- Per-face matrix math ------------------------------------------------

    /// The 6 cube faces must tile the full sphere: every axis direction from the
    /// light maps into exactly the matching face (its forward axis projects to
    /// the near center and lands inside that face's NDC), and into no other.
    #[test]
    fn cube_faces_tile_the_sphere() {
        let light = point_light([0.0, 0.0, 0.0], 20.0, true);
        let mats = cube_face_matrices(&light);

        // For each face, a point a short distance along its forward axis must
        // project inside that face's NDC (|x|,|y| ≤ 1, 0 ≤ z ≤ 1) and outside
        // every other face's NDC.
        for (face, dir) in CUBE_FACE_DIRS.iter().enumerate() {
            let world = *dir * 5.0; // 5 m along the face axis, within range
            for (other, m) in mats.iter().enumerate() {
                let clip = *m * world.extend(1.0);
                // Behind the camera or w<=0 → not in this face.
                let inside = clip.w > 0.0 && {
                    let ndc = clip.truncate() / clip.w;
                    ndc.x.abs() <= 1.0 + 1e-3
                        && ndc.y.abs() <= 1.0 + 1e-3
                        && ndc.z >= -1e-3
                        && ndc.z <= 1.0 + 1e-3
                };
                if other == face {
                    assert!(
                        inside,
                        "direction along face {face} must project inside face {face}'s NDC"
                    );
                } else {
                    assert!(
                        !inside,
                        "direction along face {face} must NOT project inside face {other}'s NDC"
                    );
                }
            }
        }
    }

    /// A known off-axis direction lands in the expected face. A point mostly
    /// along +X but slightly up must still belong to the +X face (index 0),
    /// since +X is the dominant axis.
    #[test]
    fn dominant_axis_direction_maps_to_expected_face() {
        let light = point_light([0.0, 0.0, 0.0], 20.0, true);
        let mats = cube_face_matrices(&light);

        // Mostly +X with a small +Y component: dominant axis is +X (face 0).
        let world = Vec3::new(5.0, 1.0, 0.0);
        let clip = mats[0] * world.extend(1.0);
        assert!(
            clip.w > 0.0,
            "+X-dominant point must be in front of +X face"
        );
        let ndc = clip.truncate() / clip.w;
        assert!(
            ndc.x.abs() <= 1.0 && ndc.y.abs() <= 1.0 && (0.0..=1.0).contains(&ndc.z),
            "+X-dominant point must project inside the +X face NDC, got {ndc:?}"
        );
    }

    /// The far plane follows the light's falloff range: a point just beyond the
    /// range projects past the far plane (NDC z > 1) on its face.
    #[test]
    fn far_plane_tracks_falloff_range() {
        let light = point_light([0.0, 0.0, 0.0], 10.0, true);
        let mats = cube_face_matrices(&light);
        // 12 m along +X is beyond the 10 m range.
        let clip = mats[0] * Vec3::new(12.0, 0.0, 0.0).extend(1.0);
        let ndc_z = clip.z / clip.w;
        assert!(
            ndc_z > 1.0,
            "point beyond falloff range must fall past the far plane (z>1), got {ndc_z}"
        );
    }

    /// A degenerate (zero-range) light must not produce a zero-extent or NaN
    /// frustum — the far clamp keeps the matrices finite and invertible-ish.
    #[test]
    fn zero_range_light_yields_finite_matrices() {
        let light = point_light([1.0, 2.0, 3.0], 0.0, true);
        let mats = cube_face_matrices(&light);
        for m in mats {
            for v in m.to_cols_array() {
                assert!(v.is_finite(), "face matrix entries must be finite");
            }
        }
    }

    // --- Ranking (capacity-agnostic) -----------------------------------------

    /// Pool eligibility is `is_dynamic && Point`: a dynamic point light gets a
    /// slot; a baked point light and a dynamic spot do not.
    #[test]
    fn dynamic_point_qualifies_baked_and_spot_do_not() {
        let mut spot = point_light([0.0, 0.0, 0.0], 10.0, true);
        spot.light_type = LightType::Spot;
        let lights = vec![
            point_light([0.0, 0.0, 0.0], 10.0, true), // dynamic point → slot
            point_light([5.0, 0.0, 0.0], 10.0, false), // baked point → no slot
            spot,                                     // dynamic spot → no slot
        ];
        let a = rank_point_lights(&lights, Vec3::ZERO, 0.1, &[]);
        assert_ne!(a[0], NO_SHADOW_SLOT, "dynamic point must get a cube slot");
        assert_eq!(a[1], NO_SHADOW_SLOT, "baked point must not get a slot");
        assert_eq!(
            a[2], NO_SHADOW_SLOT,
            "dynamic spot must not get a cube slot"
        );
    }

    /// Closer lights outrank farther ones (shared influence heuristic).
    #[test]
    fn closer_point_light_ranks_higher() {
        let lights = vec![
            point_light([0.0, 0.0, 0.0], 10.0, true),
            point_light([100.0, 0.0, 0.0], 10.0, true),
        ];
        let a = rank_point_lights(&lights, Vec3::ZERO, 0.1, &[]);
        assert_eq!(a[0], 0, "closer light gets slot 0");
        assert_eq!(a[1], 1, "farther light gets slot 1");
    }

    /// Capacity-agnostic overflow: with `CUBE_COUNT + N` eligible lights, exactly
    /// `CUBE_COUNT` get slots and the rest are dropped gracefully — no panic, no
    /// duplicate slots. Parameterized on the configured `CUBE_COUNT`.
    #[test]
    fn overflow_drops_lowest_ranked_within_capacity() {
        let overflow = 4;
        let total = CUBE_COUNT + overflow;
        // Place lights at increasing distance so rank == index; the farthest
        // `overflow` lights are the ones dropped.
        let lights: Vec<MapLight> = (0..total)
            .map(|i| point_light([i as f64 * 10.0, 0.0, 0.0], 10.0, true))
            .collect();
        let a = rank_point_lights(&lights, Vec3::ZERO, 0.1, &[]);

        let assigned: Vec<u32> = a.iter().copied().filter(|&s| s != NO_SHADOW_SLOT).collect();
        assert_eq!(
            assigned.len(),
            CUBE_COUNT,
            "exactly CUBE_COUNT lights get slots regardless of candidate count"
        );
        // Every assigned slot is unique and within capacity.
        let mut sorted = assigned.clone();
        sorted.sort_unstable();
        sorted.dedup();
        assert_eq!(sorted.len(), assigned.len(), "no slot assigned twice");
        assert!(
            assigned.iter().all(|&s| (s as usize) < CUBE_COUNT),
            "every slot index is within capacity"
        );
        // The closest CUBE_COUNT lights (indices 0..CUBE_COUNT) are the survivors.
        for (i, slot) in a.iter().enumerate() {
            if i < CUBE_COUNT {
                assert_ne!(*slot, NO_SHADOW_SLOT, "closest light {i} must keep a slot");
            } else {
                assert_eq!(*slot, NO_SHADOW_SLOT, "overflow light {i} must be dropped");
            }
        }
    }

    /// The per-light eligibility slice gates slot allocation: an ineligible light
    /// is dropped even when it would otherwise rank first.
    #[test]
    fn eligibility_slice_culls_ineligible_lights() {
        let lights = vec![
            point_light([0.0, 0.0, 0.0], 10.0, true), // closest, but ineligible
            point_light([50.0, 0.0, 0.0], 10.0, true),
        ];
        let a = rank_point_lights(&lights, Vec3::ZERO, 0.1, &[false, true]);
        assert_eq!(a[0], NO_SHADOW_SLOT, "ineligible light must be dropped");
        assert_ne!(a[1], NO_SHADOW_SLOT, "eligible light keeps its slot");
    }

    #[test]
    fn empty_light_list_produces_empty_assignment() {
        let a = rank_point_lights(&[], Vec3::ZERO, 0.1, &[]);
        assert!(a.is_empty());
    }

    // --- Adapter gate (no GPU) -----------------------------------------------

    /// The cube pool allocates iff the adapter reports `CUBE_ARRAY_TEXTURES`.
    /// Tested through the pure decision so no GPU context is needed.
    #[test]
    fn cube_pool_disabled_without_cube_array_support() {
        assert!(
            !cube_pool_enabled(false),
            "no CUBE_ARRAY_TEXTURES → pool disabled (point shadows cleanly off)"
        );
        assert!(
            cube_pool_enabled(true),
            "CUBE_ARRAY_TEXTURES present → pool enabled"
        );
    }

    #[test]
    fn face_layer_indexes_slot_times_six_plus_face() {
        assert_eq!(CubeShadowPool::face_layer(0, 0), 0);
        assert_eq!(CubeShadowPool::face_layer(0, 5), 5);
        assert_eq!(CubeShadowPool::face_layer(1, 0), 6);
        assert_eq!(CubeShadowPool::face_layer(2, 3), 15);
    }
}
