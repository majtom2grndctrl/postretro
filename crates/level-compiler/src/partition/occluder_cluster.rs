// Occluder clusters: touching brushes each exempt alone, judged per
// near-planar section by the occluder rule on the section's bounding box and
// by how much of that box's broad face, under its roofline, their solid covers.
// See: context/plans/in-progress/occluder-first-partition/research.md §Clusters

pub(crate) mod contact;
mod coverage;
mod section;

use glam::{DVec2, DVec3};

use super::brush_bsp::REGION_CLASSIFICATION_EPSILON;
use super::occluder_classify::{
    BrushFrame, OccluderVerdict, OpenSpace, broad_face_floor, broad_face_open_depth,
    brush_polytope, judge_box, see_over_m, vertical_broad_axis, within_rule_slack,
};
use crate::map_data::BrushVolume;
use contact::{Hull, touching_pairs};
use coverage::{SeeOverLine, covered_cells};
use section::{Member, find_sections};

pub use coverage::COVERAGE_GRID_CELLS;

/// A section occludes only when its solid covers strictly more than this
/// share of its bounding broad face under its roofline: "mostly solid".
/// Between a colonnade under a beam (about a third) and a facade a third
/// windows (about two thirds).
pub const OCCLUDER_COVERAGE_FRACTION: f64 = 0.5;

/// Axis of a section's frame across its slab.
const SECTION_NORMAL_AXIS: usize = 2;

/// A member plane whose normal is this close to a box axis can lie on the
/// box's face.
const OUTLINE_PARALLEL_COS: f64 = 1.0 - 1e-9;

/// Touching brushes, each exempt alone, and the sections they are judged by.
#[derive(Debug, Clone)]
pub struct Cluster {
    /// Brush indices, ascending.
    pub members: Vec<usize>,
    pub sections: Vec<Section>,
}

/// One near-planar section of a cluster.
#[derive(Debug, Clone)]
pub struct Section {
    /// The in-slab members, brush indices ascending. When the section
    /// occludes these join the occluder tier; members outside every
    /// occluding section stay exempt.
    pub members: Vec<usize>,
    /// Across the section's slab, the normal of its bounding broad face.
    pub normal: DVec3,
    /// Bounding box extents in the section's own frame: the two broad axes,
    /// then across the slab.
    pub extents: [f64; 3],
    /// The bounding box itself: the slab the cell-size cap's slab guard
    /// keeps its grid lines out of.
    pub(crate) frame: BrushFrame,
    /// The individual occluder rule on the bounding box.
    pub box_verdict: OccluderVerdict,
    /// Share of the bounding broad face the members cover, on the grid:
    /// of the cells under its roofline when the face has a vertical axis
    /// (see `face_coverage`), of every cell when it is a slab's.
    pub coverage: f64,
    pub occludes: bool,
    /// Outward normal of the one broad face that bounds the section in the
    /// occluder tier: `normal` or its negation (see `bounding_face`).
    pub bounding_face: DVec3,
    /// Per member, one flag per brush plane: true when the plane lies, within
    /// the contact epsilon, on the bounding face or on one of the box's four
    /// edge faces. Those planes bound the section in the occluder tier; the
    /// rest, the other broad face and the openings' jambs, sills and lintels,
    /// split only beneath it, so each opening's air lies in the cell behind
    /// the bounding face and, unless another tier plane crosses the slab,
    /// crosses into the cell in front through one portal.
    pub outline: Vec<Vec<bool>>,
}

