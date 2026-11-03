//! Mover light gate on `mover-setpiece`: the lights `setpiece_wall` blocks when closed, and where;
//! each mover's side count.
//! See: context/lib/build_pipeline.md §Compiler pipeline (step 12)

mod prl_build_support;

use std::path::PathBuf;
use std::process::Output;
use std::sync::OnceLock;

use prl_build_support::prl_build;

/// Quake map units to engine meters.
const UNIT: f64 = 0.0254;
/// Engine x is Quake −y in meters, so the arena (Quake y below the wall)
/// has the larger engine x.
fn engine_x(quake_y: f64) -> f64 {
    -quake_y * UNIT
}

/// The closed wall spans Quake y 1040 to 1072; the arena ends at y 1024 and
/// the back room starts at y 1088 (`mover-setpiece.map` header).
const WALL_ARENA_FACE_Y: f64 = 1040.0;
const WALL_BACK_FACE_Y: f64 = 1072.0;
const ARENA_EDGE_Y: f64 = 1024.0;
const BACK_ROOM_EDGE_Y: f64 = 1088.0;
/// Rounding slack on a gated texel centre, far under one texel.
const SLACK_M: f64 = 1.0e-3;

const ARENA_LIGHTS: [&str; 3] = ["256 256 256", "640 320 192", "368 560 288"];
const BACK_ROOM_LIGHTS: [&str; 2] = ["240 1376 256", "1000 1376 256"];
const SHAFT_LIGHT: &str = "992 320 288";
const CLOSED: &str = "kinematic_mover 'setpiece_wall' at first waypoint 'setpiece_wall_closed'";
/// `setpiece_room_lamp`, the dynamic light `setpiece_room` carries.
const ROOM_LAMP: &str = "'light_dynamic' at origin \"992 320 -56\"";

fn workspace_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(|crates| crates.parent())
        .expect("workspace root")
        .to_path_buf()
}

/// The one cold build every test here reads.
fn setpiece_logs() -> &'static str {
    static LOGS: OnceLock<String> = OnceLock::new();
    LOGS.get_or_init(compile_setpiece)
}

