//! The worst mandatory SH demand of any camera cell, for id 49's bound tail.
//! See: context/lib/build_pipeline.md §PRL section IDs (SH mandatory bound)

use std::time::Instant;

use postretro_level_format::SectionId;
use postretro_level_format::cell_residency_set::{CellResidencySetSection, DEFAULT_LEAD};
use postretro_level_format::cells::CellsSection;
use postretro_level_format::cluster_directory::{
    CLUSTER_HINT_FLAG_PINNED, ClusterDirectorySection, ClusterResourceDomain,
    ShMandatoryBoundEntry, ShMandatoryBoundFamily, WHOLE_MAP_WORST_CELL,
};
use postretro_level_format::sh_pool_cost::ShDenseSlotTextures;

use super::FinalizedShEmissionView;
use crate::cell_residency_bake::camera_cells;

mod report;
#[cfg(test)]
mod tests;

pub(crate) use report::ShMandatoryReport;

/// One list per cluster, flattened so a cell's walk reads each cluster's
/// members from one contiguous slice.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct ClusterLists {
    offsets: Vec<usize>,
    items: Vec<u32>,
}

impl Default for ClusterLists {
    fn default() -> Self {
        Self {
            offsets: vec![0],
            items: Vec::new(),
        }
    }
}

impl ClusterLists {
    /// Append the next cluster's list.
    pub(super) fn push(&mut self, items: impl IntoIterator<Item = u32>) {
        self.items.extend(items);
        self.offsets.push(self.items.len());
    }

    pub(super) fn of(&self, cluster: u32) -> &[u32] {
        let cluster = cluster as usize;
        &self.items[self.offsets[cluster]..self.offsets[cluster + 1]]
    }

    fn cluster_count(&self) -> usize {
        self.offsets.len() - 1
    }
}

/// One sparse pool family: which rows each cluster names, and what a row
/// costs in that pool's two capacities.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct SparseFamilyDemand {
    /// This family's row in the id-49 resource table.
    pub(super) resource_index: u32,
    pub(super) cluster_rows: ClusterLists,
    /// `(entries, tile_f16)` of each row, indexed by row.
    pub(super) row_units: Vec<(u32, u32)>,
}

/// What each cluster stores, in the pools' own units: physical dense slots
/// and sparse units, never source bytes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct ClusterStorageDemand {
    pub(super) cluster_nodes: ClusterLists,
    /// Stored tiles of each dense node, indexed by node.
    pub(super) node_slots: Vec<u32>,
    /// Ascending by `resource_index`.
    pub(super) families: Vec<SparseFamilyDemand>,
}

/// The bound's two destinations: id 49's tail and the Build Summary.
pub(super) struct ShMandatoryBound {
    pub(super) entries: Vec<ShMandatoryBoundEntry>,
    pub(super) report: ShMandatoryReport,
}

pub(super) struct ShMandatoryBoundInputs<'a> {
    pub(super) directory: &'a ClusterDirectorySection,
    pub(super) cells: &'a CellsSection,
    pub(super) residency: Option<&'a CellResidencySetSection>,
    pub(super) emission: FinalizedShEmissionView<'a>,
    /// `None` when no id 50 was written: no streamed pool exists to size.
    pub(super) demand: Option<&'a ClusterStorageDemand>,
}

/// Compute, price and report a level's bound. Runs on every build, cache hits
/// included, because pack is uncached.
pub(super) fn resolve_sh_mandatory_bound(
    inputs: ShMandatoryBoundInputs<'_>,
) -> anyhow::Result<ShMandatoryBound> {
    let ShMandatoryBoundInputs {
        directory,
        cells,
        residency,
        emission,
        demand,
    } = inputs;
    let Some(demand) = demand else {
        return Ok(ShMandatoryBound {
            entries: Vec::new(),
            report: ShMandatoryReport::default(),
        });
    };
    let started = Instant::now();
    let entries = sh_mandatory_bound(directory, cells, residency, demand)?;
    let elapsed = started.elapsed();
    let textures = ShDenseSlotTextures {
        indirect_format: emission.octahedral.irradiance_format,
        direct_format: emission.direct.map(|direct| direct.irradiance_format),
        direct_compose: emission.direct_delta.is_some() || emission.animated_direct_delta.is_some(),
        animated_direct_compose: emission.animated_direct_delta.is_some(),
    };
    let report = ShMandatoryReport::price(&entries, textures, cells, elapsed)?;
    report.log();
    Ok(ShMandatoryBound { entries, report })
}

