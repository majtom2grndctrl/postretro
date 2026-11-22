//! Maps with no area light compile byte-identical to the compiler before area
//! lights landed, but for AlphaLights' version word.
//!
//! Opt-in: set `POSTRETRO_AREA_LIGHTS_BASELINE_DIR` to a directory holding
//! `<map>.prl` for each map in [`MAPS`], each compiled by the pre-area-light
//! `prl-build` (merge base `11f27aaa9`) from `content/dev/maps/<map>.map` with
//! exactly [`FLAGS`], a `--baked-root` of its own, and
//! `POSTRETRO_LIGHTMAP_REACH_VERIFY=1`. A data-script map needs that commit's
//! `scripts-build` beside its `prl-build`. This test compiles each map the
//! same way with the current compiler and compares every section.
//!
//! See: context/lib/build_pipeline.md §PRL section IDs, §Compiler pipeline (Area lights)

mod prl_build_support;

use std::collections::BTreeMap;
use std::io::Cursor;
use std::path::{Path, PathBuf};

use postretro_level_format::{SectionId, read_container, read_section_data};

use prl_build_support::{BuildDir, prl_build};

const BASELINE_ENV: &str = "POSTRETRO_AREA_LIGHTS_BASELINE_DIR";

/// Flags both compilers bake with, after `<map> -o <out> --baked-root <dir>`.
const FLAGS: [&str; 2] = ["--no-tui", "--no-cache"];

/// Dev maps without a `light_area`: the gate fixtures, plus maps that bake
/// shadowmask owners, a spilled animated layer, script-animated lights, SDF
/// lights, a mover's probe channel, emissive volumes, fluid reflections,
/// teleporters and a lift.
const MAPS: [&str; 18] = [
    "gate-heavily-lit",
    "soft_shadow_test",
    "test_animated_weight_maps_cap",
    "test_animated_weight_maps_mixed",
    "test_animated_weight_maps_occluded",
    "test_animated_weight_maps_single",
    "shadowmask-groups-capture",
    "specular-shadowmask-capture",
    "animated-layer-spill",
    "anim-demo",
    "sdf-shadow-test",
    "test_mover_light_gate",
    "emissive-volumes-demo",
    "reflection-probes-test",
    "teleporters",
    "kinematic-platform",
    "mover-closet",
    "spawner-test",
];

/// AlphaLights v5 added the area type value; a v4 file is otherwise the same
/// records. The version is the payload's leading `u32`.
const ALPHA_LIGHTS_BEFORE: u32 = 4;
const ALPHA_LIGHTS_AFTER: u32 = 5;

fn workspace_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .expect("workspace root")
        .to_path_buf()
}

/// Each section's table version and payload, by id, and the header version.
fn sections(prl: &Path) -> (u16, BTreeMap<u32, (u16, Vec<u8>)>) {
    let bytes = std::fs::read(prl).unwrap_or_else(|e| panic!("read {}: {e}", prl.display()));
    let mut cursor = Cursor::new(bytes);
    let meta = read_container(&mut cursor).expect("read_container");
    let sections = meta
        .sections
        .iter()
        .map(|entry| {
            let payload = read_section_data(&mut cursor, &meta, entry.section_id)
                .expect("read section")
                .expect("listed section present");
            (entry.section_id, (entry.version, payload))
        })
        .collect();
    (meta.header.version, sections)
}

fn compile(map: &str, out_dir: &Path) -> PathBuf {
    let input = workspace_root().join(format!("content/dev/maps/{map}.map"));
    let source = std::fs::read_to_string(&input).expect("map source");
    assert!(!source.contains("light_area"), "{map} has an area light");
    let output = out_dir.join(format!("{map}.prl"));
    let run = prl_build()
        .arg(&input)
        .arg("-o")
        .arg(&output)
        .arg("--baked-root")
        .arg(out_dir.join(format!("{map}-baked")))
        .args(FLAGS)
        .output()
        .expect("spawn prl-build");
    assert!(
        run.status.success(),
        "prl-build {map} failed:\n{}{}",
        String::from_utf8_lossy(&run.stdout),
        String::from_utf8_lossy(&run.stderr)
    );
    output
}

/// Every difference between the baseline and current bakes of one map, one
/// line each; AlphaLights' version word is the only one expected.
fn differences(before: &Path, after: &Path) -> Vec<String> {
    let (header_before, before) = sections(before);
    let (header_after, after) = sections(after);
    let mut found = Vec::new();
    if header_before != header_after {
        found.push(format!("header version {header_before} -> {header_after}"));
    }
    let ids: std::collections::BTreeSet<u32> = before.keys().chain(after.keys()).copied().collect();
    for id in ids {
        let name = SectionId::from_u32(id).map_or_else(|| id.to_string(), |s| format!("{s:?}"));
        match (before.get(&id), after.get(&id)) {
            (Some(_), None) => found.push(format!("{name}: dropped")),
            (None, Some(_)) => found.push(format!("{name}: added")),
            (Some((v0, p0)), Some((v1, p1))) => {
                if v0 != v1 {
                    found.push(format!("{name}: table version {v0} -> {v1}"));
                }
                if p0 == p1 {
                    continue;
                }
                if id == SectionId::AlphaLights as u32
                    && p0.len() == p1.len()
                    && p0.len() >= 4
                    && p0[4..] == p1[4..]
                {
                    let word = |p: &[u8]| u32::from_le_bytes(p[..4].try_into().unwrap());
                    found.push(format!("{name}: version word {} -> {}", word(p0), word(p1)));
                } else {
                    found.push(format!(
                        "{name}: payload differs ({} -> {} bytes)",
                        p0.len(),
                        p1.len()
                    ));
                }
            }
            (None, None) => unreachable!(),
        }
    }
    found
}

// Acceptance row 39: on a map with no area light, every bake section is
// byte-identical to the build before area lights. AlphaLights' version word
// moves from 4 to 5 and nothing else, and no AreaGlow (id 62) is written.
#[test]
#[ignore = "opt-in comparison: set POSTRETRO_AREA_LIGHTS_BASELINE_DIR to PRLs from the pre-area-light compiler; minutes of bakes"]
fn maps_without_area_lights_match_the_pre_area_light_compiler() {
    let baseline = PathBuf::from(std::env::var(BASELINE_ENV).unwrap_or_else(|_| {
        panic!("set {BASELINE_ENV} to the pre-area-light PRLs (see this file's header)")
    }));
    let out = BuildDir::temp();
    let expected = vec![format!(
        "AlphaLights: version word {ALPHA_LIGHTS_BEFORE} -> {ALPHA_LIGHTS_AFTER}"
    )];
    let mut compared = 0;
    let mut failures = Vec::new();
    for map in MAPS {
        let before = baseline.join(format!("{map}.prl"));
        if !before.exists() {
            eprintln!("{map}: no baseline at {}, skipped", before.display());
            continue;
        }
        let after = compile(map, out.path());
        assert!(
            !sections(&after)
                .1
                .contains_key(&(SectionId::AreaGlow as u32)),
            "{map}: no area glow section"
        );
        let found = differences(&before, &after);
        println!("{map}: {found:?}");
        if found != expected {
            failures.push(format!("{map}: {found:?}"));
        }
        compared += 1;
    }
    assert!(compared > 0, "no baseline PRL in {}", baseline.display());
    assert!(
        failures.is_empty(),
        "sections differ beyond AlphaLights' version word:\n{}",
        failures.join("\n")
    );
}
