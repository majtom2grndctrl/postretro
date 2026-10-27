//! Pre-colour section and material-byte oracle, recorded on the indirect-scale commit.
//! See: context/plans/in-progress/coloured-sh-bounce/research.md §Byte oracle

#[path = "../src/coloured_bounce_test_fixtures.rs"]
mod fixtures;

use std::collections::BTreeMap;
use std::io::Cursor;
use std::path::{Path, PathBuf};
use std::process::Command;

use fixtures::Appearance;
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

fn revision() -> String {
    let output = Command::new("git")
        .args(["rev-parse", "HEAD"])
        .current_dir(env!("CARGO_MANIFEST_DIR"))
        .output()
        .expect("read oracle baseline revision");
    assert!(output.status.success(), "git rev-parse must succeed");
    String::from_utf8(output.stdout).unwrap().trim().to_owned()
}

fn bake(appearance: Appearance, ids: &[u32]) -> FixtureDigests {
    let temp = tempfile::tempdir().expect("isolated fixture build");
    let root = temp.path();
    std::fs::create_dir(root.join("maps")).unwrap();
    fixtures::write_textures(root);
    let map = root.join("maps/coloured-bounce.map");
    std::fs::write(&map, fixtures::room_map(appearance, true, true)).unwrap();
    let prl = root.join("room.prl");
    let output = Command::new(env!("CARGO_BIN_EXE_prl-build"))
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
    FixtureDigests { sections, sidecars }
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

/// Recording is deliberately opt-in and pins the full baseline commit ID.
/// Run before compiler changes, then check in the generated JSON alongside this test:
/// POSTRETRO_RECORD_COLOURED_BOUNCE_ORACLE=$(git rev-parse HEAD) cargo test
/// -p postretro-level-compiler --test coloured_bounce_oracle -- --ignored
#[test]
#[ignore = "cold fixture bakes; record once on indirect-scale commit, then run for regression"]
fn coloured_bounce_preserves_unaffected_section_and_sidecar_bytes() {
    let record_revision = std::env::var("POSTRETRO_RECORD_COLOURED_BOUNCE_ORACLE").ok();
    if let Some(expected) = &record_revision {
        assert_eq!(
            *expected,
            revision(),
            "record only the explicitly selected baseline commit"
        );
        assert!(
            !baseline_path().exists(),
            "remove the old oracle explicitly before re-recording"
        );
    }
    let actual = BTreeMap::from([
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
    if let Some(baseline_revision) = record_revision {
        let oracle = Oracle {
            baseline_revision,
            fixtures: actual,
        };
        std::fs::write(
            baseline_path(),
            serde_json::to_string_pretty(&oracle).unwrap() + "\n",
        )
        .unwrap();
    } else {
        let bytes = std::fs::read(baseline_path()).expect(
            "record the oracle on the indirect-scale commit before testing compiler changes",
        );
        let expected: Oracle = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(
            actual, expected.fixtures,
            "byte oracle from {}",
            expected.baseline_revision
        );
    }
}
