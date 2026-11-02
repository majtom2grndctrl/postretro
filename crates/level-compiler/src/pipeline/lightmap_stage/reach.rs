// Stage wiring of the lightmap reach cull: the coverage proof's switch and
// hole warnings, the reach index's lifetime, and the stage's one info line.
// See: context/lib/build_pipeline.md §Compiler pipeline step 12 (Exact skips)

use std::collections::HashSet;

use crate::bake_control::BakeControl;
use crate::coverage_proof::CoverageReport;
use crate::light_namespaces::StaticBakedEntry;
use crate::lightmap_bake::Chart;
use crate::lightmap_layer::{ExactSkips, ReachMismatchSlot, ReachTally, ReachWalk};
use crate::lightmap_reach::{LightmapReachIndex, ReachLight, closed_leaves, eligible_light_count};
use crate::map_data::MapAssembly;
use crate::partition::BspTree;
use crate::pipeline::watertight_open_edge_provenance;
use crate::texel_footprint::EmbeddedEmitters;

use super::reach_verify::reach_verify_enabled;

/// What the stage needs to cull by reach: the full compiler BSP, its
/// exterior flood, and how many holes the coverage proof found. Production
/// always passes it; a test fixture without a tree passes `None`, which
/// culls nothing.
#[derive(Clone, Copy)]
pub(crate) struct ReachInputs<'t> {
    pub(crate) tree: &'t BspTree,
    pub(crate) exterior: &'t HashSet<usize>,
    /// Unproven spans: any one switches the cull off for the whole map.
    pub(crate) holes: usize,
}

/// Warn about every hole the coverage proof found: the only author-facing
/// hole warning. Each sample names its location and brush, up to the sample
/// cap, after the total. Runs every build; never fails it.
pub(crate) fn warn_coverage_holes(
    report: &CoverageReport,
    brush_assembly: &[Option<usize>],
    assemblies: &[MapAssembly],
) {
    if report.unproven.is_empty() {
        return;
    }
    let samples = report.samples();
    log::warn!(
        "[Compiler] Coverage proof: {} hole(s) in world geometry ({:.3} m of face edge no \
         face closes within the tracer's tolerance). The build still succeeds. Locations \
         (world-space meters, showing {} of {}):",
        report.unproven.len(),
        report.unproven_length(),
        samples.len(),
        report.unproven.len(),
    );
    for span in samples {
        let at = span.midpoint;
        let brush = match span.open_edge() {
            Some(edge) => {
                match watertight_open_edge_provenance(&edge, brush_assembly, assemblies) {
                    Some(group) => format!("brush {} in group {group}", edge.brush_index),
                    None => format!("brush {}", edge.brush_index),
                }
            }
            None => "no source brush".to_string(),
        };
        log::warn!(
            "[Compiler]   hole near ({:.3}, {:.3}, {:.3}), {:.3} m long — {brush}",
            at.x,
            at.y,
            at.z,
            span.length,
        );
    }
}

/// The reach cull over one stage run.
pub(super) struct StageReach<'t> {
    tree: &'t BspTree,
    closed: Vec<bool>,
    /// The stage's layer lights, in its order, with its embedded-emitter
    /// flags: the cull follows the stage's flag at either floor setting and
    /// runs no embedding test of its own.
    lights: Vec<ReachLight<'t>>,
    /// Each layer light's source index, for naming it.
    sources: Vec<usize>,
    index: Option<LightmapReachIndex<'t>>,
    /// The coverage proof found a hole: nothing is culled.
    switched_off: bool,
    pub(super) verify: bool,
    pub(super) tally: ReachTally,
    pub(super) mismatch: ReachMismatchSlot,
}

