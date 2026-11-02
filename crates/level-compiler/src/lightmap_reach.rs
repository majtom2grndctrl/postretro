// Lightmap reach index: per eligible light, the open full-tree leaves open
// space can carry its light to, and per chart, the leaves and texels its
// shadow traces start in. The lightmap walk's reach cull reads it.
// See: context/lib/build_pipeline.md §Compiler pipeline step 12 (Exact skips)

use std::collections::HashSet;
use std::sync::OnceLock;

use rayon::prelude::*;

use crate::bake_control::BakeControl;
use crate::lightmap_bake::Chart;
use crate::lightmap_layer::{chart_texel_bounds, light_may_reach};
use crate::map_data::{LightType, MapLight};
use crate::partition::{BspTree, find_leaf_for_point};
use crate::texel_footprint::TexelFrame;

mod flood;
mod slab;

use flood::{Emitter, PortalGraph, ball_flood, reach_radius};
use slab::ChartFrame;

/// A set of full-tree leaves, one bit each.
#[derive(Debug, Clone)]
pub(crate) struct LeafSet {
    bits: Vec<u64>,
}

impl LeafSet {
    fn new(leaves: usize) -> Self {
        Self {
            bits: vec![0; leaves.div_ceil(64)],
        }
    }

    /// Whether `leaf` was absent.
    fn insert(&mut self, leaf: usize) -> bool {
        let (word, bit) = (leaf / 64, 1u64 << (leaf % 64));
        let absent = self.bits[word] & bit == 0;
        self.bits[word] |= bit;
        absent
    }

    pub(crate) fn contains(&self, leaf: usize) -> bool {
        self.bits
            .get(leaf / 64)
            .is_some_and(|word| word & (1 << (leaf % 64)) != 0)
    }
}

/// One light as the index takes it: the light and the stage's
/// embedded-emitter flag for it.
#[derive(Clone, Copy)]
pub(crate) struct ReachLight<'a> {
    pub(crate) light: &'a MapLight,
    pub(crate) embedded_emitter: bool,
}

/// Whether a light's pairs may be culled: a point or spot light the stage
/// does not flag as embedded, whose `f32` origin lies in an open leaf. Every
/// other light is never culled.
fn eligible(tree: &BspTree, closed: &[bool], light: ReachLight<'_>) -> bool {
    if light.embedded_emitter || light.light.light_type == LightType::Directional {
        return false;
    }
    let emitter = Emitter::of(light.light);
    if !emitter.centre.is_finite() || !light.light.falloff_range.is_finite() {
        return false;
    }
    let leaf = find_leaf_for_point(tree, emitter.centre);
    closed.get(leaf).is_some_and(|&closed| !closed)
}

/// Per full-tree leaf, whether the flood treats it as closed: solid, or in
/// the exterior flood.
pub(crate) fn closed_leaves(tree: &BspTree, exterior: &HashSet<usize>) -> Vec<bool> {
    tree.leaves
        .iter()
        .enumerate()
        .map(|(leaf, data)| data.is_solid || exterior.contains(&leaf))
        .collect()
}

/// How many of `lights` the reach cull could cull: what the stage's
/// cull-off warning counts.
pub(crate) fn eligible_light_count(
    tree: &BspTree,
    closed: &[bool],
    lights: &[ReachLight<'_>],
) -> usize {
    if tree.nodes.is_empty() {
        return 0;
    }
    lights
        .iter()
        .filter(|&&light| eligible(tree, closed, light))
        .count()
}

/// A chart's reach data, shared by every light that walks it: the open
/// leaves its slab touches, sorted, and which texels' own slabs touch a
/// closed leaf and so must trace.
pub(crate) struct ChartReach {
    open_leaves: Vec<u32>,
    width: i32,
    traced: Vec<u64>,
}

impl ChartReach {
    /// Whether interior texel `(tx, ty)` of a culled pair stores 0 without
    /// tracing: its slab touches open leaves only.
    pub(crate) fn skips(&self, tx: i32, ty: i32) -> bool {
        let bit = ty as usize * self.width as usize + tx as usize;
        self.traced[bit / 64] & (1 << (bit % 64)) == 0
    }

    /// Whether no texel traces.
    pub(crate) fn skips_all(&self) -> bool {
        self.traced.iter().all(|&word| word == 0)
    }

    /// The open full-tree leaves the chart's slab touches, ascending.
    #[cfg(test)]
    pub(crate) fn open_leaves(&self) -> &[u32] {
        &self.open_leaves
    }
}

/// A chart's frame and reach, built once, by the first walk that asks.
struct ChartEntry {
    frame: Option<ChartFrame>,
    reach: OnceLock<Option<ChartReach>>,
}

/// One eligible light's flood.
struct LightReach {
    emitter: Emitter,
    reached: LeafSet,
    /// The flood's R.
    #[cfg(test)]
    reach: f64,
}

/// Test seams on the index: where its probe notes go, and the charts its
/// slabs are built from in place of the walk's.
#[cfg(test)]
#[derive(Default)]
struct IndexSeams {
    stats: Option<&'static crate::lightmap_layer::SkipStats>,
    slab_charts: Option<&'static [Chart]>,
}

/// The lightmap reach index over one bake's lights (by position in the
/// stage's layer-light list) and charts (by atlas index).
pub(crate) struct LightmapReachIndex<'t> {
    tree: &'t BspTree,
    closed: Vec<bool>,
    lights: Vec<Option<LightReach>>,
    charts: Vec<OnceLock<ChartEntry>>,
    penumbra_floor: bool,
    #[cfg(test)]
    seams: IndexSeams,
}

