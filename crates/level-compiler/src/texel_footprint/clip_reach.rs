// Footprint clip pre-pass: which texels of a chart some triangle could clip at all.
// See: context/lib/build_pipeline.md §Compiler pipeline (step 12)

use bvh::aabb::Aabb;
use bvh::bvh::Bvh;
use glam::Vec3;
use nalgebra::Point3;

use crate::bvh_build::BvhPrimitive;
use crate::chart_raster::{chart_grid, chart_interior_dims, chart_texel_world_position};
use crate::geometry::GeometryResult;
use crate::lightmap_bake::{Chart, RAY_EPSILON};

/// Footprint reach covered by the pre-pass, in texels: every sample offset is
/// under one texel from the centre at any count.
const FOOTPRINT_REACH_TEXELS: f32 = 1.0;

/// The texels of one chart whose clip segments some triangle could block.
///
/// A clip segment runs from the texel centre to a sample point, both lifted
/// `RAY_EPSILON` along the normal, so every segment lies in the receiver's
/// plane raised by the lift and within one texel of the centre. A triangle can
/// block one only if it reaches that raised plane there: it has vertices on
/// both sides of it (within a tolerance far above rounding) and its projection
/// onto the chart comes within a texel. Triangles wholly below the raised
/// plane, the receiver and its coplanar neighbours among them, or wholly above
/// it, cannot. Every other texel's clip segments are all clear, so the walk
/// skips them; the answers, and the occluder cache a skipped texel would have
/// probed and left untouched, are those the segments give.
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

        // The chart's plane and the raised plane its clip segments lie in.
        let mut plane_normal = chart.u_axis.cross(chart.v_axis).normalize_or_zero();
        if plane_normal.dot(chart.normal) < 0.0 {
            plane_normal = -plane_normal;
        }
        let segment_height = RAY_EPSILON * plane_normal.dot(chart.normal);
        let tolerance = 0.5 * RAY_EPSILON;

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
                let heights = points.map(|p| plane_normal.dot(p - chart.origin));
                let (h_min, h_max) = (
                    heights.iter().copied().fold(f32::INFINITY, f32::min),
                    heights.iter().copied().fold(f32::NEG_INFINITY, f32::max),
                );
                if h_min > segment_height + tolerance || h_max < segment_height - tolerance {
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
