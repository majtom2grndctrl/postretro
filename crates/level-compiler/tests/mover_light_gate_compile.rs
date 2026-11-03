//! Mover light gate on generated maps: rows, shared and differing texels and probes, the mask
//! estimates, both baked tiers, and the lights and levels it cannot analyse.
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
    light_with(origin, 512, "")
}

/// A baked point light with falloff `range` in map units and `extra` key
/// lines.
fn light_with(origin: [i32; 3], range: u32, extra: &str) -> String {
    format!(
        "{{\n\"classname\" \"light\"\n\"origin\" \"{} {} {}\"\n\"light\" \"300\"\n\
         \"_falloff_range\" \"{range}\"\n\"_light_size\" \"0\"\n{extra}}}",
        origin[0], origin[1], origin[2]
    )
}

/// An animated baked light's key line.
const ANIMATED: &str = "\"style\" \"1\"\n";
/// An SDF-shadow light's key line.
const SDF_SHADOW: &str = "\"_shadow_type\" \"sdf\"\n";

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
/// success and that the gate warns nowhere; the combined log.
/// SH probes sit 1 m apart, so several lie between the room's floor and a
/// plate.
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
        .arg("1")
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

/// One debug row's probe counts: gated, shared, differing.
fn probe_counts(line: &str) -> (u64, u64, u64) {
    let (_, probes) = line
        .split_once("; ")
        .unwrap_or_else(|| panic!("probe counts follow the texel estimate: {line}"));
    (
        number_before(probes, " gated probe(s)"),
        number_before(probes, " shared,"),
        number_before(probes, " differing);"),
    )
}

/// Whether a debug row flags its light as inside a posed brush.
fn light_inside(line: &str) -> bool {
    line.contains("light inside: yes")
}

/// The `info` lines of `mover`.
fn info_lines<'a>(logs: &'a str, mover: &str) -> Vec<&'a str> {
    logs.lines()
        .filter(|line| {
            line.starts_with("[INFO") && line.contains(&format!("kinematic_mover '{mover}' "))
        })
        .filter(|line| line.contains("[MoverLight]"))
        .collect()
}

/// A light's label fragment, by authored origin.
fn at_origin(origin: [i32; 3]) -> String {
    format!("at origin \"{} {} {}\"", origin[0], origin[1], origin[2])
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

/// A plate between the ceiling lights and the floor, sliding 192 u along +y
/// from waypoint `plate_a` to `plate_b`.
fn sliding_plate() -> [String; 3] {
    let plate = box_brush([-64, -64, 96], [64, 64, 112]);
    [
        mover("plate", "plate_a", &[plate]),
        waypoint("plate_a", "plate_b", [0, 0, 104]),
        waypoint("plate_b", "", [0, 192, 104]),
    ]
}

const PLATE_FIRST: &str = "kinematic_mover 'plate' at first waypoint 'plate_a'";

/// The rows of `prefix` naming the light at `origin`.
fn light_rows<'a>(logs: &'a str, prefix: &str, origin: [i32; 3]) -> Vec<&'a str> {
    let light = at_origin(origin);
    rows(logs, prefix)
        .into_iter()
        .filter(|line| line.contains(&light))
        .collect()
}

