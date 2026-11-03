//! Mover light gate on generated maps: rows, shared and differing texels and probes, the mask
//! estimates, both baked tiers, and the lights and levels it cannot analyse.
//! See: context/lib/build_pipeline.md §Compiler pipeline (step 12)

mod prl_build_support;

use std::io::Cursor;
use std::path::PathBuf;
use std::process::Output;
use std::sync::OnceLock;
use std::time::{SystemTime, UNIX_EPOCH};

use postretro_level_format::{SectionId, read_container};
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
    mover_with(&format!("\"name\" \"{name}\"\n"), path, brushes)
}

/// A mover with `keys` (whole key lines, its name among them if any).
fn mover_with(keys: &str, path: &str, brushes: &[String]) -> String {
    format!(
        "{{\n\"classname\" \"kinematic_mover\"\n{keys}\"path\" \"{path}\"\n\
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
    compile_keeping_prl(map_text).0
}

/// [`compile`], also returning the packed PRL's bytes.
fn compile_keeping_prl(map_text: &str) -> (String, Vec<u8>) {
    let temp = TempBuildDir::new();
    let input = temp.maps_dir().join("mover-light-gate.map");
    let prl = temp.0.join("mover-light-gate.prl");
    std::fs::write(&input, format!("{map_text}\n")).expect("write fixture map");
    let output: Output = prl_build()
        .env("RUST_LOG", "debug")
        .arg(&input)
        .arg("-o")
        .arg(&prl)
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
    let bytes = std::fs::read(&prl).expect("read the packed PRL");
    (logs, bytes)
}

/// The Build Summary's mover-lighting line starting with `prefix`.
fn summary_line<'a>(logs: &'a str, prefix: &str) -> &'a str {
    let lines: Vec<&str> = logs
        .lines()
        .filter(|line| line.starts_with(prefix))
        .collect();
    assert_eq!(lines.len(), 1, "one summary line `{prefix}`\n{logs}");
    lines[0]
}

