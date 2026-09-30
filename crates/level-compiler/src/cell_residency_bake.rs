//! CellResidencySet (id 51) bake: each camera cell's lightmap residency set.
//!
//! Samples the runtime portal walk from a lattice of eye points in every
//! camera cell, over the visibility world the runtime loader builds from the
//! same Cells, Portals and CellLocator sections; dilates each sampled set by
//! one portal hop; and records, per camera cell, every cell within portal-path
//! lead `MAX_LEAD_METERS` plus each such cell's dilated set, tagged with
//! the smallest lead that makes it mandatory. Pins are not baked (id 49 carries
//! them). A level whose portals the loader would reject has no meaningful set:
//! no section is emitted and the runtime runs lightmaps all-resident.
//! See: context/lib/build_pipeline.md §PRL section IDs, §Build Cache

pub(crate) mod lead_map;
pub(crate) mod portal_distance;
pub(crate) mod pvs_sampling;

#[cfg(test)]
pub(crate) mod test_fixtures;
#[cfg(test)]
mod tests;

use postretro_level_format::cell_locator::CellLocatorSection;
use postretro_level_format::cell_residency_set::CellResidencySetSection;
use postretro_level_format::cells::CellsSection;
use postretro_level_format::portals::PortalsSection;
use postretro_level_loader::LevelWorld;

use crate::bake_control::BakeControl;
use crate::cache::{CacheKey, StageCache};
use lead_map::{
    LeadSources, MAX_LEAD_METERS, build_lead_map, dilate_one_hop, meters_fixed, portal_neighbours,
};
use portal_distance::{Neighbors, portal_graph_from_sections, recompute_pairs};
use pvs_sampling::{SamplingStats, sample_pvs};

pub const CELL_RESIDENCY_SET_STAGE_ID: &str = "cell_residency_set";
/// Bump when sampling, dilation, the lead metric, the maximum lead, or the
/// section encoding changes.
pub const CELL_RESIDENCY_SET_STAGE_VERSION: u32 = 1;

/// Camera cells: every cell the camera can stand in (not solid, not
/// exterior), charted or not. Ascending.
pub(crate) fn camera_cells(cells: &CellsSection) -> Vec<u32> {
    cells
        .cells
        .iter()
        .enumerate()
        .filter(|(_, record)| !record.is_solid() && !record.is_exterior())
        .map(|(cell, _)| cell as u32)
        .collect()
}

/// Bake or load the residency set; `None` when the level has no usable
/// portals. The world is always built first, so a cache hit never outlives a
/// loader that would now reject the portals.
pub fn cell_residency_set_bake_cached(
    cells: &CellsSection,
    portals: &PortalsSection,
    locator: &CellLocatorSection,
    cache: Option<&StageCache>,
    control: &BakeControl,
) -> anyhow::Result<Option<CellResidencySetSection>> {
    let Some(world) = residency_world(cells, portals, locator)? else {
        control.publish_total(0);
        return Ok(None);
    };
    let cameras = camera_cells(cells);
    control.publish_total(cameras.len());

    let key = cache.map(|_| cell_residency_set_cache_key(cells, portals, locator));
    if let (Some(cache), Some(key)) = (cache, key.as_ref()) {
        match cache.get(key) {
            Some(data) => match CellResidencySetSection::from_bytes(&data, cells.cells.len()) {
                Ok(section) => {
                    log::info!("[cache] cell_residency_set hit");
                    control.governor().checkpoint();
                    control.advance(cameras.len());
                    return Ok(Some(section));
                }
                Err(error) => {
                    log::warn!("[cache] corrupt cell_residency_set entry, re-baking: {error}");
                    log::info!("[cache] cell_residency_set miss");
                }
            },
            None => log::info!("[cache] cell_residency_set miss"),
        }
    }

    let section = bake(&world, cells, portals, &cameras, control);
    if let (Some(cache), Some(key)) = (cache, key.as_ref()) {
        cache.put(key, &section.to_bytes());
    }
    Ok(Some(section))
}

