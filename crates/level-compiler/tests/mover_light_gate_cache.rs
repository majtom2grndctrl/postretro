//! Mover light gate warm builds and determinism: each edit's warm report equals the edited map's
//! cold report, a mover-only edit traces no lightmap visibility, and the report never depends on
//! the build or the worker count.
//! See: context/lib/build_pipeline.md §Build Cache, §Compiler pipeline (step 12)

mod prl_build_support;

use std::path::{Path, PathBuf};
use std::process::Output;
use std::sync::OnceLock;

use prl_build_support::{REACH_VERIFY_ENV, prl_build};

const TEXTURE: &str = "50-free-textures/concrete_stone_021";

fn box_brush(min: [i32; 3], max: [i32; 3]) -> String {
    box_brush_scaled(min, max, 1)
}

/// A box whose top face's texture scale is `top_scale`.
fn box_brush_scaled(min: [i32; 3], max: [i32; 3], top_scale: u32) -> String {
    let ([x0, y0, z0], [x1, y1, z1]) = (min, max);
    format!(
        "{{\n\
         ( {x0} 0 0 ) ( {x0} 1 0 ) ( {x0} 0 1 ) {TEXTURE} 0 0 0 1 1\n\
         ( {x1} 0 0 ) ( {x1} 0 1 ) ( {x1} 1 0 ) {TEXTURE} 0 0 0 1 1\n\
         ( 0 {y0} 0 ) ( 0 {y0} 1 ) ( 1 {y0} 0 ) {TEXTURE} 0 0 0 1 1\n\
         ( 0 {y1} 0 ) ( 1 {y1} 0 ) ( 0 {y1} 1 ) {TEXTURE} 0 0 0 1 1\n\
         ( 0 0 {z0} ) ( 1 0 {z0} ) ( 0 1 {z0} ) {TEXTURE} 0 0 0 1 1\n\
         ( 0 0 {z1} ) ( 0 1 {z1} ) ( 1 0 {z1} ) {TEXTURE} 0 0 0 {top_scale} {top_scale}\n\
         }}"
    )
}

/// What an edit changes in the base map.
#[derive(Clone)]
struct Scene {
    static_light: [i32; 3],
    animated_light: [i32; 3],
    dynamic_light: [i32; 3],
    plate_name: &'static str,
    plate_last: [i32; 3],
    /// An SDF-shadow light ahead of every other light.
    leading_sdf_light: bool,
    floor_top_scale: u32,
}

impl Default for Scene {
    fn default() -> Self {
        Self {
            static_light: [-160, 96, 200],
            animated_light: [96, 128, 208],
            dynamic_light: [-208, -208, 40],
            plate_name: "plate",
            plate_last: [0, 192, 104],
            leading_sdf_light: false,
            floor_top_scale: 1,
        }
    }
}

fn light(origin: [i32; 3], range: u32, extra: &str) -> String {
    format!(
        "{{\n\"classname\" \"light\"\n\"origin\" \"{} {} {}\"\n\"light\" \"300\"\n\
         \"_falloff_range\" \"{range}\"\n\"_light_size\" \"0\"\n{extra}}}",
        origin[0], origin[1], origin[2]
    )
}

fn mover(name: &str, path: &str, brush: String) -> String {
    format!(
        "{{\n\"classname\" \"kinematic_mover\"\n\"name\" \"{name}\"\n\"path\" \"{path}\"\n\
         \"speed\" \"2\"\n{brush}\n}}"
    )
}

fn waypoint(name: &str, next: &str, origin: [i32; 3]) -> String {
    format!(
        "{{\n\"classname\" \"kinematic_waypoint\"\n\"name\" \"{name}\"\n\"next\" \"{next}\"\n\
         \"origin\" \"{} {} {}\"\n}}",
        origin[0], origin[1], origin[2]
    )
}

