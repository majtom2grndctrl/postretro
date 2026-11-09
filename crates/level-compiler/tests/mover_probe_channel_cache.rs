//! Mover probe channel, direct half, through the binary: id 57's bytes across cold builds and
//! worker counts (B12), and warm builds over the per-light memo (B15): a mover edit re-bakes only
//! the lights reaching its old or new pose, a static-light edit only that light, a retexture none,
//! and every warm PRL equals the edited map's cold PRL.
//! See: context/lib/build_pipeline.md §Build Cache, §Compiler pipeline step 12 (Mover probe channel)

mod prl_build_support;

use std::io::Cursor;
use std::path::{Path, PathBuf};
use std::process::Output;
use std::sync::OnceLock;

use postretro_level_format::mover_probe_channel::{MoverLightNamespace, MoverProbeChannelSection};
use postretro_level_format::{SectionId, read_container, read_section_data};
use prl_build_support::{REACH_VERIFY_ENV, prl_build};

const TEXTURE: &str = "50-free-textures/concrete_stone_021";

/// A box whose top face's texture scale is `top_scale`.
fn box_brush(min: [i32; 3], max: [i32; 3], top_scale: u32) -> String {
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
    plate_last: [i32; 3],
    lit_color: &'static str,
    floor_top_scale: u32,
    /// A copy of the first static light appended after every other entity,
    /// so no other light's seed or record index shifts.
    appended_light: bool,
    movers: bool,
}

impl Default for Scene {
    fn default() -> Self {
        Self {
            plate_last: [0, 192, 104],
            lit_color: "255 255 255",
            floor_top_scale: 1,
            appended_light: false,
            movers: true,
        }
    }
}

