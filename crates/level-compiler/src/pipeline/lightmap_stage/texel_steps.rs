//! Texel-step detection over the fused walk's partitions, and the report the
//! `lightmap_section` memo stores so a warm build repeats it.
//! See: context/lib/build_pipeline.md §Compiler pipeline (step 12)

use std::collections::BTreeMap;
use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

use bvh::bvh::Bvh;
use glam::Vec3;

use crate::bvh_build::BvhPrimitive;
use crate::chart_raster::{CHART_PADDING_TEXELS, chart_interior_dims, chart_texel_world_position};
use crate::geometry::GeometryResult;
use crate::light_namespaces::StaticBakedEntry;
use crate::lightmap_bake::{CutParent, segment_clear};
use crate::lightmap_layer::{LightmapLayer, SharedAtlas};
use crate::texel_step_detector::{scan_texel_steps, surface_separates};

/// Where a stepped edge lies: one face, or every sub-face of a cut face as
/// the one face the author drew.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum StepSite {
    Face(u32),
    CutFace {
        parent: u32,
        first_face: u32,
        last_face: u32,
    },
}

impl StepSite {
    /// The site's first face after the cut, which orders the report.
    fn first_face(self) -> u32 {
        match self {
            Self::Face(face) => face,
            Self::CutFace { first_face, .. } => first_face,
        }
    }
}

/// One light's stepped edges on one face.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct TexelStepWarning {
    /// The light's position among the stage's layer lights: the static
    /// lights the lightmap bakes, SDF-shadowed ones left out.
    pub(crate) light: u32,
    pub(crate) site: StepSite,
    /// Stepped axis-adjacent texel pairs, each counted once across a cut.
    pub(crate) pairs: u32,
    pub(crate) worst_difference: f32,
    /// World position of the face's earliest stepped texel, row-major on
    /// its parent grid.
    pub(crate) position: [f32; 3],
}

/// Every stepped (light, face) of one bake, in light order, then face order.
#[derive(Debug, Clone, Default, PartialEq)]
pub(crate) struct TexelStepReport {
    pub(crate) warnings: Vec<TexelStepWarning>,
}

/// One warning in the memo entry: ten little-endian words — light, site kind
/// (0 face, 1 cut face), site, first face, last face, pairs, worst difference
/// bits, then the position's bits.
const RECORD_WORDS: usize = 10;
const RECORD_BYTES: usize = RECORD_WORDS * 4;

impl TexelStepReport {
    /// The memo entry's report prefix: the warning count, then the records.
    pub(super) fn to_bytes(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(4 + self.warnings.len() * RECORD_BYTES);
        out.extend_from_slice(&(self.warnings.len() as u32).to_le_bytes());
        for warning in &self.warnings {
            let (kind, site, first, last) = match warning.site {
                StepSite::Face(face) => (0, face, face, face),
                StepSite::CutFace {
                    parent,
                    first_face,
                    last_face,
                } => (1, parent, first_face, last_face),
            };
            let [x, y, z] = warning.position.map(f32::to_bits);
            for word in [
                warning.light,
                kind,
                site,
                first,
                last,
                warning.pairs,
                warning.worst_difference.to_bits(),
                x,
                y,
                z,
            ] {
                out.extend_from_slice(&word.to_le_bytes());
            }
        }
        out
    }

    /// The report and the bytes after it, or `None` when the prefix is
    /// malformed or names a light or face this bake does not have.
    pub(super) fn parse(bytes: &[u8], lights: usize, faces: usize) -> Option<(Self, &[u8])> {
        let word = |bytes: &[u8], at: usize| -> Option<u32> {
            Some(u32::from_le_bytes(bytes.get(at..at + 4)?.try_into().ok()?))
        };
        let count = word(bytes, 0)? as usize;
        let end = count.checked_mul(RECORD_BYTES)?.checked_add(4)?;
        let records = bytes.get(4..end)?;
        let face = |face: u32| ((face as usize) < faces).then_some(face);
        let mut warnings = Vec::with_capacity(count);
        for record in records.as_chunks::<RECORD_BYTES>().0 {
            let w = |i: usize| word(record, i * 4);
            let light = w(0)?;
            if light as usize >= lights {
                return None;
            }
            let site = match w(1)? {
                0 => StepSite::Face(face(w(2)?)?),
                1 => StepSite::CutFace {
                    parent: w(2)?,
                    first_face: face(w(3)?)?,
                    last_face: face(w(4)?)?,
                },
                _ => return None,
            };
            warnings.push(TexelStepWarning {
                light,
                site,
                pairs: w(5)?,
                worst_difference: f32::from_bits(w(6)?),
                position: [w(7)?, w(8)?, w(9)?].map(f32::from_bits),
            });
        }
        Some((Self { warnings }, &bytes[end..]))
    }

