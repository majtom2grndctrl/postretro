// Planar reflection candidates: install-time water planes, grouped from up-facing fluid faces by height.
// See: context/lib/rendering_pipeline.md §7.3a

use glam::Vec3;
use postretro_level_format::environment_volumes::EnvironmentVolumeRecord;
use postretro_level_format::reflection_probes::ReflectionProbesSection;

use super::{PLANAR_MIN_PLANE_AREA, PLANE_HEIGHT_TOLERANCE, PLANE_UP_NORMAL_MIN};
use crate::fluid_surface::{face_area_centroid, face_normal, reflection::usable_probe_section};

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
    /// Union of the faces' bounds: a cheap reject only. Anything that asks
    /// whether a point or cell holds this plane's water tests `faces`.
    pub bounds_min: Vec3,
    pub bounds_max: Vec3,
    /// Total area of `faces`, in m²: what the area floor reads.
    pub area: f32,
    pub faces: Vec<PlanarFace>,
}

#[derive(Debug, Clone, Default)]
pub struct PlanarCandidates {
    pub planes: Vec<PlanarCandidate>,
}

impl PlanarCandidates {
    /// The install-time candidate planes: flat up-facing faces of
    /// `reflecting_volume_indices` (the volumes whose fluid reflects) that
    /// bind a probe in `probes`, grouped by height. Planar keeps the
    /// reflecting gate's probe requirement per face: a face with no bound
    /// probe adds no area and no score, and a plane with none never forms.
    /// Planes under [`PLANAR_MIN_PLANE_AREA`] are dropped. Ascending by
    /// height, so a candidate's index is stable for one install.
    pub fn from_volumes(
        volumes: &[EnvironmentVolumeRecord],
        reflecting_volume_indices: &[u32],
        probes: Option<&ReflectionProbesSection>,
    ) -> Self {
        let Some(probes) = usable_probe_section(probes) else {
            return Self::default();
        };
        let mut planes = group_planes(volumes, |volume, face| {
            reflecting_volume_indices.contains(&volume) && face_binds_probe(probes, volume, face)
        });
        planes.retain(|plane| plane.area >= PLANAR_MIN_PLANE_AREA);
        Self { planes }
    }
}

/// Whether surface face `face` of volume `volume` binds a probe the section
/// holds. Bindings ascend by (volume, face), so this is one binary search.
pub(super) fn face_binds_probe(section: &ReflectionProbesSection, volume: u32, face: u32) -> bool {
    section
        .bindings
        .binary_search_by(|binding| (binding.volume, binding.face).cmp(&(volume, face)))
        .is_ok_and(|index| (section.bindings[index].probe as usize) < section.probes.len())
}

/// Every flat up-facing face `keep` accepts, grouped into planes by height,
/// before the area floor. The floor's measurement
/// (`planar_plane_areas_on_baked_maps`) reads it too.
pub(super) fn group_planes(
    volumes: &[EnvironmentVolumeRecord],
    mut keep: impl FnMut(u32, u32) -> bool,
) -> Vec<PlanarCandidate> {
    let mut faces = Vec::new();
    for (volume_index, volume) in volumes.iter().enumerate() {
        for (face_index, face) in volume.surface_faces.iter().enumerate() {
            if !keep(volume_index as u32, face_index as u32) {
                continue;
            }
            let Some(normal) = face_normal(face).filter(|n| n.y > PLANE_UP_NORMAL_MIN) else {
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
            plane.area += face.area;
            plane.faces.push(face);
        } else {
            planes.push(PlanarCandidate {
                h: face.centroid.y,
                bounds_min: face.bounds_min,
                bounds_max: face.bounds_max,
                area: face.area,
                faces: vec![face],
            });
        }
    }
    planes
}