impl<'t> StageReach<'t> {
    /// Settle the switch, and warn when holes turn the cull off for a map
    /// with a light it would cull. Cheap: no flood runs here.
    pub(super) fn new(
        inputs: ReachInputs<'t>,
        layer_lights: &[&StaticBakedEntry<'t>],
        embedded_emitters: &EmbeddedEmitters,
    ) -> Self {
        let lights: Vec<ReachLight<'t>> = layer_lights
            .iter()
            .map(|entry| ReachLight {
                light: entry.light,
                embedded_emitter: embedded_emitters.is_embedded(entry.source_index),
            })
            .collect();
        let sources = layer_lights
            .iter()
            .map(|entry| entry.source_index)
            .collect();
        let closed = closed_leaves(inputs.tree, inputs.exterior);
        let switched_off = inputs.holes > 0;
        if switched_off {
            let eligible = eligible_light_count(inputs.tree, &closed, &lights);
            if eligible > 0 {
                log::warn!(
                    "[Compiler] Lightmap reach cull is off for the whole map: the coverage \
                     proof found {} hole(s) (warned above), so open space may not bound where \
                     {eligible} light(s) reach. Lighting is unchanged; Lightmap Bake runs slower.",
                    inputs.holes,
                );
            }
        }
        Self {
            tree: inputs.tree,
            closed,
            lights,
            sources,
            index: None,
            switched_off,
            verify: reach_verify_enabled(),
            tally: ReachTally::default(),
            mismatch: ReachMismatchSlot::default(),
        }
    }

    /// Build the index before a pass walks, once, unless the cull is off.
    pub(super) fn build_index(
        &mut self,
        charts: &[Chart],
        penumbra_floor: bool,
        exact_skips: ExactSkips,
        control: &BakeControl,
    ) {
        #[cfg(test)]
        let wanted = exact_skips.reach_cull || exact_skips.force_reach_cull;
        #[cfg(not(test))]
        let wanted = exact_skips.reach_cull;
        if self.index.is_some() || self.switched_off || !wanted {
            return;
        }
        let index = LightmapReachIndex::build(
            self.tree,
            self.closed.clone(),
            &self.lights,
            charts,
            penumbra_floor,
            control,
        );
        #[cfg(test)]
        let index = index.with_seams(exact_skips);
        self.index = Some(index);
    }

    /// The light's walk context, when the index may cull its pairs.
    pub(super) fn walk(&self, light: usize) -> Option<ReachWalk<'_>> {
        self.index.as_ref().map(|index| ReachWalk {
            index,
            light,
            verify: self.verify,
            tally: &self.tally,
            mismatch: &self.mismatch,
        })
    }

    /// Log the stage line, then fail on a verify mismatch, naming the light
    /// by its source label.
    pub(super) fn finish(&self, light_source_labels: &[String]) -> anyhow::Result<()> {
        let read = ReachTally::read;
        let tally = &self.tally;
        #[cfg(test)]
        LAST_REACH_COUNTS.with(|last| {
            last.set(Some(ReachCounts {
                walked: read(&tally.pairs_walked),
                culled: read(&tally.pairs_culled),
                skipped: read(&tally.texels_skipped),
                traced_in_culled: read(&tally.texels_traced_in_culled),
                rewalked: read(&tally.pairs_rewalked),
                baked: read(&tally.partitions_baked),
                loaded: read(&tally.partitions_loaded),
                switched_off: self.switched_off,
                verify: self.verify,
            }))
        });
        let (baked, loaded) = (
            read(&tally.partitions_baked),
            read(&tally.partitions_loaded),
        );
        let off = if self.switched_off {
            "; switched off for the whole map by coverage-proof holes"
        } else {
            ""
        };
        if baked == 0 {
            log::info!(
                "[Compiler] Lightmap reach cull: stage served from cache ({loaded} partition(s) \
                 loaded, no pair walked){off}"
            );
        } else {
            let verify = if self.verify {
                format!(", {} re-walked by verify mode", read(&tally.pairs_rewalked))
            } else {
                String::new()
            };
            log::info!(
                "[Compiler] Lightmap reach cull: {} pair(s) walked, {} culled{verify}; {} texel(s) \
                 skipped, {} traced inside culled pairs; {baked} partition(s) walked this build, \
                 {loaded} from cache{off}",
                read(&tally.pairs_walked),
                read(&tally.pairs_culled),
                read(&tally.texels_skipped),
                read(&tally.texels_traced_in_culled),
            );
        }
        match self.mismatch.take() {
            Some(mismatch) => {
                let label = self
                    .sources
                    .get(mismatch.light)
                    .and_then(|&source| light_source_labels.get(source))
                    .map_or_else(String::new, |label| format!(" ({label})"));
                Err(anyhow::anyhow!("[Compiler] {mismatch}{label}"))
            }
            None => Ok(()),
        }
    }
}

/// The stage's reach totals, as its line reports them.
#[cfg(test)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct ReachCounts {
    pub(crate) walked: u64,
    pub(crate) culled: u64,
    pub(crate) skipped: u64,
    pub(crate) traced_in_culled: u64,
    pub(crate) rewalked: u64,
    pub(crate) baked: u64,
    pub(crate) loaded: u64,
    pub(crate) switched_off: bool,
    pub(crate) verify: bool,
}

#[cfg(test)]
thread_local! {
    /// The last stage run's totals on this thread, set as its line logs.
    pub(crate) static LAST_REACH_COUNTS: std::cell::Cell<Option<ReachCounts>> =
        const { std::cell::Cell::new(None) };
}
