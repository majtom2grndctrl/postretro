// Occluder clusters: touching brushes each exempt alone, judged per
// near-planar section: a section occludes when a broad face of its bounding
// box passes the occluder rule and their solid covers most of that face under
// the face's roofline.
// See: context/plans/done/occluder-first-partition/research.md §Clusters

pub(crate) mod contact;
mod coverage;
mod section;

use glam::{DVec2, DVec3};

use super::brush_bsp::REGION_CLASSIFICATION_EPSILON;
use super::occluder_classify::{
    BrushFrame, FACE_SAMPLE_FRACTIONS, OccluderVerdict, OpenSpace, axis_order, broad_face_floor,
    broad_face_open_depth, brush_polytope, judge_face, see_over_m, vertical_broad_axis,
    within_rule_slack,
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

/// Plane normals this close to parallel, at one distance within the contact
/// epsilon, are copies of one plane, as coplanar members' faces are.
const PLANE_COPY_COS: f64 = 1.0 - 1e-9;

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
    /// occludes these join the occluder tier through their `outline` planes;
    /// a member with no outline plane, and every member outside every
    /// occluding section, still splits only beneath the tier.
    pub members: Vec<usize>,
    /// The canonical axis normal to both broad faces of the section's box;
    /// `bounding_face` is this or its negation.
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
    /// box on the bounding face or on one of its four edge faces, one plane
    /// to a face (see `outline`). Those planes bound the section in the
    /// occluder tier; the rest, the other broad face and the openings' jambs,
    /// sills and lintels, split only beneath it, so each opening's air lies
    /// in the cell behind the bounding face and, unless another tier plane
    /// crosses the slab, crosses into the cell in front through one portal.
    /// All false when the section does not occlude, as it splits nothing.
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
    // does under any; `standing_judgment` picks the judgment that stands for
    // it by geometry, so turning the map does not change its planes.
    let sections = find_sections(&members, &member_touching)
        .into_iter()
        .map(|shape| {
            let judgments = shape
                .frames
                .into_iter()
                .map(|frame| judge_frame(&members, &shape.members, frame, space))
                .collect();
            section_of(
                brushes,
                &members,
                &shape.members,
                standing_judgment(judgments, space),
                space,
            )
        })
        .collect();

    let mut cluster: Vec<usize> = members.iter().map(|m| m.brush).collect();
    cluster.sort_unstable();
    Cluster {
        members: cluster,
        sections,
    }
}

/// A section judged under one normal it forms under.
struct Judgment {
    /// Its box across that normal.
    frame: BrushFrame,
    box_verdict: OccluderVerdict,
    coverage: f64,
    occludes: bool,
}

