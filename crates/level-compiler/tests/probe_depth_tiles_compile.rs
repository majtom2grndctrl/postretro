//! Probe depth tiles (id 59) through a whole build: emitted with id 34, the
//! same bytes warm and cold, and the Build Summary's tile bytes priced as the
//! renderer prices them.
//! See: context/lib/build_pipeline.md §PRL section IDs · §Build Cache

mod prl_build_support;

use std::io::Cursor;
use std::path::Path;
use std::process::{Command, Output};

use postretro_level_format::probe_depth_tiles::{
    MAX_PROBE_DEPTH_TILES, ProbeDepthTilesSection, probe_depth_tile_texture_bytes,
};
use postretro_level_format::{SectionId, read_container, read_section_data};
use prl_build_support::{REACH_VERIFY_ENV, prl_build};

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

/// A lit room and a dark one, split by a 16-unit (0.41 m) wall: thinner than
/// the 1 m probe spacing, so probes either side of it get tiles.
fn thin_wall_rooms() -> String {
    let brushes = [
        box_brush([-272, -144, -16], [272, 144, 0]),
        box_brush([-272, -144, 128], [272, 144, 144]),
        box_brush([-272, -144, 0], [-256, 144, 128]),
        box_brush([256, -144, 0], [272, 144, 128]),
        box_brush([-256, -144, 0], [256, -128, 128]),
        box_brush([-256, 128, 0], [256, 144, 128]),
        box_brush([-8, -128, 0], [8, 128, 128]),
    ];
    format!(
        "{{\n\"classname\" \"worldspawn\"\n{}\n}}\n\
         {{\n\"classname\" \"player_spawn\"\n\"origin\" \"-128 0 32\"\n}}\n\
         {{\n\"classname\" \"light\"\n\"origin\" \"-128 0 96\"\n\"light\" \"300\"\n\
         \"_falloff_range\" \"512\"\n}}\n",
        brushes.join("\n")
    )
}

fn base_build(map: &Path, prl: &Path) -> Command {
    let mut command = prl_build();
    command
        .arg(map)
        .arg("-o")
        .arg(prl)
        .arg("--no-tui")
        .arg("--verbose");
    command
}

fn output_of(output: Output) -> String {
    let stdout = String::from_utf8_lossy(&output.stdout).into_owned();
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        output.status.success(),
        "prl-build failed\n{stdout}{stderr}"
    );
    format!("{stdout}{stderr}")
}

fn section(prl: &Path, id: SectionId) -> Option<Vec<u8>> {
    let bytes = std::fs::read(prl).expect("read the packed PRL");
    let mut cursor = Cursor::new(bytes.as_slice());
    let meta = read_container(&mut cursor).expect("read the container");
    read_section_data(&mut cursor, &meta, id as u32).expect("read the section")
}

fn line_with<'a>(output: &'a str, needle: &str) -> &'a str {
    let mut lines = output.lines().filter(|line| line.contains(needle));
    let line = lines
        .next()
        .unwrap_or_else(|| panic!("no line holds {needle:?} in\n{output}"));
    assert!(lines.next().is_none(), "{needle:?} printed more than once");
    line
}

// Through the binary: a cold build, then a warm build that fills the cache
// and one that hits it. Each emits id 59 beside id 34 with the cold bytes,
// logs the kept and dropped counts once, and reports tile bytes priced by the
// function the renderer's allocation ledger uses.
#[test]
fn id_59_is_byte_identical_warm_and_cold_and_priced_as_the_runtime_prices_it() {
    let temp = tempfile::tempdir().unwrap();
    let maps = temp.path().join("maps");
    std::fs::create_dir_all(&maps).unwrap();
    let map = maps.join("thin-wall.map");
    std::fs::write(&map, thin_wall_rooms()).unwrap();
    let cache = temp.path().join("cache");

    let cold_prl = temp.path().join("cold.prl");
    let cold = output_of(
        base_build(&map, &cold_prl)
            .arg("--no-cache")
            .output()
            .unwrap(),
    );
    assert!(section(&cold_prl, SectionId::OctahedralShVolume).is_some());
    let stored = section(&cold_prl, SectionId::ProbeDepthTiles).expect("id 59 rides id 34");
    let tiles = ProbeDepthTilesSection::from_bytes(&stored).unwrap();
    assert!(tiles.tile_count() > 0, "the thin wall tiles its neighbours");

    let mut outputs = vec![cold];
    for name in ["filled", "warm"] {
        let prl = temp.path().join(format!("{name}.prl"));
        let mut command = base_build(&map, &prl);
        command
            .arg("--cache-dir")
            .arg(&cache)
            .env(REACH_VERIFY_ENV, "0");
        outputs.push(output_of(command.output().unwrap()));
        assert_eq!(
            section(&prl, SectionId::ProbeDepthTiles).as_ref(),
            Some(&stored),
            "{name}: id 59 bytes"
        );
    }
    assert!(
        outputs[2].contains("[cache] sh_base_volume hit"),
        "{}",
        outputs[2]
    );

    let count = tiles.tile_count();
    let log = format!(
        "Probe depth tiles: {count} kept, 0 dropped past the {MAX_PROBE_DEPTH_TILES}-tile cap"
    );
    let bytes = probe_depth_tile_texture_bytes(count as u32);
    let summary = format!("Probe depth tiles: {count} tiles, 0 dropped");
    for output in &outputs {
        line_with(output, &log);
        let line = line_with(output, &summary);
        assert!(
            line.contains(&format!("tile texture {bytes} bytes")),
            "{line}"
        );
    }
}
