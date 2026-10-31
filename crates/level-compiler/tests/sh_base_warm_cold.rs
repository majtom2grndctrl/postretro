//! The SH base (id 34) through the real compiler CLI: a warm build bakes the
//! bytes a cold build of the same map bakes, across a sequence of edits over
//! one cache directory; a `--release` build caches nothing; an invalid
//! `_bounce_fill` warns once even when the gathered volume hits; and no warm
//! build, help text or context doc calls warm indirect light approximate.
//! See: context/lib/build_pipeline.md §Build Cache

use std::io::Cursor;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

use postretro_level_format::{SectionId, read_container, read_section_data};

const WALL: &str = "test/sh_base_wall";

struct TempBuildDir(PathBuf);

impl TempBuildDir {
    fn new(label: &str) -> Self {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system clock must be after the Unix epoch")
            .as_nanos();
        let path = std::env::temp_dir().join(format!(
            "postretro-sh-base-{label}-{}-{nonce}",
            std::process::id()
        ));
        std::fs::create_dir_all(path.join("maps")).expect("create isolated compiler directory");
        Self(path)
    }

    fn cache(&self) -> PathBuf {
        self.0.join("cache")
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
         ( {x0} 0 0 ) ( {x0} 1 0 ) ( {x0} 0 1 ) {WALL} 0 0 0 1 1\n\
         ( {x1} 0 0 ) ( {x1} 0 1 ) ( {x1} 1 0 ) {WALL} 0 0 0 1 1\n\
         ( 0 {y0} 0 ) ( 0 {y0} 1 ) ( 1 {y0} 0 ) {WALL} 0 0 0 1 1\n\
         ( 0 {y1} 0 ) ( 1 {y1} 0 ) ( 0 {y1} 1 ) {WALL} 0 0 0 1 1\n\
         ( 0 0 {z0} ) ( 1 0 {z0} ) ( 0 1 {z0} ) {WALL} 0 0 0 1 1\n\
         ( 0 0 {z1} ) ( 0 1 {z1} ) ( 1 0 {z1} ) {WALL} 0 0 0 1 1\n\
         }}"
    )
}

/// One `light` entity's key-value lines.
#[derive(Clone)]
struct Light {
    origin: [i32; 3],
    brightness: u32,
    falloff: u32,
    colour: Option<&'static str>,
    style: Option<u32>,
    phase: Option<&'static str>,
}

impl Light {
    fn at(origin: [i32; 3], falloff: u32) -> Self {
        Self {
            origin,
            brightness: 300,
            falloff,
            colour: None,
            style: None,
            phase: None,
        }
    }

    fn entity(&self) -> String {
        let [x, y, z] = self.origin;
        let mut lines = vec![
            "\"classname\" \"light\"".to_owned(),
            format!("\"origin\" \"{x} {y} {z}\""),
            format!("\"light\" \"{}\"", self.brightness),
            format!("\"_falloff_range\" \"{}\"", self.falloff),
            "\"_light_size\" \"4\"".to_owned(),
        ];
        if let Some(colour) = self.colour {
            lines.push(format!("\"_color\" \"{colour}\""));
        }
        if let Some(style) = self.style {
            lines.push(format!("\"style\" \"{style}\""));
        }
        if let Some(phase) = self.phase {
            lines.push(format!("\"_phase\" \"{phase}\""));
        }
        format!("{{\n{}\n}}", lines.join("\n"))
    }
}

/// A sealed corridor 1920 × 128 × 128 units (48.8 m long). The light at its
/// west end reaches 4 m; probes in its east third, past 32 m, stand farther
/// from it than that range plus the retired grouped bake's 16 m reach
/// cutoff, even counting a whole 4-probe group (16 m at 4 m spacing) of
/// slack, so that bake dropped the light there. They gather the light's
/// bounce all the same: later bounces carry it down the corridor
/// (`surface_radiance::bounce_tests`, the same corridor in metres).
#[derive(Clone)]
struct Corridor {
    lights: Vec<Light>,
    worldspawn: Vec<(&'static str, &'static str)>,
    /// A free-standing pillar mid-corridor: a geometry edit.
    pillar: bool,
    /// A brush outside the sealed hull, in the void: no face of it survives
    /// exterior culling.
    void_brush: bool,
    /// A `lightmap_scale_region` over the corridor's west end, at this
    /// `_lightmap_scale`: a chart-density edit only.
    scale_region: Option<&'static str>,
}

