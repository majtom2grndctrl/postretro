// Exact skips of the light-layer bake: the switches tests turn them off with,
// and the chart coverage cull partition planning applies before the walk.
// See: context/lib/build_pipeline.md §Compiler pipeline (step 12)

use std::f32::consts::FRAC_PI_2;

use glam::{DVec3, Vec3};

use crate::chart_raster::{ChartGrid, chart_grid, chart_interior_dims};
use crate::lightmap_bake::{Chart, directional_light_direction, spot_cone_angles};
use crate::map_data::{LightType, MapLight};

#[cfg(test)]
use super::skip_stats::SkipStats;

/// Which exact skips the light-layer bake takes. A skip drops only work whose
/// answer is already known, so output bytes are the same with it on or off.
/// Production takes every skip; tests turn them off, through the partition
/// window, the chart walk's argument or the test-only face walk's
/// (`bake_light_layer_controlled`), to check the skipped bake against the
/// full one. No flag, env var or cache key holds this.
#[derive(Clone, Copy, Debug)]
pub(crate) struct ExactSkips {
    /// Planning drops a `(light, chart)` pair [`chart_may_cover`] rules out.
    pub chart_cull: bool,
    /// The chart walk stores visibility 1 at a tile of texels the tile shaft
    /// cull ([`super::shaft_cull`]) proves clear, and traces none of them.
    pub shaft_cull: bool,
    /// Every covered texel of a clear tile is also traced normally, with
    /// copies of both occluder caches, and must give exactly 1.0 and leave
    /// both copies as the skipped walk holds them. On by default in tests, so
    /// every walk a test runs checks the cull.
    #[cfg(test)]
    pub verify: bool,
    /// Every tested tile counts as clear, whatever the shaft test says: the
    /// negative control that verify mode fails a skip the geometry does not
    /// allow.
    #[cfg(test)]
    pub force_clear: bool,
    /// Where the walk and planning tally what they skipped.
    #[cfg(test)]
    pub stats: Option<&'static SkipStats>,
}

impl ExactSkips {
    pub(crate) const ON: Self = Self {
        chart_cull: true,
        shaft_cull: true,
        #[cfg(test)]
        verify: true,
        #[cfg(test)]
        force_clear: false,
        #[cfg(test)]
        stats: None,
    };
    #[cfg(test)]
    pub(crate) const OFF: Self = Self {
        chart_cull: false,
        shaft_cull: false,
        verify: false,
        force_clear: false,
        stats: None,
    };

    /// These skips, tallied into `stats`.
    #[cfg(test)]
    pub(crate) fn tallied(self, stats: &'static SkipStats) -> Self {
        Self {
            stats: Some(stats),
            ..self
        }
    }
}

/// `chart_texel_world_position`'s local coordinate on `axis` of grid texel
/// `texel` (window-relative), in `f64`: the exact value of its `f32`
/// expression's inputs.
pub(super) fn exact_texel_local(grid: &ChartGrid, axis: usize, texel: i32) -> f64 {
    let frac = (f64::from(grid.origin[axis] + texel) + 0.5) / f64::from(grid.interior[axis]);
    f64::from(grid.uv_min[axis]) + frac * f64::from(grid.uv_extent[axis])
}

/// Bound, per metre of coordinate magnitude, on how far an `f32` centre from
/// `chart_texel_world_position` lies from its exact affine position. That
/// expression rounds about ten times, each within 2⁻²⁴ of the magnitude of
/// its terms: about 1.0e-6 m per metre across three components. This is
/// four times that.
const POSITION_SLACK_PER_METER: f64 = 4.0e-6;

/// Bound on the bake's `f32` rounding of a dot product of unit vectors: N·L,
/// and the spot's `aim · (−L)`. The `to_light` difference, its normalizing
/// and renormalizing, the aim's own normalizing and the dot itself each add
/// a few 2⁻²⁴, under 1.0e-6 together. This is four times that.
const COS_SLACK: f64 = 4.0e-6;

/// Bound on the bake's relative `f32` rounding of a texel's light distance
/// (`to_light.length()`), with Linear's `distance / range` on top: under
/// 4·2⁻²⁴ (2.4e-7). This is over ten times that.
const DISTANCE_SLACK: f64 = 4.0e-6;

