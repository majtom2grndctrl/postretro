// Near-planar sections of an occluder cluster: touching members whose
// broad-face normals agree, kept to a slab they share, measured in the
// section's own frame.
// See: context/plans/in-progress/occluder-first-partition/plan.md §Delegated answers (Clusters)

use std::cmp::Ordering;
use std::collections::BTreeMap;

use glam::DVec3;

use crate::partition::brush_bsp::REGION_CLASSIFICATION_EPSILON;
use crate::partition::occluder_classify::{
    BrushFrame, axis_order, canonical_direction, thinnest_axes,
};

/// Broad-face normals within this angle of a section's normal agree with it,
/// so a facade a few degrees off true still reads as one plane while the arms
/// of a corner, 90° apart, read as two sections. 10°. The same angle decides
/// which member planes can bound a face of the section's box in the occluder
/// tier (`outline`), since members join a section within it.
pub(super) const SECTION_NORMAL_MAX_ANGLE_COS: f64 = 0.984_807_753_012_208;

/// Candidate normals this close to parallel are one normal: 1 − 1e-9, about
/// 4.5e-5 rad (9 arc seconds). Tight enough that authored normals any real
/// turn sets apart stay distinct, each tried in turn; loose enough to fuse
/// one direction re-derived from different members' frames, whose rounding
/// lies orders of magnitude under it.
const NORMAL_PARALLEL_COS: f64 = 1.0 - 1e-9;

/// In-plane frame axes this close to parallel are one candidate direction,
/// for the same reasons and at the same angle as `NORMAL_PARALLEL_COS`.
const FRAME_AXIS_PARALLEL_COS: f64 = 1.0 - 1e-9;

/// Least share of the member face area within a slab that the members
/// sharing the slab's own interval must carry for it to stand as a slab.
const SLAB_SHARE: f64 = 0.5;

/// Relative slack on area comparisons: far over the rounding in a member's
/// own-frame extents, far under any real difference.
const TIE_RELATIVE: f64 = 1e-9;

/// One cluster member as section finding reads it.
pub(super) struct Member<'a> {
    /// Index into the classified brush slice.
    pub(super) brush: usize,
    pub(super) frame: &'a BrushFrame,
    pub(super) vertices: &'a [DVec3],
}

impl Member<'_> {
    /// The member's broad-face normals: its thinnest own-frame axes, every
    /// one of them when extents tie, as the classifier offers broad faces.
    fn broad_normals(&self) -> impl Iterator<Item = DVec3> + '_ {
        thinnest_axes(self.frame)
            .into_iter()
            .map(|axis| canonical_direction(self.frame.axes[axis]))
    }

    fn agrees_with(&self, normal: DVec3) -> bool {
        self.broad_normals()
            .any(|own| own.dot(normal).abs() >= SECTION_NORMAL_MAX_ANGLE_COS)
    }

    fn interval(&self, normal: DVec3) -> (f64, f64) {
        self.vertices
            .iter()
            .fold((f64::INFINITY, f64::NEG_INFINITY), |(lo, hi), v| {
                let p = v.dot(normal);
                (lo.min(p), hi.max(p))
            })
    }

    /// Face area across `normal`: the own-frame extents of the two axes
    /// other than the one nearest `normal`.
    fn face_area(&self, normal: DVec3) -> f64 {
        let across = (0..3)
            .max_by(|&a, &b| {
                let dot = |axis: usize| self.frame.axes[axis].dot(normal).abs();
                dot(a).total_cmp(&dot(b)).then(b.cmp(&a))
            })
            .expect("three axes");
        (0..3)
            .filter(|&axis| axis != across)
            .map(|axis| self.frame.extents[axis])
            .product()
    }
}

/// A section before it is judged: its members (indices into the cluster's
/// member list) and its own frame under each normal it forms under, the
/// normal last in each, in the candidates' exact order.
pub(super) struct SectionShape {
    pub(super) members: Vec<usize>,
    pub(super) frames: Vec<BrushFrame>,
}