#[test]
fn a_light_that_never_meets_a_mover_has_no_row_and_a_probe_only_light_rows_with_zero_texels() {
    // `near` lights the floor under the plate. `far` lights a floor corner
    // 7 m from it and reaches no pose. `probes_only`, 3.25 m up with a
    // 2.4 m reach, lights no surface; the plate, 0.7 m below it, shades the
    // probes under it.
    let near = [0, 0, 224];
    let far = [-200, -200, 32];
    let probes_only = [0, 0, 128];
    let plate = box_brush([-40, -40, 96], [40, 40, 100]);
    let logs = compile(&room_map(&[
        light(near),
        light_with(far, 48, ""),
        light_with(probes_only, 96, ""),
        mover("plate", "plate_a", &[plate]),
        waypoint("plate_a", "plate_b", [0, 0, 98]),
        waypoint("plate_b", "", [0, 192, 98]),
    ]));

    assert!(
        !logs
            .lines()
            .any(|line| line.contains("[MoverLight]") && line.contains(&at_origin(far))),
        "the far light is in no row or info line\n{logs}"
    );
    let near_rows = light_rows(&logs, PLATE_FIRST, near);
    assert_eq!(near_rows.len(), 1, "{logs}");
    assert!(row_counts(near_rows[0]).gated > 0, "{}", near_rows[0]);

    let probe_rows = light_rows(&logs, PLATE_FIRST, probes_only);
    assert_eq!(probe_rows.len(), 1, "the probe-only light rows\n{logs}");
    let row = row_counts(probe_rows[0]);
    let (gated_probes, _, _) = probe_counts(probe_rows[0]);
    assert_eq!(row.gated, 0, "it lights no texel: {}", probe_rows[0]);
    assert_eq!(row.every_gated_bytes, 0, "probes never enter the estimate");
    assert!(gated_probes > 0, "{}", probe_rows[0]);
    let info = info_lines(&logs, "plate");
    assert!(
        info.iter()
            .any(|line| line.contains(PLATE_FIRST) && line.contains(&at_origin(probes_only))),
        "the info line names a light with gated probes alone\n{}",
        info.join("\n")
    );
}

#[test]
fn an_animated_light_rows_from_its_weights_among_the_static_lights_and_sdf_lights_are_not_analysed()
{
    // Entity order: static, animated, static, then the two SDF-shadow lights.
    let first_static = [-32, 0, 224];
    let animated = [0, 0, 224];
    let last_static = [32, 0, 224];
    let static_sdf = [0, -32, 224];
    let animated_sdf = [0, 32, 224];
    let [plate, plate_a, plate_b] = sliding_plate();
    let logs = compile(&room_map(&[
        light(first_static),
        light_with(animated, 512, ANIMATED),
        light(last_static),
        light_with(static_sdf, 512, SDF_SHADOW),
        light_with(animated_sdf, 512, &format!("{ANIMATED}{SDF_SHADOW}")),
        plate,
        plate_a,
        plate_b,
    ]));

    let first_rows = rows(&logs, PLATE_FIRST);
    let order: Vec<usize> = [first_static, animated, last_static]
        .iter()
        .map(|&origin| {
            first_rows
                .iter()
                .position(|line| line.contains(&at_origin(origin)))
                .unwrap_or_else(|| panic!("a row for the light at {origin:?}\n{logs}"))
        })
        .collect();
    assert_eq!(
        order,
        [0, 1, 2],
        "rows follow authored entity order across tiers"
    );
    let animated_row = first_rows[1];
    assert!(row_counts(animated_row).gated > 0, "{animated_row}");
    assert!(probe_counts(animated_row).0 > 0, "{animated_row}");

    for (origin, tier) in [(static_sdf, "static"), (animated_sdf, "animated")] {
        let label = at_origin(origin);
        let lines: Vec<&str> = logs
            .lines()
            .filter(|line| line.contains("[MoverLight]") && line.contains(&label))
            .collect();
        assert_eq!(lines.len(), 1, "one line for the {tier} SDF light\n{logs}");
        assert!(
            lines[0].starts_with("[DEBUG")
                && lines[0].contains(&format!("not analysed: {tier} SDF-shadow light")),
            "{}",
            lines[0]
        );
    }
}

// Pin P6: termini are tested independently, the inside test included.
#[test]
fn a_light_inside_a_posed_brush_at_one_terminus_is_flagged_there_alone_and_its_texels_differ() {
    // The block wraps the light at `box_a` and parks 3.25 m east of it at
    // `box_b`, where it shades part of the east wall.
    let origin = [0, 0, 128];
    let block = box_brush([-32, -32, 96], [32, 32, 160]);
    let logs = compile(&room_map(&[
        light(origin),
        mover("block", "box_a", &[block]),
        waypoint("box_a", "box_b", [0, 0, 128]),
        waypoint("box_b", "", [128, 0, 128]),
    ]));

    let first = light_rows(
        &logs,
        "kinematic_mover 'block' at first waypoint 'box_a'",
        origin,
    );
    let last = light_rows(
        &logs,
        "kinematic_mover 'block' at last waypoint 'box_b'",
        origin,
    );
    assert_eq!((first.len(), last.len()), (1, 1), "{logs}");
    assert!(
        light_inside(first[0]),
        "flagged where it is inside: {}",
        first[0]
    );
    assert!(
        !light_inside(last[0]),
        "not flagged at the other terminus: {}",
        last[0]
    );

    // Every texel the light lit is gated at the first terminus, so the last
    // terminus's gated set lies inside it: shared is that set, and the rest
    // differ.
    let (at_first, at_last) = (row_counts(first[0]), row_counts(last[0]));
    assert!(
        at_last.gated > 0 && at_last.gated < at_first.gated,
        "{at_first:?} {at_last:?}"
    );
    assert_eq!(at_first.shared, at_last.gated, "{at_first:?} {at_last:?}");
    assert_eq!(at_first.differing, at_first.gated - at_last.gated);
    assert_eq!(at_last.differing, 0, "{at_last:?}");
}

