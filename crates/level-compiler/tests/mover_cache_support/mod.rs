// The mover probe channel's cache fixture: a sealed room with a sliding
// plate and a rising shutter, built through `prl-build` cold, filling a
// cache or warm over one, and the memo lines its logs carry.
// See: context/lib/build_pipeline.md §Build Cache, §Compiler pipeline step 12 (Mover probe channel)

#![allow(dead_code)] // Each suite uses a different subset.

use std::io::Cursor;
use std::path::{Path, PathBuf};
use std::process::Output;
use std::sync::OnceLock;

use postretro_level_format::{SectionId, read_container, read_section_data};

pub use crate::prl_build_support::copy_dir;
use crate::prl_build_support::{
    BuildDir, REACH_VERIFY_ENV, fixture_dir, link_dev_textures, prl_build,
};

pub const TEXTURE: &str = "50-free-textures/concrete_stone_021";
/// The floor top's texture after a retexture: another albedo.
pub const RETEXTURE: &str = "50-free-textures/concrete_pavement_036";

/// A box whose top face's texture scale is `top_scale`.
pub fn box_brush(min: [i32; 3], max: [i32; 3], top_scale: u32) -> String {
    box_brush_textured(min, max, top_scale, TEXTURE)
}

/// [`box_brush`] with `top` on its top face.
pub fn box_brush_textured(min: [i32; 3], max: [i32; 3], top_scale: u32, top: &str) -> String {
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
pub struct Scene {
    pub plate_last: [i32; 3],
    pub lit_color: &'static str,
    pub floor_top_scale: u32,
    /// The floor top's texture, as the bounce reads its albedo.
    pub floor_top: &'static str,
    pub animated_range: u32,
    /// A copy of the first static light appended after every other entity,
    /// so no other light's seed or record index shifts.
    pub appended_light: bool,
    pub movers: bool,
    /// The plate's `light_ramp_start` and `light_ramp_end`, when authored.
    pub plate_ramp: Option<(&'static str, &'static str)>,
    /// The plate's `light_poses` and `light_bounces`, when authored.
    pub plate_poses: Option<&'static str>,
    pub plate_bounces: Option<&'static str>,
    /// The shutter's `light_bounces`, when authored.
    pub shutter_bounces: Option<&'static str>,
    /// A waypoint between the shutter's two, when its path has three.
    pub shutter_middle: Option<[i32; 3]>,
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
            plate_ramp: None,
            plate_poses: None,
            plate_bounces: None,
            shutter_bounces: None,
            shutter_middle: None,
        }
    }
}

pub fn light(origin: [i32; 3], range: u32, extra: &str) -> String {
    format!(
        "{{\n\"classname\" \"light\"\n\"origin\" \"{} {} {}\"\n\"light\" \"300\"\n\
         \"_falloff_range\" \"{range}\"\n\"_light_size\" \"4\"\n{extra}}}",
        origin[0], origin[1], origin[2]
    )
}

pub fn mover(name: &str, path: &str, brush: String) -> String {
    mover_with(name, path, "", brush)
}

/// [`mover`] with `extra` keys.
pub fn mover_with(name: &str, path: &str, extra: &str, brush: String) -> String {
    format!(
        "{{\n\"classname\" \"kinematic_mover\"\n\"name\" \"{name}\"\n\"path\" \"{path}\"\n\
         \"speed\" \"2\"\n{extra}{brush}\n}}"
    )
}

pub fn waypoint(name: &str, next: &str, origin: [i32; 3]) -> String {
    format!(
        "{{\n\"classname\" \"kinematic_waypoint\"\n\"name\" \"{name}\"\n\"next\" \"{next}\"\n\
         \"origin\" \"{} {} {}\"\n}}",
        origin[0], origin[1], origin[2]
    )
}

/// A light 96 u from the plate's first pose and at least 143 u from every
/// other pose, the plate's last one moved or not; its range is 128 u.
pub const FIRST_POSE_LIGHT: [i32; 3] = [0, -160, 104];

/// A sealed 512 u room, interior z 0 to 256: three static lights, one of
/// them coloured by the scene, an animated one and a bake-only one; a plate
/// sliding under the lights and a shutter rising beside them. One static
/// light reaches only the plate's first pose. Automatic placement gives the
/// plate six even poses and no geometry knot, and the shutter three even
/// poses and six geometry knots, where its slab starts or stops enclosing
/// probes: nine.
pub fn map(scene: &Scene) -> String {
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
        let mut plate_keys = scene.plate_ramp.map_or(String::new(), |(start, end)| {
            format!("\"light_ramp_start\" \"{start}\"\n\"light_ramp_end\" \"{end}\"\n")
        });
        for (key, value) in [
            ("light_poses", scene.plate_poses),
            ("light_bounces", scene.plate_bounces),
        ] {
            if let Some(value) = value {
                plate_keys += &format!("\"{key}\" \"{value}\"\n");
            }
        }
        entities.extend([
            mover_with(
                "plate",
                "plate_a",
                &plate_keys,
                box_brush([-64, -64, 96], [64, 64, 112], 1),
            ),
            waypoint("plate_a", "plate_b", [0, 0, 104]),
            waypoint("plate_b", "", scene.plate_last),
            mover_with(
                "shutter",
                "shutter_a",
                &scene.shutter_bounces.map_or(String::new(), |bounces| {
                    format!("\"light_bounces\" \"{bounces}\"\n")
                }),
                box_brush([128, -96, 16], [144, 96, 160], 1),
            ),
        ]);
        match scene.shutter_middle {
            Some(middle) => entities.extend([
                waypoint("shutter_a", "shutter_m", [136, 0, 88]),
                waypoint("shutter_m", "shutter_b", middle),
            ]),
            None => entities.push(waypoint("shutter_a", "shutter_b", [136, 0, 88])),
        }
        entities.push(waypoint("shutter_b", "", [136, 0, 160]));
    }
    if scene.appended_light {
        entities.push(light([0, 0, 224], 288, ""));
    }
    entities.join("\n") + "\n"
}

