// Load-time proofs for the CellResidencySet section (id 51): a valid section
// decodes onto the world, and a malformed one fails the whole load.
// See: context/lib/testing_guide.md · context/lib/build_pipeline.md §PRL section IDs

use postretro_level_format::cell_residency_set::{
    CELL_RESIDENCY_SET_VERSION, CellResidencySetSection, ResidencyEntry,
};
use postretro_level_format::{SectionBlob, SectionId};

use crate::load_prl;
use crate::prl_load_test_fixtures::write_prl_load_fixture;

/// Cell count of the shared two-cell load fixture.
const FIXTURE_CELLS: u32 = 2;

fn entry(cell_id: u32, lead: u32) -> ResidencyEntry {
    ResidencyEntry { lead, cell_id }
}

/// Camera 0 needs itself at lead 0 and cell 1 at 4 m; camera 1 needs itself.
fn valid_section() -> CellResidencySetSection {
    CellResidencySetSection {
        max_lead: 32 * 1024,
        offsets: vec![0, 2, 3],
        entries: vec![entry(0, 0), entry(1, 4 * 1024), entry(1, 0)],
    }
}

fn residency_blob(data: Vec<u8>) -> SectionBlob {
    SectionBlob {
        section_id: SectionId::CellResidencySet as u32,
        version: 1,
        data,
    }
}

/// Raw section bytes with arbitrary CSR words, bypassing the encoder so a
/// malformed table reaches the loader intact.
fn raw_section(offsets: &[u32], entries: &[(u32, u32)]) -> Vec<u8> {
    let mut out = Vec::new();
    for word in [
        CELL_RESIDENCY_SET_VERSION,
        FIXTURE_CELLS,
        32 * 1024,
        entries.len() as u32,
    ] {
        out.extend_from_slice(&word.to_le_bytes());
    }
    for offset in offsets {
        out.extend_from_slice(&offset.to_le_bytes());
    }
    for &(cell, lead) in entries {
        out.extend_from_slice(&cell.to_le_bytes());
        out.extend_from_slice(&lead.to_le_bytes());
    }
    out
}

fn load_error(data: Vec<u8>, name: &str) -> String {
    let path = write_prl_load_fixture([residency_blob(data)], name);
    let result = load_prl(path.to_str().unwrap());
    std::fs::remove_file(&path).ok();
    match result {
        Ok(_) => panic!("{name}: the load must fail"),
        Err(error) => error.to_string(),
    }
}

#[test]
fn a_valid_residency_set_decodes_onto_the_world() {
    let section = valid_section();
    let path = write_prl_load_fixture(
        [residency_blob(section.to_bytes())],
        "postretro_test_residency_set_valid.prl",
    );
    let world = load_prl(path.to_str().unwrap());
    std::fs::remove_file(&path).ok();
    let world = world.expect("a valid id-51 section loads");
    assert_eq!(world.cell_residency_set, Some(section));
}

#[test]
fn a_level_without_a_residency_set_loads_with_none() {
    let path = write_prl_load_fixture([], "postretro_test_residency_set_absent.prl");
    let world = load_prl(path.to_str().unwrap());
    std::fs::remove_file(&path).ok();
    assert_eq!(world.expect("load").cell_residency_set, None);
}

// ---- Load rejects malformed residency-section rows ----

#[test]
fn load_rejects_a_residency_entry_naming_a_cell_past_the_cell_count() {
    let mut section = valid_section();
    section.entries[1] = entry(FIXTURE_CELLS, 4 * 1024);
    let message = load_error(
        section.to_bytes(),
        "postretro_test_residency_set_cell_past_count.prl",
    );
    assert!(
        message.contains("CellResidencySet validation error"),
        "{message}"
    );
    assert!(
        message.contains("names cell 2 past the cell count 2"),
        "{message}"
    );
}

#[test]
fn load_rejects_residency_csr_offsets_that_decrease() {
    // Camera 0 ends at 2, camera 1 claims to end at 1.
    let message = load_error(
        raw_section(&[0, 2, 1], &[(0, 0)]),
        "postretro_test_residency_set_csr_decrease.prl",
    );
    assert!(
        message.contains("CellResidencySet validation error"),
        "{message}"
    );
    assert!(
        message.contains("CSR offsets decrease at camera cell 1"),
        "{message}"
    );
}

