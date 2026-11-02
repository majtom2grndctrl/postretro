// Door recognition: which kinematic movers seal a doorway at their closed
// pose, judged before the occluder-tier build from canonical brush geometry
// and the first-pass tree; and the splitters each door feeds that tier.
// See: context/lib/build_pipeline.md §Compiler pipeline

use glam::DVec3;

use super::brush_bsp::REGION_CLASSIFICATION_EPSILON;
use super::occluder_classify::{
    BrushFrame, FACE_SAMPLE_FRACTIONS, OCCLUDER_FLOOR_M, OpenSpace, PROBE_OFFSET_M, at_least,
    brush_polytope,
};
use super::occluder_cluster::contact::{Hull, touching};
use super::region_polytope::RegionPolytope;
use super::types::Aabb;
use crate::map_data::{BrushVolume, MapKinematicMover};

/// Contact with a broad axis's end face counts only this far in from the
/// face's edges along the other broad axis. Solid that meets an end face
/// only along that edge touches the face beside it: the floor under a lift
/// car meets each side face along its bottom edge, which is the bottom
/// face's contact, not the side's. Contact along the edge the end face shares
/// with a broad face still counts: a door hung in front of its opening meets
/// the jambs there.
const END_CONTACT_INSET_M: f64 = PROBE_OFFSET_M;

/// A plane whose normal is this close to the broad-face normal is parallel to
/// the broad face.
const BROAD_PARALLEL_COS: f64 = 1.0 - 1e-6;

/// Depth a region must reach into a door brush to overlap it: the tier's
/// spanning tolerance, so a region that only touches the door, or meets it
/// within that tolerance, does not. A region reaching past a broad plane of
/// a brush it overlaps then reaches past it by more than that tolerance, so
/// the plane spans it and splits first; the door's edge planes are reached
/// only in a region lying between the broad planes of the brushes it
/// overlaps. With a smaller depth, a door a fraction of a millimetre proud
/// of a wall face would overlap the room in front, which no broad plane
/// spans, and its edge planes would cut the room at the door's outline.
const OVERLAP_EPSILON: f64 = REGION_CLASSIFICATION_EPSILON;

/// Open space a door's shallower broad face must front, beyond its face:
/// far above the probe offset, far below any closet a player can stand in.
/// A mover resting flush on what lies behind it fronts none.
const SHALLOW_FACE_MIN_DEPTH_M: f64 = 0.1;

/// The occluder-tier splitters of one door. Its planes split only regions
/// whose interior overlaps the door's closed brushes, and only after every
/// occluder-rank and prism plane, so they cut only the occluder-tier leaf
/// that holds the doorway: a room the door does not reach keeps its cells.
#[derive(Debug, Clone)]
pub struct DoorSplitter {
    /// Index of the mover in `MapData::kinematic_movers`.
    pub mover_index: usize,
    /// The closed brushes' planes parallel to the broad face. They split
    /// first, closing the doorway between the door's two faces.
    pub broad_planes: Vec<(DVec3, f64)>,
    /// The closed brushes' other planes. They split only the slab the broad
    /// planes leave around the door, so the portals onto the door's faces
    /// end at its edges and the door covers each one; on a wall with gaps
    /// beside the door, the gaps keep portals of their own.
    pub edge_planes: Vec<(DVec3, f64)>,
    /// Per closed brush, its outward planes, for the overlap test.
    hulls: Vec<Vec<(DVec3, f64)>>,
    bounds: Aabb,
}

impl DoorSplitter {
    /// True when `region`'s interior overlaps one of the door's brushes.
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
        self.hulls.iter().any(|planes| {
            let mut inside = region.clone();
            for &(normal, distance) in planes {
                inside = inside.clip(normal, distance - OVERLAP_EPSILON).1;
                if inside.vertices().next().is_none() {
                    return false;
                }
            }
            true
        })
    }
}

