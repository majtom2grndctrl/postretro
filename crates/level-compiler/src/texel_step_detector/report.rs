// Texel-step warnings: per-chart scans merged per (light, face) across cuts, stored with a stage's cache entry.
// See: context/lib/build_pipeline.md §Compiler pipeline (step 12)

use std::collections::BTreeMap;
use std::ops::Range;

use glam::{DVec3, Vec3};

use super::{scan_texel_steps, surface_separates};
use crate::chart_raster::chart_texel_world_position;
use crate::lightmap_bake::{Chart, CutParent};

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
    /// The light's index in the list the stage bakes: the static stage's
    /// layer lights, or the weight-map bake's animated lights.
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

/// One warning in a cache entry: ten little-endian words — light, site kind
/// (0 face, 1 cut face), site, first face, last face, pairs, worst difference
/// bits, then the position's bits.
const RECORD_WORDS: usize = 10;
const RECORD_BYTES: usize = RECORD_WORDS * 4;

impl TexelStepReport {
    /// A cache entry's report prefix: the warning count, then the records.
    pub(crate) fn to_bytes(&self) -> Vec<u8> {
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
    pub(crate) fn parse(bytes: &[u8], lights: usize, faces: usize) -> Option<(Self, &[u8])> {
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

    /// One warning per stepped (light, face), on the caller's thread. `kind`
    /// names the light list (`static`, `animated`) and `origin` a light's
    /// position by its index there. The wording is contract for the stages'
    /// tests.
    pub(crate) fn log(&self, kind: &str, origin: impl Fn(u32) -> DVec3) {
        for warning in &self.warnings {
            let origin = origin(warning.light);
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
                "[Lightmap] texel step: {kind} light #{} at ({:.2}, {:.2}, {:.2}) on {face} near \
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

/// Which cut face, if any, each chart is a sub-face of.
pub(crate) struct CutSites<'a> {
    cut_parents: &'a [CutParent],
    chart_cut: Vec<Option<usize>>,
}

impl<'a> CutSites<'a> {
    pub(crate) fn new(cut_parents: &'a [CutParent], charts: usize) -> Self {
        let mut chart_cut = vec![None; charts];
        for (index, cut) in cut_parents.iter().enumerate() {
            for slot in &mut chart_cut[cut.faces.clone()] {
                *slot = Some(index);
            }
        }
        Self {
            cut_parents,
            chart_cut,
        }
    }

    /// A cut sub-face's own `[u, v]` segment on its parent grid; `None` for
    /// an uncut face, which owns every pair it holds.
    fn own(&self, chart: usize) -> Option<[Range<u32>; 2]> {
        self.chart_cut[chart].map(|cut| {
            let cut = &self.cut_parents[cut];
            cut.own[chart - cut.faces.start].clone()
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
}

/// A rectangle of a chart's interior texels.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct TexelRect {
    pub(crate) x: u32,
    pub(crate) y: u32,
    pub(crate) width: u32,
    pub(crate) height: u32,
}

/// One chart's steps for one light, its first texel on the parent grid.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct ChartSteps {
    chart: usize,
    pairs: usize,
    worst_difference: f32,
    first: [u32; 2],
    position: Vec3,
}

/// The steps among `region` of chart `face`, streamed a row at a time:
/// `fill_row(y, row)` writes the region's row `y`, absent texels `None`. Of a
/// cut sub-face only the pairs its own segment begins count, so each pair
/// across the cut counts once; `clear(from, to)` is the scene's segment test
/// for [`surface_separates`]. `None` when nothing steps.
pub(crate) fn scan_chart(
    chart: &Chart,
    face: usize,
    sites: &CutSites<'_>,
    region: TexelRect,
    fill_row: impl FnMut(u32, &mut [Option<f32>]),
    clear: impl Fn(Vec3, Vec3) -> bool,
    threshold: f32,
) -> Option<ChartSteps> {
    let origin = chart.window.map_or([0, 0], |window| window.origin);
    let own = sites.own(face);
    let steps = scan_texel_steps(
        region.width,
        region.height,
        fill_row,
        |x, y| {
            own.as_ref().is_none_or(|[u, v]| {
                u.contains(&(origin[0] + region.x + x)) && v.contains(&(origin[1] + region.y + y))
            })
        },
        |a, b| {
            surface_separates(
                chart,
                [(region.x + a[0]) as i32, (region.y + a[1]) as i32],
                [(region.x + b[0]) as i32, (region.y + b[1]) as i32],
                &clear,
            )
        },
        threshold,
    );
    let [x, y] = steps.first?;
    let (tx, ty) = (region.x + x, region.y + y);
    Some(ChartSteps {
        chart: face,
        pairs: steps.pairs,
        worst_difference: steps.worst_difference,
        first: [origin[0] + tx, origin[1] + ty],
        position: chart_texel_world_position(chart, tx as i32, ty as i32),
    })
}

/// One warning per stepped (light, face) from per-chart scans tagged with
/// their light's index, a cut face's sub-faces merged into the face the
/// author drew. A tie for a site's earliest texel keeps the first in `found`.
pub(crate) fn merge_chart_steps(
    found: impl IntoIterator<Item = (usize, ChartSteps)>,
    sites: &CutSites<'_>,
) -> TexelStepReport {
    struct Merged {
        site: StepSite,
        pairs: u64,
        worst_difference: f32,
        first: [u32; 2],
        position: Vec3,
    }
    let mut merged: BTreeMap<(usize, u32), Merged> = BTreeMap::new();
    for (light, chart) in found {
        let site = sites.site(chart.chart);
        let entry = merged.entry((light, site.first_face())).or_insert(Merged {
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
    TexelStepReport { warnings }
}