/// Worst mandatory demand over every camera cell, with pins, each stored node
/// and row counted once however many clusters share it.
///
/// A level with no id 34 has nothing to bound. A level whose camera cells have
/// no usable mandatory sets stores the whole map at lead 0.
pub(super) fn sh_mandatory_bound(
    directory: &ClusterDirectorySection,
    cells: &CellsSection,
    residency: Option<&CellResidencySetSection>,
    demand: &ClusterStorageDemand,
) -> anyhow::Result<Vec<ShMandatoryBoundEntry>> {
    if directory.clusters.is_empty() || !names_dense_probes(directory) {
        return Ok(Vec::new());
    }
    anyhow::ensure!(
        demand.cluster_nodes.cluster_count() == directory.clusters.len(),
        "SH mandatory bound covers {} clusters, id 49 has {}",
        demand.cluster_nodes.cluster_count(),
        directory.clusters.len(),
    );
    let camera_cells = camera_cells(cells);
    let residency = residency.filter(|residency| {
        residency.camera_cell_count() == cells.cells.len() && !camera_cells.is_empty()
    });
    let Some(residency) = residency else {
        return whole_map_bound(demand).map(|entry| vec![entry]);
    };
    let cell_to_cluster = cell_to_cluster(directory)?;
    let leads = stored_leads(residency.max_lead);
    let mut walk = UnionWalk::new(demand);

    // Every camera cell shares the pins, so they are walked once and stamped
    // past any cell's stamp: no per-cell work, and no cell counts them twice.
    walk.stamp = PINNED_STAMP;
    for cluster in pinned_clusters(directory) {
        anyhow::ensure!(
            (cluster as usize) < directory.clusters.len(),
            "SH mandatory bound: pin names cluster {cluster} outside id 49"
        );
        walk.add_cluster(cluster);
    }
    let pinned = walk.totals.clone();
    walk.stamp = 0;

    let mut worst: Vec<WorstCell> = leads
        .iter()
        .map(|&lead| WorstCell::new(lead, demand))
        .collect();
    for &cell in &camera_cells {
        walk.restart(&pinned);
        // Entries ascend by lead, so one walk serves every stored lead: each
        // lead's union is the running union when the walk passes that lead.
        let mut next = 0;
        for entry in residency.entries_for(cell as usize) {
            while next < worst.len() && entry.lead > worst[next].lead {
                worst[next].offer(cell, &walk.totals);
                next += 1;
            }
            if next == worst.len() {
                break;
            }
            let cluster = *cell_to_cluster.get(entry.cell_id as usize).ok_or_else(|| {
                anyhow::anyhow!(
                    "SH mandatory bound: id 51 names cell {} outside id 49",
                    entry.cell_id
                )
            })?;
            walk.add_cluster(cluster);
        }
        for worst in &mut worst[next..] {
            worst.offer(cell, &walk.totals);
        }
    }
    worst
        .into_iter()
        .map(|worst| worst.into_entry(demand))
        .collect()
}

/// The two stored leads: default and baked maximum, or one when the maximum
/// does not exceed the default.
fn stored_leads(max_lead: u32) -> Vec<u32> {
    if max_lead > DEFAULT_LEAD {
        vec![DEFAULT_LEAD, max_lead]
    } else {
        vec![max_lead]
    }
}

/// The unique union of every cluster, which no sum of per-cluster figures can
/// give: clusters share nodes and rows.
fn whole_map_bound(demand: &ClusterStorageDemand) -> anyhow::Result<ShMandatoryBoundEntry> {
    let mut walk = UnionWalk::new(demand);
    walk.restart(&UnionTotals::new(demand));
    for cluster in 0..u32::try_from(demand.cluster_nodes.cluster_count())? {
        walk.add_cluster(cluster);
    }
    let mut worst = WorstCell::new(0, demand);
    worst.offer(WHOLE_MAP_WORST_CELL, &walk.totals);
    worst.into_entry(demand)
}

fn names_dense_probes(directory: &ClusterDirectorySection) -> bool {
    directory.resources.iter().any(|resource| {
        resource.section_id == SectionId::OctahedralShVolume as u32
            && resource.domain == ClusterResourceDomain::DenseProbe
    })
}

fn pinned_clusters(directory: &ClusterDirectorySection) -> impl Iterator<Item = u32> + '_ {
    directory
        .cluster_hints
        .iter()
        .filter(|hint| hint.flags & CLUSTER_HINT_FLAG_PINNED != 0)
        .map(|hint| hint.cluster_id)
}

fn cell_to_cluster(directory: &ClusterDirectorySection) -> anyhow::Result<Vec<u32>> {
    let mut map = vec![u32::MAX; directory.runtime_cell_count as usize];
    for (cluster_id, cluster) in directory.clusters.iter().enumerate() {
        let start = cluster.member_start as usize;
        let members = directory
            .members
            .get(start..start + cluster.member_count as usize)
            .ok_or_else(|| {
                anyhow::anyhow!("SH mandatory bound: cluster {cluster_id} members exceed id 49")
            })?;
        for &cell in members {
            *map.get_mut(cell as usize).ok_or_else(|| {
                anyhow::anyhow!("SH mandatory bound: cluster {cluster_id} names cell {cell}")
            })? = u32::try_from(cluster_id)?;
        }
    }
    anyhow::ensure!(
        !map.contains(&u32::MAX),
        "SH mandatory bound: id 49 leaves a cell in no cluster"
    );
    Ok(map)
}