/// The runtime visibility world, or `None` when the loader would run the
/// level without portals.
fn residency_world(
    cells: &CellsSection,
    portals: &PortalsSection,
    locator: &CellLocatorSection,
) -> anyhow::Result<Option<LevelWorld>> {
    if portals.portals.is_empty() {
        log::info!(
            "[Compiler] CellResidencySet: level has no portals; omitting id 51, lightmaps run all-resident"
        );
        return Ok(None);
    }
    let world = LevelWorld::visibility_only_from_sections(cells, portals, locator).map_err(
        |error| anyhow::anyhow!("CellResidencySet: the runtime loader would reject this level's cells, portals or locator: {error}"),
    )?;
    if !world.has_portals {
        log::warn!(
            "[Compiler] CellResidencySet: the runtime loader rejects this level's portals; omitting id 51, lightmaps run all-resident"
        );
        return Ok(None);
    }
    Ok(Some(world))
}

/// The uncached bake over a world that has portals.
fn bake(
    world: &LevelWorld,
    cells: &CellsSection,
    portals: &PortalsSection,
    cameras: &[u32],
    control: &BakeControl,
) -> CellResidencySetSection {
    let started = std::time::Instant::now();
    let cell_count = cells.cells.len();
    let max_lead = meters_fixed(MAX_LEAD_METERS);
    let graph = portal_graph_from_sections(cells, portals);
    let neighbors = Neighbors::from_pairs(cell_count, &recompute_pairs(&graph, max_lead));
    let pvs = sample_pvs(world, cameras, control);
    log_sampling(&pvs.stats);
    let visible = dilate_one_hop(&pvs.dense, &portal_neighbours(&graph));
    let map = build_lead_map(
        &LeadSources {
            neighbors: &neighbors,
            visible: &visible,
            cell_count,
        },
        cameras,
        max_lead,
    );
    let section = map.into_section();
    log::info!(
        "[Compiler] CellResidencySet: {} camera cells, {} entries (max {} per camera), {} bytes, max lead {MAX_LEAD_METERS} m, {:.3} s",
        cameras.len(),
        section.entries.len(),
        (0..cell_count)
            .map(|camera| section.entries_for(camera).len())
            .max()
            .unwrap_or(0),
        section.byte_len(),
        started.elapsed().as_secs_f64(),
    );
    section
}

fn log_sampling(stats: &SamplingStats) {
    log::info!(
        "[Compiler] CellResidencySet sampling: {} eye candidates ({} in cell, {} inset, {} rejected; missed into solid {}, exterior {}, other cell {}), {} camera cells without an eye point, {} walks ({} step-limited, {} frustum fallback)",
        stats.candidates,
        stats.in_cell,
        stats.inset,
        stats.rejected,
        stats.missed_into_solid,
        stats.missed_into_exterior,
        stats.missed_into_other_cell,
        stats.cells_without_eye,
        stats.walks,
        stats.step_limit_walks,
        stats.frustum_all_walks,
    );
}

/// Whole-section key over every input the bake reads: the encoded Cells,
/// Portals and CellLocator sections, the maximum lead, and the runtime portal
/// walk's epoch (the sampled sets are that walk's output). Lights, charts and
/// hints never participate, so a lighting-only edit hits.
pub(crate) fn cell_residency_set_cache_key(
    cells: &CellsSection,
    portals: &PortalsSection,
    locator: &CellLocatorSection,
) -> CacheKey {
    let mut hasher = blake3::Hasher::new();
    for bytes in [cells.to_bytes(), portals.to_bytes(), locator.to_bytes()] {
        hasher.update(&(bytes.len() as u64).to_le_bytes());
        hasher.update(&bytes);
    }
    hasher.update(&meters_fixed(MAX_LEAD_METERS).to_le_bytes());
    hasher.update(&postretro_visibility::PORTAL_WALK_EPOCH.to_le_bytes());
    CacheKey::new(
        CELL_RESIDENCY_SET_STAGE_ID,
        CELL_RESIDENCY_SET_STAGE_VERSION,
        hasher.finalize().as_bytes(),
    )
}