const SUMMARY: &str = "Mover lighting: ";

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
        let summary = summary_line(logs, SUMMARY);
        assert!(
            summary.starts_with("Mover lighting: 1 mover(s) analysed, 0 not analysed;"),
            "the Build Summary prints with the mover analysed: {summary}"
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

/// One Quake map unit in engine metres.
const UNIT: f64 = 0.0254;

/// A row's gated texel centres, engine metres: (min, max).
fn gated_centres(line: &str) -> ([f64; 3], [f64; 3]) {
    let corners = line
        .split("gated centres (")
        .nth(1)
        .and_then(|rest| rest.split(") m").next())
        .unwrap_or_else(|| panic!("gated centre bounds in {line}"));
    let numbers: Vec<f64> = corners
        .replace(")..(", ", ")
        .split(", ")
        .map(|number| number.parse().expect("a coordinate"))
        .collect();
    assert_eq!(numbers.len(), 6, "two corners: {line}");
    (
        [numbers[0], numbers[1], numbers[2]],
        [numbers[3], numbers[4], numbers[5]],
    )
}

/// A row's counts and bounds: the text after its label.
fn row_text(line: &str) -> &str {
    line.split_once(": ")
        .map(|(_, counts)| counts)
        .unwrap_or_else(|| panic!("counts follow the label: {line}"))
}

#[test]
fn the_mask_estimate_is_a_share_of_the_same_prls_lightmap_family_bytes() {
    // A static and an animated light over the plate, so ids 22 and 25 ship.
    let [plate, plate_a, plate_b] = sliding_plate();
    let (logs, prl) = compile_keeping_prl(&room_map(&[
        light([-32, 0, 224]),
        light_with([32, 0, 224], 512, ANIMATED),
        plate,
        plate_a,
        plate_b,
    ]));

    let container = read_container(&mut Cursor::new(&prl)).expect("read PRL container");
    let packed = |id: SectionId| {
        container
            .find_section(id as u32)
            .map_or(0, |section| section.size)
    };
    let (lightmap, weights) = (
        packed(SectionId::Lightmap),
        packed(SectionId::AnimatedLightWeightMaps),
    );
    assert!(lightmap > 0 && weights > 0, "ids 22 and 25 ship");
    let family = lightmap + weights + packed(SectionId::ShadowmaskAtlas);

    let line = summary_line(&logs, "  mask est.: ");
    assert_eq!(number_before(line, " B lightmap family"), family, "{line}");
    let share = |bytes: u64| format!("{:.2}%", bytes as f64 * 100.0 / family as f64);
    let every_gated = number_before(line, " B every gated");
    let differing_only = number_before(line, " B differing only");
    assert!(every_gated > 0, "{line}");
    for (bytes, label) in [
        (every_gated, "every gated"),
        (differing_only, "differing only"),
    ] {
        assert!(
            line.contains(&format!("{bytes} B {label} ({})", share(bytes))),
            "{line}"
        );
    }

    // The block's estimates total the debug rows'.
    let rows: Vec<RowCounts> = logs
        .lines()
        .filter(|line| line.starts_with("[DEBUG") && line.contains(" gated texel(s) est."))
        .map(row_counts)
        .collect();
    assert!(!rows.is_empty(), "{logs}");
    assert_eq!(
        rows.iter().map(|row| row.every_gated_bytes).sum::<u64>(),
        every_gated
    );
    assert_eq!(
        rows.iter().map(|row| row.differing_only_bytes).sum::<u64>(),
        differing_only
    );
}

#[test]
fn a_level_without_movers_prints_only_the_zero_mover_summary_line() {
    let logs = compile(&room_map(&[light([0, 0, 224])]));
    let summary: Vec<&str> = logs
        .lines()
        .filter(|line| {
            line.starts_with(SUMMARY)
                || line.starts_with("  gated est.: ")
                || line.starts_with("  side count: ")
                || line.starts_with("  mask est.: ")
        })
        .collect();
    assert_eq!(summary, ["Mover lighting: 0 movers"], "{logs}");
    let gate_lines: Vec<&str> = logs
        .lines()
        .filter(|line| line.contains("[MoverLight]"))
        .collect();
    assert!(
        gate_lines.is_empty(),
        "no gate line at info or debug:\n{}",
        gate_lines.join("\n")
    );
}

/// A cage open to +x: a back wall at −x, two side walls and a roof, each
/// its own brush, around an interior x −96..64, y −80..80, z 0..144.
fn cage() -> Vec<String> {
    vec![
        box_brush([-112, -80, 0], [-96, 80, 144]),
        box_brush([-112, 80, 0], [64, 96, 160]),
        box_brush([-112, -96, 0], [64, -80, 160]),
        box_brush([-112, -80, 144], [64, 80, 160]),
    ]
}

#[test]
fn a_hollow_mover_is_posed_as_its_brushes_not_its_hull() {
    // The light sits in the cage, 64 u or more from every brush. Every
    // segment from the room's +x wall, or the floor before it, leaves the
    // cage through its open side under the roof and between the side walls.
    let origin = [0, 0, 64];
    let logs = compile(&room_map(&[
        light(origin),
        mover("cage", "cage_a", &cage()),
        waypoint("cage_a", "cage_b", [-24, 0, 80]),
        waypoint("cage_b", "", [-136, 0, 80]),
    ]));

    let rows = light_rows(
        &logs,
        "kinematic_mover 'cage' at first waypoint 'cage_a'",
        origin,
    );
    assert_eq!(rows.len(), 1, "{logs}");
    assert!(
        !light_inside(rows[0]),
        "a light between the brushes is not inside: {}",
        rows[0]
    );
    assert!(row_counts(rows[0]).gated > 0, "the walls gate: {}", rows[0]);
    // Engine z is −(Quake x): no gated texel lies at Quake x 230 or more,
    // where the open side's segments land; the side walls gate up to x 205.
    let (min, _) = gated_centres(rows[0]);
    assert!(
        min[2] > -230.0 * UNIT,
        "a segment through the open side is not counted: {}",
        rows[0]
    );
}

/// A plate over the floor's centre, `z` its bottom in map units, sliding
/// 192 u along `y_step` from waypoint `{name}_a`.
fn stacked_plate(name: &str, z: i32, y_step: i32) -> [String; 3] {
    let (first, last) = (format!("{name}_a"), format!("{name}_b"));
    [
        mover(name, &first, &[box_brush([-48, -48, z], [48, 48, z + 8])]),
        waypoint(&first, &last, [0, 0, z + 4]),
        waypoint(&last, "", [0, y_step, z + 4]),
    ]
}

#[test]
fn each_mover_is_posed_alone_so_a_texel_behind_two_movers_counts_in_both() {
    // The floor's centre lies behind both plates. Posed together, the upper
    // plate would hide the lower one's floor; posed alone, each counts it.
    let origin = [0, 0, 224];
    let upper = stacked_plate("upper", 160, 192);
    let lower = stacked_plate("lower", 64, -192);
    let mut both = vec![light(origin)];
    both.extend(upper);
    both.extend(lower.clone());
    let mut lower_alone = vec![light(origin)];
    lower_alone.extend(lower);
    let (both, lower_alone) = (compile(&room_map(&both)), compile(&room_map(&lower_alone)));

    let first_row = |logs: &str, name: &str| -> String {
        let rows = light_rows(
            logs,
            &format!("kinematic_mover '{name}' at first waypoint '{name}_a'"),
            origin,
        );
        assert_eq!(rows.len(), 1, "{logs}");
        rows[0].to_string()
    };
    let upper = first_row(&both, "upper");
    let lower = first_row(&both, "lower");
    assert!(row_counts(&lower).gated > 0, "{lower}");
    assert_eq!(
        row_text(&lower),
        row_text(&first_row(&lower_alone, "lower")),
        "the upper plate changes nothing in the lower plate's row"
    );
    // The lower plate's shadow lies inside the upper's, so its gated texels
    // count in both rows.
    let ((upper_min, upper_max), (lower_min, lower_max)) =
        (gated_centres(&upper), gated_centres(&lower));
    for axis in 0..3 {
        assert!(
            upper_min[axis] <= lower_min[axis] && lower_max[axis] <= upper_max[axis],
            "{upper}\n{lower}"
        );
    }
}

/// One build holding two spin-axis movers and an unnamed zero-axis one.
fn spin_logs() -> &'static str {
    static LOGS: OnceLock<String> = OnceLock::new();
    LOGS.get_or_init(|| {
        compile(&room_map(&[
            light([0, 0, 224]),
            // A spin axis with no speed: scripts may still spin it.
            mover_with(
                "\"name\" \"fan\"\n\"spin_axis\" \"0 0 1\"\n",
                "fan_a",
                &[box_brush([-200, -200, 96], [-152, -152, 112])],
            ),
            waypoint("fan_a", "fan_b", [-176, -176, 104]),
            waypoint("fan_b", "", [-176, 0, 104]),
            // A one-waypoint spinner.
            mover_with(
                "\"name\" \"spinner\"\n\"spin_axis\" \"0 0 1\"\n\"spin_speed\" \"90\"\n",
                "spin_hub",
                &[box_brush([152, 152, 96], [200, 200, 112])],
            ),
            waypoint("spin_hub", "", [176, 176, 104]),
            // A zero axis, unnamed, under the light.
            mover_with(
                "\"spin_axis\" \"0 0 0\"\n",
                "still_a",
                &[box_brush([32, -96, 96], [96, -32, 112])],
            ),
            waypoint("still_a", "still_b", [64, -64, 104]),
            waypoint("still_b", "", [64, 128, 104]),
        ]))
    })
}

