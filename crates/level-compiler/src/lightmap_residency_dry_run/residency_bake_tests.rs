//! The CellResidencySet bake (id 51) against the dry run's direct evaluation
//! of `M(c, L)`, dilation included and pins excluded, at every lead
//! breakpoint up to the baked maximum; and the bake's independence of id-49
//! pins.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Instant;

use postretro_level_format::cell_residency_set::CellResidencySetSection;
use postretro_level_format::cluster_directory::{
    CLUSTER_HINT_FLAG_PINNED, ClusterDirectorySection,
};
use postretro_level_format::{SectionId, read_container, read_section_data};
use postretro_level_loader::LevelWorld;

use super::brief_set::{
    BriefSetSources, Dilation, check_every_breakpoint, direct_set, pinned_cells, visible_sources,
};
use super::{read_dry_run_input, run_dry_run};
use crate::bake_control::BakeControl;
use crate::cell_residency_bake::lead_map::{
    BRIEF_MAX_LEAD_METERS, LeadMap, meters_fixed, portal_neighbours,
};
use crate::cell_residency_bake::portal_distance::{
    Neighbors, portal_graph_from_sections, recompute_pairs,
};
use crate::cell_residency_bake::pvs_sampling::sample_pvs;
use crate::cell_residency_bake::test_fixtures::{L_OPEN_CELLS, l_corridor_sections};
use crate::cell_residency_bake::{camera_cells, cell_residency_set_bake_cached};

/// AC 2 on a synthetic level: the baked section, encoded and decoded as the
/// PRL carries it, equals direct evaluation of the dilated, unpinned set for
/// every camera cell at every breakpoint of either side.
#[test]
fn residency_set_bake_matches_direct_evaluation() {
    let (cells, portals, locator) = l_corridor_sections();
    let baked = cell_residency_set_bake_cached(
        &cells,
        &portals,
        &locator,
        None,
        &BakeControl::unrestricted(),
    )
    .expect("the bake succeeds")
    .expect("usable portals");
    let decoded = CellResidencySetSection::from_bytes(&baked.to_bytes(), cells.cells.len())
        .expect("the baked section decodes");

    // Direct evaluation from the same sections, the way the dry run reads a PRL.
    let graph = portal_graph_from_sections(&cells, &portals);
    let world = LevelWorld::visibility_only_from_sections(&cells, &portals, &locator)
        .expect("loader world");
    let cameras = camera_cells(&cells);
    let neighbors = Neighbors::from_pairs(
        cells.cells.len(),
        &recompute_pairs(&graph, meters_fixed(BRIEF_MAX_LEAD_METERS)),
    );
    let pvs = sample_pvs(&world, &cameras, &BakeControl::unrestricted());
    let visible = visible_sources(&pvs.dense, &portal_neighbours(&graph), Dilation::OneHop);
    let sources = BriefSetSources {
        neighbors: &neighbors,
        visible: &visible,
        pinned: &[],
        cell_count: cells.cells.len(),
    };
    let map = LeadMap::from_section(&decoded);
    let check = check_every_breakpoint(&map, &sources, &cameras);
    assert_eq!(check.mismatched, 0, "{check:?}");
    assert!(check.checked > 2 * cameras.len(), "{check:?}");
    assert!(check.distinct_leads >= 3, "{check:?}");

    // Non-vacuous: the set grows with lead, and dilation adds a cell the
    // undilated set lacks at lead 0.
    let undilated = visible_sources(&pvs.dense, &portal_neighbours(&graph), Dilation::None);
    let bare = BriefSetSources {
        visible: &undilated,
        ..sources
    };
    let grows = cameras.iter().any(|&camera| {
        map.mandatory(camera, 0, &[]).len()
            < map
                .mandatory(camera, meters_fixed(BRIEF_MAX_LEAD_METERS), &[])
                .len()
    });
    let dilates = cameras
        .iter()
        .any(|&camera| direct_set(&bare, camera, 0).len() < map.mandatory(camera, 0, &[]).len());
    assert!(grows && dilates, "grows {grows}, dilates {dilates}");

    // AC 3, bake half: the bake reads no pins. The far end of the Z leg is
    // out of lead and view from the start of the X leg, so it is not baked
    // for camera 0; were its cluster pinned, the runtime would add it from
    // id 49 at every lead, and the baked relation would stay as it is.
    let far = L_OPEN_CELLS - 1;
    let max = meters_fixed(BRIEF_MAX_LEAD_METERS);
    assert!(!map.mandatory(0, max, &[]).contains(&far));
    assert!(!direct_set(&sources, 0, max).contains(&far));
    assert!(map.mandatory(0, 0, &[far]).contains(&far));
}

