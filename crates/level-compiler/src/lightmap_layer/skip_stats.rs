// Test-only tallies of the light-layer bake's exact skips: dropped pairs, each
// walked chart's shaft-cull tiles and texels, and the reach cull's permit probe.
// See: context/lib/build_pipeline.md §Compiler pipeline (step 12)

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, OnceLock};

use crate::ray_traversal::Occluder;

/// What one bake's exact skips did, gathered across worker threads. Tests
/// leak one ([`SkipStats::leak`]) and hand it to the walk through
/// [`super::ExactSkips::tallied`].
#[derive(Debug, Default)]
pub(crate) struct SkipStats {
    /// `(light, chart)` pairs in range that the chart coverage cull dropped.
    dropped_pairs: AtomicUsize,
    /// Covered texels of clear tiles that verify mode traced.
    verified_texels: AtomicUsize,
    charts: Mutex<Vec<ChartTally>>,
    /// The stage's lightmap progress, read at each reach note.
    progress: OnceLock<Arc<AtomicUsize>>,
    reach_notes: Mutex<Vec<ReachNote>>,
}

/// Where in a reach-culled pair's work a note was taken.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ReachPhase {
    /// Building a chart's reach mask, inside the first walk that asks.
    MaskBuild,
    /// Starting the reach-culled walk of a pair.
    CulledWalk,
    /// Starting verify mode's re-walk of that pair.
    Rewalk,
}

/// One reach note: the phase, the face, the governor permits the noting
/// thread held, and the stage progress then, when watched.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct ReachNote {
    pub(crate) phase: ReachPhase,
    pub(crate) face: usize,
    pub(crate) permits: usize,
    pub(crate) progress: Option<usize>,
}

/// One walked `(light, chart)`'s tiles, row-major.
#[derive(Debug, Clone)]
pub(crate) struct ChartTally {
    /// The light's `f32` origin, naming it among a stage's lights.
    pub(crate) light_origin: [f32; 3],
    pub(crate) face: usize,
    /// Interior texel extent of the chart.
    pub(crate) dims: (i32, i32),
    /// Tiles per row.
    pub(crate) tiles_wide: i32,
    pub(crate) tiles: Vec<TileTally>,
    /// Whether this walk was the reach-culled one: verify mode's re-walk of
    /// a culled pair pushes its own tally after it, with this `false`.
    pub(crate) culled: bool,
    /// Every interior texel the walk visited, in walk order.
    pub(crate) texels: Vec<TexelTally>,
}

/// One texel of a walked chart.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct TexelTally {
    pub(crate) texel: (i32, i32),
    /// Both remembered occluders (shadow, clip) as the walk reached the texel.
    pub(crate) occluders: [Option<Occluder>; 2],
    /// The reach cull stored 0 here without tracing.
    pub(crate) skipped: bool,
    /// The record's raw visibility; `None` when the texel recorded nothing.
    pub(crate) visibility: Option<f32>,
}

/// One tile of a walked chart.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct TileTally {
    /// Texels with a direct term, which ask for visibility.
    pub(crate) covered: u32,
    /// Shaft tests run.
    pub(crate) tests: u32,
    /// The verdict, once tested.
    pub(crate) clear: Option<bool>,
    /// Interior texel coordinates of the tile's first covered texel.
    pub(crate) first_covered: Option<(i32, i32)>,
    /// Segments the walk traced from the tile's texels, by kind.
    pub(crate) shadow: u32,
    pub(crate) clip: u32,
    pub(crate) lead_in: u32,
}

impl TileTally {
    pub(crate) fn segments(&self) -> u32 {
        self.shadow + self.clip + self.lead_in
    }
}

impl ChartTally {
    /// Interior texel range `[tx0, tx1] × [ty0, ty1]` of tile `index`.
    pub(crate) fn tile_texels(&self, index: usize) -> [i32; 4] {
        let edge = super::shaft_cull::SHAFT_TILE_TEXELS;
        let (x, y) = (
            index as i32 % self.tiles_wide,
            index as i32 / self.tiles_wide,
        );
        [
            x * edge,
            (x * edge + edge - 1).min(self.dims.0 - 1),
            y * edge,
            (y * edge + edge - 1).min(self.dims.1 - 1),
        ]
    }

    pub(crate) fn clear_tiles(&self) -> impl Iterator<Item = usize> + '_ {
        (0..self.tiles.len()).filter(|&tile| self.tiles[tile].clear == Some(true))
    }
}

impl SkipStats {
    /// A fresh tally that outlives the bake, so `ExactSkips` stays `Copy`.
    pub(crate) fn leak() -> &'static Self {
        Box::leak(Box::default())
    }

    pub(crate) fn add_dropped_pairs(&self, count: usize) {
        self.dropped_pairs.fetch_add(count, Ordering::Relaxed);
    }

    pub(crate) fn add_verified_texel(&self) {
        self.verified_texels.fetch_add(1, Ordering::Relaxed);
    }

    pub(crate) fn push_chart(&self, chart: ChartTally) {
        self.charts.lock().unwrap().push(chart);
    }

    /// Read the stage progress behind `completed` at every reach note.
    pub(crate) fn watch_progress(&self, completed: Arc<AtomicUsize>) {
        assert!(
            self.progress.set(completed).is_ok(),
            "one progress per tally"
        );
    }

    pub(crate) fn note_reach(&self, phase: ReachPhase, face: usize) {
        let note = ReachNote {
            phase,
            face,
            permits: crate::governor::permits_held_on_this_thread(),
            progress: self
                .progress
                .get()
                .map(|completed| completed.load(Ordering::SeqCst)),
        };
        self.reach_notes.lock().unwrap().push(note);
    }

    /// Every reach note, in the order they were taken.
    pub(crate) fn reach_notes(&self) -> Vec<ReachNote> {
        self.reach_notes.lock().unwrap().clone()
    }

    pub(crate) fn dropped_pairs(&self) -> usize {
        self.dropped_pairs.load(Ordering::Relaxed)
    }

    pub(crate) fn verified_texels(&self) -> usize {
        self.verified_texels.load(Ordering::Relaxed)
    }

    /// Every walked chart, in the order the walks finished.
    pub(crate) fn charts(&self) -> Vec<ChartTally> {
        self.charts.lock().unwrap().clone()
    }

    fn tiles(&self) -> Vec<TileTally> {
        self.charts()
            .into_iter()
            .flat_map(|chart| chart.tiles)
            .collect()
    }

    pub(crate) fn tiles_tested(&self) -> u32 {
        self.tiles().iter().map(|tile| tile.tests).sum()
    }

    pub(crate) fn tiles_cleared(&self) -> usize {
        self.tiles()
            .iter()
            .filter(|tile| tile.clear == Some(true))
            .count()
    }

    /// Segments traced from the texels of clear tiles: none, for an exact cull.
    pub(crate) fn segments_in_clear_tiles(&self) -> u32 {
        self.tiles()
            .iter()
            .filter(|tile| tile.clear == Some(true))
            .map(TileTally::segments)
            .sum()
    }
}
