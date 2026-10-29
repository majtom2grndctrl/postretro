// Shadow prisms of the occluder tier: the space an occluding wall face hides,
// its broad face swept along its normal, whose free vertical end planes split
// the regions the prism overlaps after occluder planes and before door and
// cap planes.
// See: context/plans/in-progress/occluder-first-partition/index.md §Decisions

use glam::DVec3;

use super::{REGION_CLASSIFICATION_EPSILON, planes_equivalent};
use crate::map_data::BrushVolume;
use crate::partition::occluder_classify::{
    BrushFrame, OccluderClassification, OccluderVerdict, OpenSpace, PROBE_OFFSET_M,
    broad_axes_reach_floor, broad_face_counts, broad_face_counts_through_solid,
    broad_face_deepest_open_depth, thinnest_axes, vertical_broad_axis,
};
use crate::partition::region_polytope::RegionPolytope;
use crate::partition::types::Aabb;

/// A prism ends at most this multiple of its face's longer broad extent from
/// the face. An eye past a wall's end sees into its prism a width growing
/// with depth over distance in front, so the deep parts of a prism are
/// hidden from fewer eyes; the bound is a cost limit on how far a face's end
/// planes cut the space behind it, scaled with the face as that geometry is.
pub const SHADOW_PRISM_LENGTH_FACTOR: f64 = 2.0;

/// How far past an end edge's plane the free-end probes sit, beyond the wall.
pub const FREE_END_PROBE_OFFSET_M: f64 = 0.1;

/// Where along an end edge the free-end probes sit, as fractions of its
/// length from one end.
pub const FREE_END_PROBE_FRACTIONS: [f64; 5] = [0.1, 0.3, 0.5, 0.7, 0.9];

/// How far behind the face the free-end probes sit. All are under the
/// occluder floor, the shallowest a prism reaches, so every probe stands
/// within the prism's depth.
pub const FREE_END_PROBE_DEPTHS_M: [f64; 3] = [0.1, 0.5, 1.0];

/// An end edge is free when most of its probes are open: 8 of 15. Solid and
/// exterior void both block, so a wall end meeting another wall, or running
/// into the void, splits nothing.
pub const FREE_END_MIN_OPEN_PROBES: usize =
    FREE_END_PROBE_FRACTIONS.len() * FREE_END_PROBE_DEPTHS_M.len() / 2 + 1;

/// How far past an end edge's plane the joint probes sit, inside the wall's
/// own thickness. The free-end probes stand farther out and in front of the
/// face, so a wall continuing this one in line, or a cross wall thinner than
/// their offset, leaves them open.
const JOINT_PROBE_OFFSET_M: f64 = PROBE_OFFSET_M;

/// Where across the wall's thickness the joint probes sit, as fractions of
/// it from one broad face; along the end they sit at
/// `FREE_END_PROBE_FRACTIONS`.
const JOINT_PROBE_THICKNESS_FRACTIONS: [f64; 3] = [0.25, 0.5, 0.75];

/// Depth a region must reach into a prism to overlap it: the tolerance by
/// which a plane must span a region to split it, so a region reaching less
/// far in, which no end plane could cut a piece of, is not offered them.
const OVERLAP_EPSILON: f64 = REGION_CLASSIFICATION_EPSILON;

/// The space one occluding wall face hides from an eye in front of the
/// opposite face and within its ends: the face swept along its outward
/// normal to the far edge of the open space in front of it, or to the length
/// bound, whichever is nearer. Its splitters are the face's free vertical
/// end edges; the prism itself only decides where they are offered.
#[derive(Debug, Clone)]
pub struct ShadowPrism {
    /// The tier splitters, inward to the prism, so a point on one files with
    /// the hidden side: the plane of each vertical end edge of the face with
    /// open space beyond it. Never empty.
    pub(crate) splitters: Vec<(DVec3, f64)>,
    /// Outward half-spaces bounding the prism, for the overlap test: the
    /// face's plane, the far end, then the four edge planes.
    hull: [(DVec3, f64); 6],
    bounds: Aabb,
    /// How far the prism reaches from its face.
    pub depth: f64,
    /// True when the length bound, not the open space, ends the prism.
    pub truncated: bool,
}