    /// One warning per stepped (light, face), on the caller's thread. Its
    /// wording is contract for the stage's tests.
    pub(super) fn log(&self, layer_lights: &[&StaticBakedEntry<'_>]) {
        for warning in &self.warnings {
            let origin = layer_lights[warning.light as usize].light.origin;
            let face = match warning.site {
                StepSite::Face(face) => format!("face {face}"),
                StepSite::CutFace {
                    parent,
                    first_face,
                    last_face,
                } => format!("pre-cut face {parent} (now faces {first_face}..={last_face})"),
            };
            let [x, y, z] = warning.position;
            log::warn!(
                "[Lightmap] texel step: static light #{} at ({:.2}, {:.2}, {:.2}) on {face} near \
                 ({x:.2}, {y:.2}, {z:.2}): {} adjacent texel pair(s) differ by up to {:.2} in \
                 stored visibility, so the shadow edge shows the texel lattice",
                warning.light,
                origin.x,
                origin.y,
                origin.z,
                warning.pairs,
                warning.worst_difference,
            );
        }
    }
}

/// One chart's steps for one light, its first texel on the parent grid.
struct ChartSteps {
    chart: usize,
    pairs: usize,
    worst_difference: f32,
    first: [u32; 2],
    position: Vec3,
}

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
    cut_parents: &'a [CutParent],
    /// Per chart, the index of the cut parent it is a sub-face of.
    chart_cut: Vec<Option<usize>>,
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
        let mut chart_cut = vec![None; shared.charts.len()];
        for (index, cut) in cut_parents.iter().enumerate() {
            for slot in &mut chart_cut[cut.faces.clone()] {
                *slot = Some(index);
            }
        }
        Self {
            threshold,
            shared,
            bvh,
            primitives,
            geometry,
            cut_parents,
            chart_cut,
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
        let (width, height) = (width as u32, height as u32);
        let row_start = |ty: u32| {
            (placement.y + CHART_PADDING_TEXELS + ty) * self.shared.atlas_width
                + placement.x
                + CHART_PADDING_TEXELS
        };
        let texels = &partition.texels;
        let mut cursor = texels.partition_point(|texel| texel.idx < row_start(0));
        // A chart the light left without a record costs one search.
        let last = row_start(height - 1) + width;
        if texels.get(cursor).is_none_or(|texel| texel.idx >= last) {
            return None;
        }
        let origin = chart.window.map_or([0, 0], |window| window.origin);
        let own = self.chart_cut[face].map(|cut| {
            let cut = &self.cut_parents[cut];
            cut.own[face - cut.faces.start].clone()
        });
        let steps = scan_texel_steps(
            width,
            height,
            |ty, row| {
                let start = row_start(ty);
                cursor += texels[cursor..].partition_point(|texel| texel.idx < start);
                while let Some(texel) = texels.get(cursor)
                    && texel.idx < start + width
                {
                    row[(texel.idx - start) as usize] = Some(texel.raw_visibility);
                    cursor += 1;
                }
            },
            |tx, ty| {
                own.as_ref().is_none_or(|[u, v]| {
                    u.contains(&(origin[0] + tx)) && v.contains(&(origin[1] + ty))
                })
            },
            |a, b| {
                surface_separates(
                    chart,
                    [a[0] as i32, a[1] as i32],
                    [b[0] as i32, b[1] as i32],
                    |from, to| segment_clear(self.bvh, self.primitives, self.geometry, from, to),
                )
            },
            self.threshold,
        );
        let [tx, ty] = steps.first?;
        Some(ChartSteps {
            chart: face,
            pairs: steps.pairs,
            worst_difference: steps.worst_difference,
            first: [origin[0] + tx, origin[1] + ty],
            position: chart_texel_world_position(chart, tx as i32, ty as i32),
        })
    }

    fn site(&self, chart: usize) -> StepSite {
        match self.chart_cut[chart] {
            None => StepSite::Face(chart as u32),
            Some(cut) => {
                let cut = &self.cut_parents[cut];
                StepSite::CutFace {
                    parent: cut.parent,
                    first_face: cut.faces.start as u32,
                    last_face: cut.faces.end as u32 - 1,
                }
            }
        }
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
        struct Merged {
            site: StepSite,
            pairs: u64,
            worst_difference: f32,
            first: [u32; 2],
            position: Vec3,
        }
        let mut merged: BTreeMap<(usize, u32), Merged> = BTreeMap::new();
        for item in found {
            for chart in item.charts {
                let site = self.site(chart.chart);
                let entry = merged
                    .entry((item.light, site.first_face()))
                    .or_insert(Merged {
                        site,
                        pairs: 0,
                        worst_difference: 0.0,
                        first: chart.first,
                        position: chart.position,
                    });
                entry.pairs += chart.pairs as u64;
                entry.worst_difference = entry.worst_difference.max(chart.worst_difference);
                let [x, y] = chart.first;
                if (y, x) < (entry.first[1], entry.first[0]) {
                    entry.first = chart.first;
                    entry.position = chart.position;
                }
            }
        }
        let warnings = merged
            .into_iter()
            .map(|((light, _), merged)| TexelStepWarning {
                light: light as u32,
                site: merged.site,
                pairs: u32::try_from(merged.pairs).unwrap_or(u32::MAX),
                worst_difference: merged.worst_difference,
                position: merged.position.to_array(),
            })
            .collect();
        let elapsed = Duration::from_nanos(self.nanos.load(Ordering::Relaxed)) + started.elapsed();
        (TexelStepReport { warnings }, elapsed)
    }
}
