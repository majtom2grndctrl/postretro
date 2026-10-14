// Fluid surface faces split along the push brushes over them, so a current drifts exactly where its push is.
// See: context/lib/build_pipeline.md §Compiler pipeline

use glam::DVec3;

use crate::geometry_utils::split_polygon;
use crate::map_data::MapEnvironmentVolume;
use crate::partition::Aabb;

use super::surface::{
    FLUSH_EPSILON, MIN_PIECE_AREA, PARALLEL_EPSILON, SPLIT_EPSILON, box_touches_plane,
    distance_range, polygon_area, weld,
};
use crate::map_data::BrushSide;

/// One push brush: its planes in f64 and its bounds.
struct PushBrush {
    planes: Vec<(DVec3, f64)>,
    bounds: Aabb,
}

/// Every brush of every push volume, the cutters of a fluid face.
pub(super) struct PushBrushes {
    brushes: Vec<PushBrush>,
}

impl PushBrushes {
    pub(super) fn new(volumes: &[MapEnvironmentVolume]) -> Self {
        let brushes = volumes
            .iter()
            .filter(|volume| volume.push.is_some())
            .flat_map(|volume| &volume.brushes)
            .map(|brush| PushBrush {
                planes: brush
                    .planes
                    .iter()
                    .map(|&[nx, ny, nz, d]| {
                        (
                            DVec3::new(f64::from(nx), f64::from(ny), f64::from(nz)),
                            f64::from(d),
                        )
                    })
                    .collect(),
                bounds: Aabb {
                    min: DVec3::from(brush.min.map(f64::from)),
                    max: DVec3::from(brush.max.map(f64::from)),
                },
            })
            .collect();
        Self { brushes }
    }

    /// Split each exposed piece of `side` along every push brush over it.
    /// A piece wholly under a push, or wholly clear of it, stays whole.
    pub(super) fn split(&self, pieces: &mut Vec<Vec<DVec3>>, side: &BrushSide, bounds: &Aabb) {
        for push in &self.brushes {
            if pieces.is_empty() {
                return;
            }
            if !push.bounds.intersects(bounds) || !box_touches_plane(&push.bounds, side) {
                continue;
            }
            let mut kept = Vec::with_capacity(pieces.len());
            for piece in pieces.drain(..) {
                match cut_by_push(&piece, side.normal, &push.planes) {
                    Some((inside, outside)) if !outside.is_empty() => {
                        kept.push(inside);
                        kept.extend(outside);
                    }
                    _ => kept.push(piece),
                }
            }
            *pieces = kept;
        }
    }
}

/// `piece` cut into its part under the closed convex push and the parts
/// outside it, or `None` when the push covers none of it.
///
/// A face is classified as seen from just inside the fluid, where drift
/// samples the push. A push plane flush with the face and facing out of the
/// fluid (a current's top at the waterline) has the face inside it; one facing
/// into the fluid (a wind's bottom on the waterline) has the face outside it.
///
/// Cuts are area-preserving: a cut that would peel off a sliver under 1 mm²
/// is skipped, and a part left under it means the push only touches the piece,
/// which then stays whole.
fn cut_by_push(
    piece: &[DVec3],
    face_normal: DVec3,
    planes: &[(DVec3, f64)],
) -> Option<(Vec<DVec3>, Vec<Vec<DVec3>>)> {
    let mut outside = Vec::new();
    let mut remainder = piece.to_vec();
    for &(normal, distance) in planes {
        let (lo, hi) = distance_range(&remainder, normal, distance);
        let parallel = 1.0 - normal.dot(face_normal).abs() < PARALLEL_EPSILON;
        if parallel && lo >= -FLUSH_EPSILON && hi <= FLUSH_EPSILON {
            if normal.dot(face_normal) > 0.0 {
                continue;
            }
            return None;
        }
        if lo > SPLIT_EPSILON {
            return None;
        }
        if hi <= SPLIT_EPSILON {
            continue;
        }
        let (front, back) = split_polygon(&remainder, normal, distance, SPLIT_EPSILON);
        let back = weld(back?);
        if polygon_area(&back, face_normal) < MIN_PIECE_AREA {
            return None;
        }
        match front.map(weld) {
            Some(front) if polygon_area(&front, face_normal) >= MIN_PIECE_AREA => {
                outside.push(front);
                remainder = back;
            }
            // A sliver in front: keep it with the remainder rather than
            // dropping its area.
            _ => {}
        }
    }
    if polygon_area(&remainder, face_normal) < MIN_PIECE_AREA {
        return None;
    }
    Some((remainder, outside))
}
