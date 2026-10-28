// The occluder partition and the runtime cells it yields: classification on a
// first-pass tree of every brush, the two-tier build, and the cell view and
// cell-id portal set every runtime-cell reader takes.
// See: context/plans/in-progress/occluder-first-partition/plan.md §Approach notes

use std::collections::HashSet;

use anyhow::Result;

use crate::map_data::BrushVolume;
use crate::partition::occluder_classify::{
    OCCLUDER_ABSOLUTE_LENGTH_M, OCCLUDER_FLOOR_M, OCCLUDER_SEE_OVER_M, OCCLUDER_SPAN_FRACTION,
    OccluderClassification, OccluderVerdict, OpenSpace, classify_occluders,
};
use crate::partition::occluder_cluster::OCCLUDER_COVERAGE_FRACTION;
use crate::partition::{
    self, BspTree, CellTree, OccluderTier, PartitionResult, build_bsp_from_brushes,
};
use crate::portals::{self, Portal};
use crate::visibility;

/// The two-tier partition: the full tree with its faces, and the record of
/// which of its leaves each occluder-tier leaf holds.
pub(crate) struct OccluderPartition {
    pub(crate) result: PartitionResult,
    pub(crate) tier: OccluderTier,
}

/// Classify every static brush, then build the two-tier tree on the verdicts
/// and extract world faces. The classification is logged at info.
pub(crate) fn partition_with_occluders(brushes: &[BrushVolume]) -> Result<OccluderPartition> {
    let classification = classify(brushes)?;
    log_classification(&classification);
    let (result, tier) =
        partition::partition_with_occluder_tier(brushes, &classification.tier_roles())?;
    Ok(OccluderPartition { result, tier })
}

/// Classify every brush against a first pass: today's tree of every brush and
/// its exterior flood, which bound each face's open space.
pub(crate) fn classify(brushes: &[BrushVolume]) -> Result<OccluderClassification> {
    if brushes.is_empty() {
        return Ok(OccluderClassification {
            verdicts: Vec::new(),
            clusters: Vec::new(),
        });
    }
    let tree = build_bsp_from_brushes(brushes)?;
    let first_pass_portals = portals::generate_portals(&tree);
    let exterior = visibility::find_exterior_leaves_quietly(&tree, &first_pass_portals);
    Ok(classify_occluders(
        brushes,
        &OpenSpace::new(&tree, &exterior),
    ))
}

fn log_classification(classification: &OccluderClassification) {
    let count = |verdict: OccluderVerdict| {
        classification
            .verdicts
            .iter()
            .filter(|&&v| v == verdict)
            .count()
    };
    let sections = || {
        classification
            .clusters
            .iter()
            .flat_map(|cluster| &cluster.sections)
    };
    let occluding_members: HashSet<usize> = sections()
        .filter(|section| section.occludes)
        .flat_map(|section| section.members.iter().copied())
        .collect();
    log::info!(
        "[Compiler] Occluder classification: {} brushes, {} occlude alone; exempt alone: \
         {} under floor, {} no open face, {} short span, {} low top; {} clusters, {} sections, \
         {} occlude ({} member brushes); constants: floor {OCCLUDER_FLOOR_M} m, absolute \
         length {OCCLUDER_ABSOLUTE_LENGTH_M} m, span fraction {OCCLUDER_SPAN_FRACTION}, \
         see-over {OCCLUDER_SEE_OVER_M} m, coverage {OCCLUDER_COVERAGE_FRACTION}",
        classification.verdicts.len(),
        count(OccluderVerdict::Occludes),
        count(OccluderVerdict::UnderFloor),
        count(OccluderVerdict::NoOpenFace),
        count(OccluderVerdict::ShortSpan),
        count(OccluderVerdict::LowTop),
        classification.clusters.len(),
        sections().count(),
        sections().filter(|section| section.occludes).count(),
        occluding_members.len(),
    );
}

/// The runtime cells of a two-tier tree and the portals between them, in
/// cell ids. Every stage that reads cell ids reads these; the full tree and
/// its exterior flood stay with the readers of exact sub-leaf solidity.
pub(crate) struct RuntimeCells {
    pub(crate) cells: CellTree,
    pub(crate) portals: Vec<Portal>,
}

impl RuntimeCells {
    /// Group `tree`'s leaves into cells by `tier` and generate the runtime
    /// portals at the tier's nodes. `exterior_leaves` is the full tree's
    /// flood over its own portals, `full_tree_portal_count` their number,
    /// for the log.
    pub(crate) fn build(
        tree: &BspTree,
        tier: &OccluderTier,
        exterior_leaves: &HashSet<usize>,
        full_tree_portal_count: usize,
    ) -> Self {
        let cells = CellTree::build(tree, tier, exterior_leaves);
        let tier_portals = portals::generate_occluder_tier_portals(tree, tier, exterior_leaves);
        let portals = portals::runtime_portals(tier_portals, &cells);
        let solid = cells
            .tree
            .leaves
            .iter()
            .filter(|cell| cell.is_solid)
            .count();
        log::info!(
            "[Compiler] Runtime cells: {} occluder-tier leaves; {} cells ({} interior, {} solid, \
             {} exterior); {} runtime portals; full tree {} leaves, {} portals",
            tier.leaves.len(),
            cells.tree.leaves.len(),
            cells.interior_cell_count(),
            solid,
            cells.exterior_cells.len(),
            portals.len(),
            tree.leaves.len(),
            full_tree_portal_count,
        );
        Self { cells, portals }
    }
}

#[cfg(test)]
mod test_fixtures;

#[cfg(test)]
mod tests;
