// Occluder classification of individual static brushes, from canonical brush
// geometry and segment queries against a first-pass BSP of every brush.
// See: context/plans/in-progress/occluder-first-partition/research.md §Occluder rule

mod broad_face;

use std::collections::HashSet;

use glam::DVec3;

use super::brush_bsp::{REGION_CLASSIFICATION_EPSILON, TierRole};
use super::find_leaf_for_point;
use super::occluder_cluster::{Cluster, find_clusters};
use super::region_polytope::RegionPolytope;
use super::types::{Aabb, BspChild, BspTree};
use crate::map_data::BrushVolume;

pub(crate) use broad_face::{
    broad_face_floor, broad_face_open_depth, judge_box, vertical_broad_axis,
};

/// Least broad-axis length, and least open depth in front of a broad face,
/// that counts. Three player widths (capsule radius 0.2 m): a shorter axis
/// hides little more than a player, and a face fronting a shallower gap faces
/// no space a player stands in.
pub const OCCLUDER_FLOOR_M: f64 = 1.2;

/// A horizontal broad axis at least this long passes outright, so a long wall
/// or building occludes where its open span is too wide for the share test.
pub const OCCLUDER_ABSOLUTE_LENGTH_M: f64 = 20.0;

/// A horizontal broad axis covering at least this share of the open span in
/// front of its face passes: occlusion value scales with the share of the
/// space a brush blocks.
pub const OCCLUDER_SPAN_FRACTION: f64 = 0.5;

/// A vertical broad axis passes when its top stands at least this far above
/// the floor in front of its face: eye height 1.5 m plus jump reach.
pub const OCCLUDER_SEE_OVER_M: f64 = 2.5;

/// The see-over height the rule and the cluster roofline read:
/// `OCCLUDER_SEE_OVER_M`, or in tests a per-thread override that changes
/// classification with every brush's geometry held fixed (pin R16).
pub(crate) fn see_over_m() -> f64 {
    #[cfg(test)]
    if let Some(height) = see_over_override::get() {
        return height;
    }
    OCCLUDER_SEE_OVER_M
}

#[cfg(test)]
pub(crate) mod see_over_override {
    use std::cell::Cell;

    thread_local! {
        static HEIGHT: Cell<Option<f64>> = const { Cell::new(None) };
    }

    pub(super) fn get() -> Option<f64> {
        HEIGHT.with(Cell::get)
    }

    /// Run `f` with the see-over height set to `height` on this thread.
    #[allow(dead_code)]
    pub(crate) fn with<R>(height: f64, f: impl FnOnce() -> R) -> R {
        struct Restore(Option<f64>);
        impl Drop for Restore {
            fn drop(&mut self) {
                HEIGHT.with(|cell| cell.set(self.0));
            }
        }
        let _restore = Restore(HEIGHT.with(|cell| cell.replace(Some(height))));
        f()
    }
}

/// How far in front of a broad face its probes start. Small against the
/// floor, and well clear of the 1 mm region classification epsilon, so a probe
/// never starts inside the brush it measures.
pub(crate) const PROBE_OFFSET_M: f64 = 0.02;

/// Where a broad face is sampled along each of its broad axes: the centres
/// of its thirds, a 3 × 3 lattice. One centre sample misreads a floor with a
/// pillar on its centre, or a wall another wall abuts at its middle, as
/// fronting no open space.
pub(crate) const FACE_SAMPLE_FRACTIONS: [f64; 3] = [1.0 / 6.0, 0.5, 5.0 / 6.0];

/// Extents this close count as tied when choosing the broad face, so a cube
/// offers every face and the choice cannot depend on side order.
pub(crate) const AXIS_TIE_EPSILON_M: f64 = 1e-3;

/// Side normals whose dot product is within this of zero are perpendicular
/// and seed a candidate own-frame box.
const PERPENDICULAR_EPSILON: f64 = 1e-4;

