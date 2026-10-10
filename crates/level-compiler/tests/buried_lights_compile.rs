//! Buried-light wiring: a compiled PRL omits the buried light from AlphaLights and the build warns.
//! See: context/lib/build_pipeline.md §Compiler pipeline (Buried lights)

use std::io::Cursor;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::time::{SystemTime, UNIX_EPOCH};

use postretro_level_format::alpha_lights::AlphaLightsSection;
use postretro_level_format::sh_volume::OctahedralShVolumeSection;
use postretro_level_format::{SectionId, read_container, read_section_data};

const TEXTURE: &str = "50-free-textures/concrete_stone_021";
/// Quake map units to engine meters.
const UNIT: f64 = 0.0254;

struct TempBuildDir(PathBuf);

impl TempBuildDir {
    fn new() -> Self {
        static SEQUENCE: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let sequence = SEQUENCE.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system clock must be after the Unix epoch")
            .as_nanos();
        let path = std::env::temp_dir().join(format!(
            "postretro-buried-lights-compile-{}-{nonce}-{sequence}",
            std::process::id()
        ));
        std::fs::create_dir(&path).expect("create isolated compiler output directory");
        Self(path)
    }

    /// Directory the fixture map and its data script live in. The `maps/`
    /// level makes the content root, and so the texture root and the data
    /// script's mod root, resolve inside this temp directory.
    fn maps_dir(&self) -> PathBuf {
        let maps = self.0.join("maps");
        std::fs::create_dir_all(&maps).expect("create fixture maps directory");
        maps
    }
}

impl Drop for TempBuildDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

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

fn light_entity(origin: [i32; 3]) -> String {
    format!(
        "{{\n\"classname\" \"light\"\n\"origin\" \"{} {} {}\"\n\"light\" \"300\"\n\
         \"_falloff_range\" \"256\"\n\"_light_size\" \"0\"\n}}",
        origin[0], origin[1], origin[2]
    )
}

/// A sealed 256 u room (walls 16 u thick) with a 32 u pillar from floor to
/// ceiling at its centre. Entity 2 is a light inside the pillar; entity 3 is a
/// light in open space beside it. `worldspawn_keys` are extra worldspawn lines.
fn pillar_room_map(worldspawn_keys: &str) -> String {
    let brushes = [
        box_brush([-144, -144, -16], [144, 144, 0]),
        box_brush([-144, -144, 128], [144, 144, 144]),
        box_brush([-144, -144, 0], [-128, 144, 128]),
        box_brush([128, -144, 0], [144, 144, 128]),
        box_brush([-128, -144, 0], [128, -128, 128]),
        box_brush([-128, 128, 0], [128, 144, 128]),
        box_brush([-16, -16, 0], [16, 16, 128]),
    ];
    [
        format!(
            "{{\n\"classname\" \"worldspawn\"\n{worldspawn_keys}{}\n}}",
            brushes.join("\n")
        ),
        "{\n\"classname\" \"player_spawn\"\n\"origin\" \"-96 0 32\"\n}".to_string(),
        light_entity(BURIED_ORIGIN),
        light_entity(OPEN_ORIGIN),
    ]
    .join("\n")
}

/// Quake-space origins, map units.
const BURIED_ORIGIN: [i32; 3] = [0, 0, 64];
const OPEN_ORIGIN: [i32; 3] = [96, 0, 64];

/// Compile `map_text` from the temp `maps/` directory with `RUST_LOG` set to
/// `rust_log`, asserting success. Returns the output and the PRL path.
fn compile(temp: &TempBuildDir, map_text: &str, rust_log: &str) -> (Output, PathBuf) {
    let input = temp.maps_dir().join("buried-light.map");
    let output_path = temp.0.join("buried-light.prl");
    std::fs::write(&input, format!("{map_text}\n")).expect("write fixture map");

    let output = Command::new(env!("CARGO_BIN_EXE_prl-build"))
        .env("RUST_LOG", rust_log)
        .arg(&input)
        .arg("-o")
        .arg(&output_path)
        .arg("--no-cache")
        .arg("--no-tui")
        .arg("--sh-probe-spacing")
        .arg("4")
        .arg("--lightmap-density")
        .arg("0.25")
        .output()
        .expect("spawn prl-build");
    assert!(
        output.status.success(),
        "prl-build failed\n{}",
        combined_logs(&output)
    );
    (output, output_path)
}

