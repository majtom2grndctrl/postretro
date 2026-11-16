//! Texel-step detection over the fused walk's partitions, for the `lightmap_section` memo's report.
//! See: context/lib/build_pipeline.md §Compiler pipeline (step 12)

use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

use bvh::bvh::Bvh;

use crate::bvh_build::BvhPrimitive;
use crate::chart_raster::chart_interior_dims;
use crate::geometry::GeometryResult;
use crate::lightmap_bake::CutParent;
use crate::lightmap_layer::{LightmapLayer, SharedAtlas};
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
        let placement = self.shared.placements[face];
        let (width, height) = chart_interior_dims(chart);
        let index_of = |tx: u32, ty: u32| {
            let (x, y) = placement.interior_texel(tx as i32, ty as i32);
            y as u32 * self.shared.atlas_width + x as u32
        };
        let (width, height) = (width as u32, height as u32);
        let texels = &partition.texels;
        // The interior's first and one-past-last atlas index, in either
        // orientation: its placed rect's top-left and bottom-right corners.
        let first = index_of(0, 0);
        let last = index_of(width - 1, height - 1) + 1;
        let mut cursor = texels.partition_point(|texel| texel.idx < first);
        // A chart the light left without a record costs one search.
        if texels.get(cursor).is_none_or(|texel| texel.idx >= last) {
            return None;
        }
        scan_chart(
            chart,
            face,
            &self.sites,
            TexelRect {
                x: 0,
                y: 0,
                width,
                height,
            },
            |ty, row| {
                if placement.transposed {
                    // A chart row runs down an atlas column: one search per
                    // texel.
                    for (tx, value) in (0..width).zip(row.iter_mut()) {
                        let idx = index_of(tx, ty);
                        if let Ok(at) = texels.binary_search_by_key(&idx, |texel| texel.idx) {
                            *value = Some(texels[at].raw_visibility);
                        }
                    }
                    return;
                }
                let start = index_of(0, ty);
                cursor += texels[cursor..].partition_point(|texel| texel.idx < start);
                while let Some(texel) = texels.get(cursor)
                    && texel.idx < start + width
                {
                    row[(texel.idx - start) as usize] = Some(texel.raw_visibility);
                    cursor += 1;
                }
            },
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
