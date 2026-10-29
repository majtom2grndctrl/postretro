// Occluder clusters: touching brushes each exempt alone, judged per
// near-planar section: a section occludes when a broad face of its bounding
// box passes the occluder rule and their solid covers most of that face under
// the face's roofline.
// See: context/plans/in-progress/occluder-first-partition/research.md §Clusters

pub(crate) mod contact;
mod coverage;
mod section;

use glam::{DVec2, DVec3};

use super::brush_bsp::REGION_CLASSIFICATION_EPSILON;
use super::occluder_classify::{
    BrushFrame, OccluderVerdict, OpenSpace, broad_face_floor, broad_face_open_depth,
    brush_polytope, judge_box, judge_face, see_over_m, vertical_broad_axis, within_rule_slack,
};
use crate::map_data::BrushVolume;
use contact::{Hull, touching_pairs};
use coverage::{SeeOverLine, covered_cells};
use section::{Member, SECTION_NORMAL_MAX_ANGLE_COS, find_sections};

pub use coverage::COVERAGE_GRID_CELLS;

/// A section occludes only when its solid covers strictly more than this
/// share of a broad face of its bounding box that passes the occluder rule,
/// under that face's roofline: "mostly solid".
/// Between a colonnade under a beam (about a third) and a facade a third
/// windows (about two thirds).
pub const OCCLUDER_COVERAGE_FRACTION: f64 = 0.5;

/// Axis of a section's frame across its slab.
pub(crate) const SECTION_NORMAL_AXIS: usize = 2;

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
    /// The individual occluder rule on the bounding box, from either broad
    /// face.
    pub box_verdict: OccluderVerdict,
    /// Share of a broad face of the bounding box the members cover, on the
    /// grid: of the cells under that face's roofline when it has a vertical
    /// axis (see `face_coverage`), of every cell when it is a slab's. The
    /// higher share of the faces that pass the occluder rule, or of both
    /// faces when neither does.
    pub coverage: f64,
    /// True when a broad face of the bounding box passes the occluder rule
    /// and the members cover strictly more than `OCCLUDER_COVERAGE_FRACTION`
    /// of it under its roofline.
    pub occludes: bool,
    /// Outward normal of the one broad face that bounds the section in the
    /// occluder tier: `normal` or its negation (see `bounding_face`).
    pub bounding_face: DVec3,
    /// Per member, one flag per brush plane: true when the plane bounds the
    /// box on the bounding face or on one of its four edge faces (see
    /// `outline_planes`). Those planes bound the section in the occluder
    /// tier; the rest, the other broad face and the openings' jambs, sills
    /// and lintels, split only beneath it, so each opening's air lies in the
    /// cell behind the bounding face and, unless another tier plane crosses
    /// the slab, crosses into the cell in front through one portal.
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

    // A set is judged under every normal it forms under and occludes if it
    // does under any; the first occluding judgment in the set's normal order
    // stands for it, else the first.
    let sections = find_sections(&members, &member_touching)
        .into_iter()
        .map(|shape| {
            let mut first = None;
            for frame in shape.frames {
                let section = judge_section(brushes, &members, &shape.members, frame, space);
                if section.occludes {
                    return section;
                }
                first.get_or_insert(section);
            }
            first.expect("a section forms under at least one normal")
        })
        .collect();

    let mut cluster: Vec<usize> = members.iter().map(|m| m.brush).collect();
    cluster.sort_unstable();
    Cluster {
        members: cluster,
        sections,
    }
}

