// Footprint clip pre-pass: which texels of a chart some triangle could clip at all.
// See: context/lib/build_pipeline.md §Compiler pipeline (step 12)

use bvh::aabb::Aabb;
use bvh::bvh::Bvh;
use bvh::ray::Ray;
use glam::Vec3;
use nalgebra::{Point3, Vector3};

use crate::bvh_build::BvhPrimitive;
use crate::chart_raster::{chart_grid, chart_interior_dims, chart_texel_world_position};
use crate::geometry::GeometryResult;
use crate::lightmap_bake::{Chart, RAY_EPSILON, ray_triangle_hit};
use crate::ray_traversal::BoundedRay;

/// Footprint reach covered by the pre-pass, in texels: every sample offset is
/// under one texel from the centre at any count.
const FOOTPRINT_REACH_TEXELS: f32 = 1.0;

/// The slab about a chart's raised plane, the receiver's plane lifted
/// `RAY_EPSILON` along its normal, that a triangle must reach to block a
/// footprint's clip segments or lead-ins: it has corners on both sides of the
/// raised plane, within a tolerance far above rounding. The receiver and its
/// coplanar neighbours lie wholly below it.
#[derive(Debug, Clone, Copy)]
pub(crate) struct ClipBand {
    normal: Vec3,
    origin: Vec3,
    height: f32,
    tolerance: f32,
}

impl ClipBand {
    pub(crate) fn for_chart(chart: &Chart) -> Self {
        let mut normal = chart.u_axis.cross(chart.v_axis).normalize_or_zero();
        if normal.dot(chart.normal) < 0.0 {
            normal = -normal;
        }
        Self {
            normal,
            origin: chart.origin,
            height: RAY_EPSILON * normal.dot(chart.normal),
            tolerance: 0.5 * RAY_EPSILON,
        }
    }

    /// Whether the triangle with these corners reaches the band.
    pub(crate) fn admits(&self, points: [Vec3; 3]) -> bool {
        let heights = points.map(|p| self.normal.dot(p - self.origin));
        let (h_min, h_max) = (
            heights.iter().copied().fold(f32::INFINITY, f32::min),
            heights.iter().copied().fold(f32::NEG_INFINITY, f32::max),
        );
        !(h_min > self.height + self.tolerance || h_max < self.height - self.tolerance)
    }
}

/// Whether the segment `from → to` enters no triangle `band` admits, tested
/// over its whole length: unlike `segment_clear`, it starts at `from`, not
/// `RAY_EPSILON` along the segment. The footprint tests the stretch a shadow
/// ray's trace skips with it ([`super::footprint_visibility`]).
///
/// Only a triangle's front counts. Geometry extraction fans each face
/// counter-clockwise about its normal, which points out of its brush, so a
/// segment enters a wall through a front and leaves one through a back. A
/// segment that crosses a face from behind is leaving solid, toward the side
/// the face looks at; counting it would drop samples beside a wall whose rays
/// head away from it.
pub(crate) fn lead_in_clear(
    bvh: &Bvh<f32, 3>,
    primitives: &[BvhPrimitive],
    geometry: &GeometryResult,
    band: &ClipBand,
    from: Vec3,
    to: Vec3,
) -> bool {
    let delta = to - from;
    let length = delta.length();
    if length <= 0.0 {
        return true;
    }
    let dir = delta / length;
    let ray = Ray::new(
        Point3::new(from.x, from.y, from.z),
        Vector3::new(dir.x, dir.y, dir.z),
    );
    let query = BoundedRay::new(&ray, length);
    let geom = &geometry.geometry;
    for prim in bvh.traverse_iterator(&query, primitives) {
        let start = prim.index_offset as usize;
        let end = start + prim.index_count as usize;
        for tri in (start..end).step_by(3).take_while(|&tri| tri + 3 <= end) {
            let points = [0, 1, 2]
                .map(|k| Vec3::from(geom.vertices[geom.indices[tri + k] as usize].position));
            let front = (points[1] - points[0])
                .cross(points[2] - points[0])
                .dot(dir)
                < 0.0;
            if front
                && band.admits(points)
                && ray_triangle_hit(from, dir, points[0], points[1], points[2])
                    .is_some_and(|dist| dist < length)
            {
                return false;
            }
        }
    }
    true
}

/// The texels of one chart whose clip segments or lead-ins some triangle
/// could block.
///
/// A clip segment runs from the texel centre to a sample point, both lifted
/// `RAY_EPSILON` along the normal, and is tested from the centre on, never
/// behind it, so everything it tests lies in the receiver's plane raised by
/// the lift and within one texel of the centre. A triangle can block one only
/// if it reaches that raised plane there: it reaches the [`ClipBand`] and its
/// projection onto the chart comes within a texel. Triangles wholly below the
/// raised plane, the receiver and its coplanar neighbours among them, or
/// wholly above it, cannot. A lead-in leaves the raised plane, but it counts
/// only triangles the band admits, and it runs from `0.25 · RAY_EPSILON`
/// before its lifted sample point to `1.25 · RAY_EPSILON` past it: at most
/// `2.25 · RAY_EPSILON` from the sample point with the lift, and at most
/// `1.25 · RAY_EPSILON` of it along the receiver, inside the pad on the
/// texel's reach. Every other texel's clip segments and lead-ins are all
/// clear, so the walk skips them; the answers, and the occluder cache a
/// skipped texel would have probed and left untouched, are those the segments
/// give.
///
/// Light-independent: it depends on the chart, its grid and the geometry.
pub(crate) struct ClipReach {
    width: i32,
    height: i32,
    /// Interior texel rectangles `[tx0, tx1, ty0, ty1]`, inclusive.
    rects: Vec<[i32; 4]>,
    /// One bit per interior texel, row-major, when rasterized.
    mask: Option<Vec<u64>>,
}