/// A camera cell's running demand: dense slots, then each family's
/// `(entries, tile_f16)` in [`ClusterStorageDemand::families`] order.
#[derive(Debug, Clone, PartialEq, Eq)]
struct UnionTotals {
    dense_slots: u64,
    families: Vec<(u64, u64)>,
}

impl UnionTotals {
    fn new(demand: &ClusterStorageDemand) -> Self {
        Self {
            dense_slots: 0,
            families: vec![(0, 0); demand.families.len()],
        }
    }
}

/// Above every per-cell stamp, so stamped-as-pinned reads as already counted.
const PINNED_STAMP: u32 = u32::MAX;

/// A unique union built with stamp arrays: membership is one compare per
/// node or row, and starting the next cell clears nothing.
struct UnionWalk<'a> {
    demand: &'a ClusterStorageDemand,
    stamp: u32,
    cluster_stamps: Vec<u32>,
    node_stamps: Vec<u32>,
    row_stamps: Vec<Vec<u32>>,
    totals: UnionTotals,
}

impl<'a> UnionWalk<'a> {
    fn new(demand: &'a ClusterStorageDemand) -> Self {
        Self {
            demand,
            stamp: 0,
            cluster_stamps: vec![0; demand.cluster_nodes.cluster_count()],
            node_stamps: vec![0; demand.node_slots.len()],
            row_stamps: demand
                .families
                .iter()
                .map(|family| vec![0; family.row_units.len()])
                .collect(),
            totals: UnionTotals::new(demand),
        }
    }

    fn restart(&mut self, base: &UnionTotals) {
        self.stamp += 1;
        self.totals.clone_from(base);
    }

    fn add_cluster(&mut self, cluster: u32) {
        let stamp = self.stamp;
        let seen = &mut self.cluster_stamps[cluster as usize];
        if *seen >= stamp {
            return;
        }
        *seen = stamp;
        for &node in self.demand.cluster_nodes.of(cluster) {
            let seen = &mut self.node_stamps[node as usize];
            if *seen < stamp {
                *seen = stamp;
                self.totals.dense_slots += u64::from(self.demand.node_slots[node as usize]);
            }
        }
        for (index, family) in self.demand.families.iter().enumerate() {
            let stamps = &mut self.row_stamps[index];
            let totals = &mut self.totals.families[index];
            for &row in family.cluster_rows.of(cluster) {
                let seen = &mut stamps[row as usize];
                if *seen < stamp {
                    *seen = stamp;
                    let (entries, tile_f16) = family.row_units[row as usize];
                    totals.0 += u64::from(entries);
                    totals.1 += u64::from(tile_f16);
                }
            }
        }
    }
}

/// The worst demand seen at one lead.
///
/// Each figure keeps its own maximum, across different cells if need be. The
/// dense pool and every sparse pool are separate allocations, each sized from
/// its own figure, so each must hold whichever cell is worst for it. Taking
/// one cell's vector instead would undersize a family whenever its worst cell
/// is not the dense worst. Independent maxima can overstate the total for any
/// single cell, but they can never understate a pool.
#[derive(Debug)]
struct WorstCell {
    lead: u32,
    /// The cell of the dense worst. Ties keep the lowest cell.
    cell: Option<u32>,
    totals: UnionTotals,
}

impl WorstCell {
    fn new(lead: u32, demand: &ClusterStorageDemand) -> Self {
        Self {
            lead,
            cell: None,
            totals: UnionTotals::new(demand),
        }
    }

    fn offer(&mut self, cell: u32, candidate: &UnionTotals) {
        if self.cell.is_none() || candidate.dense_slots > self.totals.dense_slots {
            self.cell = Some(cell);
            self.totals.dense_slots = candidate.dense_slots;
        }
        for (worst, candidate) in self.totals.families.iter_mut().zip(&candidate.families) {
            worst.0 = worst.0.max(candidate.0);
            worst.1 = worst.1.max(candidate.1);
        }
    }

    fn into_entry(self, demand: &ClusterStorageDemand) -> anyhow::Result<ShMandatoryBoundEntry> {
        let units = |value: u64, name: &str| {
            u32::try_from(value)
                .map_err(|_| anyhow::anyhow!("SH mandatory bound: {value} {name} exceed u32"))
        };
        Ok(ShMandatoryBoundEntry {
            lead: self.lead,
            worst_dense_slots: units(self.totals.dense_slots, "dense slots")?,
            worst_cell: self
                .cell
                .ok_or_else(|| anyhow::anyhow!("SH mandatory bound saw no camera cell"))?,
            families: demand
                .families
                .iter()
                .zip(self.totals.families)
                .map(|(family, (entries, tile_f16))| {
                    Ok(ShMandatoryBoundFamily {
                        resource_index: family.resource_index,
                        worst_entries: units(entries, "sparse entries")?,
                        worst_tile_f16: units(tile_f16, "sparse f16 values")?,
                    })
                })
                .collect::<anyhow::Result<_>>()?,
        })
    }
}
