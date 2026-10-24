// Texel-step warnings: per-chart scans merged per (light, face) across cuts, stored with a stage's cache entry.
// See: context/lib/build_pipeline.md §Compiler pipeline (step 12)

use std::collections::BTreeMap;
use std::ops::Range;

use glam::Vec3;

use super::{scan_texel_steps, surface_separates};
use crate::chart_raster::{chart_interior_dims, chart_texel_world_position};
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
    /// malformed or names anything this bake does not have: a light past
    /// `lights`, a face past the bake's charts, a plain face that is now a
    /// cut sub-face, or a cut face whose parent or sub-face range differs
    /// from the bake's cut. Warnings must come in strictly ascending (light,
    /// face) order, as a bake writes them, so none can repeat.
    pub(crate) fn parse<'b>(
        bytes: &'b [u8],
        lights: usize,
        sites: &CutSites<'_>,
    ) -> Option<(Self, &'b [u8])> {
        let word = |bytes: &[u8], at: usize| -> Option<u32> {
            Some(u32::from_le_bytes(bytes.get(at..at + 4)?.try_into().ok()?))
        };
        let count = word(bytes, 0)? as usize;
        let end = count.checked_mul(RECORD_BYTES)?.checked_add(4)?;
        let records = bytes.get(4..end)?;
        let mut warnings: Vec<TexelStepWarning> = Vec::with_capacity(count);
        for record in records.as_chunks::<RECORD_BYTES>().0 {
            let w = |i: usize| word(record, i * 4);
            let light = w(0)?;
            if light as usize >= lights {
                return None;
            }
            let site = match w(1)? {
                0 if w(3)? == w(2)? && w(4)? == w(2)? => StepSite::Face(w(2)?),
                1 => StepSite::CutFace {
                    parent: w(2)?,
                    first_face: w(3)?,
                    last_face: w(4)?,
                },
                _ => return None,
            };
            if sites.site_of(site.first_face() as usize) != Some(site) {
                return None;
            }
            if warnings.last().is_some_and(|previous| {
                (previous.light, previous.site.first_face()) >= (light, site.first_face())
            }) {
                return None;
            }
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
    /// names the light list (`static`, `animated`) and `label` a light's
    /// source label by its index there: the adapter's name for it, so an
    /// author can find it in the editor. Labels are looked up here, never
    /// stored, so a replayed report names lights as the current map does. The
    /// wording is contract for both stages' tests.
    pub(crate) fn log<'l>(&self, kind: &str, label: impl Fn(u32) -> Option<&'l str>) {
        for warning in &self.warnings {
            let light =
                label(warning.light).map_or_else(|| format!("#{}", warning.light), str::to_owned);
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
                "[Lightmap] texel step: {kind} light {light} on {face} near ({x:.2}, {y:.2}, \
                 {z:.2}): {} adjacent texel pair(s) differ by up to {:.2} in stored \
                 visibility, so the shadow edge shows the texel lattice",
                warning.pairs,
                warning.worst_difference,
            );
        }
    }
}

/// Which cut face, if any, each chart is a sub-face of, and where each
/// sub-face's window sits on its parent grid.
pub(crate) struct CutSites<'a> {
    cut_parents: &'a [CutParent],
    chart_cut: Vec<Option<usize>>,
    /// Per chart, a sub-face's `[u, v]` window on its parent grid.
    windows: Vec<Option<[Range<u32>; 2]>>,
}

impl<'a> CutSites<'a> {
    pub(crate) fn new(cut_parents: &'a [CutParent], charts: &[Chart]) -> Self {
        let mut chart_cut = vec![None; charts.len()];
        for (index, cut) in cut_parents.iter().enumerate() {
            for slot in &mut chart_cut[cut.faces.clone()] {
                *slot = Some(index);
            }
        }
        let windows = charts
            .iter()
            .map(|chart| {
                chart.window.map(|window| {
                    let (width, height) = chart_interior_dims(chart);
                    let [u, v] = window.origin;
                    [u..u + width as u32, v..v + height as u32]
                })
            })
            .collect();
        Self {
            cut_parents,
            chart_cut,
            windows,
        }
    }