/// Whether `light`'s direct term can be non-zero at any interior texel of
/// `chart`. `false` is exact: the bake's own expressions give every texel the
/// walk visits (`for_each_light_layer_chart_texel`) a zero contribution, so it
/// would record none. The direct term reads the light centre only, never
/// `light_size`, so neither does this.
///
/// A point or spot light is ruled out by any one of three tests, each bounding
/// an affine or monotone quantity over the texel-centre rectangle at its
/// extremes, with margins above the rounding of the texel centres and of the
/// bake's `f32` term. Non-finite input keeps the pair.
pub(crate) fn chart_may_cover(light: &MapLight, chart: &Chart) -> bool {
    if chart.uv_extent[0] <= 0.0 || chart.uv_extent[1] <= 0.0 {
        // The walk visits no texel.
        return false;
    }
    match light.light_type {
        // Every texel reads the chart normal and one direction, so the bake's
        // N·L is a single value, and this is its expression.
        LightType::Directional => {
            chart
                .normal
                .dot(directional_light_direction(light))
                .max(0.0)
                > 0.0
        }
        LightType::Point | LightType::Spot => {
            // The bake measures from the light's `f32` origin.
            let origin = light.origin.as_vec3().as_dvec3();
            let Some(rect) = TexelRect::of(chart) else {
                return true;
            };
            if !origin.is_finite() || !chart.normal.is_finite() {
                return true;
            }
            !(rect.behind(origin, chart.normal)
                || rect.beyond_range(light, origin)
                || light.light_type == LightType::Spot && rect.outside_cone(light, origin))
        }
    }
}

/// A chart's interior texel centres in exact arithmetic: the rectangle
/// `corner + s·u_edge + t·v_edge`, `s, t ∈ [0, 1]`, through the first and last
/// centres on each axis. Centres are affine in the texel index, so this holds
/// every exact centre; `slack` bounds how far an `f32` centre strays from it.
struct TexelRect {
    corner: DVec3,
    u_edge: DVec3,
    v_edge: DVec3,
    slack: f64,
}

impl TexelRect {
    /// `chart_texel_world_position`'s expression in `f64`, at the corner
    /// texels of the chart's interior, on its grid as a sub-chart's texels are.
    fn of(chart: &Chart) -> Option<Self> {
        let grid = chart_grid(chart);
        let (width, height) = chart_interior_dims(chart);
        let local = |axis: usize, texel: i32| exact_texel_local(&grid, axis, texel);
        let origin = chart.origin.as_dvec3();
        let u_axis = chart.u_axis.as_dvec3();
        let v_axis = chart.v_axis.as_dvec3();
        let (u0, u1) = (local(0, 0), local(0, width - 1));
        let (v0, v1) = (local(1, 0), local(1, height - 1));
        // The largest magnitude among the terms the `f32` expression adds.
        let span = |axis: usize| {
            f64::from(grid.uv_min[axis]).abs() + f64::from(grid.uv_extent[axis]).abs()
        };
        let magnitude =
            (origin.abs() + u_axis.abs() * span(0) + v_axis.abs() * span(1)).max_element();
        let rect = Self {
            corner: origin + u_axis * u0 + v_axis * v0,
            u_edge: u_axis * (u1 - u0),
            v_edge: v_axis * (v1 - v0),
            slack: POSITION_SLACK_PER_METER * magnitude,
        };
        (rect.corner.is_finite()
            && rect.u_edge.is_finite()
            && rect.v_edge.is_finite()
            && rect.slack.is_finite())
        .then_some(rect)
    }

    fn corners(&self) -> [DVec3; 4] {
        [
            self.corner,
            self.corner + self.u_edge,
            self.corner + self.v_edge,
            self.corner + self.u_edge + self.v_edge,
        ]
    }

    fn centre(&self) -> DVec3 {
        self.corner + (self.u_edge + self.v_edge) * 0.5
    }

    /// Radius about [`Self::centre`] of a sphere holding every `f32` centre.
    fn radius(&self) -> f64 {
        let centre = self.centre();
        let reach = self
            .corners()
            .map(|corner| corner.distance(centre))
            .into_iter()
            .fold(0.0, f64::max);
        reach + self.slack
    }

