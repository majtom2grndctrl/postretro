use log::Level;
use postretro_level_format::cell_residency_set::CellResidencySetSection;
use postretro_test_log_capture::LogCapture;

use super::test_fixtures::{L_OPEN_CELLS, L_SOLID, l_corridor_sections};
use super::*;
use crate::reporter::StageProgress;

fn bake_uncached(
    cells: &CellsSection,
    portals: &PortalsSection,
    locator: &CellLocatorSection,
) -> Option<CellResidencySetSection> {
    cell_residency_set_bake_cached(cells, portals, locator, None, &BakeControl::unrestricted())
        .expect("the bake succeeds")
}

fn open_cache() -> (tempfile::TempDir, StageCache) {
    let dir = tempfile::tempdir().expect("cache directory");
    let cache = StageCache::new(dir.path()).expect("open stage cache");
    (dir, cache)
}

#[test]
fn bake_covers_every_cell_with_sorted_ranges_and_the_camera_first() {
    let (cells, portals, locator) = l_corridor_sections();
    let section = bake_uncached(&cells, &portals, &locator).expect("usable portals");
    assert_eq!(section.camera_cell_count(), cells.cells.len());
    assert_eq!(section.max_lead, meters_fixed(MAX_LEAD_METERS));
    assert!(
        section.entries_for(L_SOLID as usize).is_empty(),
        "a solid cell is no camera cell"
    );
    for camera in 0..L_OPEN_CELLS {
        let range = section.entries_for(camera as usize);
        assert!(
            range.iter().any(|e| e.cell_id == camera && e.lead == 0),
            "camera {camera} needs itself at lead 0: {range:?}"
        );
        assert!(
            range.windows(2).all(|w| w[0] < w[1]),
            "sorted by (lead, cell), no repeats: {range:?}"
        );
        assert!(range.iter().all(|e| e.lead <= section.max_lead));
    }
    // The format's own validation accepts it.
    let decoded = CellResidencySetSection::from_bytes(&section.to_bytes(), cells.cells.len())
        .expect("the baked section decodes");
    assert_eq!(decoded, section);
}

#[test]
fn lead_grows_the_set_and_the_corner_hides_the_far_leg() {
    let (cells, portals, locator) = l_corridor_sections();
    let section = bake_uncached(&cells, &portals, &locator).expect("usable portals");
    let range = section.entries_for(0);
    let at = |lead_meters: u32| -> Vec<u32> {
        let mut set: Vec<u32> = range
            .iter()
            .take_while(|e| e.lead <= meters_fixed(lead_meters))
            .map(|e| e.cell_id)
            .collect();
        set.sort_unstable();
        set
    };
    // From the start of the X leg the far end of the Z leg is neither in view
    // nor within the maximum lead.
    assert!(!at(MAX_LEAD_METERS).contains(&10), "{range:?}");
    assert!(at(0).len() < at(MAX_LEAD_METERS).len(), "{range:?}");
    // The whole X leg is in view down the corridor at lead 0.
    for cell in 0..5 {
        assert!(at(0).contains(&cell), "{range:?}");
    }
    assert!(range.iter().all(|e| e.cell_id != L_SOLID));
}

#[test]
fn bake_is_deterministic() {
    let (cells, portals, locator) = l_corridor_sections();
    assert_eq!(
        bake_uncached(&cells, &portals, &locator),
        bake_uncached(&cells, &portals, &locator)
    );
}

#[test]
fn a_level_without_portals_emits_no_section() {
    let (mut cells, _, locator) = l_corridor_sections();
    for record in &mut cells.cells {
        record.portal_ref_count = 0;
    }
    cells.portal_refs.clear();
    let portals = PortalsSection {
        vertices: Vec::new(),
        portals: Vec::new(),
    };
    let capture = LogCapture::start();
    assert_eq!(bake_uncached(&cells, &portals, &locator), None);
    capture.assert_logged_once(Level::Info, "CellResidencySet: level has no portals");
}

