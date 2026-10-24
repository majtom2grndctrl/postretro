// Texel-step detector: axis-adjacent jumps in stored per-light visibility on a parent texel grid.
// See: context/lib/build_pipeline.md §Compiler pipeline (step 12)

use glam::Vec3;

use crate::chart_raster::chart_texel_world_position;
use crate::lightmap_bake::{Chart, RAY_EPSILON};

mod report;
#[cfg(test)]
pub(crate) use report::StepSite;
pub(crate) use report::{
    ChartSteps, CutSites, TexelRect, TexelStepReport, merge_chart_steps, scan_chart,
};

/// Largest stored per-light visibility difference two axis-adjacent texels
/// may show before they count as a step. 0.78 is the one-texel cone kernel's
/// largest axis-adjacent difference: a mid-phase axis-aligned edge filters its
/// two texels to 0.11 and 0.89, and no occluder pattern does worse. 0.17 is
/// the sampling margin: a floored bake's worst pair reaches 0.917 (eleven of
/// twelve samples apart at 24 samples), while a hard 0/1 step is 1.0 and
/// always exceeds it; any margin under 0.22 keeps that guarantee.
pub(crate) const TEXEL_STEP_THRESHOLD: f32 = 0.78 + 0.17;

/// Whether a surface separates two texels of `chart`'s grid, given as
/// interior texel coordinates of `chart`: whether `clear(from, to)` finds the
/// segment between their ray origins (texel centres lifted `RAY_EPSILON`
/// along the normal) blocked, each end stretched past its texel so a surface
/// through a texel centre still counts.
///
/// Such a pair is no lattice step but a break in the receiver: a texel whose
/// centre lies under a solid resting on it (a chart's rectangle reaches past
/// its polygon there, or an uncut face runs beneath the solid) stores 0
/// beside a lit neighbour whose footprint the solid clipped, and so do two
/// texels either side of a wall thinner than a texel. The segment is the
/// footprint's own wall clip stretched to the neighbour, so it sees exactly
/// the surfaces that clip a footprint between them, and it never meets the
/// receiver or an occluder standing clear of the lift.
pub(crate) fn surface_separates(
    chart: &Chart,
    a: [i32; 2],
    b: [i32; 2],
    clear: impl Fn(Vec3, Vec3) -> bool,
) -> bool {
    let lift = chart.normal * RAY_EPSILON;
    let from = chart_texel_world_position(chart, a[0], a[1]) + lift;
    let to = chart_texel_world_position(chart, b[0], b[1]) + lift;
    // Shadow-ray clears skip their first `RAY_EPSILON`; stretching both ends
    // by two tests from one before `from` to two past `to`.
    let reach = (to - from).normalize_or_zero() * (2.0 * RAY_EPSILON);
    !clear(from - reach, to + reach)
}

/// One light's stored visibility over one parent face's texel grid.
/// Sub-charts of a cut face write into their parent's grid, so pairs across a
/// cut are checked. A texel with no record for the light, or a padding texel,
/// stays absent and never pairs.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct VisibilityGrid {
    width: u32,
    height: u32,
    texels: Vec<Option<f32>>,
}

impl VisibilityGrid {
    /// A `width × height` grid with every texel absent.
    pub(crate) fn new(width: u32, height: u32) -> Self {
        Self {
            width,
            height,
            texels: vec![None; width as usize * height as usize],
        }
    }

    /// Record the stored raw visibility of parent-grid texel `(x, y)`.
    pub(crate) fn set(&mut self, x: u32, y: u32, visibility: f32) {
        let index = self.index(x, y);
        self.texels[index] = Some(visibility);
    }

    pub(crate) fn get(&self, x: u32, y: u32) -> Option<f32> {
        self.texels[self.index(x, y)]
    }

    pub(crate) fn width(&self) -> u32 {
        self.width
    }

    pub(crate) fn height(&self) -> u32 {
        self.height
    }

    fn index(&self, x: u32, y: u32) -> usize {
        assert!(
            x < self.width && y < self.height,
            "texel ({x}, {y}) outside a {}x{} grid",
            self.width,
            self.height
        );
        y as usize * self.width as usize + x as usize
    }
}

