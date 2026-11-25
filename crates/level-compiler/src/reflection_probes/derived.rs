// Derived reflection probes: one per group of unclaimed reflector faces of one facing and plane whose fitted boxes match.
// See: context/lib/resource_management.md §5.1, §5.4

use glam::DVec3;
use postretro_level_format::environment_volumes::EnvironmentVolumesSection;
use rayon::prelude::*;

use super::box_fit::{fit_box, fit_rays};
use super::capture_point::capture_point_over;
use super::facing::Facing;
use super::front_side::served;
use super::placement::{
    CAPTURE_LIFT_M, PlacedProbe, PlacementScene, ReflectorFace, ReflectorSource,
    WORLD_SOURCE_ENTITY, plane_heights, plane_of,
};

/// Two faces' fitted boxes match when each of their six bounds agrees within
/// this distance. A face's box is its room's shell, which the per-azimuth
/// wall vote lands on within its 1 cm plane merge, so faces in one room match
/// with room to spare; rooms on either side of a wall differ by the room's
/// extent, a metre or more, so they never do.
pub(crate) const FIT_MATCH_TOLERANCE_M: f64 = 0.25;

type FittedBox = (DVec3, DVec3);

/// Derived probes for `faces`: the unclaimed fluid faces in id-52 order, or
/// the unclaimed glossy world faces in id-17 order. Water's probes come in
/// fluid entity order, then by each group's first face; the static world is
/// one source. Each face fits its own box from its own area centroid, lifted
/// along its facing's normal; a group is a connected component of the
/// box-match relation among one source's faces of one facing at one plane
/// height along it, so face order never changes it, faces on different
/// planes or facing opposite ways never share, and a matte strip or a
/// non-glossy surround splitting a floor or a wall into pieces never splits
/// its probe: probe boundaries belong behind occluders, not in open floor.
/// The group stores the union box (grown to hold its faces, for any facing
/// but up) and captures from its in-plane area centroid, 2 mm off its plane
/// along the normal, moved clear of a wall within
/// [`super::capture_point::CAPTURE_CLEARANCE_M`]. A probe whose point is in
/// solid, inside any fluid or outside the hull drops, warned with its
/// source; its faces bind nothing.
pub(crate) fn derive_probes(
    faces: &[&ReflectorFace],
    volumes: &EnvironmentVolumesSection,
    scene: &PlacementScene<'_>,
) -> Vec<PlacedProbe> {
    let rays = fit_rays();
    // Fits are pure and independent; collected in face order, so the result
    // never depends on the worker count.
    let boxes: Vec<FittedBox> = faces
        .par_iter()
        .map(|face| {
            let _permit = scene.governor.enter();
            let origin = face.geometry.centroid + face.facing.normal() * CAPTURE_LIFT_M;
            fit_box(
                scene.tracer,
                origin,
                scene.bounds,
                &rays,
                face.facing.fit_frame(),
            )
        })
        .collect();

    let mut probes = Vec::new();
    let mut start = 0;
    while start < faces.len() {
        let source = probe_source(faces[start]);
        let end = start + faces[start..].partition_point(|f| probe_source(f) == source);
        let source_entity = match source {
            Some(volume) => volumes.volumes[volume as usize].source_entity,
            None => WORLD_SOURCE_ENTITY,
        };
        let mut facings: Vec<Facing> = Vec::new();
        for face in &faces[start..end] {
            if !facings.contains(&face.facing) {
                facings.push(face.facing);
            }
        }
        let mut groups: Vec<Vec<usize>> = Vec::new();
        for facing in facings {
            let facing_faces: Vec<usize> = (start..end)
                .filter(|&i| faces[i].facing == facing)
                .collect();
            let heights = plane_heights(facing_faces.iter().map(|&i| faces[i].plane_height()));
            for &plane in &heights {
                let on_plane: Vec<usize> = facing_faces
                    .iter()
                    .copied()
                    .filter(|&i| plane_of(&heights, faces[i].plane_height()) == plane)
                    .collect();
                let plane_boxes: Vec<FittedBox> = on_plane.iter().map(|&i| boxes[i]).collect();
                for component in box_match_components(&plane_boxes) {
                    groups.push(component.into_iter().map(|k| on_plane[k]).collect());
                }
            }
        }
        groups.sort_by_key(|group| group[0]);
        for group in groups {
            if let Some(probe) = group_probe(source_entity, faces, &boxes, &group, scene) {
                probes.push(probe);
            }
        }
        start = end;
    }
    probes
}

/// The source a face's derived probe is grouped under: its fluid volume, or
/// `None` for the static world.
fn probe_source(face: &ReflectorFace) -> Option<u32> {
    match face.source {
        ReflectorSource::Fluid { volume, .. } => Some(volume),
        ReflectorSource::World { .. } => None,
    }
}

