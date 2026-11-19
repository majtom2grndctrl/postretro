//! `prl-build` telemetry: the run's hub, bake stage transitions through a
//! decorator around whichever reporter the mode picks, and the bake summary.
//! See: context/lib/development_guide.md §6.5 · context/lib/build_pipeline.md §Progress reporting, controls, and logging

use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use postretro_telemetry::{ExitReason, Hub, HubConfig, Kind, ProducerEntry, Source};
use serde::Serialize;

use crate::pipeline::{ORDERED_STAGES, StageId};
use crate::reporter::{BuildSummary, Reporter, StageProgress};

const PRODUCER_BAKE_STAGES: &str = "bake_stages";
const PRODUCER_BAKE_SUMMARY: &str = "bake_summary";
const PRODUCER_LOG: &str = "log";

/// Set once any stage begins: a bake that fails after it writes an
/// unsuccessful summary; one that ends before it writes none.
static BAKE_STARTED: AtomicBool = AtomicBool::new(false);

/// Start the run once arguments parse, so `--help` and a rejected argument
/// write nothing. The hub exists only with the sink variable: `prl-build`
/// keeps no live ring. Returns whether a consumer exists.
pub(crate) fn start(argv: Vec<String>) -> bool {
    let Some(sink) = postretro_telemetry::sink_from_env() else {
        return false;
    };
    postretro_telemetry::install(Hub::start(HubConfig {
        argv,
        producers: vec![
            ProducerEntry::enabled(PRODUCER_BAKE_STAGES),
            ProducerEntry::enabled(PRODUCER_BAKE_SUMMARY),
            ProducerEntry::enabled(PRODUCER_LOG),
        ],
        sink: Some(sink),
        ..HubConfig::new(Source::PrlBuild, env!("CARGO_PKG_VERSION"))
    }));
    true
}

/// Log a sink warning the hub could not log before the logger existed.
pub(crate) fn log_startup_warning() {
    if let Some(warning) = postretro_telemetry::global().and_then(Hub::take_startup_warning) {
        log::warn!("{warning}");
    }
}

/// `run_end` for a main body that returned, after a failed bake's summary.
pub(crate) fn end(result: &anyhow::Result<()>, source: &Path, output: &Path) {
    match result {
        Ok(()) => postretro_telemetry::end_run(ExitReason::Clean, None),
        Err(error) => {
            if BAKE_STARTED.load(Ordering::Acquire) {
                emit_summary(&BakeSummaryEvent::failed(source, output));
            }
            postretro_telemetry::end_run(ExitReason::Error, Some(&format!("{error:#}")));
        }
    }
}

/// `bake_stage`: one reporter transition. Order only: durations come from
/// the summary, because transitions overlap or begin late under the TUI.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub(crate) struct BakeStageEvent {
    pub(crate) stage: &'static str,
    pub(crate) transition: Transition,
    pub(crate) background: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum Transition {
    Begin,
    Finish,
    /// The stage produced no output.
    Skip,
}

/// One Build Summary row; `stage` is false for the rows that name no stage.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub(crate) struct SummaryRow {
    pub(crate) name: &'static str,
    pub(crate) ms: f64,
    pub(crate) stage: bool,
}

/// `bake_summary`: the source map and output `.prl` as absolute paths, so a
/// reducer joins an engine load to its bake. A failed bake carries no
/// durations.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub(crate) struct BakeSummaryEvent {
    pub(crate) success: bool,
    pub(crate) source: String,
    pub(crate) output: String,
    pub(crate) timings: Option<Vec<SummaryRow>>,
    pub(crate) total_ms: Option<f64>,
}

impl BakeSummaryEvent {
    pub(crate) fn succeeded(summary: &BuildSummary, source: &Path, output: &Path) -> Self {
        Self {
            success: true,
            source: absolute(source),
            output: absolute(output),
            timings: Some(
                summary
                    .timings
                    .iter()
                    .map(|(name, duration)| SummaryRow {
                        name,
                        ms: duration.as_secs_f64() * 1000.0,
                        stage: ORDERED_STAGES.iter().any(|id| id.label() == *name),
                    })
                    .collect(),
            ),
            total_ms: Some(summary.total.as_secs_f64() * 1000.0),
        }
    }

    fn failed(source: &Path, output: &Path) -> Self {
        Self {
            success: false,
            source: absolute(source),
            output: absolute(output),
            timings: None,
            total_ms: None,
        }
    }
}

fn emit_summary(event: &BakeSummaryEvent) {
    postretro_telemetry::emit(Kind::BakeSummary, event);
}

/// The path as an absolute string: canonical when it exists (the output
/// does once written), else joined to the working directory.
fn absolute(path: &Path) -> String {
    std::fs::canonicalize(path)
        .unwrap_or_else(|_| {
            std::env::current_dir()
                .map(|cwd| cwd.join(path))
                .unwrap_or_else(|_| path.to_path_buf())
        })
        .display()
        .to_string()
}

/// Wraps the reporter a mode picked: every transition and the success
/// summary also become events. The wrapped reporter's output is unchanged.
pub(crate) struct TelemetryReporter {
    inner: Arc<dyn Reporter>,
    source: std::path::PathBuf,
    output: std::path::PathBuf,
}

