// `prl-build` telemetry through the real binary: the sink opens after
// argument parsing, the run brackets itself, stage and summary events follow
// the Build Summary, and reporter output is unchanged with the sink on.
// See: context/lib/development_guide.md §6.5

mod prl_build_support;

use std::path::{Path, PathBuf};
use std::process::{Output, Stdio};

use postretro_telemetry::{Envelope, ExitReason, Kind, RunEnd, RunStart, TELEMETRY_ENV};

use prl_build_support::prl_build;

fn workspace_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .expect("level-compiler crate must be two levels below the workspace root")
        .to_path_buf()
}

fn tiny_map() -> PathBuf {
    workspace_root().join("content/dev/maps/wedge-shared-plane.map")
}

fn events(path: &Path) -> Vec<Envelope> {
    let text = std::fs::read_to_string(path).unwrap_or_default();
    text.lines()
        .map(|line| Envelope::parse_line(line).unwrap_or_else(|e| panic!("{e}: {line}")))
        .collect()
}

fn run(cwd: &Path, sink: Option<&str>, args: &[&str]) -> Output {
    let mut command = prl_build();
    command.current_dir(cwd).args(args).stdin(Stdio::null());
    match sink {
        Some(sink) => command.env(TELEMETRY_ENV, sink),
        None => command.env_remove(TELEMETRY_ENV),
    };
    command.output().expect("spawn prl-build")
}

fn run_end(events: &[Envelope]) -> RunEnd {
    let last = events.last().expect("a line");
    assert_eq!(last.kind, Kind::RunEnd, "run_end is the last line");
    serde_json::from_value(last.payload.clone()).unwrap()
}

// P14
#[test]
fn help_and_a_rejected_argument_write_nothing() {
    let dir = tempfile::tempdir().unwrap();
    let sink = dir.path().join("t.jsonl");
    let sink = sink.to_str().unwrap();
    let help = run(dir.path(), Some(sink), &["--help"]);
    assert!(help.status.success());
    let rejected = run(dir.path(), Some(sink), &["in.map", "--no-such-flag"]);
    assert!(!rejected.status.success());
    assert!(!Path::new(sink).exists(), "never a lone run_start");
}

#[test]
fn a_bake_ending_before_its_first_stage_writes_only_run_start_and_run_end() {
    let dir = tempfile::tempdir().unwrap();
    let sink = dir.path().join("t.jsonl");
    // The output directory is missing and stdin answers nothing: the bake
    // stops at the output precheck, before any stage.
    let output = dir.path().join("missing/out.prl");
    let result = run(
        dir.path(),
        Some(sink.to_str().unwrap()),
        &[
            tiny_map().to_str().unwrap(),
            "-o",
            output.to_str().unwrap(),
            "--no-tui",
        ],
    );
    assert!(!result.status.success());
    let events = events(&sink);
    let kinds: Vec<Kind> = events.iter().map(|e| e.kind).collect();
    assert_eq!(kinds, [Kind::RunStart, Kind::RunEnd], "{events:?}");
    assert_eq!(run_end(&events).reason, ExitReason::Error);
}

#[test]
fn a_failing_bake_writes_the_stages_it_began_then_a_summary_without_durations() {
    let dir = tempfile::tempdir().unwrap();
    let sink = dir.path().join("t.jsonl");
    let map = std::fs::read_to_string(tiny_map()).unwrap().replace(
        "\"classname\" \"worldspawn\"",
        "\"classname\" \"worldspawn\"\n\"data_script\" \"missing.luau\"",
    );
    let input = dir.path().join("failing.map");
    std::fs::write(&input, map).unwrap();
    let result = run(
        dir.path(),
        Some(sink.to_str().unwrap()),
        &[
            input.to_str().unwrap(),
            "-o",
            dir.path().join("out.prl").to_str().unwrap(),
            "--no-tui",
            "--no-cache",
        ],
    );
    assert!(!result.status.success());
    // Captured log records interleave; the order pinned is the events'.
    let events: Vec<Envelope> = events(&sink)
        .into_iter()
        .filter(|e| e.kind != Kind::Log)
        .collect();
    let kinds: Vec<Kind> = events.iter().map(|e| e.kind).collect();
    assert_eq!(
        kinds,
        [
            Kind::RunStart,
            Kind::BakeStage,
            Kind::BakeSummary,
            Kind::RunEnd
        ],
        "{events:?}"
    );
    assert_eq!(events[1].payload["stage"], "Parsing");
    assert_eq!(events[1].payload["transition"], "begin");
    let summary = &events[2].payload;
    assert_eq!(summary["success"], false);
    assert!(summary["timings"].is_null());
    assert!(summary["total_ms"].is_null());
    assert_eq!(run_end(&events).reason, ExitReason::Error);
}

