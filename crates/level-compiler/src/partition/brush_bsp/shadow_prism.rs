// Shadow prisms of the occluder tier: the space an occluding face hides, its
// broad face swept along its normal, whose edge planes split the regions the
// prism overlaps after occluder planes and before door and cap planes.
// See: context/plans/in-progress/occluder-first-partition/index.md §Decisions

use glam::DVec3;

use super::planes_equivalent;
use crate::map_data::BrushVolume;
use crate::partition::occluder_classify::{
    BrushFrame, OCCLUDER_FLOOR_M, OccluderClassification, OccluderVerdict, OpenSpace, at_least,
    broad_axes_reach_floor, broad_face_deepest_open_depth, thinnest_axes,
};
use crate::partition::occluder_cluster::SECTION_NORMAL_AXIS;
use crate::partition::region_polytope::RegionPolytope;
use crate::partition::types::Aabb;

/// A prism ends at most this multiple of its face's longer broad extent from
/// the face. An eye past a wall's end sees into its prism a width growing
/// with depth over distance in front, so deep prism sections are hidden from
/// fewer eyes; the bound is a cost limit on how far a face's edge planes cut
/// the space behind it, scaled with the face as that geometry is.
pub const SHADOW_PRISM_LENGTH_FACTOR: f64 = 2.0;

/// Depth a region must reach into a prism to overlap it, so a region that
/// only touches the prism does not, as `DoorSplitter::overlaps` measures it.
const OVERLAP_EPSILON: f64 = 1e-4;

/// The space one occluding broad face hides from an eye in front of the
/// opposite face and within its edges: the face swept along its outward
/// normal to the far edge of the open space in front of it, or to the
/// length bound, whichever is nearer.
#[derive(Debug, Clone)]
pub struct ShadowPrism {
    /// The tier splitters, inward to the prism, so a point on one files with
    /// the hidden side: the face's four edge planes, then the far plane when
    /// the length bound ends the prism before the open space does.
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

/// The shadow prisms of a classification: each broad face of an occluding
/// brush, and the bounding broad face of each occluding section, that fronts
/// open space at least the occluder floor deep while its opposite broad face
/// does too. Brush faces follow the classifier's tie rule, every face across
/// each thinnest axis whose broad axes reach the floor. Every prism reads
/// only its own geometry and `space`, and the set is sorted and
/// deduplicated by its hull, so it does not depend on brush order.
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
            for outward in [1.0, -1.0] {
                prisms.extend(ShadowPrism::of_face(&frame, normal_axis, outward, space));
            }
        }
    }
    let sections = classification
        .clusters
        .iter()
        .flat_map(|cluster| &cluster.sections)
        .filter(|section| section.occludes);
    for section in sections {
        let outward = if section.bounding_face.dot(section.normal) >= 0.0 {
            1.0
        } else {
            -1.0
        };
        prisms.extend(ShadowPrism::of_face(
            &section.frame,
            SECTION_NORMAL_AXIS,
            outward,
            space,
        ));
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
    /// The prism of the face of `frame`'s box whose outward normal is
    /// `outward` (±1) along `normal_axis`, or `None` when the open space in
    /// front of it, or in front of the opposite face, is under the floor at
    /// its deepest lattice sample. The prism is the space hidden from eyes
    /// in front of the opposite face, and no eye stands in a gap under the
    /// floor or in void: a room's own wall, its outer face on the void,
    /// hides the room from no one, so its prism would only cut the room.
    fn of_face(
        frame: &BrushFrame,
        normal_axis: usize,
        outward: f64,
        space: &OpenSpace<'_>,
    ) -> Option<Self> {
        let open_depth = floor_deep_open_depth(frame, normal_axis, outward, space)?;
        floor_deep_open_depth(frame, normal_axis, -outward, space)?;
        Some(Self::swept(frame, normal_axis, outward, open_depth))
    }

    /// The face swept `open_depth` along its normal, cut at the length bound.
    fn swept(frame: &BrushFrame, normal_axis: usize, outward: f64, open_depth: f64) -> Self {
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
        let mut splitters = Vec::with_capacity(5);
        for (slot, (axis, sign)) in broad
            .into_iter()
            .flat_map(|axis| [(axis, 1.0), (axis, -1.0)])
            .enumerate()
        {
            let out = frame.axes[axis] * sign;
            let edge = frame.center.dot(out) + frame.extents[axis] * 0.5;
            hull[2 + slot] = (out, edge);
            splitters.push((-out, -edge));
        }
        if truncated {
            splitters.push((-normal, -(face + depth)));
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

/// The deepest open depth in front of the face of `frame`'s box whose
/// outward normal is `outward` along `normal_axis`, when it is at least the
/// floor.
fn floor_deep_open_depth(
    frame: &BrushFrame,
    normal_axis: usize,
    outward: f64,
    space: &OpenSpace<'_>,
) -> Option<f64> {
    broad_face_deepest_open_depth(frame, normal_axis, outward, space)
        .filter(|&depth| at_least(depth, OCCLUDER_FLOOR_M))
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
