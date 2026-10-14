// Fluid surface faces: each fluid brush side minus solid world and sibling brushes.
// See: context/lib/build_pipeline.md §Compiler pipeline

use glam::DVec3;

use crate::geometry_utils::split_polygon;
use crate::map_data::{BrushSide, BrushVolume, MapEnvironmentBrush, MapEnvironmentVolume};
use crate::partition::Aabb;

use super::push_split::PushBrushes;
use super::solid_index::SolidIndex;

/// A face within this distance of a remover's plane, along a parallel normal,
/// lies on it: 1 mm, face extraction's coplanar distance tolerance. A fluid
/// face this close to a solid face is flush, not a gap.
pub(super) const FLUSH_EPSILON: f64 = 1e-3;
/// `1 - |n·m|` below this makes two plane normals parallel (≈ 0.08°).
pub(super) const PARALLEL_EPSILON: f64 = 1e-6;
/// Point classification for a cutting plane: 0.1 mm, face extraction's
/// coplanar-subtraction epsilon. Tight, because it cuts visible faces.
pub(super) const SPLIT_EPSILON: f64 = 1e-4;
/// Pieces under 1 mm² are tolerance slivers, not surface; a removed region
/// under it is a touch along an edge, not burial.
pub(super) const MIN_PIECE_AREA: f64 = 1e-6;
/// Consecutive vertices closer than this collapse into one.
const WELD_EPSILON: f64 = 1e-6;

/// One exposed convex piece of a fluid brush side, wound like its side:
/// counter-clockwise seen from outside the fluid, around the outward normal.
#[derive(Debug, Clone)]
pub(super) struct SurfacePiece<'a> {
    pub(super) vertices: Vec<DVec3>,
    pub(super) side: &'a BrushSide,
}

/// What may bury part of a fluid face: a solid world brush, or another brush
/// of the same fluid entity.
struct Remover<'a> {
    planes: &'a [(DVec3, f64)],
    bounds: &'a Aabb,
    /// Whether a remover face coplanar with the fluid face and facing the same
    /// way buries the overlap. A solid does: fluid there sits inside the solid
    /// and would z-fight with the world face. Of two sibling brushes sharing
    /// such a face, the earlier keeps the overlap and the later loses it, so the
    /// union's boundary draws exactly once.
    same_facing_coplanar_buries: bool,
}

/// One brush of the volume, prepared as both a surface source and a sibling
/// remover.
struct FluidBrush<'a> {
    sides: &'a [BrushSide],
    planes: Vec<(DVec3, f64)>,
    bounds: Aabb,
}

impl<'a> FluidBrush<'a> {
    fn new(brush: &'a MapEnvironmentBrush) -> Self {
        let mut bounds = Aabb::empty();
        for vertex in brush.sides.iter().flat_map(|side| &side.vertices) {
            bounds.expand_point(*vertex);
        }
        Self {
            sides: &brush.sides,
            // Side planes, not `brush.planes`: f64, and a degenerate side's
            // plane only grazes the hull, so dropping it leaves the hull whole.
            planes: brush
                .sides
                .iter()
                .map(|side| (side.normal, side.distance))
                .collect(),
            bounds,
        }
    }
}

/// The static world brushes a fluid face can be buried in, indexed for
/// per-face lookup.
pub(super) struct SolidWorld<'a> {
    brushes: &'a [BrushVolume],
    planes: Vec<Vec<(DVec3, f64)>>,
    index: SolidIndex,
}

impl<'a> SolidWorld<'a> {
    pub(super) fn new(brushes: &'a [BrushVolume]) -> Self {
        Self {
            brushes,
            planes: brushes
                .iter()
                .map(|brush| {
                    brush
                        .planes
                        .iter()
                        .map(|plane| (plane.normal, plane.distance))
                        .collect()
                })
                .collect(),
            index: SolidIndex::build(brushes.iter().map(|brush| brush.aabb.clone()).collect()),
        }
    }
}

/// Exposed surface pieces of one volume, in brush then side order, each cut
/// along every push brush over it. A dry volume, or a fluid volume whose
/// brushes carry no sides, has none.
pub(super) fn fluid_surface_pieces<'a>(
    volume: &'a MapEnvironmentVolume,
    solid: &SolidWorld<'_>,
    pushes: &PushBrushes,
) -> Vec<SurfacePiece<'a>> {
    if volume.fluid.is_none() {
        return Vec::new();
    }
    let brushes: Vec<FluidBrush<'a>> = volume.brushes.iter().map(FluidBrush::new).collect();
    let mut candidates = Vec::new();
    let mut pieces = Vec::new();
    for (own, brush) in brushes.iter().enumerate() {
        for side in brush.sides {
            let mut exposed = vec![side.vertices.clone()];
            let face_bounds = expanded_bounds(&side.vertices, FLUSH_EPSILON);

            solid.index.overlapping(&face_bounds, &mut candidates);
            for &solid_index in &candidates {
                let remover = Remover {
                    planes: &solid.planes[solid_index],
                    bounds: &solid.brushes[solid_index].aabb,
                    same_facing_coplanar_buries: true,
                };
                subtract_remover(&mut exposed, side, &remover);
                if exposed.is_empty() {
                    break;
                }
            }
            for (sibling, other) in brushes.iter().enumerate() {
                if exposed.is_empty() {
                    break;
                }
                if sibling == own || !other.bounds.intersects(&face_bounds) {
                    continue;
                }
                let remover = Remover {
                    planes: &other.planes,
                    bounds: &other.bounds,
                    same_facing_coplanar_buries: sibling < own,
                };
                subtract_remover(&mut exposed, side, &remover);
            }
            pushes.split(&mut exposed, side, &face_bounds);

            pieces.extend(
                exposed
                    .into_iter()
                    .map(|vertices| SurfacePiece { vertices, side }),
            );
        }
    }
    pieces
}

