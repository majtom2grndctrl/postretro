// Cluster-section coverage: the share of a broad face of a section's bounding
// box, under that face's roofline, that its members' solid covers, rasterized
// on a fixed grid in the section's frame.
// See: context/plans/in-progress/occluder-first-partition/index.md §Decisions (Clusters)

use glam::{DVec2, DVec3};

/// Cells along each broad axis of a section's face. Fixed, so cost is bounded
/// whatever the face's size, and divisible by 2, 3, 4, 5, 6, 8, 10, 12, 15 and
/// 16, so simple fractions of a face land on cell boundaries and a face cut
/// that way reads its exact share.
pub const COVERAGE_GRID_CELLS: usize = 240;

/// Slack on the inside test, far under a cell, so a member edge on a cell
/// boundary never claims the cells on its other side.
const INSIDE_EPSILON: f64 = 1e-9;

/// The see-over line across a face whose second axis points up: over each
/// column along the first axis, the face's points at world height
/// `heights[column]`, the see-over height above the floor in front of that
/// column. A face point at coordinates `(s, t)` stands at world height
/// `plane_y + s·axes[0].y + t·axes[1].y`, where `plane_y` is the height the
/// face plane's offset along its normal adds.
pub(super) struct SeeOverLine {
    /// One height per column, `COVERAGE_GRID_CELLS` of them.
    pub(super) heights: Vec<f64>,
    pub(super) plane_y: f64,
}

/// Cells of the `COVERAGE_GRID_CELLS`² grid over the face spanned by `axes`
/// (unit, perpendicular) that count, and those of them whose centres some
/// member covers, as `(covered, counted)`. The face runs `lo..lo + extent`
/// along each axis; each member is its vertices, projected onto the face as
/// its silhouette.
///
/// With no `roofline` every cell counts. With one, `axes[1]` points up and
/// each column along `axes[0]` counts from the face's foot to the higher of
/// its highest covered cell and its own see-over line at the column's
/// centre: sky over a lower roof does not count, while a column whose solid
/// stops under the line, or that holds none, still counts up to it.
pub(super) fn covered_cells(
    axes: [DVec3; 2],
    lo: DVec2,
    extent: DVec2,
    members: &[&[DVec3]],
    roofline: Option<&SeeOverLine>,
) -> (usize, usize) {
    let n = COVERAGE_GRID_CELLS;
    let total = n * n;
    if extent.x <= 0.0 || extent.y <= 0.0 {
        return (0, total);
    }
    let mut covered = vec![false; total];
    for vertices in members {
        let grid: Vec<DVec2> = vertices
            .iter()
            .map(|p| {
                let face = DVec2::new(p.dot(axes[0]), p.dot(axes[1]));
                (face - lo) / extent * n as f64
            })
            .collect();
        let silhouette = convex_hull(grid);
        if silhouette.len() < 3 {
            continue;
        }
        let (min, max) = silhouette.iter().fold(
            (DVec2::splat(f64::INFINITY), DVec2::splat(f64::NEG_INFINITY)),
            |(min, max), &p| (min.min(p), max.max(p)),
        );
        let cells = |lo: f64, hi: f64| {
            let first = (lo - 0.5).ceil().max(0.0) as usize;
            let last = ((hi - 0.5).floor() as isize).min(n as isize - 1);
            first..(last + 1).max(0) as usize
        };
        for j in cells(min.y, max.y) {
            for i in cells(min.x, max.x) {
                let centre = DVec2::new(i as f64 + 0.5, j as f64 + 0.5);
                if inside(&silhouette, centre) {
                    covered[j * n + i] = true;
                }
            }
        }
    }
    let covered_count = covered.iter().filter(|&&cell| cell).count();
    let Some(line) = roofline else {
        return (covered_count, total);
    };
    debug_assert_eq!(line.heights.len(), n, "one see-over height per column");
    let counted = (0..n)
        .zip(&line.heights)
        .map(|(i, &height)| {
            let solid_top = (0..n)
                .rev()
                .find(|&j| covered[j * n + i])
                .map_or(0, |j| j + 1);
            let s = lo.x + (i as f64 + 0.5) / n as f64 * extent.x;
            let t = (height - line.plane_y - s * axes[0].y) / axes[1].y;
            // Rows whose centres lie at or under the line, within the face.
            let rows = (t - lo.y) / extent.y * n as f64;
            let under_line = (rows + 0.5 + INSIDE_EPSILON).floor().clamp(0.0, n as f64) as usize;
            solid_top.max(under_line)
        })
        .sum();
    (covered_count, counted)
}