impl Corridor {
    fn new() -> Self {
        Self {
            lights: vec![Light::at([64, 64, 64], 160), Light::at([640, 64, 96], 256)],
            worldspawn: Vec::new(),
            pillar: false,
            void_brush: false,
            scale_region: None,
        }
    }

    fn map(&self) -> String {
        let mut brushes = vec![
            box_brush([-16, -16, -16], [1936, 144, 0]),
            box_brush([-16, -16, 128], [1936, 144, 144]),
            box_brush([-16, -16, 0], [0, 144, 128]),
            box_brush([1920, -16, 0], [1936, 144, 128]),
            box_brush([0, -16, 0], [1920, 0, 128]),
            box_brush([0, 128, 0], [1920, 144, 128]),
        ];
        if self.pillar {
            brushes.push(box_brush([900, 48, 0], [932, 80, 128]));
        }
        if self.void_brush {
            brushes.push(box_brush([400, 400, 0], [464, 464, 64]));
        }
        let worldspawn_keys: String = self
            .worldspawn
            .iter()
            .map(|(key, value)| format!("\"{key}\" \"{value}\"\n"))
            .collect();
        let mut entities = vec![
            format!(
                "{{\n\"classname\" \"worldspawn\"\n{worldspawn_keys}{}\n}}",
                brushes.join("\n")
            ),
            "{\n\"classname\" \"player_spawn\"\n\"origin\" \"1800 64 32\"\n}".to_owned(),
        ];
        entities.extend(self.lights.iter().map(Light::entity));
        if let Some(scale) = self.scale_region {
            entities.push(format!(
                "{{\n\"classname\" \"lightmap_scale_region\"\n\"_lightmap_scale\" \"{scale}\"\n{}\n}}",
                box_brush([-32, -32, -32], [400, 160, 160])
            ));
        }
        entities.join("\n") + "\n"
    }
}

/// One build's id-34 bytes and log.
struct Built {
    id_34: Vec<u8>,
    prl: PathBuf,
    log: String,
}

impl Built {
    fn count(&self, needle: &str) -> usize {
        self.log.matches(needle).count()
    }

    /// Warnings logged as they happened: the end-of-build summary repeats
    /// each one, indented, and those copies are not counted.
    fn logged_warnings(&self, needle: &str) -> usize {
        self.log
            .lines()
            .filter(|line| line.starts_with("[WARN") && line.contains(needle))
            .count()
    }
}

/// Build `corridor` into its own output; `cache` names the cache directory
/// for a warm build, `None` builds cold.
fn build(
    temp: &TempBuildDir,
    corridor: &Corridor,
    name: &str,
    cache: Option<&Path>,
    extra: &[&str],
) -> Built {
    let map = temp.0.join("maps").join(format!("{name}.map"));
    std::fs::write(&map, corridor.map()).expect("write fixture map");
    let output_path = temp.0.join(format!("{name}.prl"));
    let mut command = Command::new(env!("CARGO_BIN_EXE_prl-build"));
    command
        .env("RUST_LOG", "info")
        .arg(&map)
        .arg("-o")
        .arg(&output_path)
        .arg("--baked-root")
        .arg(temp.0.join("baked"))
        .args([
            "--no-tui",
            "--sh-probe-spacing",
            "4",
            "--lightmap-density",
            "0.25",
            "--soft-shadow-samples",
            "4",
        ]);
    match cache {
        Some(dir) => command.arg("--cache-dir").arg(dir),
        None => command.arg("--release"),
    };
    let output = command.args(extra).output().expect("spawn prl-build");
    let log = String::from_utf8_lossy(&output.stdout).into_owned()
        + &String::from_utf8_lossy(&output.stderr);
    assert!(
        output.status.success(),
        "prl-build failed for {name}:\n{log}"
    );
    Built {
        id_34: section(&output_path, SectionId::OctahedralShVolume),
        prl: output_path,
        log,
    }
}

