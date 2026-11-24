// Glossy world-face census: reflector faces grouped by plane and edge contact, the probe count they would ask for.
// See: context/lib/resource_management.md §5.1, §5.4

use glam::DVec3;
use postretro_level_format::reflection_probes::MAX_REFLECTION_PROBES;

use crate::geometry::GeometryResult;

use super::glossy_materials::GlossyMaterials;
use super::placement::{PLANE_TOLERANCE_M, UP_FACING_MIN_COS};
use super::world_reflectors::{WorldReflectorFace, world_reflector_faces};

/// Faces sharing an edge within this distance are one group. Brush faces
/// meet exactly; the slack covers f32 vertex rounding.
const EDGE_TOLERANCE_M: f64 = PLANE_TOLERANCE_M;

/// What one probe per coplanar, edge-connected glossy face group would cost.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct GlossyCensus {
    pub used_materials: usize,
    pub glossy_materials: usize,
    pub unreadable_materials: usize,
    pub faces: usize,
    pub unrecovered_faces: usize,
    /// Planes: normals within 1°, offsets within 1 mm.
    pub planes: usize,
    /// Edge-connected groups within each plane: the probe estimate.
    pub groups: usize,
}

/// The glossy world faces and the groups one probe each would serve.
pub(super) struct ReflectorGroups {
    pub faces: Vec<WorldReflectorFace>,
    pub unrecovered: usize,
    /// Indices into `faces`, by plane.
    pub planes: Vec<Vec<usize>>,
    /// Indices into `faces`: each plane's edge-connected groups, planes in
    /// order.
    pub groups: Vec<Vec<usize>>,
}

pub(super) fn group_reflectors(
    geometry: &GeometryResult,
    materials: &GlossyMaterials,
) -> ReflectorGroups {
    let reflectors = world_reflector_faces(geometry, materials);
    let planes = coplanar_planes(&reflectors.faces);
    let groups = planes
        .iter()
        .flat_map(|plane| edge_connected_groups(&reflectors.faces, plane))
        .collect();
    ReflectorGroups {
        faces: reflectors.faces,
        unrecovered: reflectors.unrecovered,
        planes,
        groups,
    }
}

/// The census of `geometry`'s glossy world faces under `materials`' verdicts.
pub(crate) fn take_census(geometry: &GeometryResult, materials: &GlossyMaterials) -> GlossyCensus {
    let grouped = group_reflectors(geometry, materials);
    GlossyCensus {
        used_materials: materials.used,
        glossy_materials: materials.glossy(),
        unreadable_materials: materials.unreadable,
        faces: grouped.faces.len(),
        unrecovered_faces: grouped.unrecovered,
        planes: grouped.planes.len(),
        groups: grouped.groups.len(),
    }
}

/// Logs the census's line under `materials`' verdicts: on every build, the
/// opt-in check whether or not glossy placement runs.
pub(crate) fn report_census(geometry: &GeometryResult, materials: &GlossyMaterials) {
    log_census(&take_census(geometry, materials));
}

/// The census's report line. The group count is the probe estimate glossy
/// faces ask for against the cap; placement, when it runs, groups faces by
/// facing, plane and fitted box instead, so its count can differ.
fn log_census(census: &GlossyCensus) {
    log::info!(
        "[ReflectionProbes] glossy census: {} of {} material(s) reflect; {} world face(s) on {} \
         plane(s) in {} edge-connected group(s), an estimated {} probe(s) against the \
         {MAX_REFLECTION_PROBES}-probe cap ({} unreadable material(s), {} face(s) with no \
         polygon or no area)",
        census.glossy_materials,
        census.used_materials,
        census.faces,
        census.planes,
        census.groups,
        census.groups,
        census.unreadable_materials,
        census.unrecovered_faces,
    );
}

/// Faces by plane, each plane's faces ascending. A face joins the first plane
/// whose first face's normal lies within 1° of its own and whose offset,
/// measured along that normal, lies within [`PLANE_TOLERANCE_M`]; opposite
/// facings are always different planes.
pub(super) fn coplanar_planes(faces: &[WorldReflectorFace]) -> Vec<Vec<usize>> {
    let mut planes: Vec<(DVec3, f64, Vec<usize>)> = Vec::new();
    for (index, face) in faces.iter().enumerate() {
        let normal = face.geometry.normal;
        let plane = planes.iter_mut().find(|(n, offset, _)| {
            normal.dot(*n) >= UP_FACING_MIN_COS
                && (face.geometry.centroid.dot(*n) - offset).abs() < PLANE_TOLERANCE_M
        });
        match plane {
            Some((_, _, members)) => members.push(index),
            None => planes.push((normal, face.plane_offset, vec![index])),
        }
    }
    planes.into_iter().map(|(_, _, members)| members).collect()
}

