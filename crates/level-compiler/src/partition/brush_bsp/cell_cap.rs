// Cell-size cap of the occluder tier: world-grid splitters that divide an
// occluder-tier leaf wider than the cap, ranked after occluder, wall free-end
// and door planes.
// See: context/plans/in-progress/occluder-first-partition/index.md §Decisions

use glam::DVec3;

use super::{REGION_CLASSIFICATION_EPSILON, planes_equivalent};
use crate::geometry_utils::{clip_winding_to_half_spaces, polygon_area};
use crate::partition::occluder_classify::{BrushFrame, OccluderClassification, OpenSpace};
use crate::partition::region_polytope::RegionPolytope;
use crate::partition::types::Aabb;

/// Widest horizontal extent an occluder-tier leaf keeps before the cap splits
/// it. Without a cap, one interior cell holds everything exempt brushes used
/// to cut apart, so it draws whole and its residency cannot divide. 1024
/// authoring units at the 0.0254 m unit scale, q3map's blocksize, so grid
/// lines fall on the coarse authoring grid where walls and floors already
/// meet. Of the swept 512, 768 and 1024 units it walks least on every map.
pub const CELL_SIZE_CAP_M: f64 = 26.0096;

/// A cap line must leave each piece at least this share of the cap along the
/// split axis, so the cap never cuts a sliver cell off a region's edge, such
/// as a section's slab, which bounds the region it lies in. Only a region
/// under one and a half caps can stay whole on this guard alone: wider, the
/// line nearest its middle leaves both pieces at least a quarter cap.
pub const CAP_MIN_PIECE_FRACTION: f64 = 0.25;

/// A cap line's slice through its region, within the bounds of the region's
/// open space, must be strictly more than this share open by area: neither
/// solid nor exterior void in the first pass. A line run along a wall's
/// thickness or through a solid filled by touching brushes adds a boundary
/// where little crosses it, and a region of exterior void, such as the slack
/// around the world, has no open slice to split. Solid outside the bounds of
/// the region's open space, such as exempt mass over a corridor in a taller
/// region, is not judged: no open space lies across it to divide.
pub const CAP_MIN_OPEN_SLICE_FRACTION: f64 = 0.5;

/// Clipping tolerance for a cap line's slice against the bounds of its
/// region's open space, as region clipping uses.
const SLICE_CLIP_EPSILON_M: f64 = 1e-6;

/// How far to either side of a cap line the open-slice guard reads its
/// slice. A slice on the line itself can lie on brush faces, where the first
/// pass files it to whichever side its node plane, chosen by brush order,
/// puts in front; just off the line, each reading is exact.
const SLICE_SIDE_OFFSET_M: f64 = REGION_CLASSIFICATION_EPSILON;

/// Share of a slice's area by which its open area must clear
/// `CAP_MIN_OPEN_SLICE_FRACTION`, so a slice exactly half open on the
/// authoring grid, such as a corridor under exempt mass as tall again, reads
/// half open whatever the rounding of its summed pieces.
const SLICE_AREA_TOLERANCE: f64 = 1e-9;

/// A section whose slab normal lies within 30° of a cap axis runs along that
/// axis's lines: one crossing its slab stays inside it for at least twice the
/// slab's thickness, and would cut the openings along it back into thin cells
/// of their own, the window cells one portal per window removed.
const SLAB_ALONG_MIN_COS: f64 = 0.866_025_403_784_438_6;

/// The horizontal world axes the cap splits on; it never splits along up.
const CAP_AXES: [usize; 2] = [0, 2];

/// What the cap reads besides region geometry: the first pass's open space,
/// for the open-slice guard, and the slabs of occluding cluster sections.
pub struct CellSizeCap<'a> {
    space: OpenSpace<'a>,
    slabs: Vec<Slab>,
}

/// An occluding section's bounding box, as the slab guard tests it.
struct Slab {
    /// Across the slab: the section's normal.
    normal: DVec3,
    /// Inward half-spaces of the box, inset by the classification epsilon so
    /// a region that only touches the slab does not reach into it.
    inward: [(DVec3, f64); 6],
    bounds: Aabb,
}

