// Reflection-probe front-side rule: a probe binding world faces serves only faces whose front its capture point lies on.
// See: context/lib/resource_management.md §5.4 (Placement)

use super::placement::{CAPTURE_LIFT_M, PlacedProbe, ReflectorFace, ReflectorSource};

/// A capture point serves a face when it lies at least this far in front of
/// the face's plane, along the face's own normal: half the lift, so a point
/// lifted off the face passes through f32 rounding and one on its plane, or
/// behind it, does not.
const FRONT_SIDE_MIN_M: f64 = CAPTURE_LIFT_M * 0.5;

/// The faces among `members` (the faces `probe` binds) whose front side its
/// capture point is not on, in `members`' order. A water-only probe serves
/// every face, as it always has: water is absent from every capture, and its
/// capture point sits over the lowest water it binds.
pub(crate) fn unserved_faces(
    probe: &PlacedProbe,
    members: &[&ReflectorFace],
) -> Vec<ReflectorSource> {
    if probe.world_faces.is_empty() {
        return Vec::new();
    }
    let capture = probe.capture_point_vec3().as_dvec3();
    members
        .iter()
        .filter(|face| {
            let g = &face.geometry;
            (capture - g.centroid).dot(g.normal) < FRONT_SIDE_MIN_M
        })
        .map(|face| face.source)
        .collect()
}

/// `probe` without the faces among `members` its capture point does not
/// serve, each named in one warning under `label`; `None` when none is left.
pub(crate) fn served(
    mut probe: PlacedProbe,
    members: &[&ReflectorFace],
    label: &str,
) -> Option<PlacedProbe> {
    let unserved = unserved_faces(&probe, members);
    if unserved.is_empty() {
        return Some(probe);
    }
    let named: Vec<String> = unserved.iter().map(describe).collect();
    log::warn!(
        "[ReflectionProbes] {label}: capture point {:?} is not in front of {}; they bind \
         nothing",
        probe.capture_point,
        named.join(", "),
    );
    let kept: Vec<&&ReflectorFace> = members
        .iter()
        .filter(|face| !unserved.contains(&face.source))
        .collect();
    probe.bound_area = kept.iter().map(|face| face.geometry.area).sum();
    probe.fluid_faces = kept.iter().filter_map(|face| face.fluid_face()).collect();
    probe.world_faces = kept.iter().filter_map(|face| face.world_face()).collect();
    if kept.is_empty() {
        log::warn!("[ReflectionProbes] {label} serves no face it binds; dropped");
        return None;
    }
    Some(probe)
}

/// A reflector face as the placement warnings name it.
pub(super) fn describe(source: &ReflectorSource) -> String {
    match source {
        ReflectorSource::Fluid { volume, face } => {
            format!("fluid face {face} of volume {volume}")
        }
        ReflectorSource::World { face } => format!("glossy world face {face}"),
    }
}