impl ClipReach {
    /// The rectangles alone, tested per texel: for callers that sample a few
    /// texels of a large chart.
    pub(crate) fn for_chart(
        chart: &Chart,
        bvh: &Bvh<f32, 3>,
        primitives: &[BvhPrimitive],
        geometry: &GeometryResult,
    ) -> Self {
        let (width, height) = chart_interior_dims(chart);
        let mut reach = Self {
            width,
            height,
            rects: Vec::new(),
            mask: None,
        };
        if chart.uv_extent[0] <= 0.0 || chart.uv_extent[1] <= 0.0 {
            return reach;
        }
        let grid = chart_grid(chart);
        let step = [
            grid.uv_extent[0] / grid.interior[0] as f32,
            grid.uv_extent[1] / grid.interior[1] as f32,
        ];
        // Margins far above f32 rounding at map scale, far below a texel.
        let pad = 4.0 * RAY_EPSILON;
        let margin = [
            FOOTPRINT_REACH_TEXELS * step[0] + pad,
            FOOTPRINT_REACH_TEXELS * step[1] + pad,
        ];

        let band = ClipBand::for_chart(chart);

        // Candidate primitives: the chart's texel centres, padded by the reach.
        let corners = [
            (0, 0),
            (width - 1, 0),
            (0, height - 1),
            (width - 1, height - 1),
        ]
        .map(|(tx, ty)| chart_texel_world_position(chart, tx, ty));
        let lo = corners.iter().copied().fold(Vec3::INFINITY, Vec3::min);
        let hi = corners.iter().copied().fold(Vec3::NEG_INFINITY, Vec3::max);
        let grow = Vec3::splat(margin[0].max(margin[1]) + 2.0 * RAY_EPSILON);
        let (lo, hi) = (lo - grow, hi + grow);
        let query = Aabb::with_bounds(Point3::new(lo.x, lo.y, lo.z), Point3::new(hi.x, hi.y, hi.z));

        let geom = &geometry.geometry;
        // Texel index range whose centres' local coordinate lies in `[a, b]`
        // on `axis`, one texel wider each side against rounding.
        let texel_range = |axis: usize, a: f32, b: f32, len: i32| -> Option<(i32, i32)> {
            let to_grid = |c: f32| (c - grid.uv_min[axis]) / step[axis] - 0.5;
            let first = (to_grid(a).floor() as i32 - 1 - grid.origin[axis]).max(0);
            let last = (to_grid(b).ceil() as i32 + 1 - grid.origin[axis]).min(len - 1);
            (first <= last).then_some((first, last))
        };
        for prim in bvh.traverse_iterator(&query, primitives) {
            let start = prim.index_offset as usize;
            let end = start + prim.index_count as usize;
            for tri in (start..end).step_by(3).take_while(|&tri| tri + 3 <= end) {
                let points = [0, 1, 2]
                    .map(|k| Vec3::from(geom.vertices[geom.indices[tri + k] as usize].position));
                if !band.admits(points) {
                    continue;
                }
                let local = points.map(|p| {
                    let d = p - chart.origin;
                    [d.dot(chart.u_axis), d.dot(chart.v_axis)]
                });
                let span = |axis: usize| {
                    let values = local.map(|l| l[axis]);
                    (
                        values.iter().copied().fold(f32::INFINITY, f32::min) - margin[axis],
                        values.iter().copied().fold(f32::NEG_INFINITY, f32::max) + margin[axis],
                    )
                };
                let ((u0, u1), (v0, v1)) = (span(0), span(1));
                if let (Some((tx0, tx1)), Some((ty0, ty1))) = (
                    texel_range(0, u0, u1, width),
                    texel_range(1, v0, v1, height),
                ) {
                    reach.rects.push([tx0, tx1, ty0, ty1]);
                }
            }
        }
        reach
    }

    /// [`ClipReach::for_chart`] rasterized to one bit per texel, for a walk
    /// over the whole chart.
    pub(crate) fn rasterized(
        chart: &Chart,
        bvh: &Bvh<f32, 3>,
        primitives: &[BvhPrimitive],
        geometry: &GeometryResult,
    ) -> Self {
        let mut reach = Self::for_chart(chart, bvh, primitives, geometry);
        let texels = reach.width as usize * reach.height as usize;
        let mut mask = vec![0u64; texels.div_ceil(64)];
        for &[tx0, tx1, ty0, ty1] in &reach.rects {
            for ty in ty0..=ty1 {
                for tx in tx0..=tx1 {
                    let bit = ty as usize * reach.width as usize + tx as usize;
                    mask[bit / 64] |= 1 << (bit % 64);
                }
            }
        }
        reach.mask = Some(mask);
        reach
    }

    /// Whether interior texel `(tx, ty)`'s clip segments need tracing.
    pub(crate) fn may_clip(&self, tx: i32, ty: i32) -> bool {
        debug_assert!((0..self.width).contains(&tx) && (0..self.height).contains(&ty));
        match &self.mask {
            Some(mask) => {
                let bit = ty as usize * self.width as usize + tx as usize;
                mask[bit / 64] & (1 << (bit % 64)) != 0
            }
            None => self.rects.iter().any(|&[tx0, tx1, ty0, ty1]| {
                (tx0..=tx1).contains(&tx) && (ty0..=ty1).contains(&ty)
            }),
        }
    }
}