/// The unnamed mover's label: its brush-bounds centre, Quake (64, −64, 104)
/// in engine metres.
fn unnamed_label() -> String {
    format!(
        "kinematic_mover at ({:.3}, {:.3}, {:.3}) m",
        64.0 * UNIT,
        104.0 * UNIT,
        -64.0 * UNIT
    )
}

#[test]
fn spin_axis_movers_are_not_analysed_and_a_zero_axis_mover_is() {
    let logs = spin_logs();
    for name in ["fan", "spinner"] {
        let reason = format!(
            "[MoverLight] kinematic_mover '{name}' not analysed: spin axis set, so its parked \
             rotation is unknowable at compile time"
        );
        assert!(
            logs.lines()
                .any(|line| line.starts_with("[DEBUG") && line.ends_with(&reason)),
            "{name} is not analysed, with its reason\n{logs}"
        );
    }
    let still = unnamed_label();
    assert!(
        logs.lines().any(|line| line.starts_with("[DEBUG")
            && line.contains(&format!("[MoverLight] {still} side count: "))),
        "the zero-axis mover is analysed\n{logs}"
    );
    assert!(!logs.contains(&format!("{still} not analysed")), "{logs}");
    let summary = summary_line(logs, SUMMARY);
    assert!(
        summary.starts_with("Mover lighting: 1 mover(s) analysed, 2 not analysed;"),
        "{summary}"
    );
}

