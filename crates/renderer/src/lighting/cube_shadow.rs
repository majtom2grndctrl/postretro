// Dynamic point-light cube-array shadow pool: per-light 6-face omnidirectional
// depth, ranked into a fixed-capacity cube-array. Each occupied face renders
// cone-culled WORLD geometry (static occluders — crates, pillars) plus skinned
// entity occluders, mirroring the spot pool's occluder split.
//
// WHY per-face world draws fit the budget: a naive world draw costs 6 full
// world-BVH rasterizations per point light. The per-region GPU frustum cull
// (`shadow_cull.rs`, one indirect sub-region per (slot, face) gated by that
// face's 90° frustum) bounds the world cost to the geometry each face can
// actually see, which is what lets dynamic point lights shadow static geometry
// under a predictable budget.
//
// See: context/lib/rendering_pipeline.md §7.1 (shadow passes), §4 (lighting)

use crate::lighting::spot_shadow::SHADOW_DEPTH_FORMAT;
use glam::{Mat4, Vec3};
use postretro_level_loader::MapLight;

/// Near-clip distance for a cube face's perspective projection. Matches the spot
/// path's `SHADOW_NEAR_CLIP` — close enough that depth bias controls acne, far
/// enough to keep precision.
pub const CUBE_NEAR_CLIP: f32 = 0.1;

/// Number of cube slots in the pool, sized to realistic concurrent demand after
/// PVS culling + influence ranking — NOT worst case. Each occupied slot draws
/// cone-culled world geometry + entity occluders into 6 faces, so the cost
/// (and the VRAM) scales with this.
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

/// Per-layer look directions for a slot's 6 faces, paired with
/// [`CUBE_FACE_UPS`].
///
/// WHY this is not the plain +X,-X,+Y,-Y,+Z,-Z hardware face order: the
/// hardware cube-sampling convention's per-face (s, t, major-axis) basis is
/// LEFT-handed, so no right-handed `look_at_rh` view can reproduce it — one
/// mirror is required somewhere between render and sample. Rather than mirror
/// the projection (which reverses triangle winding and would need `Cw`
/// variants of the shared depth pipelines), the shader mirrors the LOOKUP:
/// `sample_point_shadow` flips the y component of the direction before
/// sampling (`shadow_sample.wgsl`). Under a y-flipped lookup the hardware's
/// ±Y faces exchange, so layer 2 (hardware +Y) holds the image rendered
/// looking -Y and layer 3 (hardware -Y) the image rendered looking +Y; the
/// other four faces are y-mirror-invariant in face SELECTION and come out
/// texel-exact with their GL-style ups below. The
/// `cube_face_layers_round_trip_hardware_sampling` test pins the whole
/// arrangement (tables + shader flip) against an emulation of the hardware
/// convention, so neither side can drift alone.
const CUBE_FACE_DIRS: [Vec3; CUBE_FACES] = [
    Vec3::new(1.0, 0.0, 0.0),  // layer 0: hardware +X face
    Vec3::new(-1.0, 0.0, 0.0), // layer 1: hardware -X face
    Vec3::new(0.0, -1.0, 0.0), // layer 2: hardware +Y face (y-flipped lookup)
    Vec3::new(0.0, 1.0, 0.0),  // layer 3: hardware -Y face (y-flipped lookup)
    Vec3::new(0.0, 0.0, 1.0),  // layer 4: hardware +Z face
    Vec3::new(0.0, 0.0, -1.0), // layer 5: hardware -Z face
];

/// Per-face "up" vectors, paired with `CUBE_FACE_DIRS`. The Y-looking layers
/// use a Z up (any vector not colinear with the look direction); the other
/// four use -Y. Together with the shader's y-flipped lookup these place every
/// direction at exactly the texel the hardware sampler reads — see the
/// [`CUBE_FACE_DIRS`] doc and the round-trip test for the derivation.
const CUBE_FACE_UPS: [Vec3; CUBE_FACES] = [
    Vec3::new(0.0, -1.0, 0.0), // layer 0 (+X)
    Vec3::new(0.0, -1.0, 0.0), // layer 1 (-X)
    Vec3::new(0.0, 0.0, -1.0), // layer 2 (looks -Y)
    Vec3::new(0.0, 0.0, 1.0),  // layer 3 (looks +Y)
    Vec3::new(0.0, -1.0, 0.0), // layer 4 (+Z)
    Vec3::new(0.0, -1.0, 0.0), // layer 5 (-Z)
];

