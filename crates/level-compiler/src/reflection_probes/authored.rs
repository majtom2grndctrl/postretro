// Authored reflection probes: each box's probe per facing, its capture point and drops, and which reflector faces it binds.
// See: context/lib/resource_management.md §5.1, §5.4

use glam::{DVec3, Vec3};

use crate::map_data::MapReflectionVolume;

use super::capture_point::{PointClassifier, capture_point_over};
use super::facing::Facing;
use super::front_side::served;
use super::placement::{PLANE_TOLERANCE_M, PlacedProbe, ReflectorFace, plane_heights, tied};

/// A box keeping more probes than this warns, naming the box and its count:
/// a box room's floor, ceiling and four walls make six.
pub(crate) const BOX_PROBE_WARNING: usize = 6;

/// The authored probes kept, in map entity order, and which faces (indexed
/// as the faces passed in) they claimed.
pub(crate) struct AuthoredPlacement {
    pub probes: Vec<PlacedProbe>,
    pub claimed: Vec<bool>,
}

/// One authored box's probe for one facing in f64, with its capture point.
struct Candidate<'a> {
    volume: &'a MapReflectionVolume,
    min: DVec3,
    max: DVec3,
    facing: Facing,
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

    fn label(&self) -> String {
        format!("{} probe {}", self.volume.source_label, self.facing)
    }
}

/// Capture points first, one per facing of the faces inside each box, each
/// fixed from those faces before any binding, then the drops, then binding
/// among the kept probes. A face binds the probe of its facing in the
/// smallest kept box holding its area centroid; volumes within
/// [`super::placement::TIE_RELATIVE_TOLERANCE`] tie, going to the earlier
/// box in map entity order. A dropped probe's faces bind as if its box were
/// absent. A kept probe that binds nothing drops too: it claimed nothing, so
/// nothing rebinds. A probe binding world faces then keeps only the faces
/// whose front its capture point lies on (`front_side`); the rest bind
/// nothing and stay claimed. Water alone in a box places as it always has.
pub(crate) fn place_authored(
    faces: &[ReflectorFace],
    authored: &[MapReflectionVolume],
    classifier: &PointClassifier<'_>,
) -> anyhow::Result<AuthoredPlacement> {
    let world_offered = faces.iter().any(|face| face.world_face().is_some());
    let mut kept: Vec<Candidate<'_>> = Vec::new();
    for volume in authored {
        let min = DVec3::from(volume.min.map(f64::from));
        let max = DVec3::from(volume.max.map(f64::from));
        let contains = |p: DVec3| p.cmpge(min).all() && p.cmple(max).all();
        let inside: Vec<&ReflectorFace> = faces
            .iter()
            .filter(|face| contains(face.geometry.centroid))
            .collect();
        if inside.is_empty() {
            warn_binds_no_face(volume, world_offered);
            continue;
        }
        let mut facings: Vec<Facing> = Vec::new();
        for face in &inside {
            if !facings.contains(&face.facing) {
                facings.push(face.facing);
            }
        }
        for facing in facings {
            let mut candidate = Candidate {
                volume,
                min,
                max,
                facing,
                capture: DVec3::ZERO,
            };
            let facing_faces: Vec<&ReflectorFace> = inside
                .iter()
                .copied()
                .filter(|face| face.facing == facing)
                .collect();
            let (raw, plane_faces) = authored_capture_point(&candidate, &facing_faces)?;
            let box_min = Vec3::from(volume.min);
            let box_max = Vec3::from(volume.max);
            match classifier.settle(raw, box_min, box_max, &plane_faces) {
                Ok(point) => candidate.capture = point.as_dvec3(),
                Err((point, fault)) => {
                    // Water's message, unchanged where water alone places.
                    let what = if facing.is_up() {
                        volume.source_label.clone()
                    } else {
                        candidate.label()
                    };
                    log::warn!(
                        "[ReflectionProbes] {what} capture point {:?} is {fault}; dropped",
                        point.to_array(),
                    );
                    continue;
                }
            }
            kept.push(candidate);
        }
    }

    let mut claimed = vec![false; faces.len()];
    let mut bound: Vec<Vec<usize>> = vec![Vec::new(); kept.len()];
    for (index, face) in faces.iter().enumerate() {
        let centroid = face.geometry.centroid;
        let serves = |c: &Candidate<'_>| c.facing == face.facing && c.contains(centroid);
        let smallest = kept
            .iter()
            .filter(|candidate| serves(candidate))
            .map(|candidate| candidate.volume_m3())
            .fold(f64::INFINITY, f64::min);
        // The earliest box tied with the smallest, in map entity order.
        let slot = kept
            .iter()
            .position(|c| serves(c) && tied(c.volume_m3(), smallest));
        if let Some(slot) = slot {
            claimed[index] = true;
            bound[slot].push(index);
        }
    }

    let mut probes = Vec::new();
    // A box's probes are contiguous in `kept`, one per facing.
    let mut start = 0;
    while start < kept.len() {
        let volume = kept[start].volume;
        let end = start
            + kept[start..].partition_point(|candidate| std::ptr::eq(candidate.volume, volume));
        let first = probes.len();
        if bound[start..end].iter().all(Vec::is_empty) {
            warn_binds_no_face(volume, world_offered);
        }
        for (candidate, members) in kept[start..end].iter().zip(&bound[start..end]) {
            if members.is_empty() {
                if end - start > 1 {
                    log::warn!(
                        "[ReflectionProbes] {} binds no face a smaller box leaves it; dropped",
                        candidate.label()
                    );
                }
                continue;
            }
            let members: Vec<&ReflectorFace> = members.iter().map(|&i| &faces[i]).collect();
            warn_planes(candidate, &members);
            let probe = PlacedProbe {
                source_entity: volume.source_entity,
                authored: true,
                box_min: volume.min,
                box_max: volume.max,
                capture_point: candidate.capture.as_vec3().to_array(),
                facing: candidate.facing,
                bound_area: members.iter().map(|face| face.geometry.area).sum(),
                fluid_faces: members
                    .iter()
                    .filter_map(|face| face.fluid_face())
                    .collect(),
                world_faces: members
                    .iter()
                    .filter_map(|face| face.world_face())
                    .collect(),
            };
            probes.extend(served(probe, &members, &candidate.label()));
        }
        let count = probes.len() - first;
        if count > BOX_PROBE_WARNING {
            log::warn!(
                "[ReflectionProbes] {} keeps {count} probes, one per facing of the faces it \
                 binds, past {BOX_PROBE_WARNING}; each takes a probe slot, so split the box or \
                 leave some faces to derived probes",
                volume.source_label,
            );
        }
        start = end;
    }
    Ok(AuthoredPlacement { probes, claimed })
}