/// Slack on every rule comparison, so a measure exactly at a constant passes
/// despite unit-scale rounding: the occluder tests are inclusive.
const RULE_EPSILON_M: f64 = 1e-6;

/// Open space as a first-pass tree sees it: solid leaves and exterior leaves
/// block, every other leaf is open.
pub struct OpenSpace<'a> {
    tree: &'a BspTree,
    exterior_leaves: &'a HashSet<usize>,
    /// Farthest a segment query looks: twice the extent of every leaf, so an
    /// unbounded line still returns a finite, order-free span.
    reach: f64,
}

impl<'a> OpenSpace<'a> {
    pub fn new(tree: &'a BspTree, exterior_leaves: &'a HashSet<usize>) -> Self {
        let mut bounds = Aabb::empty();
        for leaf in &tree.leaves {
            if leaf.bounds.is_valid() {
                bounds.expand_aabb(&leaf.bounds);
            }
        }
        let reach = if bounds.is_valid() {
            2.0 * (bounds.max - bounds.min).length()
        } else {
            0.0
        };
        Self {
            tree,
            exterior_leaves,
            reach,
        }
    }

    /// Distance along unit `dir` from `origin` to the first solid or exterior
    /// leaf, capped at `reach`. Zero when `origin` itself is blocked.
    pub fn distance_to_blocking(&self, origin: DVec3, dir: DVec3) -> f64 {
        if self.tree.leaves.is_empty() {
            return 0.0;
        }
        let root = if self.tree.nodes.is_empty() {
            BspChild::Leaf(0)
        } else {
            BspChild::Node(0)
        };
        self.first_leaf(&root, origin, dir, 0.0, self.reach, true)
            .unwrap_or(self.reach)
    }

    /// Distance along unit `dir` from `origin`, through any solid or exterior
    /// `origin` starts in, then across open space to the next solid or
    /// exterior leaf, capped at `reach`: how far a top that meets or sinks
    /// into a ceiling stands above the floor below it.
    pub fn distance_through_to_blocking(&self, origin: DVec3, dir: DVec3) -> f64 {
        if self.tree.leaves.is_empty() {
            return 0.0;
        }
        let root = if self.tree.nodes.is_empty() {
            BspChild::Leaf(0)
        } else {
            BspChild::Node(0)
        };
        let Some(open) = self.first_leaf(&root, origin, dir, 0.0, self.reach, false) else {
            return self.reach;
        };
        // Past the solid's face, so the search never starts on its plane.
        let from = if open > 0.0 {
            open + PROBE_OFFSET_M
        } else {
            0.0
        };
        if from >= self.reach {
            return self.reach;
        }
        self.first_leaf(&root, origin, dir, from, self.reach, true)
            .unwrap_or(self.reach)
    }

    /// True when `point` lies in a solid or exterior leaf.
    pub fn is_blocked(&self, point: DVec3) -> bool {
        if self.tree.leaves.is_empty() {
            return true;
        }
        let leaf = find_leaf_for_point(self.tree, point);
        self.tree.leaves[leaf].is_solid || self.exterior_leaves.contains(&leaf)
    }

