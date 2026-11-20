//! The engine's telemetry run: the hub with its producers, the logger it
//! tees, and `run_end` on every exit the process controls. Emit points live
//! beside the log line each event renders.
//! See: context/lib/development_guide.md §6.5

use postretro_telemetry::{ExitReason, Hub, HubConfig, ProducerEntry, RingCaps, Source};

use crate::cpu_timing::CPU_TIMING_ENV;
use crate::startup::camera_route_flags::{CAMERA_ROUTE_FEATURE, CAMERA_ROUTE_FLAG};

pub(crate) mod producers {
    pub(crate) const LOAD_PHASES: &str = "load_phases";
    pub(crate) const GEOMETRY_INSTALL: &str = "geometry_install";
    pub(crate) const SH_STREAMING: &str = "sh_streaming";
    pub(crate) const LIGHTMAP_STREAMING: &str = "lightmap_streaming";
    pub(crate) const CPU_TIMING: &str = "cpu_timing";
    pub(crate) const GPU_TIMING: &str = "gpu_timing";
    pub(crate) const PLAYBACK: &str = "playback";
    pub(crate) const LOG: &str = "log";
}

pub(crate) const GPU_TIMING_ENV: &str = "POSTRETRO_GPU_TIMING";

/// How the engine runs, as far as its timing producers care.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum RunMode {
    Windowed,
    /// `--headless`: renders no frames.
    Headless,
    /// `--capture`: its timing goes to the capture measurement report, not
    /// to timing windows.
    Capture,
}

impl RunMode {
    fn of(args: &[String]) -> Self {
        if crate::startup::headless_arg(args).is_some() {
            RunMode::Headless
        } else if crate::startup::capture_arg(args).is_some() {
            RunMode::Capture
        } else {
            RunMode::Windowed
        }
    }

    /// Why this mode writes no timing windows, if it writes none.
    fn no_timing_windows(self) -> Option<&'static str> {
        match self {
            RunMode::Windowed => None,
            RunMode::Headless => Some("a headless run renders no frames"),
            RunMode::Capture => Some("a capture run reports timing in its measurement report"),
        }
    }
}

/// Every engine producer's state at launch. GPU timing stays pending until
/// the renderer has its adapter. Headless and capture runs close no timing
/// window, so each timing producer they were asked for is unsupported there.
/// Playback's windows follow `--camera-route` (`route_named`).
pub(crate) fn engine_producers(
    mode: RunMode,
    cpu_variable: Option<&str>,
    gpu_variable: Option<&str>,
    route_named: bool,
) -> Vec<ProducerEntry> {
    let cpu = match (mode.no_timing_windows(), cpu_variable == Some("1")) {
        (Some(reason), true) => {
            ProducerEntry::unsupported(producers::CPU_TIMING, reason).with_variable(CPU_TIMING_ENV)
        }
        _ => ProducerEntry::gated(producers::CPU_TIMING, CPU_TIMING_ENV, cpu_variable),
    };
    let gpu = match (mode.no_timing_windows(), gpu_variable == Some("1")) {
        (_, false) => ProducerEntry::disabled(producers::GPU_TIMING, GPU_TIMING_ENV),
        (Some(reason), true) => {
            ProducerEntry::unsupported(producers::GPU_TIMING, reason).with_variable(GPU_TIMING_ENV)
        }
        (None, true) => ProducerEntry::pending(producers::GPU_TIMING, GPU_TIMING_ENV),
    };
    vec![
        ProducerEntry::enabled(producers::LOAD_PHASES),
        ProducerEntry::enabled(producers::GEOMETRY_INSTALL),
        ProducerEntry::enabled(producers::SH_STREAMING),
        ProducerEntry::enabled(producers::LIGHTMAP_STREAMING),
        cpu,
        gpu,
        playback_producer(mode, route_named, cfg!(feature = "dev-tools")),
        ProducerEntry::enabled(producers::LOG),
    ]
}