impl<'t> LightmapReachIndex<'t> {
    /// Flood every eligible light of `lights` over `tree`'s open leaves. Its
    /// own portal graph lives only until every flood is built. Floods run
    /// in parallel, each under one governor permit.
    pub(crate) fn build(
        tree: &'t BspTree,
        closed: Vec<bool>,
        lights: &[ReachLight<'_>],
        charts: &[Chart],
        penumbra_floor: bool,
        control: &BakeControl,
    ) -> Self {
        let graph = {
            let _permit = control.governor().enter();
            PortalGraph::new(tree)
        };
        let chart_bounds: Vec<_> = charts.iter().map(chart_texel_bounds).collect();
        let chart_radius: Vec<f64> = charts
            .iter()
            .map(|chart| {
                if penumbra_floor {
                    f64::from(TexelFrame::for_chart(chart).radius())
                } else {
                    // The centre sampler traces from the texel centre alone.
                    0.0
                }
            })
            .collect();
        let floods: Vec<Option<LightReach>> = lights
            .par_iter()
            .map(|&light| {
                if tree.nodes.is_empty() || !eligible(tree, &closed, light) {
                    return None;
                }
                let _permit = control.governor().enter();
                // The largest footprint among the charts the light's
                // range-only reach admits: the walk plans no other chart.
                let footprint = chart_bounds
                    .iter()
                    .zip(&chart_radius)
                    .filter(|(bounds, _)| light_may_reach(light.light, **bounds))
                    .map(|(_, &radius)| radius)
                    .fold(0.0, f64::max);
                let emitter = Emitter::of(light.light);
                let reach = reach_radius(light.light, emitter, footprint);
                ball_flood(tree, &closed, &graph, emitter, reach).map(|reached| LightReach {
                    emitter,
                    reached,
                    #[cfg(test)]
                    reach,
                })
            })
            .collect();
        drop(graph);
        Self {
            tree,
            closed,
            lights: floods,
            charts: (0..charts.len()).map(|_| OnceLock::new()).collect(),
            penumbra_floor,
            #[cfg(test)]
            seams: IndexSeams::default(),
        }
    }

    /// Take the test seams `exact_skips` carries: its tally receives a note
    /// at every chart mask build, and its slab charts replace the walk's.
    #[cfg(test)]
    pub(crate) fn with_seams(mut self, exact_skips: crate::lightmap_layer::ExactSkips) -> Self {
        self.seams = IndexSeams {
            stats: exact_skips.stats,
            slab_charts: exact_skips.slab_charts,
        };
        self
    }

    /// Lights whose pairs the index may cull.
    #[cfg(test)]
    pub(crate) fn culling_light_count(&self) -> usize {
        self.lights.iter().filter(|light| light.is_some()).count()
    }

    /// The chart's reach, building it on first use. Serial, taking no lock
    /// or permit: walks that need it while another builds it wait holding
    /// their permits, as for the shaft scene.
    fn chart(&self, chart_idx: usize, chart: &Chart) -> Option<(&ChartFrame, &ChartReach)> {
        let entry = self.charts[chart_idx].get_or_init(|| {
            #[cfg(test)]
            let chart = self
                .seams
                .slab_charts
                .map_or(chart, |charts| &charts[chart_idx]);
            ChartEntry {
                frame: ChartFrame::new(chart, self.penumbra_floor),
                reach: OnceLock::new(),
            }
        });
        let frame = entry.frame.as_ref()?;
        let reach = entry
            .reach
            .get_or_init(|| {
                #[cfg(test)]
                if let Some(stats) = self.seams.stats {
                    stats.note_reach(crate::lightmap_layer::ReachPhase::MaskBuild, chart_idx);
                }
                Some(frame.reach(self.tree, &self.closed))
            })
            .as_ref()?;
        Some((frame, reach))
    }

    /// The chart's reach when the `(light, chart)` pair is culled: the light
    /// is eligible, its emitter clears the chart's slab, and every open leaf
    /// the slab touches lies outside its flood. `None` keeps the pair.
    pub(crate) fn culled_chart(
        &self,
        light: usize,
        chart_idx: usize,
        chart: &Chart,
    ) -> Option<&ChartReach> {
        let light = self.lights.get(light)?.as_ref()?;
        let (frame, reach) = self.chart(chart_idx, chart)?;
        let culled = frame.emitter_clears(light.emitter.centre, light.emitter.size)
            && !reach
                .open_leaves
                .iter()
                .any(|&leaf| light.reached.contains(leaf as usize));
        culled.then_some(reach)
    }

    /// The chart's reach for a pair forced culled, whatever the flood
    /// reached: the planted fault verify mode must catch.
    #[cfg(test)]
    pub(crate) fn forced_chart(&self, chart_idx: usize, chart: &Chart) -> Option<&ChartReach> {
        self.chart(chart_idx, chart).map(|(_, reach)| reach)
    }

    /// `light`'s R; `None` for a light the index never culls.
    #[cfg(test)]
    pub(crate) fn reach_radius(&self, light: usize) -> Option<f64> {
        Some(self.lights.get(light)?.as_ref()?.reach)
    }

    /// Whether `light`'s flood reached full-tree leaf `leaf`; `None` for a
    /// light the index never culls.
    #[cfg(test)]
    pub(crate) fn reaches(&self, light: usize, leaf: usize) -> Option<bool> {
        Some(self.lights.get(light)?.as_ref()?.reached.contains(leaf))
    }
}

#[cfg(test)]
mod seed_tests;
#[cfg(test)]
pub(crate) mod test_fixtures;
#[cfg(test)]
mod tests;
