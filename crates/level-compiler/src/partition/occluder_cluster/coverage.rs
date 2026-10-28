// Cluster-section coverage: the share of a section's bounding broad face its
// members' solid covers, rasterized on a fixed grid in the section's frame.
// See: context/plans/in-progress/occluder-first-partition/research.md §Clusters

use glam::{DVec2, DVec3};

/// Cells along each broad axis of a section's face. Fixed, so cost is bounded
/// whatever the face's size, and divisible by 2, 3, 4, 5, 6, 8, 10, 12, 15 and
/// 16, so simple fractions of a face land on cell boundaries and a face cut
/// that way reads its exact share.
pub const COVERAGE_GRID_CELLS: usize = 240;

/// Slack on the inside test, far under a cell, so a member edge on a cell
/// boundary never claims the cells on its other side.
const INSIDE_EPSILON: f64 = 1e-9;

/// Cells of the `COVERAGE_GRID_CELLS`² grid over the face spanned by `u` and
/// `v` (unit, perpendicular) whose centres some member covers, as
/// `(covered, total)`. The face runs `lo..lo + extent` along each axis; each
/// member is its vertices, projected onto the face as its silhouette.
pub(super) fn covered_cells(
    axes: [DVec3; 2],
    lo: DVec2,
    extent: DVec2,
    members: &[&[DVec3]],
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
    (covered.iter().filter(|&&cell| cell).count(), total)
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
    points.dedup_by(|a, b| (*a - *b).abs().max_element() <= INSIDE_EPSILON);
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
