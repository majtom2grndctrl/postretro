// Occluder classification of individual static brushes, from canonical brush
// geometry and segment queries against a first-pass BSP of every brush.
// See: context/plans/in-progress/occluder-first-partition/research.md §Occluder rule

use std::collections::HashSet;
use std::f64::consts::FRAC_1_SQRT_2;

use glam::DVec3;

use super::find_leaf_for_point;
use super::region_polytope::RegionPolytope;
use super::types::{Aabb, BspChild, BspTree};
use crate::map_data::BrushVolume;

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

/// An axis within 45° of up takes the see-over test; any other takes the
/// horizontal one.
const VERTICAL_AXIS_MIN_COS: f64 = FRAC_1_SQRT_2;

/// How far in front of a broad face its probes start. Small against the
/// floor, and well clear of the 1 mm region classification epsilon, so a probe
/// never starts inside the brush it measures.
const PROBE_OFFSET_M: f64 = 0.02;

/// Where a broad face is sampled along each of its broad axes: the centres
/// of its thirds, a 3 × 3 lattice. One centre sample misreads a floor with a
/// pillar on its centre, or a wall another wall abuts at its middle, as
/// fronting no open space.
const FACE_SAMPLE_FRACTIONS: [f64; 3] = [1.0 / 6.0, 0.5, 5.0 / 6.0];

