// Authored reflection probes: each box's probe per facing, its capture point and drops, and which reflector faces it binds.
// See: context/lib/resource_management.md §5.1, §5.4

use glam::{DVec3, Vec3};

use crate::map_data::MapReflectionVolume;

use super::capture_point::{PointClassifier, PointFault, capture_point_over};
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

/// One authored box's probe for one facing in f64: the faces of that facing
/// inside the box and, once settled, its capture point.
struct Candidate<'a> {
    volume: &'a MapReflectionVolume,
    min: DVec3,
    max: DVec3,
    facing: Facing,
    /// The faces of `facing` whose area centroid lies in the box, in face
    /// order.
    inside: Vec<&'a ReflectorFace>,
    capture: Option<DVec3>,
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

    /// Whether the box holds no world face of its facing: its probe can
    /// bind only water, so its point is fixed before any binding, as water's
    /// always was.
    fn water_only(&self) -> bool {
        self.inside.iter().all(|face| face.world_face().is_none())
    }
}

/// A settled capture point, or the point and fault that drop the probe.
type Settled = Result<DVec3, (Vec3, PointFault)>;

/// Capture points, drops and binding. A face binds the probe of its facing
/// in the smallest kept box holding its area centroid; volumes within
/// [`super::placement::TIE_RELATIVE_TOLERANCE`] tie, going to the earlier
/// box in map entity order. A dropped probe's faces bind as if its box were
/// absent.
///
/// A box holding only water of a facing fixes that probe's point first,
/// from all that water, and drops it before any binding, as water always
/// placed. A box holding a world face of the facing fixes the point from
/// what its probe binds ([`point_faces`]), so binding comes first; when that
/// point drops, its faces rebind and the points are fixed again. The
/// smallest such box drops first: every other box its faces could rebind to
/// is no smaller, so no smaller box's binding or point moves.
///
/// A kept probe that binds nothing drops too: it claimed nothing, so nothing
/// rebinds. A probe binding world faces then keeps only the faces whose
/// front its capture point lies on (`front_side`); the rest bind nothing and
/// stay claimed.
pub(crate) fn place_authored<'a>(
    faces: &'a [ReflectorFace],
    authored: &'a [MapReflectionVolume],
    classifier: &PointClassifier<'_>,
) -> anyhow::Result<AuthoredPlacement> {
    let world_offered = faces.iter().any(|face| face.world_face().is_some());
    let mut kept: Vec<Candidate<'a>> = Vec::new();
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
                inside: inside
                    .iter()
                    .copied()
                    .filter(|face| face.facing == facing)
                    .collect(),
                capture: None,
            };
            if candidate.water_only() {
                let (water, frontmost) = point_faces(&candidate, &[]);
                match settle(&candidate, &water, frontmost, classifier)? {
                    Ok(point) => candidate.capture = Some(point),
                    Err((point, fault)) => {
                        warn_dropped(&candidate, point, fault);
                        continue;
                    }
                }
            }
            kept.push(candidate);
        }
    }

    let bound = loop {
        let bound = bind(faces, &kept);
        let settled: Vec<Option<anyhow::Result<Settled>>> = kept
            .iter()
            .zip(&bound)
            .map(|(candidate, members)| {
                (!candidate.water_only() && !members.is_empty()).then(|| {
                    let members: Vec<&'a ReflectorFace> =
                        members.iter().map(|&i| &faces[i]).collect();
                    let (from, frontmost) = point_faces(candidate, &members);
                    settle(candidate, &from, frontmost, classifier)
                })
            })
            .collect();
        // The first of the smallest, in map entity order.
        let smallest_fault = settled
            .iter()
            .enumerate()
            .filter_map(|(slot, settled)| match settled {
                Some(Ok(Err(fault))) => Some((slot, *fault)),
                _ => None,
            })
            .min_by(|a, b| kept[a.0].volume_m3().total_cmp(&kept[b.0].volume_m3()));
        if let Some((slot, (point, fault))) = smallest_fault {
            warn_dropped(&kept[slot], point, fault);
            kept.remove(slot);
            continue;
        }
        for (candidate, settled) in kept.iter_mut().zip(settled) {
            if let Some(settled) = settled
                && let Ok(point) = settled?
            {
                candidate.capture = Some(point);
            }
        }
        break bound;
    };
    let mut claimed = vec![false; faces.len()];
    for &index in bound.iter().flatten() {
        claimed[index] = true;
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
                capture_point: candidate
                    .capture
                    .expect("a probe binding faces has settled its point")
                    .as_vec3()
                    .to_array(),
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

/// Each kept candidate's faces, as indices into `faces`: a face binds the
/// candidate of its facing in the smallest box holding its area centroid,
/// the earliest box tied with the smallest in map entity order.
fn bind(faces: &[ReflectorFace], kept: &[Candidate<'_>]) -> Vec<Vec<usize>> {
    let mut bound: Vec<Vec<usize>> = vec![Vec::new(); kept.len()];
    for (index, face) in faces.iter().enumerate() {
        let centroid = face.geometry.centroid;
        let serves = |c: &Candidate<'_>| c.facing == face.facing && c.contains(centroid);
        let smallest = kept
            .iter()
            .filter(|candidate| serves(candidate))
            .map(|candidate| candidate.volume_m3())
            .fold(f64::INFINITY, f64::min);
        let slot = kept
            .iter()
            .position(|c| serves(c) && tied(c.volume_m3(), smallest));
        if let Some(slot) = slot {
            bound[slot].push(index);
        }
    }
    bound
}

/// The faces a probe's capture point is fixed from, and whether it captures
/// in front of the frontmost of their planes along the normal rather than
/// over the lowest. Once `members`, the faces it binds, include a world
/// face: those faces, frontmost, so the point lies in front of every face it
/// binds and of none it leaves to a smaller box. Else the water of its
/// facing inside its box, bound or not, lowest, as water always captured: it
/// sits over real water even when a smaller box claims that basin.
fn point_faces<'a>(
    candidate: &Candidate<'a>,
    members: &[&'a ReflectorFace],
) -> (Vec<&'a ReflectorFace>, bool) {
    if members.iter().any(|face| face.world_face().is_some()) {
        (members.to_vec(), true)
    } else {
        let water = candidate
            .inside
            .iter()
            .copied()
            .filter(|face| face.world_face().is_none())
            .collect();
        (water, false)
    }
}

/// `candidate`'s capture point from `faces` ([`authored_capture_point`]),
/// settled against the classifier within its box.
fn settle(
    candidate: &Candidate<'_>,
    faces: &[&ReflectorFace],
    frontmost: bool,
    classifier: &PointClassifier<'_>,
) -> anyhow::Result<Settled> {
    let (raw, plane_faces) = authored_capture_point(candidate, faces, frontmost)?;
    let box_min = Vec3::from(candidate.volume.min);
    let box_max = Vec3::from(candidate.volume.max);
    Ok(classifier
        .settle(raw, box_min, box_max, &plane_faces)
        .map(|point| point.as_dvec3()))
}

/// A probe dropped for its capture point. Water's message is unchanged: an
/// up-facing probe names its box alone.
fn warn_dropped(candidate: &Candidate<'_>, point: Vec3, fault: PointFault) {
    let what = if candidate.facing.is_up() {
        candidate.volume.source_label.clone()
    } else {
        candidate.label()
    };
    log::warn!(
        "[ReflectionProbes] {what} capture point {:?} is {fault}; dropped",
        point.to_array(),
    );
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
/// captures over the lowest water in its box; a probe binding world faces
/// names the planes' heights along its normal and captures in front of the
/// frontmost it binds.
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
             it captures in front of the frontmost it binds",
            candidate.label(),
            named.join(", "),
        );
    }
}

/// The in-plane area centroid of `faces` (one facing) on one plane, 2 mm
/// off that plane along the facing's normal, moved onto them when off them,
/// clamped to the box, then shifted by the authored offset. That plane is the
/// frontmost of theirs along the normal when `frontmost`, else the lowest
/// ([`point_faces`] picks). The clamp comes first: it absorbs a
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
    faces: &[&'f ReflectorFace],
    frontmost: bool,
) -> anyhow::Result<(DVec3, Vec<&'f ReflectorFace>)> {
    let heights = faces.iter().map(|face| face.plane_height());
    let plane = if frontmost {
        heights.fold(f64::NEG_INFINITY, f64::max)
    } else {
        heights.fold(f64::INFINITY, f64::min)
    };
    let plane_faces: Vec<&ReflectorFace> = faces
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
        anyhow::bail!(
            "{} `_capture_offset` moves its capture point to {:?}, outside its box {:?}..{:?}",
            candidate.label(),
            offset.as_vec3().to_array(),
            candidate.volume.min,
            candidate.volume.max,
        );
    }
    Ok((offset, plane_faces))
}