/// The shadow prisms of a classification: each wall face (one with a
/// vertical broad axis) of an occluding brush that counts under the occluder
/// rule's face test while its opposite broad face does too, read through any
/// solid backing it, and has at least one free vertical end. Faces follow
/// the classifier's tie rule, every face across each thinnest axis whose
/// broad axes reach the floor. Slab faces, wall tops and bottoms, cluster
/// sections and far ends add no splitter. Every prism reads only its own
/// geometry and `space`, and the set is sorted and deduplicated by its hull,
/// so it does not depend on brush order.
pub fn shadow_prisms(
    brushes: &[BrushVolume],
    classification: &OccluderClassification,
    space: &OpenSpace<'_>,
) -> Vec<ShadowPrism> {
    let mut prisms = Vec::new();
    for (brush, &verdict) in brushes.iter().zip(&classification.verdicts) {
        if verdict != OccluderVerdict::Occludes {
            continue;
        }
        let Some(frame) = BrushFrame::of(brush) else {
            continue;
        };
        for normal_axis in thinnest_axes(&frame) {
            if !broad_axes_reach_floor(&frame, normal_axis) {
                continue;
            }
            let Some(vertical_axis) = vertical_broad_axis(&frame, normal_axis) else {
                continue;
            };
            for outward in [1.0, -1.0] {
                prisms.extend(ShadowPrism::of_wall_face(
                    &frame,
                    normal_axis,
                    vertical_axis,
                    outward,
                    space,
                ));
            }
        }
    }
    prisms.sort_by(|a, b| hull_order(&a.hull, &b.hull));
    prisms.dedup_by(|later, earlier| {
        later
            .hull
            .iter()
            .zip(&earlier.hull)
            .all(|(&x, &y)| planes_equivalent(x, y))
    });
    prisms
}

impl ShadowPrism {
    /// The prism of the wall face of `frame`'s box whose outward normal is
    /// `outward` (±1) along `normal_axis`, its ends running along
    /// `vertical_axis`. `None` when the face, or the opposite face, does not
    /// count under the occluder rule's face test, or when neither end is
    /// free. The prism is the space hidden from eyes in front of the opposite
    /// face, and no eye stands in a gap under the floor or in void: a room's
    /// own wall, its outer face on the void, hides the room from no one, so
    /// its prism would only cut the room. The opposite face is read through
    /// any solid backing it, so each outer layer of a wall built in layers
    /// counts the open space beyond the whole wall.
    fn of_wall_face(
        frame: &BrushFrame,
        normal_axis: usize,
        vertical_axis: usize,
        outward: f64,
        space: &OpenSpace<'_>,
    ) -> Option<Self> {
        if !broad_face_counts(frame, normal_axis, outward, space)
            || !broad_face_counts_through_solid(frame, normal_axis, -outward, space)
        {
            return None;
        }
        let open_depth = broad_face_deepest_open_depth(frame, normal_axis, outward, space)?;
        let splitters = free_end_planes(frame, normal_axis, vertical_axis, outward, space);
        if splitters.is_empty() {
            return None;
        }
        Some(Self::swept(
            frame,
            normal_axis,
            outward,
            open_depth,
            splitters,
        ))
    }

    /// The face swept `open_depth` along its normal, cut at the length bound.
    fn swept(
        frame: &BrushFrame,
        normal_axis: usize,
        outward: f64,
        open_depth: f64,
        splitters: Vec<(DVec3, f64)>,
    ) -> Self {
        let normal = frame.axes[normal_axis] * outward;
        let face = frame.center.dot(normal) + frame.extents[normal_axis] * 0.5;
        let broad = [(normal_axis + 1) % 3, (normal_axis + 2) % 3];
        let longer = frame.extents[broad[0]].max(frame.extents[broad[1]]);
        let bound = SHADOW_PRISM_LENGTH_FACTOR * longer;
        let truncated = bound < open_depth;
        let depth = open_depth.min(bound);

        let mut hull = [(DVec3::ZERO, 0.0); 6];
        hull[0] = (-normal, -face);
        hull[1] = (normal, face + depth);
        for (slot, (axis, sign)) in broad
            .into_iter()
            .flat_map(|axis| [(axis, 1.0), (axis, -1.0)])
            .enumerate()
        {
            let out = frame.axes[axis] * sign;
            hull[2 + slot] = (out, frame.center.dot(out) + frame.extents[axis] * 0.5);
        }

        let face_center = frame.center + normal * (frame.extents[normal_axis] * 0.5);
        let half = broad.map(|axis| frame.axes[axis] * (frame.extents[axis] * 0.5));
        let mut bounds = Aabb::empty();
        for reach in [0.0, depth] {
            for a in [1.0, -1.0] {
                for b in [1.0, -1.0] {
                    bounds.expand_point(face_center + half[0] * a + half[1] * b + normal * reach);
                }
            }
        }

        Self {
            splitters,
            hull,
            bounds,
            depth,
            truncated,
        }
    }