/// Find and judge every cluster of touching brushes whose `verdicts` are
/// exempt. A brush that occludes alone never joins one, so tiles touching
/// only through an occluding floor form no cluster. Clusters are components
/// of the contact graph and their sections sets of members, so neither
/// depends on brush order.
pub(super) fn find_clusters(
    brushes: &[BrushVolume],
    verdicts: &[OccluderVerdict],
    space: &OpenSpace<'_>,
) -> Vec<Cluster> {
    let exempt: Vec<usize> = (0..brushes.len())
        .filter(|&i| verdicts[i] != OccluderVerdict::Occludes)
        .collect();
    let mut hulls = Vec::with_capacity(exempt.len());
    let mut frames = Vec::with_capacity(exempt.len());
    let mut kept = Vec::with_capacity(exempt.len());
    for &brush in &exempt {
        let Some(frame) = BrushFrame::of(&brushes[brush]) else {
            continue;
        };
        hulls.push(Hull::of(&brush_polytope(&brushes[brush])));
        frames.push(frame);
        kept.push(brush);
    }

    let mut touching: Vec<Vec<usize>> = vec![Vec::new(); kept.len()];
    for (i, j) in touching_pairs(&hulls) {
        touching[i].push(j);
        touching[j].push(i);
    }

    let mut clusters = Vec::new();
    let mut visited = vec![false; kept.len()];
    for start in 0..kept.len() {
        if visited[start] || touching[start].is_empty() {
            continue;
        }
        visited[start] = true;
        let mut stack = vec![start];
        let mut component = Vec::new();
        while let Some(i) = stack.pop() {
            component.push(i);
            for &j in &touching[i] {
                if !visited[j] {
                    visited[j] = true;
                    stack.push(j);
                }
            }
        }
        component.sort_unstable();
        clusters.push(judge_cluster(
            brushes, &component, &kept, &frames, &hulls, &touching, space,
        ));
    }
    clusters
}

/// Judge one cluster: `component` indexes the kept exempt brushes.
fn judge_cluster(
    brushes: &[BrushVolume],
    component: &[usize],
    kept: &[usize],
    frames: &[BrushFrame],
    hulls: &[Hull],
    touching: &[Vec<usize>],
    space: &OpenSpace<'_>,
) -> Cluster {
    let local = |i: usize| component.binary_search(&i).ok();
    let members: Vec<Member<'_>> = component
        .iter()
        .map(|&i| Member {
            brush: kept[i],
            frame: &frames[i],
            vertices: &hulls[i].vertices,
        })
        .collect();
    let member_touching: Vec<Vec<usize>> = component
        .iter()
        .map(|&i| touching[i].iter().filter_map(|&j| local(j)).collect())
        .collect();

    let sections = find_sections(&members, &member_touching)
        .into_iter()
        .map(|shape| {
            let frame = shape.frame;
            let box_verdict = judge_box(&frame, [SECTION_NORMAL_AXIS], space);
            let solids: Vec<&[DVec3]> =
                shape.members.iter().map(|&m| members[m].vertices).collect();
            let (covered, counted) = face_coverage(&frame, &solids, space);
            let coverage = if counted == 0 {
                0.0
            } else {
                covered as f64 / counted as f64
            };
            // Strict, and exact on the integer cell counts (pin R12).
            let dense = covered as f64 > OCCLUDER_COVERAGE_FRACTION * counted as f64;
            let mut in_slab: Vec<usize> = shape.members.iter().map(|&m| members[m].brush).collect();
            in_slab.sort_unstable();
            let bounding_face = bounding_face(&frame, space);
            let outline = in_slab
                .iter()
                .map(|&brush| outline_planes(&brushes[brush], &frame, bounding_face))
                .collect();
            Section {
                members: in_slab,
                normal: frame.axes[SECTION_NORMAL_AXIS],
                extents: frame.extents,
                box_verdict,
                coverage,
                occludes: box_verdict == OccluderVerdict::Occludes && dense,
                bounding_face,
                outline,
                frame,
            }
        })
        .collect();

    let mut cluster: Vec<usize> = members.iter().map(|m| m.brush).collect();
    cluster.sort_unstable();
    Cluster {
        members: cluster,
        sections,
    }
}