/// Judge one section: `in_section` indexes `members`, and `frame` is its
/// box across one normal it forms under. Each broad face is judged as a
/// single brush's is, by the occluder rule from that face, and must also be
/// mostly covered under its own roofline. Each face's lattice is sampled
/// once, and the box's verdict folds the two faces'.
fn judge_frame(
    members: &[Member<'_>],
    in_section: &[usize],
    frame: BrushFrame,
    space: &OpenSpace<'_>,
) -> Judgment {
    let verdicts =
        [1.0, -1.0].map(|outward| judge_face(&frame, SECTION_NORMAL_AXIS, outward, space));
    let box_verdict = verdicts[0].furthest(verdicts[1]);
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
    let faces = [(verdicts[0], along), (verdicts[1], against)].map(|(verdict, cells)| {
        let passes = verdict == OccluderVerdict::Occludes;
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
    Judgment {
        frame,
        box_verdict,
        coverage,
        occludes,
    }
}

/// The judgment that stands for a set judged under several normals, given
/// in the set's normal order. Of the occluding ones: the tightest box, as a
/// brush's own frame is its tightest box; then the higher coverage; then the
/// box whose bounding face fronts the deeper open space, as the bounding
/// face itself is chosen. Each reads only geometry and the first pass, so
/// only a tie on all three falls to the normals' order, which follows world
/// orientation. An exempt set keeps its first judgment: it splits nothing.
fn standing_judgment(judgments: Vec<Judgment>, space: &OpenSpace<'_>) -> Judgment {
    let mut first = None;
    let mut occluding: Vec<Judgment> = Vec::new();
    for judgment in judgments {
        if judgment.occludes {
            occluding.push(judgment);
        } else if first.is_none() {
            first = Some(judgment);
        }
    }
    if occluding.is_empty() {
        return first.expect("a section forms under at least one normal");
    }

    // Extents and shares carry rounding from each frame's fit; the slack
    // keeps a tie a tie whichever way the map is turned.
    let volume = |judgment: &Judgment| judgment.frame.extents.iter().product::<f64>();
    let least = occluding.iter().map(volume).fold(f64::INFINITY, f64::min);
    occluding.retain(|judgment| volume(judgment) <= least + least * 1e-9 + 1e-12);
    let most = occluding
        .iter()
        .map(|judgment| judgment.coverage)
        .fold(f64::NEG_INFINITY, f64::max);
    occluding.retain(|judgment| judgment.coverage >= most - 1e-9);
    if occluding.len() > 1 {
        let depth = |judgment: &Judgment| {
            [1.0, -1.0]
                .map(|outward| {
                    broad_face_open_depth(&judgment.frame, SECTION_NORMAL_AXIS, outward, space)
                })
                .into_iter()
                .fold(0.0, f64::max)
        };
        let depths: Vec<f64> = occluding.iter().map(depth).collect();
        let deepest = depths.iter().copied().fold(0.0, f64::max);
        occluding = occluding
            .into_iter()
            .zip(depths)
            .filter(|&(_, open)| within_rule_slack(open, deepest))
            .map(|(judgment, _)| judgment)
            .collect();
    }
    occluding
        .into_iter()
        .next()
        .expect("the deepest judgment stands")
}

/// The section a set's standing judgment makes: its in-slab members, and,
/// when it occludes, the planes that bound it in the occluder tier.
fn section_of(
    brushes: &[BrushVolume],
    members: &[Member<'_>],
    in_section: &[usize],
    judgment: Judgment,
    space: &OpenSpace<'_>,
) -> Section {
    let Judgment {
        frame,
        box_verdict,
        coverage,
        occludes,
    } = judgment;
    let mut in_slab: Vec<&Member<'_>> = in_section.iter().map(|&m| &members[m]).collect();
    in_slab.sort_unstable_by_key(|member| member.brush);
    let bounding_face = bounding_face(&frame, space);
    let outline = if occludes {
        outline(brushes, &in_slab, &frame, bounding_face)
    } else {
        in_slab
            .iter()
            .map(|member| vec![false; brushes[member.brush].planes.len()])
            .collect()
    };
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
/// normal, per column of the face's lattice: the face's own, as the rule
/// judges each face from its own side. `None` for a slab's face, both broad
/// axes horizontal, or a face with no sample fronting open space at least
/// the floor deep.
fn face_floor(frame: &BrushFrame, outward: f64, space: &OpenSpace<'_>) -> Option<[Option<f64>; 3]> {
    let up = vertical_broad_axis(frame, SECTION_NORMAL_AXIS)?;
    let floors = broad_face_floor(frame, SECTION_NORMAL_AXIS, up, outward, space)?;
    floors.iter().any(Option::is_some).then_some(floors)
}

/// The section's covered and counted cells over a broad face of its box.
///
/// A face with a vertical broad axis (within 45° of up, the rule's vertical
/// test) and `floors` in front counts under its roofline: each column along
/// the horizontal axis counts up to the higher of its members' solid and the
/// see-over height above the floor in front of that column. So sky over a
/// lower neighbour's roof does not count against a row, while a low wall
/// between tall posts, or a gap with no solid, still counts to see-over
/// height, and a stairwell in front of part of a face lowers the floor only
/// there. A slab's face, both broad axes horizontal, or a face with no
/// floor, counts every cell. Both broad faces project alike, so only the
/// floors tell them apart.
fn face_coverage(
    frame: &BrushFrame,
    solids: &[&[DVec3]],
    floors: Option<[Option<f64>; 3]>,
) -> (usize, usize) {
    let normal = frame.axes[SECTION_NORMAL_AXIS];
    // `[along, up]` broad axes with the up one's sign, and the see-over line.
    let ([along, up], up_sign, roofline) = match vertical_broad_axis(frame, SECTION_NORMAL_AXIS) {
        Some(up) => {
            let roofline = floors.map(|floors| SeeOverLine {
                heights: column_floors(floors)
                    .map(|floor| floor + see_over_m())
                    .collect(),
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

/// The floor in front of each coverage column, along the face's horizontal
/// axis: that of the nearest lattice column with one, as `broad_face_floor`
/// gives them, so the roofline reads the floor where the rule reads it. No
/// grid column's centre lies midway between two lattice columns, so the
/// nearest is never a tie. `floors` holds at least one floor.
fn column_floors(floors: [Option<f64>; 3]) -> impl Iterator<Item = f64> {
    (0..COVERAGE_GRID_CELLS).map(move |column| {
        let at = (column as f64 + 0.5) / COVERAGE_GRID_CELLS as f64;
        FACE_SAMPLE_FRACTIONS
            .into_iter()
            .zip(floors)
            .filter_map(|(fraction, floor)| floor.map(|floor| ((fraction - at).abs(), floor)))
            .min_by(|a, b| a.0.total_cmp(&b.0))
            .map(|(_, floor)| floor)
            .expect("a lattice column with a floor")
    })
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

/// Per member of `in_slab`, one flag per brush plane: true when the plane
/// bounds `frame`'s box on its bounding face (outward normal
/// `bounding_face`) or on an edge face, never on the other broad face. A
/// plane can bound a box face when its normal lies within the section angle
/// of the face's and a corner on it lies on the face. Members join a section
/// within that angle, so members a few degrees off square to the box still
/// reach its faces, and an occluding section of box brushes always has a
/// plane on its bounding face.
///
/// Each box face takes one of those planes, with every copy of it: the one
/// the section's solid stands least in front of. A plane at occluder rank
/// extends through every region it spans, so the face of each of a row of
/// louvres a few degrees off square, parallel to the next a few centimetres
/// behind it, would cut the open space in front into strips; one plane
/// bounds the section as one face of its box does. Members square to the
/// box share that plane, so a square section keeps every member's plane on
/// it.
fn outline(
    brushes: &[BrushVolume],
    in_slab: &[&Member<'_>],
    frame: &BrushFrame,
    bounding_face: DVec3,
) -> Vec<Vec<bool>> {
    /// A member plane that can bound one face of the box.
    struct Candidate {
        member: usize,
        plane: usize,
        normal: DVec3,
        distance: f64,
        /// How far the section's solid stands in front of the plane.
        protrusion: f64,
    }

    let on = |point: DVec3, normal: DVec3, distance: f64| {
        (point.dot(normal) - distance).abs() <= REGION_CLASSIFICATION_EPSILON
    };
    let vertices: Vec<DVec3> = in_slab
        .iter()
        .flat_map(|member| member.vertices.iter().copied())
        .collect();
    let mut outline: Vec<Vec<bool>> = in_slab
        .iter()
        .map(|member| vec![false; brushes[member.brush].planes.len()])
        .collect();
    for (axis, &box_axis) in frame.axes.iter().enumerate() {
        for sign in [1.0, -1.0] {
            let outward = box_axis * sign;
            if axis == SECTION_NORMAL_AXIS && outward.dot(bounding_face) < 0.0 {
                continue;
            }
            let face = frame.center.dot(outward) + frame.extents[axis] * 0.5;
            let mut candidates: Vec<Candidate> = Vec::new();
            for (member, slab_member) in in_slab.iter().enumerate() {
                for (plane, brush_plane) in brushes[slab_member.brush].planes.iter().enumerate() {
                    let (normal, distance) = (brush_plane.normal, brush_plane.distance);
                    if normal.dot(outward) < SECTION_NORMAL_MAX_ANGLE_COS {
                        continue;
                    }
                    let touches = slab_member
                        .vertices
                        .iter()
                        .any(|&corner| on(corner, normal, distance) && on(corner, outward, face));
                    if !touches {
                        continue;
                    }
                    let protrusion = vertices
                        .iter()
                        .map(|vertex| vertex.dot(normal) - distance)
                        .fold(f64::NEG_INFINITY, f64::max);
                    candidates.push(Candidate {
                        member,
                        plane,
                        normal,
                        distance,
                        protrusion,
                    });
                }
            }

            // Corners carry rounding from each member's clip, so near-equal
            // protrusions tie; the tie then goes to the plane's own exact
            // data, most square to the face first, so neither brush nor side
            // order decides it.
            let least = candidates
                .iter()
                .map(|candidate| candidate.protrusion)
                .fold(f64::INFINITY, f64::min);
            let Some(chosen) = candidates
                .iter()
                .filter(|candidate| candidate.protrusion <= least + REGION_CLASSIFICATION_EPSILON)
                .min_by(|a, b| {
                    b.normal
                        .dot(outward)
                        .total_cmp(&a.normal.dot(outward))
                        .then(axis_order(a.normal, b.normal))
                        .then(a.distance.total_cmp(&b.distance))
                })
            else {
                continue;
            };
            let (normal, distance) = (chosen.normal, chosen.distance);
            for candidate in &candidates {
                if candidate.normal.dot(normal) >= PLANE_COPY_COS
                    && (candidate.distance - distance).abs() <= REGION_CLASSIFICATION_EPSILON
                {
                    outline[candidate.member][candidate.plane] = true;
                }
            }
        }
    }
    outline
}

// The rows these fixtures serve run in `portals::occluder_tier`, beside the
// classifier's.
#[cfg(test)]
#[allow(dead_code)]
pub(crate) mod test_fixtures;
