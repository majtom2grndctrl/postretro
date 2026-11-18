//! Texel-step detection over the fused walk's partitions, for the `lightmap_section` memo's report.
//! See: context/lib/build_pipeline.md §Compiler pipeline (step 12)

use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

use bvh::bvh::Bvh;

use crate::bvh_build::BvhPrimitive;
use crate::chart_raster::{ChartPlacement, chart_interior_dims};
use crate::geometry::GeometryResult;
use crate::lightmap_bake::CutParent;
use crate::lightmap_layer::{LayerTexel, LightmapLayer, SharedAtlas};
use crate::ray_traversal::segment_clear;
use crate::texel_step_detector::{
    ChartSteps, CutSites, TexelRect, TexelStepReport, merge_chart_steps, scan_chart,
};

/// The steps one partition held, from the item that produced it.
struct ItemSteps {
    item: usize,
    light: usize,
    charts: Vec<ChartSteps>,
}

/// Runs the detector over each partition the compose pass bakes or loads, on
/// the worker that produced it, and gathers the results for the caller's
/// thread: log capture never reaches a Rayon worker, and worker order must
/// not reach the report.
pub(super) struct StepDetector<'a> {
    threshold: f32,
    shared: &'a SharedAtlas<'a>,
    bvh: &'a Bvh<f32, 3>,
    primitives: &'a [BvhPrimitive],
    geometry: &'a GeometryResult,
    sites: CutSites<'a>,
    found: Mutex<Vec<ItemSteps>>,
    /// Detector time summed over workers.
    nanos: AtomicU64,
}

impl<'a> StepDetector<'a> {
    pub(super) fn new(
        threshold: f32,
        shared: &'a SharedAtlas<'a>,
        bvh: &'a Bvh<f32, 3>,
        primitives: &'a [BvhPrimitive],
        geometry: &'a GeometryResult,
        cut_parents: &'a [CutParent],
    ) -> Self {
        Self {
            threshold,
            shared,
            bvh,
            primitives,
            geometry,
            sites: CutSites::new(cut_parents, shared.charts),
            found: Mutex::new(Vec::new()),
            nanos: AtomicU64::new(0),
        }
    }

    /// Check `partition`, item `item` of light position `light`, over
    /// `charts` of its layer.
    pub(super) fn detect(
        &self,
        item: usize,
        light: usize,
        partition: &LightmapLayer,
        charts: impl Iterator<Item = usize>,
    ) {
        let started = Instant::now();
        let found: Vec<ChartSteps> = charts
            .filter_map(|chart| self.scan_chart(partition, chart))
            .collect();
        if !found.is_empty() {
            self.found
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .push(ItemSteps {
                    item,
                    light,
                    charts: found,
                });
        }
        self.nanos
            .fetch_add(started.elapsed().as_nanos() as u64, Ordering::Relaxed);
    }

    /// One chart's owned steps in `partition`, streamed row by row from its
    /// sorted records, or `None` when nothing steps.
    fn scan_chart(&self, partition: &LightmapLayer, face: usize) -> Option<ChartSteps> {
        let chart = &self.shared.charts[face];
        if chart.uv_extent[0] <= 0.0 || chart.uv_extent[1] <= 0.0 {
            return None;
        }
        let (width, height) = chart_interior_dims(chart);
        let mut rows = ChartRows::new(
            &partition.texels,
            self.shared.placements[face],
            self.shared.atlas_width,
            width as u32,
            height as u32,
        )?;
        scan_chart(
            chart,
            face,
            &self.sites,
            TexelRect {
                x: 0,
                y: 0,
                width: width as u32,
                height: height as u32,
            },
            |ty, row| rows.fill(ty, row),
            |from, to| segment_clear(self.bvh, self.primitives, self.geometry, from, to),
            self.threshold,
        )
    }