/// Build the 6 light-space view-projection matrices for a point light's cube
/// faces, in `CUBE_FACE_DIRS` order. Each is a 90° perspective (aspect 1.0,
/// near `CUBE_NEAR_CLIP`, far from the light's falloff range) times that face's
/// look-at view. Together the 6 frusta tile the full sphere around the light.
///
/// Pure math — no GPU. Unit-tested for sphere coverage and direction→face
/// mapping. The far plane clamps `falloff_range` to a small minimum so a
/// zero-range or degenerate light still yields a finite frustum.
///
/// Far-plane freshness contract: dynamic candidates are refreshed from the
/// current GPU upload before this function runs. The projection and the
/// shader's depth reconstruction therefore use the same origin and range,
/// including attached-light movement and CPU-evaluated radius animation.
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

/// Renderer-owned cube-array point-shadow pool. A single `Depth32Float`
/// cube-array texture allocated once, with:
///   * one `CubeArray` view bound into the forward pass's group-5 sample path,
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
    /// `CubeArray` view for sampling in the forward pass — bound into the shared
    /// group-5 BGL at binding 5 (`render/mod.rs`), sampled by the point-light
    /// case of the forward light loop.
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
    /// [`postretro_lighting::entity_occluder_eligible`]. Gates ONLY the skinned
    /// entity draw — every occupied face renders its world-depth baseline
    /// regardless, exactly like the spot pool's per-slot entity gate.
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
            // Pool faces are cleared render attachments for entity-only draws;
            // static world depth remains separately sampled from the promoted cache.
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT
                | wgpu::TextureUsages::TEXTURE_BINDING
                | wgpu::TextureUsages::COPY_DST,
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

    /// Clear all per-frame occupancy: face matrices, entity gates, assignment.
    /// The cube counterpart of [`SpotShadowPool::clear_occupancy`] — called
    /// when a frame ranks zero candidates so no stale occupied face keeps its
    /// 6 depth passes rasterizing world geometry that no light samples.
    ///
    /// [`SpotShadowPool::clear_occupancy`]: crate::lighting::spot_shadow::SpotShadowPool::clear_occupancy
    pub fn clear_occupancy(&mut self) {
        self.face_matrices.fill(None);
        self.slot_entity_eligible.fill(false);
        self.slot_assignment.clear();
    }
}

/// Pure disable decision for the cube pool, factored out so the
/// adapter-capability gate is unit-testable without a GPU. The pool allocates
/// iff the adapter reports `CUBE_ARRAY_TEXTURES`.
pub fn cube_pool_enabled(cube_array_supported: bool) -> bool {
    cube_array_supported
}

/// Whether the cube depth loop must open a `Clear(1.0)` render pass for an
/// occupied (`face_matrix.is_some()`) face THIS frame.
///
/// The decisive invariant the renderer loop must honour: an occupied face is
/// cleared to the far plane (NDC depth 1.0) EVERY frame it is occupied,
/// regardless of whether any world geometry or skinned-mesh occluder exists —
/// exactly as the spot pool always lays down a `Clear(1.0)` baseline for every
/// occupied slot. Every ranked slot's faces are occupied (occupancy is
/// independent of entity eligibility), and the world-depth draw rides the same
/// pass, so a ranked slot is always safe for the shader to sample.
///
/// Returns `true` for any occupied face. The depth loop gates the occluder
/// draws (world on `has_geometry`, entities on the mesh frame plan + the
/// slot's entity eligibility), but the clear itself must NOT be gated: gating
/// it (the prior bug) left an occupied cube uncleared whenever no occluder
/// existed, so an on-screen point light sampled stale/zero depth and read
/// fully shadowed (`CompareFunction::Less`: a positive reference is never
/// `< 0`), zeroing its world illumination.
///
/// With the clear unconditional, an occluder-free face stores 1.0, the shader's
/// reference (`<= 1.0`) compares `reference < 1.0` for any fragment nearer than
/// the far plane, and every PCF tap returns lit — shadow factor 1.0 (full light).
pub fn cube_face_needs_clear(face_occupied: bool) -> bool {
    face_occupied
}

#[cfg(test)]
mod tests {
    use super::*;
    use postretro_level_loader::{FalloffModel, ShadowType};