/// A sealed 512 u room, interior z 0 to 256: three static lights, an
/// animated one and a dynamic one; a plate sliding under the lights, a
/// shutter rising beside them, and a lift in a corner no baked light reaches.
/// One static light reaches only the plate's first pose.
fn map(scene: &Scene) -> String {
    let world = [
        box_brush_scaled([-272, -272, -16], [272, 272, 0], scene.floor_top_scale),
        box_brush([-272, -272, 256], [272, 272, 272]),
        box_brush([-272, -272, 0], [-256, 272, 256]),
        box_brush([256, -272, 0], [272, 272, 256]),
        box_brush([-256, -272, 0], [256, -256, 256]),
        box_brush([-256, 256, 0], [256, 272, 256]),
    ];
    let mut entities = vec![
        format!("{{\n\"classname\" \"worldspawn\"\n{}\n}}", world.join("\n")),
        "{\n\"classname\" \"player_spawn\"\n\"origin\" \"-192 0 32\"\n}".to_string(),
    ];
    if scene.leading_sdf_light {
        entities.push(light([-96, -96, 200], 288, "\"_shadow_type\" \"sdf\"\n"));
    }
    entities.extend([
        light([0, 0, 224], 288, ""),
        light(scene.static_light, 288, ""),
        light(FIRST_POSE_LIGHT, 128, ""),
        light(scene.animated_light, 256, "\"style\" \"1\"\n"),
        light(scene.dynamic_light, 48, "").replacen("\"light\"\n", "\"light_dynamic\"\n", 1),
        mover(
            scene.plate_name,
            "plate_a",
            box_brush([-64, -64, 96], [64, 64, 112]),
        ),
        waypoint("plate_a", "plate_b", [0, 0, 104]),
        waypoint("plate_b", "", scene.plate_last),
        mover(
            "shutter",
            "shutter_a",
            box_brush([128, -96, 16], [144, 96, 160]),
        ),
        waypoint("shutter_a", "shutter_b", [136, 0, 88]),
        waypoint("shutter_b", "", [136, 0, 160]),
        mover(
            "crate_lift",
            "lift_a",
            box_brush([184, -232, 8], [232, -184, 40]),
        ),
        waypoint("lift_a", "lift_b", [208, -208, 24]),
        waypoint("lift_b", "", [208, -208, 56]),
    ]);
    entities.join("\n") + "\n"
}

/// A light 96 u from the plate's first pose and at least 143 u from every
/// other pose, the plate's last one moved or not; its range is 128 u.
const FIRST_POSE_LIGHT: [i32; 3] = [0, -160, 104];

/// A temp directory whose `maps/` holds the input, so the content root sits
/// inside it.
fn map_dir(map_text: &str) -> (tempfile::TempDir, PathBuf) {
    let dir = tempfile::tempdir().expect("create build directory");
    let maps = dir.path().join("maps");
    std::fs::create_dir_all(&maps).expect("create maps directory");
    let input = maps.join("mover-light-gate-cache.map");
    std::fs::write(&input, map_text).expect("write fixture map");
    (dir, input)
}