/// The groups `plane`'s faces form, as indices into `faces`, each ascending,
/// in order of their first face. Two faces join when an
/// edge of one overlaps an edge of the other along a stretch longer than
/// [`EDGE_TOLERANCE_M`]. Faces only touching at a corner stay apart. Brush
/// splits leave T-junctions, so edges are matched by overlap, not by shared
/// vertices.
pub(super) fn edge_connected_groups(
    faces: &[WorldReflectorFace],
    plane: &[usize],
) -> Vec<Vec<usize>> {
    let outlines: Vec<Vec<DVec3>> = plane
        .iter()
        .map(|&i| faces[i].outline_points().collect())
        .collect();
    let bounds: Vec<(DVec3, DVec3)> = outlines
        .iter()
        .map(|points| {
            let min = points.iter().fold(DVec3::INFINITY, |m, &p| m.min(p));
            let max = points.iter().fold(DVec3::NEG_INFINITY, |m, &p| m.max(p));
            (min - EDGE_TOLERANCE_M, max + EDGE_TOLERANCE_M)
        })
        .collect();
    // Sweep along x: only faces whose padded boxes overlap can share an edge.
    let mut order: Vec<usize> = (0..plane.len()).collect();
    order.sort_by(|&a, &b| bounds[a].0.x.total_cmp(&bounds[b].0.x));
    let mut parent: Vec<usize> = (0..plane.len()).collect();
    for (at, &a) in order.iter().enumerate() {
        for &b in &order[at + 1..] {
            if bounds[b].0.x > bounds[a].1.x {
                break;
            }
            let overlap =
                bounds[a].0.cmple(bounds[b].1).all() && bounds[b].0.cmple(bounds[a].1).all();
            if overlap && outlines_share_edge(&outlines[a], &outlines[b]) {
                let (ra, rb) = (root(&mut parent, a), root(&mut parent, b));
                parent[ra.max(rb)] = ra.min(rb);
            }
        }
    }
    // Roots are each group's lowest member, so group order follows `plane`.
    let mut groups: Vec<Vec<usize>> = Vec::new();
    let mut group_of_root: Vec<Option<usize>> = vec![None; plane.len()];
    for (member, &face) in plane.iter().enumerate() {
        let r = root(&mut parent, member);
        let group = *group_of_root[r].get_or_insert_with(|| {
            groups.push(Vec::new());
            groups.len() - 1
        });
        groups[group].push(face);
    }
    groups
}

fn root(parent: &mut [usize], mut i: usize) -> usize {
    while parent[i] != i {
        parent[i] = parent[parent[i]];
        i = parent[i];
    }
    i
}

fn edges(points: &[DVec3]) -> impl Iterator<Item = (DVec3, DVec3)> + '_ {
    (0..points.len()).map(move |i| (points[i], points[(i + 1) % points.len()]))
}

fn outlines_share_edge(a: &[DVec3], b: &[DVec3]) -> bool {
    edges(a).any(|(a0, a1)| edges(b).any(|(b0, b1)| segments_overlap(a0, a1, b0, b1)))
}

/// Whether segment b lies on segment a's line within [`EDGE_TOLERANCE_M`]
/// and overlaps it along a stretch longer than that.
fn segments_overlap(a0: DVec3, a1: DVec3, b0: DVec3, b1: DVec3) -> bool {
    let along = a1 - a0;
    let length = along.length();
    if length <= EDGE_TOLERANCE_M {
        return false;
    }
    let dir = along / length;
    let off_line = |p: DVec3| {
        let rel = p - a0;
        (rel - dir * rel.dot(dir)).length()
    };
    if off_line(b0) > EDGE_TOLERANCE_M || off_line(b1) > EDGE_TOLERANCE_M {
        return false;
    }
    let (t0, t1) = ((b0 - a0).dot(dir), (b1 - a0).dot(dir));
    t0.max(t1).min(length) - t0.min(t1).max(0.0) > EDGE_TOLERANCE_M
}
