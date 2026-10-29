// The occluder partition and the runtime cells it yields: classification on a
// first-pass tree of every brush, the two-tier build, and the cell view and
// cell-id portal set every runtime-cell reader takes.
// See: context/plans/in-progress/occluder-first-partition/plan.md §Approach notes

use std::collections::HashSet;

use anyhow::Result;

use crate::map_data::{BrushVolume, MapKinematicMover};
use crate::partition::occluder_classify::{
    OCCLUDER_ABSOLUTE_LENGTH_M, OCCLUDER_FLOOR_M, OCCLUDER_SEE_OVER_M, OCCLUDER_SPAN_FRACTION,
    OccluderClassification, OccluderVerdict, OpenSpace, classify_occluders,
};
use crate::partition::occluder_cluster::OCCLUDER_COVERAGE_FRACTION;
use crate::partition::{
    self, BspTree, CAP_MIN_OPEN_SLICE_FRACTION, CAP_MIN_PIECE_FRACTION, CELL_SIZE_CAP_M,
    CellSizeCap, CellTree, DoorSplitter, FREE_END_MIN_OPEN_PROBES, FREE_END_PROBE_DEPTHS_M,
    FREE_END_PROBE_FRACTIONS, FREE_END_PROBE_OFFSET_M, OccluderTier, PartitionResult,
    SHADOW_PRISM_LENGTH_FACTOR, ShadowPrism, build_bsp_from_brushes, recognize_doors,
    shadow_prisms,
};
use crate::portals::{self, Portal};
use crate::visibility;

/// The two-tier partition: the full tree with its faces, and the record of
/// which of its leaves each occluder-tier leaf holds.
pub(crate) struct OccluderPartition {
    pub(crate) result: PartitionResult,
    pub(crate) tier: OccluderTier,
}

/// Classify every static brush and recognise the doors among `movers`, then
/// build the two-tier tree on the verdicts, the shadow prisms of the
/// occluding wall faces, the door splitters and the cell-size cap, and extract
/// world faces. Classification, prisms and doors are logged at info.
pub(crate) fn partition_with_occluders(
    brushes: &[BrushVolume],
    movers: &[MapKinematicMover],
) -> Result<OccluderPartition> {
    let first_pass = FirstPass::build(brushes)?;
    let (classification, doors) = match &first_pass {
        Some(first_pass) => {
            let space = first_pass.space();
            (
                classify_occluders(brushes, &space),
                recognize_doors(movers, brushes, &space),
            )
        }
        None => (OccluderClassification::default(), Vec::new()),
    };
    log_classification(&classification);
    log_doors(&doors, movers);
    let cap = first_pass
        .as_ref()
        .map(|first_pass| CellSizeCap::new(first_pass.space(), &classification));
    let prisms = first_pass
        .as_ref()
        .map(|first_pass| shadow_prisms(brushes, &classification, &first_pass.space()))
        .unwrap_or_default();
    log_prisms(&prisms);
    let (result, tier) = partition::partition_with_occluder_tier(
        brushes,
        &classification.tier_roles(),
        &prisms,
        &doors,
        cap.as_ref(),
    )?;
    Ok(OccluderPartition { result, tier })
}

/// Classify every brush against a first pass: the single-tier tree of every
/// static brush and its exterior flood, which bound each face's open space.
#[cfg(test)]
pub(crate) fn classify(brushes: &[BrushVolume]) -> Result<OccluderClassification> {
    Ok(match FirstPass::build(brushes)? {
        Some(first_pass) => classify_occluders(brushes, &first_pass.space()),
        None => OccluderClassification::default(),
    })
}

/// The partition and runtime cells `pipeline.rs` builds across its
/// partition and visibility stages, for tests that must exercise what ships.
#[cfg(test)]
pub(crate) struct ShippedCells {
    pub(crate) partition: OccluderPartition,
    /// The full tree's exterior flood over its own portals.
    pub(crate) full_exterior: HashSet<usize>,
    pub(crate) runtime: RuntimeCells,
}

#[cfg(test)]
impl ShippedCells {
    pub(crate) fn build(brushes: &[BrushVolume], movers: &[MapKinematicMover]) -> Result<Self> {
        let partition = partition_with_occluders(brushes, movers)?;
        let (runtime, full_exterior) =
            RuntimeCells::of_partition(&partition.result.tree, &partition.tier);
        Ok(Self {
            partition,
            full_exterior,
            runtime,
        })
    }
}