/// How a build reads the stage cache.
enum CacheMode<'a> {
    /// `--no-cache`: the cold reference.
    Cold,
    /// Fill `dir` from nothing, verify mode on as in every test bake.
    Fill(&'a Path),
    /// Warm from `dir`. Verify mode off: it reads neither the layer cache nor
    /// the section memo, so a warm build would re-walk.
    Warm(&'a Path),
}

/// Build `map_text` with `-v`, `RUST_LOG=debug` and `flags`; the combined
/// log. The gate never warns.
fn build(map_text: &str, mode: CacheMode<'_>, jobs: u32, flags: &[&str]) -> String {
    let (dir, input) = map_dir(map_text);
    let mut command = prl_build();
    command
        .env("RUST_LOG", "debug")
        .arg(&input)
        .arg("-o")
        .arg(dir.path().join("mover-light-gate-cache.prl"))
        .args(["--no-tui", "-v", "--jobs", &jobs.to_string()])
        .args(["--sh-probe-spacing", "1", "--lightmap-density", "0.25"])
        .args(flags);
    match mode {
        CacheMode::Cold => {
            command.arg("--no-cache");
        }
        CacheMode::Fill(cache) => {
            command.arg("--cache-dir").arg(cache);
        }
        CacheMode::Warm(cache) => {
            command
                .env(REACH_VERIFY_ENV, "0")
                .arg("--cache-dir")
                .arg(cache);
        }
    }
    let output: Output = command.output().expect("spawn prl-build");
    let logs = format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(output.status.success(), "prl-build failed\n{logs}");
    let warned: Vec<&str> = logs
        .lines()
        .filter(|line| line.contains("[MoverLight]"))
        .filter(|line| line.starts_with("[WARN") || line.starts_with("[ERROR"))
        .collect();
    assert!(
        warned.is_empty(),
        "the gate never warns:\n{}",
        warned.join("\n")
    );
    logs
}

const JOBS: u32 = 6;

/// Summary lines of the Build Summary's mover-lighting block.
const SUMMARY_PREFIXES: [&str; 4] = [
    "Mover lighting: ",
    "  gated est.: ",
    "  side count: ",
    "  mask est.: ",
];

/// The report: every `[MoverLight]` line at info and debug, then the Build
/// Summary block.
fn report(logs: &str) -> Vec<String> {
    let lines: Vec<String> = logs
        .lines()
        .filter(|line| line.contains("[MoverLight]"))
        .chain(logs.lines().filter(|line| {
            SUMMARY_PREFIXES
                .iter()
                .any(|prefix| line.starts_with(prefix))
        }))
        .map(str::to_owned)
        .collect();
    assert_eq!(
        lines
            .iter()
            .filter(|line| line.starts_with("Mover lighting: "))
            .count(),
        1,
        "one summary block\n{logs}"
    );
    lines
}

/// The base map's cold build with a cache: the cache every warm edit starts
/// from (copied, so edits never see each other's entries) and its log.
fn base() -> &'static (tempfile::TempDir, String) {
    static BASE: OnceLock<(tempfile::TempDir, String)> = OnceLock::new();
    BASE.get_or_init(|| {
        let cache = tempfile::tempdir().expect("create base cache");
        let logs = build(
            &map(&Scene::default()),
            CacheMode::Fill(cache.path()),
            JOBS,
            &[],
        );
        (cache, logs)
    })
}

fn copy_dir(from: &Path, to: &Path) {
    std::fs::create_dir_all(to).expect("create cache copy");
    for entry in std::fs::read_dir(from).expect("list cache") {
        let entry = entry.expect("cache entry");
        let target = to.join(entry.file_name());
        if entry.file_type().expect("cache entry type").is_dir() {
            copy_dir(&entry.path(), &target);
        } else {
            std::fs::copy(entry.path(), &target).expect("copy cache entry");
        }
    }
}

/// Warm-build `scene` with `flags` over a copy of the base cache, then cold-
/// build it; assert the reports match. The warm log.
fn warm_equals_cold(scene: &Scene, flags: &[&str]) -> String {
    let cache = tempfile::tempdir().expect("create warm cache");
    copy_dir(base().0.path(), cache.path());
    let text = map(scene);
    let warm = build(&text, CacheMode::Warm(cache.path()), JOBS, flags);
    let cold = build(&text, CacheMode::Cold, JOBS, flags);
    assert_eq!(
        report(&warm),
        report(&cold),
        "the warm report equals the edited map's cold report\n--- warm\n{warm}\n--- cold\n{cold}"
    );
    warm
}

fn assert_logged(logs: &str, needle: &str) {
    assert!(
        logs.lines().any(|line| line.contains(needle)),
        "expected a line containing `{needle}`\n{logs}"
    );
}

fn assert_not_logged(logs: &str, needle: &str) {
    let found: Vec<&str> = logs.lines().filter(|line| line.contains(needle)).collect();
    assert!(
        found.is_empty(),
        "no line containing `{needle}`:\n{}",
        found.join("\n")
    );
}

/// A memo's miss share, `(re-gated, probed)`, from its
/// `[cache] <stage> miss (k of N light(s) re-gate)` line.
fn miss_share(logs: &str, stage: &str) -> (usize, usize) {
    let marker = format!("[cache] {stage} miss (");
    let line = logs
        .lines()
        .find(|line| line.contains(&marker))
        .unwrap_or_else(|| panic!("a {stage} miss line\n{logs}"));
    let share = &line[line.find(&marker).unwrap() + marker.len()..];
    let mut words = share.split(' ');
    let regated = words.next().and_then(|count| count.parse().ok());
    let probed = words.nth(1).and_then(|count| count.parse().ok());
    match (regated, probed) {
        (Some(regated), Some(probed)) => (regated, probed),
        _ => panic!("a miss share in {line}"),
    }
}

