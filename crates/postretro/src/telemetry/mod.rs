//! The engine's telemetry run: the hub with its producers, the logger it
//! tees, and `run_end` on every exit the process controls. Emit points live
//! beside the log line each event renders.
//! See: context/lib/development_guide.md §6.5

use postretro_telemetry::{ExitReason, Hub, HubConfig, ProducerEntry, RingCaps, Source};

use crate::cpu_timing::CPU_TIMING_ENV;

pub(crate) mod producers {
    pub(crate) const LOAD_PHASES: &str = "load_phases";
    pub(crate) const GEOMETRY_INSTALL: &str = "geometry_install";
    pub(crate) const SH_STREAMING: &str = "sh_streaming";
    pub(crate) const LIGHTMAP_STREAMING: &str = "lightmap_streaming";
    pub(crate) const CPU_TIMING: &str = "cpu_timing";
    pub(crate) const GPU_TIMING: &str = "gpu_timing";
    pub(crate) const LOG: &str = "log";
}

pub(crate) const GPU_TIMING_ENV: &str = "POSTRETRO_GPU_TIMING";

/// Every engine producer's state at launch. GPU timing stays pending until
/// the renderer has its adapter; a headless run renders nothing, so both
/// timing producers it was asked for are unsupported there.
pub(crate) fn engine_producers(
    headless: bool,
    cpu_variable: Option<&str>,
    gpu_variable: Option<&str>,
) -> Vec<ProducerEntry> {
    const HEADLESS: &str = "a headless run renders no frames";
    let cpu = match (headless, cpu_variable == Some("1")) {
        (true, true) => ProducerEntry::unsupported(producers::CPU_TIMING, HEADLESS),
        _ => ProducerEntry::gated(producers::CPU_TIMING, CPU_TIMING_ENV, cpu_variable),
    };
    let gpu = match (headless, gpu_variable == Some("1")) {
        (_, false) => ProducerEntry::disabled(producers::GPU_TIMING, GPU_TIMING_ENV),
        (true, true) => ProducerEntry::unsupported(producers::GPU_TIMING, HEADLESS),
        (false, true) => ProducerEntry::pending(producers::GPU_TIMING, GPU_TIMING_ENV),
    };
    vec![
        ProducerEntry::enabled(producers::LOAD_PHASES),
        ProducerEntry::enabled(producers::GEOMETRY_INSTALL),
        ProducerEntry::enabled(producers::SH_STREAMING),
        ProducerEntry::enabled(producers::LIGHTMAP_STREAMING),
        cpu,
        gpu,
        ProducerEntry::enabled(producers::LOG),
    ]
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
    let headless = crate::startup::headless_arg(args).is_some();
    postretro_telemetry::install(Hub::start(HubConfig {
        argv: args.to_vec(),
        producers: engine_producers(
            headless,
            env(CPU_TIMING_ENV).as_deref(),
            env(GPU_TIMING_ENV).as_deref(),
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

/// Install env_logger: exactly today's `init` without a consumer, behind the
/// tee with one. Then log a sink warning the hub could not log before.
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
/// headless, and a stock binary refusing either mode.
pub(crate) fn end_run_before_exit(code: i32, mode: &str) {
    let reason = if code == 0 {
        ExitReason::Clean
    } else {
        ExitReason::Error
    };
    postretro_telemetry::end_run(reason, Some(mode));
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
                ProducerEntry::enabled(producers::GPU_TIMING)
            }
            postretro_renderer::GpuTimingSupport::NotRequested => {
                ProducerEntry::disabled(producers::GPU_TIMING, GPU_TIMING_ENV)
            }
            postretro_renderer::GpuTimingSupport::Unsupported => ProducerEntry::unsupported(
                producers::GPU_TIMING,
                "the adapter lacks TIMESTAMP_QUERY and TIMESTAMP_QUERY_INSIDE_ENCODERS",
            ),
        };
        hub.set_producer_state(entry);
    });
}

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
        let producers = engine_producers(false, None, None);
        let cpu = state(&producers, producers::CPU_TIMING);
        assert_eq!(cpu.state, ProducerState::Disabled);
        assert_eq!(cpu.variable.as_deref(), Some(CPU_TIMING_ENV));
        let gpu = state(&producers, producers::GPU_TIMING);
        assert_eq!(gpu.state, ProducerState::Disabled);
        assert_eq!(gpu.variable.as_deref(), Some(GPU_TIMING_ENV));
    }

    #[test]
    fn with_their_variables_cpu_reads_enabled_and_gpu_pending() {
        let producers = engine_producers(false, Some("1"), Some("1"));
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
        let producers = engine_producers(true, Some("1"), Some("1"));
        for name in [producers::CPU_TIMING, producers::GPU_TIMING] {
            let entry = state(&producers, name);
            assert_eq!(entry.state, ProducerState::Unsupported);
            assert!(entry.reason.is_some());
        }
    }

    #[test]
    fn every_engine_producer_is_listed_once() {
        let producers = engine_producers(false, None, None);
        let mut names: Vec<&str> = producers.iter().map(|p| p.producer.as_str()).collect();
        names.sort_unstable();
        names.dedup();
        assert_eq!(names.len(), producers.len());
    }
}
