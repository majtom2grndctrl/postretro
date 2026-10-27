//! Pre-colour section and material-byte oracle, recorded on the indirect-scale commit.
//! See: context/plans/in-progress/coloured-sh-bounce/research.md §Byte oracle

#[path = "../src/coloured_bounce_test_fixtures.rs"]
mod fixtures;

use std::collections::BTreeMap;
use std::io::Cursor;
use std::path::{Path, PathBuf};
use std::process::Command;

use fixtures::Appearance;
use postretro_level_format::animated_direct_sh_delta_volumes::AnimatedDirectShDeltaVolumesSection;
use postretro_level_format::delta_sh_volumes::DeltaShVolumesSection;
use postretro_level_format::direct_sh_delta_volumes::DirectShDeltaVolumesSection;
use postretro_level_format::{read_container, read_section_data};
use serde::{Deserialize, Serialize};

#[derive(Debug, Serialize, Deserialize)]
struct Oracle {
    baseline_revision: String,
    fixtures: BTreeMap<String, FixtureDigests>,
}

#[derive(Debug, PartialEq, Eq, Serialize, Deserialize)]
struct FixtureDigests {
    sections: BTreeMap<u32, String>,
    sidecars: BTreeMap<String, String>,
}

fn baseline_path() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/coloured_bounce_oracle.json")
}

const BASELINE_REVISION: &str = "ecbc27435023931f919484036619f339438f1e30";
const SAVED_COMPILER_SHA256: &str =
    "098a6fc94c21ef020e065806b3a2e9600fccea3208c3c5240dccc6c72a332496";

#[derive(Debug, PartialEq, Eq, Serialize, Deserialize)]
struct Id41Shape {
    section_bytes: usize,
    entries: usize,
    valid_probes: u32,
    cell_levels: [usize; 3],
    payload_halves: usize,
}

#[derive(Debug, Serialize, Deserialize)]
struct UniformId41Oracle {
    baseline_revision: String,
    compiler_sha256: String,
    fixture_map_blake3: String,
    section_blake3: String,
    shape: Id41Shape,
}

fn uniform_baseline_path() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/coloured_bounce_id41_uniform.json")
}

fn uniform_room_map() -> String {
    fixtures::room_map(Appearance::RedWall, true, true).replacen(
        "\"classname\" \"worldspawn\"",
        "\"classname\" \"worldspawn\"\n\"_sh_coarsen\" \"0\"",
        1,
    )
}

fn fixture_map_digest() -> String {
    blake3::hash(uniform_room_map().as_bytes())
        .to_hex()
        .to_string()
}

fn bake(appearance: Appearance, ids: &[u32]) -> FixtureDigests {
    bake_using(
        Path::new(env!("CARGO_BIN_EXE_prl-build")),
        appearance,
        ids,
        false,
    )
    .0
}