#[test]
fn a_level_whose_portals_the_loader_rejects_emits_no_section() {
    let (cells, mut portals, locator) = l_corridor_sections();
    // A two-vertex portal: the loader drops the whole portal set.
    portals.portals[2].vertex_count = 2;
    let capture = LogCapture::start();
    assert_eq!(bake_uncached(&cells, &portals, &locator), None);
    capture.assert_logged_once(
        Level::Warn,
        "CellResidencySet: the runtime loader rejects this level's portals",
    );
}

#[test]
fn warm_bake_hits_the_cache_with_the_cold_section() {
    let (cells, portals, locator) = l_corridor_sections();
    let cold = bake_uncached(&cells, &portals, &locator);
    let (_dir, cache) = open_cache();

    let progress = StageProgress::indeterminate();
    let control = BakeControl::new(
        std::sync::Arc::new(crate::governor::Governor::new(1, false)),
        &progress,
    );
    let first = cell_residency_set_bake_cached(&cells, &portals, &locator, Some(&cache), &control)
        .expect("miss bakes");
    assert_eq!(first, cold);
    assert_eq!(progress.total(), Some(L_OPEN_CELLS as usize));
    assert_eq!(progress.completed(), L_OPEN_CELLS as usize);
    let access = cache.test_access(CELL_RESIDENCY_SET_STAGE_ID);
    assert_eq!((access.read_hits, access.writes), (0, 1));

    let progress = StageProgress::indeterminate();
    let control = BakeControl::new(
        std::sync::Arc::new(crate::governor::Governor::new(1, false)),
        &progress,
    );
    let capture = LogCapture::start();
    let second = cell_residency_set_bake_cached(&cells, &portals, &locator, Some(&cache), &control)
        .expect("hit loads");
    assert_eq!(second, cold);
    capture.assert_logged_once(Level::Info, "[cache] cell_residency_set hit");
    assert_eq!(progress.completed(), L_OPEN_CELLS as usize);
    let access = cache.test_access(CELL_RESIDENCY_SET_STAGE_ID);
    assert_eq!((access.read_hits, access.writes), (1, 1));
}

#[test]
fn a_corrupt_cache_entry_rebakes() {
    let (cells, portals, locator) = l_corridor_sections();
    let cold = bake_uncached(&cells, &portals, &locator);
    let (_dir, cache) = open_cache();
    cache.put(
        &cell_residency_set_cache_key(&cells, &portals, &locator),
        b"not a residency set",
    );
    let capture = LogCapture::start();
    let rebaked = cell_residency_set_bake_cached(
        &cells,
        &portals,
        &locator,
        Some(&cache),
        &BakeControl::unrestricted(),
    )
    .expect("a corrupt entry re-bakes");
    assert_eq!(rebaked, cold);
    capture.assert_logged_once(Level::Warn, "[cache] corrupt cell_residency_set entry");
}

#[test]
fn cache_key_tracks_every_input_section() {
    let (cells, portals, locator) = l_corridor_sections();
    let key = |cells: &CellsSection, portals: &PortalsSection, locator: &CellLocatorSection| {
        cell_residency_set_cache_key(cells, portals, locator).as_filename()
    };
    let base = key(&cells, &portals, &locator);
    assert_eq!(base, key(&cells, &portals, &locator));

    let mut moved_cell = cells.clone();
    moved_cell.cells[3].bounds_max[1] = 5.0;
    let mut moved_portal = portals.clone();
    moved_portal.vertices[0][1] = 0.5;
    let mut moved_plane = locator.clone();
    moved_plane.nodes[2].plane_distance = 11.0;
    for changed in [
        key(&moved_cell, &portals, &locator),
        key(&cells, &moved_portal, &locator),
        key(&cells, &portals, &moved_plane),
    ] {
        assert_ne!(changed, base);
    }
}
