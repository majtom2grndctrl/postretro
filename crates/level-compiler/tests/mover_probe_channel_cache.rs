//! Mover probe channel through the binary: id 57's bytes across cold builds and worker counts, and warm builds over the
//! per-light direct memos and the per-mover bounce memo: a mover edit re-bakes only the
//! lights reaching its old or new pose, no id-41 or id-45 entry, and only that mover's bounce states; a static-light edit only that light's direct
//! entries but every mover's bounce states; an animated-light edit only its id-45 entries; a
//! retexture no direct entry but every bounce state; and every warm PRL equals the edited map's
//! cold PRL. Over one cache, edits and their revert build warm equal to cold, and the revert
//! hits every memo, with id 57's payloads resident or streamed with id 50's cells.
//! See: context/lib/build_pipeline.md §Build Cache, §Compiler pipeline step 12 (Mover probe channel)

mod prl_build_support;

use std::io::Cursor;
use std::path::{Path, PathBuf};
use std::process::Output;
use std::sync::OnceLock;

use postretro_level_format::mover_probe_channel::{
    MoverChannelBlock, MoverLightNamespace, MoverPayloadPlacement, MoverProbeChannelSection,
};
use postretro_level_format::{SectionId, read_container, read_section_data};
use prl_build_support::{REACH_VERIFY_ENV, prl_build};

const TEXTURE: &str = "50-free-textures/concrete_stone_021";
/// The floor top's texture after a retexture: another albedo.
const RETEXTURE: &str = "50-free-textures/concrete_pavement_036";

/// A box whose top face's texture scale is `top_scale`.
fn box_brush(min: [i32; 3], max: [i32; 3], top_scale: u32) -> String {
    box_brush_textured(min, max, top_scale, TEXTURE)
}

/// [`box_brush`] with `top` on its top face.
fn box_brush_textured(min: [i32; 3], max: [i32; 3], top_scale: u32, top: &str) -> String {
    let ([x0, y0, z0], [x1, y1, z1]) = (min, max);
    format!(
        "{{\n\
         ( {x0} 0 0 ) ( {x0} 1 0 ) ( {x0} 0 1 ) {TEXTURE} 0 0 0 1 1\n\
         ( {x1} 0 0 ) ( {x1} 0 1 ) ( {x1} 1 0 ) {TEXTURE} 0 0 0 1 1\n\
         ( 0 {y0} 0 ) ( 0 {y0} 1 ) ( 1 {y0} 0 ) {TEXTURE} 0 0 0 1 1\n\
         ( 0 {y1} 0 ) ( 1 {y1} 0 ) ( 0 {y1} 1 ) {TEXTURE} 0 0 0 1 1\n\
         ( 0 0 {z0} ) ( 1 0 {z0} ) ( 0 1 {z0} ) {TEXTURE} 0 0 0 1 1\n\
         ( 0 0 {z1} ) ( 0 1 {z1} ) ( 1 0 {z1} ) {top} 0 0 0 {top_scale} {top_scale}\n\
         }}"
    )
}