/// True when `p` lies inside the counter-clockwise convex `polygon`.
fn inside(polygon: &[DVec2], p: DVec2) -> bool {
    (0..polygon.len()).all(|i| {
        let a = polygon[i];
        let b = polygon[(i + 1) % polygon.len()];
        (b - a).perp_dot(p - a) >= -INSIDE_EPSILON
    })
}

/// Counter-clockwise convex hull (Andrew's monotone chain), collinear points
/// dropped.
fn convex_hull(mut points: Vec<DVec2>) -> Vec<DVec2> {
    points.sort_by(|a, b| a.x.total_cmp(&b.x).then(a.y.total_cmp(&b.y)));
    // Every near-duplicate goes, not only neighbours: rounding can sort the
    // copies of one corner apart, and the chain would then pop a true corner
    // across their near-zero spacing. The first copy in sorted order stays,
    // so the result does not depend on vertex order.
    let mut unique: Vec<DVec2> = Vec::with_capacity(points.len());
    for point in points {
        if !unique
            .iter()
            .any(|kept| (point - *kept).abs().max_element() <= INSIDE_EPSILON)
        {
            unique.push(point);
        }
    }
    let points = unique;
    if points.len() < 3 {
        return points;
    }
    let mut hull: Vec<DVec2> = Vec::with_capacity(points.len() * 2);
    for pass in [
        points.clone(),
        points.iter().rev().copied().collect::<Vec<_>>(),
    ] {
        let start = hull.len();
        for point in pass {
            while hull.len() >= start + 2
                && (hull[hull.len() - 1] - hull[hull.len() - 2])
                    .perp_dot(point - hull[hull.len() - 2])
                    <= INSIDE_EPSILON
            {
                hull.pop();
            }
            hull.push(point);
        }
        hull.pop();
    }
    hull
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A box's corners from `lo` to `hi` on the face's `(along, up)` plane.
    fn rect(lo: DVec2, hi: DVec2) -> Vec<DVec3> {
        [lo.x, hi.x]
            .into_iter()
            .flat_map(|x| [lo.y, hi.y].map(|y| DVec3::new(x, y, 0.0)))
            .flat_map(|p| [p, p + DVec3::Z])
            .collect()
    }

    /// Coverage over a face 24 m along and 6 m up, grid cells 0.1 × 0.025 m,
    /// standing on the floor with the see-over line 2.5 m up when
    /// `roofline`.
    fn cells(members: &[Vec<DVec3>], roofline: bool) -> (usize, usize) {
        let line = SeeOverLine {
            heights: vec![2.5; COVERAGE_GRID_CELLS],
            plane_y: 0.0,
        };
        cells_under(members, roofline.then_some(&line))
    }

    /// Coverage over `cells`' face under `roofline`.
    fn cells_under(members: &[Vec<DVec3>], roofline: Option<&SeeOverLine>) -> (usize, usize) {
        let members: Vec<&[DVec3]> = members.iter().map(Vec::as_slice).collect();
        covered_cells(
            [DVec3::X, DVec3::Y],
            DVec2::ZERO,
            DVec2::new(24.0, 6.0),
            &members,
            roofline,
        )
    }

    #[test]
    fn roofline_counts_each_column_up_to_its_solid_or_the_see_over_line() {
        // Columns 0..60 under a 6 m tower, 60..120 under a 4 m one, 120..180
        // under a 1 m wall, 180..240 empty.
        let members = [
            rect(DVec2::ZERO, DVec2::new(6.0, 6.0)),
            rect(DVec2::new(6.0, 0.0), DVec2::new(12.0, 4.0)),
            rect(DVec2::new(12.0, 0.0), DVec2::new(18.0, 1.0)),
        ];
        let (covered, counted) = cells(&members, true);
        assert_eq!(covered, 60 * 240 + 60 * 160 + 60 * 40);
        assert_eq!(counted, 60 * 240 + 60 * 160 + 60 * 100 + 60 * 100);
    }

    #[test]
    fn roofline_counts_each_column_up_to_its_own_see_over_line() {
        // A wall 1 m tall the face's length. Over columns 0..120 the floor in
        // front lies at the face's foot, so they count to 2.5 m; over
        // 120..240 it lies 2 m lower, under a stairwell, so the line stands
        // at 0.5 m and they count only to the wall's top.
        let wall = [rect(DVec2::ZERO, DVec2::new(24.0, 1.0))];
        let half = COVERAGE_GRID_CELLS / 2;
        let line = SeeOverLine {
            heights: (0..COVERAGE_GRID_CELLS)
                .map(|i| if i < half { 2.5 } else { 0.5 })
                .collect(),
            plane_y: 0.0,
        };
        let (covered, counted) = cells_under(&wall, Some(&line));
        assert_eq!(covered, 240 * 40);
        assert_eq!(counted, 120 * 100 + 120 * 40);
    }

    // Regression: a pier whose clipped corners carried rounding of up to
    // 1e-12 m rasterized as a triangle. Near-duplicate projections that did
    // not sort next to each other survived the dedup, and the hull popped a
    // true corner across their near-zero spacing.
    #[test]
    fn silhouette_keeps_every_corner_when_duplicate_corners_differ_by_rounding() {
        // A pier 1 × 15 m of a facade, its corners as its brush hull lists
        // them, once per facet.
        let (x0, x1) = (31.400000000000002, 32.400000000000006);
        let noisy = [
            (x1, 0.0, 20.0),
            (x1, 0.0, 20.3),
            (x1, 15.0, 20.3),
            (x1, 15.0, 20.0),
            (x0, 15.0, 20.0),
            (x0, 15.0, 20.3),
            (x0, 0.0, 20.3),
            (x0, 0.0, 20.0),
            (32.40000000000066, 15.0, 20.0),
            (32.400000000000425, 15.0, 20.3),
            (31.400000000000635, 15.0, 20.3),
            (31.400000000000823, 15.0, 20.0),
            (31.400000000000635, 0.0, 20.0),
            (31.400000000000823, 0.0, 20.3),
            (32.40000000000067, 0.0, 20.3),
            (32.40000000000043, 0.0, 20.0),
            (31.40000000000073, 0.0, 20.3),
            (31.40000000000073, 15.0, 20.3),
            (32.400000000000546, 15.0, 20.3),
            (32.400000000000546, 0.0, 20.3),
            (31.400000000000727, 15.0, 20.0),
            (31.400000000000727, 0.0, 20.0),
            (32.400000000000546, 0.0, 20.0),
            (32.400000000000546, 15.0, 20.0),
        ];
        let pier: Vec<DVec3> = noisy.map(|(x, y, z)| DVec3::new(x, y, z)).to_vec();
        let (covered, _) = covered_cells(
            [DVec3::X, DVec3::Y],
            DVec2::new(8.0, 0.0),
            DVec2::new(45.0, 21.0),
            &[&pier],
            None,
        );
        // Columns 125..=129 (centres 23.53..24.28 m along), rows 0..171
        // (centres up to 14.96 m).
        assert_eq!(covered, 5 * 171);
    }

    #[test]
    fn no_roofline_counts_every_cell_of_the_face() {
        let members = [rect(DVec2::ZERO, DVec2::new(6.0, 6.0))];
        assert_eq!(cells(&members, false), (60 * 240, 240 * 240));
    }
}