/// Judge one section: `in_section` indexes `members`, and `frame` is its
/// box across one normal it forms under. Each broad face is judged as a
/// single brush's is, by the occluder rule from that face, and must also be
/// mostly covered under its own roofline.
fn judge_section(
    brushes: &[BrushVolume],
    members: &[Member<'_>],
    in_section: &[usize],
    frame: BrushFrame,
    space: &OpenSpace<'_>,
) -> Section {
    let box_verdict = judge_box(&frame, [SECTION_NORMAL_AXIS], space);
    let solids: Vec<&[DVec3]> = in_section.iter().map(|&m| members[m].vertices).collect();
    let floors = [1.0, -1.0].map(|outward| face_floor(&frame, outward, space));
    let along = face_coverage(&frame, &solids, floors[0]);
    // Both faces usually read one floor; the raster is the costly part.
    let against = if floors[1] == floors[0] {
        along
    } else {
        face_coverage(&frame, &solids, floors[1])
    };
    let share = |(covered, counted): (usize, usize)| {
        if counted == 0 {
            0.0
        } else {
            covered as f64 / counted as f64
        }
    };
    let faces = [(1.0, along), (-1.0, against)].map(|(outward, cells)| {
        let passes =
            judge_face(&frame, SECTION_NORMAL_AXIS, outward, space) == OccluderVerdict::Occludes;
        // Strict, and exact on the integer cell counts.
        let dense = cells.0 as f64 > OCCLUDER_COVERAGE_FRACTION * cells.1 as f64;
        (passes, dense, share(cells))
    });
    let occludes = faces.iter().any(|&(passes, dense, _)| passes && dense);
    let coverage = faces
        .iter()
        .filter(|&&(passes, _, _)| passes)
        .map(|&(_, _, share)| share)
        .reduce(f64::max)
        .unwrap_or(faces[0].2.max(faces[1].2));

    let mut in_slab: Vec<&Member<'_>> = in_section.iter().map(|&m| &members[m]).collect();
    in_slab.sort_unstable_by_key(|member| member.brush);
    let bounding_face = bounding_face(&frame, space);
    let outline = in_slab
        .iter()
        .map(|member| {
            outline_planes(
                &brushes[member.brush],
                member.vertices,
                &frame,
                bounding_face,
            )
        })
        .collect();
    Section {
        members: in_slab.iter().map(|member| member.brush).collect(),
        normal: frame.axes[SECTION_NORMAL_AXIS],
        extents: frame.extents,
        box_verdict,
        coverage,
        occludes,
        bounding_face,
        outline,
        frame,
    }
}

/// World height of the floor the see-over test reads in front of the
/// section's broad face whose outward normal is `outward` (±1) times its
/// normal: the face's own, as the rule judges each face from its own side.
/// `None` for a slab's face, both broad axes horizontal, or a face with no
/// sample fronting open space at least the floor deep.
fn face_floor(frame: &BrushFrame, outward: f64, space: &OpenSpace<'_>) -> Option<f64> {
    let up = vertical_broad_axis(frame, SECTION_NORMAL_AXIS)?;
    broad_face_floor(frame, SECTION_NORMAL_AXIS, up, outward, space)
}

/// The section's covered and counted cells over a broad face of its box.
///
/// A face with a vertical broad axis (within 45° of up, the rule's vertical
/// test) and a `floor` in front counts under its roofline: each column along
/// the horizontal axis counts up to the higher of its members' solid and the
/// see-over height above that floor. So sky over a lower neighbour's roof
/// does not count against a row, while a low wall between tall posts, or a
/// gap with no solid, still counts to see-over height. A slab's face, both
/// broad axes horizontal, or a face with no floor, counts every cell. Both
/// broad faces project alike, so only the floor tells them apart.
fn face_coverage(frame: &BrushFrame, solids: &[&[DVec3]], floor: Option<f64>) -> (usize, usize) {
    let normal = frame.axes[SECTION_NORMAL_AXIS];
    // `[along, up]` broad axes with the up one's sign, and the see-over line.
    let ([along, up], up_sign, roofline) = match vertical_broad_axis(frame, SECTION_NORMAL_AXIS) {
        Some(up) => {
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

/// One flag per plane of `brush`, whose corners are `vertices`: true when
/// the plane bounds `frame`'s box on its bounding face (outward normal
/// `bounding_face`) or on an edge face, never on the other broad face. A
/// plane bounds a box face when its normal lies within the section angle of
/// the face's and a corner on it lies on the face. Members join a section
/// within that angle, so members a few degrees off square to the box still
/// bound it where they reach its faces, and an occluding section of box
/// brushes always has a plane on its bounding face.
fn outline_planes(
    brush: &BrushVolume,
    vertices: &[DVec3],
    frame: &BrushFrame,
    bounding_face: DVec3,
) -> Vec<bool> {
    let on = |point: DVec3, normal: DVec3, distance: f64| {
        (point.dot(normal) - distance).abs() <= REGION_CLASSIFICATION_EPSILON
    };
    brush
        .planes
        .iter()
        .map(|plane| {
            (0..3).any(|axis| {
                let along = plane.normal.dot(frame.axes[axis]);
                if along.abs() < SECTION_NORMAL_MAX_ANGLE_COS {
                    return false;
                }
                let outward = frame.axes[axis] * along.signum();
                if axis == SECTION_NORMAL_AXIS && outward.dot(bounding_face) < 0.0 {
                    return false;
                }
                let face = frame.center.dot(outward) + frame.extents[axis] * 0.5;
                vertices.iter().any(|&corner| {
                    on(corner, plane.normal, plane.distance) && on(corner, outward, face)
                })
            })
        })
        .collect()
}

// The rows these fixtures serve run in `portals::occluder_tier`, beside the
// classifier's.
#[cfg(test)]
#[allow(dead_code)]
pub(crate) mod test_fixtures;
