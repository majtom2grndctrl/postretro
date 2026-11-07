//! Mover light gate on `mover-setpiece`: the lights `setpiece_wall` blocks closed and open
//! and `setpiece_room` blocks at either end, the texels and probes they gate and where; each
//! mover's side count. Built from `mover-setpiece-lightgate-fixture.map`, the set piece as it
//! was lit when these rows were proved, so relighting the playable map moves no row here.
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
/// the back room starts at y 1088 (`mover-setpiece-lightgate-fixture.map`).
const WALL_ARENA_FACE_Y: f64 = 1040.0;
const WALL_BACK_FACE_Y: f64 = 1072.0;
const ARENA_EDGE_Y: f64 = 1024.0;
const BACK_ROOM_EDGE_Y: f64 = 1088.0;
/// Rounding slack on a gated texel centre, far under one texel.
const SLACK_M: f64 = 1.0e-3;
/// The pocket the open wall fills starts at Quake x 640: its mouth plane,
/// engine z = −(Quake x) in metres.
const MOUTH_Z: f64 = -640.0 * UNIT;
/// The lightmap texel pitch: the compiler's default density, since the map
/// sets none and the build passes no `--lightmap-density`.
const TEXEL_PITCH_M: f64 = 0.025;

const ARENA_LIGHTS: [&str; 3] = ["256 256 256", "640 320 192", "368 560 288"];
const BACK_ROOM_LIGHTS: [&str; 2] = ["240 1376 256", "1000 1376 256"];
const SHAFT_LIGHT: &str = "992 320 288";
const CLOSED: &str = "kinematic_mover 'setpiece_wall' at first waypoint 'setpiece_wall_closed'";
const LOWERED: &str = "kinematic_mover 'setpiece_room' at first waypoint 'setpiece_room_lowered'";
const RAISED: &str = "kinematic_mover 'setpiece_room' at last waypoint 'setpiece_room_raised'";
const OPEN: &str = "kinematic_mover 'setpiece_wall' at last waypoint 'setpiece_wall_open'";
/// The baked lights that light the pocket through its mouth. The other
/// arena light and the shaft light lie past their falloff from it, and the
/// east back-room light lies east of the mouth, so no segment from the
/// pocket reaches it.
const POCKET_LIGHTS: [&str; 3] = ["640 320 192", "368 560 288", "240 1376 256"];
/// The arena light on the mouth plane. Every centre segment the open wall
/// gates for it crosses static wall beside the mouth, never the closed
/// wall, so the bake lit those texels only through off-centre samples.
const MOUTH_PLANE_LIGHT: &str = "640 320 192";
/// The pocket lights whose segments from the pocket also cross the closed
/// wall: the spot, and the west back-room light behind the wall. Open, each
/// gates some texels at both termini and some only open.
const SHARED_POCKET_LIGHTS: [&str; 2] = ["368 560 288", "240 1376 256"];
/// The arena's point lights; the spot is aimed at the wall, away from the
/// shaft.
const ARENA_POINT_LIGHTS: [&str; 2] = ["256 256 256", "640 320 192"];
/// `setpiece_room_lamp`, the dynamic light `setpiece_room` carries.
const ROOM_LAMP: &str = "'light_dynamic' at origin \"992 320 -56\"";

fn workspace_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(|crates| crates.parent())
        .expect("workspace root")
        .to_path_buf()
}

/// The one cold build every test here reads. The report module logs at
/// trace too, for the differing-centre bounds of rows gated this build.
fn setpiece_logs() -> &'static str {
    static LOGS: OnceLock<String> = OnceLock::new();
    LOGS.get_or_init(compile_setpiece)
}