/// Lines with every digit run collapsed, and the monitor's progress lines
/// (their count depends on its polling) dropped: what stays must match
/// byte for byte with the sink on and off.
fn shape(text: &[u8]) -> Vec<String> {
    let text = String::from_utf8_lossy(text);
    text.lines()
        .filter(|line| !line.contains("ETA"))
        .map(|line| {
            let mut out = String::new();
            let mut in_digits = false;
            for c in line.chars() {
                if c.is_ascii_digit() {
                    if !in_digits {
                        out.push('#');
                    }
                    in_digits = true;
                } else {
                    in_digits = false;
                    out.push(c);
                }
            }
            out
        })
        .collect()
}

#[test]
fn a_plain_bake_brackets_itself_and_leaves_its_output_unchanged() {
    let dir = tempfile::tempdir().unwrap();
    let args = |out: &str| {
        vec![
            tiny_map().to_string_lossy().into_owned(),
            "-o".to_owned(),
            out.to_owned(),
            "--no-tui".to_owned(),
            "--no-cache".to_owned(),
            "-j".to_owned(),
            "2".to_owned(),
        ]
    };
    let off = run(
        dir.path(),
        None,
        &args("off.prl")
            .iter()
            .map(String::as_str)
            .collect::<Vec<_>>(),
    );
    assert!(
        off.status.success(),
        "{}",
        String::from_utf8_lossy(&off.stderr)
    );
    // A relative sink resolves against the bake's working directory.
    let on = run(
        dir.path(),
        Some("relative/t.jsonl"),
        &args("on.prl")
            .iter()
            .map(String::as_str)
            .collect::<Vec<_>>(),
    );
    assert!(
        on.status.success(),
        "{}",
        String::from_utf8_lossy(&on.stderr)
    );
    let sink = dir.path().join("relative/t.jsonl");
    assert!(!sink.exists(), "a missing sink directory disables the file");
    std::fs::create_dir_all(dir.path().join("relative")).unwrap();
    let on = run(
        dir.path(),
        Some("relative/t.jsonl"),
        &args("on.prl")
            .iter()
            .map(String::as_str)
            .collect::<Vec<_>>(),
    );
    assert!(on.status.success());
    assert_eq!(
        shape(&on.stderr),
        shape(&off.stderr)
            .into_iter()
            .map(|line| line.replace("off.prl", "on.prl"))
            .collect::<Vec<_>>(),
        "stderr holds the same lines in the same order"
    );
    assert_eq!(
        shape(&on.stdout),
        shape(&off.stdout),
        "the Build Summary is unchanged"
    );

    let events = events(&sink);
    assert_eq!(events[0].kind, Kind::RunStart);
    assert_eq!(events[0].seq, 0);
    let start: RunStart = serde_json::from_value(events[0].payload.clone()).unwrap();
    let expected_sink = std::fs::canonicalize(dir.path())
        .unwrap()
        .join("relative/t.jsonl");
    assert_eq!(
        std::fs::canonicalize(start.sink.as_deref().unwrap()).unwrap(),
        std::fs::canonicalize(expected_sink).unwrap()
    );
    for (index, event) in events.iter().enumerate() {
        assert_eq!(event.seq, index as u64);
        assert_eq!(event.level, None);
    }
    assert_eq!(run_end(&events).reason, ExitReason::Clean);
    let summary = events
        .iter()
        .find(|e| e.kind == Kind::BakeSummary)
        .expect("a summary");
    assert_eq!(summary.payload["success"], true);
    let output = std::fs::canonicalize(dir.path().join("on.prl")).unwrap();
    assert_eq!(
        summary.payload["output"].as_str(),
        Some(output.to_string_lossy().as_ref())
    );
    // Every Build Summary row on stdout carries the event's duration.
    let stdout = String::from_utf8_lossy(&on.stdout);
    for row in summary.payload["timings"].as_array().unwrap() {
        let name = row["name"].as_str().unwrap();
        let seconds = row["ms"].as_f64().unwrap() / 1000.0;
        let printed = format!("  {: <15} {:>6.2}s", name, seconds as f32);
        assert!(stdout.contains(&printed), "{printed:?} in\n{stdout}");
    }
}