fn section(prl: &Path, id: SectionId) -> Vec<u8> {
    let bytes = std::fs::read(prl).expect("read compiled PRL");
    let mut cursor = Cursor::new(bytes);
    let metadata = read_container(&mut cursor).expect("read PRL container");
    read_section_data(&mut cursor, &metadata, id as u32)
        .expect("read section")
        .unwrap_or_else(|| panic!("{id:?} is written"))
}

/// What the retired warm SH warning said; no warm build may log it.
const RETIRED_WARNING: &str = "indirect lighting is APPROXIMATE";

/// Build `corridor` warm into `cache` and cold, and assert the warm id-34
/// bytes are the cold ones. Returns the warm build.
fn warm_equals_cold(
    temp: &TempBuildDir,
    corridor: &Corridor,
    step: &str,
    cache: &Path,
    extra: &[&str],
) -> Built {
    let warm = build(temp, corridor, &format!("{step}-warm"), Some(cache), extra);
    let cold = build(temp, corridor, &format!("{step}-cold"), None, extra);
    assert!(
        warm.id_34 == cold.id_34,
        "{step}: warm id 34 differs from a cold bake of the same map"
    );
    assert!(
        !warm.log.contains(RETIRED_WARNING),
        "{step}: a warm build still calls indirect light approximate:\n{}",
        warm.log
    );
    warm
}

// AC 9, research P2, P9, P12, P13, P18, P20, P21: over one cache directory,
// warm id 34 equals a cold bake of the map after each edit in turn. AC 21:
// no warm build logs the retired approximation warning. AC 15 at the CLI:
// the animated-only edits hit the gathered volume.
#[test]
fn warm_id_34_equals_cold_after_every_edit_over_one_cache() {
    let temp = TempBuildDir::new("sequence");
    let cache = temp.cache();
    let cache = cache.as_path();
    let mut corridor = Corridor::new();
    let base = warm_equals_cold(&temp, &corridor, "base", cache, &[]);
    assert_eq!(base.count("[cache] sh_base_volume miss"), 1);

    corridor.lights[1].brightness = 450;
    let a = corridor.clone();
    warm_equals_cold(&temp, &corridor, "edit-a", cache, &[]);
    corridor.lights[1].origin = [800, 64, 96];
    warm_equals_cold(&temp, &corridor, "edit-b", cache, &[]);
    corridor = a;
    let reverted = warm_equals_cold(&temp, &corridor, "revert-a", cache, &[]);
    assert_eq!(
        reverted.count("[cache] sh_base_volume hit"),
        1,
        "A's volume"
    );

    corridor.lights.remove(0);
    corridor.lights.push(Light::at([320, 64, 32], 192));
    warm_equals_cold(&temp, &corridor, "remove-and-add", cache, &[]);
    corridor.lights.reverse();
    let reordered = warm_equals_cold(&temp, &corridor, "reorder", cache, &[]);
    assert_eq!(reordered.count("[cache] sh_base_volume hit"), 1, "reorder");
    corridor.lights[0].colour = Some("255 128 64");
    warm_equals_cold(&temp, &corridor, "colour", cache, &[]);
    corridor.worldspawn.push(("_bounce_fill", "0.5"));
    corridor.lights[1].brightness = 200;
    let lit = corridor.clone();
    warm_equals_cold(&temp, &corridor, "fill-and-light", cache, &[]);
    corridor.lights.clear();
    warm_equals_cold(&temp, &corridor, "no-static-light", cache, &[]);

    // The geometry and validity edits return to the lit corridor.
    corridor = lit;
    corridor.pillar = true;
    warm_equals_cold(&temp, &corridor, "geometry", cache, &[]);
    corridor.void_brush = true;
    warm_equals_cold(&temp, &corridor, "void-brush", cache, &[]);
    warm_equals_cold(
        &temp,
        &corridor,
        "probe-spacing",
        cache,
        &["--sh-probe-spacing", "3"],
    );

    let mut pulse = Light::at([1000, 64, 64], 192);
    pulse.style = Some(1);
    corridor.lights.insert(1, pulse);
    let added = warm_equals_cold(&temp, &corridor, "animated-added", cache, &[]);
    corridor.lights[1].phase = Some("0.5");
    let retimed = warm_equals_cold(&temp, &corridor, "animated-retimed", cache, &[]);
    corridor.lights.remove(1);
    let removed = warm_equals_cold(&temp, &corridor, "animated-removed", cache, &[]);
    for (step, built) in [
        ("added", &added),
        ("re-timed", &retimed),
        ("removed", &removed),
    ] {
        assert_eq!(
            built.count("[cache] sh_base_volume hit"),
            1,
            "an animated light {step} leaves the gathered volume cached"
        );
        assert_eq!(built.count("[cache] sh_patch_layer miss"), 0, "{step}");
    }
    assert!(
        added.id_34 != retimed.id_34,
        "re-timing moves the descriptors"
    );
}

