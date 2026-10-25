// Tile shaft cull: proves that no triangle can block any segment the static
// layer walk traces from a tile of texels toward a point or spot emitter.
// See: context/lib/build_pipeline.md §Compiler pipeline (step 12)

use std::cell::{Cell, OnceCell};

use bvh::aabb::Aabb;
use bvh::bvh::Bvh;
use glam::{DVec3, Vec3};
use nalgebra::Point3;

use super::chart_cull::exact_texel_local;
use crate::bvh_build::BvhPrimitive;
use crate::chart_raster::{ChartGrid, chart_grid, chart_interior_dims};
use crate::geometry::GeometryResult;
use crate::lightmap_bake::{Chart, RAY_EPSILON};
use crate::map_data::{LightType, MapLight};
use crate::texel_footprint::{TexelFrame, TexelSampler};
pub(crate) use margin::ShaftScene;
use margin::{Candidate, Hull, HullAxis, HullBounds};

/// Edge of a shaft-cull tile, in texels. Tiles anchor at the (sub-)chart's
/// first interior texel, so a chart's last tile on each axis may be narrower.
pub(crate) const SHAFT_TILE_TEXELS: i32 = 8;

/// The lifted slab every ray origin, clip segment and lead-in starts in, in
/// `RAY_EPSILON`s along the chart normal: origins sit at 1, lead-ins reach
/// 0.75 to 2.25.
const SLAB_FLOOR: f64 = 0.5;
const SLAB_CEILING: f64 = 2.5;

/// In-plane pad past the footprint radius, in `RAY_EPSILON`s: a clip
/// segment starts one behind the centre and a lead-in reaches 1.25 along it.
const PAD_SLACK: f64 = 4.0;

/// Slack on the emitter sphere, in metres, over `light_size`: area targets
/// lie on the sphere to within rounding far below it.
const EMITTER_SLACK: f64 = 1.0e-3;

/// Least height, in `RAY_EPSILON`s, of the emitter's nearest target above
/// the texel plane. A lead-in runs 1.25 past its origin; with the target at
/// least this far up, that point lies on the traced segment, inside the hull.
const EMITTER_CLEARANCE: f64 = 5.0;

/// How far the chart frame may stray from orthonormal for the slab to hold
/// lead-ins, whose direction is any world direction.
const FRAME_SKEW_LIMIT: f64 = 1.0e-3;
const FRAME_NORM_LIMIT: f64 = 1.0e-6;

/// Relative slack on the footprint radius: offsets go through `f32` texel
/// steps along axes within [`FRAME_NORM_LIMIT`] of unit length.
const RADIUS_SLACK: f64 = 1.0e-5;

/// One `(light, chart)` walk's shaft test: the chart frame and emitter in
/// `f64`, and the scene to query.
pub(crate) struct ShaftCull<'a> {
    origin: DVec3,
    u_axis: DVec3,
    v_axis: DVec3,
    normal: DVec3,
    grid: ChartGrid,
    dims: (i32, i32),
    /// In-plane pad in chart-axis units: the footprint radius on the floored
    /// sampler, the lift's slack alone on the centre sampler.
    pad: f64,
    terms: DVec3,
    centre: DVec3,
    radius: f64,
    /// Whether the containment argument holds for this chart and light: a
    /// near-orthonormal frame and an emitter clear of the slab. Without it
    /// every tile tests blocked.
    contained: bool,
    /// The first index of the triangle that blocked this walk's last blocked
    /// tile. One thread walks the chart, so it needs no synchronisation.
    last_blocker: Cell<Option<usize>>,
    scene: &'a ShaftScene,
    bvh: &'a Bvh<f32, 3>,
    primitives: &'a [BvhPrimitive],
    geometry: &'a GeometryResult,
}