fn group_probe(
    source_entity: u32,
    faces: &[&ReflectorFace],
    boxes: &[FittedBox],
    group: &[usize],
    scene: &PlacementScene<'_>,
) -> Option<PlacedProbe> {
    let members: Vec<&ReflectorFace> = group.iter().map(|&i| faces[i]).collect();
    let facing = members[0].facing;
    let (mut lo, mut hi) = boxes[group[0]];
    for &i in group {
        lo = lo.min(boxes[i].0);
        hi = hi.max(boxes[i].1);
    }
    // A fitted box's floor is its fit origin's plane, which holds a face of
    // a fit-frame axis (water, a wall, a ceiling) within the lift; a slope
    // fits along its nearest axis and leans out of it. Box projection needs
    // every bound fragment inside the box. Water's box is as it always was.
    if !facing.is_up() {
        for member in &members {
            lo = lo.min(member.min);
            hi = hi.max(member.max);
        }
    }
    // Water captures over its lowest plane, as it always has; a world group
    // in front of its frontmost, so the point serves every face it binds.
    let heights = members.iter().map(|m| m.plane_height());
    let plane = if members[0].world_face().is_some() {
        heights.fold(f64::NEG_INFINITY, f64::max)
    } else {
        heights.fold(f64::INFINITY, f64::min)
    };
    let box_min = lo.as_vec3();
    let box_max = hi.as_vec3();
    // Each member's centroid lies in its own box, so the group's lies in the
    // union; a nearest-point move stays on a member's water. The clamp in
    // `settle` only absorbs f32 rounding at a box face; a point outside the
    // hull, which a hull-clamped box cannot hold, drops on its own fault
    // before it.
    let raw = capture_point_over(&members, plane);
    let capture_point = match scene.classifier.settle(raw, box_min, box_max, &members) {
        Ok(point) => point,
        Err((point, fault)) => {
            match members[0].world_face() {
                None => log::warn!(
                    "[ReflectionProbes] derived probe of fluid entity {source_entity}: capture \
                     point {:?} is {fault}; dropped, its faces reflect nothing",
                    point.to_array(),
                ),
                Some(face) => log::warn!(
                    "[ReflectionProbes] derived probe of glossy world face {face} and {} more: \
                     capture point {:?} is {fault}; dropped, its faces reflect nothing",
                    members.len() - 1,
                    point.to_array(),
                ),
            }
            return None;
        }
    };
    let probe = PlacedProbe {
        source_entity,
        authored: false,
        box_min: box_min.to_array(),
        box_max: box_max.to_array(),
        capture_point: capture_point.to_array(),
        facing,
        bound_area: members.iter().map(|m| m.geometry.area).sum(),
        fluid_faces: members.iter().filter_map(|m| m.fluid_face()).collect(),
        world_faces: members.iter().filter_map(|m| m.world_face()).collect(),
    };
    // A group lies on one plane and the settle moves only in it, so its
    // capture point serves every face but one tilted against the facing; the
    // rule holds every world probe as it holds authored ones.
    let label = match probe.world_faces.first() {
        Some(face) => format!("derived probe of glossy world face {face}"),
        None => format!("derived probe of fluid entity {source_entity}"),
    };
    served(probe, &members, &label)
}

/// Connected components of the box-match relation (every bound within
/// [`FIT_MATCH_TOLERANCE_M`]), as ascending indices into `boxes`, ordered by
/// first member. Matching does not chain, so a component can hold boxes that
/// differ by more than the tolerance; components, unlike first-match
/// grouping, do not depend on order.
pub(crate) fn box_match_components(boxes: &[FittedBox]) -> Vec<Vec<usize>> {
    let mut parent: Vec<usize> = (0..boxes.len()).collect();
    fn root(parent: &mut [usize], mut i: usize) -> usize {
        while parent[i] != i {
            parent[i] = parent[parent[i]];
            i = parent[i];
        }
        i
    }
    for a in 0..boxes.len() {
        for b in a + 1..boxes.len() {
            if boxes_match(boxes[a], boxes[b]) {
                let (ra, rb) = (root(&mut parent, a), root(&mut parent, b));
                // The lower index roots, so a component's root is its first
                // member.
                parent[ra.max(rb)] = ra.min(rb);
            }
        }
    }
    let mut components: Vec<Vec<usize>> = Vec::new();
    let mut slot_of_root = vec![usize::MAX; boxes.len()];
    for i in 0..boxes.len() {
        let r = root(&mut parent, i);
        if slot_of_root[r] == usize::MAX {
            slot_of_root[r] = components.len();
            components.push(Vec::new());
        }
        components[slot_of_root[r]].push(i);
    }
    components
}

fn boxes_match(a: FittedBox, b: FittedBox) -> bool {
    (a.0 - b.0).abs().max_element() <= FIT_MATCH_TOLERANCE_M
        && (a.1 - b.1).abs().max_element() <= FIT_MATCH_TOLERANCE_M
}

#[cfg(test)]
mod tests {
    use super::*;

    fn unit_box(x: f64) -> FittedBox {
        (DVec3::new(x, 0.0, 0.0), DVec3::new(x + 4.0, 3.0, 4.0))
    }

    // P31: A matches B, B matches C, A does not match C: one group of all
    // three, whatever the face order.
    #[test]
    fn a_match_chain_is_one_group_in_every_face_order() {
        let step = FIT_MATCH_TOLERANCE_M * 0.75;
        let (a, b, c) = (unit_box(0.0), unit_box(step), unit_box(2.0 * step));
        assert!(boxes_match(a, b) && boxes_match(b, c) && !boxes_match(a, c));
        let far = unit_box(10.0);
        for order in [
            [a, b, c, far],
            [a, c, b, far],
            [b, a, c, far],
            [b, c, a, far],
            [c, a, b, far],
            [c, b, a, far],
            [far, c, a, b],
        ] {
            let mut groups: Vec<Vec<FittedBox>> = box_match_components(&order)
                .into_iter()
                .map(|group| group.into_iter().map(|i| order[i]).collect())
                .collect();
            groups.sort_by_key(|group| group.len());
            assert_eq!(groups.len(), 2, "{order:?}");
            assert_eq!(groups[0], vec![far]);
            assert_eq!(groups[1].len(), 3);
        }
        // Ordered by first member, members ascending.
        assert_eq!(
            box_match_components(&[far, c, a, b]),
            vec![vec![0], vec![1, 2, 3]]
        );
    }
}