/// What the detector found on one (light, parent face) grid.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub(crate) struct TexelSteps {
    /// Axis-adjacent present pairs whose difference exceeds the threshold.
    pub(crate) pairs: usize,
    /// Largest axis-adjacent difference among present pairs.
    pub(crate) worst_difference: f32,
    /// The earliest texel, in row-major order, that begins a stepped pair:
    /// its left or upper texel. `None` when nothing steps.
    pub(crate) first: Option<[u32; 2]>,
}

/// Count the axis-adjacent texel pairs on `grid` whose stored visibilities
/// differ by more than `threshold` (production callers pass
/// [`TEXEL_STEP_THRESHOLD`]). Reads stored values only, so it traces nothing.
/// A pair needs both texels present; a NaN never counts.
pub(crate) fn texel_steps(grid: &VisibilityGrid, threshold: f32) -> TexelSteps {
    scan_texel_steps(
        grid.width,
        grid.height,
        |y, row| {
            for (x, texel) in row.iter_mut().enumerate() {
                *texel = grid.get(x as u32, y);
            }
        },
        |_, _| true,
        |_, _| false,
        threshold,
    )
}

/// [`texel_steps`] over a `width × height` grid streamed a row at a time,
/// for callers whose visibility lives in sparse records rather than a grid.
///
/// `fill_row(y, row)` writes row `y` (every slot, absent ones `None`); rows
/// arrive in ascending order and only two are live. `owns(x, y)` says
/// whether the pairs that begin at texel `(x, y)` belong to this scan: a
/// sub-chart overlapping its neighbours checks only the pairs its own segment
/// begins, so a cut face counts each pair once. `separated(a, b)` is asked
/// only for an owned pair above the threshold; a separated pair is no step
/// and adds nothing, not even to the worst difference.
pub(crate) fn scan_texel_steps(
    width: u32,
    height: u32,
    mut fill_row: impl FnMut(u32, &mut [Option<f32>]),
    owns: impl Fn(u32, u32) -> bool,
    mut separated: impl FnMut([u32; 2], [u32; 2]) -> bool,
    threshold: f32,
) -> TexelSteps {
    let mut steps = TexelSteps::default();
    let mut pair = |a: [u32; 2], va: Option<f32>, b: [u32; 2], vb: Option<f32>| {
        let (Some(va), Some(vb)) = (va, vb) else {
            return;
        };
        if !owns(a[0], a[1]) {
            return;
        }
        let difference = (va - vb).abs();
        if difference > threshold {
            if separated(a, b) {
                return;
            }
            steps.pairs += 1;
            if steps.first.is_none_or(|[x, y]| (a[1], a[0]) < (y, x)) {
                steps.first = Some(a);
            }
        }
        if difference > steps.worst_difference {
            steps.worst_difference = difference;
        }
    };
    let mut above = vec![None; width as usize];
    let mut row = vec![None; width as usize];
    for y in 0..height {
        row.fill(None);
        fill_row(y, &mut row);
        for x in 0..width {
            let here = row[x as usize];
            if y > 0 {
                pair([x, y - 1], above[x as usize], [x, y], here);
            }
            if x + 1 < width {
                pair([x, y], here, [x + 1, y], row[x as usize + 1]);
            }
        }
        std::mem::swap(&mut above, &mut row);
    }
    steps
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A grid whose texels left of column `edge` hold `left` and the rest `right`.
    fn split_grid(width: u32, height: u32, edge: u32, left: f32, right: f32) -> VisibilityGrid {
        let mut grid = VisibilityGrid::new(width, height);
        for y in 0..height {
            for x in 0..width {
                grid.set(x, y, if x < edge { left } else { right });
            }
        }
        grid
    }

    #[test]
    fn hard_zero_one_step_counts_every_crossing_pair() {
        let grid = split_grid(6, 4, 3, 0.0, 1.0);
        let steps = texel_steps(&grid, TEXEL_STEP_THRESHOLD);
        assert_eq!(steps.pairs, 4, "one stepped pair per row");
        assert_eq!(steps.worst_difference, 1.0);
    }

    #[test]
    fn floored_edge_with_the_kernels_worst_pair_stays_silent() {
        // A mid-phase axis-aligned edge filtered by the cone kernel: texels at
        // 0.11 and 0.89 either side, exact 0 and 1 beyond, and the floored
        // bake's worst sampled pair (0.875) on another row.
        let mut grid = VisibilityGrid::new(6, 2);
        for (x, v) in [0.0, 0.0, 0.11, 0.89, 1.0, 1.0].into_iter().enumerate() {
            grid.set(x as u32, 0, v);
        }
        for (x, v) in [0.0, 0.0, 0.0625, 0.9375, 1.0, 1.0].into_iter().enumerate() {
            grid.set(x as u32, 1, v);
        }
        let steps = texel_steps(&grid, TEXEL_STEP_THRESHOLD);
        assert_eq!(steps.pairs, 0);
        assert!((steps.worst_difference - 0.875).abs() < 1.0e-6);
    }

    #[test]
    fn absent_neighbours_never_pair() {
        // A lit texel beside texels with no record for this light (padding,
        // or outside its reach), and a shadowed island ringed by absences.
        let mut grid = VisibilityGrid::new(5, 5);
        grid.set(0, 0, 1.0);
        grid.set(2, 2, 0.0);
        grid.set(4, 4, 1.0);
        let steps = texel_steps(&grid, TEXEL_STEP_THRESHOLD);
        assert_eq!(steps, TexelSteps::default());
    }

    #[test]
    fn first_step_is_the_earliest_row_major_pair_start() {
        // One dark texel at (3, 2) of a lit grid: the vertical pair above it
        // begins at (3, 1), the horizontal one at (2, 2). Row 1 comes first,
        // and a second dark texel at (1, 2) moves the start to (1, 1).
        let mut grid = VisibilityGrid::new(4, 3);
        for y in 0..3 {
            for x in 0..4 {
                grid.set(x, y, 1.0);
            }
        }
        grid.set(3, 2, 0.0);
        let steps = texel_steps(&grid, TEXEL_STEP_THRESHOLD);
        assert_eq!((steps.pairs, steps.first), (2, Some([3, 1])));
        grid.set(1, 2, 0.0);
        let steps = texel_steps(&grid, TEXEL_STEP_THRESHOLD);
        assert_eq!((steps.pairs, steps.first), (5, Some([1, 1])));
    }

    #[test]
    fn unowned_and_separated_pairs_never_step() {
        // A 0/1 split down the middle: one stepped pair per row. Rows the
        // scan does not own, and pairs a surface separates, add nothing.
        let grid = split_grid(4, 3, 2, 0.0, 1.0);
        let fill = |y: u32, row: &mut [Option<f32>]| {
            for (x, texel) in row.iter_mut().enumerate() {
                *texel = grid.get(x as u32, y);
            }
        };
        let owned_rows = scan_texel_steps(
            4,
            3,
            fill,
            |_, y| y != 1,
            |_, _| false,
            TEXEL_STEP_THRESHOLD,
        );
        assert_eq!(owned_rows.pairs, 2);
        let mut asked = Vec::new();
        let separated_row = scan_texel_steps(
            4,
            3,
            fill,
            |_, _| true,
            |a, b| {
                asked.push((a, b));
                a[1] == 0
            },
            TEXEL_STEP_THRESHOLD,
        );
        assert_eq!(separated_row.pairs, 2);
        assert_eq!(separated_row.first, Some([1, 1]));
        assert_eq!(
            asked,
            vec![([1, 0], [2, 0]), ([1, 1], [2, 1]), ([1, 2], [2, 2])],
            "only pairs above the threshold are tested for a separating surface"
        );
        let all_separated =
            scan_texel_steps(4, 3, fill, |_, _| true, |_, _| true, TEXEL_STEP_THRESHOLD);
        assert_eq!(all_separated, TexelSteps::default());
    }

    #[test]
    fn steps_counts_vertical_pairs_and_ignores_nan() {
        let mut grid = VisibilityGrid::new(2, 3);
        grid.set(0, 0, 1.0);
        grid.set(0, 1, 0.0);
        grid.set(0, 2, f32::NAN);
        grid.set(1, 0, f32::NAN);
        let steps = texel_steps(&grid, TEXEL_STEP_THRESHOLD);
        assert_eq!(steps.pairs, 1);
        assert_eq!(steps.worst_difference, 1.0);
    }
}