/// Playback's `playback_window` events: enabled when the launch names a
/// route in a windowed run of a build with playback's feature; disabled,
/// naming the flag, without one; unsupported where playback cannot run.
fn playback_producer(mode: RunMode, route_named: bool, feature: bool) -> ProducerEntry {
    let unsupported = |reason: &str| {
        ProducerEntry::unsupported(producers::PLAYBACK, reason).with_variable(CAMERA_ROUTE_FLAG)
    };
    if !feature {
        return unsupported(&format!(
            "camera-route playback needs the `{CAMERA_ROUTE_FEATURE}` feature, which this build lacks"
        ));
    }
    match (mode, route_named) {
        (_, false) => ProducerEntry::disabled(producers::PLAYBACK, CAMERA_ROUTE_FLAG),
        (RunMode::Headless, true) => unsupported("a headless run renders no frames"),
        (RunMode::Capture, true) => unsupported("a capture run plays no camera route"),
        (RunMode::Windowed, true) => {
            ProducerEntry::enabled(producers::PLAYBACK).with_variable(CAMERA_ROUTE_FLAG)
        }
    }
}

/// Start the engine's run before any log record: the hub only when a
/// consumer exists (the sink variable, or a live channel to read the ring),
/// so a plain run builds no payload and formats no extra record. Returns
/// whether a consumer exists.
pub(crate) fn start_engine_run(args: &[String]) -> bool {
    let sink = postretro_telemetry::sink_from_env();
    let ring = live_ring_requested(args).then_some(RingCaps::DEFAULT);
    if sink.is_none() && ring.is_none() {
        return false;
    }
    let env = |name: &str| std::env::var(name).ok();
    postretro_telemetry::install(Hub::start(HubConfig {
        argv: args.to_vec(),
        producers: engine_producers(
            RunMode::of(args),
            env(CPU_TIMING_ENV).as_deref(),
            env(GPU_TIMING_ENV).as_deref(),
            crate::startup::camera_route_flags::names_camera_route(args),
        ),
        sink,
        ring,
        ..HubConfig::new(Source::Engine, env!("CARGO_PKG_VERSION"))
    }));
    true
}

/// The live ring exists only in a run whose `events` verb can read it.
fn live_ring_requested(args: &[String]) -> bool {
    #[cfg(feature = "observe-live")]
    {
        crate::startup::observe_live_port_arg(args).is_some()
    }
    #[cfg(not(feature = "observe-live"))]
    {
        let _ = args;
        false
    }
}

/// Install env_logger: exactly `env_logger::init()` without a consumer,
/// behind the tee with one. Then log a sink warning the hub could not log
/// before.
pub(crate) fn install_engine_logger(consumer: bool) {
    if consumer {
        let logger = env_logger::Builder::from_default_env().build();
        let max = logger.filter();
        // Nothing has installed a logger yet: main calls this first.
        let _ = postretro_telemetry::install_logger(Box::new(logger), max, true);
    } else {
        env_logger::init();
    }
    if let Some(warning) = postretro_telemetry::global().and_then(Hub::take_startup_warning) {
        log::warn!("{warning}");
    }
}

/// `run_end` for a main body that returned.
pub(crate) fn end_engine_run(result: &anyhow::Result<()>) {
    match result {
        Ok(()) => postretro_telemetry::end_run(ExitReason::Clean, None),
        Err(error) => {
            postretro_telemetry::end_run(ExitReason::Error, Some(&format!("{error:#}")));
        }
    }
}

/// `run_end` before a `process::exit` that skips main's return: capture,
/// headless, and a stock binary refusing either mode. `detail` names the
/// mode and, on an error, the error.
pub(crate) fn end_run_before_exit(code: i32, detail: &str) {
    let reason = if code == 0 {
        ExitReason::Clean
    } else {
        ExitReason::Error
    };
    postretro_telemetry::end_run(reason, Some(detail));
}

/// `gpu_timing_window`: one closed GPU window. A pass sampled in no
/// readback has no mean, never a zero.
#[derive(Debug, Clone, PartialEq, serde::Serialize)]
pub(crate) struct GpuWindowEvent {
    pub(crate) readbacks: u32,
    pub(crate) passes: Vec<GpuPassEvent>,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize)]
pub(crate) struct GpuPassEvent {
    pub(crate) label: &'static str,
    pub(crate) mean_ms: Option<f32>,
    pub(crate) sampled_readbacks: u32,
    pub(crate) malformed_readbacks: u32,
}