/// The debug side-count lines, one per analysed mover.
fn side_count_lines(logs: &str) -> Vec<&str> {
    logs.lines()
        .filter(|line| line.starts_with("[DEBUG") && line.contains(" side count: "))
        .collect()
}

// Pin P2: with no static lightmap light, the walk returns before any
// consumer; probes and animated weights still gate.
#[test]
fn levels_without_static_lightmap_lights_complete_and_analyse_every_mover() {
    let level = |lights: &[String]| {
        let [plate, plate_a, plate_b] = sliding_plate();
        let mut entities = lights.to_vec();
        entities.extend([plate, plate_a, plate_b]);
        compile(&room_map(&entities))
    };

    let no_baked = level(&[]);
    let sdf_only = level(&[light_with([0, 0, 224], 512, SDF_SHADOW)]);
    let animated_only = level(&[light_with([0, 0, 224], 512, ANIMATED)]);
    for logs in [&no_baked, &sdf_only, &animated_only] {
        let analysed = side_count_lines(logs);
        assert_eq!(analysed.len(), 1, "the one mover is analysed\n{logs}");
        assert!(
            analysed[0].contains("kinematic_mover 'plate'"),
            "{}",
            analysed[0]
        );
        assert!(
            !logs.contains("[MoverLight] kinematic_mover 'plate' not analysed"),
            "{logs}"
        );
    }
    for logs in [&no_baked, &sdf_only] {
        assert!(
            !logs.lines().any(|line| line.contains(" gated texel(s)")),
            "no baked light the gate analyses, so no row\n{logs}"
        );
    }
    assert!(
        sdf_only.contains("not analysed: static SDF-shadow light"),
        "{sdf_only}"
    );

    // Without a static lightmap light the atlas places no chart, so no id-25
    // weight exists: the animated light rows on probes alone.
    let animated_rows = rows(&animated_only, PLATE_FIRST);
    assert_eq!(animated_rows.len(), 1, "{animated_only}");
    assert_eq!(
        row_counts(animated_rows[0]).gated,
        0,
        "{}",
        animated_rows[0]
    );
    assert!(probe_counts(animated_rows[0]).0 > 0, "{}", animated_rows[0]);
}

#[test]
fn an_animated_light_gates_the_texels_and_probes_it_would_as_a_static_light() {
    // One light over the plate, baked animated, then static: the weight-map
    // gate anchors and lifts each texel as the static walk does.
    let [plate, plate_a, plate_b] = sliding_plate();
    let origin = [0, 0, 224];
    // A static light keeps the atlas, so the animated one bakes weights.
    let anchor = light([-200, -200, 224]);
    let level = |extra: &str| {
        compile(&room_map(&[
            anchor.clone(),
            light_with(origin, 512, extra),
            plate.clone(),
            plate_a.clone(),
            plate_b.clone(),
        ]))
    };
    let (animated, baked_static) = (level(ANIMATED), level(""));
    for prefix in [
        PLATE_FIRST,
        "kinematic_mover 'plate' at last waypoint 'plate_b'",
    ] {
        let counts = |logs: &str| -> String {
            let rows = light_rows(logs, prefix, origin);
            assert_eq!(rows.len(), 1, "{logs}");
            let (_, counts) = rows[0].split_once(": ").expect("counts follow the label");
            counts.to_string()
        };
        let (from_weights, from_layers) = (counts(&animated), counts(&baked_static));
        assert!(row_counts(&from_weights).gated > 0, "{from_weights}");
        assert_eq!(from_weights, from_layers);
    }
}
