// The engine's telemetry run through the real binary, built with no
// optional feature: the file sink is written in every build, `run_start` is
// the first line ahead of every captured record, and an error exit ends with
// `run_end` naming it. The error exit under test is a boot map that does not
// exist: where no window can open the run fails at the window, and where one
// can, the boot map load fails and exits non-zero. A windowed run on a
// sleeping or locked display stalls before either, so every launch here is
// bounded by `RUN_LIMIT`.
// See: context/lib/development_guide.md §6.5

use std::io::Read;
use std::path::Path;
use std::process::{Command, Output, Stdio};
use std::time::{Duration, Instant};

use postretro_telemetry::{Envelope, ExitReason, Kind, RunEnd, RunStart, TELEMETRY_ENV};

/// How long a launched engine may run before the test kills it. Every run
/// here ends on its own in a few seconds; a window stalled on a sleeping or
/// locked display would otherwise hang the suite.
const RUN_LIMIT: Duration = Duration::from_secs(30);

/// Runs the engine to its exit, or kills it and fails the test once it passes
/// [`RUN_LIMIT`].
fn run_bounded(command: &mut Command, what: &str) -> Output {
    let mut child = command
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap_or_else(|e| panic!("launch {what}: {e}"));
    // Drain both pipes while waiting, so a full pipe cannot stall the child.
    let drain = |mut pipe: Box<dyn Read + Send>| {
        std::thread::spawn(move || {
            let mut bytes = Vec::new();
            pipe.read_to_end(&mut bytes).ok();
            bytes
        })
    };
    let stdout = drain(Box::new(child.stdout.take().unwrap()));
    let stderr = drain(Box::new(child.stderr.take().unwrap()));
    let deadline = Instant::now() + RUN_LIMIT;
    let status = loop {
        if let Some(status) = child.try_wait().expect("poll the engine") {
            break status;
        }
        if Instant::now() >= deadline {
            child.kill().ok();
            child.wait().ok();
            panic!(
                "{what} did not exit within {}s; killed. A sleeping or locked \
                 display stalls a windowed run. stderr:\n{}",
                RUN_LIMIT.as_secs(),
                String::from_utf8_lossy(&stderr.join().unwrap_or_default())
            );
        }
        std::thread::sleep(Duration::from_millis(50));
    };
    Output {
        status,
        stdout: stdout.join().unwrap(),
        stderr: stderr.join().unwrap(),
    }
}

fn events(path: &Path) -> Vec<Envelope> {
    std::fs::read_to_string(path)
        .unwrap_or_default()
        .lines()
        .map(|line| Envelope::parse_line(line).unwrap_or_else(|e| panic!("{e}: {line}")))
        .collect()
}

#[test]
fn an_engine_run_writes_the_sink_and_brackets_its_exit() {
    let dir = tempfile::tempdir().unwrap();
    let output = run_bounded(
        Command::new(env!("CARGO_BIN_EXE_postretro"))
            .current_dir(dir.path())
            // A path that names no map: whether or not a window opens, the
            // run ends on its own with an error.
            .arg("no-such-map.prl")
            .env(TELEMETRY_ENV, "t.jsonl")
            .env_remove("POSTRETRO_CPU_TIMING")
            .env_remove("POSTRETRO_GPU_TIMING")
            .env_remove("RUST_LOG")
            .env("WAYLAND_DISPLAY", "")
            .env("DISPLAY", ""),
        "postretro",
    );
    assert!(!output.status.success());
    let events = events(&dir.path().join("t.jsonl"));
    assert!(
        !events.is_empty(),
        "stderr:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(events[0].kind, Kind::RunStart);
    assert_eq!(events[0].seq, 0);
    let start: RunStart = serde_json::from_value(events[0].payload.clone()).unwrap();
    // A relative sink resolves against the run's working directory.
    assert_eq!(
        std::fs::canonicalize(start.sink.as_deref().unwrap()).unwrap(),
        std::fs::canonicalize(dir.path()).unwrap().join("t.jsonl")
    );
    let gpu = start
        .producers
        .iter()
        .find(|p| p.producer == "gpu_timing")
        .unwrap();
    assert_eq!(gpu.variable.as_deref(), Some("POSTRETRO_GPU_TIMING"));
    // The starting record is captured even though stderr's default filter
    // hides info.
    assert!(
        events
            .iter()
            .any(|e| e.kind == Kind::Log && e.payload["message"] == "[Engine] Postretro starting")
    );
    assert!(!String::from_utf8_lossy(&output.stderr).contains("Postretro starting"));
    for (index, event) in events.iter().enumerate() {
        assert_eq!(event.seq, index as u64);
    }
    let last = events.last().unwrap();
    assert_eq!(last.kind, Kind::RunEnd, "{events:?}");
    let end: RunEnd = serde_json::from_value(last.payload.clone()).unwrap();
    assert_eq!(end.reason, ExitReason::Error);
}

#[test]
fn without_the_sink_variable_the_engine_writes_no_file() {
    let dir = tempfile::tempdir().unwrap();
    let output = run_bounded(
        Command::new(env!("CARGO_BIN_EXE_postretro"))
            .current_dir(dir.path())
            .arg("no-such-map.prl")
            .env_remove(TELEMETRY_ENV)
            .env("WAYLAND_DISPLAY", "")
            .env("DISPLAY", ""),
        "postretro",
    );
    assert!(!output.status.success());
    assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 0);
}

// A headless run asked for both timing producers lists each unsupported with
// its reason, writes no timing window, and leaves nothing pending at its
// `run_end`, which its `process::exit` path writes, naming the mode.
#[cfg(feature = "observability")]
#[test]
fn a_headless_run_lists_timing_unsupported_and_ends_through_its_exit() {
    let dir = tempfile::tempdir().unwrap();
    let output = run_bounded(
        Command::new(env!("CARGO_BIN_EXE_postretro"))
            .current_dir(dir.path())
            .args(["--headless", "no-such-runspec.json"])
            .env(TELEMETRY_ENV, "t.jsonl")
            .env("POSTRETRO_CPU_TIMING", "1")
            .env("POSTRETRO_GPU_TIMING", "1"),
        "postretro --headless",
    );
    assert!(!output.status.success());
    let events = events(&dir.path().join("t.jsonl"));
    let start: RunStart = serde_json::from_value(events[0].payload.clone()).unwrap();
    for name in ["cpu_timing", "gpu_timing"] {
        let entry = start.producers.iter().find(|p| p.producer == name).unwrap();
        assert_eq!(entry.state, postretro_telemetry::ProducerState::Unsupported);
        assert!(entry.reason.is_some());
    }
    assert!(
        start
            .producers
            .iter()
            .all(|p| p.state != postretro_telemetry::ProducerState::Pending)
    );
    assert!(events.iter().all(|e| !matches!(
        e.kind,
        Kind::CpuTimingWindow | Kind::GpuTimingWindow | Kind::ProducerState
    )));
    let end: RunEnd = serde_json::from_value(events.last().unwrap().payload.clone()).unwrap();
    assert_eq!(end.reason, ExitReason::Error);
    assert!(
        end.detail
            .as_deref()
            .is_some_and(|detail| detail.starts_with("headless: ")),
        "{:?}",
        end.detail
    );
}