impl Slab {
    fn of(frame: &BrushFrame) -> Self {
        let mut inward = [(DVec3::ZERO, 0.0); 6];
        for axis in 0..3 {
            let (dir, centre) = (frame.axes[axis], frame.center.dot(frame.axes[axis]));
            let half = frame.extents[axis] * 0.5 - REGION_CLASSIFICATION_EPSILON;
            inward[2 * axis] = (dir, centre - half);
            inward[2 * axis + 1] = (-dir, -(centre + half));
        }
        Self {
            normal: frame.axes[2],
            inward,
            bounds: frame_bounds(frame),
        }
    }

    /// True when the part of `region` inside the slab reaches both sides of
    /// the plane: the plane runs through the slab within the region.
    fn crossed_within(&self, region: &RegionPolytope, (normal, line): (DVec3, f64)) -> bool {
        let mut inside = region.clone();
        for &(dir, distance) in &self.inward {
            inside = inside.clip(dir, distance).0;
            if inside.vertices().next().is_none() {
                return false;
            }
        }
        inside.plane_spans(normal, line, REGION_CLASSIFICATION_EPSILON)
    }
}

impl<'a> CellSizeCap<'a> {
    pub fn new(space: OpenSpace<'a>, classification: &OccluderClassification) -> Self {
        let slabs = classification
            .clusters
            .iter()
            .flat_map(|cluster| &cluster.sections)
            .filter(|section| section.occludes)
            .map(|section| Slab::of(&section.frame))
            .collect();
        Self { space, slabs }
    }

    /// The cap splitter for a region no occluder, wall free-end or door plane
    /// spans, or `None` when the region fits the cap or no line passes the
    /// guards.
    /// Axes go widest first, ties in axis order; on each axis, world-grid
    /// lines at multiples of the cap go nearest the region's middle first,
    /// ties to the lower line. The choice reads only the region, the first
    /// pass and the classification, so it does not depend on brush order.
    pub(super) fn splitter(
        &self,
        region: &RegionPolytope,
        ancestor_planes: &[(DVec3, f64)],
    ) -> Option<(DVec3, f64)> {
        let bounds = region.vertex_aabb();
        if !bounds.is_valid() {
            return None;
        }
        let extent = |axis: usize| bounds.max[axis] - bounds.min[axis];
        let mut axes: Vec<usize> = CAP_AXES
            .into_iter()
            .filter(|&axis| extent(axis) > CELL_SIZE_CAP_M + REGION_CLASSIFICATION_EPSILON)
            .collect();
        if axes.is_empty() {
            return None;
        }
        axes.sort_by(|&a, &b| extent(b).total_cmp(&extent(a)).then(a.cmp(&b)));
        let open = self.space.open_bounds_within(&bounds);
        if !open.is_valid() {
            return None;
        }

        axes.into_iter().find_map(|axis| {
            grid_lines(bounds.min[axis], bounds.max[axis])
                .into_iter()
                .map(|line| {
                    let mut normal = DVec3::ZERO;
                    normal[axis] = 1.0;
                    (normal, line)
                })
                .find(|&plane| self.accepts(axis, plane, region, &bounds, &open, ancestor_planes))
        })
    }

    fn accepts(
        &self,
        axis: usize,
        (normal, line): (DVec3, f64),
        region: &RegionPolytope,
        bounds: &Aabb,
        open: &Aabb,
        ancestor_planes: &[(DVec3, f64)],
    ) -> bool {
        let least_piece = CAP_MIN_PIECE_FRACTION * CELL_SIZE_CAP_M;
        if line - bounds.min[axis] < least_piece || bounds.max[axis] - line < least_piece {
            return false;
        }
        if ancestor_planes.iter().any(|&ancestor| {
            planes_equivalent(ancestor, (normal, line))
                || planes_equivalent(ancestor, (-normal, -line))
        }) {
            return false;
        }
        region.plane_spans(normal, line, REGION_CLASSIFICATION_EPSILON)
            && !self.inside_a_slab(axis, (normal, line), region, bounds)
            && self.slice_mostly_open(axis, (normal, line), region, open)
    }

    /// True when the line runs through the slab of an occluding section that
    /// lies along it, within the region.
    fn inside_a_slab(
        &self,
        axis: usize,
        plane: (DVec3, f64),
        region: &RegionPolytope,
        region_bounds: &Aabb,
    ) -> bool {
        self.slabs.iter().any(|slab| {
            slab.normal[axis].abs() >= SLAB_ALONG_MIN_COS
                && (0..3).all(|a| {
                    slab.bounds.min[a] < region_bounds.max[a]
                        && region_bounds.min[a] < slab.bounds.max[a]
                })
                && slab.crossed_within(region, plane)
        })
    }

