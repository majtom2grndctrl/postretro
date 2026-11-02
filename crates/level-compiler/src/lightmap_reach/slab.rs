// Chart and per-texel slabs: the full-tree leaves a chart's shadow-trace
// starts can lie in, and which texels can start one in a closed leaf.
// See: context/lib/build_pipeline.md §Compiler pipeline step 12 (Exact skips)

use glam::DVec3;

use crate::chart_raster::{ChartGrid, chart_grid, chart_interior_dims, chart_texel_world_position};
use crate::lightmap_bake::{Chart, RAY_EPSILON};
use crate::lightmap_layer::{FRAME_NORM_LIMIT, FRAME_SKEW_LIMIT, exact_texel_local};
use crate::partition::region_polytope::RegionPolytope;
use crate::partition::{Aabb, BspChild, BspTree};
use crate::texel_footprint::TexelFrame;

use super::ChartReach;
use super::flood::Emitter;

/// Relative slack on the footprint radius: offsets go through `f32` texel
/// steps along axes within [`FRAME_NORM_LIMIT`] of unit length.
const RADIUS_SLACK: f64 = 1.0e-5;

/// Relative slack on the lift and the skipped `RAY_EPSILON`, each an `f32`
/// product of a near-unit vector.
const EPSILON_SLACK: f64 = 1.0e-5;

/// Bound, per metre of distance from the world origin, on how far an `f32`
/// shadow-trace start lies from the exact sum of its `f32` texel centre and
/// the offsets the walk adds to it. Three `f32` adds place it (the footprint
/// point, the lift, the trace start), each rounding within 2⁻²⁴ (6e-8) of
/// its result's length; one more 6e-8 is spare.
const TRACE_START_SLACK_PER_METER: f64 = 4.0 * 6.0e-8;

/// Bound, per metre of the chart's coordinates, on the `f64` rounding of a
/// texel centre's measured drift: a few 2⁻⁵³ (1.1e-16).
const DRIFT_SLACK_PER_METER: f64 = 1.0e-12;

/// Absolute slack under those bounds: subnormal results, and coordinates
/// near the origin.
const TRACE_START_SLACK: f64 = 1.0e-6;

/// How far past a tree plane a slab piece still counts as touching its side:
/// well above `RegionPolytope`'s 1e-6 clip epsilon and the `f64` rounding of
/// the clipped vertices, so a computed leaf set is a superset of the leaves
/// the slab touches.
const DESCENT_TOLERANCE: f64 = 1.0e-5;

/// Gap, in metres, by which two ranges must miss each other to count as
/// separated: above the `f64` rounding of slab corners and piece vertices.
const SEPARATION_TOLERANCE: f64 = 1.0e-9;

/// A box in chart-frame coordinates: `a` along `u_axis`, `b` along `v_axis`,
/// `c` along the normal, about the chart origin.
#[derive(Debug, Clone, Copy)]
pub(super) struct FrameBox {
    pub(super) lo: DVec3,
    pub(super) hi: DVec3,
}

/// A chart's frame in `f64` and the slab every texel's shadow-trace starts
/// lie in, as the walk reconstructs them (`chart_texel_world_position` with
/// the stored frame, never the face plane).
///
/// Each texel's slab sits on its exact affine centre (`exact_texel_local`,
/// frame height 0), moved by the drift of the walk's `f32` centres from
/// those: measured over every texel, in frame coordinates, so the slab
/// rounds only the few `f32` adds the walk makes after the centre.
///
/// A shadow trace from a footprint point starts at `point + normal·ε +
/// dir·ε` (`segment_clear` skips its first `RAY_EPSILON`), `point` within a
/// footprint radius of the texel centre in the chart's plane. Its frame
/// height is `ε + ε·dir_c`; when the emitter clears the slab
/// ([`ChartFrame::emitter_clears`]) `dir_c ≥ 0`, so it lies in `[ε, 2ε]`
/// widened by rounding. In-plane it lies within the footprint radius and one
/// `ε` of the centre. A stored 0 is exact when every one of those traces is
/// blocked, so these are the only points the slab must hold.
pub(super) struct ChartFrame {
    origin: DVec3,
    axes: [DVec3; 3],
    /// Dual basis: the frame coordinates of `p` are `dual[i] · (p − origin)`.
    dual: [DVec3; 3],
    grid: ChartGrid,
    dims: (i32, i32),
    /// A texel's box about its exact affine centre `(a, b, 0)`: its low and
    /// high corners' offsets.
    extent: [DVec3; 2],
}