    /// Bounds of the open space inside `bounds`: the union of each open
    /// leaf's bounds clipped to `bounds`, counting only leaves that reach
    /// past the region classification epsilon into it on every axis. Empty
    /// when none does.
    pub fn open_bounds_within(&self, bounds: &Aabb) -> Aabb {
        let mut open = Aabb::empty();
        if self.tree.leaves.is_empty() || !bounds.is_valid() {
            return open;
        }
        let (centre, half) = (bounds.centroid(), (bounds.max - bounds.min) * 0.5);
        let mut pending = vec![if self.tree.nodes.is_empty() {
            BspChild::Leaf(0)
        } else {
            BspChild::Node(0)
        }];
        while let Some(child) = pending.pop() {
            match child {
                BspChild::Leaf(index) => {
                    let leaf = &self.tree.leaves[index];
                    if leaf.is_solid || self.exterior_leaves.contains(&index) {
                        continue;
                    }
                    let clipped = Aabb {
                        min: leaf.bounds.min.max(bounds.min),
                        max: leaf.bounds.max.min(bounds.max),
                    };
                    if (clipped.max - clipped.min)
                        .cmpgt(DVec3::splat(REGION_CLASSIFICATION_EPSILON))
                        .all()
                    {
                        open.expand_aabb(&clipped);
                    }
                }
                BspChild::Node(index) => {
                    let node = &self.tree.nodes[index];
                    let side = centre.dot(node.plane_normal) - node.plane_distance;
                    let reach = node.plane_normal.abs().dot(half);
                    // Same sign rule as `find_leaf_for_point`: on the plane is front.
                    if side + reach >= 0.0 {
                        pending.push(node.front.clone());
                    }
                    if side - reach < 0.0 {
                        pending.push(node.back.clone());
                    }
                }
            }
        }
        open
    }

    /// First parameter in `t0..t1` along the segment where it enters a leaf
    /// whose blocked state (solid or exterior) is `blocked`.
    fn first_leaf(
        &self,
        child: &BspChild,
        origin: DVec3,
        dir: DVec3,
        t0: f64,
        t1: f64,
        blocked: bool,
    ) -> Option<f64> {
        match child {
            BspChild::Leaf(leaf) => {
                let is_blocked =
                    self.tree.leaves[*leaf].is_solid || self.exterior_leaves.contains(leaf);
                (is_blocked == blocked).then_some(t0)
            }
            BspChild::Node(node) => {
                let node = &self.tree.nodes[*node];
                let side = |t: f64| (origin + dir * t).dot(node.plane_normal) - node.plane_distance;
                // Same sign rule as `find_leaf_for_point`: on the plane is front.
                let (d0, d1) = (side(t0), side(t1));
                if d0 >= 0.0 && d1 >= 0.0 {
                    return self.first_leaf(&node.front, origin, dir, t0, t1, blocked);
                }
                if d0 < 0.0 && d1 < 0.0 {
                    return self.first_leaf(&node.back, origin, dir, t0, t1, blocked);
                }
                let t_mid = t0 + (t1 - t0) * (d0 / (d0 - d1));
                let (near, far) = if d0 >= 0.0 {
                    (&node.front, &node.back)
                } else {
                    (&node.back, &node.front)
                };
                self.first_leaf(near, origin, dir, t0, t_mid, blocked)
                    .or_else(|| self.first_leaf(far, origin, dir, t_mid, t1, blocked))
            }
        }
    }
}

/// One brush's classification. Exempt verdicts name the test the brush came
/// closest to passing on, for the classifier's report.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OccluderVerdict {
    Occludes,
    /// Every candidate broad face has an axis under the floor.
    UnderFloor,
    /// No broad face fronts open space at least the floor deep.
    NoOpenFace,
    /// A counted face's horizontal axis is under both the absolute length and
    /// the share of its open span.
    ShortSpan,
    /// A counted face's vertical axis tops out under the see-over height.
    LowTop,
}

/// Per-brush verdicts, index-aligned with the classified brush slice, and the
/// clusters their exempt brushes form.
#[derive(Debug, Clone, Default)]
pub struct OccluderClassification {
    /// Each brush judged alone.
    pub verdicts: Vec<OccluderVerdict>,
    /// Touching brushes each exempt alone, judged per near-planar section.
    pub clusters: Vec<Cluster>,
}