fn bake_using(
    binary: &Path,
    appearance: Appearance,
    ids: &[u32],
    uniform: bool,
) -> (FixtureDigests, Option<Id41Shape>) {
    let temp = tempfile::tempdir().expect("isolated fixture build");
    let root = temp.path();
    std::fs::create_dir(root.join("maps")).unwrap();
    fixtures::write_textures(root);
    let map = root.join("maps/coloured-bounce.map");
    let map_text = if uniform {
        uniform_room_map()
    } else {
        fixtures::room_map(appearance, true, true)
    };
    std::fs::write(&map, map_text).unwrap();
    let prl = root.join("room.prl");
    let output = Command::new(binary)
        .env("RUST_LOG", "warn")
        .arg(&map)
        .arg("-o")
        .arg(&prl)
        .arg("--baked-root")
        .arg(root.join("baked"))
        .args([
            "--no-cache",
            "--no-tui",
            "--jobs",
            "1",
            "--lightmap-density",
            "0.25",
            "--sh-probe-spacing",
            "1",
            "--uncompressed-irradiance",
        ])
        .output()
        .expect("spawn prl-build");
    assert!(
        output.status.success(),
        "fixture compile failed\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
    );
    let mut cursor = Cursor::new(std::fs::read(prl).unwrap());
    let metadata = read_container(&mut cursor).unwrap();
    // The approved storage dependency applies to id 41 only.
    for id in [27, 45] {
        if let Some(bytes) = read_section_data(&mut cursor, &metadata, id).unwrap() {
            let levels = if id == 27 {
                DeltaShVolumesSection::from_bytes(&bytes)
                    .unwrap()
                    .cell_levels
            } else {
                AnimatedDirectShDeltaVolumesSection::from_bytes(&bytes)
                    .unwrap()
                    .cell_levels
            };
            assert!(
                levels.iter().all(|&level| level == 0),
                "id {id} must retain uniform L0 storage"
            );
        }
    }
    let shape = read_section_data(&mut cursor, &metadata, 41)
        .unwrap()
        .map(|bytes| {
            let section = DirectShDeltaVolumesSection::from_bytes(&bytes).unwrap();
            let mut cell_levels = [0; 3];
            for &level in &section.cell_levels {
                cell_levels[usize::from(level)] += 1;
            }
            let shape = Id41Shape {
                section_bytes: bytes.len(),
                entries: section.affinity_lights.len(),
                valid_probes: section
                    .valid_probe_masks
                    .iter()
                    .map(|mask| mask.count_ones())
                    .sum(),
                cell_levels,
                payload_halves: section.delta_subblocks.len(),
            };
            eprintln!(
                "[id41 storage] uniform={uniform}, compiler={}, {shape:?}",
                binary.display()
            );
            if uniform {
                assert!(
                    shape.entries > 0 && shape.payload_halves > 0,
                    "uniform oracle must carry actual direct samples"
                );
                assert!(
                    shape.cell_levels[0] > 0 && shape.cell_levels[1..] == [0, 0],
                    "uniform fixture must disable adaptive id41 storage"
                );
            }
            shape
        });
    let sections = ids
        .iter()
        .map(|&id| {
            let bytes = read_section_data(&mut cursor, &metadata, id)
                .unwrap_or_else(|e| panic!("read section {id}: {e}"))
                .unwrap_or_else(|| panic!("fixture must exercise section {id}"));
            assert!(!bytes.is_empty(), "section {id} must carry a payload");
            (id, blake3::hash(&bytes).to_hex().to_string())
        })
        .collect();
    let mut sidecars = BTreeMap::new();
    collect_sidecars(&root.join("baked/materials"), &mut sidecars);
    (FixtureDigests { sections, sidecars }, shape)
}

fn collect_sidecars(directory: &Path, digests: &mut BTreeMap<String, String>) {
    if !directory.exists() {
        return;
    }
    for entry in std::fs::read_dir(directory).unwrap() {
        let path = entry.unwrap().path();
        if path.is_dir() {
            collect_sidecars(&path, digests);
        } else if path.extension().is_some_and(|e| e == "prm") {
            let name = path.file_name().unwrap().to_string_lossy().into_owned();
            let digest = blake3::hash(&std::fs::read(path).unwrap())
                .to_hex()
                .to_string();
            assert!(
                digests.insert(name, digest).is_none(),
                "unique sidecar names"
            );
        }
    }
}

#[test]
#[ignore = "cold fixture byte-oracle regression"]
fn coloured_bounce_preserves_unaffected_section_and_sidecar_bytes() {
    let mut actual = BTreeMap::from([
        (
            "red-wall".to_owned(),
            bake(Appearance::RedWall, &[35, 41, 45, 47, 48]),
        ),
        (
            "unresolved-slots".to_owned(),
            bake(Appearance::Unresolved, &[34, 27, 54]),
        ),
        (
            "unresolved-sources".to_owned(),
            bake(Appearance::MissingSources, &[34, 27, 54]),
        ),
    ]);
    assert!(
        !actual["red-wall"].sidecars.is_empty(),
        "fixture PNGs must produce real sidecars"
    );
    assert!(
        !actual["unresolved-slots"].sidecars.is_empty(),
        "slot-only sources must bake"
    );
    assert!(
        actual["unresolved-sources"].sidecars.is_empty(),
        "absent sources have no sidecar"
    );
    let bytes =
        std::fs::read(baseline_path()).expect("the immutable pre-colour oracle must be present");
    let mut expected: Oracle = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(expected.baseline_revision, BASELINE_REVISION);
    // Preserve the original adaptive digest in its checked-in record. Its direct
    // samples are now guarded at the owner's approved uniform-storage boundary.
    let old_adaptive = expected
        .fixtures
        .get_mut("red-wall")
        .unwrap()
        .sections
        .remove(&41)
        .expect("original adaptive id41 baseline remains recorded");
    let new_adaptive = actual
        .get_mut("red-wall")
        .unwrap()
        .sections
        .remove(&41)
        .unwrap();
    eprintln!("[id41 adaptive digest] pre-colour={old_adaptive}, current={new_adaptive}");
    assert_eq!(
        actual, expected.fixtures,
        "byte oracle from {}",
        expected.baseline_revision
    );
}

