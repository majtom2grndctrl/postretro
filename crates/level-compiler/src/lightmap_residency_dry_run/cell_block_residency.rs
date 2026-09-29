//! Cell-block residency over the sampled visible sets: each lead's summed
//! block bytes, the static pool lower bound (every camera cell's M(c) packed
//! from scratch into 2048² layers), and the fragmentation walks at one lead.

use rayon::prelude::*;

use super::block_pool_sim::{
    SIM_SEED, SIM_STEPS, SimInputs, WalkResult, run_walks, shelf_layers_from_scratch,
};
use super::cell_blocks::{BlockDims, POOL_LAYER_EDGE};
use super::mandatory::{Granularity, MandatoryContext};
use super::visible_set::{MOVEMENT_LEADS_METERS, VisibleSetInputs, lead_reach};
use crate::lightmap_bake::MaxRects;

/// Lead the fragmentation walks follow.
pub(crate) const SIM_LEAD_METERS: u32 = 16;

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct BlockLeadResult {
    pub lead_meters: u32,
    /// Summed block bytes of each camera cell's cell-granular M(c), parallel
    /// to the camera cells.
    pub bytes: Vec<u64>,
    /// 2048² layers each M(c) needs packed from scratch with MaxRects.
    pub static_layers: Vec<u32>,
    /// Blocks in M(c) too large for any pool layer, summed over camera cells.
    pub unplaceable_blocks: usize,
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct CellBlockResidency {
    pub leads: Vec<BlockLeadResult>,
    /// 2048² layers each M(c) at `SIM_LEAD_METERS` needs packed from scratch
    /// with the shelf allocator the walks use.
    pub shelf_static_layers: Vec<u32>,
    pub walks: Vec<WalkResult>,
}

pub(crate) fn run_cell_block_residency(
    inputs: &VisibleSetInputs<'_>,
    pvs: &[Vec<u32>],
) -> CellBlockResidency {
    let blocks = inputs.cell_blocks;
    let mut context = MandatoryContext::new(inputs.input);
    let mut leads = Vec::new();
    let mut sim_sets = Vec::new();
    let mut sim_static = Vec::new();
    for lead in MOVEMENT_LEADS_METERS {
        let sets: Vec<Vec<u32>> = inputs
            .camera_cells
            .iter()
            .map(|&camera| {
                let (reached, _) = lead_reach(camera, lead, inputs.neighbors, pvs);
                context.set_from_reached(camera, &reached, Granularity::Cell)
            })
            .collect();
        let static_fit: Vec<(u32, usize)> = sets
            .par_iter()
            .map(|set| static_maxrects_layers(blocks.set_dims(set).map(|(_, d)| d)))
            .collect();
        leads.push(BlockLeadResult {
            lead_meters: lead,
            bytes: sets.iter().map(|set| blocks.set_bytes(set)).collect(),
            static_layers: static_fit.iter().map(|&(layers, _)| layers).collect(),
            unplaceable_blocks: static_fit.iter().map(|&(_, over)| over).sum(),
        });
        if lead == SIM_LEAD_METERS {
            sim_sets = sets;
            sim_static = leads.last().map_or(Vec::new(), |l| l.static_layers.clone());
        }
    }
    let shelf_static_layers = sim_sets
        .par_iter()
        .map(|set| shelf_layers_from_scratch(blocks, set))
        .collect();
    let walks = run_walks(&SimInputs {
        blocks,
        camera_cells: inputs.camera_cells,
        sets: &sim_sets,
        static_layers: &sim_static,
        graph: inputs.graph,
        static_worst_layers: sim_static.iter().copied().max().unwrap_or(0),
        steps: SIM_STEPS,
        seed: SIM_SEED,
    });
    CellBlockResidency {
        leads,
        shelf_static_layers,
        walks,
    }
}

/// Layers `blocks` need packed first-fit decreasing by area into
/// `POOL_LAYER_EDGE²` MaxRects layers, and how many blocks cannot fit a layer
/// at all (left out of the count).
pub(crate) fn static_maxrects_layers(blocks: impl Iterator<Item = BlockDims>) -> (u32, usize) {
    let mut order: Vec<BlockDims> = blocks.collect();
    order.sort_by(|a, b| {
        b.area()
            .cmp(&a.area())
            .then(b.height.cmp(&a.height))
            .then(b.width.cmp(&a.width))
    });
    let mut layers: Vec<MaxRects> = Vec::new();
    let mut unplaceable = 0;
    for dims in order {
        if !dims.fits_pool_layer() {
            unplaceable += 1;
            continue;
        }
        let placed = layers
            .iter_mut()
            .any(|layer| layer.insert(dims.width, dims.height).is_some());
        if !placed {
            let mut layer = MaxRects::new(POOL_LAYER_EDGE, POOL_LAYER_EDGE);
            layer
                .insert(dims.width, dims.height)
                .expect("a block no larger than a layer fits an empty one");
            layers.push(layer);
        }
    }
    (layers.len() as u32, unplaceable)
}
