// Texel-step detector: axis-adjacent jumps in stored per-light visibility on a parent texel grid.
// See: context/lib/build_pipeline.md §Compiler pipeline (step 12)

/// Largest stored per-light visibility difference two axis-adjacent texels
/// may show before they count as a step. 0.78 is the one-texel cone kernel's
/// largest axis-adjacent difference: a mid-phase axis-aligned edge filters its
/// two texels to 0.11 and 0.89, and no occluder pattern does worse. 0.17 is
/// the sampling margin: a floored bake's worst pair reaches 0.917 (eleven of
/// twelve samples apart at 24 samples), while a hard 0/1 step is 1.0 and
/// always exceeds it; any margin under 0.22 keeps that guarantee.
pub(crate) const TEXEL_STEP_THRESHOLD: f32 = 0.78 + 0.17;

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
}

/// Count the axis-adjacent texel pairs on `grid` whose stored visibilities
/// differ by more than `threshold` (production callers pass
/// [`TEXEL_STEP_THRESHOLD`]). Reads stored values only, so it traces nothing.
/// A pair needs both texels present; a NaN never counts.
pub(crate) fn texel_steps(grid: &VisibilityGrid, threshold: f32) -> TexelSteps {
    let mut steps = TexelSteps::default();
    let mut pair = |a: Option<f32>, b: Option<f32>| {
        let (Some(a), Some(b)) = (a, b) else {
            return;
        };
        let difference = (a - b).abs();
        if difference > steps.worst_difference {
            steps.worst_difference = difference;
        }
        if difference > threshold {
            steps.pairs += 1;
        }
    };
    for y in 0..grid.height {
        for x in 0..grid.width {
            let here = grid.get(x, y);
            if x + 1 < grid.width {
                pair(here, grid.get(x + 1, y));
            }
            if y + 1 < grid.height {
                pair(here, grid.get(x, y + 1));
            }
        }
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