/// Recognise the doors among `movers`: a mover is a door when, at its closed
/// (authored) pose, some pair of opposite faces of its own-frame box, its
/// broad faces, has static solid touching both ends of at least one axis
/// across them, within the contact epsilon, open space at least the
/// occluder floor deep in front of one, and open space at least
/// `SHALLOW_FACE_MIN_DEPTH_M` deep in front of the other: a door into a
/// closet shallower than the floor still seals the closet off.
/// A lift car docked in a room touches only the floor and is no door.
/// Movers are never part of `space` or `static_brushes`, so a door is never
/// a blocking surface for its wall's classification.
pub fn recognize_doors(
    movers: &[MapKinematicMover],
    static_brushes: &[BrushVolume],
    space: &OpenSpace<'_>,
) -> Vec<DoorSplitter> {
    movers
        .iter()
        .enumerate()
        .filter_map(|(mover_index, mover)| {
            let normal = door_broad_normal(&mover.brush_volumes, static_brushes, space)?;
            Some(DoorSplitter::new(mover_index, &mover.brush_volumes, normal))
        })
        .collect()
}

impl DoorSplitter {
    fn new(mover_index: usize, brushes: &[BrushVolume], broad_normal: DVec3) -> Self {
        let mut broad_planes = Vec::new();
        let mut edge_planes = Vec::new();
        let mut bounds = Aabb::empty();
        for brush in brushes {
            bounds.expand_aabb(&brush.aabb);
            for plane in &brush.planes {
                // Inward, so a point on a door plane files with the door's
                // side, open in the static world, and never with the static
                // solid the door rests on or meets.
                let key = (-plane.normal, -plane.distance);
                if plane.normal.dot(broad_normal).abs() >= BROAD_PARALLEL_COS {
                    broad_planes.push(key);
                } else {
                    edge_planes.push(key);
                }
            }
        }
        Self {
            mover_index,
            broad_planes,
            edge_planes,
            hulls: brushes
                .iter()
                .map(|brush| {
                    brush
                        .planes
                        .iter()
                        .map(|plane| (plane.normal, plane.distance))
                        .collect()
                })
                .collect(),
            bounds,
        }
    }
}

/// The broad-face normal of a closed mover that seals a doorway, or `None`.
/// Each own-frame axis offers the pair of faces across it, thinnest first,
/// ties in the frame's canonical axis order; the first pair that passes is
/// the door's broad face. A door is usually thinnest through its doorway, but
/// one filling a doorway in a thick wall can be shallower than it is deep:
/// its faces across its height front a floor and a lintel, never open space,
/// so only the pair facing the rooms passes.
fn door_broad_normal(
    brushes: &[BrushVolume],
    static_brushes: &[BrushVolume],
    space: &OpenSpace<'_>,
) -> Option<DVec3> {
    if brushes.is_empty() {
        return None;
    }
    let frame = BrushFrame::of_brushes(brushes)?;
    let mut normal_axes = [0, 1, 2];
    normal_axes.sort_by(|&a, &b| {
        frame.extents[a]
            .total_cmp(&frame.extents[b])
            .then(a.cmp(&b))
    });
    let mut near = NearbySolid::around(brushes, static_brushes);
    normal_axes
        .into_iter()
        .find(|&normal_axis| {
            let broad = [(normal_axis + 1) % 3, (normal_axis + 2) % 3];
            let spans_opening = broad.iter().any(|&axis| {
                let other = broad[0] + broad[1] - axis;
                [1.0, -1.0]
                    .iter()
                    .all(|&sign| near.touches(&end_face(&frame, normal_axis, axis, other, sign)))
            });
            let fronts = |sign: f64, depth: f64| {
                fronts_open_space(&frame, normal_axis, broad, sign, depth, space)
            };
            spans_opening
                && [(1.0, -1.0), (-1.0, 1.0)].iter().any(|&(deep, shallow)| {
                    fronts(deep, OCCLUDER_FLOOR_M) && fronts(shallow, SHALLOW_FACE_MIN_DEPTH_M)
                })
        })
        .map(|normal_axis| frame.axes[normal_axis])
}

/// The end of `frame`'s box across broad axis `axis` on side `sign`: its
/// face, spanning the thin axis whole and the other broad axis inset by
/// `END_CONTACT_INSET_M`, the plane it lies on, outward, and the box's
/// mid-plane across the same axis.
struct EndFace {
    face: Hull,
    outward: DVec3,
    offset: f64,
    middle: f64,
}