/// Find a cluster's sections. `members` is the cluster; `touching[i]` lists
/// the members touching member `i`. For each candidate normal, the members
/// agreeing with it split into touching components; each of the component's
/// slabs keeps the members lying within it, which split again into touching
/// components, and each of two or more members is a section. A set that
/// forms under several normals is one section, judged under each: which
/// normal comes first in world order must not decide it, or turning a map
/// would change its verdict, and the judgment that stands for it is chosen
/// by geometry (`standing_judgment`). Every step works on sets, so the
/// result does not depend on member order.
pub(super) fn find_sections(members: &[Member<'_>], touching: &[Vec<usize>]) -> Vec<SectionShape> {
    let mut normals_by_set: BTreeMap<Vec<usize>, Vec<DVec3>> = BTreeMap::new();
    for normal in candidate_normals(members) {
        let agreeing: Vec<bool> = members.iter().map(|m| m.agrees_with(normal)).collect();
        for component in components(touching, &agreeing) {
            if component.len() < 2 {
                continue;
            }
            for slab in slabs(members, &component, normal) {
                let mut in_slab = vec![false; members.len()];
                for &i in &component {
                    in_slab[i] = within(members[i].interval(normal), slab);
                }
                for section in components(touching, &in_slab) {
                    if section.len() < 2 {
                        continue;
                    }
                    let normals = normals_by_set.entry(section).or_default();
                    // Nested slabs can yield one set twice under one normal.
                    if normals.last() != Some(&normal) {
                        normals.push(normal);
                    }
                }
            }
        }
    }
    normals_by_set
        .into_iter()
        .map(|(section, normals)| SectionShape {
            frames: normals
                .into_iter()
                .map(|normal| section_frame(members, &section, normal))
                .collect(),
            members: section,
        })
        .collect()
}

/// Every distinct broad-face normal of every member, in an exact order.
/// Each is tried: one standing for every normal within the section angle
/// would be the first in world order, so a member a few degrees off could
/// take a facade's normal from it in one orientation and not in another.
fn candidate_normals(members: &[Member<'_>]) -> Vec<DVec3> {
    let mut normals: Vec<DVec3> = members.iter().flat_map(Member::broad_normals).collect();
    normals.sort_by(|a, b| axis_order(*a, *b));
    let mut distinct: Vec<DVec3> = Vec::new();
    for normal in normals {
        // One direction re-derived from different members is one normal; the
        // first in order stays.
        if !distinct
            .iter()
            .any(|kept| kept.dot(normal).abs() >= NORMAL_PARALLEL_COS)
        {
            distinct.push(normal);
        }
    }
    distinct
}

/// Touching components among the members `keep` marks, each sorted.
fn components(touching: &[Vec<usize>], keep: &[bool]) -> Vec<Vec<usize>> {
    let mut visited = vec![false; keep.len()];
    let mut out = Vec::new();
    for start in 0..keep.len() {
        if !keep[start] || visited[start] {
            continue;
        }
        visited[start] = true;
        let mut stack = vec![start];
        let mut component = Vec::new();
        while let Some(i) = stack.pop() {
            component.push(i);
            for &j in &touching[i] {
                if keep[j] && !visited[j] {
                    visited[j] = true;
                    stack.push(j);
                }
            }
        }
        component.sort_unstable();
        out.push(component);
    }
    out
}

fn within((lo, hi): (f64, f64), (slab_lo, slab_hi): (f64, f64)) -> bool {
    lo >= slab_lo - REGION_CLASSIFICATION_EPSILON && hi <= slab_hi + REGION_CLASSIFICATION_EPSILON
}

/// The component's slabs along `normal`: each interval its members share,
/// within the contact epsilon at both ends, that is its own layer's, not a
/// deeper member's happening to enclose a thinner layer. A shared interval
/// stands when the members sharing it carry at least `SLAB_SHARE` of the
/// face area lying within it. So a facade's strips set their slab beside a
/// block behind them, and a lamp proud of them, a block pushed through them
/// or a pillar on a tiled floor falls outside it.
fn slabs(members: &[Member<'_>], component: &[usize], normal: DVec3) -> Vec<(f64, f64)> {
    let same = |a: (f64, f64), b: (f64, f64)| {
        (a.0 - b.0).abs() <= REGION_CLASSIFICATION_EPSILON
            && (a.1 - b.1).abs() <= REGION_CLASSIFICATION_EPSILON
    };
    let mut intervals: Vec<(f64, f64)> = component
        .iter()
        .map(|&i| members[i].interval(normal))
        .collect();
    // Sorted before merging, so which copy stands for an interval does not
    // depend on member order.
    intervals.sort_by(|a, b| a.0.total_cmp(&b.0).then(a.1.total_cmp(&b.1)));
    let mut distinct: Vec<(f64, f64)> = Vec::new();
    for interval in intervals {
        if !distinct.iter().any(|&kept| same(kept, interval)) {
            distinct.push(interval);
        }
    }
    distinct
        .into_iter()
        .filter(|&slab| {
            let (mut shared, mut held) = (0.0, 0.0);
            for &i in component {
                let interval = members[i].interval(normal);
                if within(interval, slab) {
                    let area = members[i].face_area(normal);
                    held += area;
                    if same(interval, slab) {
                        shared += area;
                    }
                }
            }
            // Areas carry rounding from each member's own frame; the slack
            // keeps a share exactly at the bound on one side in every order.
            shared >= SLAB_SHARE * held * (1.0 - TIE_RELATIVE)
        })
        .collect()
}

/// The section's own frame `[u, v, normal]`: of the in-plane directions its
/// members' own frames offer, the one whose rectangle round the projected
/// members is smallest, so a facade turned 45° measures as it would square.
fn section_frame(members: &[Member<'_>], section: &[usize], normal: DVec3) -> BrushFrame {
    let vertices: Vec<DVec3> = section
        .iter()
        .flat_map(|&i| members[i].vertices.iter().copied())
        .collect();

    let mut candidates: Vec<DVec3> = Vec::new();
    for &i in section {
        for axis in members[i].frame.axes {
            let in_plane = axis - normal * axis.dot(normal);
            let Some(u) = in_plane.try_normalize() else {
                continue;
            };
            if axis.dot(normal).abs() >= SECTION_NORMAL_MAX_ANGLE_COS {
                continue;
            }
            let u = canonical_direction(u);
            let v = canonical_direction(normal.cross(u));
            // u and v span the same rectangle; keep the one first in order.
            candidates.push(if axis_order(v, u) == Ordering::Less {
                v
            } else {
                u
            });
        }
    }
    // Sorted before merging, so which near-parallel copy stands for a
    // direction does not depend on member order.
    candidates.sort_by(|a, b| axis_order(*a, *b));
    candidates.dedup_by(|later, earlier| later.dot(*earlier).abs() >= FRAME_AXIS_PARALLEL_COS);

    let frame_for = |u: DVec3| {
        let v = canonical_direction(normal.cross(u));
        BrushFrame::fit([u, v, normal], &vertices)
    };
    let area = |frame: &BrushFrame| frame.extents[0] * frame.extents[1];
    let frames: Vec<BrushFrame> = if candidates.is_empty() {
        let fallback = canonical_direction(normal.any_orthonormal_vector());
        vec![frame_for(fallback)]
    } else {
        candidates.into_iter().map(frame_for).collect()
    };
    let least = frames.iter().map(area).fold(f64::INFINITY, f64::min);
    let tolerance = least * 1e-9 + 1e-12;
    frames
        .into_iter()
        .find(|frame| area(frame) <= least + tolerance)
        .expect("at least one frame")
}