impl TelemetryReporter {
    /// `inner` unwrapped when no hub exists, so a plain bake pays nothing.
    pub(crate) fn wrap(
        inner: Arc<dyn Reporter>,
        source: &Path,
        output: &Path,
    ) -> Arc<dyn Reporter> {
        if !postretro_telemetry::active() {
            return inner;
        }
        Arc::new(Self {
            inner,
            source: source.to_path_buf(),
            output: output.to_path_buf(),
        })
    }

    fn transition(id: StageId, transition: Transition, background: bool) {
        if transition == Transition::Begin {
            BAKE_STARTED.store(true, Ordering::Release);
        }
        postretro_telemetry::emit(
            Kind::BakeStage,
            &BakeStageEvent {
                stage: id.label(),
                transition,
                background,
            },
        );
    }
}

impl Reporter for TelemetryReporter {
    fn begin_stage(&self, id: StageId) {
        Self::transition(id, Transition::Begin, false);
        self.inner.begin_stage(id);
    }

    fn begin_background_stage(&self, id: StageId) {
        Self::transition(id, Transition::Begin, true);
        self.inner.begin_background_stage(id);
    }

    fn declare_progress(&self, id: StageId, progress: StageProgress) {
        self.inner.declare_progress(id, progress);
    }

    fn finish_stage(&self, id: StageId) {
        self.inner.finish_stage(id);
        Self::transition(id, Transition::Finish, false);
    }

    fn skip_stage(&self, id: StageId) {
        self.inner.skip_stage(id);
        Self::transition(id, Transition::Skip, false);
    }

    fn finalize(&self, summary: BuildSummary) {
        emit_summary(&BakeSummaryEvent::succeeded(
            &summary,
            &self.source,
            &self.output,
        ));
        self.inner.finalize(summary);
    }

