//! The SH mandatory bound through a whole build: what id 49 stores and what
//! the Build Summary tells the author, with and without usable portals.
//! See: context/lib/build_pipeline.md §PRL section IDs (SH mandatory bound)

mod prl_build_support;

use std::io::Cursor;
use std::path::{Path, PathBuf};

use postretro_level_format::cluster_directory::{
    ClusterDirectorySection, ShMandatoryBoundEntry, WHOLE_MAP_WORST_CELL,
};
use postretro_level_format::{SectionId, read_container, read_section_data};
use prl_build_support::prl_build;

const TEXTURE: &str = "50-free-textures/concrete_stone_021";

fn box_brush(min: [i32; 3], max: [i32; 3]) -> String {
    let ([x0, y0, z0], [x1, y1, z1]) = (min, max);
    format!(
        "{{\n\
         ( {x0} 0 0 ) ( {x0} 1 0 ) ( {x0} 0 1 ) {TEXTURE} 0 0 0 1 1\n\
         ( {x1} 0 0 ) ( {x1} 0 1 ) ( {x1} 1 0 ) {TEXTURE} 0 0 0 1 1\n\
         ( 0 {y0} 0 ) ( 0 {y0} 1 ) ( 1 {y0} 0 ) {TEXTURE} 0 0 0 1 1\n\
         ( 0 {y1} 0 ) ( 1 {y1} 0 ) ( 0 {y1} 1 ) {TEXTURE} 0 0 0 1 1\n\
         ( 0 0 {z0} ) ( 1 0 {z0} ) ( 0 1 {z0} ) {TEXTURE} 0 0 0 1 1\n\
         ( 0 0 {z1} ) ( 0 1 {z1} ) ( 1 0 {z1} ) {TEXTURE} 0 0 0 1 1\n\
         }}"
    )
}

/// A sealed room with one light: one cell, so no portals and no id 51.
fn sealed_room() -> String {
    let brushes = [
        box_brush([-272, -272, -16], [272, 272, 0]),
        box_brush([-272, -272, 256], [272, 272, 272]),
        box_brush([-272, -272, 0], [-256, 272, 256]),
        box_brush([256, -272, 0], [272, 272, 256]),
        box_brush([-256, -272, 0], [256, -256, 256]),
        box_brush([-256, 256, 0], [256, 272, 256]),
    ];
    format!(
        "{{\n\"classname\" \"worldspawn\"\n{}\n}}\n\
         {{\n\"classname\" \"player_spawn\"\n\"origin\" \"0 0 32\"\n}}\n\
         {{\n\"classname\" \"light\"\n\"origin\" \"0 0 128\"\n\"light\" \"300\"\n\
         \"_falloff_range\" \"512\"\n\"_light_size\" \"0\"\n}}\n",
        brushes.join("\n")
    )
}

/// Compile `map` at 4 m probe spacing: stdout, and id 49's stored bound.
fn compile(map: &Path, prl: &Path) -> (String, Vec<ShMandatoryBoundEntry>) {
    let output = prl_build()
        .arg(map)
        .arg("-o")
        .arg(prl)
        .arg("--no-cache")
        .arg("--no-tui")
        .arg("--sh-probe-spacing")
        .arg("4")
        .output()
        .expect("spawn prl-build");
    let stdout = String::from_utf8_lossy(&output.stdout).into_owned();
    assert!(
        output.status.success(),
        "prl-build failed\n{stdout}{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let bytes = std::fs::read(prl).expect("read the packed PRL");
    let mut cursor = Cursor::new(bytes.as_slice());
    let meta = read_container(&mut cursor).expect("read the container");
    let directory = read_section_data(&mut cursor, &meta, SectionId::ClusterDirectory as u32)
        .expect("read id 49")
        .expect("every build writes id 49");
    let directory = ClusterDirectorySection::from_bytes(&directory).expect("decode id 49");
    (stdout, directory.mandatory_bound)
}

fn summary_line<'a>(stdout: &'a str, prefix: &str) -> &'a str {
    stdout
        .lines()
        .find(|line| line.starts_with(prefix))
        .unwrap_or_else(|| panic!("no line starts {prefix:?} in\n{stdout}"))
}

#[test]
fn a_level_without_portals_stores_and_reports_the_whole_map() {
    let temp = tempfile::tempdir().unwrap();
    let maps = temp.path().join("maps");
    std::fs::create_dir(&maps).unwrap();
    let map = maps.join("sealed-room.map");
    std::fs::write(&map, sealed_room()).unwrap();

    let (stdout, bound) = compile(&map, &temp.path().join("sealed-room.prl"));

    assert_eq!(bound.len(), 1);
    assert_eq!(bound[0].lead, 0);
    assert_eq!(bound[0].worst_cell, WHOLE_MAP_WORST_CELL);
    assert!(bound[0].worst_dense_slots > 0);
    let line = summary_line(&stdout, "SH mandatory memory:");
    assert!(line.contains("the whole map"), "{line}");
    summary_line(&stdout, "  SH bound");
}

#[test]
fn a_level_with_portals_stores_both_leads_and_names_the_worst_cell() {
    let temp = tempfile::tempdir().unwrap();
    let map = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../content/dev/maps/sh-streaming-hinted-door.map");

    let (stdout, bound) = compile(&map, &temp.path().join("hinted-door.prl"));

    assert_eq!(bound.len(), 2, "default lead and baked maximum");
    assert!(bound[0].lead < bound[1].lead);
    assert!(bound.iter().all(|entry| entry.worst_dense_slots > 0));
    let line = summary_line(&stdout, "SH mandatory memory:");
    assert!(
        line.contains(&format!("peaks in cell {}, near", bound[0].worst_cell)),
        "{line}"
    );
    assert!(
        line.contains(&format!("{} probe slots", bound[0].worst_dense_slots)),
        "{line}"
    );
    summary_line(&stdout, "  at the maximum 32 m lead:");
}