// AC 17, research P14: `--release` reads and writes no cache, so a warm
// build after only `--release` builds misses every layer and the gathered
// volume, and its id 34 is the `--release` bytes.
#[test]
fn a_warm_build_after_only_release_builds_misses_everything_and_ships_release_bytes() {
    let temp = TempBuildDir::new("release");
    let corridor = Corridor::new();
    let cache = temp.cache();
    let release = |name: &str| {
        build(
            &temp,
            &corridor,
            name,
            None,
            &["--cache-dir", cache.to_str().unwrap()],
        )
    };
    let first = release("release-1");
    release("release-2");
    assert!(
        !cache.exists() || std::fs::read_dir(&cache).unwrap().next().is_none(),
        "--release writes no cache entry"
    );
    let warm = build(&temp, &corridor, "warm", Some(&cache), &[]);
    assert!(warm.id_34 == first.id_34, "warm equals --release");
    assert_eq!(warm.count("[cache] sh_base_volume miss"), 1);
    assert_eq!(warm.count("[cache] sh_patch_layer hit"), 0);
    assert_eq!(warm.count("[cache] sh_patch_layer miss"), 2);
}

// AC 23: two cold bakes that differ only in lightmap density, or only in a
// `_lightmap_scale` region, bake identical id-34 bytes; a warm build after
// either edit hits the gathered volume, so it writes it again no more than
// it re-shades a patch layer.
#[test]
fn lightmap_density_and_scale_region_edits_leave_id_34_and_its_cache_alone() {
    let temp = TempBuildDir::new("density");
    let corridor = Corridor::new();
    let mut scaled = Corridor::new();
    scaled.scale_region = Some("2.0");
    let cache = temp.cache();
    let cold = build(&temp, &corridor, "cold", None, &[]);
    let warm = build(&temp, &corridor, "warm", Some(&cache), &[]);
    assert!(warm.id_34 == cold.id_34);
    for (edit, map, extra) in [
        ("density", &corridor, &["--lightmap-density", "0.125"][..]),
        ("scale region", &scaled, &[][..]),
    ] {
        let edited_cold = build(&temp, map, &format!("{edit}-cold"), None, extra);
        assert!(edited_cold.id_34 == cold.id_34, "{edit}: cold id 34 moved");
        assert!(
            section(&edited_cold.prl, SectionId::Lightmap)
                != section(&cold.prl, SectionId::Lightmap),
            "{edit}: the edit moves the lightmap"
        );
        let edited_warm = build(&temp, map, &format!("{edit}-warm"), Some(&cache), extra);
        assert!(edited_warm.id_34 == cold.id_34, "{edit}: warm id 34 moved");
        assert_eq!(edited_warm.count("[cache] sh_base_volume hit"), 1, "{edit}");
        assert_eq!(
            edited_warm.count("[cache] sh_base_volume miss"),
            0,
            "{edit}"
        );
        assert_eq!(
            edited_warm.count("[cache] sh_patch_layer miss"),
            0,
            "{edit}"
        );
    }
}