/// The reach cull's stage line.
fn reach_line(logs: &str) -> &str {
    logs.lines()
        .find(|line| line.contains("[Compiler] Lightmap reach cull:"))
        .unwrap_or_else(|| panic!("a reach stage line\n{logs}"))
}

/// The number of partitions the cold base build walked.
fn base_partitions() -> u64 {
    let line = reach_line(&base().1);
    let head = line
        .split(" partition(s) walked this build")
        .next()
        .filter(|head| head.len() < line.len())
        .unwrap_or_else(|| panic!("partitions walked in {line}"));
    head.rsplit(' ')
        .next()
        .and_then(|count| count.parse().ok())
        .unwrap_or_else(|| panic!("a partition count in {line}"))
}

// Moving one mover's last waypoint re-gates only the lights reaching its old
// or new last pose, from the memos and cached partitions: no lightmap
// visibility traces, and the light reaching only its first pose still hits.
#[test]
fn moving_one_movers_last_waypoint_builds_warm_from_the_memos_without_tracing_lightmap_visibility()
{
    let warm = warm_equals_cold(
        &Scene {
            plate_last: [0, 128, 104],
            ..Scene::default()
        },
        &[],
    );
    assert_logged(&warm, "[cache] lightmap_section hit");
    assert_logged(&warm, "[cache] shadowmask_atlas hit");
    assert_logged(&warm, "[cache] mover_light_texels miss");
    let reach = reach_line(&warm);
    assert!(
        reach.contains("stage served from the lightmap_section memo")
            && reach.contains("no pair walked"),
        "the warm build walks no lightmap pair: {reach}"
    );
    assert_not_logged(&warm, "[cache] lightmap_layer miss");
    for stage in ["mover_light_texels", "mover_light_probes"] {
        let (regated, probed) = miss_share(&warm, stage);
        assert_eq!(
            regated + 1,
            probed,
            "every {stage} light but the first-pose one re-gates\n{warm}"
        );
    }
    assert_ne!(
        report(&warm),
        report(&base().1),
        "moving the plate changes the report"
    );
}

#[test]
fn editing_one_static_light_rebuilds_a_warm_report_equal_to_cold() {
    warm_equals_cold(
        &Scene {
            static_light: [-128, 64, 192],
            ..Scene::default()
        },
        &[],
    );
}

// Moving an animated light leaves the static rows standing; only the
// animated gate reruns.
#[test]
fn moving_one_animated_light_rebuilds_a_warm_report_equal_to_cold() {
    let warm = warm_equals_cold(
        &Scene {
            animated_light: [64, 96, 216],
            ..Scene::default()
        },
        &[],
    );
    assert_logged(&warm, "[cache] mover_light_texels hit");
    assert_logged(&warm, "[cache] mover_light_animated miss");
}

#[test]
fn moving_a_dynamic_light_into_a_movers_swept_bounds_rebuilds_a_warm_report_equal_to_cold() {
    let warm = warm_equals_cold(
        &Scene {
            dynamic_light: [0, 96, 128],
            ..Scene::default()
        },
        &[],
    );
    let side_counts = |logs: &str| {
        logs.lines()
            .filter(|line| line.starts_with("  side count: "))
            .map(str::to_owned)
            .collect::<Vec<_>>()
    };
    assert_ne!(
        side_counts(&warm),
        side_counts(&base().1),
        "the dynamic light now reaches a mover"
    );
    for memo in ["texels", "probes", "animated"] {
        assert_logged(&warm, &format!("[cache] mover_light_{memo} hit"));
    }
}

#[test]
fn adding_an_sdf_shadow_light_rebuilds_a_warm_report_equal_to_cold() {
    warm_equals_cold(
        &Scene {
            leading_sdf_light: true,
            ..Scene::default()
        },
        &[],
    );
}

#[test]
fn renaming_a_mover_rebuilds_a_warm_report_equal_to_cold() {
    let warm = warm_equals_cold(
        &Scene {
            plate_name: "sliding_plate",
            ..Scene::default()
        },
        &[],
    );
    assert_logged(&warm, "kinematic_mover 'sliding_plate'");
    for memo in ["texels", "probes", "animated"] {
        assert_logged(&warm, &format!("[cache] mover_light_{memo} hit"));
    }
}