impl ChartFrame {
    /// `None` when the chart has no texels or its frame is too far from
    /// orthonormal to bound: its pairs are then kept.
    pub(super) fn new(chart: &Chart, penumbra_floor: bool) -> Option<Self> {
        if chart.uv_extent[0] <= 0.0 || chart.uv_extent[1] <= 0.0 {
            return None;
        }
        let grid = chart_grid(chart);
        let origin = chart.origin.as_dvec3();
        let axes = [
            chart.u_axis.as_dvec3(),
            chart.v_axis.as_dvec3(),
            chart.normal.as_dvec3(),
        ];
        let unit = |v: DVec3| (v.length() - 1.0).abs() <= FRAME_NORM_LIMIT;
        let orthogonal = |a: DVec3, b: DVec3| a.dot(b).abs() <= FRAME_SKEW_LIMIT;
        if !(axes.iter().all(|&axis| unit(axis))
            && orthogonal(axes[0], axes[1])
            && orthogonal(axes[0], axes[2])
            && orthogonal(axes[1], axes[2])
            && origin.is_finite())
        {
            return None;
        }
        let det = axes[0].dot(axes[1].cross(axes[2]));
        let dual = [
            axes[1].cross(axes[2]) / det,
            axes[2].cross(axes[0]) / det,
            axes[0].cross(axes[1]) / det,
        ];
        let radius = if penumbra_floor {
            f64::from(TexelFrame::for_chart(chart).radius())
        } else {
            0.0
        };
        let dims = chart_interior_dims(chart);
        let mut frame = Self {
            origin,
            axes,
            dual,
            grid,
            dims,
            extent: [DVec3::ZERO; 2],
        };
        // The walk's centres: their drift from the exact affine centres, in
        // frame coordinates, and their largest distance from the world
        // origin.
        let (mut drift_lo, mut drift_hi) = (DVec3::ZERO, DVec3::ZERO);
        let mut centre_magnitude = 0.0f64;
        for ty in 0..dims.1 {
            for tx in 0..dims.0 {
                let centre = chart_texel_world_position(chart, tx, ty).as_dvec3();
                let drift = frame.frame_coords(centre)
                    - DVec3::new(frame.local(0, tx), frame.local(1, ty), 0.0);
                if !drift.is_finite() {
                    return None;
                }
                drift_lo = drift_lo.min(drift);
                drift_hi = drift_hi.max(drift);
                centre_magnitude = centre_magnitude.max(centre.length());
            }
        }
        // Every start lies within a footprint radius and two `RAY_EPSILON`s
        // of its centre, so this bounds the length of every `f32` sum the
        // walk rounds after the centre.
        let magnitude = centre_magnitude + radius + 3.0 * f64::from(RAY_EPSILON);
        let span = |axis: usize| {
            f64::from(frame.grid.uv_min[axis]).abs() + f64::from(frame.grid.uv_extent[axis]).abs()
        };
        let coordinates = origin.length() + span(0) + span(1) + centre_magnitude;
        let slack = TRACE_START_SLACK_PER_METER * magnitude
            + DRIFT_SLACK_PER_METER * coordinates
            + TRACE_START_SLACK;
        let epsilon = f64::from(RAY_EPSILON) * (1.0 + EPSILON_SLACK);
        let radius_slack = radius * RADIUS_SLACK;
        let in_plane =
            |axis: usize| radius + radius_slack + (epsilon + slack) * dual[axis].length();
        let height_slack = slack * dual[2].length() + radius_slack;
        let pad = DVec3::new(in_plane(0), in_plane(1), 0.0);
        let lowest = f64::from(RAY_EPSILON) * (1.0 - EPSILON_SLACK) - height_slack;
        let highest = epsilon * (1.0 + dual[2].length()) + height_slack;
        frame.extent = [
            drift_lo - pad + DVec3::Z * lowest,
            drift_hi + pad + DVec3::Z * highest,
        ];
        let finite =
            frame.dual.iter().all(|d| d.is_finite()) && frame.extent.iter().all(|e| e.is_finite());
        finite.then_some(frame)
    }

    /// Whether every emitter target lies above the whole slab in frame
    /// height, so every shadow trace climbs from its lifted origin and its
    /// start stays in `[ε, 2ε]` up to rounding.
    pub(super) fn emitter_clears(&self, emitter: Emitter) -> bool {
        let height = self.dual[2].dot(emitter.centre - self.origin);
        let lowest = height - self.dual[2].length() * emitter.target_radius();
        lowest >= self.extent[1].z
    }

