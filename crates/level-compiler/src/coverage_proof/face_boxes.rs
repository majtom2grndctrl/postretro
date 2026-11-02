// Coverage proof candidates: each face's oriented box, grown by τ, that a partner's edge must meet.
// See: context/lib/build_pipeline.md §Compiler pipeline step 12 (Exact skips)

use glam::DVec3;

use super::edges::face_triangles;
use crate::geometry::GeometryResult;
use crate::ray_traversal::triangle_points;

/// A face's corners bounded in a frame of its own: an axis along its
/// longest triangle edge, its normal, and the third. A point within `pad`
/// of the face lies within `pad` of the box, so an edge whose segment
/// misses the grown box can pair with no edge of the face on any piece.
pub(super) struct FaceBox {
    origin: DVec3,
    axes: [DVec3; 3],
    lo: [f64; 3],
    hi: [f64; 3],
}

impl FaceBox {
    /// The segment `p`–`q` meets the box: a slab clip in its frame.
    pub(super) fn meets(&self, p: DVec3, q: DVec3) -> bool {
        let (mut enter, mut leave) = (0.0f64, 1.0f64);
        for k in 0..3 {
            let (a, b) = (
                (p - self.origin).dot(self.axes[k]),
                (q - self.origin).dot(self.axes[k]),
            );
            let d = b - a;
            if d == 0.0 {
                if a < self.lo[k] || a > self.hi[k] {
                    return false;
                }
                continue;
            }
            let (t0, t1) = ((self.lo[k] - a) / d, (self.hi[k] - a) / d);
            enter = enter.max(t0.min(t1));
            leave = leave.min(t0.max(t1));
            if enter > leave {
                return false;
            }
        }
        true
    }
}

/// Every face's box grown by `pad`, or `None` for a face with no triangle
/// of nonzero area, which then prunes nothing.
pub(super) fn face_boxes(geometry: &GeometryResult, pad: f64) -> Vec<Option<FaceBox>> {
    geometry
        .face_index_ranges
        .iter()
        .map(|range| {
            let corners: Vec<DVec3> = face_triangles(range)
                .flat_map(|tri| triangle_points(geometry, tri).map(|c| c.as_dvec3()))
                .collect();
            let (mut along, mut normal) = (DVec3::ZERO, DVec3::ZERO);
            for tri in corners.chunks(3) {
                let cross = (tri[1] - tri[0]).cross(tri[2] - tri[0]);
                if cross.length_squared() > normal.length_squared() {
                    normal = cross;
                }
                for edge in [tri[1] - tri[0], tri[2] - tri[1], tri[0] - tri[2]] {
                    if edge.length_squared() > along.length_squared() {
                        along = edge;
                    }
                }
            }
            let normal = normal.try_normalize()?;
            let along = (along - normal * along.dot(normal)).try_normalize()?;
            let axes = [along, normal.cross(along), normal];
            let origin = corners[0];
            let (mut lo, mut hi) = ([f64::INFINITY; 3], [f64::NEG_INFINITY; 3]);
            for &c in &corners {
                for k in 0..3 {
                    let x = (c - origin).dot(axes[k]);
                    lo[k] = lo[k].min(x - pad);
                    hi[k] = hi[k].max(x + pad);
                }
            }
            Some(FaceBox {
                origin,
                axes,
                lo,
                hi,
            })
        })
        .collect()
}