/// The doors among `movers`, recognised against the first pass as
/// `partition_with_occluders` recognises them.
#[cfg(test)]
pub(crate) fn doors(
    brushes: &[BrushVolume],
    movers: &[MapKinematicMover],
) -> Result<Vec<DoorSplitter>> {
    Ok(match FirstPass::build(brushes)? {
        Some(first_pass) => recognize_doors(movers, brushes, &first_pass.space()),
        None => Vec::new(),
    })
}

/// The single-tier tree of every static brush and its exterior flood: the
/// open space classification and door recognition measure against. `None`
/// for a map with no brushes.
struct FirstPass {
    tree: BspTree,
    exterior: HashSet<usize>,
}

impl FirstPass {
    fn build(brushes: &[BrushVolume]) -> Result<Option<Self>> {
        if brushes.is_empty() {
            return Ok(None);
        }
        let tree = build_bsp_from_brushes(brushes)?;
        let first_pass_portals = portals::generate_portals(&tree);
        let exterior = visibility::find_exterior_leaves_quietly(&tree, &first_pass_portals);
        Ok(Some(Self { tree, exterior }))
    }

    fn space(&self) -> OpenSpace<'_> {
        OpenSpace::new(&self.tree, &self.exterior)
    }
}

fn log_prisms(prisms: &[ShadowPrism]) {
    let probes = FREE_END_PROBE_FRACTIONS.len() * FREE_END_PROBE_DEPTHS_M.len();
    log::info!(
        "[Compiler] Shadow prisms: {} occluding wall faces with a free end, {} end planes, \
         {} ended by the length bound ({SHADOW_PRISM_LENGTH_FACTOR} × the face's longer broad \
         extent); an end is free when it meets no solid and at least \
         {FREE_END_MIN_OPEN_PROBES} of its {probes} probes {FREE_END_PROBE_OFFSET_M} m past it \
         are open",
        prisms.len(),
        prisms
            .iter()
            .map(|prism| prism.splitters.len())
            .sum::<usize>(),
        prisms.iter().filter(|prism| prism.truncated).count(),
    );
}

fn log_doors(doors: &[DoorSplitter], movers: &[MapKinematicMover]) {
    let ids: Vec<String> = doors
        .iter()
        .map(|door| movers[door.mover_index].mover_id.to_string())
        .collect();
    log::info!(
        "[Compiler] Door recognition: {} of {} kinematic movers seal a doorway at their \
         closed pose{}{}",
        doors.len(),
        movers.len(),
        if ids.is_empty() { "" } else { ": movers " },
        ids.join(", "),
    );
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
    /// The runtime cells of a two-tier partition as the pipeline ships them,
    /// and the full tree's exterior flood over its own portals, which the
    /// readers of exact sub-leaf solidity take. The full tree's portals feed
    /// only that flood and are dropped here.
    pub(crate) fn of_partition(tree: &BspTree, tier: &OccluderTier) -> (Self, HashSet<usize>) {
        let full_tree_portals = portals::generate_portals(tree);
        let full_exterior = visibility::find_exterior_leaves(tree, &full_tree_portals);
        let runtime = Self::build(tree, tier, &full_exterior, full_tree_portals.len());
        (runtime, full_exterior)
    }

    /// Group `tree`'s leaves into cells, one per connected interior pocket
    /// of each of `tier`'s leaves, and generate the runtime portals at the
    /// tier's nodes. `exterior_leaves` is the full tree's flood over its own
    /// portals, `full_tree_portal_count` their number, for the log.
    pub(crate) fn build(
        tree: &BspTree,
        tier: &OccluderTier,
        exterior_leaves: &HashSet<usize>,
        full_tree_portal_count: usize,
    ) -> Self {
        let pockets = portals::interior_pockets(tree, tier, exterior_leaves);
        let cells = CellTree::build(tree, tier, exterior_leaves, &pockets);
        let portals = portals::generate_runtime_portals(tree, tier, &cells);
        let solid = cells
            .tree
            .leaves
            .iter()
            .filter(|cell| cell.is_solid)
            .count();
        log::info!(
            "[Compiler] Runtime cells: {} occluder-tier leaves ({} holding several interior \
             pockets); {} cells ({} interior, {} solid, {} exterior); {} runtime portals; full \
             tree {} leaves, {} portals; cell-size cap {CELL_SIZE_CAP_M} m, least piece \
             {CAP_MIN_PIECE_FRACTION}, open slice over {CAP_MIN_OPEN_SLICE_FRACTION}",
            tier.leaves.len(),
            cells.split_tier_leaf_count(),
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

#[cfg(test)]
mod door_tests;

#[cfg(test)]
mod hint_tests;

#[cfg(test)]
mod cap_tests;

#[cfg(test)]
mod leak_tests;

#[cfg(test)]
mod prism_tests;

#[cfg(test)]
mod pocket_tests;