impl From<&postretro_renderer::FrameTimingSnapshot> for GpuWindowEvent {
    fn from(window: &postretro_renderer::FrameTimingSnapshot) -> Self {
        Self {
            readbacks: window.readbacks,
            passes: window
                .passes
                .iter()
                .map(|pass| GpuPassEvent {
                    label: pass.label,
                    mean_ms: pass.average_ms,
                    sampled_readbacks: pass.sampled_readbacks,
                    malformed_readbacks: pass.malformed_readbacks,
                })
                .collect(),
        }
    }
}

/// Emit the GPU timing window that closed this frame, if one did, under the
/// level current at its close: GPU windows are never reset at a level
/// change and may span one.
pub(crate) fn emit_gpu_window(renderer: &mut postretro_renderer::Renderer) {
    if let Some(window) = renderer.take_gpu_timing_window() {
        postretro_telemetry::emit_with(postretro_telemetry::Kind::GpuTimingWindow, || {
            GpuWindowEvent::from(&window)
        });
    }
}

/// Settle GPU timing once the renderer has its adapter: enabled, or
/// unsupported with the adapter's reason. Settles only a pending producer,
/// so a launch without the variable stays disabled.
pub(crate) fn settle_gpu_timing(support: postretro_renderer::GpuTimingSupport) {
    postretro_telemetry::with_hub(|hub| {
        if hub.producer_state(producers::GPU_TIMING)
            != Some(postretro_telemetry::ProducerState::Pending)
        {
            return;
        }
        let entry = match support {
            postretro_renderer::GpuTimingSupport::Enabled => {
                ProducerEntry::enabled(producers::GPU_TIMING).with_variable(GPU_TIMING_ENV)
            }
            postretro_renderer::GpuTimingSupport::NotRequested => {
                ProducerEntry::disabled(producers::GPU_TIMING, GPU_TIMING_ENV)
            }
            postretro_renderer::GpuTimingSupport::Unsupported => ProducerEntry::unsupported(
                producers::GPU_TIMING,
                "the adapter lacks TIMESTAMP_QUERY and TIMESTAMP_QUERY_INSIDE_ENCODERS",
            )
            .with_variable(GPU_TIMING_ENV),
        };
        hub.set_producer_state(entry);
    });
}

#[cfg(test)]
mod playback_tests;

#[cfg(test)]
mod tests {
    use super::*;
    use postretro_telemetry::ProducerState;

    fn state(producers: &[ProducerEntry], name: &str) -> ProducerEntry {
        producers
            .iter()
            .find(|entry| entry.producer == name)
            .cloned()
            .unwrap_or_else(|| panic!("{name} listed"))
    }

    #[test]
    fn timing_producers_read_disabled_naming_their_variable_without_it() {
        let producers = engine_producers(RunMode::Windowed, None, None, false);
        let cpu = state(&producers, producers::CPU_TIMING);
        assert_eq!(cpu.state, ProducerState::Disabled);
        assert_eq!(cpu.variable.as_deref(), Some(CPU_TIMING_ENV));
        let gpu = state(&producers, producers::GPU_TIMING);
        assert_eq!(gpu.state, ProducerState::Disabled);
        assert_eq!(gpu.variable.as_deref(), Some(GPU_TIMING_ENV));
    }

    #[test]
    fn with_their_variables_cpu_reads_enabled_and_gpu_pending() {
        let producers = engine_producers(RunMode::Windowed, Some("1"), Some("1"), false);
        assert_eq!(
            state(&producers, producers::CPU_TIMING).state,
            ProducerState::Enabled
        );
        assert_eq!(
            state(&producers, producers::GPU_TIMING).state,
            ProducerState::Pending
        );
    }

    #[test]
    fn a_headless_run_lists_both_requested_timing_producers_unsupported_with_reasons() {
        let producers = engine_producers(RunMode::Headless, Some("1"), Some("1"), false);
        for name in [producers::CPU_TIMING, producers::GPU_TIMING] {
            let entry = state(&producers, name);
            assert_eq!(entry.state, ProducerState::Unsupported);
            assert!(entry.reason.is_some());
        }
    }

    #[test]
    fn a_capture_run_lists_requested_timing_producers_unsupported_naming_their_variables() {
        let producers = engine_producers(RunMode::Capture, Some("1"), Some("1"), false);
        for (name, variable) in [
            (producers::CPU_TIMING, CPU_TIMING_ENV),
            (producers::GPU_TIMING, GPU_TIMING_ENV),
        ] {
            let entry = state(&producers, name);
            assert_eq!(entry.state, ProducerState::Unsupported);
            assert_eq!(entry.variable.as_deref(), Some(variable));
        }
    }