/// How a build reads the stage cache.
pub enum CacheMode<'a> {
    Cold,
    /// Fill `dir` from nothing.
    Fill(&'a Path),
    /// Warm from `dir`, reach verify off so the lightmap memos hit too.
    Warm(&'a Path),
}

pub struct Built {
    _dir: BuildDir,
    pub prl: PathBuf,
    pub logs: String,
}

pub fn build(scene: &Scene, mode: CacheMode<'_>, jobs: u32) -> Built {
    build_with(scene, mode, jobs, &[])
}

/// [`build`] with extra `prl-build` flags.
pub fn build_with(scene: &Scene, mode: CacheMode<'_>, jobs: u32, flags: &[&str]) -> Built {
    build_in(BuildDir::temp(), scene, mode, jobs, flags)
}

/// [`build_with`] into `dir`.
pub fn build_in(
    dir: BuildDir,
    scene: &Scene,
    mode: CacheMode<'_>,
    jobs: u32,
    flags: &[&str],
) -> Built {
    let maps = dir.path().join("maps");
    std::fs::create_dir_all(&maps).expect("create maps directory");
    // The dev textures, so a retexture changes the floor's bounce albedo.
    link_dev_textures(dir.path());
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

pub fn section(prl: &Path, id: SectionId) -> Option<Vec<u8>> {
    let bytes = std::fs::read(prl).expect("read the packed PRL");
    let mut cursor = Cursor::new(bytes.as_slice());
    let meta = read_container(&mut cursor).expect("read the container");
    read_section_data(&mut cursor, &meta, id as u32).expect("read the section")
}

pub const JOBS: u32 = 6;

/// The base map's cold build with a cache, at the interactive default
/// (`low`) fidelity: what every warm edit starts from. Its cache directory,
/// then the build.
pub fn base() -> &'static (PathBuf, Built) {
    static BASE: OnceLock<(PathBuf, Built)> = OnceLock::new();
    BASE.get_or_init(|| {
        let cache = fixture_dir("base-cache");
        let built = build_in(
            BuildDir::fixture("base-build"),
            &Scene::default(),
            CacheMode::Fill(&cache),
            JOBS,
            &[],
        );
        (cache, built)
    })
}

/// `--bake-fidelity high`: each mover's own `light_bounces`.
pub const HIGH: &[&str] = &["--bake-fidelity", "high"];

/// [`base`] built `--bake-fidelity high`: its cache holds `high` entries.
pub fn high_base() -> &'static (PathBuf, Built) {
    static BASE: OnceLock<(PathBuf, Built)> = OnceLock::new();
    BASE.get_or_init(|| {
        let cache = fixture_dir("high-base-cache");
        let built = build_in(
            BuildDir::fixture("high-base-build"),
            &Scene::default(),
            CacheMode::Fill(&cache),
            JOBS,
            HIGH,
        );
        (cache, built)
    })
}

/// The whole packed PRL.
pub fn prl_bytes(built: &Built) -> Vec<u8> {
    std::fs::read(&built.prl).expect("read the packed PRL")
}

/// Assert two PRLs are byte-identical, naming the first difference.
pub fn assert_same_prl(a: &Built, b: &Built, what: &str) {
    let (a, b) = (prl_bytes(a), prl_bytes(b));
    assert!(
        a == b,
        "{what}: {} B against {} B, first difference at byte {:?}",
        a.len(),
        b.len(),
        a.iter().zip(&b).position(|(a, b)| a != b)
    );
}

/// Warm-build `scene` over a copy of the base cache, then cold-build it;
/// assert the PRLs are byte-identical. The warm log.
pub fn warm_equals_cold(scene: &Scene) -> String {
    let cache = tempfile::tempdir().expect("create warm cache");
    copy_dir(&base().0, cache.path());
    warm_over(scene, cache.path())
}

/// Warm-build `scene` over `cache`, then cold-build it; assert the PRLs are
/// byte-identical. The warm log.
pub fn warm_over(scene: &Scene, cache: &Path) -> String {
    warm_over_with(scene, cache, &[])
}

