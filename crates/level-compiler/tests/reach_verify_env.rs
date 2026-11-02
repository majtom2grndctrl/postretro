//! The release-path read of `POSTRETRO_LIGHTMAP_REACH_VERIFY`: a malformed
//! value warns once and leaves verify mode off, `1` turns it on. Unit tests
//! never reach this read, because they pin verify mode in-process.

mod prl_build_support;

use std::path::{Path, PathBuf};
use std::process::Output;

use prl_build_support::{REACH_VERIFY_ENV, prl_build};

const WARNING: &str = "is neither 1 nor 0";
const STAGE_LINE: &str = "Lightmap reach cull:";

fn workspace() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .to_owned()
}

/// A cold, cache-free bake of the smallest lit dev fixture with the switch set
/// to `value`.
fn bake(value: &str) -> Output {
    let scratch = tempfile::tempdir().unwrap();
    let input = workspace().join("content/dev/maps/test_animated_weight_maps_mixed.map");
    prl_build()
        .current_dir(workspace())
        .env(REACH_VERIFY_ENV, value)
        .env("RUST_LOG", "info")
        .env("NO_COLOR", "1")
        .arg(&input)
        .arg("-o")
        .arg(scratch.path().join("out.prl"))
        .args([
            "--no-tui",
            "--no-cache",
            "--verbose",
            "--sh-probe-spacing",
            "4",
            "--lightmap-density",
            "0.25",
            "--soft-shadow-samples",
            "4",
            "--uncompressed-irradiance",
            "-j",
            "1",
        ])
        .arg("--baked-root")
        .arg(scratch.path().join("baked"))
        .output()
        .expect("run prl-build")
}

/// The stage line from stderr, where live log records go.
fn stage_line(stderr: &str) -> &str {
    let lines: Vec<&str> = stderr.lines().filter(|l| l.contains(STAGE_LINE)).collect();
    assert_eq!(lines.len(), 1, "one reach stage line expected:\n{stderr}");
    lines[0]
}

#[test]
fn malformed_switch_warns_once_and_leaves_verify_off() {
    let output = bake("banana");
    let stderr = String::from_utf8_lossy(&output.stderr);
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(output.status.success(), "{stdout}\n{stderr}");
    // The live record goes to stderr once; the end-of-build WARN summary
    // replays it once on stdout.
    assert_eq!(stderr.matches(WARNING).count(), 1, "{stderr}");
    assert_eq!(stdout.matches(WARNING).count(), 1, "{stdout}");
    let line = stage_line(&stderr);
    assert!(!line.contains("re-walked"), "verify stayed off: {line}");
}

#[test]
fn switch_set_to_one_turns_verify_on_without_a_warning() {
    let output = bake("1");
    let stderr = String::from_utf8_lossy(&output.stderr);
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(output.status.success(), "{stdout}\n{stderr}");
    assert!(!stderr.contains(WARNING), "{stderr}");
    let line = stage_line(&stderr);
    assert!(line.contains("re-walked"), "verify is on: {line}");
}