impl OccluderClassification {
    /// One role per brush, the shape `build_bsp_with_occluder_tier` takes: a
    /// brush that occludes alone is an occluder; an in-slab member of an
    /// occluding section splits in the tier on its section's bounding face
    /// and box edges only; every other brush splits only beneath the tier.
    pub fn tier_roles(&self) -> Vec<TierRole> {
        let mut roles: Vec<TierRole> = self
            .verdicts
            .iter()
            .map(|&verdict| match verdict {
                OccluderVerdict::Occludes => TierRole::Occluder,
                _ => TierRole::Exempt,
            })
            .collect();
        for section in self.clusters.iter().flat_map(|cluster| &cluster.sections) {
            if !section.occludes {
                continue;
            }
            for (&member, outline) in section.members.iter().zip(&section.outline) {
                match &mut roles[member] {
                    role @ TierRole::Exempt => *role = TierRole::SectionMember(outline.clone()),
                    // A member of two occluding sections bounds both.
                    TierRole::SectionMember(flags) => {
                        for (flag, &on_box) in flags.iter_mut().zip(outline) {
                            *flag |= on_box;
                        }
                    }
                    TierRole::Occluder => {}
                }
            }
        }
        roles
    }

    /// One flag per brush: true when any of its planes split inside the
    /// occluder tier.
    pub fn occluder_flags(&self) -> Vec<bool> {
        self.tier_roles()
            .iter()
            .map(|role| *role != TierRole::Exempt)
            .collect()
    }
}

/// Classify every brush, then judge the clusters of touching exempt brushes.
/// Each verdict reads only that brush's geometry and `space`, and clusters
/// are sets judged on their own geometry, so nothing depends on brush order.
pub fn classify_occluders(
    brushes: &[BrushVolume],
    space: &OpenSpace<'_>,
) -> OccluderClassification {
    let verdicts: Vec<OccluderVerdict> = brushes
        .iter()
        .map(|brush| classify_brush(brush, space))
        .collect();
    let clusters = find_clusters(brushes, &verdicts, space);
    OccluderClassification { verdicts, clusters }
}

fn classify_brush(brush: &BrushVolume, space: &OpenSpace<'_>) -> OccluderVerdict {
    let Some(frame) = BrushFrame::of(brush) else {
        return OccluderVerdict::UnderFloor;
    };
    let thinnest = frame.extents.iter().copied().fold(f64::INFINITY, f64::min);
    let normal_axes = (0..3).filter(|&axis| frame.extents[axis] <= thinnest + AXIS_TIE_EPSILON_M);
    judge_box(&frame, normal_axes, space)
}

pub(crate) fn at_least(measure: f64, constant: f64) -> bool {
    measure >= constant - RULE_EPSILON_M
}

/// True when `a` and `b` differ by no more than the rule's slack.
pub(crate) fn within_rule_slack(a: f64, b: f64) -> bool {
    (a - b).abs() <= RULE_EPSILON_M
}

/// A brush's own-frame box: the minimum-volume box over every pair of its
/// perpendicular side normals, so a rotated brush measures as it would
/// unrotated. Brushes with no perpendicular sides measure on world axes.
#[derive(Debug, Clone)]
pub(crate) struct BrushFrame {
    pub(crate) axes: [DVec3; 3],
    pub(crate) extents: [f64; 3],
    pub(crate) center: DVec3,
}

impl BrushFrame {
    pub(crate) fn of(brush: &BrushVolume) -> Option<Self> {
        Self::of_brushes(std::slice::from_ref(brush))
    }

    /// The own-frame box of a union of brushes, such as a mover's closed
    /// pose: the minimum-volume box over every brush's perpendicular side
    /// pairs, fitted to every brush's corners.
    pub(crate) fn of_brushes(brushes: &[BrushVolume]) -> Option<Self> {
        let vertices: Vec<DVec3> = brushes.iter().flat_map(brush_vertices).collect();
        if vertices.is_empty() {
            return None;
        }

        let mut frames: Vec<Self> = Vec::new();
        for brush in brushes {
            for (i, a) in brush.planes.iter().enumerate() {
                for b in &brush.planes[i + 1..] {
                    if a.normal.dot(b.normal).abs() > PERPENDICULAR_EPSILON {
                        continue;
                    }
                    let u = a.normal.normalize();
                    let v = (b.normal - u * u.dot(b.normal)).normalize();
                    frames.push(Self::fit(canonical_axes([u, v, u.cross(v)]), &vertices));
                }
            }
        }
        if frames.is_empty() {
            frames.push(Self::fit([DVec3::X, DVec3::Y, DVec3::Z], &vertices));
        }

        // Smallest volume, ties broken by an exact order on the axes, so the
        // result is the same whatever order the sides arrive in.
        let least = frames
            .iter()
            .map(Self::volume)
            .fold(f64::INFINITY, f64::min);
        let tolerance = least * 1e-9 + 1e-12;
        frames
            .into_iter()
            .filter(|frame| frame.volume() <= least + tolerance)
            .min_by(|x, y| axes_order(&x.axes, &y.axes))
    }