fn combined_logs(output: &Output) -> String {
    format!(
        "stdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    )
}

fn section(prl: &Path, id: SectionId) -> Vec<u8> {
    let bytes = std::fs::read(prl).expect("read compiled PRL");
    let mut cursor = Cursor::new(bytes);
    let metadata = read_container(&mut cursor).expect("read PRL container");
    read_section_data(&mut cursor, &metadata, id as u32)
        .unwrap_or_else(|error| panic!("read {id:?} section: {error}"))
        .unwrap_or_else(|| panic!("{id:?} section must be present"))
}

/// The PRL's only AlphaLights record is the open-space light.
fn assert_only_open_light_survives(prl: &Path) {
    let section = AlphaLightsSection::from_bytes(&section(prl, SectionId::AlphaLights))
        .expect("AlphaLights decodes");
    assert_eq!(
        section.lights.len(),
        1,
        "only the open-space light gets an AlphaLights record"
    );
    // engine = (-quake_y, quake_z, -quake_x) * 0.0254 m
    let expected = [
        -f64::from(OPEN_ORIGIN[1]) * UNIT,
        f64::from(OPEN_ORIGIN[2]) * UNIT,
        -f64::from(OPEN_ORIGIN[0]) * UNIT,
    ];
    let origin = section.lights[0].origin;
    assert!(
        origin
            .iter()
            .zip(expected)
            .all(|(got, want)| (got - want).abs() < 1.0e-6),
        "the surviving record is the open-space light: got {origin:?}, want {expected:?}"
    );
}

#[test]
fn compiled_prl_omits_a_light_inside_solid_and_warns_naming_it() {
    let temp = TempBuildDir::new();
    let (output, prl) = compile(&temp, &pillar_room_map(""), "warn");

    assert_only_open_light_survives(&prl);

    let logs = combined_logs(&output);
    let named = logs
        .lines()
        .filter(|line| line.contains("Light inside solid geometry"))
        .collect::<Vec<_>>();
    assert!(
        !named.is_empty()
            && named.iter().all(|line| {
                line.contains("entity 2 'light'") && line.contains("origin \"0 0 64\"")
            }),
        "the build warns naming the buried light, and only it:\n{logs}"
    );
}

/// Animates only the first light the compile-time query returns. The query
/// answers in map order, which puts the buried light (source index 0) ahead
/// of the open one (source index 1).
const FIRST_LIGHT_SCRIPT: &str = r#"import { defineReaction, getMapEntities } from "postretro";

export function setupLevel(_ctx: unknown) {
  const first = getMapEntities("light").slice(0, 1);
  return {
    reactions: [
      defineReaction("levelLoad", {
        sequence: first.flatMap((light) =>
          light.pulse({ min: 0.2, max: 1.0, periodMs: 1000 }),
        ),
      }),
    ],
  };
}
"#;

#[test]
fn data_script_light_query_omits_buried_light_so_its_first_result_is_the_open_light() {
    // Were the buried light in the script's light table, it would be the
    // first result and take the animation. The namespaces then drop it, so
    // the open light would stay steady: no animated descriptor, no slot, and
    // the reservation log would name light 0.
    let temp = TempBuildDir::new();
    std::fs::write(temp.maps_dir().join("first-light.ts"), FIRST_LIGHT_SCRIPT)
        .expect("write fixture data script");
    let map = pillar_room_map("\"data_script\" \"first-light.ts\"\n");
    let (output, prl) = compile(&temp, &map, "info");

    assert_only_open_light_survives(&prl);
    let sh = OctahedralShVolumeSection::from_bytes(&section(&prl, SectionId::OctahedralShVolume))
        .expect("OctahedralShVolume decodes");
    assert_eq!(
        sh.animation_descriptors.len(),
        1,
        "the script's target owns the only animated descriptor"
    );
    assert_eq!(
        sh.slot_for_map_light,
        [0],
        "the open light, the only AlphaLights record, owns animated slot 0"
    );

    let logs = combined_logs(&output);
    let reservations = logs
        .lines()
        .filter(|line| line.contains("derived animated-bake reservation for static light"))
        .collect::<Vec<_>>();
    assert!(
        reservations.len() == 1 && reservations[0].contains("static light 1 "),
        "the script targets source light 1 (open), never 0 (buried):\n{logs}"
    );
}
