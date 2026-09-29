//! The brief's mandatory set, in the shape its baked cell residency section
//! would store: per camera cell `c`, each cell `x` with `lead(c, x)`, the
//! smallest lead `L` at which `x` joins
//!
//! `M(c, L) = W(c, L) ∪ ⋃_{c' ∈ W(c, L)} Dil(PVS(c')) ∪ Pinned`
//!
//! `W(c, L)` is `c` plus every cell within untruncated hub-metric distance
//! `L`; `PVS` is the sampled set from `pvs_sampling`; `Dil(S)` adds every
//! one-hop portal neighbour of a cell in `S`; `Pinned` is the cells of the
//! clusters id 49 flags pinned. There is no camera-cluster term. The lead map
//! omits `Pinned`, as the brief's wire format does: pins come from id 49.
//! See: context/plans/drafts/spatial-residency--lightmap-cell-blocks/index.md

use postretro_level_format::cell_visibility::CELL_VISIBILITY_DISTANCE_FIXED_POINT_SCALE;
use rayon::prelude::*;

use super::DryRunInput;
use super::mandatory::Neighbors;
use super::portal_distance::PortalGraphInput;

/// Baked maximum lead: the brief's lean.
pub(crate) const BRIEF_MAX_LEAD_METERS: u32 = 32;

pub(crate) fn meters_fixed(meters: u32) -> u32 {
    meters * CELL_VISIBILITY_DISTANCE_FIXED_POINT_SCALE
}

/// Whether each visible set grows by one portal hop (PVS to PHS).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Dilation {
    OneHop,
    None,
}

impl Dilation {
    pub(crate) const ALL: [Dilation; 2] = [Dilation::OneHop, Dilation::None];

    pub(crate) fn label(self) -> &'static str {
        match self {
            Dilation::OneHop => "dilated",
            Dilation::None => "undilated",
        }
    }
}

/// Portal neighbours of every cell id, ascending: the id-15 portals the
/// runtime walk traverses, minus self-loops and out-of-range endpoints.
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

/// Per cell id, the set `W` contributes for that cell besides itself: its
/// sampled PVS, dilated by one portal hop when asked. Ascending. A cell with
/// no sampled PVS contributes nothing, so `Dil(∅) = ∅`.
pub(crate) fn visible_sources(
    pvs: &[Vec<u32>],
    neighbours: &[Vec<u32>],
    dilation: Dilation,
) -> Vec<Vec<u32>> {
    match dilation {
        Dilation::None => pvs.to_vec(),
        Dilation::OneHop => pvs
            .par_iter()
            .map(|visible| {
                let mut dilated = visible.clone();
                for &cell in visible {
                    dilated.extend_from_slice(&neighbours[cell as usize]);
                }
                dilated.sort_unstable();
                dilated.dedup();
                dilated
            })
            .collect(),
    }
}

/// Cells of every cluster id 49 flags pinned, ascending.
pub(crate) fn pinned_cells(input: &DryRunInput) -> Vec<u32> {
    let members = input.cluster_members();
    let mut cells: Vec<u32> = input
        .pinned_clusters
        .iter()
        .flat_map(|&cluster| members[cluster as usize].iter().copied())
        .collect();
    cells.sort_unstable();
    cells.dedup();
    cells
}

/// What the formula reads, shared by the lead map and direct evaluation.
pub(crate) struct BriefSetSources<'a> {
    /// Untruncated hub-metric partners covering `BRIEF_MAX_LEAD_METERS`.
    pub neighbors: &'a Neighbors,
    /// `visible_sources` output for one dilation.
    pub visible: &'a [Vec<u32>],
    /// `pinned_cells` output.
    pub pinned: &'a [u32],
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
    pub(crate) fn entries_of(&self, camera: u32) -> &[(u32, u32)] {
        let start = self.offsets[camera as usize] as usize;
        let end = self.offsets[camera as usize + 1] as usize;
        &self.entries[start..end]
    }

    pub(crate) fn lead_of(&self, camera: u32, cell: u32) -> Option<u32> {
        self.entries_of(camera)
            .iter()
            .find(|&&(entry, _)| entry == cell)
            .map(|&(_, lead)| lead)
    }

    /// `M(c, L)` read from the map, pins added, ascending.
    pub(crate) fn mandatory(&self, camera: u32, lead_fixed: u32, pinned: &[u32]) -> Vec<u32> {
        let mut set: Vec<u32> = self
            .entries_of(camera)
            .iter()
            .take_while(|&&(_, lead)| lead <= lead_fixed)
            .map(|&(cell, _)| cell)
            .chain(pinned.iter().copied())
            .collect();
        set.sort_unstable();
        set.dedup();
        set
    }

    /// Prefetch band at `lead_fixed`: entries with `lead_fixed < lead <=
    /// max`, in lead order. Pinned cells are always mandatory, never band.
    pub(crate) fn band(&self, camera: u32, lead_fixed: u32, pinned: &[u32]) -> Vec<u32> {
        self.entries_of(camera)
            .iter()
            .filter(|&&(cell, lead)| lead > lead_fixed && pinned.binary_search(&cell).is_err())
            .map(|&(cell, _)| cell)
            .collect()
    }
}

/// Every camera cell's lead map up to `max_lead_fixed`. `W`'s origins arrive
/// nearest first, so the first origin to reach a cell sets its lead.
pub(crate) fn build_lead_map(
    sources: &BriefSetSources<'_>,
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

/// `M(c, L)` straight from the formula, pins included, ascending.
pub(crate) fn direct_set(sources: &BriefSetSources<'_>, camera: u32, lead_fixed: u32) -> Vec<u32> {
    let mut set = sources.pinned.to_vec();
    for origin in std::iter::once(camera).chain(sources.neighbors.within(camera, lead_fixed)) {
        set.push(origin);
        set.extend_from_slice(&sources.visible[origin as usize]);
    }
    set.sort_unstable();
    set.dedup();
    set
}

/// Camera cells whose map-read `M(c, L)` differs from direct evaluation, at
/// each of `leads_fixed`; returns `(checked, mismatched)`.
pub(crate) fn check_against_direct(
    map: &LeadMap,
    sources: &BriefSetSources<'_>,
    camera_cells: &[u32],
    leads_fixed: &[u32],
) -> (usize, usize) {
    let mismatched = camera_cells
        .par_iter()
        .map(|&camera| {
            leads_fixed
                .iter()
                .filter(|&&lead| {
                    map.mandatory(camera, lead, sources.pinned) != direct_set(sources, camera, lead)
                })
                .count()
        })
        .sum();
    (camera_cells.len() * leads_fixed.len(), mismatched)
}