    /// Whether chart `chart` counts the pair from parent-grid texel `a` to
    /// its neighbour `b`. An uncut face counts every pair it holds. A cut
    /// face's sub-faces overlap, so each pair counts in one of them: the one
    /// whose own segment holds `a`; failing that, the one whose own segment
    /// holds `b`; failing both, the first whose window holds both. The
    /// fallbacks catch pairs reaching into a piece the polygon missed, which
    /// emits no sub-face, so the cut face checks every pair a sub-chart
    /// holds, as the uncut face does.
    fn owns_pair(&self, chart: usize, a: [u32; 2], b: [u32; 2]) -> bool {
        let Some(cut) = self.chart_cut[chart] else {
            return true;
        };
        let cut = &self.cut_parents[cut];
        let holds =
            |rect: &[Range<u32>; 2], [u, v]: [u32; 2]| rect[0].contains(&u) && rect[1].contains(&v);
        let me = chart - cut.faces.start;
        if holds(&cut.own[me], a) {
            return true;
        }
        let owner_of = |texel| cut.own.iter().position(|own| holds(own, texel));
        if let Some(owner) = owner_of(a).or_else(|| owner_of(b)) {
            return owner == me;
        }
        cut.faces.clone().position(|face| {
            self.windows[face]
                .as_ref()
                .is_some_and(|window| holds(window, a) && holds(window, b))
        }) == Some(me)
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

    /// The site chart `chart` reports under; `None` past the bake's charts.
    fn site_of(&self, chart: usize) -> Option<StepSite> {
        (chart < self.chart_cut.len()).then(|| self.site(chart))
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
/// cut sub-face only the pairs it owns count (`CutSites::owns_pair`), so each
/// pair across the cut counts once; `clear(from, to)` is the scene's segment
/// test for [`surface_separates`]. `None` when nothing steps.
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
    let on_parent = |[x, y]: [u32; 2]| [origin[0] + region.x + x, origin[1] + region.y + y];
    let steps = scan_texel_steps(
        region.width,
        region.height,
        fill_row,
        |a, b| sites.owns_pair(face, on_parent(a), on_parent(b)),
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::chart_raster::CHART_PADDING_TEXELS;
    use crate::lightmap_bake::ChartWindow;

    /// A chart `width × height` interior texels, a window at `origin` on an
    /// 8 × 8 parent grid when given.
    fn chart(width: u32, height: u32, origin: Option<[u32; 2]>) -> Chart {
        let padding = 2 * CHART_PADDING_TEXELS;
        Chart {
            origin: Vec3::ZERO,
            u_axis: Vec3::X,
            v_axis: Vec3::Z,
            uv_min: [0.0, 0.0],
            uv_extent: [width as f32, height as f32],
            normal: Vec3::Y,
            width_texels: width + padding,
            height_texels: height + padding,
            leaf_index: 0,
            window: origin.map(|origin| ChartWindow {
                grid_uv_min: [0.0, 0.0],
                grid_uv_extent: [8.0, 8.0],
                grid_interior: [8, 8],
                origin,
            }),
        }
    }

    /// Face 0 uncut; faces 1 and 2 the two halves of pre-cut face 1, cut at
    /// u = 4 with two texels of overlap; face 3 uncut.
    fn split_bake() -> (Vec<CutParent>, Vec<Chart>) {
        let cuts = vec![CutParent {
            parent: 1,
            faces: 1..3,
            own: vec![[0..4, 0..8], [4..8, 0..8]],
        }];
        let charts = vec![
            chart(8, 8, None),
            chart(6, 8, Some([0, 0])),
            chart(6, 8, Some([2, 0])),
            chart(8, 8, None),
        ];
        (cuts, charts)
    }

    fn warning(light: u32, site: StepSite) -> TexelStepWarning {
        TexelStepWarning {
            light,
            site,
            pairs: 3,
            worst_difference: 1.0,
            position: [1.0, 2.0, 3.0],
        }
    }

    const CUT: StepSite = StepSite::CutFace {
        parent: 1,
        first_face: 1,
        last_face: 2,
    };

    fn report(warnings: Vec<TexelStepWarning>) -> Vec<u8> {
        TexelStepReport { warnings }.to_bytes()
    }

    #[test]
    fn a_report_round_trips_and_returns_the_bytes_after_it() {
        let (cuts, charts) = split_bake();
        let sites = CutSites::new(&cuts, &charts);
        let stored = TexelStepReport {
            warnings: vec![
                warning(0, StepSite::Face(0)),
                warning(0, CUT),
                warning(1, StepSite::Face(3)),
            ],
        };
        let bytes = [stored.to_bytes(), b"section".to_vec()].concat();
        let (parsed, rest) = TexelStepReport::parse(&bytes, 2, &sites).expect("a valid report");
        assert_eq!(parsed, stored);
        assert_eq!(rest, b"section");
        let empty = TexelStepReport::default().to_bytes();
        assert_eq!(
            TexelStepReport::parse(&empty, 0, &sites),
            Some((TexelStepReport::default(), &[][..]))
        );
    }

    // Every malformed or stale prefix is refused, so the stage re-bakes
    // rather than logging a wrong or repeated warning.
    #[test]
    fn parse_rejects_every_prefix_this_bake_does_not_have() {
        let (cuts, charts) = split_bake();
        let sites = CutSites::new(&cuts, &charts);
        let parse = |bytes: &[u8], lights: usize| {
            TexelStepReport::parse(bytes, lights, &sites).map(|(report, _)| report)
        };
        let valid = report(vec![warning(0, StepSite::Face(0)), warning(1, CUT)]);
        assert!(parse(&valid, 2).is_some());

        // Truncated: no whole count, a short record, or a count past the
        // records present.
        assert!(parse(&valid[..3], 2).is_none());
        assert!(parse(&valid[..valid.len() - 1], 2).is_none());
        let mut overcount = valid.clone();
        overcount[0] = 3;
        assert!(parse(&overcount, 2).is_none());

        // A light past the bake's list.
        assert!(parse(&valid, 1).is_none());

        // A face past the bake's charts, plain or cut.
        assert!(parse(&report(vec![warning(0, StepSite::Face(4))]), 1).is_none());
        let past = StepSite::CutFace {
            parent: 1,
            first_face: 4,
            last_face: 5,
        };
        assert!(parse(&report(vec![warning(0, past)]), 1).is_none());

        // An unknown site kind, and a plain face whose spare words disagree.
        let mut kind = report(vec![warning(0, StepSite::Face(0))]);
        kind[8] = 2;
        assert!(parse(&kind, 1).is_none());
        let mut spare = report(vec![warning(0, StepSite::Face(0))]);
        spare[16] = 3;
        assert!(parse(&spare, 1).is_none());

        // Cut sites that do not match the bake's cut: another parent, a
        // shorter sub-face range, and plain faces that are now sub-faces.
        for stale in [
            StepSite::CutFace {
                parent: 0,
                first_face: 1,
                last_face: 2,
            },
            StepSite::CutFace {
                parent: 1,
                first_face: 1,
                last_face: 1,
            },
            StepSite::Face(1),
            StepSite::Face(2),
        ] {
            assert!(
                parse(&report(vec![warning(0, stale)]), 1).is_none(),
                "{stale:?}"
            );
        }

        // A cut site whose first face lies past its last.
        let backwards = StepSite::CutFace {
            parent: 1,
            first_face: 2,
            last_face: 1,
        };
        assert!(parse(&report(vec![warning(0, backwards)]), 1).is_none());

        // Out of (light, face) order, or repeated.
        let face = |light, face| warning(light, StepSite::Face(face));
        assert!(parse(&report(vec![face(1, 0), face(0, 3)]), 2).is_none());
        assert!(parse(&report(vec![face(0, 3), face(0, 0)]), 2).is_none());
        assert!(parse(&report(vec![face(0, 0), face(0, 0)]), 2).is_none());
    }

    // A pair counts in exactly one sub-face whenever some sub-chart holds
    // both its texels, including pairs that reach into a piece the polygon
    // missed, which emits no sub-face.
    #[test]
    fn every_pair_a_sub_chart_holds_counts_in_exactly_one_sub_face() {
        // An 8 × 8 parent cut at 4 on both axes; the polygon missed piece
        // (0, 0), so pieces (1, 0), (0, 1) and (1, 1) are faces 0, 1 and 2,
        // each window two texels past its inner cut lines.
        let cuts = vec![CutParent {
            parent: 0,
            faces: 0..3,
            own: vec![[4..8, 0..4], [0..4, 4..8], [4..8, 4..8]],
        }];
        let charts = vec![
            chart(6, 6, Some([2, 0])),
            chart(6, 6, Some([0, 2])),
            chart(6, 6, Some([2, 2])),
        ];
        let sites = CutSites::new(&cuts, &charts);
        let windows = [[2..8, 0..6], [0..6, 2..8], [2..8, 2..8]];
        let held = |face: usize, [u, v]: [u32; 2]| {
            windows[face][0].contains(&u) && windows[face][1].contains(&v)
        };
        let mut into_the_missed_piece = 0;
        for v in 0..8 {
            for u in 0..8 {
                let a = [u, v];
                for b in [[u + 1, v], [u, v + 1]] {
                    if b[0] >= 8 || b[1] >= 8 {
                        continue;
                    }
                    let holders: Vec<usize> =
                        (0..3).filter(|&f| held(f, a) && held(f, b)).collect();
                    let owners: Vec<usize> = holders
                        .iter()
                        .copied()
                        .filter(|&f| sites.owns_pair(f, a, b))
                        .collect();
                    let expected = usize::from(!holders.is_empty());
                    assert_eq!(owners.len(), expected, "{a:?}-{b:?}: {owners:?}");
                    if !owners.is_empty() && u < 4 && v < 4 {
                        into_the_missed_piece += 1;
                    }
                }
            }
        }
        assert!(into_the_missed_piece > 0, "the missed piece's pairs count");
        // An uncut face owns every pair.
        let uncut = [chart(8, 8, None)];
        assert!(CutSites::new(&[], &uncut).owns_pair(0, [0, 0], [1, 0]));
    }
}