/// Explicit recorder: only the saved, proven pre-colour executable may generate
/// the new record. The original adaptive oracle is never written by this target.
#[test]
#[ignore = "opt-in uniform-id41 baseline capture from the saved old compiler"]
fn record_uniform_id41_from_saved_pre_colour_compiler() {
    let Ok(binary) = std::env::var("POSTRETRO_RECORD_COLOURED_BOUNCE_ID41_UNIFORM") else {
        eprintln!("uniform id41 recorder inactive: no saved-compiler path supplied");
        return;
    };
    let workspace = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .parent()
        .unwrap();
    let saved = workspace.join("work/ab/grey/prl-build");
    assert_eq!(
        Path::new(&binary),
        saved,
        "record only from the exact saved pre-colour compiler path"
    );
    let output = Command::new("/usr/bin/shasum")
        .args(["-a", "256"])
        .arg(&saved)
        .output()
        .expect("hash saved compiler");
    assert!(
        output.status.success(),
        "saved compiler hash must be readable"
    );
    let output = String::from_utf8(output.stdout).unwrap();
    assert_eq!(
        output.split_whitespace().next().unwrap(),
        SAVED_COMPILER_SHA256,
        "saved compiler must match the artifact proven at {BASELINE_REVISION}"
    );
    assert!(
        !uniform_baseline_path().exists(),
        "uniform baseline already exists; recorder cannot overwrite it"
    );
    let (digests, shape) = bake_using(&saved, Appearance::RedWall, &[41], true);
    let oracle = UniformId41Oracle {
        baseline_revision: BASELINE_REVISION.into(),
        compiler_sha256: SAVED_COMPILER_SHA256.into(),
        fixture_map_blake3: fixture_map_digest(),
        section_blake3: digests.sections[&41].clone(),
        shape: shape.expect("uniform fixture must emit id41"),
    };
    std::fs::write(
        uniform_baseline_path(),
        serde_json::to_string_pretty(&oracle).unwrap() + "\n",
    )
    .unwrap();
}

#[test]
#[ignore = "cold uniform-storage direct-sample byte-oracle regression"]
fn uniform_id41_preserves_pre_colour_direct_samples() {
    let bytes = std::fs::read(uniform_baseline_path())
        .expect("capture uniform id41 with the saved old compiler first");
    let expected: UniformId41Oracle = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(expected.baseline_revision, BASELINE_REVISION);
    assert_eq!(expected.compiler_sha256, SAVED_COMPILER_SHA256);
    assert_eq!(
        expected.fixture_map_blake3,
        fixture_map_digest(),
        "both compilers must use exactly the same fixture map"
    );
    let (actual, shape) = bake_using(
        Path::new(env!("CARGO_BIN_EXE_prl-build")),
        Appearance::RedWall,
        &[41],
        true,
    );
    assert_eq!(
        actual.sections[&41], expected.section_blake3,
        "uniform id41 direct samples, masks and CSR must remain exact"
    );
    assert_eq!(
        shape.unwrap(),
        expected.shape,
        "uniform direct storage shape must remain exact"
    );
    // Packing follows the legacy exact-zero drop and geometry-validity compaction.
    // Neither reads indirect radiance: dropped raw zeros are not separately hashed.
}
