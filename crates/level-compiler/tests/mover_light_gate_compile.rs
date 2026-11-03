//! Mover light gate on generated maps: rows, shared and differing texels, and the mask estimates.
//! See: context/lib/build_pipeline.md §Compiler pipeline (step 12)

mod prl_build_support;

use std::path::PathBuf;
use std::process::Output;
use std::time::{SystemTime, UNIX_EPOCH};

use prl_build_support::prl_build;

const TEXTURE: &str = "50-free-textures/concrete_stone_021";

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
            "postretro-mover-light-gate-compile-{}-{nonce}-{sequence}",
            std::process::id()
        ));
        std::fs::create_dir(&path).expect("create isolated compiler output directory");
        Self(path)
    }

    /// The `maps/` level puts the content root inside this temp directory.
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

/// A sealed 512 u room (walls 16 u thick), interior z 0 to 256, with a
/// player spawn and `entities` after it.
fn room_map(entities: &[String]) -> String {
    let brushes = [
        box_brush([-272, -272, -16], [272, 272, 0]),
        box_brush([-272, -272, 256], [272, 272, 272]),
        box_brush([-272, -272, 0], [-256, 272, 256]),
        box_brush([256, -272, 0], [272, 272, 256]),
        box_brush([-256, -272, 0], [256, -256, 256]),
        box_brush([-256, 256, 0], [256, 272, 256]),
    ];
    let mut map = vec![
        format!(
            "{{\n\"classname\" \"worldspawn\"\n{}\n}}",
            brushes.join("\n")
        ),
        "{\n\"classname\" \"player_spawn\"\n\"origin\" \"-192 0 32\"\n}".to_string(),
    ];
    map.extend_from_slice(entities);
    map.join("\n")
}

fn light(origin: [i32; 3]) -> String {
    format!(
        "{{\n\"classname\" \"light\"\n\"origin\" \"{} {} {}\"\n\"light\" \"300\"\n\
         \"_falloff_range\" \"512\"\n\"_light_size\" \"0\"\n}}",
        origin[0], origin[1], origin[2]
    )
}

fn mover(name: &str, path: &str, brushes: &[String]) -> String {
    format!(
        "{{\n\"classname\" \"kinematic_mover\"\n\"name\" \"{name}\"\n\"path\" \"{path}\"\n\
         \"speed\" \"2\"\n{}\n}}",
        brushes.join("\n")
    )
}

fn waypoint(name: &str, next: &str, origin: [i32; 3]) -> String {
    format!(
        "{{\n\"classname\" \"kinematic_waypoint\"\n\"name\" \"{name}\"\n\"next\" \"{next}\"\n\
         \"origin\" \"{} {} {}\"\n}}",
        origin[0], origin[1], origin[2]
    )
}

/// Compile `map_text` cold with `-v` and `RUST_LOG=debug`, asserting
/// success; the combined log.
fn compile(map_text: &str) -> String {
    let temp = TempBuildDir::new();
    let input = temp.maps_dir().join("mover-light-gate.map");
    std::fs::write(&input, format!("{map_text}\n")).expect("write fixture map");
    let output: Output = prl_build()
        .env("RUST_LOG", "debug")
        .arg(&input)
        .arg("-o")
        .arg(temp.0.join("mover-light-gate.prl"))
        .arg("--no-cache")
        .arg("--no-tui")
        .arg("-v")
        .arg("--sh-probe-spacing")
        .arg("4")
        .arg("--lightmap-density")
        .arg("0.25")
        .output()
        .expect("spawn prl-build");
    let logs = format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(output.status.success(), "prl-build failed\n{logs}");
    logs
}

/// One debug row's texel counts and estimates.
#[derive(Debug)]
struct RowCounts {
    gated: u64,
    shared: u64,
    differing: u64,
    blocks: u64,
    differing_blocks: u64,
    every_gated_bytes: u64,
    differing_only_bytes: u64,
}

/// The number before `marker` in `line`.
fn number_before(line: &str, marker: &str) -> u64 {
    let head = line
        .split(marker)
        .next()
        .filter(|head| head.len() < line.len())
        .unwrap_or_else(|| panic!("`{marker}` in {line}"));
    head.rsplit([' ', '('])
        .next()
        .and_then(|number| number.parse().ok())
        .unwrap_or_else(|| panic!("a count before `{marker}` in {line}"))
}

fn row_counts(line: &str) -> RowCounts {
    RowCounts {
        gated: number_before(line, " gated texel(s)"),
        shared: number_before(line, " shared,"),
        differing: number_before(line, " differing) in"),
        blocks: number_before(line, " cell block(s)"),
        differing_blocks: number_before(line, " differing), mask"),
        every_gated_bytes: number_before(line, " B every gated"),
        differing_only_bytes: number_before(line, " B differing only"),
    }
}

/// The debug rows whose prefix is `prefix`.
fn rows<'a>(logs: &'a str, prefix: &str) -> Vec<&'a str> {
    logs.lines()
        .filter(|line| line.starts_with("[DEBUG") && line.contains(&format!("{prefix}, ")))
        .collect()
}

// Pin P5: a path that returns to its start poses both termini alike.
#[test]
fn a_mover_whose_last_waypoint_returns_to_its_first_reports_no_differing_texels() {
    // A plate between the ceiling light and the floor; its path runs out
    // 128 u and back, so the last waypoint sits on the first.
    let plate = box_brush([-64, -64, 96], [64, 64, 112]);
    let logs = compile(&room_map(&[
        light([0, 0, 224]),
        mover("loop_plate", "loop_a", &[plate]),
        waypoint("loop_a", "loop_b", [0, 0, 104]),
        waypoint("loop_b", "loop_c", [128, 0, 104]),
        waypoint("loop_c", "", [0, 0, 104]),
    ]));

    let first = rows(
        &logs,
        "kinematic_mover 'loop_plate' at first waypoint 'loop_a'",
    );
    let last = rows(
        &logs,
        "kinematic_mover 'loop_plate' at last waypoint 'loop_c'",
    );
    assert_eq!(
        (first.len(), last.len()),
        (1, 1),
        "the light gates one row at each terminus\n{logs}"
    );
    let (at_first, at_last) = (row_counts(first[0]), row_counts(last[0]));
    for row in [&at_first, &at_last] {
        assert!(row.gated > 0, "the plate shadows the floor: {row:?}");
        assert_eq!(row.shared, row.gated, "{row:?}");
        assert_eq!((row.differing, row.differing_blocks), (0, 0), "{row:?}");
        assert_eq!(row.differing_only_bytes, 0, "{row:?}");
        assert_eq!(row.every_gated_bytes, row.gated + 8 * row.blocks, "{row:?}");
    }
    assert_eq!(at_first.gated, at_last.gated);
}
