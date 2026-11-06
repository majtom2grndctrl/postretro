//! The lightmap pool bound through a whole build: what id 58 stores, what the
//! Build Summary tells the author, and what the loader makes of it, with and
//! without usable portals.
//! See: context/lib/build_pipeline.md §PRL section IDs (Lightmap pool bound)

mod prl_build_support;

use std::io::Cursor;
use std::path::Path;
use std::process::{Command, Output};

use postretro_level_format::lightmap::LightmapHeader;
use postretro_level_format::lightmap_pool_bound::LightmapPoolBoundSection;
use postretro_level_format::{SectionId, read_container, read_section_data};
use prl_build_support::{REACH_VERIFY_ENV, prl_build};

const TEXTURE: &str = "50-free-textures/concrete_stone_021";
const LIGHT: &str = "{\n\"classname\" \"light\"\n\"origin\" \"-128 0 96\"\n\"light\" \"300\"\n\
                     \"_falloff_range\" \"512\"\n\"_light_size\" \"0\"\n}";

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

/// `brushes` around a player spawn and a light west of the origin.
fn lit_map(brushes: &[String]) -> String {
    format!(
        "{{\n\"classname\" \"worldspawn\"\n{}\n}}\n\
         {{\n\"classname\" \"player_spawn\"\n\"origin\" \"-128 0 32\"\n}}\n{LIGHT}\n",
        brushes.join("\n")
    )
}

fn shell() -> Vec<String> {
    vec![
        box_brush([-272, -144, -16], [272, 144, 0]),
        box_brush([-272, -144, 128], [272, 144, 144]),
        box_brush([-272, -144, 0], [-256, 144, 128]),
        box_brush([256, -144, 0], [272, 144, 128]),
        box_brush([-256, -144, 0], [256, -128, 128]),
        box_brush([-256, 128, 0], [256, 144, 128]),
    ]
}

/// Two rooms joined by a doorway at x = 0, so the level has usable portals.
fn two_rooms() -> String {
    let mut brushes = shell();
    brushes.extend([
        box_brush([-16, -128, 0], [16, -32, 128]),
        box_brush([-16, 32, 0], [16, 128, 128]),
        box_brush([-16, -32, 96], [16, 32, 128]),
    ]);
    lit_map(&brushes)
}

/// One sealed room: one cell, so no portals and no id 51.
fn sealed_room() -> String {
    lit_map(&shell())
}

fn write_map(root: &Path, name: &str, source: &str) -> std::path::PathBuf {
    let maps = root.join("maps");
    std::fs::create_dir_all(&maps).unwrap();
    let map = maps.join(name);
    std::fs::write(&map, source).unwrap();
    map
}

fn base_build(map: &Path, prl: &Path) -> Command {
    let mut command = prl_build();
    command
        .arg(map)
        .arg("-o")
        .arg(prl)
        .arg("--no-tui")
        .arg("--verbose")
        .arg("--sh-probe-spacing")
        .arg("4");
    command
}

fn stdout_of(output: Output) -> String {
    let stdout = String::from_utf8_lossy(&output.stdout).into_owned();
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        output.status.success(),
        "prl-build failed\n{stdout}{stderr}"
    );
    format!("{stdout}{stderr}")
}

fn cold_build(map: &Path, prl: &Path) -> String {
    stdout_of(base_build(map, prl).arg("--no-cache").output().unwrap())
}

/// Verify mode reads neither lighting memo, and these builds must hit them.
fn cached_build(map: &Path, prl: &Path, cache: &Path) -> String {
    let mut command = base_build(map, prl);
    command
        .arg("--cache-dir")
        .arg(cache)
        .env(REACH_VERIFY_ENV, "0");
    stdout_of(command.output().unwrap())
}

fn section(prl: &Path, id: SectionId) -> Option<Vec<u8>> {
    let bytes = std::fs::read(prl).expect("read the packed PRL");
    let mut cursor = Cursor::new(bytes.as_slice());
    let meta = read_container(&mut cursor).expect("read the container");
    read_section_data(&mut cursor, &meta, id as u32).expect("read the section")
}

fn block_count(prl: &Path) -> u32 {
    let id_22 = section(prl, SectionId::Lightmap).expect("every build writes id 22");
    LightmapHeader::from_bytes(&id_22).unwrap().block_count
}

fn summary_line<'a>(output: &'a str, prefix: &str) -> &'a str {
    output
        .lines()
        .find(|line| line.starts_with(prefix))
        .unwrap_or_else(|| panic!("no line starts {prefix:?} in\n{output}"))
}

#[test]
fn id_58_is_byte_identical_warm_and_cold() {
    let temp = tempfile::tempdir().unwrap();
    let map = write_map(temp.path(), "two-rooms.map", &two_rooms());
    let cache = temp.path().join("cache");
    let (cold_prl, filled_prl, warm_prl) = (
        temp.path().join("cold.prl"),
        temp.path().join("filled.prl"),
        temp.path().join("warm.prl"),
    );

    let cold = cold_build(&map, &cold_prl);
    cached_build(&map, &filled_prl, &cache);
    let warm = cached_build(&map, &warm_prl, &cache);

    assert!(block_count(&cold_prl) > 0, "the fixture bakes lightmaps");
    assert!(
        section(&cold_prl, SectionId::CellResidencySet).is_some(),
        "the fixture has usable portals"
    );
    let stored = section(&cold_prl, SectionId::LightmapPoolBound).expect("id 58 is emitted");
    let bound = LightmapPoolBoundSection::from_bytes(&stored).unwrap();
    assert!(bound.worst_lightmap_layers > 0, "{bound:?}");
    assert_eq!(
        section(&warm_prl, SectionId::LightmapPoolBound),
        Some(stored),
    );

    assert!(warm.contains("[cache] lightmap_section hit"), "{warm}");
    assert!(warm.contains("[cache] shadowmask_atlas hit"), "{warm}");
    for output in [&cold, &warm] {
        // The mask floor's check runs after the bake, memo hit or not.
        assert!(
            output.contains("Shadowmask runtime all-resident pool: "),
            "{output}"
        );
        let line = summary_line(output, "Lightmap pool memory:");
        assert!(line.contains("at 16 m lead"), "{line}");
        assert!(line.contains("demand peaks in cell "), "{line}");
        summary_line(output, "  Lightmap bound");
    }
    assert_eq!(
        summary_line(&cold, "Lightmap pool memory:"),
        summary_line(&warm, "Lightmap pool memory:"),
    );

    let world = postretro_level_loader::load_prl(cold_prl.to_str().unwrap()).unwrap();
    let manifest = world.lightmap_stream_manifest().expect("the level streams");
    assert_eq!(manifest.pool_bound(), Some(bound));
}

#[test]
fn level_without_id_51_has_no_bound_and_runs_all_resident() {
    let temp = tempfile::tempdir().unwrap();
    let map = write_map(temp.path(), "sealed-room.map", &sealed_room());
    let prl = temp.path().join("sealed-room.prl");

    let output = cold_build(&map, &prl);

    assert!(block_count(&prl) > 0, "the fixture bakes lightmaps");
    assert_eq!(section(&prl, SectionId::CellResidencySet), None);
    assert_eq!(section(&prl, SectionId::LightmapPoolBound), None);
    let line = summary_line(&output, "Lightmap pool memory:");
    assert!(line.contains("the whole map"), "{line}");
    assert!(!line.contains("cell"), "no worst cell: {line}");

    let world = postretro_level_loader::load_prl(prl.to_str().unwrap()).unwrap();
    assert!(!world.lightmap_storage.is_streaming());
}