    #[test]
    fn every_engine_producer_is_listed_once() {
        let producers = engine_producers(RunMode::Windowed, None, None, false);
        let mut names: Vec<&str> = producers.iter().map(|p| p.producer.as_str()).collect();
        names.sort_unstable();
        names.dedup();
        assert_eq!(names.len(), producers.len());
    }

    fn scoped(
        producers: Vec<ProducerEntry>,
    ) -> (std::sync::Arc<Hub>, postretro_telemetry::scoped::ScopedHub) {
        let hub = std::sync::Arc::new(Hub::start(HubConfig {
            memory: true,
            producers,
            ..HubConfig::new(Source::Engine, "t")
        }));
        let scope = postretro_telemetry::scoped::install(std::sync::Arc::clone(&hub));
        (hub, scope)
    }

    fn parsed(hub: &Hub) -> Vec<postretro_telemetry::Envelope> {
        hub.lines()
            .iter()
            .map(|line| postretro_telemetry::Envelope::parse_line(line).unwrap())
            .collect()
    }

    // P1: GPU timing requested on an adapter without timestamp queries reads
    // pending at `run_start`, then settles unsupported with its reason as soon
    // as the adapter exists, before any window could close.
    #[test]
    fn pending_gpu_timing_settles_unsupported_with_its_reason_or_enabled() {
        for (support, expected) in [
            (
                postretro_renderer::GpuTimingSupport::Unsupported,
                ProducerState::Unsupported,
            ),
            (
                postretro_renderer::GpuTimingSupport::Enabled,
                ProducerState::Enabled,
            ),
        ] {
            let (hub, _scope) = scoped(engine_producers(RunMode::Windowed, None, Some("1"), false));
            settle_gpu_timing(support);
            settle_gpu_timing(support);
            let events = parsed(&hub);
            assert_eq!(events.len(), 2, "run_start, one settle");
            let start: postretro_telemetry::RunStart =
                serde_json::from_value(events[0].payload.clone()).unwrap();
            assert_eq!(
                state(&start.producers, producers::GPU_TIMING).state,
                ProducerState::Pending
            );
            let settled: ProducerEntry = serde_json::from_value(events[1].payload.clone()).unwrap();
            assert_eq!(settled.state, expected);
            assert_eq!(settled.variable.as_deref(), Some(GPU_TIMING_ENV));
            assert_eq!(
                settled.reason.is_some(),
                expected == ProducerState::Unsupported
            );
        }
    }

    #[test]
    fn a_disabled_gpu_producer_is_never_settled() {
        let (hub, _scope) = scoped(engine_producers(RunMode::Windowed, None, None, false));
        settle_gpu_timing(postretro_renderer::GpuTimingSupport::NotRequested);
        assert_eq!(parsed(&hub).len(), 1);
    }

    // A GPU window spanning a level change names the level current at its
    // close; an unsampled pass carries a null mean, never zero.
    #[test]
    fn a_gpu_window_names_the_level_at_its_close_and_keeps_absent_means_null() {
        let (hub, _scope) = scoped(Vec::new());
        let level = |generation| postretro_telemetry::Level {
            generation,
            map: format!("/maps/{generation}.prl"),
        };
        postretro_telemetry::set_level(Some(level(1)));
        postretro_telemetry::set_level(Some(level(2)));
        let window = postretro_renderer::FrameTimingSnapshot {
            readbacks: 120,
            passes: vec![
                postretro_renderer::PassTiming {
                    label: "forward",
                    average_ms: Some(2.5),
                    sampled_readbacks: 120,
                    malformed_readbacks: 0,
                },
                postretro_renderer::PassTiming {
                    label: "sh_compose",
                    average_ms: None,
                    sampled_readbacks: 0,
                    malformed_readbacks: 0,
                },
            ],
        };
        postretro_telemetry::emit(
            postretro_telemetry::Kind::GpuTimingWindow,
            &GpuWindowEvent::from(&window),
        );
        let event = parsed(&hub).pop().unwrap();
        assert_eq!(event.level, Some(level(2)));
        assert_eq!(event.payload["passes"][0]["mean_ms"], 2.5);
        assert!(event.payload["passes"][1]["mean_ms"].is_null());
    }
}
