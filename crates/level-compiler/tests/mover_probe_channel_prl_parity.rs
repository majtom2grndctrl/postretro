//! Bake row B17: a map with no movers, and a map whose movers change nothing in the probe data,
//! emit no MoverProbeChannel (id 57) and pack byte-identical to the parent commit's compiler.
//!
//! Opt-in and `#[ignore]`: it runs a second compiler. A missing baseline fails the test, so a
//! green run always means both maps were compared.
//!
//! Baseline: the parent commit's `prl-build`, named by
//! `POSTRETRO_MOVER_CHANNEL_PARITY_BASELINE_BIN`. Both compilers build the same map files from
//! this checkout with the same flags, so no path differs between them. Build the binary at the
//! parent in this checkout, copy it out, then return (`development_guide.md` §Worktree builds):
//!
//! ```text
//! git switch --detach "$PARENT"
//! cargo build --release -p postretro-level-compiler --bin prl-build
//! cp target/release/prl-build "$BASE/prl-build-parent"
//! git switch -
//! POSTRETRO_MOVER_CHANNEL_PARITY_BASELINE_BIN="$BASE/prl-build-parent" \
//!   cargo test -p postretro-level-compiler --test mover_probe_channel_prl_parity -- --ignored
//! ```
//!
//! The no-change map is `mover-closet-no-door` with a mover parked far outside its sealed hull:
//! no light reaches it and no probe ray can, so its direct, bounce and depth blocks are empty.
//!
//! See: context/lib/build_pipeline.md §Compiler pipeline step 12 (Mover probe channel)

mod prl_build_support;

use std::io::Cursor;
use std::path::{Path, PathBuf};
use std::process::Command;

use postretro_level_format::{SectionId, read_container, read_section_data};
use prl_build_support::REACH_VERIFY_ENV;

const BASELINE_ENV: &str = "POSTRETRO_MOVER_CHANNEL_PARITY_BASELINE_BIN";
/// Worker count for every parity bake, baseline and branch alike.
const PARITY_JOBS: &str = "6";

fn workspace_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(|crates| crates.parent())
        .expect("workspace root")
        .to_path_buf()
}

/// The parent commit's compiler, or a failure naming the variable.
fn baseline_compiler() -> PathBuf {
    let path = std::env::var_os(BASELINE_ENV)
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            panic!(
                "set {BASELINE_ENV} to the parent commit's prl-build; this file's module doc \
                 gives the commands"
            )
        });
    assert!(
        path.is_file(),
        "baseline compiler missing: {}",
        path.display()
    );
    path
}

/// A mover parked 200 m outside `mover-closet-no-door`'s hull, with its
/// path's two waypoints.
const FAR_MOVER: &str = r#"
{
"classname" "kinematic_mover"
"name" "far_slab"
"path" "far_slab_a"
"speed" "1"
{
( 8000 8000 0 ) ( 8000 8001 0 ) ( 8000 8000 1 ) metal/metal_rough_046a 0 0 0 1 1
( 8064 8000 0 ) ( 8064 8000 1 ) ( 8064 8001 0 ) metal/metal_rough_046a 0 0 0 1 1
( 8000 8000 0 ) ( 8000 8000 1 ) ( 8001 8000 0 ) metal/metal_rough_046a 0 0 0 1 1
( 8000 8064 0 ) ( 8001 8064 0 ) ( 8000 8064 1 ) metal/metal_rough_046a 0 0 0 1 1
( 8000 8000 0 ) ( 8001 8000 0 ) ( 8000 8001 0 ) metal/metal_rough_046a 0 0 0 1 1
( 8000 8000 64 ) ( 8000 8001 64 ) ( 8001 8000 64 ) metal/metal_rough_046a 0 0 0 1 1
}
}
{
"classname" "kinematic_waypoint"
"name" "far_slab_a"
"next" "far_slab_b"
"origin" "8032 8032 32"
}
{
"classname" "kinematic_waypoint"
"name" "far_slab_b"
"origin" "8032 8032 160"
}
"#;

/// The parity maps, written beside each other under `dir/maps`.
fn parity_maps(dir: &Path) -> Vec<PathBuf> {
    let maps = dir.join("maps");
    std::fs::create_dir_all(&maps).expect("create maps directory");
    let no_door =
        std::fs::read_to_string(workspace_root().join("content/dev/maps/mover-closet-no-door.map"))
            .expect("read mover-closet-no-door");
    let unchanged = maps.join("mover-closet-far-mover.map");
    std::fs::write(&unchanged, format!("{no_door}\n{FAR_MOVER}")).expect("write far-mover map");
    let no_movers = maps.join("mover-closet-no-door.map");
    std::fs::write(&no_movers, no_door).expect("write no-mover map");
    vec![no_movers, unchanged]
}

/// Compile `map` cold with `compiler` into `out`.
fn compile(compiler: &Path, map: &Path, out: &Path) -> PathBuf {
    std::fs::create_dir_all(out).expect("create output directory");
    let prl = out
        .join(map.file_name().expect("map name"))
        .with_extension("prl");
    let run = Command::new(compiler)
        .arg(map)
        .arg("-o")
        .arg(&prl)
        .args(["--no-cache", "--no-tui", "--jobs", PARITY_JOBS])
        .arg("--baked-root")
        .arg(out.join("baked"))
        .env(REACH_VERIFY_ENV, "0")
        .current_dir(workspace_root())
        .output()
        .expect("spawn prl-build");
    assert!(
        run.status.success(),
        "{} failed on {}\n{}{}",
        compiler.display(),
        map.display(),
        String::from_utf8_lossy(&run.stdout),
        String::from_utf8_lossy(&run.stderr)
    );
    prl
}

fn has_channel(prl: &Path) -> bool {
    let bytes = std::fs::read(prl).expect("read the packed PRL");
    let mut cursor = Cursor::new(bytes.as_slice());
    let meta = read_container(&mut cursor).expect("read the container");
    read_section_data(&mut cursor, &meta, SectionId::MoverProbeChannel as u32)
        .expect("read the section table")
        .is_some()
}

#[test]
#[ignore = "opt-in comparison: set POSTRETRO_MOVER_CHANNEL_PARITY_BASELINE_BIN to the parent commit's prl-build"]
fn maps_whose_movers_change_nothing_pack_byte_identical_to_the_parent_compiler() {
    let baseline = baseline_compiler();
    let branch = PathBuf::from(env!("CARGO_BIN_EXE_prl-build"));
    let dir = tempfile::tempdir().expect("create output directory");
    let mut compared = 0;
    for map in parity_maps(dir.path()) {
        let before = compile(&baseline, &map, &dir.path().join("parent"));
        let after = compile(&branch, &map, &dir.path().join("branch"));
        assert!(!has_channel(&after), "{}: no id 57", map.display());
        let (before_bytes, after_bytes) = (
            std::fs::read(&before).expect("read baseline PRL"),
            std::fs::read(&after).expect("read compiled PRL"),
        );
        assert!(
            before_bytes == after_bytes,
            "{}: packed PRL differs from the parent compiler's: {} B against {} B, first \
             difference at byte {:?}",
            map.display(),
            before_bytes.len(),
            after_bytes.len(),
            before_bytes
                .iter()
                .zip(&after_bytes)
                .position(|(a, b)| a != b)
        );
        compared += 1;
    }
    assert_eq!(compared, 2, "both parity maps compared");
}
