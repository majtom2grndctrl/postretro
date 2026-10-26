// Planar water reflection, GPU-free: the mirror camera, candidate planes, selection and presets.
// See: context/lib/rendering_pipeline.md §7.3a

use glam::{Mat4, Vec3, Vec4};
use postretro_level_format::environment_volumes::EnvironmentVolumeRecord;
use postretro_level_loader::LevelWorld;
use postretro_visibility::{Frustum, extract_frustum_planes};

use crate::fluid_surface::{
    face_area_centroid, face_normal,
    fog::FLUID_FOG_BAND,
    reflection_quality::{FluidReflectionQuality, reflection_target_extent},
};

/// Faces, and fluid triangles, within this many metres of one height share a plane.
pub const PLANE_HEIGHT_TOLERANCE: f32 = 0.01;
/// The oblique near plane sits this far beneath the water, so the waterline
/// keeps its own geometry in the mirror.
pub const PLANAR_CLIP_BIAS: f32 = 0.02;
/// A challenger must score above this multiple of the held plane to switch.
pub const PLANE_SELECTION_HYSTERESIS: f32 = 1.25;

/// One planar tier's engine-constant preset: the planar layer's per-axis
/// scale of the scene extent and the absolute-metre distance limits. `None`
/// leaves a distance unlimited. Never a player setting or map key.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PlanarPreset {
    pub scale: f32,
    /// Mirror reach: distance from the mirror eye to the reflected surface.
    pub d: Option<f32>,
    pub band_d: f32,
    /// Receiver reach: distance from the main eye to the water fragment.
    pub r: Option<f32>,
    pub band_r: f32,
}

/// Both planar tiers start at half scale with D and R off; the owner retunes
/// them from a motion read.
const PLANAR_PRESET_START: PlanarPreset = PlanarPreset {
    scale: 0.5,
    d: None,
    band_d: 4.0,
    r: None,
    band_r: 4.0,
};

const PLANAR_PRESETS: [(FluidReflectionQuality, PlanarPreset); 2] = [
    (FluidReflectionQuality::VeryHigh, PLANAR_PRESET_START),
    (FluidReflectionQuality::Ultra, PLANAR_PRESET_START),
];

/// The planar preset of `quality`: `Some` only at very high and ultra, the
/// tiers that run the planar layer.
pub fn planar_preset(quality: FluidReflectionQuality) -> Option<PlanarPreset> {
    PLANAR_PRESETS
        .iter()
        .find(|(tier, _)| *tier == quality)
        .map(|(_, preset)| preset.sanitized())
}

impl PlanarPreset {
    /// A supported scale (¼, ½ or 1), positive finite limits and bands.
    pub fn sanitized(self) -> Self {
        let scale = if self.scale.is_finite() {
            [0.25_f32, 0.5, 1.0]
                .into_iter()
                .min_by(|a, b| (self.scale - a).abs().total_cmp(&(self.scale - b).abs()))
                .expect("three supported planar scales")
        } else {
            0.5
        };
        let limit = |value: Option<f32>| value.filter(|v| v.is_finite() && *v > 0.0);
        let band = |value: f32| {
            if value.is_finite() {
                value.max(0.001)
            } else {
                4.0
            }
        };
        Self {
            scale,
            d: limit(self.d),
            band_d: band(self.band_d),
            r: limit(self.r),
            band_r: band(self.band_r),
        }
    }

    /// The planar layer's extent for a `scene` extent: rounded up, at least 1×1.
    pub fn target_extent(self, scene: (u32, u32)) -> (u32, u32) {
        reflection_target_extent(scene, self.sanitized().scale)
    }
}

/// Absolute-metre fade. An unlimited distance keeps full coverage.
pub fn planar_fade(distance: f32, limit: Option<f32>, band: f32) -> f32 {
    match limit {
        None => 1.0,
        Some(limit) if band > 0.0 => ((limit - distance) / band).clamp(0.0, 1.0),
        Some(limit) => f32::from(distance < limit),
    }
}

/// Install-time face geometry of one candidate plane.
#[derive(Debug, Clone, PartialEq)]
pub struct PlanarFace {
    pub volume_index: u32,
    pub face_index: u32,
    pub vertices: Vec<Vec3>,
    pub bounds_min: Vec3,
    pub bounds_max: Vec3,
    pub centroid: Vec3,
    pub area: f32,
}

#[derive(Debug, Clone, PartialEq)]
pub struct PlanarCandidate {
    pub h: f32,
    pub bounds_min: Vec3,
    pub bounds_max: Vec3,
    pub faces: Vec<PlanarFace>,
}

#[derive(Debug, Clone, Default)]
pub struct PlanarCandidates {
    pub planes: Vec<PlanarCandidate>,
}

