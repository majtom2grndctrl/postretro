//! The brief's mandatory set as a formula, for checking the baked relation:
//! per camera cell `c`, each cell `x` with `lead(c, x)`, the smallest lead
//! `L` at which `x` joins
//!
//! `M(c, L) = W(c, L) ∪ ⋃_{c' ∈ W(c, L)} Dil(PVS(c')) ∪ Pinned`
//!
//! The lead map itself is the CellResidencySet bake's
//! (`cell_residency_bake::lead_map`); this module adds the undilated variant,
//! the pins the runtime adds from id 49, and direct evaluation of the formula.
//! There is no camera-cluster term. See: context/plans/large-map-spatial-residency.md

use postretro_level_format::cell_residency_set::CellResidencySetSection;
use rayon::prelude::*;

use super::DryRunInput;
use crate::cell_residency_bake::lead_map::{LeadMap, LeadSources, dilate_one_hop};
use crate::cell_residency_bake::portal_distance::Neighbors;

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
        Dilation::OneHop => dilate_one_hop(pvs, neighbours),
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

impl BriefSetSources<'_> {
    fn lead_sources(&self) -> LeadSources<'_> {
        LeadSources {
            neighbors: self.neighbors,
            visible: self.visible,
            cell_count: self.cell_count,
        }
    }
}

impl LeadMap {
    /// The lead map a decoded id-51 section encodes.
    pub(crate) fn from_section(section: &CellResidencySetSection) -> Self {
        Self {
            max_lead_fixed: section.max_lead,
            offsets: section.offsets.clone(),
            entries: section
                .entries
                .iter()
                .map(|entry| (entry.cell_id, entry.lead))
                .collect(),
        }
    }

    pub(crate) fn entries_of(&self, camera: u32) -> &[(u32, u32)] {
        let start = self.offsets[camera as usize] as usize;
        let end = self.offsets[camera as usize + 1] as usize;
        &self.entries[start..end]
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

/// The bake's lead map over these sources; pins are never baked.
pub(crate) fn build_lead_map(
    sources: &BriefSetSources<'_>,
    camera_cells: &[u32],
    max_lead_fixed: u32,
) -> LeadMap {
    crate::cell_residency_bake::lead_map::build_lead_map(
        &sources.lead_sources(),
        camera_cells,
        max_lead_fixed,
    )
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
        .map(|&camera| mismatches(map, sources, camera, leads_fixed))
        .sum();
    (camera_cells.len() * leads_fixed.len(), mismatched)
}

/// Map-read `M(c, L)` against direct evaluation at every lead up to the
/// map's maximum.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct LeadCheck {
    /// `(camera, lead)` pairs compared.
    pub checked: usize,
    pub mismatched: usize,
    /// Distinct lead values among the map's entries.
    pub distinct_leads: usize,
}

/// Both sides are step functions of `L` that change only at one of the
/// camera's own map leads or one of its hub-metric partner distances, so
/// comparing at each of those breakpoints compares every `L` up to the map's
/// maximum.
pub(crate) fn check_every_breakpoint(
    map: &LeadMap,
    sources: &BriefSetSources<'_>,
    camera_cells: &[u32],
) -> LeadCheck {
    let (checked, mismatched) = camera_cells
        .par_iter()
        .map(|&camera| {
            let mut leads: Vec<u32> = map
                .entries_of(camera)
                .iter()
                .map(|&(_, lead)| lead)
                .chain(
                    sources
                        .neighbors
                        .within_distances(camera, map.max_lead_fixed)
                        .map(|(_, distance)| distance),
                )
                .chain(std::iter::once(0))
                .collect();
            leads.sort_unstable();
            leads.dedup();
            (leads.len(), mismatches(map, sources, camera, &leads))
        })
        .reduce(|| (0, 0), |a, b| (a.0 + b.0, a.1 + b.1));
    let mut distinct: Vec<u32> = map.entries.iter().map(|&(_, lead)| lead).collect();
    distinct.sort_unstable();
    distinct.dedup();
    LeadCheck {
        checked,
        mismatched,
        distinct_leads: distinct.len(),
    }
}

fn mismatches(map: &LeadMap, sources: &BriefSetSources<'_>, camera: u32, leads: &[u32]) -> usize {
    leads
        .iter()
        .filter(|&&lead| {
            map.mandatory(camera, lead, sources.pinned) != direct_set(sources, camera, lead)
        })
        .count()
}
