// Reflection-probe self-transparency: the static faces in a glossy probe's own reflector plane, absent from its primary rays.
// See: context/lib/resource_management.md §5.4 (Placement)

use glam::DVec3;

use crate::geometry::GeometryResult;

use super::placement::{
    PLANE_TOLERANCE_M, PlacedProbe, ReflectorFace, ReflectorSource, UP_FACING_MIN_COS,
};

/// Each static face's plane as its triangles give it.
pub(crate) struct FacePlanes {
    /// Per id-17 face, its unit winding normal and area centroid; `None` for
    /// a face with no triangle or no area.
    planes: Vec<Option<(DVec3, DVec3)>>,
}

impl FacePlanes {
    pub(crate) fn new(geometry: &GeometryResult) -> Self {
        let section = &geometry.geometry;
        let position =
            |index: u32| DVec3::from(section.vertices[index as usize].position.map(f64::from));
        let planes = geometry
            .face_index_ranges
            .iter()
            .map(|range| {
                let start = range.index_offset as usize;
                let indices = section
                    .indices
                    .get(start..start + range.index_count as usize)?;
                let (mut winding, mut weighted, mut area) = (DVec3::ZERO, DVec3::ZERO, 0.0);
                for &[a, b, c] in indices.as_chunks::<3>().0 {
                    let (a, b, c) = (position(a), position(b), position(c));
                    let cross = (b - a).cross(c - a);
                    let twice_area = cross.length();
                    winding += cross;
                    weighted += (a + b + c) / 3.0 * twice_area;
                    area += twice_area;
                }
                let normal = winding.try_normalize()?;
                (area > f64::EPSILON).then(|| (normal, weighted / area))
            })
            .collect();
        Self { planes }
    }

    /// The static faces in the planes of `probe`'s bound world faces, facing
    /// as they do, ascending; empty for a probe binding none. `world_faces`
    /// holds the placed world faces in id-17 order. A probe binding faces on
    /// several planes of its facing (an authored box over two floor heights)
    /// passes through each of them, and through nothing between them.
    ///
    /// The capture point sits 2 mm off that plane, so a face in it fills the
    /// texels at the capture's horizon with hits a few centimetres away, and
    /// the capture-shadow test then withholds the reflector's own fragments
    /// beyond them. Nothing in the plane can show in the reflection, which
    /// leaves the plane, so every face there is transparent, bound or not:
    /// water is likewise absent from every capture, and a flush inlay's
    /// surround or a matte strip crossing a glossy floor casts no capture
    /// shadow either. Faces facing the other way are back faces from the
    /// capture point, which the trace already skips.
    ///
    /// "Facing as they do" is one test against the bound faces' area-weighted
    /// mean normal: a face's normal within 1° of it ([`UP_FACING_MIN_COS`]),
    /// and its centroid within [`PLANE_TOLERANCE_M`] of a bound face's along
    /// it. Faces of one facing lie within 1° of its normal, so bound faces
    /// may differ by up to 2°: a face in a bound face's plane, that bound
    /// face included, can lie more than 1° off the mean, stay opaque and cast
    /// a capture shadow. A known edge, accepted rather than fixed.
    pub(crate) fn reflector_plane_faces(
        &self,
        probe: &PlacedProbe,
        world_faces: &[ReflectorFace],
    ) -> Vec<u32> {
        let bound: Vec<&ReflectorFace> = probe
            .world_faces
            .iter()
            .filter_map(|&face| {
                let source = ReflectorSource::World { face };
                let at = world_faces
                    .binary_search_by_key(&source, |f| f.source)
                    .ok()?;
                Some(&world_faces[at])
            })
            .collect();
        let Some(normal) = bound
            .iter()
            .map(|face| face.geometry.normal * face.geometry.area)
            .sum::<DVec3>()
            .try_normalize()
        else {
            return Vec::new();
        };
        let heights: Vec<f64> = bound
            .iter()
            .map(|face| face.geometry.centroid.dot(normal))
            .collect();
        self.planes
            .iter()
            .enumerate()
            .filter_map(|(face, plane)| {
                let (face_normal, centroid) = (*plane)?;
                let height = centroid.dot(normal);
                (face_normal.dot(normal) >= UP_FACING_MIN_COS
                    && heights
                        .iter()
                        .any(|&bound| (height - bound).abs() < PLANE_TOLERANCE_M))
                .then_some(face as u32)
            })
            .collect()
    }
}

/// Each probe's transparent faces ([`FacePlanes::reflector_plane_faces`]),
/// parallel to `probes`: empty for a probe binding no world face.
pub(crate) fn transparent_faces(
    geometry: &GeometryResult,
    probes: &[PlacedProbe],
    world_faces: &[ReflectorFace],
) -> Vec<Vec<u32>> {
    if probes.iter().all(|probe| probe.world_faces.is_empty()) {
        return vec![Vec::new(); probes.len()];
    }
    let planes = FacePlanes::new(geometry);
    probes
        .iter()
        .map(|probe| planes.reflector_plane_faces(probe, world_faces))
        .collect()
}