#[test]
fn an_unnamed_movers_info_line_names_it_by_its_brush_bounds_centre() {
    let logs = spin_logs();
    let label = unnamed_label();
    let info: Vec<&str> = logs
        .lines()
        .filter(|line| line.starts_with("[INFO") && line.contains("[MoverLight]"))
        .filter(|line| line.contains(&format!("{label} at first waypoint 'still_a' gates ")))
        .collect();
    assert_eq!(info.len(), 1, "{logs}");
    assert!(
        info[0].contains(" gates 1 baked light(s): ") && info[0].ends_with(&at_origin([0, 0, 224])),
        "{}",
        info[0]
    );
}

#[test]
fn a_three_waypoint_mover_is_posed_at_its_first_and_last_waypoints_only() {
    // `corner` lights the floor 72 u under it, 96 u at most; only the plate
    // parked at `tour_b`, just above the floor there, lies in its reach.
    let overhead = [0, 0, 224];
    let corner = [-176, -176, 72];
    let plate = || box_brush([-48, -48, 96], [48, 48, 104]);
    let logs = compile(&room_map(&[
        light(overhead),
        light_with(corner, 96, ""),
        mover("tour", "tour_a", &[plate()]),
        waypoint("tour_a", "tour_b", [0, 0, 100]),
        waypoint("tour_b", "tour_c", [-176, -176, 44]),
        waypoint("tour_c", "", [176, 176, 100]),
        // The same plate ending at the middle pose shows that pose gates it.
        mover("control", "control_a", &[plate()]),
        waypoint("control_a", "control_b", [0, 0, 100]),
        waypoint("control_b", "", [-176, -176, 44]),
    ]));

    assert_eq!(
        light_rows(
            &logs,
            "kinematic_mover 'control' at last waypoint 'control_b'",
            corner
        )
        .len(),
        1,
        "parked at the middle pose, the plate gates the corner light\n{logs}"
    );
    let tour_lines: Vec<&str> = logs
        .lines()
        .filter(|line| line.contains("[MoverLight] kinematic_mover 'tour' "))
        .collect();
    assert!(
        !tour_lines
            .iter()
            .any(|line| line.contains(&at_origin(corner)) || line.contains("'tour_b'")),
        "the middle waypoint is never posed:\n{}",
        tour_lines.join("\n")
    );
    for prefix in [
        "kinematic_mover 'tour' at first waypoint 'tour_a'",
        "kinematic_mover 'tour' at last waypoint 'tour_c'",
    ] {
        assert_eq!(light_rows(&logs, prefix, overhead).len(), 1, "{logs}");
    }
}