#[test]
fn load_rejects_residency_csr_offsets_past_entry_count() {
    // Two entries, but the CSR ends at 3.
    let message = load_error(
        raw_section(&[0, 1, 3], &[(0, 0), (1, 0)]),
        "postretro_test_residency_set_csr_past_count.prl",
    );
    assert!(
        message.contains("CellResidencySet validation error"),
        "{message}"
    );
    assert!(
        message.contains("CSR ends at 3, entry_count is 2"),
        "{message}"
    );
}

#[test]
fn load_rejects_a_residency_set_sized_for_another_cell_count() {
    let mut bytes = valid_section().to_bytes();
    bytes[4..8].copy_from_slice(&3u32.to_le_bytes());
    let message = load_error(bytes, "postretro_test_residency_set_cell_count.prl");
    assert!(
        message.contains("CellResidencySet validation error"),
        "{message}"
    );
    assert!(
        message.contains("covers 3 camera cells, level has 2"),
        "{message}"
    );
}

// ---- The bake-time visibility world uses the loader's own conversion ----

mod visibility_world_from_sections {
    use postretro_level_format::cell_locator::{
        CellLocatorChild, CellLocatorNodeRecord, CellLocatorSection,
    };
    use postretro_level_format::cells::{CellRecord, CellsSection};
    use postretro_level_format::portals::{PortalRecord, PortalsSection};

    use crate::LevelWorld;

    /// Two unit cubes on X sharing the x = 1 face through portal 0.
    fn sections(
        portal_vertices: Vec<[f32; 3]>,
    ) -> (CellsSection, PortalsSection, CellLocatorSection) {
        let cell = |x: f32, refs_at: u32| CellRecord {
            bounds_min: [x, 0.0, 0.0],
            bounds_max: [x + 1.0, 1.0, 1.0],
            flags: 0,
            face_start: 0,
            face_count: 0,
            portal_ref_start: refs_at,
            portal_ref_count: 1,
        };
        let cells = CellsSection {
            cells: vec![cell(0.0, 0), cell(1.0, 1)],
            portal_refs: vec![0, 0],
        };
        let portals = PortalsSection {
            portals: vec![PortalRecord {
                vertex_start: 0,
                vertex_count: portal_vertices.len() as u32,
                front_leaf: 0,
                back_leaf: 1,
            }],
            vertices: portal_vertices,
        };
        let locator = CellLocatorSection {
            root: CellLocatorChild::Node(0),
            nodes: vec![CellLocatorNodeRecord {
                plane_normal: [1.0, 0.0, 0.0],
                plane_distance: 1.0,
                front: CellLocatorChild::Cell(1),
                back: CellLocatorChild::Cell(0),
            }],
        };
        (cells, portals, locator)
    }

    #[test]
    fn usable_portals_convert_as_on_load() {
        let quad = vec![
            [1.0, 0.0, 0.0],
            [1.0, 1.0, 0.0],
            [1.0, 1.0, 1.0],
            [1.0, 0.0, 1.0],
        ];
        let (cells, portals, locator) = sections(quad);
        let world = LevelWorld::visibility_only_from_sections(&cells, &portals, &locator)
            .expect("a valid two-cell world");
        assert!(world.has_portals);
        assert_eq!(world.portals.len(), 1);
        assert_eq!(
            (world.portals[0].front_cell, world.portals[0].back_cell),
            (0, 1)
        );
        assert_eq!(world.cells.len(), 2);
        assert_eq!(world.locate_cell(glam::Vec3::new(1.5, 0.5, 0.5)), 1);
    }

    #[test]
    fn an_unusable_portal_drops_the_whole_set_as_on_load() {
        // Two vertices: `convert_usable_portals` rejects the section.
        let (cells, portals, locator) = sections(vec![[1.0, 0.0, 0.0], [1.0, 1.0, 0.0]]);
        let world = LevelWorld::visibility_only_from_sections(&cells, &portals, &locator)
            .expect("the no-portals fallback is still a valid world");
        assert!(!world.has_portals);
        assert!(world.portals.is_empty());
    }
}