/// AC 2 end to end and AC 3's bake half, on the hinted doorway fixture: a
/// compiled PRL's id 51 equals the dry run's direct evaluation (read through
/// the dry run's own PRL reader and report), a pinned cell enters no camera
/// cell's baked set merely for being pinned, and removing the pin leaves id
/// 51 byte-identical.
#[test]
fn compiled_residency_set_matches_direct_evaluation_and_ignores_pins() {
    let dir = tempfile::tempdir().expect("temporary output directory");
    let source_map = workspace_root().join("content/dev/maps/sh-streaming-hinted-door.map");
    let pinned_prl = dir.path().join("hinted-door.prl");
    let started = Instant::now();
    compile(&source_map, &pinned_prl);
    let compile_seconds = started.elapsed().as_secs_f64();

    let baked = residency_section(&pinned_prl);
    let input = read_dry_run_input(&pinned_prl).expect("the dry run reads a new PRL");
    assert_eq!(input.baked_residency_set.as_ref(), Some(&baked));

    // The dry-run report (the yardstick's path) checks id 51 itself.
    let report = run_dry_run(&input);
    let brief = &report
        .visible_set
        .as_ref()
        .expect("portal graph and world present")
        .brief_set;
    let check = brief.baked.expect("the report checks the baked section");
    assert_eq!(check.mismatched, 0, "{check:?}");
    assert!(check.checked >= report.camera_cells.len(), "{check:?}");
    assert!(report.render().contains("baked id 51 vs direct evaluation"));

    // AC 3, bake half. The fixture pins a near-side cluster.
    let pinned = pinned_cells(&input);
    assert!(!pinned.is_empty(), "the fixture must pin a cluster");
    let graph = input.portal_graph.as_ref().expect("portal graph");
    let world = input.visibility_world.as_ref().expect("visibility world");
    let neighbors = Neighbors::from_pairs(
        input.cell_count(),
        &recompute_pairs(graph, meters_fixed(BRIEF_MAX_LEAD_METERS)),
    );
    let pvs = sample_pvs(world, &report.camera_cells, &BakeControl::unrestricted());
    let visible = visible_sources(&pvs.dense, &portal_neighbours(graph), Dilation::OneHop);
    let unpinned = BriefSetSources {
        neighbors: &neighbors,
        visible: &visible,
        pinned: &[],
        cell_count: input.cell_count(),
    };
    // Every cell of this two-room fixture is in view of every camera cell, so
    // the pinned cell is always reached; the pin-independence proof is the
    // byte comparison below. The per-cell rule still holds for each entry.
    let map = LeadMap::from_section(&baked);
    for &camera in &report.camera_cells {
        for lead in [0, meters_fixed(16), meters_fixed(BRIEF_MAX_LEAD_METERS)] {
            let set = map.mandatory(camera, lead, &[]);
            let reached = direct_set(&unpinned, camera, lead);
            for &cell in &pinned {
                // A pinned cell is baked exactly when lead or visibility
                // reaches it, never for the pin alone.
                assert_eq!(
                    set.contains(&cell),
                    reached.contains(&cell),
                    "camera {camera}, lead {lead}, pinned cell {cell}"
                );
            }
        }
    }

    // Removing the pin changes id 49 and leaves id 51 byte-identical.
    let unpinned_map = dir.path().join("hinted-door-unpinned.map");
    std::fs::write(
        &unpinned_map,
        without_resident_volume(&map_text(&source_map)),
    )
    .unwrap();
    let unpinned_prl = dir.path().join("hinted-door-unpinned.prl");
    compile(&unpinned_map, &unpinned_prl);
    assert!(has_pinned_hint(&pinned_prl));
    assert!(!has_pinned_hint(&unpinned_prl));
    assert_eq!(
        section_bytes(&pinned_prl, SectionId::CellResidencySet),
        section_bytes(&unpinned_prl, SectionId::CellResidencySet),
        "id 51 must not depend on id-49 pins"
    );
    println!(
        "hinted door: compile {compile_seconds:.2} s, id 51 {} bytes, {} entries",
        baked.byte_len(),
        baked.entries.len()
    );
}

