//! Visible-set mandatory bytes: the no-pop-in bound.
//!
//! The engine has no draw distance, so a portal-path distance cannot bound
//! what the player sees. Instead the mandatory set of camera cell `c` is
//! everything visible from anywhere in the cells a movement lead `L` reaches,
//! plus those cells:
//!
//! `M(c) = cluster(c) ∪ pinned ∪ ⋃_{c' ∈ {c} ∪ within(c, L)} ({c'} ∪ PVS(c'))`
//!
//! `within` is the untruncated hub-metric neighbourhood and `PVS` the sampled
//! set from `pvs_sampling`, so every figure here is a lower bound (per cell
//! volume; see `pvs_sampling`).

use postretro_level_format::cell_visibility::CELL_VISIBILITY_DISTANCE_FIXED_POINT_SCALE;
use postretro_level_loader::LevelWorld;
use rayon::prelude::*;

use super::DryRunInput;
use super::brief_set_residency::{BriefSetResult, run_brief_set};
use super::cell_block_residency::{CellBlockResidency, run_cell_block_residency};
use super::cell_blocks::CellBlocks;
use super::layouts::Layout;
use super::mandatory::{
    CellFootprint, Granularity, MandatoryBytes, MandatoryContext, mandatory_bytes,
};
use super::tiles::{TileLayout, UnitStamp};
use crate::bake_control::BakeControl;
use crate::cell_residency_bake::portal_distance::{HubGraph, Neighbors, PortalGraphInput};
use crate::cell_residency_bake::pvs_sampling::{
    LATTICE_STEPS, SampleDensity, SampledPvs, SamplingStats, sample_pvs,
};

/// Movement leads: sampled PVS alone, then about 1 s and 2 s of sustained
/// movement at 11–15 m/s.
pub(crate) const MOVEMENT_LEADS_METERS: [u32; 3] = [0, 16, 32];

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct LeadResult {
    pub lead_meters: u32,
    /// Camera cells, ascending, with their mandatory bytes.
    pub cells: Vec<(u32, MandatoryBytes)>,
    /// Reached cells with no sampled PVS (not camera cells); they join the
    /// set alone.
    pub reached_without_pvs: usize,
    /// Per camera cell, parallel to `cells`: the same set's resident bytes
    /// under each tile layout.
    pub tile_bytes: Vec<Vec<u64>>,
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct GranularityResult {
    pub granularity: Granularity,
    pub leads: Vec<LeadResult>,
}

/// Per camera cell, the farthest hub-metric distance to a cell it sees.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Sightlines {
    /// Camera cells, ascending, with meters; cells that see only themselves
    /// read 0.
    pub per_cell: Vec<(u32, f64)>,
    /// Sampled-visible cells the hub metric has no path to.
    pub unreachable_visible: usize,
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct VisibleSetResult {
    /// Every lead at `SampleDensity::Dense`, one entry per granularity.
    pub dense: Vec<GranularityResult>,
    /// Lead 0 only at `SampleDensity::Sparse`, for the convergence check.
    pub sparse_lead_zero: Vec<GranularityResult>,
    pub stats: SamplingStats,
    /// Mean sampled PVS size over camera cells, sparse then dense.
    pub mean_pvs: (f64, f64),
    /// Camera cells whose dense PVS is larger than the sparse one.
    pub cells_grown_by_density: usize,
    pub sightlines: Sightlines,
    /// Dense cell-granular sets costed as cell blocks, and the pool walks.
    pub cell_blocks: CellBlockResidency,
    /// The brief's lead-map set over the dense PVS, with and without dilation.
    pub brief_set: BriefSetResult,
    /// Portals the runtime loader would reject (`DryRunInput`).
    pub loader_rejected_portals: usize,
}

pub(crate) struct VisibleSetInputs<'a> {
    pub input: &'a DryRunInput,
    pub world: &'a LevelWorld,
    pub graph: &'a PortalGraphInput,
    /// Untruncated hub-metric partners; must cover the largest lead.
    pub neighbors: &'a Neighbors,
    pub camera_cells: &'a [u32],
    pub footprint: &'a CellFootprint,
    pub layouts: &'a [Layout],
    pub tile_layouts: &'a [TileLayout],
    pub cell_blocks: &'a CellBlocks,
}

impl SampleDensity {
    pub(crate) fn label(self) -> String {
        match self {
            SampleDensity::Sparse => "centroid + 8 inset corners".to_string(),
            SampleDensity::Dense => {
                format!("{LATTICE_STEPS}x{LATTICE_STEPS}x{LATTICE_STEPS} inset lattice")
            }
        }
    }
}

impl SampledPvs {
    pub(crate) fn sets(&self, density: SampleDensity) -> &[Vec<u32>] {
        match density {
            SampleDensity::Sparse => &self.sparse,
            SampleDensity::Dense => &self.dense,
        }
    }
}

