//! Every mover fixture packs the parent commit's bytes after the mover
//! channel's bounded bake: each map under `content/dev/maps` holding a
//! `kinematic_mover`, at `low` and `high`, cold, warm over an empty cache and
//! on a full hit, on four workers, against the parent's cold bake. A parent
//! map with no analysed mover compares too, at no extra cost to the proof.
//!
//! Opt-in and `#[ignore]`: it runs a second compiler, and the big maps take
//! hours. A missing baseline fails the test, so a green run always compared.
//! `POSTRETRO_MOVER_PARITY_MAPS` narrows it to a comma-separated list of map
//! stems. Both compilers take `--working-set-max-size 64GiB`: the parent
//! refuses `stress-env-volumes` at the default, and no gate changes a byte.
//!
//! Baseline: `prl-build` at 10df2fcb (1a66523a plus this brief's plan),
//! named by `POSTRETRO_MOVER_BOUNDED_BAKE_PARENT_BIN`. Build it in this
//! checkout, copy it out, then return (`development_guide.md` §Worktree
//! builds):
//!
//! ```text
//! git switch --detach 10df2fcb
//! cargo build --release -p postretro-level-compiler --bin prl-build
//! cp target/release/prl-build "$BASE/prl-build-parent"
//! git switch -
//! POSTRETRO_MOVER_BOUNDED_BAKE_PARENT_BIN="$BASE/prl-build-parent" \
//!   cargo test -p postretro-level-compiler --test mover_bounded_bake_parity -- --ignored --nocapture
//! ```
//!
//! See: context/lib/build_pipeline.md §Compiler pipeline step 12 (Mover probe channel)

mod prl_build_support;

use std::io::Cursor;
use std::path::{Path, PathBuf};
use std::process::Command;

use postretro_level_format::{read_container, read_section_data};
use prl_build_support::{REACH_VERIFY_ENV, prl_build};

const BASELINE_ENV: &str = "POSTRETRO_MOVER_BOUNDED_BAKE_PARENT_BIN";
const MAPS_ENV: &str = "POSTRETRO_MOVER_PARITY_MAPS";
const JOBS: &str = "4";
const BUDGET: &str = "64GiB";

fn workspace_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .expect("workspace root")
        .to_path_buf()
}

fn baseline_compiler() -> PathBuf {
    let path = std::env::var_os(BASELINE_ENV)
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            panic!("set {BASELINE_ENV} to the parent commit's prl-build; the module doc says how")
        });
    assert!(
        path.is_file(),
        "{BASELINE_ENV}: {} is no file",
        path.display()
    );
    path
}

/// The mover fixtures: every dev map holding a `kinematic_mover`, or the
/// stems `POSTRETRO_MOVER_PARITY_MAPS` names.
fn fixtures() -> Vec<PathBuf> {
    let maps = workspace_root().join("content/dev/maps");
    if let Some(names) = std::env::var_os(MAPS_ENV) {
        return names
            .to_string_lossy()
            .split(',')
            .map(|name| maps.join(format!("{}.map", name.trim())))
            .collect();
    }
    let mut found: Vec<PathBuf> = std::fs::read_dir(&maps)
        .expect("read dev maps")
        .map(|entry| entry.expect("dev map").path())
        .filter(|path| path.extension().is_some_and(|ext| ext == "map"))
        .filter(|path| {
            std::fs::read_to_string(path)
                .is_ok_and(|text| text.contains("\"classname\" \"kinematic_mover\""))
        })
        .collect();
    found.sort();
    found
}

/// Bake `map` in place with `compiler` into `out`, its baked texture
/// sidecars beside `out` rather than in the dev tree.
fn bake(compiler: Command, map: &Path, out: &Path, flags: &[&str], cache: Option<&Path>) -> String {
    let mut command = compiler;
    command
        .env("RUST_LOG", "info")
        .arg(map)
        .arg("-o")
        .arg(out)
        .args(["--no-tui", "--jobs", JOBS, "--working-set-max-size", BUDGET])
        .arg("--baked-root")
        .arg(out.with_extension("baked"))
        .args(flags);
    match cache {
        None => {
            command.arg("--no-cache");
        }
        Some(cache) => {
            command
                .env(REACH_VERIFY_ENV, "0")
                .arg("--cache-dir")
                .arg(cache);
        }
    }
    let output = command.output().expect("spawn prl-build");
    let logs = format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        output.status.success(),
        "{} failed on {}\n{logs}",
        command.get_program().to_string_lossy(),
        map.display()
    );
    logs
}

fn sections(prl: &Path) -> Vec<(u32, Vec<u8>)> {
    let bytes = std::fs::read(prl).expect("read PRL");
    let mut cursor = Cursor::new(bytes.as_slice());
    let meta = read_container(&mut cursor).expect("read container");
    meta.sections
        .iter()
        .map(|entry| {
            let data = read_section_data(&mut cursor, &meta, entry.section_id)
                .expect("read section")
                .expect("listed section");
            (entry.section_id, data)
        })
        .collect()
}

/// Assert two PRLs carry the same sections, byte for byte, naming the first
/// that differs.
fn assert_same(parent: &Path, branch: &Path, what: &str) {
    let (parent, branch) = (sections(parent), sections(branch));
    let ids = |sections: &[(u32, Vec<u8>)]| sections.iter().map(|(id, _)| *id).collect::<Vec<_>>();
    assert_eq!(ids(&parent), ids(&branch), "{what}: section lists differ");
    for ((id, parent), (_, branch)) in parent.iter().zip(&branch) {
        assert!(
            parent == branch,
            "{what}: section {id} differs ({} B against {} B, first at byte {:?})",
            parent.len(),
            branch.len(),
            parent.iter().zip(branch).position(|(a, b)| a != b)
        );
    }
}

#[test]
#[ignore = "runs the parent commit's prl-build over every mover fixture (hours with the big maps); set POSTRETRO_MOVER_BOUNDED_BAKE_PARENT_BIN, see the module doc"]
fn mover_bounded_bake_parent_parity() {
    let parent = baseline_compiler();
    for map in fixtures() {
        let stem = map
            .file_stem()
            .expect("map stem")
            .to_string_lossy()
            .into_owned();
        for (fidelity, flags) in [
            ("low", ["--bake-fidelity", "low"]),
            ("high", ["--bake-fidelity", "high"]),
        ] {
            let dir = tempfile::tempdir().expect("bake dir");
            let parent_prl = dir.path().join("parent.prl");
            bake(Command::new(&parent), &map, &parent_prl, &flags, None);
            let cache = tempfile::tempdir().expect("cache");
            for (mode, cache) in [
                ("cold", None),
                ("warm empty", Some(cache.path())),
                ("full hit", Some(cache.path())),
            ] {
                let branch_prl = dir.path().join("branch.prl");
                bake(prl_build(), &map, &branch_prl, &flags, cache);
                assert_same(
                    &parent_prl,
                    &branch_prl,
                    &format!("{stem} {fidelity} {mode}"),
                );
                eprintln!("{stem} {fidelity} {mode}: identical");
            }
        }
    }
}