fn compile_setpiece() -> String {
    let workspace = workspace_root();
    let input = workspace.join("content/dev/maps/mover-setpiece.map");
    assert!(input.exists(), "fixture map missing: {}", input.display());
    let temp = tempfile::tempdir().expect("create output directory");
    let output: Output = prl_build()
        .env("RUST_LOG", "debug")
        .arg(&input)
        .arg("-o")
        .arg(temp.path().join("mover-setpiece.prl"))
        .arg("--no-cache")
        .arg("--no-tui")
        .arg("-v")
        .arg("--baked-root")
        .arg(temp.path().join("baked"))
        .current_dir(&workspace)
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

/// One closed-terminus debug row: the light's authored origin and the world
/// bounds of its gated texel centres.
struct Row {
    origin: String,
    min_x: f64,
    max_x: f64,
}

fn closed_rows(logs: &str) -> Vec<Row> {
    logs.lines()
        .filter(|line| line.starts_with("[DEBUG") && line.contains(&format!("{CLOSED}, ")))
        .map(|line| {
            let origin = line
                .split("at origin \"")
                .nth(1)
                .and_then(|rest| rest.split('"').next())
                .unwrap_or_else(|| panic!("row names its light by origin: {line}"))
                .to_string();
            let bounds = line
                .split("gated centres (")
                .nth(1)
                .unwrap_or_else(|| panic!("row carries gated centres: {line}"));
            let numbers: Vec<f64> = bounds
                .trim_end_matches(") m")
                .replace(")..(", ", ")
                .split(", ")
                .map(|number| number.parse().expect("a coordinate"))
                .collect();
            assert_eq!(numbers.len(), 6, "two corners: {line}");
            Row {
                origin,
                min_x: numbers[0],
                max_x: numbers[3],
            }
        })
        .collect()
}

#[test]
fn setpiece_wall_closed_gates_every_baked_light_but_the_shaft_light_across_the_wall() {
    let logs = setpiece_logs();

    let warned: Vec<&str> = logs
        .lines()
        .filter(|line| line.contains("[MoverLight]"))
        .filter(|line| line.contains("[WARN") || line.contains("[ERROR"))
        .collect();
    assert!(
        warned.is_empty(),
        "the gate never warns:\n{}",
        warned.join("\n")
    );

    let info: Vec<&str> = logs
        .lines()
        .filter(|line| line.starts_with("[INFO") && line.contains(CLOSED))
        .collect();
    assert_eq!(info.len(), 1, "one info line for the closed wall\n{logs}");
    assert!(
        info[0].contains("gates 5 baked light(s)") && info[0].ends_with("and 1 more"),
        "{}",
        info[0]
    );

    let rows = closed_rows(logs);
    let mut origins: Vec<&str> = rows.iter().map(|row| row.origin.as_str()).collect();
    origins.sort_unstable();
    let mut expected: Vec<&str> = ARENA_LIGHTS
        .iter()
        .chain(&BACK_ROOM_LIGHTS)
        .copied()
        .collect();
    expected.sort_unstable();
    assert_eq!(origins, expected, "closed-wall rows\n{logs}");

    let shaft = format!("at origin \"{SHAFT_LIGHT}\"");
    assert!(
        !logs
            .lines()
            .any(|line| line.contains("'setpiece_wall'") && line.contains(&shaft)),
        "the shaft light is in no setpiece_wall row"
    );

    // Where the gated texels lie: an arena light's never reach past the
    // wall's arena face into the arena, and reach the back room; a back-room
    // light's mirror that.
    for row in &rows {
        if ARENA_LIGHTS.contains(&row.origin.as_str()) {
            assert!(
                row.max_x <= engine_x(WALL_ARENA_FACE_Y) + SLACK_M,
                "arena light {} gates a texel on the arena side: max x {}",
                row.origin,
                row.max_x
            );
            assert!(
                row.min_x < engine_x(BACK_ROOM_EDGE_Y),
                "arena light {} gates no back-room texel: min x {}",
                row.origin,
                row.min_x
            );
        } else {
            assert!(
                row.min_x >= engine_x(WALL_BACK_FACE_Y) - SLACK_M,
                "back-room light {} gates a texel on the back-room side: min x {}",
                row.origin,
                row.min_x
            );
            assert!(
                row.max_x > engine_x(ARENA_EDGE_Y),
                "back-room light {} gates no arena texel: max x {}",
                row.origin,
                row.max_x
            );
        }
    }
}

/// The `debug` side-count line of mover `name`.
fn side_count_line<'a>(logs: &'a str, name: &str) -> &'a str {
    let lines: Vec<&str> = logs
        .lines()
        .filter(|line| {
            line.starts_with("[DEBUG")
                && line.contains(&format!("kinematic_mover '{name}' side count: "))
        })
        .collect();
    assert_eq!(lines.len(), 1, "one side-count line for {name}\n{logs}");
    lines[0]
}

#[test]
fn setpiece_room_side_count_is_its_carried_lamp_and_the_wall_has_none() {
    let logs = setpiece_logs();

    let room = side_count_line(logs, "setpiece_room");
    let carried = room
        .split("side count: ")
        .nth(1)
        .expect("counts follow the label");
    assert!(
        carried.starts_with("1 carried dynamic light(s): ")
            && carried.contains(ROOM_LAMP)
            && carried.ends_with("; 0 other dynamic light(s)"),
        "{room}"
    );
    let wall = side_count_line(logs, "setpiece_wall");
    assert!(
        wall.ends_with("side count: 0 carried dynamic light(s); 0 other dynamic light(s)"),
        "{wall}"
    );

    let elsewhere: Vec<&str> = logs
        .lines()
        .filter(|line| line.contains("[MoverLight]") && line.contains(ROOM_LAMP))
        .filter(|line| !line.contains(" side count: "))
        .collect();
    assert!(
        elsewhere.is_empty(),
        "a dynamic light is in no row and no not-analysed line:\n{}",
        elsewhere.join("\n")
    );
}