impl PlanarCandidates {
    /// Only probe-bound, reflecting-variant volumes belong in the input ids.
    pub fn from_volumes(
        volumes: &[EnvironmentVolumeRecord],
        reflecting_volume_indices: &[u32],
    ) -> Self {
        let mut faces = Vec::new();
        for (volume_index, volume) in volumes.iter().enumerate() {
            if !reflecting_volume_indices.contains(&(volume_index as u32)) {
                continue;
            }
            for (face_index, face) in volume.surface_faces.iter().enumerate() {
                let Some(normal) = face_normal(face).filter(|n| n.y > 0.9999) else {
                    continue;
                };
                let vertices: Vec<Vec3> = face
                    .vertices
                    .iter()
                    .map(|v| Vec3::from(v.position))
                    .collect();
                let bounds_min = vertices
                    .iter()
                    .copied()
                    .fold(Vec3::splat(f32::INFINITY), Vec3::min);
                let bounds_max = vertices
                    .iter()
                    .copied()
                    .fold(Vec3::splat(f32::NEG_INFINITY), Vec3::max);
                if bounds_max.y - bounds_min.y > PLANE_HEIGHT_TOLERANCE {
                    continue;
                }
                let first = vertices[0];
                let area = (1..vertices.len() - 1)
                    .map(|i| {
                        (vertices[i] - first)
                            .cross(vertices[i + 1] - first)
                            .dot(normal)
                            * 0.5
                    })
                    .sum::<f32>()
                    .abs();
                if !area.is_finite() || area <= 0.0 {
                    continue;
                }
                faces.push(PlanarFace {
                    volume_index: volume_index as u32,
                    face_index: face_index as u32,
                    centroid: face_area_centroid(face, normal).as_vec3(),
                    vertices,
                    bounds_min,
                    bounds_max,
                    area,
                });
            }
        }
        faces.sort_by(|a, b| a.centroid.y.total_cmp(&b.centroid.y));
        let mut planes: Vec<PlanarCandidate> = Vec::new();
        for face in faces {
            if let Some(plane) = planes
                .last_mut()
                .filter(|p| (p.h - face.centroid.y).abs() <= PLANE_HEIGHT_TOLERANCE)
            {
                plane.bounds_min = plane.bounds_min.min(face.bounds_min);
                plane.bounds_max = plane.bounds_max.max(face.bounds_max);
                plane.faces.push(face);
            } else {
                planes.push(PlanarCandidate {
                    h: face.centroid.y,
                    bounds_min: face.bounds_min,
                    bounds_max: face.bounds_max,
                    faces: vec![face],
                });
            }
        }
        Self { planes }
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SelectedPlane {
    pub candidate_index: usize,
    pub h: f32,
    pub bounds_min: Vec3,
    pub bounds_max: Vec3,
    pub score: f32,
}

/// Estimated projected solid angle, with 1.25× persistence for the prior plane.
pub fn select_plane(
    candidates: &PlanarCandidates,
    eye: Vec3,
    main_vp: Mat4,
    previous: Option<SelectedPlane>,
) -> Option<SelectedPlane> {
    let frustum = extract_frustum_planes(main_vp);
    let score = |plane: &PlanarCandidate| -> f32 {
        if eye.y - plane.h <= FLUID_FOG_BAND {
            return 0.0;
        }
        plane
            .faces
            .iter()
            .filter(|face| bounds_in_frustum(face.bounds_min, face.bounds_max, &frustum))
            .map(|face| {
                let delta = eye - face.centroid;
                face.area * delta.y.abs() / delta.length_squared().max(0.0001).powf(1.5)
            })
            .sum()
    };
    let mut best: Option<SelectedPlane> = None;
    for (candidate_index, plane) in candidates.planes.iter().enumerate() {
        let score = score(plane);
        if score > 0.0 && best.is_none_or(|best| score > best.score) {
            best = Some(SelectedPlane {
                candidate_index,
                h: plane.h,
                bounds_min: plane.bounds_min,
                bounds_max: plane.bounds_max,
                score,
            });
        }
    }
    if let (Some(best), Some(previous)) = (best, previous)
        && let Some(plane) = candidates.planes.get(previous.candidate_index)
    {
        let previous_score = score(plane);
        if previous_score > 0.0 && best.score <= previous_score * PLANE_SELECTION_HYSTERESIS {
            return Some(SelectedPlane {
                candidate_index: previous.candidate_index,
                h: plane.h,
                bounds_min: plane.bounds_min,
                bounds_max: plane.bounds_max,
                score: previous_score,
            });
        }
    }
    best
}

fn bounds_in_frustum(min: Vec3, max: Vec3, frustum: &Frustum) -> bool {
    frustum.planes.iter().all(|plane| {
        let p = Vec3::new(
            if plane.normal.x >= 0.0 { max.x } else { min.x },
            if plane.normal.y >= 0.0 { max.y } else { min.y },
            if plane.normal.z >= 0.0 { max.z } else { min.z },
        );
        plane.normal.dot(p) + plane.dist >= 0.0
    })
}

/// Main-visible water cells seed a mirror walk whose apex may lie in solid.
/// `main_cells` may include empty reachable cells. Output storage is reused.
pub fn collect_mirror_seed_cells(
    main_cells: &[u32],
    world: &LevelWorld,
    plane: &SelectedPlane,
    eye: Vec3,
    r: Option<f32>,
    output: &mut Vec<u32>,
) {
    output.clear();
    for &id in main_cells {
        let Some(cell) = world.cells.get(id as usize) else {
            continue;
        };
        if cell.is_solid
            || cell.is_exterior
            || cell.bounds_max.y < plane.h - FLUID_FOG_BAND
            || cell.bounds_min.y > plane.h + FLUID_FOG_BAND
            || cell.bounds_max.x < plane.bounds_min.x
            || cell.bounds_min.x > plane.bounds_max.x
            || cell.bounds_max.z < plane.bounds_min.z
            || cell.bounds_min.z > plane.bounds_max.z
        {
            continue;
        }
        let water_min = Vec3::new(
            cell.bounds_min.x.max(plane.bounds_min.x),
            plane.h,
            cell.bounds_min.z.max(plane.bounds_min.z),
        );
        let water_max = Vec3::new(
            cell.bounds_max.x.min(plane.bounds_max.x),
            plane.h,
            cell.bounds_max.z.min(plane.bounds_max.z),
        );
        if r.is_some_and(|r| eye.distance_squared(eye.clamp(water_min, water_max)) > r * r) {
            continue;
        }
        output.push(id);
    }
    output.sort_unstable();
    output.dedup();
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct MirrorCamera {
    pub eye: Vec3,
    pub cull_vp: Mat4,
    pub raster_vp: Mat4,
    /// World-space inward clip half-space: dot(plane, vec4(p, 1)) >= 0.
    pub clip_plane: Vec4,
    pub h: f32,
}

impl MirrorCamera {
    /// `projection` is the main camera's finite RH DirectX perspective.
    /// Cull depth uses D; raster depth keeps the main far distance.
    pub fn new(eye: Vec3, view: Mat4, projection: Mat4, h: f32, d: Option<f32>) -> Option<Self> {
        if !eye.is_finite()
            || !view.is_finite()
            || !projection.is_finite()
            || !h.is_finite()
            || eye.y - h <= FLUID_FOG_BAND
        {
            return None;
        }
        let reflection = Mat4::from_cols(
            Vec4::X,
            -Vec4::Y,
            Vec4::Z,
            Vec4::new(0.0, 2.0 * h, 0.0, 1.0),
        );
        let mirror_view = view * reflection;
        let clip_plane = Vec4::new(0.0, 1.0, 0.0, -(h - PLANAR_CLIP_BIAS));
        let camera_plane = mirror_view.inverse().transpose() * clip_plane;
        // Lengyel's oblique near plane: for [0,1] depth it replaces row 2.
        let corner = Vec4::new(
            if camera_plane.x >= 0.0 { 1.0 } else { -1.0 },
            if camera_plane.y >= 0.0 { 1.0 } else { -1.0 },
            1.0,
            1.0,
        );
        let denominator = camera_plane.dot(projection.inverse() * corner);
        if !denominator.is_finite() || denominator <= 1.0e-6 {
            return None;
        }
        let near_row = camera_plane / denominator;
        let mut oblique = projection;
        oblique.x_axis.z = near_row.x;
        oblique.y_axis.z = near_row.y;
        oblique.z_axis.z = near_row.z;
        oblique.w_axis.z = near_row.w;
        let mut cull_projection = projection;
        if let Some(d) = d.filter(|d| d.is_finite() && *d > 0.0) {
            let near = projection.w_axis.z / projection.z_axis.z;
            let far = d.max(near + 0.001);
            cull_projection.z_axis.z = far / (near - far);
            cull_projection.w_axis.z = near * far / (near - far);
        }
        // The mirror flips handedness; flipping x restores the main winding.
        let flip_x = Mat4::from_diagonal(Vec4::new(-1.0, 1.0, 1.0, 1.0));
        Some(Self {
            eye: reflection.transform_point3(eye),
            cull_vp: cull_projection * mirror_view,
            raster_vp: flip_x * oblique * mirror_view,
            clip_plane,
            h,
        })
    }

    /// The cull frustum's six inward planes (left, right, bottom, top, near,
    /// far) as `dot(plane, vec4(p, 1)) >= 0`, for [0, 1] depth. Unnormalized
    /// and allocation-free, for per-leaf tests on the frame path.
    pub fn cull_planes(&self) -> [Vec4; 6] {
        let m = self.cull_vp;
        let (r0, r1, r2, r3) = (m.row(0), m.row(1), m.row(2), m.row(3));
        [r3 + r0, r3 - r0, r3 + r1, r3 - r1, r2, r3 - r2]
    }
}

/// Whether an AABB reaches inside every plane of `planes` (the p-vertex
/// test): conservative, so a box straddling a plane passes.
pub fn aabb_in_planes(min: Vec3, max: Vec3, planes: &[Vec4; 6]) -> bool {
    planes.iter().all(|plane| {
        let p = Vec3::new(
            if plane.x >= 0.0 { max.x } else { min.x },
            if plane.y >= 0.0 { max.y } else { min.y },
            if plane.z >= 0.0 { max.z } else { min.z },
        );
        plane.truncate().dot(p) + plane.w >= 0.0
    })
}

#[cfg(test)]
#[path = "planar_reflection_tests.rs"]
mod tests;
