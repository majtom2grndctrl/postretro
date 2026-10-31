// Surface radiance cache through the production pipeline: patch domain and cache lifetime.
// See: context/lib/build_pipeline.md §Compiler pipeline

use std::path::Path;
use std::sync::Arc;
use std::time::{Duration, Instant};

use glam::Vec3;
use postretro_test_log_capture::LogCapture;

use super::PatchSet;
use super::stage::stage_events;
use crate::coloured_bounce_test_fixtures::{
    Appearance, mover_entity, room_map, without_fluid, write_texture, write_textures,
};
use crate::pipeline::StageId;
use crate::reporter::{PlainReporter, Reporter, StageProgress};

/// Half the room's interior edge and its height, in metres (128 map units).
const ROOM_HALF_M: f32 = 128.0 * 0.0254;

/// The coloured-bounce room (red west wall, one static light) with a
/// mover in one corner and no fluid: its walls' outer faces are
/// exterior-culled, and the mover's faces are not static world.
fn mover_room(root: &Path) -> std::path::PathBuf {
    write_textures(root);
    write_texture(root, "lift", [40, 160, 40]);
    std::fs::create_dir_all(root.join("maps")).unwrap();
    let map = root.join("maps/mover_room.map");
    let text =
        without_fluid(room_map(Appearance::RedWall, true, false)) + &mover_entity("bounce/lift");
    std::fs::write(&map, text).unwrap();
    map
}

/// The plain reporter, plus a stage-entry marker on the compiling thread.
struct StageEntryRecorder(PlainReporter);

impl Reporter for StageEntryRecorder {
    fn begin_stage(&self, id: StageId) {
        stage_events::record(format!("begin {}", id.label()));
        self.0.begin_stage(id);
    }
    fn begin_background_stage(&self, id: StageId) {
        stage_events::record(format!("begin {}", id.label()));
        self.0.begin_background_stage(id);
    }
    fn declare_progress(&self, id: StageId, progress: StageProgress) {
        self.0.declare_progress(id, progress);
    }
    fn finish_stage(&self, id: StageId) {
        self.0.finish_stage(id);
    }
    fn skip_stage(&self, id: StageId) {
        self.0.skip_stage(id);
    }
    fn finalize(&self, timings: &[(&'static str, Duration)], total: Duration) {
        self.0.finalize(timings, total);
    }
    fn finalize_failure(&self) {
        self.0.finalize_failure();
    }
}

/// Compile `map` cold (`--no-cache`) in process; the log lines this thread
/// captured and the stage events it recorded.
fn compile_cold(root: &Path, map: &Path) -> (Vec<String>, Vec<String>) {
    let path = |path: &Path| path.to_string_lossy().into_owned();
    let args = crate::parse_args_from(
        [
            path(map),
            "-o".to_owned(),
            path(&root.join("out.prl")),
            "--baked-root".to_owned(),
            path(&root.join("baked")),
            "--no-cache".to_owned(),
            "--sh-probe-spacing".to_owned(),
            "2".to_owned(),
            "--lightmap-density".to_owned(),
            "0.25".to_owned(),
        ]
        .into_iter(),
    )
    .unwrap();
    let logs = LogCapture::start();
    stage_events::take();
    let started = Instant::now();
    let reporter: Arc<dyn Reporter> = Arc::new(StageEntryRecorder(PlainReporter::new(
        started,
        crate::logger::LogSink::default(),
    )));
    crate::pipeline::run(
        &args,
        None,
        started,
        reporter,
        Arc::new(crate::governor::Governor::new(2, false)),
    )
    .expect("the mover room compiles");
    let messages = logs.records().into_iter().map(|r| r.message).collect();
    (messages, stage_events::take())
}

// AC 32, research P26: the cold SH base bake frees its patch cache
// (patches, the summed layers, the gather's view of them) inside the SH
// base stage, before the delta SH stage begins.
// AC 33, the production half: the gate's patch set is the one the fixture
// front end builds over the same map's static world geometry.
#[test]
fn the_cold_sh_base_bake_frees_its_patch_cache_before_the_delta_stage_begins() {
    let root = tempfile::tempdir().unwrap();
    let map = mover_room(root.path());
    let (logs, events) = compile_cold(root.path(), &map);

    let at = |event: &str| {
        events
            .iter()
            .position(|e| e == event)
            .unwrap_or_else(|| panic!("no `{event}` in {events:?}"))
    };
    let dropped = at(stage_events::PATCH_CACHE_DROPPED);
    assert_eq!(
        events
            .iter()
            .filter(|e| *e == stage_events::PATCH_CACHE_DROPPED)
            .count(),
        1,
        "{events:?}"
    );
    assert!(at("begin SH Bake") < dropped, "{events:?}");
    assert!(dropped < at("begin Delta SH Bake"), "{events:?}");

    let gate = logs
        .iter()
        .find(|line| line.contains("SH base working-set gate: estimated peak "))
        .expect("the SH base gate logs its prediction");
    let logged: usize = gate
        .split_once("admitted (")
        .and_then(|(_, tail)| tail.split_once(" patches"))
        .map(|(count, _)| count.parse().unwrap())
        .unwrap_or_else(|| panic!("{gate}"));
    let fixture = crate::fixture_pipeline::load_map(&map);
    assert_eq!(logged, PatchSet::build(&fixture.geometry).len());
}

// AC 33: patches tile exactly the static world faces exterior culling
// keeps. The room's walls are slabs whose outer faces the flood culls, and
// a mover stands in one corner: every patch lies on one of the six interior
// planes, the six together cover the interior's area, and nothing lies on
// the mover or an outer face.
#[test]
fn patches_tile_the_interior_and_skip_the_mover_and_exterior_faces() {
    let root = tempfile::tempdir().unwrap();
    let fixture = crate::fixture_pipeline::load_map(&mover_room(root.path()));
    let patches = PatchSet::build(&fixture.geometry);
    assert!(patches.len() > 0);
    let on_plane = |value: f32, plane: f32| (value - plane).abs() < 1.0e-3;
    // Engine Y is up; X and Z span the floor, symmetric about the origin.
    let plane_of = |c: Vec3| {
        if on_plane(c.y, 0.0) {
            Some(0)
        } else if on_plane(c.y, ROOM_HALF_M) {
            Some(1)
        } else if on_plane(c.x, -ROOM_HALF_M) {
            Some(2)
        } else if on_plane(c.x, ROOM_HALF_M) {
            Some(3)
        } else if on_plane(c.z, -ROOM_HALF_M) {
            Some(4)
        } else if on_plane(c.z, ROOM_HALF_M) {
            Some(5)
        } else {
            None
        }
    };
    let mut area = [0.0f64; 6];
    for patch in patches.patches() {
        let c = patch.centroid;
        let inside = c.x.abs() <= ROOM_HALF_M + 1.0e-3
            && c.z.abs() <= ROOM_HALF_M + 1.0e-3
            && (-1.0e-3..=ROOM_HALF_M + 1.0e-3).contains(&c.y);
        assert!(inside, "a patch outside the interior: {c}");
        let plane = plane_of(c).unwrap_or_else(|| panic!("a patch off the interior planes: {c}"));
        area[plane] += f64::from(patch.area);
    }
    let side = f64::from(2.0 * ROOM_HALF_M);
    let height = f64::from(ROOM_HALF_M);
    for (plane, expected) in [side * side, side * side]
        .into_iter()
        .chain([side * height; 4])
        .enumerate()
    {
        assert!(
            (area[plane] - expected).abs() < 1.0e-3 * expected,
            "plane {plane}: {} m² of {expected} m²",
            area[plane]
        );
    }
}