impl<'a> ShaftCull<'a> {
    /// `None` for a sun: its segments run a fixed 10 km, beyond any hull.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn new(
        light: &MapLight,
        chart: &Chart,
        sampler: &TexelSampler,
        frame: &TexelFrame,
        scene: &'a ShaftScene,
        bvh: &'a Bvh<f32, 3>,
        primitives: &'a [BvhPrimitive],
        geometry: &'a GeometryResult,
    ) -> Option<Self> {
        if light.light_type == LightType::Directional {
            return None;
        }
        let epsilon = f64::from(RAY_EPSILON);
        let grid = chart_grid(chart);
        let (origin, u_axis, v_axis, normal) = (
            chart.origin.as_dvec3(),
            chart.u_axis.as_dvec3(),
            chart.v_axis.as_dvec3(),
            chart.normal.as_dvec3(),
        );
        let pad = if sampler.is_footprint() {
            f64::from(frame.radius()) * (1.0 + RADIUS_SLACK)
        } else {
            0.0
        } + PAD_SLACK * epsilon;
        let span = |axis: usize| {
            f64::from(grid.uv_min[axis]).abs() + f64::from(grid.uv_extent[axis]).abs()
        };
        let terms = origin.abs() + u_axis.abs() * span(0) + v_axis.abs() * span(1);
        // The light's `f32` origin, as every ray target reads it.
        let centre = light.origin.as_vec3().as_dvec3();
        let size = f64::from(light.light_size);
        let radius = if size > 0.0 {
            size + EMITTER_SLACK
        } else {
            0.0
        };

        let unit = |v: DVec3| (v.length() - 1.0).abs() <= FRAME_NORM_LIMIT;
        let orthogonal = |a: DVec3, b: DVec3| a.dot(b).abs() <= FRAME_SKEW_LIMIT;
        let mut plane = u_axis.cross(v_axis).normalize_or_zero();
        if plane.dot(normal) < 0.0 {
            plane = -plane;
        }
        let clearance = plane.dot(centre - origin) - size.max(0.0);
        let contained = unit(u_axis)
            && unit(v_axis)
            && unit(normal)
            && orthogonal(u_axis, v_axis)
            && orthogonal(u_axis, normal)
            && orthogonal(v_axis, normal)
            && clearance >= EMITTER_CLEARANCE * epsilon
            && terms.is_finite()
            && centre.is_finite()
            && radius.is_finite()
            && pad.is_finite();
        Some(Self {
            origin,
            u_axis,
            v_axis,
            normal,
            grid,
            dims: chart_interior_dims(chart),
            pad,
            terms,
            centre,
            radius,
            contained,
            last_blocker: Cell::new(None),
            scene,
            bvh,
            primitives,
            geometry,
        })
    }

    /// Whether every segment the walk would trace from tile `(tile_x,
    /// tile_y)`'s texels is certainly clear: every triangle whose box comes
    /// within the query growth of the hull's box is separated from the hull
    /// by its own margin. `false` costs only speed.
    pub(crate) fn tile_clear(&self, tile_x: i32, tile_y: i32) -> bool {
        if !self.contained {
            return false;
        }
        let hull = self.tile_hull(tile_x, tile_y);
        let bounds = HullBounds::new(hull);
        let Some(growth) = self.scene.query_growth(&bounds) else {
            return false;
        };
        let (lo, hi) = (bounds.lo - growth, bounds.hi + growth);
        if !(lo.is_finite() && hi.is_finite()) {
            return false;
        }
        // Rounded outward, so the box query cannot miss a triangle whose box
        // the `f64` box reaches.
        let query = Aabb::with_bounds(
            Point3::new(f32_below(lo.x), f32_below(lo.y), f32_below(lo.z)),
            Point3::new(f32_above(hi.x), f32_above(hi.y), f32_above(hi.z)),
        );
        let slab_centre = hull.corners.iter().copied().sum::<DVec3>() / 8.0;
        let frame_axes = [
            self.u_axis,
            self.v_axis,
            self.normal,
            self.centre - slab_centre,
        ];
        // Built on the first candidate its own normal leaves unseparated.
        let hull_axes = OnceCell::new();
        let hull_axes = || {
            hull_axes
                .get_or_init(|| self.hull_axes(&bounds, &hull))
                .as_slice()
        };
        let geom = &self.geometry.geometry;
        let blocks = |tri: usize| {
            let points = [0, 1, 2]
                .map(|k| Vec3::from(geom.vertices[geom.indices[tri + k] as usize].position));
            let tri_lo = points[0].min(points[1]).min(points[2]).as_dvec3();
            let tri_hi = points[0].max(points[1]).max(points[2]).as_dvec3();
            if tri_hi.cmplt(lo).any() || tri_lo.cmpgt(hi).any() {
                // Farther than the growth: separated whether tested or not.
                return false;
            }
            Candidate::of(points)
                .is_none_or(|candidate| !bounds.separates(&candidate, hull_axes, &frame_axes))
        };
        // The triangle that blocked the last blocked tile goes first: across a
        // shadow it usually blocks the next tile too, and then no query runs.
        // A clear verdict still needs every candidate separated, so the order
        // changes only how soon a blocked tile is found.
        if self.last_blocker.get().is_some_and(blocks) {
            return false;
        }
        for prim in self.bvh.traverse_iterator(&query, self.primitives) {
            let start = prim.index_offset as usize;
            let end = start + prim.index_count as usize;
            for tri in (start..end).step_by(3).take_while(|&tri| tri + 3 <= end) {
                if blocks(tri) {
                    self.last_blocker.set(Some(tri));
                    return false;
                }
            }
        }
        true
    }

    /// The hull of tile `(tile_x, tile_y)`: its first and last texel centres
    /// on each axis from the exact chart formula, padded, at the slab's two
    /// heights, with the emitter sphere.
    fn tile_hull(&self, tile_x: i32, tile_y: i32) -> Hull {
        let edge = SHAFT_TILE_TEXELS;
        let span = |axis: usize, tile: i32, len: i32| {
            let first = exact_texel_local(&self.grid, axis, tile * edge);
            let last = exact_texel_local(&self.grid, axis, (tile * edge + edge - 1).min(len - 1));
            [first.min(last) - self.pad, first.max(last) + self.pad]
        };
        let lu = span(0, tile_x, self.dims.0);
        let lv = span(1, tile_y, self.dims.1);
        let epsilon = f64::from(RAY_EPSILON);
        let heights = [SLAB_FLOOR * epsilon, SLAB_CEILING * epsilon];
        let mut corners = [DVec3::ZERO; 8];
        for (i, corner) in corners.iter_mut().enumerate() {
            *corner = self.origin
                + self.u_axis * lu[i & 1]
                + self.v_axis * lv[(i >> 1) & 1]
                + self.normal * heights[i >> 2];
        }
        Hull {
            corners,
            centre: self.centre,
            radius: self.radius,
            terms: self.terms,
        }
    }

    /// The axes every candidate of one tile tests after its own normal, read
    /// from the hull alone: the receiver normal, then the hull's own faces
    /// ([`Hull::tangent_plane`]) through each side of the padded rectangle,
    /// at both slab heights, and planes through each rectangle corner across
    /// its outward diagonal, each tangent to the emitter. Any unit axis is a
    /// certificate at its own margin; these are the ones a candidate beside
    /// the shaft most often clears on.
    fn hull_axes(&self, bounds: &HullBounds, hull: &Hull) -> [Option<HullAxis>; HULL_AXES] {
        let mut axes = [None; HULL_AXES];
        axes[0] = bounds.hull_axis(self.normal);
        let mut next = 1;
        for height in [0, 4] {
            // Sides: (first corner on the side, run along it, outward).
            let sides = [
                (0, self.v_axis, -self.u_axis),
                (1, self.v_axis, self.u_axis),
                (0, self.u_axis, -self.v_axis),
                (2, self.u_axis, self.v_axis),
            ];
            for (corner, run, outward) in sides {
                axes[next] = hull
                    .tangent_plane(hull.corners[height + corner], run, outward)
                    .and_then(|axis| bounds.hull_axis(axis));
                next += 1;
            }
            for corner in 0..4 {
                let outward = if corner & 1 == 0 {
                    -self.u_axis
                } else {
                    self.u_axis
                } + if corner & 2 == 0 {
                    -self.v_axis
                } else {
                    self.v_axis
                };
                axes[next] = hull
                    .tangent_plane(
                        hull.corners[height + corner],
                        self.normal.cross(outward),
                        outward,
                    )
                    .and_then(|axis| bounds.hull_axis(axis));
                next += 1;
            }
        }
        axes
    }

    /// The candidate certificates [`Self::tile_clear`] would read for one
    /// triangle against tile `(tile_x, tile_y)`: (gap, margin) along its own
    /// normal and along `axis`.
    #[cfg(test)]
    pub(crate) fn certificates(
        &self,
        tile: (i32, i32),
        points: [Vec3; 3],
        axis: DVec3,
    ) -> ((f64, f64), Option<(f64, f64)>) {
        let bounds = HullBounds::new(self.tile_hull(tile.0, tile.1));
        let candidate = Candidate::of(points).expect("a non-degenerate triangle");
        (
            bounds.normal_certificate(&candidate),
            bounds.axis_certificate(&candidate, axis),
        )
    }

    /// The largest `axis · x` over tile `(tile_x, tile_y)`'s hull.
    #[cfg(test)]
    pub(crate) fn tile_support(&self, tile: (i32, i32), axis: DVec3) -> f64 {
        self.tile_hull(tile.0, tile.1).range(axis).1
    }

    /// The candidate query's growth for tile `(tile_x, tile_y)`.
    #[cfg(test)]
    pub(crate) fn tile_query_growth(&self, tile: (i32, i32)) -> Option<f64> {
        self.scene
            .query_growth(&HullBounds::new(self.tile_hull(tile.0, tile.1)))
    }
}

/// Axes [`ShaftCull::hull_axes`] builds per tile.
const HULL_AXES: usize = 17;

/// The largest `f32` at or below `x`.
fn f32_below(x: f64) -> f32 {
    let narrow = x as f32;
    if f64::from(narrow) > x {
        narrow.next_down()
    } else {
        narrow
    }
}

/// The smallest `f32` at or above `x`.
fn f32_above(x: f64) -> f32 {
    let narrow = x as f32;
    if f64::from(narrow) < x {
        narrow.next_up()
    } else {
        narrow
    }
}

mod margin;
#[cfg(test)]
mod tests;