/// The fill warning's text, as the parse logs it.
const FILL_WARNING: &str = "worldspawn `_bounce_fill` value";

// AC 19: an invalid `_bounce_fill` bakes the default and warns exactly once
// per compile, cold, warm, and warm with the gathered volume a hit.
#[test]
fn an_invalid_bounce_fill_warns_once_even_when_the_volume_hits() {
    let temp = TempBuildDir::new("fill");
    let mut corridor = Corridor::new();
    corridor.worldspawn.push(("_bounce_fill", "2.5"));
    let cache = temp.cache();
    let cold = build(&temp, &corridor, "cold", None, &[]);
    let first = build(&temp, &corridor, "warm-1", Some(&cache), &[]);
    let hit = build(&temp, &corridor, "warm-2", Some(&cache), &[]);
    assert_eq!(hit.count("[cache] sh_base_volume hit"), 1);
    for (build, built) in [("cold", &cold), ("warm", &first), ("warm hit", &hit)] {
        assert_eq!(
            built.logged_warnings(FILL_WARNING),
            1,
            "{build}:\n{}",
            built.log
        );
    }
    let defaulted = build(&temp, &Corridor::new(), "default", None, &[]);
    assert!(hit.id_34 == defaulted.id_34, "the default fill bakes");
    assert!(cold.id_34 == hit.id_34);
}

fn workspace_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(2)
        .expect("crates/level-compiler sits two below the workspace")
        .to_path_buf()
}

/// `text` from its `## {heading}` line to the next `## ` heading.
fn doc_section<'a>(text: &'a str, heading: &str) -> &'a str {
    let start = text
        .find(&format!("\n## {heading}\n"))
        .unwrap_or_else(|| panic!("no section {heading}"));
    let rest = &text[start + 1..];
    let end = rest[3..].find("\n## ").map_or(rest.len(), |end| end + 3);
    &rest[..end]
}

/// Phrases that describe warm indirect light as approximate, the retired
/// per-probe-group SH entries, or their determinism exemption.
const RETIRED_SH_PHRASES: [&str; 11] = [
    "approximate indirect",
    "approximate warm",
    "benign approximation",
    "dimmer-or-equal",
    "per-probe-group",
    "sh_group",
    "finer-than-whole-volume",
    "indirect sh baked finer than the whole volume",
    "grouped vs monolithic",
    "warning naming `--release`",
    "approximate grouped",
];

// AC 21: `prl-build --help`, `build_pipeline.md` §Build Cache and
// `testing_guide.md` no longer call warm indirect light approximate, name
// per-group SH entries, or exempt them from determinism.
#[test]
fn help_and_context_docs_no_longer_describe_approximate_warm_sh() {
    let help = Command::new(env!("CARGO_BIN_EXE_prl-build"))
        .arg("--help")
        .output()
        .expect("spawn prl-build --help");
    let help =
        String::from_utf8_lossy(&help.stdout).into_owned() + &String::from_utf8_lossy(&help.stderr);
    assert!(help.contains("--release"), "the help text printed:\n{help}");
    let root = workspace_root();
    let read = |path: &str| {
        std::fs::read_to_string(root.join(path)).unwrap_or_else(|_| panic!("read {path}"))
    };
    let build_pipeline = read("context/lib/build_pipeline.md");
    let sources = [
        ("prl-build --help", help.clone()),
        (
            "build_pipeline.md §Build Cache",
            doc_section(&build_pipeline, "Build Cache").to_owned(),
        ),
        ("testing_guide.md", read("context/lib/testing_guide.md")),
    ];
    let mut found = Vec::new();
    for (source, text) in &sources {
        let lower = text.to_lowercase();
        for phrase in RETIRED_SH_PHRASES {
            if lower.contains(phrase) {
                found.push(format!("{source}: \"{phrase}\""));
            }
        }
    }
    if help.to_lowercase().contains("approximate") {
        found.push("prl-build --help: \"approximate\"".to_owned());
    }
    assert!(
        found.is_empty(),
        "retired warm-SH wording remains:\n{}",
        found.join("\n")
    );
}
