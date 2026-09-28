// Batch dump byte-identity, with and without CPU stage timing requested.
// See: context/lib/networking.md §Not netcode: the live introspection channel

use std::fs;
use std::process::Command;

#[path = "capture_support/mod.rs"]
#[allow(dead_code)]
mod support;

use support::{compile_dev_map, workspace_root};

fn headless_dump(runspec: &std::path::Path, cpu_timing: Option<&str>) -> std::process::Output {
    let mut command = Command::new(env!("CARGO_BIN_EXE_postretro"));
    command
        .arg("--headless")
        .arg(runspec)
        .env_remove("POSTRETRO_CPU_TIMING")
        .current_dir(workspace_root());
    if let Some(value) = cpu_timing {
        // Set on the child only: the test never mutates its own environment.
        command.env("POSTRETRO_CPU_TIMING", value);
    }
    command.output().expect("launch postretro --headless")
}

#[test]
fn identical_batch_runs_are_byte_identical_whatever_the_cpu_timing_env() {
    let workspace = workspace_root();
    let map = compile_dev_map(&workspace, "spawner-test");
    let temp = tempfile::tempdir().expect("create runspec directory");
    let runspec = temp.path().join("runspec.json");
    fs::write(
        &runspec,
        serde_json::to_vec(&serde_json::json!({
            "map": map.to_path_buf().display().to_string(),
            "ticks": 30
        }))
        .expect("serialize runspec"),
    )
    .expect("write runspec");

    let runs = [
        headless_dump(&runspec, None),
        headless_dump(&runspec, None),
        headless_dump(&runspec, Some("1")),
        headless_dump(&runspec, Some("1")),
    ];
    for run in &runs {
        assert!(
            run.status.success(),
            "headless run failed\nstderr:\n{}",
            String::from_utf8_lossy(&run.stderr)
        );
    }
    assert!(!runs[0].stdout.is_empty());
    for run in &runs[1..] {
        assert_eq!(run.stdout, runs[0].stdout, "batch dump bytes differ");
    }
}

#[test]
fn batch_runspec_requesting_cpu_timing_is_rejected_by_name() {
    let temp = tempfile::tempdir().expect("create runspec directory");
    let runspec = temp.path().join("runspec.json");
    fs::write(
        &runspec,
        br#"{ "map": "content/dev/maps/unused.prl", "ticks": 1, "dump": { "cpu_timing": true } }"#,
    )
    .expect("write runspec");

    let run = headless_dump(&runspec, Some("1"));
    assert!(!run.status.success());
    assert!(run.stdout.is_empty(), "no partial document on rejection");
    let stderr = String::from_utf8_lossy(&run.stderr);
    assert!(stderr.contains("dump.cpu_timing"), "{stderr}");
}
