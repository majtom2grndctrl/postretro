// Surface radiance cache through the production pipeline: patch domain, cache lifetime, layer keys.
// See: context/lib/build_pipeline.md §Compiler pipeline

use std::path::Path;
use std::sync::Arc;
use std::time::{Duration, Instant};

use glam::Vec3;
use postretro_test_log_capture::LogCapture;

use super::PatchSet;
use super::layer::SH_PATCH_LAYER_STAGE_ID;
use super::layer_set::LAYER_PLAN_DROPPED;
use super::patches::PATCH_SET_DROPPED;
use super::stage::stage_events;
use super::volume::SH_BASE_VOLUME_STAGE_ID;
use crate::cache::CacheTestAccess;
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

/// Compile `map` in process, cold (`--no-cache`) or warm over `cache_dir`;
/// the log lines this thread captured and the stage events it recorded.
fn compile_recorded(
    root: &Path,
    map: &Path,
    cache_dir: Option<&Path>,
) -> (Vec<String>, Vec<String>) {
    let path = |path: &Path| path.to_string_lossy().into_owned();
    let cache_flags = match cache_dir {
        Some(dir) => vec!["--cache-dir".to_owned(), path(dir)],
        None => vec!["--no-cache".to_owned()],
    };
    let args = crate::parse_args_from(
        [
            path(map),
            "-o".to_owned(),
            path(&root.join("out.prl")),
            "--baked-root".to_owned(),
            path(&root.join("baked")),
            "--sh-probe-spacing".to_owned(),
            "2".to_owned(),
            "--lightmap-density".to_owned(),
            "0.25".to_owned(),
        ]
        .into_iter()
        .chain(cache_flags),
    )
    .unwrap();
    let cache = cache_dir.map(|_| crate::construct_stage_cache(&args).expect("a warm build"));
    let logs = LogCapture::start();
    stage_events::take();
    let started = Instant::now();
    let reporter: Arc<dyn Reporter> = Arc::new(StageEntryRecorder(PlainReporter::new(
        started,
        crate::logger::LogSink::default(),
    )));
    crate::pipeline::run(
        &args,
        cache,
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
    let (logs, events) = compile_recorded(root.path(), &map, None);

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

// AC 32 on the hit path, research P26: a warm build whose gathered volume
// hits builds no patch cache, yet still holds the gate's patch set and its
// layer plan. Both drop inside the SH base stage, before the delta SH
// stage begins.
#[test]
fn a_warm_volume_hit_frees_the_gates_patch_set_before_the_delta_stage_begins() {
    let root = tempfile::tempdir().unwrap();
    let map = mover_room(root.path());
    let cache = root.path().join("cache");
    compile_recorded(root.path(), &map, Some(&cache));
    let (logs, events) = compile_recorded(root.path(), &map, Some(&cache));

    assert!(
        logs.iter()
            .any(|line| line.contains("[cache] sh_base_volume hit")),
        "the second build hits the gathered volume: {logs:?}"
    );
    assert!(
        !events.contains(&stage_events::PATCH_CACHE_DROPPED.to_owned()),
        "a volume hit builds no patch cache: {events:?}"
    );
    let at = |event: &str| {
        events
            .iter()
            .position(|e| e == event)
            .unwrap_or_else(|| panic!("no `{event}` in {events:?}"))
    };
    let (sh_begins, delta_begins) = (at("begin SH Bake"), at("begin Delta SH Bake"));
    for marker in [PATCH_SET_DROPPED, LAYER_PLAN_DROPPED] {
        let drops: Vec<usize> = events
            .iter()
            .enumerate()
            .filter(|(_, e)| *e == marker)
            .map(|(position, _)| position)
            .collect();
        assert_eq!(drops.len(), 1, "{marker}: {events:?}");
        assert!(
            sh_begins < drops[0] && drops[0] < delta_begins,
            "{marker}: {events:?}"
        );
    }
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

/// A sealed 256 u room (the embedded-emitter fixture's) holding two
/// lights, at Quake-unit `west` and `centre`.
fn two_light_room(west: [i32; 3], centre: [i32; 3]) -> String {
    use crate::embedded_emitter_probe_tests::box_brush;
    let brushes = [
        box_brush([-16, -16, -16], [272, 272, 0]),
        box_brush([-16, -16, 128], [272, 272, 144]),
        box_brush([-16, -16, 0], [0, 272, 128]),
        box_brush([256, -16, 0], [272, 272, 128]),
        box_brush([0, -16, 0], [256, 0, 128]),
        box_brush([0, 256, 0], [256, 272, 128]),
    ];
    let light = |[x, y, z]: [i32; 3]| {
        format!(
            "{{\n\"classname\" \"light\"\n\"origin\" \"{x} {y} {z}\"\n\"light\" \"300\"\n\
             \"_falloff_range\" \"256\"\n}}"
        )
    };
    [
        format!(
            "{{\n\"classname\" \"worldspawn\"\n{}\n}}",
            brushes.join("\n")
        ),
        "{\n\"classname\" \"player_spawn\"\n\"origin\" \"128 128 24\"\n}".to_owned(),
        light(west),
        light(centre),
    ]
    .join("\n")
        + "\n"
}

/// One warm in-process compile of `map_text` with its stage cache under
/// `root` and `extra` flags: the patch-layer cache accesses it made and
/// the embedded-emitter warnings it logged on this thread.
fn compile_warm(root: &Path, map_text: &str, extra: &[&str]) -> (CacheTestAccess, usize) {
    let (layers, _, warnings) = compile_warm_volume(root, map_text, extra);
    (layers, warnings)
}

/// [`compile_warm`], with the gathered volume's cache accesses beside the
/// layers'.
fn compile_warm_volume(
    root: &Path,
    map_text: &str,
    extra: &[&str],
) -> (CacheTestAccess, CacheTestAccess, usize) {
    let maps = root.join("maps");
    std::fs::create_dir_all(&maps).unwrap();
    std::fs::create_dir_all(root.join("baked/materials")).unwrap();
    let map = maps.join("two_lights.map");
    std::fs::write(&map, map_text).unwrap();
    let path = |path: &Path| path.to_string_lossy().into_owned();
    let args = crate::parse_args_from(
        [
            path(&map),
            "-o".to_owned(),
            path(&root.join("out.prl")),
            "--baked-root".to_owned(),
            path(&root.join("baked")),
            "--cache-dir".to_owned(),
            path(&root.join("cache")),
            "--sh-probe-spacing".to_owned(),
            "4".to_owned(),
            "--lightmap-density".to_owned(),
            "0.25".to_owned(),
        ]
        .into_iter()
        .chain(extra.iter().map(|flag| (*flag).to_owned())),
    )
    .unwrap();
    let cache = crate::construct_stage_cache(&args).expect("a warm build");
    let logs = LogCapture::start();
    let started = Instant::now();
    let reporter: Arc<dyn Reporter> = Arc::new(PlainReporter::new(
        started,
        crate::logger::LogSink::default(),
    ));
    crate::pipeline::run(
        &args,
        Some(cache.clone()),
        started,
        reporter,
        Arc::new(crate::governor::Governor::new(2, false)),
    )
    .expect("the two-light room compiles");
    let warnings = logs
        .records()
        .into_iter()
        .filter(|record| record.message.contains("emitter inside geometry"))
        .count();
    (
        cache.test_access(SH_PATCH_LAYER_STAGE_ID),
        cache.test_access(SH_BASE_VOLUME_STAGE_ID),
        warnings,
    )
}

// AC 12, research P16, P19: each layer key folds only its own light's
// effective embedded flag. Moving the flagged light out of its wall, or
// another light into one, re-shades only the moved light's layer; toggling
// `--no-penumbra-floor` re-shades only the flagged light's; changing
// `--soft-shadow-samples` re-shades none: the gathered volume does not key
// on it and hits, and with the probe spacing moved too (a volume miss) both
// layers hit.
#[test]
fn a_flagged_lights_edits_and_the_floor_toggle_reshade_only_flagged_layers() {
    let root = tempfile::tempdir().unwrap();
    let flagged = two_light_room([6, 128, 64], [128, 128, 64]);
    let reshaded = |access: CacheTestAccess| (access.writes, access.read_hits);

    let (first, warnings) = compile_warm(root.path(), &flagged, &[]);
    assert_eq!(warnings, 1, "the west light is flagged");
    assert_eq!(reshaded(first), (2, 0), "{first:?}");

    let (samples, volume, _) =
        compile_warm_volume(root.path(), &flagged, &["--soft-shadow-samples", "8"]);
    assert_eq!(
        (reshaded(samples), reshaded(volume)),
        ((0, 0), (0, 1)),
        "--soft-shadow-samples: {samples:?} {volume:?}"
    );
    let (samples, volume, _) = compile_warm_volume(
        root.path(),
        &flagged,
        &["--soft-shadow-samples", "8", "--sh-probe-spacing", "3"],
    );
    assert_eq!(
        (reshaded(samples), reshaded(volume)),
        ((0, 2), (1, 0)),
        "--soft-shadow-samples, new spacing: {samples:?} {volume:?}"
    );

    let (floor_off, warnings) = compile_warm(root.path(), &flagged, &["--no-penumbra-floor"]);
    assert_eq!(warnings, 0, "no flag without the floor");
    assert_eq!(
        reshaded(floor_off),
        (1, 1),
        "--no-penumbra-floor: {floor_off:?}"
    );

    let out_of_wall = two_light_room([64, 128, 64], [128, 128, 64]);
    let (moved_out, warnings) = compile_warm(root.path(), &out_of_wall, &[]);
    assert_eq!(warnings, 0);
    assert_eq!(
        reshaded(moved_out),
        (1, 1),
        "out of the wall: {moved_out:?}"
    );

    let into_wall = two_light_room([6, 128, 64], [250, 128, 64]);
    let (moved_in, warnings) = compile_warm(root.path(), &into_wall, &[]);
    assert_eq!(warnings, 2, "both lights are flagged");
    assert_eq!(reshaded(moved_in), (1, 1), "into the wall: {moved_in:?}");
}