fn end_face(frame: &BrushFrame, thin: usize, axis: usize, other: usize, sign: f64) -> EndFace {
    let outward = frame.axes[axis] * sign;
    let centre = frame.center + outward * (frame.extents[axis] * 0.5);
    let u = frame.axes[thin] * (frame.extents[thin] * 0.5);
    let v = frame.axes[other] * (frame.extents[other] * 0.5 - END_CONTACT_INSET_M).max(0.0);
    EndFace {
        face: Hull::of_rectangle(
            [
                centre - u - v,
                centre + u - v,
                centre + u + v,
                centre - u + v,
            ],
            frame.axes[thin],
            frame.axes[other],
        ),
        outward,
        offset: centre.dot(outward),
        middle: frame.center.dot(outward),
    }
}

/// True when most open samples of the broad face on side `sign` front open
/// space at least `min_depth` deep, sampled as the classifier samples a
/// broad face.
fn fronts_open_space(
    frame: &BrushFrame,
    normal_axis: usize,
    broad: [usize; 2],
    sign: f64,
    min_depth: f64,
    space: &OpenSpace<'_>,
) -> bool {
    let normal = frame.axes[normal_axis] * sign;
    let centre = frame.center + normal * (frame.extents[normal_axis] * 0.5);
    let offset =
        |axis: usize, fraction: f64| frame.axes[axis] * ((fraction - 0.5) * frame.extents[axis]);
    let (mut open, mut deep) = (0_usize, 0_usize);
    for &a in &FACE_SAMPLE_FRACTIONS {
        for &b in &FACE_SAMPLE_FRACTIONS {
            let probe =
                centre + offset(broad[0], a) + offset(broad[1], b) + normal * PROBE_OFFSET_M;
            if space.is_blocked(probe) {
                continue;
            }
            open += 1;
            let depth = PROBE_OFFSET_M + space.distance_to_blocking(probe, normal);
            if at_least(depth, min_depth) {
                deep += 1;
            }
        }
    }
    open > 0 && 2 * deep > open
}

/// The static brushes near a closed mover, their hulls built on first use.
struct NearbySolid<'a> {
    brushes: Vec<&'a BrushVolume>,
    hulls: Vec<Option<Hull>>,
}

impl<'a> NearbySolid<'a> {
    /// Static brushes whose bounds come within the contact epsilon of the
    /// mover's.
    fn around(mover: &[BrushVolume], static_brushes: &'a [BrushVolume]) -> Self {
        let mut bounds = Aabb::empty();
        for brush in mover {
            bounds.expand_aabb(&brush.aabb);
        }
        let pad = REGION_CLASSIFICATION_EPSILON;
        let brushes: Vec<&BrushVolume> = static_brushes
            .iter()
            .filter(|brush| {
                (0..3).all(|axis| {
                    brush.aabb.min[axis] <= bounds.max[axis] + pad
                        && bounds.min[axis] <= brush.aabb.max[axis] + pad
                })
            })
            .collect();
        let hulls = brushes.iter().map(|_| None).collect();
        Self { brushes, hulls }
    }

    /// True when some nearby static brush touches `end`'s face from beyond
    /// it, as a jamb, lintel or floor frames the door: it reaches past the
    /// end's plane and stops short of the mover's mid-plane. A neighbour flush
    /// with the end, such as a step whose top meets a docked platform's top
    /// along an edge, never reaches past the plane; a crate standing through
    /// a docked platform runs past the middle, so it frames nothing.
    fn touches(&mut self, end: &EndFace) -> bool {
        (0..self.brushes.len()).any(|i| {
            let hull =
                self.hulls[i].get_or_insert_with(|| Hull::of(&brush_polytope(self.brushes[i])));
            let (inner, outer) = hull.projection(end.outward);
            outer > end.offset + REGION_CLASSIFICATION_EPSILON
                && inner > end.middle
                && touching(hull, &end.face)
        })
    }
}