    /// Whether a light at `origin` is behind the plane of `normal` at every
    /// texel centre, by enough that the bake's N·L rounds to at most zero.
    ///
    /// `n · (light − p)` is affine in `p`, so the four corners bound it. All
    /// four are read: the chart normal comes from a vertex normal and need
    /// not be orthogonal to the u axis, so the centres need not lie in one
    /// plane of `normal`. The margin covers `slack`, plus [`COS_SLACK`] on N·L
    /// scaled by the farthest a centre can be from the light.
    fn behind(&self, origin: DVec3, normal: Vec3) -> bool {
        let normal = normal.as_dvec3();
        let highest = self
            .corners()
            .map(|corner| normal.dot(origin - corner))
            .into_iter()
            .fold(f64::NEG_INFINITY, f64::max);
        let farthest = origin.distance(self.centre()) + self.radius();
        highest < -(normal.length() * self.slack + COS_SLACK * farthest)
    }

    /// Whether every texel centre lies past the light's range, where every
    /// falloff model is zero: Linear from `distance / range ≥ 1`, the inverse
    /// models from `distance > range`. The range is clamped as `falloff`
    /// clamps it; the margin covers `slack` and [`DISTANCE_SLACK`].
    fn beyond_range(&self, light: &MapLight, origin: DVec3) -> bool {
        let range = f64::from(light.falloff_range.max(1.0e-4));
        self.distance_to(origin) - self.slack > range * (1.0 + DISTANCE_SLACK)
    }

    /// Whether every texel centre lies outside the spot's outer cone, by
    /// enough that the bake's cos θ rounds to at most cos outer, where its
    /// smoothstep is exactly zero whatever the inner angle. Tested on the
    /// rectangle's bounding sphere: the directions into it form a cap about
    /// the centre, whose nearest direction to the aim is at `α − β` (`α` the
    /// centre's angle off the aim, `sin β = radius / distance`).
    ///
    /// Declines, which is exact, for an outer angle outside `(0, π/2)`, a zero
    /// aim, and a light inside the sphere; authored spots sit well inside
    /// that range.
    fn outside_cone(&self, light: &MapLight, origin: DVec3) -> bool {
        let (aim, _, outer) = spot_cone_angles(light);
        if !(outer > 0.0 && outer < FRAC_PI_2) || aim == Vec3::ZERO {
            return false;
        }
        let to_centre = self.centre() - origin;
        let distance = to_centre.length();
        let radius = self.radius();
        if distance <= radius {
            return false;
        }
        let axis = aim.as_dvec3().normalize();
        let along = axis.dot(to_centre);
        let across = (to_centre - axis * along).length();
        if along > 0.0 && across < radius {
            // `α < β`: the cap holds the aim.
            return false;
        }
        // cos(α − β), with cos α = along / d and sin α = across / d.
        let max_cos = (along * (distance * distance - radius * radius).sqrt() + across * radius)
            / (distance * distance);
        max_cos <= f64::from(outer.cos()) - COS_SLACK
    }

    /// Exact distance from `p` to the rectangle.
    fn distance_to(&self, p: DVec3) -> f64 {
        let offset = p - self.corner;
        let uu = self.u_edge.length_squared();
        let vv = self.v_edge.length_squared();
        let uv = self.u_edge.dot(self.v_edge);
        let det = uu * vv - uv * uv;
        if det > 0.0 {
            let (ou, ov) = (offset.dot(self.u_edge), offset.dot(self.v_edge));
            let s = (vv * ou - uv * ov) / det;
            let t = (uu * ov - uv * ou) / det;
            if (0.0..=1.0).contains(&s) && (0.0..=1.0).contains(&t) {
                return (offset - self.u_edge * s - self.v_edge * t).length();
            }
        }
        // Otherwise the nearest point is on the boundary, which also covers a
        // one-texel row, column or chart.
        let [c00, c10, c01, _] = self.corners();
        [
            (c00, self.u_edge),
            (c00, self.v_edge),
            (c10, self.v_edge),
            (c01, self.u_edge),
        ]
        .into_iter()
        .map(|(start, edge)| distance_to_segment(p, start, edge))
        .fold(f64::INFINITY, f64::min)
    }
}

fn distance_to_segment(p: DVec3, start: DVec3, edge: DVec3) -> f64 {
    let length_squared = edge.length_squared();
    let t = if length_squared > 0.0 {
        ((p - start).dot(edge) / length_squared).clamp(0.0, 1.0)
    } else {
        0.0
    };
    p.distance(start + edge * t)
}
