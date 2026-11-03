// The coverage proof as the author sees it, through the production pipeline:
// parity spans the proof closes log at info and the map culls; faces removed
// past the sample cap warn, capped, and switch the reach cull off, warm or
// cold, with the cull-off line only while a light could be culled.
// See: context/lib/build_pipeline.md §Compiler pipeline step 12 (Exact skips)

use std::sync::Arc;
use std::time::Instant;

use glam::Vec3;
use postretro_test_log_capture::CapturedRecord;

use super::*;
use crate::geometry::GeometryResult;
use crate::lightmap_reach::test_fixtures::{Wall, two_rooms};
use crate::partition::OPEN_EDGE_SAMPLE_CAP;
use crate::pipeline::face_removal_seam::with_removed_faces;
use crate::pipeline::lightmap_stage::{LAST_REACH_COUNTS, ReachCounts, with_reach_verify};
use crate::reflection_probes::placement_test_fixtures::{UNIT, box_brush, entity};

/// Map-unit corners of the crates on room B's floor, 32 u tall.
const CRATES: [[f64; 2]; 6] = [
    [300.0, 32.0],
    [300.0, 112.0],
    [300.0, 192.0],
    [420.0, 32.0],
    [420.0, 112.0],
    [420.0, 192.0],
];
const CRATE_TOP: f64 = 32.0;

/// The two rooms, `extra` world brushes, a player start and `light` in
/// room A.
fn map_text(extra: &[String], light: &str) -> String {
    let mut world = two_rooms(Wall::Solid);
    world.extend_from_slice(extra);
    [
        entity("worldspawn", "", &world),
        entity("player_spawn", "\"origin\" \"64 128 24\"", &[]),
        light.to_owned(),
    ]
    .join("\n")
        + "\n"
}

/// A point light at room A's centre, reaching room B through the wall.
fn point_light() -> String {
    entity(
        "light",
        "\"origin\" \"128 128 64\"\n\"light\" \"300\"\n\"_falloff_range\" \"240\"",
        &[],
    )
}

fn sun() -> String {
    entity(
        "light_sun",
        "\"origin\" \"128 128 64\"\n\"light\" \"300\"\n\"angles\" \"-60 30 0\"",
        &[],
    )
}

fn crates() -> Vec<String> {
    CRATES
        .iter()
        .map(|&[x, y]| box_brush([x, y, 0.0], [x + 32.0, y + 32.0, CRATE_TOP]))
        .collect()
}

/// Whether `face` is a crate's top: up-facing, at the crates' height.
fn crate_top(geometry: &GeometryResult, face: usize) -> bool {
    let range = geometry.face_index_ranges[face];
    let start = range.index_offset as usize;
    let section = &geometry.geometry;
    let corners: Vec<Vec3> = section.indices[start..start + range.index_count as usize]
        .iter()
        .map(|&i| Vec3::from(section.vertices[i as usize].position))
        .collect();
    let Some(&first) = corners.first() else {
        return false;
    };
    let normal = section.vertices[section.indices[start] as usize].decode_normal();
    normal[1] > 0.99
        && corners
            .iter()
            .all(|c| (f64::from(c.y) - CRATE_TOP * UNIT).abs() < 1.0e-4 && c.y == first.y)
}

/// One in-process compile of `map_text` through the production pipeline,
/// with its stage cache under `root`: every record it logged on this
/// thread, and the reach cull's totals.
fn compile(root: &std::path::Path, map_text: &str) -> (Vec<CapturedRecord>, ReachCounts) {
    let maps = root.join("maps");
    std::fs::create_dir_all(&maps).expect("maps directory");
    std::fs::create_dir_all(root.join("baked").join("materials")).expect("baked root");
    let map = maps.join("reach.map");
    std::fs::write(&map, map_text).expect("write the map");
    let path = |name: &str| root.join(name).to_string_lossy().into_owned();
    let args = crate::parse_args_from(
        [
            map.to_string_lossy().into_owned(),
            "--cache-dir".to_owned(),
            path("cache"),
            "--baked-root".to_owned(),
            path("baked"),
            "-o".to_owned(),
            path("reach.prl"),
            "--sh-probe-spacing".to_owned(),
            "4".to_owned(),
            "--lightmap-density".to_owned(),
            "0.25".to_owned(),
        ]
        .into_iter(),
    )
    .expect("arguments parse");
    let cache = crate::construct_stage_cache(&args);
    let capture = LogCapture::start();
    LAST_REACH_COUNTS.with(|last| last.set(None));
    let started = Instant::now();
    let reporter: Arc<dyn crate::reporter::Reporter> = Arc::new(
        crate::reporter::PlainReporter::new(started, crate::logger::LogSink::default()),
    );
    crate::pipeline::run(
        &args,
        cache,
        started,
        reporter,
        Arc::new(crate::governor::Governor::new(1, false)),
    )
    .expect("the map compiles");
    let counts = LAST_REACH_COUNTS
        .with(|last| last.get())
        .expect("the stage logs its reach line");
    (capture.records(), counts)
}

fn warnings(records: &[CapturedRecord]) -> Vec<&str> {
    records
        .iter()
        .filter(|record| record.level == Level::Warn)
        .map(|record| record.message.as_str())
        .collect()
}