/// The binding-no-face warning: a box over no reflector face, and a kept box
/// every face of which a smaller box claimed. Water's message is unchanged
/// on a level that offers no glossy world face.
fn warn_binds_no_face(volume: &MapReflectionVolume, world_offered: bool) {
    let what = if world_offered {
        "up-facing fluid face or glossy world face"
    } else {
        "up-facing fluid face"
    };
    log::warn!(
        "[ReflectionProbes] {} binds no {what}; dropped",
        volume.source_label
    );
}

/// The several-planes warning: water's names the heights it binds and
/// captures over the lowest; a probe binding world faces names the planes'
/// heights along its normal and captures in front of the frontmost.
fn warn_planes(candidate: &Candidate<'_>, members: &[&ReflectorFace]) {
    let heights = plane_heights(members.iter().map(|face| face.plane_height()));
    if heights.len() <= 1 {
        return;
    }
    let named: Vec<String> = heights.iter().map(|h| format!("{h:.3}")).collect();
    if members.iter().all(|face| face.world_face().is_none()) {
        log::warn!(
            "[ReflectionProbes] {} binds water at more than one height ({} m); it captures \
             above the lowest water inside it",
            candidate.volume.source_label,
            named.join(", "),
        );
    } else {
        log::warn!(
            "[ReflectionProbes] {} binds faces on more than one plane ({} m along its normal); \
             it captures in front of the frontmost inside it",
            candidate.label(),
            named.join(", "),
        );
    }
}

/// The in-plane area centroid of the inside faces of the candidate's facing
/// on one plane, 2 mm off that plane along the facing's normal, moved onto
/// them when off them, clamped to the box, then shifted by the authored
/// offset. That plane is water's lowest, as it always was, or, once the
/// faces include a world face, the frontmost along the normal, so the point
/// lies in front of every face it binds. The clamp comes first: it absorbs a
/// nearest-point move past the wall of a straddling face, and a box whose
/// top sits within the lift of its water, so an offset of `"0 0"` always
/// holds. The stored offset `(x, 0, z)` moves the point within the facing's
/// plane: unchanged on a floor or a ceiling, along the wall on a wall. An
/// offset that then carries the point out of the box is a build error naming
/// the box and its facing: box projection needs the point inside its box.
/// Returned with the faces on that plane, which the point stays over when it
/// moves clear of a wall.
fn authored_capture_point<'f>(
    candidate: &Candidate<'_>,
    inside: &[&'f ReflectorFace],
) -> anyhow::Result<(DVec3, Vec<&'f ReflectorFace>)> {
    let frontmost = inside.iter().any(|face| face.world_face().is_some());
    let heights = inside.iter().map(|face| face.plane_height());
    let plane = if frontmost {
        heights.fold(f64::NEG_INFINITY, f64::max)
    } else {
        heights.fold(f64::INFINITY, f64::min)
    };
    let plane_faces: Vec<&ReflectorFace> = inside
        .iter()
        .copied()
        .filter(|face| {
            let behind = if frontmost {
                plane - face.plane_height()
            } else {
                face.plane_height() - plane
            };
            behind < PLANE_TOLERANCE_M
        })
        .collect();
    let (lo, hi) = (candidate.min, candidate.max);
    let point = capture_point_over(&plane_faces, plane).clamp(lo, hi);
    let Some([x, z]) = candidate.volume.capture_offset else {
        return Ok((point, plane_faces));
    };
    let stored = DVec3::new(f64::from(x), 0.0, f64::from(z));
    let offset = if candidate.facing.is_up() {
        point + stored
    } else {
        let normal = candidate.facing.normal();
        point + (stored - normal * stored.dot(normal))
    };
    let outside = if candidate.facing.is_up() {
        offset.x < lo.x || offset.x > hi.x || offset.z < lo.z || offset.z > hi.z
    } else {
        offset.cmplt(lo).any() || offset.cmpgt(hi).any()
    };
    if outside {
        let what = if candidate.facing.is_up() {
            candidate.volume.source_label.clone()
        } else {
            candidate.label()
        };
        anyhow::bail!(
            "{what} `_capture_offset` moves its capture point to {:?}, outside its box {:?}..{:?}",
            offset.as_vec3().to_array(),
            candidate.volume.min,
            candidate.volume.max,
        );
    }
    Ok((offset, plane_faces))
}
