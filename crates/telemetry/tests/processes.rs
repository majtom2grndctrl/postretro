// The file sink across processes: two appending at once interleave whole
// lines, and a killed process leaves every line before the kill parseable.
// Each child is this test binary re-run on one test with a mode variable.
// See: context/lib/development_guide.md §6.5

use std::path::{Path, PathBuf};
use std::process::{Child, Command};
use std::time::{Duration, Instant};

use postretro_telemetry::{
    Envelope, ExitReason, Hub, HubConfig, Kind, LogRecord, MESSAGE_CAP_BYTES, Source,
};

const MODE: &str = "POSTRETRO_TELEMETRY_TEST_CHILD";
const SINK: &str = "POSTRETRO_TELEMETRY_TEST_SINK";
const LINES_PER_CHILD: u64 = 400;

#[derive(serde::Serialize)]
struct Probe {
    n: u64,
}

/// The child body: does nothing unless spawned with a mode.
#[test]
fn child_writer() {
    let Ok(mode) = std::env::var(MODE) else {
        return;
    };
    let sink = PathBuf::from(std::env::var(SINK).expect("child sink"));
    let source = if mode == "bake" {
        Source::PrlBuild
    } else {
        Source::Engine
    };
    let hub = Hub::start(HubConfig {
        sink: Some(sink),
        ..HubConfig::new(source, "test")
    });
    let at_cap = "x".repeat(MESSAGE_CAP_BYTES + 100);
    for n in 0..LINES_PER_CHILD {
        if n % 10 == 0 {
            hub.log_record(log::Level::Warn, "child", &at_cap);
        } else {
            hub.emit(Kind::CpuTimingWindow, &Probe { n });
        }
    }
    if mode == "hang" {
        loop {
            std::thread::sleep(Duration::from_secs(60));
        }
    }
    hub.end_run(ExitReason::Clean, None);
}

fn spawn_child(mode: &str, sink: &Path) -> Child {
    Command::new(std::env::current_exe().unwrap())
        .args(["--exact", "child_writer", "--nocapture", "--test-threads=1"])
        .env(MODE, mode)
        .env(SINK, sink)
        .spawn()
        .expect("spawn child")
}

fn scratch(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "postretro-telemetry-proc-{name}-{}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir.join("t.jsonl")
}

fn parse_all(path: &Path) -> Vec<Envelope> {
    let text = std::fs::read_to_string(path).unwrap();
    assert!(
        text.is_empty() || text.ends_with('\n'),
        "a trailing partial line"
    );
    text.lines()
        .map(|line| Envelope::parse_line(line).unwrap_or_else(|e| panic!("{e}: {line}")))
        .collect()
}

// P11
#[test]
fn an_engine_run_and_a_bake_appending_at_once_write_two_whole_runs() {
    let sink = scratch("append");
    let mut engine = spawn_child("engine", &sink);
    let mut bake = spawn_child("bake", &sink);
    assert!(engine.wait().unwrap().success());
    assert!(bake.wait().unwrap().success());
    let envelopes = parse_all(&sink);
    let mut runs: Vec<&str> = envelopes.iter().map(|e| e.run.as_str()).collect();
    runs.sort_unstable();
    runs.dedup();
    assert_eq!(runs.len(), 2, "distinct run ids");
    for run in runs {
        let lines: Vec<&Envelope> = envelopes.iter().filter(|e| e.run == run).collect();
        let seqs: Vec<u64> = lines.iter().map(|e| e.seq).collect();
        assert_eq!(seqs, (0..lines.len() as u64).collect::<Vec<_>>());
        assert_eq!(lines.first().unwrap().kind, Kind::RunStart);
        assert_eq!(lines.last().unwrap().kind, Kind::RunEnd);
    }
    let capped = envelopes
        .iter()
        .filter(|e| e.kind == Kind::Log)
        .map(|e| serde_json::from_value::<LogRecord>(e.payload.clone()).unwrap())
        .inspect(|record| assert!(record.truncated))
        .count();
    assert_eq!(capped as u64, 2 * LINES_PER_CHILD / 10);
}

#[test]
fn a_killed_process_leaves_every_line_before_the_kill_parseable() {
    let sink = scratch("kill");
    let mut child = spawn_child("hang", &sink);
    let deadline = Instant::now() + Duration::from_secs(30);
    loop {
        let written = std::fs::read_to_string(&sink)
            .map(|text| text.lines().count() as u64)
            .unwrap_or(0);
        if written > LINES_PER_CHILD {
            break;
        }
        assert!(Instant::now() < deadline, "child never wrote its lines");
        std::thread::sleep(Duration::from_millis(20));
    }
    child.kill().unwrap();
    let _ = child.wait();
    let envelopes = parse_all(&sink);
    assert_eq!(envelopes.len() as u64, LINES_PER_CHILD + 1);
    assert!(
        envelopes.iter().all(|e| e.kind != Kind::RunEnd),
        "a kill writes no run_end"
    );
}