/// What an edit changes in the base map.
#[derive(Clone)]
struct Scene {
    plate_last: [i32; 3],
    lit_color: &'static str,
    floor_top_scale: u32,
    /// The floor top's texture, as the bounce reads its albedo.
    floor_top: &'static str,
    animated_range: u32,
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
            floor_top: TEXTURE,
            animated_range: 256,
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
/// them coloured by the scene, an animated one and a bake-only one; a plate
/// sliding under the lights and a shutter rising beside them. One static
/// light reaches only the plate's first pose.
fn map(scene: &Scene) -> String {
    let world = [
        box_brush_textured(
            [-272, -272, -16],
            [272, 272, 0],
            scene.floor_top_scale,
            scene.floor_top,
        ),
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
        light([96, 128, 208], scene.animated_range, "\"style\" \"1\"\n"),
        light([0, 64, 200], 256, "\"_bake_only\" \"1\"\n"),
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
    build_with(scene, mode, jobs, &[])
}

/// [`build`] with extra `prl-build` flags.
fn build_with(scene: &Scene, mode: CacheMode<'_>, jobs: u32, flags: &[&str]) -> Built {
    let dir = tempfile::tempdir().expect("create build directory");
    let maps = dir.path().join("maps");
    std::fs::create_dir_all(&maps).expect("create maps directory");
    // The dev textures, so a retexture changes the floor's bounce albedo.
    #[cfg(unix)]
    std::os::unix::fs::symlink(
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../content/dev/textures"),
        dir.path().join("textures"),
    )
    .expect("link the dev textures");
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
    warm_over(scene, cache.path())
}

/// Warm-build `scene` over `cache`, then cold-build it; assert the PRLs are
/// byte-identical. The warm log.
fn warm_over(scene: &Scene, cache: &Path) -> String {
    warm_over_with(scene, cache, &[])
}

/// [`warm_over`] with extra `prl-build` flags on both builds.
fn warm_over_with(scene: &Scene, cache: &Path, flags: &[&str]) -> String {
    let warm = build_with(scene, CacheMode::Warm(cache), JOBS, flags);
    let cold = build_with(scene, CacheMode::Cold, JOBS, flags);
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

/// The mover direct memo's `(re-baked, probed)` share from its miss line.
fn miss_share(logs: &str) -> (usize, usize) {
    memo_miss_share(logs, MOVER_DIRECT)
}

/// The per-light memos: the mover direct half, then ids 41 and 45.
const MOVER_DIRECT: &str = "mover_probe_direct";
/// The per-mover bounce memo, and its two movers.
const MOVER_BOUNCE: &str = "mover_probe_bounce";
const MOVERS: usize = 2;

/// The bounce memo's `(re-baked, probed)` movers from its line: a hit line
/// re-bakes none.
fn bounce_share(logs: &str) -> (usize, usize) {
    if logs
        .lines()
        .any(|line| line.contains(&format!("[cache] {MOVER_BOUNCE} hit ({MOVERS} mover(s))")))
    {
        return (0, MOVERS);
    }
    let marker = format!("[cache] {MOVER_BOUNCE} miss (");
    let line = logs
        .lines()
        .find(|line| line.contains(&marker))
        .unwrap_or_else(|| panic!("a {MOVER_BOUNCE} line\n{logs}"));
    let share = &line[line.find(&marker).unwrap() + marker.len()..];
    let mut words = share.split(' ');
    match (
        words.next().and_then(|count| count.parse().ok()),
        words.nth(1).and_then(|count| count.parse().ok()),
    ) {
        (Some(rebaked), Some(probed)) => (rebaked, probed),
        _ => panic!("a miss share in {line}"),
    }
}

/// Whether the per-mover bounce memo line for `mover` reads `outcome`.
fn bounce_memo_line(logs: &str, mover: &str, outcome: &str) -> bool {
    logs.lines().any(|line| {
        line.contains("probe channel bounce memo: mover")
            && line.contains(&format!("'{mover}'"))
            && line.ends_with(outcome)
    })
}
const ENTITY_SHADOW_DIRECT: &str = "direct_sh_delta_light";
const ANIMATED_DIRECT: &str = "animated_direct_sh_delta_light";

/// A per-light memo's `(re-baked, probed)` share from its miss line.
fn memo_miss_share(logs: &str, stage: &str) -> (usize, usize) {
    let marker = format!("[cache] {stage} miss (");
    let line = logs
        .lines()
        .find(|line| line.contains(&marker))
        .unwrap_or_else(|| panic!("a {stage} miss line\n{logs}"));
    let share = &line[line.find(&marker).unwrap() + marker.len()..];
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
    base_probed_by(MOVER_DIRECT)
}

/// A memo's probed light count in the base build, from its fill line.
fn base_probed_by(stage: &str) -> usize {
    let (rebaked, probed) = memo_miss_share(&base().1.logs, stage);
    assert_eq!(rebaked, probed, "the fill build misses every {stage} light");
    probed
}

/// Every light of `stage`'s memo hit: nothing behind that section traced.
fn assert_all_hit(logs: &str, stage: &str) {
    assert_logged(
        logs,
        &format!("[cache] {stage} hit ({} light(s))", base_probed_by(stage)),
    );
}

// Direct half: id 57 is byte-identical across two cold builds
// and across one and four workers; every namespace carries entries, and the
// runtime loader accepts the section.
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
    assert!(
        !channel.bounce_entries.is_empty() && !channel.depth_records.is_empty(),
        "the section carries bounce and depth blocks too"
    );
    for namespace in [
        MoverLightNamespace::StaticLight,
        MoverLightNamespace::AnimatedBaked,
        MoverLightNamespace::BakeOnlyStaticLight,
    ] {
        assert!(
            channel
                .direct_records
                .iter()
                .any(|record| record.namespace == namespace),
            "a record in {namespace:?}"
        );
    }
    assert!(
        channel
            .direct_records
            .iter()
            .filter(|record| record.namespace == MoverLightNamespace::BakeOnlyStaticLight)
            .all(|record| record.light_index == 0 && record.carries_own_tile),
        "the one bake-only light is roster index 0 and carries its own tile"
    );
    let world = postretro_level_loader::load_prl(first.prl.to_str().expect("UTF-8 path"))
        .expect("the runtime loads the packed channel");
    assert_eq!(world.mover_probe_channel(), Some(&channel));
}

// Direct half: a map with no mover packs no id 57.
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

// Direct half: moving the plate's last waypoint traces nothing
// for ids 34, 35, 41 and 45, re-bakes every light's entries but the one
// reaching only the plate's first pose, and packs the edited map's cold bytes.
#[test]
fn moving_a_movers_last_waypoint_rebakes_only_the_lights_reaching_its_poses() {
    let warm = warm_equals_cold(&Scene {
        plate_last: [0, 128, 104],
        ..Scene::default()
    });
    assert_logged(&warm, "[cache] sh_base_volume hit");
    assert_logged(&warm, "[cache] direct_sh_volume hit");
    assert_all_hit(&warm, ENTITY_SHADOW_DIRECT);
    assert_all_hit(&warm, ANIMATED_DIRECT);
    assert_eq!(
        miss_share(&warm),
        (base_probed() - 1, base_probed()),
        "every light but the first-pose one re-bakes\n{warm}"
    );
    // Bounce half: only the plate's states re-bake, from the cached layers.
    assert_eq!(bounce_share(&warm), (1, MOVERS), "{warm}");
    assert!(bounce_memo_line(&warm, "plate", "miss"), "{warm}");
    assert!(bounce_memo_line(&warm, "shutter", "hit"), "{warm}");
}

// Direct half: editing one static light's colour re-bakes no
// other light's entries, in id 57 or id 41, and no id-45 entry.
#[test]
fn editing_one_static_lights_colour_rebakes_only_its_entries() {
    let warm = warm_equals_cold(&Scene {
        lit_color: "255 128 64",
        ..Scene::default()
    });
    assert_eq!(miss_share(&warm), (1, base_probed()), "{warm}");
    assert_eq!(
        bounce_share(&warm),
        (MOVERS, MOVERS),
        "every mover's bounce re-bakes"
    );
    assert!(
        base_probed_by(ENTITY_SHADOW_DIRECT) > 1,
        "the fixture selects more than one entity-shadow light"
    );
    assert_eq!(
        memo_miss_share(&warm, ENTITY_SHADOW_DIRECT),
        (1, base_probed_by(ENTITY_SHADOW_DIRECT)),
        "{warm}"
    );
    assert_all_hit(&warm, ANIMATED_DIRECT);
}

// Editing the animated light's reach re-bakes its id-45 entries and no id-41
// entry.
#[test]
fn editing_the_animated_light_rebakes_only_its_animated_direct_entries() {
    let warm = warm_equals_cold(&Scene {
        animated_range: 240,
        ..Scene::default()
    });
    assert_eq!(
        memo_miss_share(&warm, ANIMATED_DIRECT),
        (1, base_probed_by(ANIMATED_DIRECT)),
        "{warm}"
    );
    assert_all_hit(&warm, ENTITY_SHADOW_DIRECT);
}

// Direct half: retexturing a static surface moves nothing the
// direct half, id 41 or id 45 traces, so every light hits.
#[test]
fn retexturing_a_static_surface_rebakes_no_direct_entry() {
    let warm = warm_equals_cold(&Scene {
        floor_top_scale: 2,
        ..Scene::default()
    });
    for stage in [MOVER_DIRECT, ENTITY_SHADOW_DIRECT, ANIMATED_DIRECT] {
        assert_all_hit(&warm, stage);
    }
}

// Direct half: duplicating a static light, appended so no
// other light's seed shifts, re-bakes only the duplicate's entries, in id 57
// and id 41.
#[test]
fn duplicating_a_static_light_rebakes_only_the_duplicate() {
    let warm = warm_equals_cold(&Scene {
        appended_light: true,
        ..Scene::default()
    });
    assert_eq!(miss_share(&warm), (1, base_probed() + 1), "{warm}");
    assert_eq!(
        memo_miss_share(&warm, ENTITY_SHADOW_DIRECT),
        (1, base_probed_by(ENTITY_SHADOW_DIRECT) + 1),
        "{warm}"
    );
    assert_all_hit(&warm, ANIMATED_DIRECT);
    assert_eq!(
        bounce_share(&warm),
        (MOVERS, MOVERS),
        "every mover's bounce re-bakes"
    );
}

// Bounce half: retexturing a static surface changes the
// bounce albedo id 34 reads, so every mover's bounce states re-bake, while
// no direct entry does.
#[test]
fn retexturing_a_static_surface_rebakes_every_movers_bounce() {
    let warm = warm_equals_cold(&Scene {
        floor_top: RETEXTURE,
        ..Scene::default()
    });
    assert_logged(&warm, "[cache] sh_base_volume miss");
    assert_eq!(bounce_share(&warm), (MOVERS, MOVERS), "{warm}");
    for stage in [MOVER_DIRECT, ENTITY_SHADOW_DIRECT, ANIMATED_DIRECT] {
        assert_all_hit(&warm, stage);
    }
}

// Over one cache, a mover edit, then a static-light edit
// on top of it, then reverting both, each build warm equal to cold; the
// revert hits every direct, bounce and id-34 memo.
#[test]
fn edits_and_their_revert_over_one_cache_build_warm_equal_to_cold() {
    let cache = tempfile::tempdir().expect("create edit-order cache");
    copy_dir(base().0.path(), cache.path());
    let moved = Scene {
        plate_last: [0, 128, 104],
        ..Scene::default()
    };
    let warm = warm_over(&moved, cache.path());
    assert_eq!(bounce_share(&warm), (1, MOVERS), "{warm}");
    let relit = Scene {
        lit_color: "255 128 64",
        ..moved
    };
    let warm = warm_over(&relit, cache.path());
    assert_eq!(bounce_share(&warm), (MOVERS, MOVERS), "{warm}");
    let warm = warm_over(&Scene::default(), cache.path());
    assert_logged(&warm, "[cache] sh_base_volume hit");
    assert_logged(&warm, "[cache] direct_sh_volume hit");
    for stage in [MOVER_DIRECT, ENTITY_SHADOW_DIRECT, ANIMATED_DIRECT] {
        assert_all_hit(&warm, stage);
    }
    assert_eq!(
        bounce_share(&warm),
        (0, MOVERS),
        "the revert hits every bounce memo\n{warm}"
    );
}

// With streamed payloads: with the resident bar at zero
// both of id 57's payloads stream with id 50's cells; over a cache that build
// filled, a mover edit, then its revert, each build warm equal to cold, and the
// revert hits every direct and bounce memo.
#[test]
fn a_streamed_channel_builds_warm_equal_to_cold() {
    const STREAM_BOTH: &[&str] = &["--mover-payload-resident-bar", "0"];
    let cache = tempfile::tempdir().expect("create streamed cache");
    let filled = build_with(
        &Scene::default(),
        CacheMode::Fill(cache.path()),
        JOBS,
        STREAM_BOTH,
    );
    let channel = MoverProbeChannelSection::from_bytes(
        &section(&filled.prl, SectionId::MoverProbeChannel).expect("id 57"),
    )
    .expect("id 57 decodes");
    for block in MoverChannelBlock::BOTH {
        assert_eq!(
            channel.placement(block),
            MoverPayloadPlacement::Streamed,
            "{block}"
        );
    }
    let moved = Scene {
        plate_last: [0, 128, 104],
        ..Scene::default()
    };
    let warm = warm_over_with(&moved, cache.path(), STREAM_BOTH);
    assert_eq!(bounce_share(&warm), (1, MOVERS), "{warm}");
    let warm = warm_over_with(&Scene::default(), cache.path(), STREAM_BOTH);
    for stage in [MOVER_DIRECT, ENTITY_SHADOW_DIRECT, ANIMATED_DIRECT] {
        assert_all_hit(&warm, stage);
    }
    assert_eq!(bounce_share(&warm), (0, MOVERS), "{warm}");
}