    fn finalize_failure(&self) {
        self.inner.finalize_failure();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;
    use std::time::Duration;

    /// Records the calls it receives, to prove the decorator forwards each.
    #[derive(Default)]
    struct Recording(Mutex<Vec<String>>);

    impl Reporter for Recording {
        fn begin_stage(&self, id: StageId) {
            self.0.lock().unwrap().push(format!("begin {}", id.label()));
        }
        fn begin_background_stage(&self, id: StageId) {
            self.0
                .lock()
                .unwrap()
                .push(format!("background {}", id.label()));
        }
        fn declare_progress(&self, id: StageId, _: StageProgress) {
            self.0
                .lock()
                .unwrap()
                .push(format!("progress {}", id.label()));
        }
        fn finish_stage(&self, id: StageId) {
            self.0
                .lock()
                .unwrap()
                .push(format!("finish {}", id.label()));
        }
        fn skip_stage(&self, id: StageId) {
            self.0.lock().unwrap().push(format!("skip {}", id.label()));
        }
        fn finalize(&self, summary: BuildSummary) {
            self.0
                .lock()
                .unwrap()
                .push(format!("finalize {}", summary.timings.len()));
        }
        fn finalize_failure(&self) {
            self.0.lock().unwrap().push(String::from("failure"));
        }
    }

    fn hub() -> (Arc<Hub>, postretro_telemetry::scoped::ScopedHub) {
        let hub = Arc::new(Hub::start(HubConfig {
            memory: true,
            ..HubConfig::new(Source::PrlBuild, "t")
        }));
        let scope = postretro_telemetry::scoped::install(Arc::clone(&hub));
        (hub, scope)
    }

    fn payloads(hub: &Hub, kind: Kind) -> Vec<serde_json::Value> {
        hub.lines()
            .iter()
            .map(|line| postretro_telemetry::Envelope::parse_line(line).unwrap())
            .filter(|event| event.kind == kind)
            .map(|event| event.payload)
            .collect()
    }

    #[test]
    fn the_decorator_forwards_every_call_and_emits_each_transition() {
        let (hub, _scope) = hub();
        let inner = Arc::new(Recording::default());
        let reporter =
            TelemetryReporter::wrap(inner.clone(), Path::new("a.map"), Path::new("a.prl"));
        reporter.begin_stage(StageId::Parsing);
        reporter.declare_progress(StageId::Parsing, StageProgress::indeterminate());
        reporter.finish_stage(StageId::Parsing);
        reporter.begin_background_stage(StageId::ShadowmaskAtlas);
        reporter.skip_stage(StageId::NavMesh);
        reporter.finalize_failure();
        assert_eq!(
            *inner.0.lock().unwrap(),
            [
                "begin Parsing",
                "progress Parsing",
                "finish Parsing",
                "background ShadowmaskAtlas",
                "skip NavMesh",
                "failure"
            ]
        );
        let stages: Vec<(String, String)> = payloads(&hub, Kind::BakeStage)
            .into_iter()
            .map(|p| {
                (
                    p["stage"].as_str().unwrap().to_string(),
                    p["transition"].as_str().unwrap().to_string(),
                )
            })
            .collect();
        assert_eq!(
            stages,
            [
                ("Parsing".into(), "begin".into()),
                ("Parsing".into(), "finish".into()),
                ("ShadowmaskAtlas".into(), "begin".into()),
                ("NavMesh".into(), "skip".into()),
            ]
        );
    }

    #[test]
    fn the_summary_event_marks_stage_rows_and_keeps_the_summary_durations() {
        let (hub, _scope) = hub();
        let inner = Arc::new(Recording::default());
        let reporter = TelemetryReporter::wrap(inner, Path::new("a.map"), Path::new("a.prl"));
        reporter.finalize(BuildSummary {
            timings: vec![
                ("Parsing", Duration::from_millis(1500)),
                ("  TexelSteps", Duration::from_millis(250)),
                ("SH bound", Duration::from_millis(5)),
            ],
            total: Duration::from_millis(1755),
            details: Vec::new(),
        });
        let summary = &payloads(&hub, Kind::BakeSummary)[0];
        assert_eq!(summary["success"], true);
        let rows = summary["timings"].as_array().unwrap();
        assert_eq!(rows[0]["stage"], true);
        assert_eq!(rows[0]["ms"], 1500.0);
        assert_eq!(rows[1]["stage"], false);
        assert_eq!(rows[2]["stage"], false);
        assert_eq!(summary["total_ms"], 1755.0);
        assert!(Path::new(summary["output"].as_str().unwrap()).is_absolute());
    }

    #[test]
    fn a_failed_summary_carries_no_durations() {
        let event = BakeSummaryEvent::failed(Path::new("a.map"), Path::new("a.prl"));
        let json = serde_json::to_value(&event).unwrap();
        assert_eq!(json["success"], false);
        assert!(json["timings"].is_null());
        assert!(json["total_ms"].is_null());
    }

    #[test]
    fn without_a_hub_the_reporter_is_returned_unwrapped() {
        let inner: Arc<dyn Reporter> = Arc::new(Recording::default());
        let wrapped = TelemetryReporter::wrap(Arc::clone(&inner), Path::new("a"), Path::new("b"));
        assert!(Arc::ptr_eq(&inner, &wrapped));
    }

    fn stage_order(hub: &Hub) -> Vec<String> {
        // Each stage once, at its begin (or at a skip that had no begin), in
        // event order.
        let mut order: Vec<String> = Vec::new();
        for event in payloads(hub, Kind::BakeStage) {
            let stage = event["stage"].as_str().unwrap().to_string();
            if event["transition"] != "finish" && !order.contains(&stage) {
                order.push(stage);
            }
        }
        order
    }

    fn summary_stage_rows(hub: &Hub) -> (Vec<String>, serde_json::Value) {
        let summary = payloads(hub, Kind::BakeSummary).pop().expect("a summary");
        let rows = summary["timings"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|row| row["stage"] == true)
            .map(|row| row["name"].as_str().unwrap().to_string())
            .collect();
        (rows, summary)
    }

    fn tiny_map_args(out: &Path) -> crate::Args {
        let map = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../content/dev/maps/wedge-shared-plane.map");
        crate::parse_args_from(
            [
                map.to_string_lossy().into_owned(),
                "-o".to_owned(),
                out.to_string_lossy().into_owned(),
                "--no-cache".to_owned(),
            ]
            .into_iter(),
        )
        .unwrap()
    }

    // P7: under both reporters, the stage events list the Build Summary's
    // stages, minus its non-stage rows, in summary order; the summary event's
    // durations are the Build Summary's own.
    #[test]
    fn stage_events_follow_the_build_summary_under_the_plain_and_tui_reporters() {
        for tui in [false, true] {
            let (hub, _scope) = hub();
            let root = tempfile::tempdir().unwrap();
            let out = root.path().join("out.prl");
            let args = tiny_map_args(&out);
            let governor = Arc::new(crate::governor::Governor::new(2, false));
            let started = std::time::Instant::now();
            let logs = crate::logger::LogSink::default();
            if tui {
                let prepared = crate::pipeline::prepare(&args).unwrap();
                let planned = crate::pipeline::planned_stages(&prepared.map_data.lights);
                let inner: Arc<dyn Reporter> =
                    Arc::new(crate::tui::TuiReporter::new(&planned, logs));
                let reporter = TelemetryReporter::wrap(inner, &args.input, &args.output);
                crate::pipeline::run_prepared(&args, None, started, reporter, governor, prepared)
                    .unwrap();
            } else {
                let inner: Arc<dyn Reporter> =
                    Arc::new(crate::reporter::PlainReporter::new(started, logs));
                let reporter = TelemetryReporter::wrap(inner, &args.input, &args.output);
                crate::pipeline::run(&args, None, started, reporter, governor).unwrap();
            }
            let (rows, summary) = summary_stage_rows(&hub);
            assert_eq!(stage_order(&hub), rows, "tui: {tui}");
            assert_eq!(summary["success"], true);
            assert_eq!(
                summary["output"].as_str(),
                Some(
                    std::fs::canonicalize(&out)
                        .unwrap()
                        .to_string_lossy()
                        .as_ref()
                )
            );
        }
    }
}