    fn local(&self, axis: usize, texel: i32) -> f64 {
        exact_texel_local(&self.grid, axis, texel)
    }

    pub(super) fn texel_box(&self, tx: i32, ty: i32) -> FrameBox {
        let centre = DVec3::new(self.local(0, tx), self.local(1, ty), 0.0);
        FrameBox {
            lo: centre + self.extent[0],
            hi: centre + self.extent[1],
        }
    }

    /// The union of every texel's box, widened by [`DESCENT_TOLERANCE`].
    fn chart_box(&self) -> FrameBox {
        let (first, last) = (
            self.texel_box(0, 0),
            self.texel_box(self.dims.0 - 1, self.dims.1 - 1),
        );
        let widen = DVec3::splat(DESCENT_TOLERANCE);
        FrameBox {
            lo: first.lo.min(last.lo) - widen,
            hi: first.hi.max(last.hi) + widen,
        }
    }

    fn world(&self, frame: DVec3) -> DVec3 {
        self.origin + self.axes[0] * frame.x + self.axes[1] * frame.y + self.axes[2] * frame.z
    }

    pub(super) fn frame_coords(&self, world: DVec3) -> DVec3 {
        let offset = world - self.origin;
        DVec3::new(
            self.dual[0].dot(offset),
            self.dual[1].dot(offset),
            self.dual[2].dot(offset),
        )
    }

    /// The box as a convex polytope in world space: its bounding AABB cut by
    /// the six frame planes.
    fn polytope(&self, frame_box: FrameBox) -> RegionPolytope {
        let corners: Vec<DVec3> = (0..8)
            .map(|i| {
                self.world(DVec3::new(
                    if i & 1 == 0 {
                        frame_box.lo.x
                    } else {
                        frame_box.hi.x
                    },
                    if i & 2 == 0 {
                        frame_box.lo.y
                    } else {
                        frame_box.hi.y
                    },
                    if i & 4 == 0 {
                        frame_box.lo.z
                    } else {
                        frame_box.hi.z
                    },
                ))
            })
            .collect();
        let mut bounds = Aabb::from_points(&corners);
        bounds.min -= DVec3::splat(1.0e-3);
        bounds.max += DVec3::splat(1.0e-3);
        let mut polytope = RegionPolytope::from_aabb(&bounds);
        for axis in 0..3 {
            let length = self.dual[axis].length();
            let normal = self.dual[axis] / length;
            let at = |coordinate: f64| (normal.dot(self.origin) * length + coordinate) / length;
            polytope = polytope.clip(normal, at(frame_box.lo[axis])).0;
            polytope = polytope.clip(normal, at(frame_box.hi[axis])).1;
        }
        polytope
    }

    /// The chart's open leaves and per-texel mask: one descent of the chart
    /// slab through the tree, then each texel's box tested against every
    /// closed piece the descent found. `None` when it finds no leaf: every
    /// slab lies in some leaf, so an empty set means the search lost it, and
    /// a pair culled on it would skip every texel.
    pub(super) fn reach(&self, tree: &BspTree, closed: &[bool]) -> Option<ChartReach> {
        let mut open_leaves = Vec::new();
        let mut pieces = Vec::new();
        if !tree.nodes.is_empty() {
            descend(
                tree,
                BspChild::Node(0),
                self.polytope(self.chart_box()),
                &mut |leaf, piece| {
                    if closed[leaf] {
                        pieces.push(piece);
                    } else {
                        open_leaves.push(leaf as u32);
                    }
                },
            );
        }
        if open_leaves.is_empty() && pieces.is_empty() {
            return None;
        }
        open_leaves.sort_unstable();
        open_leaves.dedup();
        let (width, height) = self.dims;
        let mut traced = vec![0u64; (width as usize * height as usize).div_ceil(64)];
        for piece in &pieces {
            self.mark_traced(piece, &mut traced);
        }
        Some(ChartReach {
            open_leaves,
            width,
            traced,
        })
    }