    /// True when `region`'s interior overlaps the prism.
    pub(crate) fn overlaps(&self, region: &RegionPolytope) -> bool {
        let region_bounds = region.vertex_aabb();
        if !region_bounds.is_valid() {
            return false;
        }
        let separated = (0..3).any(|axis| {
            region_bounds.max[axis] <= self.bounds.min[axis] + OVERLAP_EPSILON
                || self.bounds.max[axis] <= region_bounds.min[axis] + OVERLAP_EPSILON
        });
        if separated {
            return false;
        }
        let mut inside = region.clone();
        for &(normal, distance) in &self.hull {
            inside = inside.clip(normal, distance - OVERLAP_EPSILON).1;
            if inside.vertices().next().is_none() {
                return false;
            }
        }
        true
    }
}

// Hooks for the prism rows, which run bin-side in `pipeline::runtime_cells`
// against the first pass's exterior flood; the lib test target never calls them.
#[cfg(test)]
#[allow(dead_code)]
impl ShadowPrism {
    /// The outward normal of the face the prism sweeps.
    pub(crate) fn face_normal(&self) -> DVec3 {
        -self.hull[0].0
    }

    /// The plane of the face the prism sweeps, as `face_normal · p = distance`.
    pub(crate) fn face_distance(&self) -> f64 {
        -self.hull[0].1
    }
}

/// The inward plane of each vertical end edge of the wall face of `frame`'s
/// box whose outward normal is `outward` along `normal_axis`, kept when the
/// end meets no solid and the space beyond it, behind the face, is free: at
/// least `FREE_END_MIN_OPEN_PROBES` of the probes `FREE_END_PROBE_OFFSET_M`
/// past the end's plane, along the end and at each depth behind the face,
/// lie in open space. The end meets solid when most of the joint probes just
/// past it, inside the wall's thickness, are blocked, as where another wall
/// continues it in line or crosses it. An end meeting solid adds no
/// splitter: it hides nothing past itself, and its plane, infinite across
/// the tier, would only cut open space elsewhere.
fn free_end_planes(
    frame: &BrushFrame,
    normal_axis: usize,
    vertical_axis: usize,
    outward: f64,
    space: &OpenSpace<'_>,
) -> Vec<(DVec3, f64)> {
    let end_axis = 3 - normal_axis - vertical_axis;
    let normal = frame.axes[normal_axis] * outward;
    let face_center = frame.center + normal * (frame.extents[normal_axis] * 0.5);
    let along = frame.axes[vertical_axis] * frame.extents[vertical_axis];
    let across = frame.axes[normal_axis] * frame.extents[normal_axis];
    let half_length = frame.extents[end_axis] * 0.5;
    let joint_probes = FREE_END_PROBE_FRACTIONS.len() * JOINT_PROBE_THICKNESS_FRACTIONS.len();
    let mut planes = Vec::with_capacity(2);
    for sign in [1.0, -1.0] {
        let out = frame.axes[end_axis] * sign;
        let joint = frame.center + out * (half_length + JOINT_PROBE_OFFSET_M);
        let blocked = FREE_END_PROBE_FRACTIONS
            .iter()
            .flat_map(|&fraction| {
                JOINT_PROBE_THICKNESS_FRACTIONS
                    .iter()
                    .map(move |&depth| joint + along * (fraction - 0.5) + across * (depth - 0.5))
            })
            .filter(|&probe| space.is_blocked(probe))
            .count();
        if 2 * blocked > joint_probes {
            continue;
        }
        let beyond = face_center + out * (half_length + FREE_END_PROBE_OFFSET_M);
        let open = FREE_END_PROBE_FRACTIONS
            .iter()
            .flat_map(|&fraction| {
                FREE_END_PROBE_DEPTHS_M
                    .iter()
                    .map(move |&behind| beyond + along * (fraction - 0.5) + normal * behind)
            })
            .filter(|&probe| !space.is_blocked(probe))
            .count();
        if open >= FREE_END_MIN_OPEN_PROBES {
            let edge = frame.center.dot(out) + half_length;
            planes.push((-out, -edge));
        }
    }
    planes
}

/// An exact total order on prism hulls, for an order-free sort.
fn hull_order(a: &[(DVec3, f64); 6], b: &[(DVec3, f64); 6]) -> std::cmp::Ordering {
    a.iter()
        .zip(b)
        .map(|(x, y)| {
            x.0.x
                .total_cmp(&y.0.x)
                .then(x.0.y.total_cmp(&y.0.y))
                .then(x.0.z.total_cmp(&y.0.z))
                .then(x.1.total_cmp(&y.1))
        })
        .find(|order| order.is_ne())
        .unwrap_or(std::cmp::Ordering::Equal)
}