fn compile_setpiece() -> String {
    let workspace = workspace_root();
    let input = workspace.join("content/dev/maps/mover-setpiece-lightgate-fixture.map");
    assert!(input.exists(), "fixture map missing: {}", input.display());
    let temp = tempfile::tempdir().expect("create output directory");
    let output: Output = prl_build()
        .env(
            "RUST_LOG",
            "debug,prl_build::mover_light_gate::report=trace",
        )
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

/// World bounds of gated centres, as a row prints them.
#[derive(Debug, Clone, Copy)]
struct Bounds {
    min: [f64; 3],
    max: [f64; 3],
}

/// The bounds after `marker` in `line`; `None` when it prints "none".
fn bounds_after(line: &str, marker: &str) -> Option<Bounds> {
    let rest = line
        .split(marker)
        .nth(1)
        .unwrap_or_else(|| panic!("`{marker}` in {line}"));
    if rest.starts_with("none") {
        return None;
    }
    let corners = rest
        .strip_prefix('(')
        .and_then(|rest| rest.split(") m").next())
        .unwrap_or_else(|| panic!("bounds after `{marker}` in {line}"));
    let numbers: Vec<f64> = corners
        .replace(")..(", ", ")
        .split(", ")
        .map(|number| number.parse().expect("a coordinate"))
        .collect();
    assert_eq!(numbers.len(), 6, "two corners: {line}");
    Some(Bounds {
        min: [numbers[0], numbers[1], numbers[2]],
        max: [numbers[3], numbers[4], numbers[5]],
    })
}

/// The number before `marker` in `text`.
fn number_before(text: &str, marker: &str) -> u64 {
    let head = text
        .split(marker)
        .next()
        .filter(|head| head.len() < text.len())
        .unwrap_or_else(|| panic!("`{marker}` in {text}"));
    head.rsplit([' ', '('])
        .next()
        .and_then(|number| number.parse().ok())
        .unwrap_or_else(|| panic!("a count before `{marker}` in {text}"))
}

/// One debug row: its light's authored origin, counts and gated bounds.
#[derive(Debug)]
struct Row {
    origin: String,
    gated_texels: u64,
    shared_texels: u64,
    differing_texels: u64,
    gated_probes: u64,
    shared_probes: u64,
    texels: Option<Bounds>,
    probes: Option<Bounds>,
}

/// The debug rows of `prefix`, a mover at one terminus.
fn rows(logs: &str, prefix: &str) -> Vec<Row> {
    logs.lines()
        .filter(|line| line.starts_with("[DEBUG") && line.contains(&format!("{prefix}, ")))
        .map(|line| {
            let origin = line
                .split("at origin \"")
                .nth(1)
                .and_then(|rest| rest.split('"').next())
                .unwrap_or_else(|| panic!("row names its light by origin: {line}"))
                .to_string();
            let (_, probes) = line
                .split_once("; ")
                .unwrap_or_else(|| panic!("probe counts follow the texel estimate: {line}"));
            Row {
                origin,
                gated_texels: number_before(line, " gated texel(s)"),
                shared_texels: number_before(line, " shared,"),
                differing_texels: number_before(line, " differing) in"),
                gated_probes: number_before(probes, " gated probe(s)"),
                shared_probes: number_before(probes, " shared,"),
                texels: bounds_after(line, "gated centres "),
                probes: bounds_after(line, "gated probe centres "),
            }
        })
        .collect()
}

/// The differing texel centres of `prefix`'s row for the light at
/// `origin`, from its trace line; `None` when no texel differs.
fn differing_centres(logs: &str, prefix: &str, origin: &str) -> Option<Bounds> {
    let lines: Vec<&str> = logs
        .lines()
        .filter(|line| {
            line.starts_with("[TRACE")
                && line.contains(&format!("{prefix}, "))
                && line.contains(&format!("at origin \"{origin}\": differing centres "))
        })
        .collect();
    assert!(lines.len() <= 1, "one differing-centre line per row");
    lines
        .first()
        .and_then(|line| bounds_after(line, "differing centres "))
}

/// How far the bounds reach from the pocket's mouth plane, in texel
/// pitches: the larger of its two z extremes' distances.
fn pitches_from_mouth(bounds: &Bounds) -> f64 {
    let farthest = (bounds.min[2] - MOUTH_Z)
        .abs()
        .max((bounds.max[2] - MOUTH_Z).abs());
    farthest / TEXEL_PITCH_M
}

/// No gate line at warn or above.
fn assert_gate_never_warns(logs: &str) {
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
}

#[test]
fn setpiece_wall_closed_gates_every_baked_light_but_the_shaft_light_across_the_wall() {
    let logs = setpiece_logs();
    assert_gate_never_warns(logs);

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

    let rows = rows(logs, CLOSED);
    // The info line names the first four rows' lights, in report order, by
    // origin; the fifth is the "1 more".
    let named: Vec<&str> = info[0]
        .split("at origin \"")
        .skip(1)
        .map(|rest| rest.split('"').next().expect("a closing quote"))
        .collect();
    let in_order: Vec<&str> = rows.iter().map(|row| row.origin.as_str()).collect();
    assert_eq!(named, in_order[..4], "{}", info[0]);

    let mut origins = in_order;
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

    // Where the gated texels and probes lie. An arena light's lie at or
    // past the wall's arena face, none in the arena, and some in the back
    // room; a back-room light's mirror that. A probe inside the closed wall
    // counts: covered means dark.
    for row in &rows {
        assert!(row.gated_probes > 0, "light {} gates no probe", row.origin);
        let texels = row.texels.expect("gated texel bounds");
        let probes = row.probes.expect("gated probe bounds");
        for (what, bounds) in [("texel", texels), ("probe", probes)] {
            let (min_x, max_x) = (bounds.min[0], bounds.max[0]);
            if ARENA_LIGHTS.contains(&row.origin.as_str()) {
                assert!(
                    max_x <= engine_x(WALL_ARENA_FACE_Y) + SLACK_M,
                    "arena light {} gates a {what} on the arena side: max x {max_x}",
                    row.origin,
                );
                assert!(
                    min_x < engine_x(BACK_ROOM_EDGE_Y),
                    "arena light {} gates no back-room {what}: min x {min_x}",
                    row.origin,
                );
            } else {
                assert!(
                    min_x >= engine_x(WALL_BACK_FACE_Y) - SLACK_M,
                    "back-room light {} gates a {what} on the back-room side: min x {min_x}",
                    row.origin,
                );
                assert!(
                    max_x > engine_x(ARENA_EDGE_Y),
                    "back-room light {} gates no arena {what}: max x {max_x}",
                    row.origin,
                );
            }
        }
    }
}

/// The rising room's shaft, in engine metres: Quake x 832 to 1152 and y 160
/// to 480 under the arena floor (engine y 0), down to the shaft floor.
fn inside_shaft(bounds: &Bounds) -> bool {
    let (x_min, x_max) = (engine_x(480.0), engine_x(160.0));
    let (z_min, z_max) = (-1152.0 * UNIT, -832.0 * UNIT);
    let floor = -192.0 * UNIT;
    bounds.min[0] >= x_min - SLACK_M
        && bounds.max[0] <= x_max + SLACK_M
        && bounds.min[2] >= z_min - SLACK_M
        && bounds.max[2] <= z_max + SLACK_M
        && bounds.min[1] >= floor - SLACK_M
        && bounds.max[1] <= SLACK_M
}

#[test]
fn setpiece_room_gates_the_shaft_lowered_and_the_arena_floor_raised() {
    let logs = setpiece_logs();
    assert_gate_never_warns(logs);
    let lowered = rows(logs, LOWERED);
    let raised = rows(logs, RAISED);

    for light in ARENA_POINT_LIGHTS {
        let at = |rows: &[Row]| -> usize {
            rows.iter()
                .position(|row| row.origin == light)
                .unwrap_or_else(|| panic!("a row for arena light {light}\n{logs}"))
        };
        let (low, high) = (&lowered[at(&lowered)], &raised[at(&raised)]);

        // Lowered, the roof closes the shaft: the arena light's gated texels
        // and probes all lie in it, and the raised room's floor, flush with
        // the arena floor, still covers each of them.
        assert!(low.gated_texels > 0 && low.gated_probes > 0, "{low:?}");
        assert!(inside_shaft(&low.texels.expect("texels")), "{low:?}");
        assert!(inside_shaft(&low.probes.expect("probes")), "{low:?}");
        assert_eq!(low.shared_texels, low.gated_texels, "{low:?}");
        assert_eq!(low.shared_probes, low.gated_probes, "{low:?}");
        assert_eq!(high.shared_texels, low.shared_texels, "{high:?}");

        // Raised, it gates texels the lowered room left lit, beyond the
        // shaft's footprint.
        assert!(high.differing_texels > 0, "{high:?}");
        assert!(!inside_shaft(&high.texels.expect("texels")), "{high:?}");
    }
    // Raised, the room shades the arena light farthest from it on the floor
    // alone: its gated texels reach the arena floor (engine y 0) and
    // nothing above it.
    let corner = raised
        .iter()
        .find(|row| row.origin == ARENA_POINT_LIGHTS[0])
        .expect("a raised row");
    let texels = corner.texels.expect("texels");
    assert!(texels.max[1].abs() <= SLACK_M, "{corner:?}");
}

#[test]
fn setpiece_wall_open_gates_the_pocket_it_fills_and_shares_what_the_closed_wall_gates() {
    let logs = setpiece_logs();
    assert_gate_never_warns(logs);
    let open = rows(logs, OPEN);
    let closed = rows(logs, CLOSED);

    let mut origins: Vec<&str> = open.iter().map(|row| row.origin.as_str()).collect();
    origins.sort_unstable();
    let mut expected = POCKET_LIGHTS.to_vec();
    expected.sort_unstable();
    assert_eq!(origins, expected, "open-wall rows\n{logs}");

    for row in &open {
        // The open wall lies wholly past the mouth plane and every light
        // here lies on or before it, so a segment from a texel before the
        // plane never reaches the wall: every gated centre is past it.
        assert!(row.gated_texels > 0, "{row:?}");
        let texels = row.texels.expect("gated texel bounds");
        assert!(texels.max[2] <= MOUTH_Z + SLACK_M, "{row:?}");

        // A texel gated at both termini counts as shared in both rows.
        let at_closed = closed
            .iter()
            .find(|closed| closed.origin == row.origin)
            .unwrap_or_else(|| panic!("a closed row for {}\n{logs}", row.origin));
        assert_eq!(row.shared_texels, at_closed.shared_texels, "{row:?}");
        if SHARED_POCKET_LIGHTS.contains(&row.origin.as_str()) {
            // Pin P1: the open wall gates pocket texels whose segments also
            // cross the closed pose, and those are shared, not differing.
            assert!(
                row.shared_texels > 0 && row.shared_texels < row.gated_texels,
                "{row:?}"
            );
        }
        assert_eq!(row.shared_probes, at_closed.shared_probes, "{row:?}");
        assert_eq!(
            row.differing_texels > 0,
            differing_centres(logs, OPEN, &row.origin).is_some(),
            "a row with differing texels bounds them: {row:?}"
        );
    }
}

#[test]
fn setpiece_wall_mouth_plane_light_open_only_texels_lie_within_one_column() {
    // The static-edge bias of the one-segment estimate, bounded for the one
    // light whose open-only texels come only from it: none is gated closed,
    // so all are open-only, and each lies within 1.5 pitches of the mouth
    // plane, so its footprint reaches the one-texel strip there. Other
    // lights' open-only texels follow the closed wall's shadow edge into
    // the pocket, wherever their placement puts it.
    let logs = setpiece_logs();
    let row = rows(logs, OPEN)
        .into_iter()
        .find(|row| row.origin == MOUTH_PLANE_LIGHT)
        .unwrap_or_else(|| panic!("an open row for {MOUTH_PLANE_LIGHT}\n{logs}"));
    assert_eq!(row.shared_texels, 0, "{row:?}");
    let bounds = differing_centres(logs, OPEN, MOUTH_PLANE_LIGHT).expect("differing centres");
    let pitches = pitches_from_mouth(&bounds);
    assert!(
        pitches <= 1.5,
        "{pitches:.2} pitches from the mouth: {bounds:?}"
    );
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
    assert_gate_never_warns(logs);

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

    // The Build Summary totals both movers' side counts, every mover analysed.
    let summary: Vec<&str> = logs
        .lines()
        .filter(|line| line.starts_with("Mover lighting: ") || line.starts_with("  side count: "))
        .collect();
    assert_eq!(summary.len(), 2, "{logs}");
    assert!(
        summary[0].starts_with("Mover lighting: 2 mover(s) analysed, 0 not analysed;"),
        "{}",
        summary[0]
    );
    assert_eq!(
        summary[1],
        "  side count: 1 carried dynamic light(s), 0 other"
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