pub(crate) fn run_visible_set(inputs: &VisibleSetInputs<'_>) -> VisibleSetResult {
    let pvs = sample_pvs(
        inputs.world,
        inputs.camera_cells,
        &BakeControl::unrestricted(),
    );
    let evaluate = |density, leads: &[u32]| -> Vec<GranularityResult> {
        Granularity::ALL
            .map(|granularity| GranularityResult {
                granularity,
                leads: leads
                    .iter()
                    .map(|&lead| evaluate_lead(inputs, &pvs, density, granularity, lead))
                    .collect(),
            })
            .to_vec()
    };
    let dense = evaluate(SampleDensity::Dense, &MOVEMENT_LEADS_METERS);
    let sparse_lead_zero = evaluate(SampleDensity::Sparse, &[0]);

    let camera_count = inputs.camera_cells.len().max(1) as f64;
    let mean = |sets: &[Vec<u32>]| {
        inputs
            .camera_cells
            .iter()
            .map(|&c| sets[c as usize].len())
            .sum::<usize>() as f64
            / camera_count
    };
    VisibleSetResult {
        dense,
        sparse_lead_zero,
        stats: pvs.stats,
        mean_pvs: (mean(&pvs.sparse), mean(&pvs.dense)),
        cells_grown_by_density: inputs
            .camera_cells
            .iter()
            .filter(|&&c| pvs.dense[c as usize].len() > pvs.sparse[c as usize].len())
            .count(),
        sightlines: sightlines(inputs.graph, inputs.camera_cells, &pvs.dense),
        cell_blocks: run_cell_block_residency(inputs, &pvs.dense),
        brief_set: run_brief_set(inputs, &pvs.dense),
        loader_rejected_portals: inputs.input.loader_rejected_portals,
    }
}

/// The cells camera cell `camera` reaches under lead `lead_meters`: itself,
/// its hub-metric partners within the lead, and each one's sampled PVS.
/// Also returns how many of those origins had no sampled PVS.
pub(crate) fn lead_reach(
    camera: u32,
    lead_meters: u32,
    neighbors: &Neighbors,
    pvs: &[Vec<u32>],
) -> (Vec<u32>, usize) {
    let fixed = lead_meters * CELL_VISIBILITY_DISTANCE_FIXED_POINT_SCALE;
    let mut reached = Vec::new();
    let mut without_pvs = 0;
    for origin in std::iter::once(camera).chain(neighbors.within(camera, fixed)) {
        reached.push(origin);
        let visible = &pvs[origin as usize];
        if visible.is_empty() {
            without_pvs += 1;
        }
        reached.extend_from_slice(visible);
    }
    (reached, without_pvs)
}

fn evaluate_lead(
    inputs: &VisibleSetInputs<'_>,
    pvs: &SampledPvs,
    density: SampleDensity,
    granularity: Granularity,
    lead_meters: u32,
) -> LeadResult {
    let sets = pvs.sets(density);
    let mut context = MandatoryContext::new(inputs.input);
    let mut layer_stamp = Vec::new();
    let mut unit_stamp = UnitStamp::default();
    let mut reached_without_pvs = 0;
    let mut tile_bytes = Vec::with_capacity(inputs.camera_cells.len());
    let cells = inputs
        .camera_cells
        .iter()
        .map(|&camera| {
            let (reached, without_pvs) = lead_reach(camera, lead_meters, inputs.neighbors, sets);
            reached_without_pvs += without_pvs;
            let set = context.set_from_reached(camera, &reached, granularity);
            tile_bytes.push(
                inputs
                    .tile_layouts
                    .iter()
                    .map(|layout| layout.mandatory_bytes(&set, &mut unit_stamp))
                    .collect(),
            );
            (
                camera,
                mandatory_bytes(&set, inputs.footprint, inputs.layouts, &mut layer_stamp),
            )
        })
        .collect();
    LeadResult {
        lead_meters,
        cells,
        reached_without_pvs,
        tile_bytes,
    }
}

fn sightlines(graph: &PortalGraphInput, camera_cells: &[u32], pvs: &[Vec<u32>]) -> Sightlines {
    let hubs = HubGraph::new(graph);
    let per_cell: Vec<(u32, f64, usize)> = camera_cells
        .par_iter()
        .map_init(
            || hubs.scratch(),
            |scratch, &camera| {
                hubs.settle_from(camera, f64::INFINITY, scratch);
                let mut farthest = 0.0f64;
                let mut unreachable = 0;
                for &visible in pvs[camera as usize].iter().filter(|&&v| v != camera) {
                    match hubs.meters_to(scratch, visible) {
                        Some(meters) => farthest = farthest.max(meters),
                        None => unreachable += 1,
                    }
                }
                (camera, farthest, unreachable)
            },
        )
        .collect();
    Sightlines {
        unreachable_visible: per_cell.iter().map(|&(_, _, u)| u).sum(),
        per_cell: per_cell.into_iter().map(|(c, m, _)| (c, m)).collect(),
    }
}