fn light(origin: [i32; 3], range: u32, extra: &str) -> String {
    format!(
        "{{\n\"classname\" \"light\"\n\"origin\" \"{} {} {}\"\n\"light\" \"300\"\n\
         \"_falloff_range\" \"{range}\"\n\"_light_size\" \"4\"\n{extra}}}",
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

/// A light 96 u from the plate's first pose and at least 143 u from every
/// other pose, the plate's last one moved or not; its range is 128 u.
const FIRST_POSE_LIGHT: [i32; 3] = [0, -160, 104];

/// A sealed 512 u room, interior z 0 to 256: three static lights, one of
/// them coloured by the scene, and an animated one; a plate sliding under the
/// lights and a shutter rising beside them. One static light reaches only
/// the plate's first pose.
fn map(scene: &Scene) -> String {
    let world = [
        box_brush([-272, -272, -16], [272, 272, 0], scene.floor_top_scale),
        box_brush([-272, -272, 256], [272, 272, 272], 1),
        box_brush([-272, -272, 0], [-256, 272, 256], 1),
        box_brush([256, -272, 0], [272, 272, 256], 1),
        box_brush([-256, -272, 0], [256, -256, 256], 1),
        box_brush([-256, 256, 0], [256, 272, 256], 1),
    ];
    let mut entities = vec![
        format!("{{\n\"classname\" \"worldspawn\"\n{}\n}}", world.join("\n")),
        "{\n\"classname\" \"player_spawn\"\n\"origin\" \"-192 0 32\"\n}".to_string(),
        light([0, 0, 224], 288, ""),
        light(
            [-160, 96, 200],
            288,
            &format!("\"_color\" \"{}\"\n", scene.lit_color),
        ),
        light(FIRST_POSE_LIGHT, 128, ""),
        light([96, 128, 208], 256, "\"style\" \"1\"\n"),
    ];
    if scene.movers {
        entities.extend([
            mover(
                "plate",
                "plate_a",
                box_brush([-64, -64, 96], [64, 64, 112], 1),
            ),
            waypoint("plate_a", "plate_b", [0, 0, 104]),
            waypoint("plate_b", "", scene.plate_last),
            mover(
                "shutter",
                "shutter_a",
                box_brush([128, -96, 16], [144, 96, 160], 1),
            ),
            waypoint("shutter_a", "shutter_b", [136, 0, 88]),
            waypoint("shutter_b", "", [136, 0, 160]),
        ]);
    }
    if scene.appended_light {
        entities.push(light([0, 0, 224], 288, ""));
    }
    entities.join("\n") + "\n"
}

/// How a build reads the stage cache.
enum CacheMode<'a> {
    Cold,
    /// Fill `dir` from nothing.
    Fill(&'a Path),
    /// Warm from `dir`, reach verify off so the lightmap memos hit too.
    Warm(&'a Path),
}

struct Built {
    _dir: tempfile::TempDir,
    prl: PathBuf,
    logs: String,
}

fn build(scene: &Scene, mode: CacheMode<'_>, jobs: u32) -> Built {
    let dir = tempfile::tempdir().expect("create build directory");
    let maps = dir.path().join("maps");
    std::fs::create_dir_all(&maps).expect("create maps directory");
    let input = maps.join("mover-probe-channel-cache.map");
    std::fs::write(&input, map(scene)).expect("write fixture map");
    let prl = dir.path().join("mover-probe-channel-cache.prl");
    let mut command = prl_build();
    command
        .env("RUST_LOG", "info")
        .arg(&input)
        .arg("-o")
        .arg(&prl)
        .args(["--no-tui", "--jobs", &jobs.to_string()])
        .args(["--sh-probe-spacing", "1", "--lightmap-density", "0.25"]);
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
    Built {
        _dir: dir,
        prl,
        logs,
    }
}

fn section(prl: &Path, id: SectionId) -> Option<Vec<u8>> {
    let bytes = std::fs::read(prl).expect("read the packed PRL");
    let mut cursor = Cursor::new(bytes.as_slice());
    let meta = read_container(&mut cursor).expect("read the container");
    read_section_data(&mut cursor, &meta, id as u32).expect("read the section")
}

const JOBS: u32 = 6;

/// The base map's cold build with a cache: what every warm edit starts from.
fn base() -> &'static (tempfile::TempDir, Built) {
    static BASE: OnceLock<(tempfile::TempDir, Built)> = OnceLock::new();
    BASE.get_or_init(|| {
        let cache = tempfile::tempdir().expect("create base cache");
        let built = build(&Scene::default(), CacheMode::Fill(cache.path()), JOBS);
        (cache, built)
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

/// Warm-build `scene` over a copy of the base cache, then cold-build it;
/// assert the PRLs are byte-identical. The warm log.
fn warm_equals_cold(scene: &Scene) -> String {
    let cache = tempfile::tempdir().expect("create warm cache");
    copy_dir(base().0.path(), cache.path());
    let warm = build(scene, CacheMode::Warm(cache.path()), JOBS);
    let cold = build(scene, CacheMode::Cold, JOBS);
    let (warm_bytes, cold_bytes) = (
        std::fs::read(&warm.prl).expect("read warm PRL"),
        std::fs::read(&cold.prl).expect("read cold PRL"),
    );
    assert!(
        warm_bytes == cold_bytes,
        "the warm PRL equals the edited map's cold PRL: {} B against {} B, first difference \
         at byte {:?}",
        warm_bytes.len(),
        cold_bytes.len(),
        warm_bytes.iter().zip(&cold_bytes).position(|(a, b)| a != b)
    );
    assert!(
        section(&cold.prl, SectionId::MoverProbeChannel).is_some(),
        "the edited map still gates lights"
    );
    warm.logs
}

fn assert_logged(logs: &str, needle: &str) {
    assert!(
        logs.lines().any(|line| line.contains(needle)),
        "expected a line containing `{needle}`\n{logs}"
    );
}

/// The direct memo's `(re-baked, probed)` share from its miss line.
fn miss_share(logs: &str) -> (usize, usize) {
    let marker = "[cache] mover_probe_direct miss (";
    let line = logs
        .lines()
        .find(|line| line.contains(marker))
        .unwrap_or_else(|| panic!("a mover_probe_direct miss line\n{logs}"));
    let share = &line[line.find(marker).unwrap() + marker.len()..];
    let mut words = share.split(' ');
    let rebaked = words.next().and_then(|count| count.parse().ok());
    let probed = words.nth(1).and_then(|count| count.parse().ok());
    match (rebaked, probed) {
        (Some(rebaked), Some(probed)) => (rebaked, probed),
        _ => panic!("a miss share in {line}"),
    }
}

/// The base build's probed light count, from its fill line.
fn base_probed() -> usize {
    let (rebaked, probed) = miss_share(&base().1.logs);
    assert_eq!(rebaked, probed, "the fill build misses every light");
    probed
}

// Bake row B12, direct half: id 57 is byte-identical across two cold builds
// and across one and four workers; both namespaces carry entries.
#[test]
fn the_section_is_identical_across_cold_builds_and_worker_counts() {
    let scene = Scene::default();
    let first = build(&scene, CacheMode::Cold, 4);
    let second = build(&scene, CacheMode::Cold, 4);
    let one_worker = build(&scene, CacheMode::Cold, 1);
    let bytes = section(&first.prl, SectionId::MoverProbeChannel).expect("the movers gate lights");
    assert_eq!(
        section(&second.prl, SectionId::MoverProbeChannel).as_ref(),
        Some(&bytes)
    );
    assert_eq!(
        section(&one_worker.prl, SectionId::MoverProbeChannel).as_ref(),
        Some(&bytes)
    );
    let channel = MoverProbeChannelSection::from_bytes(&bytes).expect("decodes");
    for namespace in [
        MoverLightNamespace::StaticLight,
        MoverLightNamespace::AnimatedBaked,
    ] {
        assert!(
            channel
                .direct_entries
                .iter()
                .any(|entry| entry.namespace == namespace),
            "an entry in {namespace:?}"
        );
    }
}

// Bake row B17, direct half: a map with no mover packs no id 57.
#[test]
fn a_map_without_movers_packs_no_section() {
    let built = build(
        &Scene {
            movers: false,
            ..Scene::default()
        },
        CacheMode::Cold,
        JOBS,
    );
    assert!(section(&built.prl, SectionId::MoverProbeChannel).is_none());
}

// Bake row B15, direct half: moving the plate's last waypoint traces nothing
// for ids 34 and 35, re-bakes every light's entries but the one reaching only
// the plate's first pose, and packs the edited map's cold bytes.
#[test]
fn moving_a_movers_last_waypoint_rebakes_only_the_lights_reaching_its_poses() {
    let warm = warm_equals_cold(&Scene {
        plate_last: [0, 128, 104],
        ..Scene::default()
    });
    assert_logged(&warm, "[cache] sh_base_volume hit");
    assert_logged(&warm, "[cache] direct_sh_volume hit");
    assert_eq!(
        miss_share(&warm),
        (base_probed() - 1, base_probed()),
        "every light but the first-pose one re-bakes\n{warm}"
    );
}

// Bake row B15, direct half: editing one static light's colour re-bakes no
// other light's entries.
#[test]
fn editing_one_static_lights_colour_rebakes_only_its_entries() {
    let warm = warm_equals_cold(&Scene {
        lit_color: "255 128 64",
        ..Scene::default()
    });
    assert_eq!(miss_share(&warm), (1, base_probed()), "{warm}");
}

// Bake row B15, direct half: retexturing a static surface moves nothing the
// direct half traces, so every light hits.
#[test]
fn retexturing_a_static_surface_rebakes_no_direct_entry() {
    let warm = warm_equals_cold(&Scene {
        floor_top_scale: 2,
        ..Scene::default()
    });
    assert_logged(
        &warm,
        &format!(
            "[cache] mover_probe_direct hit ({} light(s))",
            base_probed()
        ),
    );
}

// Bake row B15, direct half: duplicating a static light, appended so no
// other light's seed shifts, re-bakes only the duplicate's entries.
#[test]
fn duplicating_a_static_light_rebakes_only_the_duplicate() {
    let warm = warm_equals_cold(&Scene {
        appended_light: true,
        ..Scene::default()
    });
    assert_eq!(miss_share(&warm), (1, base_probed() + 1), "{warm}");
}