/// [`warm_over`] with extra `prl-build` flags on both builds.
pub fn warm_over_with(scene: &Scene, cache: &Path, flags: &[&str]) -> String {
    let warm = build_with(scene, CacheMode::Warm(cache), JOBS, flags);
    let cold = build_with(scene, CacheMode::Cold, JOBS, flags);
    assert_same_prl(
        &warm,
        &cold,
        "the warm PRL equals the edited map's cold PRL",
    );
    assert!(
        section(&cold.prl, SectionId::MoverProbeChannel).is_some(),
        "the edited map still gates lights"
    );
    warm.logs
}

pub fn assert_logged(logs: &str, needle: &str) {
    assert!(
        logs.lines().any(|line| line.contains(needle)),
        "expected a line containing `{needle}`\n{logs}"
    );
}

/// The mover direct memo's `(re-baked, probed)` share from its miss line.
pub fn miss_share(logs: &str) -> (usize, usize) {
    memo_miss_share(logs, MOVER_DIRECT)
}

/// The per-light memos: the mover direct half, then ids 41 and 45.
pub const MOVER_DIRECT: &str = "mover_probe_direct";
/// The per-pose bounce memo, and the fixture's two movers.
pub const MOVER_BOUNCE: &str = "mover_probe_bounce";
pub const MOVERS: usize = 2;

/// The numbers in the bounce memo's `[cache]` line, in order: a hit line's
/// `(poses, movers)`, a miss line's `(re-baked poses, poses, re-baking
/// movers, movers)`.
fn bounce_line_counts(logs: &str) -> (bool, Vec<usize>) {
    let line = logs
        .lines()
        .find(|line| line.contains(&format!("[cache] {MOVER_BOUNCE} ")))
        .unwrap_or_else(|| panic!("a {MOVER_BOUNCE} line\n{logs}"));
    let hit = line.contains(&format!("[cache] {MOVER_BOUNCE} hit ("));
    let counts = line
        .split(|c: char| !c.is_ascii_digit())
        .filter(|word| !word.is_empty())
        .map(|word| word.parse().expect("a count"))
        .collect();
    (hit, counts)
}

/// The bounce memo's `(re-baking, probed)` movers from its line: a hit line
/// re-bakes none.
pub fn bounce_share(logs: &str) -> (usize, usize) {
    match bounce_line_counts(logs) {
        (true, counts) => (0, counts[counts.len() - 1]),
        (false, counts) => (counts[counts.len() - 2], counts[counts.len() - 1]),
    }
}

/// The bounce memo's `(re-baked, probed)` poses from its line: a hit line
/// re-bakes none.
pub fn pose_share(logs: &str) -> (usize, usize) {
    match bounce_line_counts(logs) {
        (true, counts) => (0, counts[counts.len() - 2]),
        (false, counts) => (counts[counts.len() - 4], counts[counts.len() - 3]),
    }
}

/// What the per-mover bounce memo line says for `mover`: `hit (N pose(s))`,
/// or `miss: ...` naming the termini and the poses that re-bake.
pub fn mover_memo(logs: &str, mover: &str) -> String {
    let line = logs
        .lines()
        .find(|line| {
            line.contains("probe channel bounce memo: mover")
                && line.contains(&format!("'{mover}'"))
        })
        .unwrap_or_else(|| panic!("a bounce memo line for {mover}\n{logs}"));
    let at = line.find(") ").expect("the mover id") + 2;
    line[at..].to_string()
}

/// Whether the per-mover bounce memo line for `mover` reads `outcome`
/// (`hit` or `miss`).
pub fn bounce_memo_line(logs: &str, mover: &str, outcome: &str) -> bool {
    mover_memo(logs, mover).starts_with(outcome)
}

/// The near-field probes the re-bake line for `mover` reports.
pub fn near_field_probes(logs: &str, mover: &str) -> usize {
    let line = logs
        .lines()
        .find(|line| {
            line.contains("probe channel bounce: mover")
                && line.contains(&format!("'{mover}'"))
                && line.contains("near-field probe(s)")
        })
        .unwrap_or_else(|| panic!("a re-bake line for {mover}\n{logs}"));
    let end = line.find(" near-field probe(s)").expect("the count");
    let start = line[..end].rfind(' ').expect("a word before the count") + 1;
    line[start..end].parse().expect("a count")
}

pub const ENTITY_SHADOW_DIRECT: &str = "direct_sh_delta_light";
pub const ANIMATED_DIRECT: &str = "animated_direct_sh_delta_light";

/// A per-light memo's `(re-baked, probed)` share from its miss line.
pub fn memo_miss_share(logs: &str, stage: &str) -> (usize, usize) {
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
pub fn base_probed() -> usize {
    base_probed_by(MOVER_DIRECT)
}

/// A memo's probed light count in the base build, from its fill line.
pub fn base_probed_by(stage: &str) -> usize {
    let (rebaked, probed) = memo_miss_share(&base().1.logs, stage);
    assert_eq!(rebaked, probed, "the fill build misses every {stage} light");
    probed
}

/// Every light of `stage`'s memo hit: nothing behind that section traced.
pub fn assert_all_hit(logs: &str, stage: &str) {
    assert_logged(
        logs,
        &format!("[cache] {stage} hit ({} light(s))", base_probed_by(stage)),
    );
}
