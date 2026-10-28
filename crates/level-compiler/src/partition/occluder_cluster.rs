// Occluder clusters: touching brushes each exempt alone, judged per
// near-planar section by the occluder rule on the section's bounding box and
// by how much of that box's broad face their solid covers.
// See: context/plans/in-progress/occluder-first-partition/research.md §Clusters

pub(crate) mod contact;
mod coverage;
mod section;

use glam::{DVec2, DVec3};

use super::brush_bsp::REGION_CLASSIFICATION_EPSILON;
use super::occluder_classify::{BrushFrame, OccluderVerdict, OpenSpace, brush_polytope, judge_box};
use crate::map_data::BrushVolume;
use contact::{Hull, touching_pairs};
use coverage::covered_cells;
use section::{Member, find_sections};

pub use coverage::COVERAGE_GRID_CELLS;

/// A section occludes only when its solid covers strictly more than this
/// share of its bounding broad face: "mostly solid". Between a colonnade under
/// a beam (about a third) and a facade a third windows (about two thirds).
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
    /// The individual occluder rule on the bounding box.
    pub box_verdict: OccluderVerdict,
    /// Share of the bounding broad face the members cover, on the grid.
    pub coverage: f64,
    pub occludes: bool,
    /// Per member, one flag per brush plane: true when the plane lies on the
    /// bounding box, within the contact epsilon. Those planes bound the
    /// section in the occluder tier; the rest shape its openings.
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
            let half = DVec2::new(frame.extents[0], frame.extents[1]) * 0.5;
            let centre = DVec2::new(
                frame.center.dot(frame.axes[0]),
                frame.center.dot(frame.axes[1]),
            );
            let solids: Vec<&[DVec3]> =
                shape.members.iter().map(|&m| members[m].vertices).collect();
            let (covered, total) = covered_cells(
                [frame.axes[0], frame.axes[1]],
                centre - half,
                half * 2.0,
                &solids,
            );
            let coverage = covered as f64 / total as f64;
            // Strict, and exact on the integer cell counts (pin R12).
            let dense = covered as f64 > OCCLUDER_COVERAGE_FRACTION * total as f64;
            let mut in_slab: Vec<usize> = shape.members.iter().map(|&m| members[m].brush).collect();
            in_slab.sort_unstable();
            let outline = in_slab
                .iter()
                .map(|&brush| outline_planes(&brushes[brush], &frame))
                .collect();
            Section {
                members: in_slab,
                normal: frame.axes[SECTION_NORMAL_AXIS],
                extents: frame.extents,
                box_verdict,
                coverage,
                occludes: box_verdict == OccluderVerdict::Occludes && dense,
                outline,
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

/// One flag per plane of `brush`: true when the plane lies on a face of
/// `frame`'s box.
fn outline_planes(brush: &BrushVolume, frame: &BrushFrame) -> Vec<bool> {
    brush
        .planes
        .iter()
        .map(|plane| {
            (0..3).any(|axis| {
                let along = plane.normal.dot(frame.axes[axis]);
                if along.abs() < OUTLINE_PARALLEL_COS {
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
