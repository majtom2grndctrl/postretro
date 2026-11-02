// The chart walk's side of the reach cull: which pairs it culls, its stage
// tallies, and verify mode's record comparison.
// See: context/lib/build_pipeline.md §Compiler pipeline step 12 (Exact skips)

use std::fmt;
use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};

use glam::{DVec3, Vec3};

use super::chart_cull::ExactSkips;
use super::{LayerTexel, SharedAtlas};
use crate::chart_raster::{CHART_PADDING_TEXELS, chart_texel_world_position};
use crate::lightmap_bake::Chart;
use crate::lightmap_reach::{ChartReach, LightmapReachIndex};

/// One light's view of the reach index for its chart walks.
#[derive(Clone, Copy)]
pub(crate) struct ReachWalk<'a> {
    pub(crate) index: &'a LightmapReachIndex<'a>,
    /// The light's position in the stage's layer-light list.
    pub(crate) light: usize,
    /// Re-walk every culled pair as a kept pair and require equal records.
    pub(crate) verify: bool,
    pub(crate) tally: &'a ReachTally,
    pub(crate) mismatch: &'a ReachMismatchSlot,
}

impl<'a> ReachWalk<'a> {
    /// The chart's reach when this walk culls the pair, else `None`.
    pub(super) fn culled_chart(
        &self,
        face_idx: usize,
        chart: &Chart,
        exact_skips: ExactSkips,
    ) -> Option<&'a ChartReach> {
        #[cfg(test)]
        if exact_skips.force_reach_cull {
            return self.index.forced_chart(face_idx, chart);
        }
        if !exact_skips.reach_cull {
            return None;
        }
        self.index.culled_chart(self.light, face_idx, chart)
    }
}

/// Stage totals of the reach cull, summed over every walk.
#[derive(Debug, Default)]
pub(crate) struct ReachTally {
    pub(crate) pairs_walked: AtomicU64,
    pub(crate) pairs_culled: AtomicU64,
    pub(crate) texels_skipped: AtomicU64,
    pub(crate) texels_traced_in_culled: AtomicU64,
    pub(crate) pairs_rewalked: AtomicU64,
    pub(crate) partitions_baked: AtomicU64,
    pub(crate) partitions_loaded: AtomicU64,
}

impl ReachTally {
    pub(crate) fn add(counter: &AtomicU64, n: u64) {
        counter.fetch_add(n, Ordering::Relaxed);
    }

    pub(crate) fn read(counter: &AtomicU64) -> u64 {
        counter.load(Ordering::Relaxed)
    }
}

/// A culled pair whose record differs from the kept walk's.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct ReachMismatch {
    pub(crate) light: usize,
    pub(crate) light_origin: DVec3,
    pub(crate) chart: usize,
    pub(crate) texel: (i32, i32),
    pub(crate) world: Vec3,
    /// The culled walk's raw visibility, `None` when it held no record.
    pub(crate) culled: Option<f32>,
    pub(crate) kept: Option<f32>,
}

impl fmt::Display for ReachMismatch {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let value =
            |v: Option<f32>| v.map_or_else(|| "no record".to_string(), |v| format!("{v:?}"));
        write!(
            f,
            "lightmap reach verify: light {} at ({:.3}, {:.3}, {:.3}), chart {}, texel ({}, {}) \
             at ({:.3}, {:.3}, {:.3}): the culled walk stored {}, the kept walk {}",
            self.light,
            self.light_origin.x,
            self.light_origin.y,
            self.light_origin.z,
            self.chart,
            self.texel.0,
            self.texel.1,
            self.world.x,
            self.world.y,
            self.world.z,
            value(self.culled),
            value(self.kept),
        )
    }
}

/// Where a mismatch sorts: (light position, chart, atlas texel index).
type MismatchKey = (usize, usize, u32);

/// The first mismatch by (light, chart, texel index), whatever order the
/// workers find them in, so the error names the same texel at any worker
/// count.
#[derive(Debug, Default)]
pub(crate) struct ReachMismatchSlot {
    first: Mutex<Option<(MismatchKey, ReachMismatch)>>,
}

impl ReachMismatchSlot {
    fn record(&self, key: MismatchKey, mismatch: ReachMismatch) {
        let mut first = self
            .first
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if first.as_ref().is_none_or(|(held, _)| key < *held) {
            *first = Some((key, mismatch));
        }
    }

    pub(crate) fn take(&self) -> Option<ReachMismatch> {
        self.first
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .take()
            .map(|(_, mismatch)| mismatch)
    }
}

/// Verify mode: compare a culled walk's records with the kept walk's, bit
/// for bit, and record the first difference. Whether they differ.
pub(super) fn check_rewalk(
    walk: &ReachWalk<'_>,
    light_origin: DVec3,
    atlas: &SharedAtlas<'_>,
    face_idx: usize,
    culled: &[LayerTexel],
    kept: &[LayerTexel],
) -> bool {
    ReachTally::add(&walk.tally.pairs_rewalked, 1);
    let same = |a: &LayerTexel, b: &LayerTexel| {
        a.idx == b.idx && a.raw_visibility.to_bits() == b.raw_visibility.to_bits()
    };
    let differs = (0..culled.len().max(kept.len()))
        .find(|&i| !matches!((culled.get(i), kept.get(i)), (Some(a), Some(b)) if same(a, b)));
    let Some(at) = differs else {
        return false;
    };
    // Both walks visit texels in one order and record the same covered set,
    // so a difference at `at` names one texel; take the lower index.
    let idx = match (culled.get(at), kept.get(at)) {
        (Some(a), Some(b)) => a.idx.min(b.idx),
        (Some(a), None) | (None, Some(a)) => a.idx,
        (None, None) => return false,
    };
    let find = |records: &[LayerTexel]| {
        records
            .iter()
            .find(|texel| texel.idx == idx)
            .map(|texel| texel.raw_visibility)
    };
    let placement = &atlas.placements[face_idx];
    let chart = &atlas.charts[face_idx];
    let padding = CHART_PADDING_TEXELS as i32;
    let tx = (idx % atlas.atlas_width) as i32 - placement.x as i32 - padding;
    let ty = (idx / atlas.atlas_width) as i32 - placement.y as i32 - padding;
    walk.mismatch.record(
        (walk.light, face_idx, idx),
        ReachMismatch {
            light: walk.light,
            light_origin,
            chart: face_idx,
            texel: (tx, ty),
            world: chart_texel_world_position(chart, tx, ty),
            culled: find(culled),
            kept: find(kept),
        },
    );
    true
}
