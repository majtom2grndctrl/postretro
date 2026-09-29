//! The baked residency relation: per camera cell `c`, each cell `x` with
//! `lead(c, x)`, the smallest lead `L` at which `x` joins
//!
//! `M(c, L) = W(c, L) ∪ ⋃_{c' ∈ W(c, L)} Dil(PVS(c'))`
//!
//! `W(c, L)` is `c` plus every cell within untruncated hub-metric distance
//! `L`; `PVS` is the sampled set from `pvs_sampling`; `Dil(S)` adds every
//! one-hop portal neighbour of a cell in `S`. Pinned clusters are not baked:
//! the runtime adds them from id 49.
//! See: context/plans/in-progress/spatial-residency--lightmap-cell-blocks/index.md

use postretro_level_format::cell_residency_set::{CellResidencySetSection, ResidencyEntry};
use postretro_level_format::cell_visibility::CELL_VISIBILITY_DISTANCE_FIXED_POINT_SCALE;
use rayon::prelude::*;

use super::portal_distance::{Neighbors, PortalGraphInput};

/// Baked maximum lead, in metres.
pub(crate) const BRIEF_MAX_LEAD_METERS: u32 = 32;

pub(crate) fn meters_fixed(meters: u32) -> u32 {
    meters * CELL_VISIBILITY_DISTANCE_FIXED_POINT_SCALE
}

/// Portal neighbours of every cell id, ascending: the id-15 portals the
/// runtime walk traverses, minus self-loops and out-of-range endpoints. On a
/// PRL the loader accepts it agrees with `HubGraph`'s portal adjacency,
/// since the loader rejects the solid endpoints and degenerate polygons
/// `HubGraph` skips. It can reach exterior cells. Those hold no charts, so
/// they cost no block bytes, only set cells and section entries.
pub(crate) fn portal_neighbours(graph: &PortalGraphInput) -> Vec<Vec<u32>> {
    let cell_count = graph.cells.len();
    let mut neighbours = vec![Vec::new(); cell_count];
    for portal in &graph.portals {
        let (front, back) = (portal.front as usize, portal.back as usize);
        if front < cell_count && back < cell_count && front != back {
            neighbours[front].push(portal.back);
            neighbours[back].push(portal.front);
        }
    }
    for cells in &mut neighbours {
        cells.sort_unstable();
        cells.dedup();
    }
    neighbours
}

/// Each sampled PVS grown by one portal hop (PVS to PHS), ascending. A cell
/// with no sampled PVS stays empty: `Dil(∅) = ∅`.
pub(crate) fn dilate_one_hop(pvs: &[Vec<u32>], neighbours: &[Vec<u32>]) -> Vec<Vec<u32>> {
    pvs.par_iter()
        .map(|visible| {
            let mut dilated = visible.clone();
            for &cell in visible {
                dilated.extend_from_slice(&neighbours[cell as usize]);
            }
            dilated.sort_unstable();
            dilated.dedup();
            dilated
        })
        .collect()
}

/// What the lead map reads for every camera cell.
pub(crate) struct LeadSources<'a> {
    /// Untruncated hub-metric partners covering the map's maximum lead.
    pub neighbors: &'a Neighbors,
    /// Per cell id, the set it contributes besides itself: its visible set,
    /// dilated or not.
    pub visible: &'a [Vec<u32>],
    pub cell_count: usize,
}

/// The baked relation: CSR over every cell id (non-camera cells have empty
/// ranges); each camera cell's `(cell, lead)` entries sorted by lead, then
/// cell. Leads are id-46 fixed point.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct LeadMap {
    pub max_lead_fixed: u32,
    /// `cell_count + 1` offsets into `entries`.
    pub offsets: Vec<u32>,
    pub entries: Vec<(u32, u32)>,
}

impl LeadMap {
    /// The id-51 section this map encodes as.
    pub(crate) fn into_section(self) -> CellResidencySetSection {
        CellResidencySetSection {
            max_lead: self.max_lead_fixed,
            offsets: self.offsets,
            entries: self
                .entries
                .into_iter()
                .map(|(cell_id, lead)| ResidencyEntry { lead, cell_id })
                .collect(),
        }
    }
}

/// Every camera cell's lead map up to `max_lead_fixed`. `W`'s origins arrive
/// nearest first, so the first origin to reach a cell sets its lead.
pub(crate) fn build_lead_map(
    sources: &LeadSources<'_>,
    camera_cells: &[u32],
    max_lead_fixed: u32,
) -> LeadMap {
    let per_camera: Vec<Vec<(u32, u32)>> = camera_cells
        .par_iter()
        .map_init(
            || (vec![0u32; sources.cell_count], 0u32),
            |(stamp, generation), &camera| {
                *generation += 1;
                let mut entries = Vec::new();
                let origins = std::iter::once((camera, 0))
                    .chain(sources.neighbors.within_distances(camera, max_lead_fixed));
                for (origin, lead) in origins {
                    let reached = std::iter::once(origin)
                        .chain(sources.visible[origin as usize].iter().copied());
                    for cell in reached {
                        if stamp[cell as usize] != *generation {
                            stamp[cell as usize] = *generation;
                            entries.push((cell, lead));
                        }
                    }
                }
                entries.sort_unstable_by_key(|&(cell, lead)| (lead, cell));
                entries
            },
        )
        .collect();
    let mut ranges = vec![Vec::new(); sources.cell_count];
    for (&camera, entries) in camera_cells.iter().zip(per_camera) {
        ranges[camera as usize] = entries;
    }
    let mut offsets = Vec::with_capacity(sources.cell_count + 1);
    let mut entries = Vec::new();
    offsets.push(0);
    for range in ranges {
        entries.extend(range);
        offsets.push(u32::try_from(entries.len()).expect("entry count fits u32"));
    }
    LeadMap {
        max_lead_fixed,
        offsets,
        entries,
    }
}