/// The coverage proof's warning lines and the cull-off line, in order.
fn hole_warnings(records: &[CapturedRecord]) -> Vec<String> {
    warnings(records)
        .into_iter()
        .filter(|message| {
            message.contains("Coverage proof")
                || message.contains("hole near")
                || message.contains("reach cull is off")
        })
        .map(str::to_owned)
        .collect()
}

// A box on a crate, set back 0.2 mm on two sides: face extraction drops the
// exposed sub-millimetre strips, so edge parity reports odd-covered spans
// there, while the proof matches each step's edges within τ. The spans log
// at info, no warning names a hole, and the map culls.
#[test]
fn coverage_proof_closes_parity_spans_and_the_map_culls() {
    let root = tempfile::tempdir().expect("temp root");
    let setback = 0.2e-3 / UNIT;
    let steps = [
        box_brush([300.0, 64.0, 0.0], [364.0, 128.0, 32.0]),
        box_brush(
            [300.0 + setback, 64.0 + setback, 32.0],
            [364.0 - setback, 128.0, 48.0],
        ),
    ];
    let (records, counts) = compile(root.path(), &map_text(&steps, &point_light()));
    let info = |part: &str| {
        records
            .iter()
            .any(|record| record.level == Level::Info && record.message.contains(part))
    };
    assert!(info("Watertightness (edge parity):"), "parity spans log");
    assert!(info("odd-covered span near"));
    assert!(info(" 0 unproven span(s)"), "the proof proves every edge");
    let warned: Vec<_> = warnings(&records)
        .into_iter()
        .filter(|message| {
            let message = message.to_lowercase();
            message.contains("hole") || message.contains("watertight")
        })
        .collect();
    assert!(warned.is_empty(), "{warned:?}");
    assert!(!counts.switched_off && counts.culled > 0, "{counts:?}");
}

// Every crate top removed after extraction: the proof finds more holes than
// the sample cap. The map culls nothing; the warning gives the total and
// names capped locations with their brushes, then says the cull is off for
// the whole map, the same on a warm rebuild. Without the removal the map
// culls; with a sun for its only light the holes still warn, and no cull-off
// line appears.
#[test]
fn coverage_proof_holes_past_the_sample_cap_switch_the_cull_off_and_warn() {
    let holed =
        |text: &str, root: &std::path::Path| with_removed_faces(crate_top, || compile(root, text));
    let root = tempfile::tempdir().expect("temp root");
    let map = map_text(&crates(), &point_light());
    let (cold, counts) = holed(&map, root.path());
    assert!(counts.switched_off && counts.culled == 0, "{counts:?}");
    let lines = hole_warnings(&cold);
    let total: usize = lines[0]
        .split("Coverage proof: ")
        .nth(1)
        .and_then(|rest| rest.split(' ').next())
        .and_then(|n| n.parse().ok())
        .unwrap_or_else(|| panic!("the first line gives the total: {lines:?}"));
    assert!(total > OPEN_EDGE_SAMPLE_CAP, "{total} hole(s)");
    assert!(
        lines[0].contains(&format!("showing {OPEN_EDGE_SAMPLE_CAP} of {total}")),
        "{}",
        lines[0]
    );
    let samples: Vec<_> = lines
        .iter()
        .filter(|line| line.contains("hole near"))
        .collect();
    assert_eq!(samples.len(), OPEN_EDGE_SAMPLE_CAP, "{lines:?}");
    assert!(
        samples.iter().all(|line| line.contains("— brush ")),
        "{samples:?}"
    );
    assert!(
        lines
            .last()
            .is_some_and(|line| line.contains("Lightmap reach cull is off for the whole map")),
        "{lines:?}"
    );

    // Warm, with production's walk and caches: the stage is served from the
    // `lightmap_section` memo.
    let (warm, warm_counts) = with_reach_verify(false, || holed(&map, root.path()));
    assert_eq!(hole_warnings(&warm), lines, "a warm rebuild warns the same");
    assert!(
        warm_counts.baked == 0 && warm_counts.walked == 0,
        "{warm_counts:?}"
    );
    assert!(
        warm.iter().any(|record| record.level == Level::Info
            && record.message
                == "[Compiler] Lightmap reach cull: stage served from the lightmap_section \
                    memo, no pair walked; switched off for the whole map by coverage-proof holes"),
        "the warm stage line names the memo that served it"
    );

    let intact = tempfile::tempdir().expect("temp root");
    let (records, counts) = compile(intact.path(), &map);
    assert!(!counts.switched_off && counts.culled > 0, "{counts:?}");
    assert!(hole_warnings(&records).is_empty());

    let sunlit = tempfile::tempdir().expect("temp root");
    let (records, counts) = holed(&map_text(&crates(), &sun()), sunlit.path());
    let lines_sun = hole_warnings(&records);
    assert_eq!(lines_sun.first(), lines.first(), "the hole warning stays");
    assert!(
        !lines_sun
            .iter()
            .any(|line| line.contains("reach cull is off")),
        "{lines_sun:?}"
    );
    assert!(counts.switched_off && counts.culled == 0);
}