    /// The box along `axes` enclosing `vertices`.
    pub(crate) fn fit(axes: [DVec3; 3], vertices: &[DVec3]) -> Self {
        let mut extents = [0.0; 3];
        let mut center = DVec3::ZERO;
        for (axis, extent) in axes.iter().zip(extents.iter_mut()) {
            let (lo, hi) =
                vertices
                    .iter()
                    .fold((f64::INFINITY, f64::NEG_INFINITY), |(lo, hi), vertex| {
                        let projection = vertex.dot(*axis);
                        (lo.min(projection), hi.max(projection))
                    });
            *extent = hi - lo;
            center += *axis * ((lo + hi) * 0.5);
        }
        Self {
            axes,
            extents,
            center,
        }
    }

    fn volume(&self) -> f64 {
        self.extents.iter().product()
    }
}

/// Sign each axis so its first non-negligible component is positive, then
/// sort, so the same box from any side pair compares equal.
fn canonical_axes(mut axes: [DVec3; 3]) -> [DVec3; 3] {
    for axis in &mut axes {
        *axis = canonical_direction(*axis);
    }
    axes.sort_by(|x, y| axis_order(*x, *y));
    axes
}

/// `direction` or its negation, whichever has a positive first
/// non-negligible component, so a line's two directions compare equal.
pub(crate) fn canonical_direction(direction: DVec3) -> DVec3 {
    let first = direction
        .to_array()
        .into_iter()
        .find(|component| component.abs() > 1e-9)
        .unwrap_or(1.0);
    if first < 0.0 { -direction } else { direction }
}

/// An exact total order on directions, for order-free tie-breaks.
pub(crate) fn axis_order(x: DVec3, y: DVec3) -> std::cmp::Ordering {
    x.x.total_cmp(&y.x)
        .then(x.y.total_cmp(&y.y))
        .then(x.z.total_cmp(&y.z))
}

fn axes_order(x: &[DVec3; 3], y: &[DVec3; 3]) -> std::cmp::Ordering {
    axis_order(x[0], y[0])
        .then(axis_order(x[1], y[1]))
        .then(axis_order(x[2], y[2]))
}

/// The brush's corner points, cut from its padded AABB by its own planes, so
/// they come from canonical planes alone and not from textured sides.
fn brush_vertices(brush: &BrushVolume) -> Vec<DVec3> {
    brush_polytope(brush).vertices().collect()
}

/// The brush as a convex polytope cut from its padded AABB by its planes.
pub(crate) fn brush_polytope(brush: &BrushVolume) -> RegionPolytope {
    let pad = DVec3::ONE;
    let mut polytope = RegionPolytope::from_aabb(&Aabb {
        min: brush.aabb.min - pad,
        max: brush.aabb.max + pad,
    });
    for plane in &brush.planes {
        polytope = polytope.clip(plane.normal, plane.distance).1;
    }
    polytope
}

// The rows these fixtures serve run in `portals::occluder_tier`: they need the
// exterior flood, which only the `prl-build` target compiles, so the library
// target's test build leaves the fixtures unused.
#[cfg(test)]
#[allow(dead_code)]
pub(crate) mod test_fixtures;
