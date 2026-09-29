//! Brief-set residency: each dilation's lead map to `BRIEF_MAX_LEAD_METERS`,
//! checked against direct evaluation, costed as cell blocks at each movement
//! lead; the prefetch band at `SIM_LEAD_METERS`; the would-be residency
//! section's size; and the band-aware pool walks over the dilated set.

use rayon::prelude::*;

use super::band_pool_sim::{BandSimInputs, BandWalks, pool_caps, run_band_walks};
use super::block_pool_sim::{SIM_SEED, SIM_STEPS, shelf_layers_from_scratch};
use super::brief_set::{
    BRIEF_MAX_LEAD_METERS, BriefSetSources, Dilation, LeadCheck, LeadMap, build_lead_map,
    check_every_breakpoint, meters_fixed, pinned_cells, portal_neighbours, visible_sources,
};
use super::cell_block_residency::SIM_LEAD_METERS;
use super::mandatory::mandatory_bytes;
use super::render::percentile_desc;
use super::visible_set::{MOVEMENT_LEADS_METERS, VisibleSetInputs};

/// Wire format of the brief's cell residency set: a four-`u32` header,
/// `u32` CSR offsets, and `(cell_id u32, lead u32)` entries.
const SECTION_HEADER_BYTES: u64 = 16;
const SECTION_OFFSET_BYTES: u64 = 4;
const SECTION_ENTRY_BYTES: u64 = 8;

/// One lead's `M(c, L)`, parallel to the camera cells.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct BriefLeadResult {
    pub lead_meters: u32,
    pub set_cells: Vec<usize>,
    /// Summed cell-block bytes (id 22 + id 42 at each block's extent).
    pub block_bytes: Vec<u64>,
    /// Chart texel bytes, id 42 charged as blocks charge it.
    pub texel_exact: Vec<f64>,
    /// Shelf allocator layers packing `M(c, L)` from scratch.
    pub shelf_layers: Vec<u32>,
}

/// The prefetch band at one lead, parallel to the camera cells.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct BandResult {
    pub lead_meters: u32,
    pub cells: Vec<usize>,
    pub block_bytes: Vec<u64>,
    /// `M(c, L)` block bytes plus the band's.
    pub with_mandatory_bytes: Vec<u64>,
}

/// Size of the would-be cell residency section for one lead map, with its
/// CSR over every cell id as the wire format indexes it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct SectionSize {
    pub cell_count: usize,
    pub entries: usize,
    /// Entries naming an exterior cell, which holds no charts.
    pub exterior_entries: usize,
    pub max_entries_per_camera: usize,
}