    /// True when, just to each side of the line, strictly more than
    /// `CAP_MIN_OPEN_SLICE_FRACTION` of the slice through `region`, clipped
    /// to `open`, the bounds of the region's open space, lies in open space,
    /// by area. Open space crosses the line only where it is open on both
    /// sides, and neither reading depends on how the first pass oriented a
    /// node plane lying on the line.
    fn slice_mostly_open(
        &self,
        axis: usize,
        (normal, line): (DVec3, f64),
        region: &RegionPolytope,
        open: &Aabb,
    ) -> bool {
        [-SLICE_SIDE_OFFSET_M, SLICE_SIDE_OFFSET_M]
            .into_iter()
            .all(|offset| self.slice_open_at(axis, (normal, line + offset), region, open))
    }

    /// The open-slice test on the slice of `region` along the plane
    /// `normal · p = line`.
    fn slice_open_at(
        &self,
        axis: usize,
        (normal, line): (DVec3, f64),
        region: &RegionPolytope,
        open: &Aabb,
    ) -> bool {
        let Some(slice) = region.slice(normal, line) else {
            return false;
        };
        let mut within_open = Vec::with_capacity(4);
        for a in [(axis + 1) % 3, (axis + 2) % 3] {
            let mut across = DVec3::ZERO;
            across[a] = 1.0;
            within_open.push((across, open.min[a]));
            within_open.push((-across, -open.max[a]));
        }
        let Some(slice) = clip_winding_to_half_spaces(slice, &within_open, SLICE_CLIP_EPSILON_M)
        else {
            return false;
        };
        let area = polygon_area(&slice);
        let least_open = (CAP_MIN_OPEN_SLICE_FRACTION + SLICE_AREA_TOLERANCE) * area;
        area > 0.0 && self.space.open_area(&slice) > least_open
    }
}

// Hooks for the cap rows, which run bin-side in `pipeline::runtime_cells`
// against the first pass's exterior flood; the lib test target never calls them.
#[cfg(test)]
#[allow(dead_code)]
impl CellSizeCap<'_> {
    /// Whether the cap takes the grid line at `line` on world `axis` in
    /// `region`, with no ancestor planes.
    pub(crate) fn accepts_line(&self, axis: usize, line: f64, region: &RegionPolytope) -> bool {
        let mut normal = DVec3::ZERO;
        normal[axis] = 1.0;
        let bounds = region.vertex_aabb();
        let open = self.space.open_bounds_within(&bounds);
        self.accepts(axis, (normal, line), region, &bounds, &open, &[])
    }

    /// Whether the line runs through any occluding section's slab within
    /// `region`, whatever the slab's direction.
    pub(crate) fn crosses_a_slab(&self, axis: usize, line: f64, region: &RegionPolytope) -> bool {
        let mut normal = DVec3::ZERO;
        normal[axis] = 1.0;
        self.slabs
            .iter()
            .any(|slab| slab.crossed_within(region, (normal, line)))
    }
}

/// The world-grid lines at multiples of the cap between `min` and `max`,
/// nearest their middle first, ties to the lower line.
fn grid_lines(min: f64, max: f64) -> Vec<f64> {
    let first = (min / CELL_SIZE_CAP_M).ceil() as i64;
    let last = (max / CELL_SIZE_CAP_M).floor() as i64;
    let middle = 0.5 * (min + max);
    let mut lines: Vec<i64> = (first..=last).collect();
    lines.sort_by(|&a, &b| {
        let off = |k: i64| (k as f64 * CELL_SIZE_CAP_M - middle).abs();
        off(a).total_cmp(&off(b)).then(a.cmp(&b))
    });
    lines
        .into_iter()
        .map(|k| k as f64 * CELL_SIZE_CAP_M)
        .collect()
}

/// World bounds of an own-frame box.
fn frame_bounds(frame: &BrushFrame) -> Aabb {
    let reach = (0..3)
        .map(|axis| frame.axes[axis].abs() * (frame.extents[axis] * 0.5))
        .fold(DVec3::ZERO, |sum, half| sum + half);
    Aabb {
        min: frame.center - reach,
        max: frame.center + reach,
    }
}