#[test]
fn toggling_the_penumbra_floor_rebuilds_a_warm_report_equal_to_cold() {
    warm_equals_cold(&Scene::default(), &["--no-penumbra-floor"]);
}

#[test]
fn retexturing_one_face_rebuilds_a_warm_report_equal_to_cold() {
    let warm = warm_equals_cold(
        &Scene {
            floor_top_scale: 2,
            ..Scene::default()
        },
        &[],
    );
    // The floor's texture coordinates are in its lights' layer keys.
    assert_logged(&warm, "[cache] lightmap_section miss");
}

// Only the percentage moves: no gating input changed.
#[test]
fn toggling_uncompressed_irradiance_hits_the_gating_memo_and_moves_only_the_percentage() {
    let warm = warm_equals_cold(&Scene::default(), &["--uncompressed-irradiance"]);
    assert_logged(&warm, "[cache] lightmap_section miss");
    for memo in ["texels", "probes", "animated"] {
        assert_logged(&warm, &format!("[cache] mover_light_{memo} hit"));
    }
    let without_mask_line = |logs: &str| {
        report(logs)
            .into_iter()
            .filter(|line| !line.starts_with("  mask est.: "))
            .collect::<Vec<_>>()
    };
    assert_eq!(without_mask_line(&warm), without_mask_line(&base().1));
    assert_ne!(report(&warm), report(&base().1), "the percentage moves");
}

// A lightmap memo hit with both a shadowmask and a gating miss reads each
// partition once, for both.
#[test]
fn a_mover_edit_and_an_owner_index_shift_read_each_partition_once_for_shadowmask_and_gate() {
    let warm = warm_equals_cold(
        &Scene {
            plate_last: [0, 128, 104],
            leading_sdf_light: true,
            ..Scene::default()
        },
        &[],
    );
    assert_logged(&warm, "[cache] lightmap_section hit");
    assert_logged(&warm, "[cache] shadowmask_atlas miss");
    assert_logged(&warm, "[cache] mover_light_texels miss");
    // Every static light owns, so the union is every partition the cold base
    // build walked; a second consumer reading its own would double it.
    let partitions = base_partitions();
    let reach = reach_line(&warm);
    assert!(
        reach.contains(&format!(
            "the lightmap_section memo and {partitions} cached partition(s), no pair walked"
        )),
        "each of {partitions} partitions read once: {reach}"
    );
    let reads = warm
        .lines()
        .filter(|line| line.contains("[cache] lightmap_layer hit"))
        .count() as u64;
    assert_eq!(reads, partitions, "one layer read per partition");
    assert_not_logged(&warm, "[cache] lightmap_layer miss");
}

// The report is identical across cold builds and worker counts, and a
// (mover, terminus) without rows logs no info line.
#[test]
fn the_report_is_identical_across_cold_builds_and_worker_counts() {
    let text = map(&Scene::default());
    let first = build(&text, CacheMode::Cold, 4, &[]);
    let second = build(&text, CacheMode::Cold, 4, &[]);
    let one_worker = build(&text, CacheMode::Cold, 1, &[]);
    let lines = report(&first);
    assert_eq!(report(&second), lines, "two cold builds");
    assert_eq!(report(&one_worker), lines, "--jobs 1 against --jobs 4");

    let info: Vec<&String> = lines
        .iter()
        .filter(|line| line.starts_with("[INFO"))
        .collect();
    assert!(
        info.len() >= 2,
        "the plate and shutter gate lights\n{first}"
    );
    for line in &info {
        assert!(
            !line.contains("texel(s)") && !line.contains("probe(s)"),
            "per-row counts never appear at info: {line}"
        );
    }
    assert!(
        lines.iter().any(|line| line.starts_with("[DEBUG")
            && line.contains("kinematic_mover 'crate_lift' side count")),
        "the lift is analysed\n{first}"
    );
    assert!(
        !lines
            .iter()
            .any(|line| line.contains("kinematic_mover 'crate_lift' at ")),
        "the lift meets no baked light, so it has no row and no info line\n{first}"
    );
}