impl SectionSize {
    pub(crate) fn bytes(&self) -> u64 {
        SECTION_HEADER_BYTES
            + (self.cell_count as u64 + 1) * SECTION_OFFSET_BYTES
            + self.entries as u64 * SECTION_ENTRY_BYTES
    }
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct BriefVariant {
    pub dilation: Dilation,
    pub leads: Vec<BriefLeadResult>,
    pub band: BandResult,
    pub section: SectionSize,
    /// Map-read `M(c, L)` against direct evaluation at every lead.
    pub consistency: LeadCheck,
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct BriefSetResult {
    pub max_lead_meters: u32,
    pub pinned_cells: usize,
    pub variants: Vec<BriefVariant>,
    /// Band-aware walks over the dilated set at `SIM_LEAD_METERS`.
    pub walks: BandWalks,
}

pub(crate) fn run_brief_set(inputs: &VisibleSetInputs<'_>, pvs: &[Vec<u32>]) -> BriefSetResult {
    let max_lead_meters = BRIEF_MAX_LEAD_METERS;
    assert!(
        MOVEMENT_LEADS_METERS
            .iter()
            .all(|&lead| lead <= max_lead_meters),
        "every measured lead lies within the baked maximum"
    );
    let neighbours = portal_neighbours(inputs.graph);
    let pinned = pinned_cells(inputs.input);
    let mut walks = None;
    let variants = Dilation::ALL
        .iter()
        .map(|&dilation| {
            let visible = visible_sources(pvs, &neighbours, dilation);
            let sources = BriefSetSources {
                neighbors: inputs.neighbors,
                visible: &visible,
                pinned: &pinned,
                cell_count: inputs.input.cell_count(),
            };
            let map = build_lead_map(&sources, inputs.camera_cells, meters_fixed(max_lead_meters));
            let consistency = check_every_breakpoint(&map, &sources, inputs.camera_cells);
            let leads: Vec<BriefLeadResult> = MOVEMENT_LEADS_METERS
                .iter()
                .map(|&lead| evaluate_lead(inputs, &map, &pinned, lead))
                .collect();
            let band = evaluate_band(inputs, &map, &pinned, SIM_LEAD_METERS);
            if dilation == Dilation::OneHop {
                let sim_lead = leads
                    .iter()
                    .find(|lead| lead.lead_meters == SIM_LEAD_METERS)
                    .expect("the walks' lead is a movement lead");
                walks = Some(band_walks(inputs, &map, &pinned, sim_lead));
            }
            BriefVariant {
                dilation,
                leads,
                band,
                section: SectionSize {
                    cell_count: inputs.input.cell_count(),
                    entries: map.entries.len(),
                    exterior_entries: map
                        .entries
                        .iter()
                        .filter(|&&(cell, _)| inputs.input.cells[cell as usize].exterior)
                        .count(),
                    max_entries_per_camera: inputs
                        .camera_cells
                        .iter()
                        .map(|&camera| map.entries_of(camera).len())
                        .max()
                        .unwrap_or(0),
                },
                consistency,
            }
        })
        .collect();
    BriefSetResult {
        max_lead_meters,
        pinned_cells: pinned.len(),
        variants,
        walks: walks.expect("Dilation::ALL includes the dilated set"),
    }
}

fn evaluate_lead(
    inputs: &VisibleSetInputs<'_>,
    map: &LeadMap,
    pinned: &[u32],
    lead_meters: u32,
) -> BriefLeadResult {
    let fixed = meters_fixed(lead_meters);
    let per_camera: Vec<(usize, u64, f64, u32)> = inputs
        .camera_cells
        .par_iter()
        .map_init(Vec::new, |layer_stamp, &camera| {
            let set = map.mandatory(camera, fixed, pinned);
            let exact = mandatory_bytes(&set, inputs.footprint, &[], layer_stamp);
            (
                set.len(),
                inputs.cell_blocks.set_bytes(&set),
                exact.texel_exact_charging_mask(),
                shelf_layers_from_scratch(inputs.cell_blocks, &set),
            )
        })
        .collect();
    BriefLeadResult {
        lead_meters,
        set_cells: per_camera.iter().map(|r| r.0).collect(),
        block_bytes: per_camera.iter().map(|r| r.1).collect(),
        texel_exact: per_camera.iter().map(|r| r.2).collect(),
        shelf_layers: per_camera.iter().map(|r| r.3).collect(),
    }
}

fn evaluate_band(
    inputs: &VisibleSetInputs<'_>,
    map: &LeadMap,
    pinned: &[u32],
    lead_meters: u32,
) -> BandResult {
    let fixed = meters_fixed(lead_meters);
    let blocks = inputs.cell_blocks;
    let per_camera: Vec<(usize, u64, u64)> = inputs
        .camera_cells
        .iter()
        .map(|&camera| {
            let band = map.band(camera, fixed, pinned);
            let band_bytes = blocks.set_bytes(&band);
            let mandatory = blocks.set_bytes(&map.mandatory(camera, fixed, pinned));
            (band.len(), band_bytes, band_bytes + mandatory)
        })
        .collect();
    BandResult {
        lead_meters,
        cells: per_camera.iter().map(|r| r.0).collect(),
        block_bytes: per_camera.iter().map(|r| r.1).collect(),
        with_mandatory_bytes: per_camera.iter().map(|r| r.2).collect(),
    }
}

fn band_walks(
    inputs: &VisibleSetInputs<'_>,
    map: &LeadMap,
    pinned: &[u32],
    sim_lead: &BriefLeadResult,
) -> BandWalks {
    let fixed = meters_fixed(sim_lead.lead_meters);
    let mandatory: Vec<Vec<u32>> = inputs
        .camera_cells
        .iter()
        .map(|&camera| map.mandatory(camera, fixed, pinned))
        .collect();
    let band: Vec<Vec<u32>> = inputs
        .camera_cells
        .iter()
        .map(|&camera| map.band(camera, fixed, pinned))
        .collect();
    let mut layers: Vec<(f64, u32)> = sim_lead
        .shelf_layers
        .iter()
        .zip(inputs.camera_cells)
        .map(|(&layers, &cell)| (f64::from(layers), cell))
        .collect();
    layers.sort_by(|a, b| b.0.total_cmp(&a.0).then(a.1.cmp(&b.1)));
    let worst = layers.first().map_or(0, |v| v.0 as u32);
    let p95 = percentile_desc(&layers, 95) as u32;
    run_band_walks(
        &BandSimInputs {
            blocks: inputs.cell_blocks,
            mandatory: &mandatory,
            band: &band,
            mandatory_shelf_layers: &sim_lead.shelf_layers,
        },
        inputs.camera_cells,
        inputs.graph,
        pool_caps(worst, p95),
        SIM_STEPS,
        SIM_SEED,
    )
}