    /// Mark every texel whose box `piece` does not provably miss.
    fn mark_traced(&self, piece: &RegionPolytope, traced: &mut [u64]) {
        let vertices: Vec<DVec3> = piece.vertices().collect();
        if vertices.is_empty() {
            return;
        }
        let frame: Vec<DVec3> = vertices.iter().map(|&v| self.frame_coords(v)).collect();
        let lo = frame.iter().copied().fold(DVec3::INFINITY, DVec3::min);
        let hi = frame.iter().copied().fold(DVec3::NEG_INFINITY, DVec3::max);
        let axes: Vec<DVec3> = piece.facets().map(|(normal, _, _)| normal).collect();
        let (width, height) = self.dims;
        let candidates = |axis: usize, count: i32| {
            // `local` is increasing in the texel index; this range is only a
            // prefilter, widened by a texel each way, and every candidate is
            // tested exactly below.
            let grid = &self.grid;
            let index = |coordinate: f64| {
                (coordinate - f64::from(grid.uv_min[axis])) / f64::from(grid.uv_extent[axis])
                    * f64::from(grid.interior[axis])
                    - 0.5
                    - f64::from(grid.origin[axis])
            };
            let first = index(lo[axis] - self.extent[1][axis]).floor() - 1.0;
            let last = index(hi[axis] - self.extent[0][axis]).ceil() + 1.0;
            let first = first.clamp(0.0, f64::from(count - 1)) as i32;
            let last = last.clamp(0.0, f64::from(count - 1)) as i32;
            first..=last
        };
        for ty in candidates(1, height) {
            for tx in candidates(0, width) {
                let bit = ty as usize * width as usize + tx as usize;
                if traced[bit / 64] & (1 << (bit % 64)) != 0 {
                    continue;
                }
                let texel = self.texel_box(tx, ty);
                if !self.separated(texel, (lo, hi), &vertices, &axes) {
                    traced[bit / 64] |= 1 << (bit % 64);
                }
            }
        }
    }

    /// Whether a separating axis proves `texel` and the piece disjoint: the
    /// three frame axes, then the piece's facet normals. `false` when
    /// neither proves it, which keeps the texel traced.
    fn separated(
        &self,
        texel: FrameBox,
        (lo, hi): (DVec3, DVec3),
        vertices: &[DVec3],
        axes: &[DVec3],
    ) -> bool {
        let gap = SEPARATION_TOLERANCE;
        if (texel.hi + gap).cmplt(lo).any() || (texel.lo - gap).cmpgt(hi).any() {
            return true;
        }
        let centre = (texel.lo + texel.hi) * 0.5;
        let half = (texel.hi - texel.lo) * 0.5;
        let world_centre = self.world(centre);
        axes.iter().any(|&axis| {
            let reach = (0..3)
                .map(|i| (axis.dot(self.axes[i]) * half[i]).abs())
                .sum::<f64>();
            let middle = axis.dot(world_centre);
            let (min, max) =
                vertices
                    .iter()
                    .fold((f64::INFINITY, f64::NEG_INFINITY), |(min, max), v| {
                        let x = axis.dot(*v);
                        (min.min(x), max.max(x))
                    });
            let scale = gap * (1.0 + middle.abs());
            middle + reach + scale < min || middle - reach - scale > max
        })
    }
}

/// Push `piece` down the tree from `child`, handing every leaf it touches,
/// with the piece of it there, to `visit`. A side is entered when the piece
/// comes within [`DESCENT_TOLERANCE`] of it, and the piece clipped on a
/// plane shifted out by that tolerance, so each computed piece holds the
/// exact one.
fn descend(
    tree: &BspTree,
    child: BspChild,
    piece: RegionPolytope,
    visit: &mut impl FnMut(usize, RegionPolytope),
) {
    let node = match child {
        BspChild::Leaf(leaf) => return visit(leaf, piece),
        BspChild::Node(node) => &tree.nodes[node],
    };
    let (normal, distance) = (node.plane_normal, node.plane_distance);
    let (lo, hi) = piece
        .vertices()
        .map(|v| v.dot(normal) - distance)
        .fold((f64::INFINITY, f64::NEG_INFINITY), |(lo, hi), d| {
            (lo.min(d), hi.max(d))
        });
    if lo > hi {
        return;
    }
    let tolerance = DESCENT_TOLERANCE;
    let front = (hi >= -tolerance).then(|| {
        if lo < -tolerance {
            piece.clip(normal, distance - tolerance).0
        } else {
            piece.clone()
        }
    });
    let back = (lo <= tolerance).then(|| {
        if hi > tolerance {
            piece.clip(normal, distance + tolerance).1
        } else {
            piece
        }
    });
    for (side, child) in [(front, &node.front), (back, &node.back)] {
        if let Some(side) = side
            && side.vertices().next().is_some()
        {
            descend(tree, child.clone(), side, visit);
        }
    }
}