    /// The report, one warning per stepped (light, face) with a cut face's
    /// sub-faces merged, and the detector's time summed over workers: an
    /// upper bound on its share of the stage's wall time.
    pub(super) fn finish(mut self) -> (TexelStepReport, Duration) {
        let started = Instant::now();
        let mut found = std::mem::take(
            self.found
                .get_mut()
                .unwrap_or_else(|poisoned| poisoned.into_inner()),
        );
        found.sort_unstable_by_key(|steps| steps.item);
        let report = merge_chart_steps(
            found.into_iter().flat_map(|item| {
                item.charts
                    .into_iter()
                    .map(move |chart| (item.light, chart))
            }),
            &self.sites,
        );
        let elapsed = Duration::from_nanos(self.nanos.load(Ordering::Relaxed)) + started.elapsed();
        (report, elapsed)
    }
}

/// Streams a chart's interior rows, in chart order, out of a partition's
/// records sorted by atlas index. Every cursor only moves forward, so a
/// chart's rows read each of its records once in either orientation.
struct ChartRows<'t> {
    texels: &'t [LayerTexel],
    placement: ChartPlacement,
    atlas_width: u32,
    width: u32,
    /// Upright: the next unread record; the chart's rows follow atlas order.
    cursor: usize,
    /// Transposed: a chart row runs down an atlas column, so chart column
    /// `tx` lies on one atlas row and keeps its own cursor there.
    row_cursors: Vec<usize>,
    /// The lowest row `fill` may take next: the cursors only move forward.
    next_row: u32,
}

impl<'t> ChartRows<'t> {
    /// `None` when no record falls inside the chart's interior rect: a chart
    /// the light left without a record costs one search.
    fn new(
        texels: &'t [LayerTexel],
        placement: ChartPlacement,
        atlas_width: u32,
        width: u32,
        height: u32,
    ) -> Option<Self> {
        let mut rows = Self {
            texels,
            placement,
            atlas_width,
            width,
            cursor: 0,
            row_cursors: Vec::new(),
            next_row: 0,
        };
        // The placed rect's top-left and bottom-right corners bound the
        // interior's atlas indices in either orientation.
        let first = rows.index_of(0, 0);
        let last = rows.index_of(width - 1, height - 1) + 1;
        rows.cursor = texels.partition_point(|texel| texel.idx < first);
        if texels
            .get(rows.cursor)
            .is_none_or(|texel| texel.idx >= last)
        {
            return None;
        }
        if placement.transposed {
            // Atlas rows rise with `tx`, so each row's start is searched
            // after the previous one's.
            let mut at = rows.cursor;
            rows.row_cursors = (0..width)
                .map(|tx| {
                    let start = rows.index_of(tx, 0);
                    at += texels[at..].partition_point(|texel| texel.idx < start);
                    at
                })
                .collect();
        }
        Some(rows)
    }

    fn index_of(&self, tx: u32, ty: u32) -> u32 {
        let (x, y) = self.placement.interior_texel(tx as i32, ty as i32);
        y as u32 * self.atlas_width + x as u32
    }

    /// Writes chart row `ty` into `row`, leaving absent texels untouched.
    /// Rows must come in strictly increasing `ty`, from 0.
    fn fill(&mut self, ty: u32, row: &mut [Option<f32>]) {
        debug_assert!(
            ty >= self.next_row,
            "chart row {ty} after row {}: rows must strictly increase",
            self.next_row.wrapping_sub(1)
        );
        self.next_row = ty + 1;
        let texels = self.texels;
        if self.placement.transposed {
            for tx in 0..self.width {
                let idx = self.index_of(tx, ty);
                let cursor = &mut self.row_cursors[tx as usize];
                while texels.get(*cursor).is_some_and(|texel| texel.idx < idx) {
                    *cursor += 1;
                }
                if let Some(texel) = texels.get(*cursor)
                    && texel.idx == idx
                {
                    row[tx as usize] = Some(texel.raw_visibility);
                    *cursor += 1;
                }
            }
            return;
        }
        let start = self.index_of(0, ty);
        self.cursor += texels[self.cursor..].partition_point(|texel| texel.idx < start);
        while let Some(texel) = texels.get(self.cursor)
            && texel.idx < start + self.width
        {
            row[(texel.idx - start) as usize] = Some(texel.raw_visibility);
            self.cursor += 1;
        }
    }
}

#[cfg(test)]
#[path = "texel_steps_tests.rs"]
mod tests;
