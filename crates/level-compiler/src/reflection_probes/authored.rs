// Authored reflection probes: each box's capture point, its drops, and which up-facing faces it binds.
// See: context/lib/resource_management.md §5.1

use glam::{DVec3, Vec3};

use crate::map_data::MapReflectionVolume;

use super::capture_point::{PointClassifier, capture_point_over};
use super::placement::{PLANE_TOLERANCE_M, PlacedProbe, UpFacingFace, plane_heights, tied};

/// The authored probes kept, in map entity order, and which faces (indexed
/// as the faces passed in) they claimed.
pub(crate) struct AuthoredPlacement {
    pub probes: Vec<PlacedProbe>,
    pub claimed: Vec<bool>,
}

/// One authored box in f64, with its probe's capture point once it has one.
struct Candidate<'a> {
    volume: &'a MapReflectionVolume,
    min: DVec3,
    max: DVec3,
    capture: DVec3,
}

impl Candidate<'_> {
    /// Inclusive, so a centroid on the face two boxes share lies in both.
    fn contains(&self, point: DVec3) -> bool {
        point.cmpge(self.min).all() && point.cmple(self.max).all()
    }

    fn volume_m3(&self) -> f64 {
        (self.max - self.min).element_product()
    }
}

/// Capture points first, each fixed from the faces inside its box before any
/// binding, then the drops, then binding among the kept boxes. A face binds
/// the smallest kept box holding its area centroid; volumes within
/// [`super::placement::TIE_RELATIVE_TOLERANCE`] tie, going to the earlier box
/// in map entity order. A dropped box's faces bind as if it were absent. A
/// kept box that binds nothing drops too: it claimed nothing, so nothing
/// rebinds.
pub(crate) fn place_authored(
    faces: &[UpFacingFace],
    authored: &[MapReflectionVolume],
    classifier: &PointClassifier<'_>,
) -> anyhow::Result<AuthoredPlacement> {
    let mut kept: Vec<Candidate<'_>> = Vec::new();
    for volume in authored {
        let min = DVec3::from(volume.min.map(f64::from));
        let max = DVec3::from(volume.max.map(f64::from));
        let mut candidate = Candidate {
            volume,
            min,
            max,
            capture: DVec3::ZERO,
        };
        let inside: Vec<&UpFacingFace> = faces
            .iter()
            .filter(|face| candidate.contains(face.geometry.centroid))
            .collect();
        if inside.is_empty() {
            warn_binds_no_face(volume);
            continue;
        }
        let (raw, water) = authored_capture_point(&candidate, &inside)?;
        let box_min = Vec3::from(volume.min);
        let box_max = Vec3::from(volume.max);
        match classifier.settle(raw, box_min, box_max, &water) {
            Ok(point) => candidate.capture = point.as_dvec3(),
            Err((point, fault)) => {
                log::warn!(
                    "[ReflectionProbes] {} capture point {:?} is {fault}; dropped",
                    volume.source_label,
                    point.to_array(),
                );
                continue;
            }
        }
        kept.push(candidate);
    }

    let mut claimed = vec![false; faces.len()];
    let mut bound: Vec<Vec<usize>> = vec![Vec::new(); kept.len()];
    for (index, face) in faces.iter().enumerate() {
        let centroid = face.geometry.centroid;
        let smallest = kept
            .iter()
            .filter(|candidate| candidate.contains(centroid))
            .map(|candidate| candidate.volume_m3())
            .fold(f64::INFINITY, f64::min);
        // The earliest box tied with the smallest, in map entity order.
        let slot = kept
            .iter()
            .position(|c| c.contains(centroid) && tied(c.volume_m3(), smallest));
        if let Some(slot) = slot {
            claimed[index] = true;
            bound[slot].push(index);
        }
    }

    let mut probes = Vec::new();
    for (candidate, members) in kept.iter().zip(&bound) {
        if members.is_empty() {
            warn_binds_no_face(candidate.volume);
            continue;
        }
        let heights = plane_heights(members.iter().map(|&i| faces[i].geometry.plane_height()));
        if heights.len() > 1 {
            let named: Vec<String> = heights.iter().map(|h| format!("{h:.3}")).collect();
            log::warn!(
                "[ReflectionProbes] {} binds water at more than one height ({} m); it captures \
                 above the lowest water inside it",
                candidate.volume.source_label,
                named.join(", "),
            );
        }
        probes.push(PlacedProbe {
            source_entity: candidate.volume.source_entity,
            authored: true,
            box_min: candidate.volume.min,
            box_max: candidate.volume.max,
            capture_point: candidate.capture.as_vec3().to_array(),
            bound_area: members.iter().map(|&i| faces[i].geometry.area).sum(),
            faces: members
                .iter()
                .map(|&i| (faces[i].volume, faces[i].face))
                .collect(),
        });
    }
    Ok(AuthoredPlacement { probes, claimed })
}

/// The binding-no-face warning: a box over no water, and a kept box every
/// face of which a smaller box claimed.
fn warn_binds_no_face(volume: &MapReflectionVolume) {
    log::warn!(
        "[ReflectionProbes] {} binds no up-facing fluid face; dropped",
        volume.source_label
    );
}

/// The horizontal area centroid of the inside faces on the lowest plane, 2 mm
/// above that plane, moved onto their water when off it, clamped to the box,
/// then shifted by the authored offset. The clamp comes first: it absorbs a
/// nearest-point move past the wall of a straddling face, and a box whose top
/// sits within the lift of its water, so an offset of `"0 0"` always holds.
/// An offset that then carries the point out of the box is a build error:
/// box projection needs the point inside its box. Returned with the
/// lowest-plane faces, the water the point stays over when it moves clear of
/// a wall.
fn authored_capture_point<'f>(
    candidate: &Candidate<'_>,
    inside: &[&'f UpFacingFace],
) -> anyhow::Result<(DVec3, Vec<&'f UpFacingFace>)> {
    let lowest = inside
        .iter()
        .map(|face| face.geometry.plane_height())
        .fold(f64::INFINITY, f64::min);
    let lowest_faces: Vec<&UpFacingFace> = inside
        .iter()
        .copied()
        .filter(|face| face.geometry.plane_height() - lowest < PLANE_TOLERANCE_M)
        .collect();
    let (lo, hi) = (candidate.min, candidate.max);
    let point = capture_point_over(&lowest_faces, lowest).clamp(lo, hi);
    let Some([x, z]) = candidate.volume.capture_offset else {
        return Ok((point, lowest_faces));
    };
    let offset = point + DVec3::new(f64::from(x), 0.0, f64::from(z));
    if offset.x < lo.x || offset.x > hi.x || offset.z < lo.z || offset.z > hi.z {
        anyhow::bail!(
            "{} `_capture_offset` moves its capture point to {:?}, outside its box {:?}..{:?}",
            candidate.volume.source_label,
            offset.as_vec3().to_array(),
            candidate.volume.min,
            candidate.volume.max,
        );
    }
    Ok((offset, lowest_faces))
}