    fn point_light(origin: [f64; 3], falloff_range: f32, is_dynamic: bool) -> MapLight {
        MapLight {
            origin,
            light_type: postretro_level_loader::LightType::Point,
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
            cell_index: 0,
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

    /// Regression (branch `claude/dynamic-mesh-shadows-jvl96j`, the bug a91bb61
    /// MISSED): an on-screen dynamic point light owns a ranked cube slot and the
    /// shader samples it. Its occupied faces must be cleared to the far plane
    /// (1.0) every frame they are occupied, EVEN when no occluder is drawn (no
    /// skinned mesh in the PVS, and — for empty maps — no world geometry).
    /// Before the fix the whole cube depth loop — clear included — was gated on
    /// a mesh frame plan existing, so with no in-PVS mesh the occupied faces
    /// were never cleared and held ~0.0; the on-screen light then read fully
    /// shadowed and winked out, while off-screen (no-slot/sentinel) lights
    /// stayed lit.
    ///
    /// `cube_face_needs_clear` encodes the corrected invariant: an occupied face
    /// is cleared regardless of occluder presence. Unoccupied faces are skipped.
    /// (With world geometry now rendered into every occupied face, occupancy no
    /// longer depends on entity eligibility, so the invariant covers every
    /// ranked slot.)
    #[test]
    fn occupied_face_clears_without_occluders() {
        // Occupied face: must be cleared whether or not any occluder exists.
        assert!(
            cube_face_needs_clear(true),
            "an occupied cube face must be cleared (to far=1.0) every frame, \
             with or without occluder draws — otherwise an on-screen point \
             light samples uncleared depth and is zeroed"
        );
        // Unoccupied face (no per-face matrix): nothing to clear or sample.
        assert!(
            !cube_face_needs_clear(false),
            "an unoccupied cube face is skipped (no clear, no sample)"
        );
    }

    #[test]
    fn face_layer_indexes_slot_times_six_plus_face() {
        assert_eq!(CubeShadowPool::face_layer(0, 0), 0);
        assert_eq!(CubeShadowPool::face_layer(0, 5), 5);
        assert_eq!(CubeShadowPool::face_layer(1, 0), 6);
        assert_eq!(CubeShadowPool::face_layer(2, 3), 15);
    }

    // --- WGSL/Rust constant sync (forward cube sampling) ---------------------

    /// The forward shader reconstructs each cube fragment's NDC depth from the
    /// SAME near plane and face resolution the depth pass projected with. Pin the
    /// WGSL literals against the Rust source of truth so a change to one without
    /// the other (which would silently mis-shadow or break the PCF spacing) fails
    /// here rather than only on a live GPU.
    #[test]
    fn forward_cube_sampling_constants_match_pool() {
        // The cube-sampling constants and PCF kernel live in the shared
        // `shadow_sample.wgsl` snippet (extracted from forward.wgsl so the
        // skinned-mesh pass can reuse them), concatenated into the forward
        // module at pipeline build.
        const SHADOW_SRC: &str = include_str!("../shaders/shadow_sample.wgsl");

        // CUBE_NEAR_CLIP must match `cube_face_matrices`' near plane.
        let near_marker = "const CUBE_NEAR_CLIP: f32 = ";
        let near = SHADOW_SRC
            .find(near_marker)
            .map(|i| i + near_marker.len())
            .and_then(|start| {
                let end = SHADOW_SRC[start..].find(';')? + start;
                SHADOW_SRC[start..end].trim().parse::<f32>().ok()
            })
            .expect("shadow_sample.wgsl must declare CUBE_NEAR_CLIP");
        assert_eq!(
            near, CUBE_NEAR_CLIP,
            "shadow_sample.wgsl CUBE_NEAR_CLIP must match cube_shadow::CUBE_NEAR_CLIP"
        );

        // The PCF tap spacing divides by the named `CUBE_FACE_RESOLUTION` const,
        // which must equal the Rust `CUBE_FACE_RESOLUTION`. Pin the WGSL const's
        // value so the two cannot drift, and confirm the tap spacing actually uses
        // the named const (not a re-introduced bare literal).
        let res_marker = "const CUBE_FACE_RESOLUTION: f32 = ";
        let res = SHADOW_SRC
            .find(res_marker)
            .map(|i| i + res_marker.len())
            .and_then(|start| {
                let end = SHADOW_SRC[start..].find(';')? + start;
                SHADOW_SRC[start..end].trim().parse::<f32>().ok()
            })
            .expect("shadow_sample.wgsl must declare CUBE_FACE_RESOLUTION");
        assert_eq!(
            res, CUBE_FACE_RESOLUTION as f32,
            "shadow_sample.wgsl CUBE_FACE_RESOLUTION must match cube_shadow::CUBE_FACE_RESOLUTION"
        );
        assert!(
            SHADOW_SRC.contains("/ CUBE_FACE_RESOLUTION)"),
            "shadow_sample.wgsl must scale the PCF tap by the named CUBE_FACE_RESOLUTION const"
        );
    }

    /// WebGPU cube-map sampling emulation: face selection by dominant axis
    /// plus the spec's per-face (sc, tc, ma) table, with t increasing DOWNWARD
    /// (wgpu top-left texture origin). This is what `textureSampleCompareLevel`
    /// does with the lookup vector before the depth compare.
    fn hardware_cube_lookup(r: Vec3) -> (usize, f32, f32) {
        let (ax, ay, az) = (r.x.abs(), r.y.abs(), r.z.abs());
        let (face, sc, tc, ma) = if ax >= ay && ax >= az {
            if r.x >= 0.0 {
                (0, -r.z, -r.y, ax)
            } else {
                (1, r.z, -r.y, ax)
            }
        } else if ay >= ax && ay >= az {
            if r.y >= 0.0 {
                (2, r.x, r.z, ay)
            } else {
                (3, r.x, -r.z, ay)
            }
        } else if r.z >= 0.0 {
            (4, r.x, -r.y, az)
        } else {
            (5, -r.x, -r.y, az)
        };
        (face, 0.5 * (sc / ma + 1.0), 0.5 * (tc / ma + 1.0))
    }

    /// The load-bearing invariant of the whole cube path: for ANY direction,
    /// the depth texel the shader's y-flipped lookup reads is exactly the
    /// texel the depth pass rendered that direction's geometry to — same
    /// layer, same uv, and the same NDC depth the shader reconstructs as its
    /// compare reference. The hardware convention's per-face basis is
    /// left-handed, so a right-handed face render can only match it through
    /// one mirror: the shader's lookup y-flip, completed by the swapped ±Y
    /// layers in `CUBE_FACE_DIRS`. If either side changes alone (the tables,
    /// the layer order, or the WGSL flip), every direction reads a mirrored
    /// texel — walls compare against ceiling/floor depths and shade
    /// themselves into darkness under a point light that should light them.
    #[test]
    fn cube_face_layers_round_trip_hardware_sampling() {
        let far = 20.0f32;
        let light = point_light([0.0, 0.0, 0.0], far, true);
        let mats = cube_face_matrices(&light);

        // Pin the WGSL flip so it can't be "simplified away" without failing
        // here (the render-side tables assume it).
        const SHADOW_SRC: &str = include_str!("../shaders/shadow_sample.wgsl");
        assert!(
            SHADOW_SRC.contains("vec3<f32>(dir.x, -dir.y, dir.z)"),
            "shadow_sample.wgsl must sample the cube with a y-flipped lookup vector"
        );

        // Dense sphere sweep — off-axis directions on every face.
        for i in 0..32 {
            for j in 0..16 {
                let theta = (i as f32 + 0.5) / 32.0 * std::f32::consts::TAU;
                let phi = (j as f32 + 0.5) / 16.0 * std::f32::consts::PI;
                let r = Vec3::new(phi.sin() * theta.cos(), phi.cos(), phi.sin() * theta.sin());
                let dist = 5.0f32;

                // Shader side: y-flipped lookup → hardware face + uv.
                let (face, hu, hv) = hardware_cube_lookup(Vec3::new(r.x, -r.y, r.z));

                // Render side: the same world point through that layer's matrix.
                let clip = mats[face] * (r * dist).extend(1.0);
                assert!(clip.w > 0.0, "dir {r:?} must be in front of layer {face}");
                let ndc = clip.truncate() / clip.w;
                let (ru, rv) = (0.5 + 0.5 * ndc.x, 0.5 - 0.5 * ndc.y);
                assert!(
                    (hu - ru).abs() < 1e-4 && (hv - rv).abs() < 1e-4,
                    "dir {r:?}: hardware reads layer {face} uv ({hu:.4},{hv:.4}) but \
                     the depth pass rendered it at ({ru:.4},{rv:.4})"
                );

                // The shader's reference-depth reconstruction matches the NDC
                // depth the pass stored at that texel.
                let axis_depth = r.x.abs().max(r.y.abs()).max(r.z.abs()) * dist;
                let a = far / (far - CUBE_NEAR_CLIP);
                let reference = a - (CUBE_NEAR_CLIP * far) / ((far - CUBE_NEAR_CLIP) * axis_depth);
                assert!(
                    (reference - ndc.z).abs() < 1e-4,
                    "dir {r:?}: reconstructed reference {reference} != stored NDC depth {}",
                    ndc.z
                );
            }
        }
    }
}
