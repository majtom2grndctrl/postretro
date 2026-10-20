// Fluid surface faces split along the push brushes over them, so a current drifts exactly where its push is.
// See: context/lib/build_pipeline.md §Compiler pipeline

use glam::DVec3;

use crate::geometry_utils::split_polygon;
use crate::map_data::{BrushSide, MapEnvironmentVolume};
use crate::partition::Aabb;

use super::surface::{
    FLUSH_EPSILON, MIN_PIECE_AREA, SPLIT_EPSILON, box_reaches_plane, distance_range, polygon_area,
    weld,
};

/// How far inside the fluid a face samples its push, meters. Must equal the
/// runtime's `FLUID_DRIFT_SAMPLE_DEPTH` in
/// `crates/postretro/src/fluid_presentation/drift.rs`: the split is only
/// right if it sees each push as that sample does.
const DRIFT_SAMPLE_DEPTH: f64 = 2e-3;

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

    /// Split each exposed piece of `side` along every push brush over it. A
    /// piece wholly under a push, or wholly clear of it, stays whole and
    /// untouched. `face_bounds` is the side's vertex bounds grown by
    /// [`FLUSH_EPSILON`].
    pub(super) fn split(&self, pieces: &mut Vec<Vec<DVec3>>, side: &BrushSide, face_bounds: &Aabb) {
        // Every sample point lies on the side's plane moved inward by the
        // sample depth, inside the face's bounds grown by that depth: a push
        // missing either holds none of them.
        let sample_distance = side.distance - DRIFT_SAMPLE_DEPTH;
        let sample_bounds = Aabb {
            min: face_bounds.min - DVec3::splat(DRIFT_SAMPLE_DEPTH),
            max: face_bounds.max + DVec3::splat(DRIFT_SAMPLE_DEPTH),
        };
        for push in &self.brushes {
            if pieces.is_empty() {
                break;
            }
            // The box tests only cull, and `cut_by_push` decides; the plane
            // test takes the tolerance removers reach a face's plane with.
            if !push.bounds.intersects(&sample_bounds)
                || !box_reaches_plane(&push.bounds, side.normal, sample_distance, FLUSH_EPSILON)
            {
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
/// A face point is under the push when its drift sample, the point moved
/// [`DRIFT_SAMPLE_DEPTH`] into the fluid along `-face_normal`, is. For push
/// plane `(m, d)` that sample is inside when the face point is inside
/// `(m, d + depth·(m·f))`, so each plane cuts the face shifted by its share of
/// the sample depth. A plane parallel to the face classifies it whole: a
/// current's top flush with the waterline, or just under it, has the face
/// inside it, and a wind's bottom resting on the waterline has it outside.
/// An oblique plane cuts where the sample crosses it, not where the face does.
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
        let distance = distance + DRIFT_SAMPLE_DEPTH * normal.dot(face_normal);
        let (lo, hi) = distance_range(&remainder, normal, distance);
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
            // A sliver in front: `remainder` is left untouched, so the sliver
            // stays with it rather than dropping its area.
            _ => {}
        }
    }
    if polygon_area(&remainder, face_normal) < MIN_PIECE_AREA {
        return None;
    }
    Some((remainder, outside))
}