/// Replace every piece with its part outside `remover`.
fn subtract_remover(pieces: &mut Vec<Vec<DVec3>>, side: &BrushSide, remover: &Remover<'_>) {
    if remover.planes.is_empty() || !box_touches_plane(remover.bounds, side) {
        return;
    }
    let mut kept = Vec::with_capacity(pieces.len());
    for piece in pieces.drain(..) {
        match outside_parts(&piece, side.normal, remover) {
            Some(outside) => kept.extend(outside),
            None => kept.push(piece),
        }
    }
    *pieces = kept;
}

/// The part of convex `piece` outside the closed convex `remover`, as convex
/// pieces, or `None` when the remover buries none of it.
///
/// Successive plane splits: each remover plane peels off the remainder's part
/// in front of it, and what survives every plane lies inside the remover. A
/// remainder lying on a remover plane (a flush face) stays inside it, unless
/// that face faces the same way and the remover does not bury such overlap.
fn outside_parts(
    piece: &[DVec3],
    face_normal: DVec3,
    remover: &Remover<'_>,
) -> Option<Vec<Vec<DVec3>>> {
    let mut outside = Vec::new();
    let mut remainder = piece.to_vec();
    for &(normal, distance) in remover.planes {
        let (lo, hi) = distance_range(&remainder, normal, distance);
        let parallel = 1.0 - normal.dot(face_normal).abs() < PARALLEL_EPSILON;
        if parallel && lo >= -FLUSH_EPSILON && hi <= FLUSH_EPSILON {
            if normal.dot(face_normal) > 0.0 && !remover.same_facing_coplanar_buries {
                return None;
            }
            continue;
        }
        if lo > SPLIT_EPSILON {
            // Wholly in front of one plane: the remover misses the piece.
            return None;
        }
        if hi <= SPLIT_EPSILON {
            continue;
        }
        let (front, back) = split_polygon(&remainder, normal, distance, SPLIT_EPSILON);
        if let Some(front) = front.map(weld)
            && polygon_area(&front, face_normal) >= MIN_PIECE_AREA
        {
            outside.push(front);
        }
        // Nothing behind this plane: the remover misses the piece.
        remainder = weld(back?);
    }
    // An edge or corner touch buries nothing: keep the piece whole rather than
    // fragmenting it along planes that removed no area.
    if polygon_area(&remainder, face_normal) < MIN_PIECE_AREA {
        return None;
    }
    Some(outside)
}

/// Whether the remover's box reaches the face's plane. A box wholly in front
/// of it or wholly behind it cannot hold any point of the face.
pub(super) fn box_touches_plane(bounds: &Aabb, side: &BrushSide) -> bool {
    let normal = side.normal;
    let near = DVec3::new(
        if normal.x >= 0.0 {
            bounds.min.x
        } else {
            bounds.max.x
        },
        if normal.y >= 0.0 {
            bounds.min.y
        } else {
            bounds.max.y
        },
        if normal.z >= 0.0 {
            bounds.min.z
        } else {
            bounds.max.z
        },
    );
    let far = DVec3::new(
        if normal.x >= 0.0 {
            bounds.max.x
        } else {
            bounds.min.x
        },
        if normal.y >= 0.0 {
            bounds.max.y
        } else {
            bounds.min.y
        },
        if normal.z >= 0.0 {
            bounds.max.z
        } else {
            bounds.min.z
        },
    );
    near.dot(normal) - side.distance <= FLUSH_EPSILON
        && far.dot(normal) - side.distance >= -FLUSH_EPSILON
}

pub(super) fn distance_range(polygon: &[DVec3], normal: DVec3, distance: f64) -> (f64, f64) {
    polygon
        .iter()
        .map(|vertex| vertex.dot(normal) - distance)
        .fold((f64::INFINITY, f64::NEG_INFINITY), |(lo, hi), d| {
            (lo.min(d), hi.max(d))
        })
}

fn expanded_bounds(polygon: &[DVec3], margin: f64) -> Aabb {
    let mut bounds = Aabb::empty();
    for vertex in polygon {
        bounds.expand_point(*vertex);
    }
    Aabb {
        min: bounds.min - DVec3::splat(margin),
        max: bounds.max + DVec3::splat(margin),
    }
}

/// Collapse consecutive coincident vertices a split can leave behind.
pub(super) fn weld(polygon: Vec<DVec3>) -> Vec<DVec3> {
    let mut welded: Vec<DVec3> = Vec::with_capacity(polygon.len());
    for vertex in polygon {
        if welded
            .last()
            .is_none_or(|last| last.distance_squared(vertex) > WELD_EPSILON * WELD_EPSILON)
        {
            welded.push(vertex);
        }
    }
    while welded.len() > 1
        && welded[0].distance_squared(welded[welded.len() - 1]) <= WELD_EPSILON * WELD_EPSILON
    {
        welded.pop();
    }
    welded
}

pub(super) fn polygon_area(polygon: &[DVec3], normal: DVec3) -> f64 {
    if polygon.len() < 3 {
        return 0.0;
    }
    let mut twice = DVec3::ZERO;
    for i in 0..polygon.len() {
        twice += polygon[i].cross(polygon[(i + 1) % polygon.len()]);
    }
    0.5 * twice.dot(normal).abs()
}