fn compile(map: &Path, output: &Path) {
    let args = crate::parse_args_from(
        [
            map.to_str().expect("fixture path is UTF-8").to_owned(),
            "--no-cache".to_owned(),
            "--sh-probe-spacing".to_owned(),
            "4".to_owned(),
            "-o".to_owned(),
            output.to_str().expect("temporary path is UTF-8").to_owned(),
        ]
        .into_iter(),
    )
    .expect("fixture arguments parse");
    let started = Instant::now();
    let reporter: Arc<dyn crate::reporter::Reporter> = Arc::new(
        crate::reporter::PlainReporter::new(started, crate::logger::LogSink::default()),
    );
    crate::pipeline::run(
        &args,
        None,
        started,
        reporter,
        Arc::new(crate::governor::Governor::new(1, false)),
    )
    .expect("fixture compiles");
}

fn workspace_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .expect("workspace root")
        .to_path_buf()
}

fn map_text(path: &Path) -> String {
    std::fs::read_to_string(path).expect("fixture map is readable")
}

/// The map with its `stream_resident_volume` entity removed. Hint brushes are
/// peeled off before BSP construction, so cells and portals are unchanged.
fn without_resident_volume(text: &str) -> String {
    let marker = "\"classname\" \"stream_resident_volume\"";
    let at = text.find(marker).expect("the fixture pins a cluster");
    let start = text[..at].rfind('{').expect("entity opens");
    let mut depth = 0;
    let mut end = start;
    for (offset, ch) in text[start..].char_indices() {
        match ch {
            '{' => depth += 1,
            '}' => {
                depth -= 1;
                if depth == 0 {
                    end = start + offset + 1;
                    break;
                }
            }
            _ => {}
        }
    }
    format!("{}{}", &text[..start], &text[end..])
}

fn section_bytes(path: &Path, id: SectionId) -> Option<Vec<u8>> {
    let mut reader = std::io::BufReader::new(std::fs::File::open(path).expect("open PRL"));
    let meta = read_container(&mut reader).expect("read container");
    read_section_data(&mut reader, &meta, id as u32).expect("read section")
}

fn residency_section(path: &Path) -> CellResidencySetSection {
    let cells = postretro_level_format::cells::CellsSection::from_bytes(
        &section_bytes(path, SectionId::Cells).expect("Cells"),
    )
    .expect("parse Cells");
    CellResidencySetSection::from_bytes(
        &section_bytes(path, SectionId::CellResidencySet).expect("a compiled PRL carries id 51"),
        cells.cells.len(),
    )
    .expect("parse CellResidencySet")
}

fn has_pinned_hint(path: &Path) -> bool {
    ClusterDirectorySection::from_bytes(
        &section_bytes(path, SectionId::ClusterDirectory).expect("id 49"),
    )
    .expect("parse id 49")
    .cluster_hints
    .iter()
    .any(|hint| hint.flags & CLUSTER_HINT_FLAG_PINNED != 0)
}