/// Extents this close count as tied when choosing the broad face, so a cube
/// offers every face and the choice cannot depend on side order.
const AXIS_TIE_EPSILON_M: f64 = 1e-3;

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
        self.first_blocking(&root, origin, dir, 0.0, self.reach)
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

    fn first_blocking(
        &self,
        child: &BspChild,
        origin: DVec3,
        dir: DVec3,
        t0: f64,
        t1: f64,
    ) -> Option<f64> {
        match child {
            BspChild::Leaf(leaf) => {
                let blocked =
                    self.tree.leaves[*leaf].is_solid || self.exterior_leaves.contains(leaf);
                blocked.then_some(t0)
            }
            BspChild::Node(node) => {
                let node = &self.tree.nodes[*node];
                let side = |t: f64| (origin + dir * t).dot(node.plane_normal) - node.plane_distance;
                // Same sign rule as `find_leaf_for_point`: on the plane is front.
                let (d0, d1) = (side(t0), side(t1));
                if d0 >= 0.0 && d1 >= 0.0 {
                    return self.first_blocking(&node.front, origin, dir, t0, t1);
                }
                if d0 < 0.0 && d1 < 0.0 {
                    return self.first_blocking(&node.back, origin, dir, t0, t1);
                }
                let t_mid = t0 + (t1 - t0) * (d0 / (d0 - d1));
                let (near, far) = if d0 >= 0.0 {
                    (&node.front, &node.back)
                } else {
                    (&node.back, &node.front)
                };
                self.first_blocking(near, origin, dir, t0, t_mid)
                    .or_else(|| self.first_blocking(far, origin, dir, t_mid, t1))
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

/// Per-brush verdicts, index-aligned with the classified brush slice.
#[derive(Debug, Clone)]
pub struct OccluderClassification {
    pub verdicts: Vec<OccluderVerdict>,
}

impl OccluderClassification {
    /// One flag per brush, the shape `build_bsp_with_occluder_tier` takes.
    pub fn occluder_flags(&self) -> Vec<bool> {
        self.verdicts
            .iter()
            .map(|&verdict| verdict == OccluderVerdict::Occludes)
            .collect()
    }
}

/// Classify every brush. Each verdict reads only that brush's geometry and
/// `space`, so no brush's class depends on another's or on brush order.
pub fn classify_occluders(
    brushes: &[BrushVolume],
    space: &OpenSpace<'_>,
) -> OccluderClassification {
    OccluderClassification {
        verdicts: brushes
            .iter()
            .map(|brush| classify_brush(brush, space))
            .collect(),
    }
}

fn classify_brush(brush: &BrushVolume, space: &OpenSpace<'_>) -> OccluderVerdict {
    let Some(frame) = BrushFrame::of(brush) else {
        return OccluderVerdict::UnderFloor;
    };
    let thinnest = frame.extents.iter().copied().fold(f64::INFINITY, f64::min);

    // Exempt verdicts rank by how far the brush got; the furthest one wins.
    let mut verdict = OccluderVerdict::UnderFloor;
    for normal_axis in 0..3 {
        if frame.extents[normal_axis] > thinnest + AXIS_TIE_EPSILON_M {
            continue;
        }
        let broad = [(normal_axis + 1) % 3, (normal_axis + 2) % 3];
        if broad
            .iter()
            .any(|&axis| !at_least(frame.extents[axis], OCCLUDER_FLOOR_M))
        {
            continue;
        }
        verdict = rank(verdict, OccluderVerdict::NoOpenFace);
        for sign in [1.0, -1.0] {
            let face = BroadFace {
                normal: frame.axes[normal_axis] * sign,
                center: frame.center
                    + frame.axes[normal_axis] * (sign * frame.extents[normal_axis] * 0.5),
            };
            let face_verdict = face.judge(&frame, broad, space);
            if face_verdict == OccluderVerdict::Occludes {
                return face_verdict;
            }
            verdict = rank(verdict, face_verdict);
        }
    }
    verdict
}

fn rank(current: OccluderVerdict, candidate: OccluderVerdict) -> OccluderVerdict {
    fn order(verdict: OccluderVerdict) -> u8 {
        match verdict {
            OccluderVerdict::UnderFloor => 0,
            OccluderVerdict::NoOpenFace => 1,
            OccluderVerdict::ShortSpan => 2,
            OccluderVerdict::LowTop => 3,
            OccluderVerdict::Occludes => 4,
        }
    }
    if order(candidate) > order(current) {
        candidate
    } else {
        current
    }
}

fn at_least(measure: f64, constant: f64) -> bool {
    measure >= constant - RULE_EPSILON_M
}

/// One broad face of a brush's own-frame box, outward normal and centre.
struct BroadFace {
    normal: DVec3,
    center: DVec3,
}

impl BroadFace {
    /// Judge the face at each open sample of its lattice; it passes when
    /// most of them pass. A sample whose probe starts in blocked space reads
    /// the brush touching the face there, not the space in front, so it is
    /// skipped.
    fn judge(
        &self,
        frame: &BrushFrame,
        broad: [usize; 2],
        space: &OpenSpace<'_>,
    ) -> OccluderVerdict {
        let offset = |axis: usize, fraction: f64| {
            frame.axes[axis] * ((fraction - 0.5) * frame.extents[axis])
        };
        let (mut open, mut passing) = (0_usize, 0_usize);
        let mut failing = OccluderVerdict::NoOpenFace;
        for &a in &FACE_SAMPLE_FRACTIONS {
            for &b in &FACE_SAMPLE_FRACTIONS {
                let offsets = [offset(broad[0], a), offset(broad[1], b)];
                let probe = self.center + offsets[0] + offsets[1] + self.normal * PROBE_OFFSET_M;
                if space.is_blocked(probe) {
                    continue;
                }
                open += 1;
                match self.judge_sample(probe, offsets, frame, broad, space) {
                    OccluderVerdict::Occludes => passing += 1,
                    verdict => failing = rank(failing, verdict),
                }
            }
        }
        if open > 0 && 2 * passing > open {
            OccluderVerdict::Occludes
        } else {
            failing
        }
    }

    /// The rule at one sample: `probe` lies in front of the face, displaced
    /// from its centre by `offsets` along the two broad axes.
    fn judge_sample(
        &self,
        probe: DVec3,
        offsets: [DVec3; 2],
        frame: &BrushFrame,
        broad: [usize; 2],
        space: &OpenSpace<'_>,
    ) -> OccluderVerdict {
        let depth = PROBE_OFFSET_M + space.distance_to_blocking(probe, self.normal);
        if !at_least(depth, OCCLUDER_FLOOR_M) {
            return OccluderVerdict::NoOpenFace;
        }
        for (slot, axis) in broad.into_iter().enumerate() {
            let direction = frame.axes[axis];
            let length = frame.extents[axis];
            let upness = direction.dot(DVec3::Y);
            if upness.abs() >= VERTICAL_AXIS_MIN_COS {
                // Start just under the top: a top meeting a ceiling lies on
                // the ceiling's plane, which may descend into its solid.
                let up = direction * upness.signum();
                let top = self.center
                    + offsets[1 - slot]
                    + up * (length * 0.5)
                    + self.normal * PROBE_OFFSET_M;
                let start = top - DVec3::Y * PROBE_OFFSET_M;
                let see_over = PROBE_OFFSET_M + space.distance_to_blocking(start, DVec3::NEG_Y);
                if !at_least(see_over, OCCLUDER_SEE_OVER_M) {
                    return OccluderVerdict::LowTop;
                }
            } else {
                let across = (direction - DVec3::Y * upness).normalize();
                let span = space.distance_to_blocking(probe, across)
                    + space.distance_to_blocking(probe, -across);
                if !at_least(length, OCCLUDER_ABSOLUTE_LENGTH_M)
                    && !at_least(length, OCCLUDER_SPAN_FRACTION * span)
                {
                    return OccluderVerdict::ShortSpan;
                }
            }
        }
        OccluderVerdict::Occludes
    }
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
        let vertices = brush_vertices(brush);
        if vertices.is_empty() {
            return None;
        }

        let mut frames: Vec<Self> = Vec::new();
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

    fn fit(axes: [DVec3; 3], vertices: &[DVec3]) -> Self {
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
        let first = axis
            .to_array()
            .into_iter()
            .find(|component| component.abs() > 1e-9)
            .unwrap_or(1.0);
        if first < 0.0 {
            *axis = -*axis;
        }
    }
    axes.sort_by(|x, y| axis_order(*x, *y));
    axes
}

fn axis_order(x: DVec3, y: DVec3) -> std::cmp::Ordering {
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
    let pad = DVec3::ONE;
    let mut polytope = RegionPolytope::from_aabb(&Aabb {
        min: brush.aabb.min - pad,
        max: brush.aabb.max + pad,
    });
    for plane in &brush.planes {
        polytope = polytope.clip(plane.normal, plane.distance).1;
    }
    polytope.vertices().collect()
}

// The rows these fixtures serve run in `portals::occluder_tier`: they need the
// exterior flood, which only the `prl-build` target compiles, so the library
// target's test build leaves the fixtures unused.
#[cfg(test)]
#[allow(dead_code)]
pub(crate) mod test_fixtures;