/// The section's covered and counted cells over its bounding broad face.
///
/// A face with a vertical broad axis (within 45° of up, the rule's vertical
/// test) counts under its roofline: each column along the horizontal axis
/// counts up to the higher of its members' solid and the see-over height
/// above the floor in front. The floor is the lowest the see-over test reads
/// in front of either broad face, as the rule passes from either. So sky over
/// a lower neighbour's roof does not count against a row, while a low wall
/// between tall posts, or a gap with no solid, still counts to see-over
/// height. A slab's face, both broad axes horizontal, or a face with no
/// sample fronting open space, counts every cell.
fn face_coverage(frame: &BrushFrame, solids: &[&[DVec3]], space: &OpenSpace<'_>) -> (usize, usize) {
    let normal = frame.axes[SECTION_NORMAL_AXIS];
    // `[along, up]` broad axes with the up one's sign, and the see-over line.
    let ([along, up], up_sign, roofline) = match vertical_broad_axis(frame, SECTION_NORMAL_AXIS) {
        Some(up) => {
            let floor = [1.0, -1.0]
                .into_iter()
                .filter_map(|outward| {
                    broad_face_floor(frame, SECTION_NORMAL_AXIS, up, outward, space)
                })
                .reduce(f64::min);
            let roofline = floor.map(|floor| SeeOverLine {
                height: floor + see_over_m(),
                plane_y: frame.center.dot(normal) * normal.y,
            });
            let along = 3 - SECTION_NORMAL_AXIS - up;
            ([along, up], frame.axes[up].y.signum(), roofline)
        }
        None => ([0, 1], 1.0, None),
    };
    let axes = [frame.axes[along], frame.axes[up] * up_sign];
    let extent = DVec2::new(frame.extents[along], frame.extents[up]);
    let lo = DVec2::new(frame.center.dot(axes[0]), frame.center.dot(axes[1])) - extent * 0.5;
    covered_cells(axes, lo, extent, solids, roofline.as_ref())
}

/// The section's bounding face: of its two broad faces, the one fronting the
/// deeper open space, as the face lattice measures it; on a tie, the one
/// facing along the canonical section normal. Splitting on one broad face
/// gives each opening one portal where no other tier plane crosses the slab;
/// on the deeper side, the larger and more widely seen cell gains none of the
/// openings' air or reveals, which go to the shallower, more enclosed cell
/// behind the face. Reads only canonical geometry and the first pass, so it
/// does not depend on brush order.
fn bounding_face(frame: &BrushFrame, space: &OpenSpace<'_>) -> DVec3 {
    let normal = frame.axes[SECTION_NORMAL_AXIS];
    let depth = |outward: f64| broad_face_open_depth(frame, SECTION_NORMAL_AXIS, outward, space);
    let (along, against) = (depth(1.0), depth(-1.0));
    if against > along && !within_rule_slack(against, along) {
        -normal
    } else {
        normal
    }
}

/// One flag per plane of `brush`: true when the plane lies on `frame`'s
/// box, on its bounding face (outward normal `bounding_face`) or on an edge
/// face, never on the other broad face.
fn outline_planes(brush: &BrushVolume, frame: &BrushFrame, bounding_face: DVec3) -> Vec<bool> {
    brush
        .planes
        .iter()
        .map(|plane| {
            (0..3).any(|axis| {
                let along = plane.normal.dot(frame.axes[axis]);
                if along.abs() < OUTLINE_PARALLEL_COS {
                    return false;
                }
                if axis == SECTION_NORMAL_AXIS && plane.normal.dot(bounding_face) < 0.0 {
                    return false;
                }
                let face =
                    frame.center.dot(frame.axes[axis]) * along.signum() + frame.extents[axis] * 0.5;
                (plane.distance - face).abs() <= REGION_CLASSIFICATION_EPSILON
            })
        })
        .collect()
}

// The rows these fixtures serve run in `portals::occluder_tier`, beside the
// classifier's.
#[cfg(test)]
#[allow(dead_code)]
pub(crate) mod test_fixtures;
