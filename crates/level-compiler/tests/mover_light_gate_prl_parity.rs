//! The mover light gate leaves packed PRLs unchanged: `campaign-test` and `mover-setpiece`
//! compile byte-identical to the branch's base commit.
//!
//! Opt-in and `#[ignore]`: each map is a cold `--release` bake. Unlike the
//! fitting-map comparison in `oversize_lightmap_maps.rs`, a missing baseline
//! fails the test, so a green run always means both maps were compared.
//!
//! Baselines: `<map>.prl` for each of [`PARITY_MAPS`] in the directory named
//! by `POSTRETRO_MOVER_LIGHT_PARITY_BASELINE_DIR`, built at the merge-base
//! with main with exactly the flags [`compile`] passes. Build them in **this
//! checkout**, not a second worktree: `campaign-test` packs its light script's
//! absolute source path, so a baseline built at another path differs by that
//! string. From the workspace root, on a clean tree, with `$BASE` an absolute
//! path outside the checkout:
//!
//! ```text
//! git switch --detach "$(git merge-base HEAD origin/main)"
//! mkdir -p "$BASE"
//! for map in campaign-test mover-setpiece; do
//!   POSTRETRO_LIGHTMAP_REACH_VERIFY=0 cargo run --release -p postretro-level-compiler \
//!     --bin prl-build -- "content/dev/maps/$map.map" -o "$BASE/$map.prl" \
//!     --no-cache --no-tui --jobs 6 --baked-root "$BASE/baked"
//! done
//! git switch -
//! POSTRETRO_MOVER_LIGHT_PARITY_BASELINE_DIR="$BASE" cargo test -p postretro-level-compiler \
//!   --test mover_light_gate_prl_parity -- --ignored
//! ```
//!
//! See: context/lib/build_pipeline.md §Compiler pipeline (step 12)

mod prl_build_support;

use std::path::{Path, PathBuf};
use std::process::Command;

use prl_build_support::REACH_VERIFY_ENV;

const BASELINE_ENV: &str = "POSTRETRO_MOVER_LIGHT_PARITY_BASELINE_DIR";
const PARITY_MAPS: [&str; 2] = ["campaign-test", "mover-setpiece"];
/// Worker count for every parity bake, baseline and branch alike.
const PARITY_JOBS: &str = "6";

fn workspace_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(|crates| crates.parent())
        .expect("workspace root")
        .to_path_buf()
}

/// The baseline PRL of every parity map, or a failure naming each one
/// missing.
fn baselines() -> Vec<(&'static str, PathBuf)> {
    let dir = std::env::var_os(BASELINE_ENV)
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            panic!(
                "set {BASELINE_ENV} to a directory holding {} built at the merge-base with main; \
             this file's module doc gives the commands",
                PARITY_MAPS.map(|map| format!("{map}.prl")).join(" and ")
            )
        });
    let found: Vec<(&str, PathBuf)> = PARITY_MAPS
        .iter()
        .map(|&map| (map, dir.join(format!("{map}.prl"))))
        .collect();
    let missing: Vec<String> = found
        .iter()
        .filter(|(_, path)| !path.is_file())
        .map(|(_, path)| path.display().to_string())
        .collect();
    assert!(
        missing.is_empty(),
        "baseline PRL missing: {}; build each at the merge-base with main as this file's \
         module doc describes",
        missing.join(", ")
    );
    found
}

/// Compile `content/dev/maps/<map>.map` cold, with default bake flags and
/// [`PARITY_JOBS`] workers, into `dir`.
fn compile(map: &str, dir: &Path) -> PathBuf {
    let workspace = workspace_root();
    let output = dir.join(format!("{map}.prl"));
    let cargo = std::env::var("CARGO").unwrap_or_else(|_| "cargo".into());
    let run = Command::new(cargo)
        .args([
            "run",
            "--quiet",
            "--release",
            "-p",
            "postretro-level-compiler",
            "--bin",
            "prl-build",
            "--",
        ])
        .arg(workspace.join(format!("content/dev/maps/{map}.map")))
        .arg("-o")
        .arg(&output)
        .args(["--no-cache", "--no-tui", "--jobs", PARITY_JOBS])
        .arg("--baked-root")
        .arg(dir.join("baked"))
        .env(REACH_VERIFY_ENV, "0")
        .current_dir(&workspace)
        .output()
        .expect("spawn prl-build");
    assert!(
        run.status.success(),
        "prl-build {map} failed\n{}{}",
        String::from_utf8_lossy(&run.stdout),
        String::from_utf8_lossy(&run.stderr)
    );
    output
}

#[test]
#[ignore = "opt-in comparison: set POSTRETRO_MOVER_LIGHT_PARITY_BASELINE_DIR to PRLs built at the merge-base with main; multi-minute release bakes"]
fn campaign_test_and_mover_setpiece_pack_byte_identical_to_the_base_commit() {
    let baselines = baselines();
    let out = tempfile::tempdir().expect("create output directory");
    let mut compared = 0;
    for (map, before) in &baselines {
        let after = compile(map, out.path());
        let (before_bytes, after_bytes) = (
            std::fs::read(before).expect("read baseline PRL"),
            std::fs::read(&after).expect("read compiled PRL"),
        );
        let first_difference = before_bytes
            .iter()
            .zip(&after_bytes)
            .position(|(a, b)| a != b);
        assert!(
            before_bytes == after_bytes,
            "{map}: packed PRL differs from {}: {} B against {} B, first difference at byte {:?}",
            before.display(),
            before_bytes.len(),
            after_bytes.len(),
            first_difference
        );
        compared += 1;
    }
    assert_eq!(compared, PARITY_MAPS.len(), "every parity map compared");
    eprintln!("{compared} maps compared: {}", PARITY_MAPS.join(", "));
}
