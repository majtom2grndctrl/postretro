//! Startup timing lines as telemetry events: boot, mod-init and level-load
//! phases, and unload, each built from the `StartupTimings` its line
//! formats.
//! See: context/lib/boot_sequence.md §Startup timing vocabulary · context/lib/development_guide.md §6.5

use std::path::Path;
use std::time::Duration;

use postretro_telemetry::{Kind, Level};
use serde::Serialize;

use super::StartupTimings;

/// One top-level stage of a startup line, with the marks shown in its
/// parentheses.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub(crate) struct PhaseEvent {
    pub(crate) name: String,
    pub(crate) ms: f64,
    pub(crate) marks: Vec<MarkEvent>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub(crate) struct MarkEvent {
    pub(crate) name: String,
    pub(crate) ms: f64,
}

/// `boot_phases`, `mod_phases` and `load_phases`. A load's carries its
/// generation and `.prl` path; boot and mod-init carry neither.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub(crate) struct PhasesEvent {
    pub(crate) generation: Option<u64>,
    pub(crate) prl: Option<String>,
    pub(crate) phases: Vec<PhaseEvent>,
}

/// `unload`: the level it unloads and how long the unload took.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub(crate) struct UnloadEvent {
    pub(crate) generation: u64,
    pub(crate) prl: String,
    pub(crate) unload_ms: f64,
}

fn ms(duration: Duration) -> f64 {
    duration.as_secs_f64() * 1000.0
}

impl StartupTimings {
    /// The typed view of the line `summary` formats: each top-level stage
    /// in order with its contained marks.
    pub(crate) fn phase_events(&self) -> Vec<PhaseEvent> {
        self.entries
            .iter()
            .enumerate()
            .map(|(index, (stage, duration))| PhaseEvent {
                name: (*stage).to_string(),
                ms: ms(*duration),
                marks: self
                    .contained
                    .iter()
                    .find(|(at, _)| *at == index)
                    .map(|(_, inside)| {
                        inside
                            .iter()
                            .map(|(mark, duration)| MarkEvent {
                                name: mark.to_string(),
                                ms: ms(*duration),
                            })
                            .collect()
                    })
                    .unwrap_or_default(),
            })
            .collect()
    }
}

/// Emit boot or mod-init phases beside their `[Startup]` line.
pub(crate) fn emit_phases(kind: Kind, timings: &StartupTimings) {
    postretro_telemetry::emit_with(kind, || PhasesEvent {
        generation: None,
        prl: None,
        phases: timings.phase_events(),
    });
}

/// Emit line C as `load_phases`, under the running level it measured: line
/// C is logged after its load committed.
pub(crate) fn emit_load_phases(timings: &StartupTimings) {
    postretro_telemetry::emit_with(Kind::LoadPhases, || {
        let level = postretro_telemetry::level();
        PhasesEvent {
            generation: level.as_ref().map(|level| level.generation),
            prl: level.map(|level| level.map),
            phases: timings.phase_events(),
        }
    });
}

/// Emit `unload` naming the level it unloaded, then clear the running
/// level: events from here on belong to no level until the next commit.
pub(crate) fn emit_unload(unload_time: Duration) {
    let Some(level) = postretro_telemetry::level() else {
        return;
    };
    postretro_telemetry::emit_for_level(
        Kind::Unload,
        Some(&level),
        &UnloadEvent {
            generation: level.generation,
            prl: level.map.clone(),
            unload_ms: ms(unload_time),
        },
    );
    postretro_telemetry::set_level(None);
}

/// `geometry_install`: the renderer's frozen line for the load's set build.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub(crate) struct GeometryInstallEvent {
    pub(crate) generation: u64,
    pub(crate) has_geometry: bool,
    pub(crate) total_ms: f64,
    pub(crate) phases: Vec<MarkEvent>,
    pub(crate) other_ms: f64,
}

impl GeometryInstallEvent {
    pub(crate) fn new(generation: u64, timing: &postretro_renderer::GeometryInstallTiming) -> Self {
        Self {
            generation,
            has_geometry: timing.has_geometry,
            total_ms: ms(timing.total),
            phases: timing
                .phases
                .iter()
                .map(|(name, duration)| MarkEvent {
                    name: (*name).to_string(),
                    ms: ms(*duration),
                })
                .collect(),
            other_ms: ms(timing.other),
        }
    }
}

/// Emit a load's geometry install once the load can no longer be
/// cancelled, under its generation; the envelope names the running level
/// (none in the loading-screen order, the level it replaces beside one).
pub(crate) fn emit_geometry_install(
    generation: u64,
    timing: Option<postretro_renderer::GeometryInstallTiming>,
) {
    if let Some(timing) = timing {
        postretro_telemetry::emit_with(Kind::GeometryInstall, || {
            GeometryInstallEvent::new(generation, &timing)
        });
    }
}

/// The running level a load commits as: its generation and its `.prl` as an
/// absolute path, so a bake summary naming the same file joins it whatever
/// either process's working directory was.
pub(crate) fn committed_level(generation: u64, map_path: &Path) -> Level {
    let absolute = std::fs::canonicalize(map_path).unwrap_or_else(|_| {
        std::env::current_dir()
            .map(|cwd| cwd.join(map_path))
            .unwrap_or_else(|_| map_path.to_path_buf())
    });
    Level {
        generation,
        map: absolute.display().to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::borrow::Cow;

    /// Line C's own text, split back into `(stage, ms, [(mark, ms)])`.
    fn parse_summary(line: &str) -> Vec<(String, f64, Vec<(String, f64)>)> {
        let body = line.strip_prefix("[Startup] ").expect("startup prefix");
        let mut out = Vec::new();
        let mut rest = body;
        while !rest.is_empty() {
            let (name, after) = rest.split_once('=').unwrap();
            let (value, mut after) = after.split_once("ms").unwrap();
            let mut marks = Vec::new();
            if let Some(inner) = after.strip_prefix(" (incl. ") {
                let (inner, tail) = inner.split_once(')').unwrap();
                for mark in inner.split(", ") {
                    let (mark, value) = mark.split_once('=').unwrap();
                    marks.push((
                        mark.to_string(),
                        value.trim_end_matches("ms").parse().unwrap(),
                    ));
                }
                after = tail;
            }
            out.push((name.to_string(), value.parse().unwrap(), marks));
            rest = after.strip_prefix(", ").unwrap_or(after);
        }
        out
    }

    fn fixed_line_c() -> StartupTimings {
        let mut timings = StartupTimings::new();
        timings
            .entries
            .push(("load_dispatch", Duration::from_micros(1_250)));
        timings
            .entries
            .push(("bundle_ready", Duration::from_micros(40_040)));
        timings.contained.push((
            1,
            vec![
                (Cow::Borrowed("prl_parse"), Duration::from_micros(12_300)),
                (
                    Cow::Owned(String::from("model_parse_x3")),
                    Duration::from_micros(7_770),
                ),
            ],
        ));
        timings
            .entries
            .push(("commit_frame", Duration::from_micros(9_000)));
        timings.contained.push((
            2,
            vec![
                (Cow::Borrowed("commit_checks"), Duration::from_micros(2_000)),
                (Cow::Borrowed("set_publish"), Duration::from_micros(7_000)),
            ],
        ));
        timings
    }

    #[test]
    fn load_phases_event_and_line_c_agree_field_for_field() {
        let timings = fixed_line_c();
        let line = parse_summary(&timings.summary());
        let events = timings.phase_events();
        assert_eq!(line.len(), events.len());
        for ((name, value, marks), event) in line.iter().zip(&events) {
            assert_eq!(name, &event.name);
            assert_eq!(format!("{value:.1}"), format!("{:.1}", event.ms));
            assert_eq!(marks.len(), event.marks.len());
            for ((mark, value), event) in marks.iter().zip(&event.marks) {
                assert_eq!(mark, &event.name);
                assert_eq!(format!("{value:.1}"), format!("{:.1}", event.ms));
            }
        }
        // The commit frame's marks sum to it, in the event as on the line.
        let commit = &events[2];
        let sum: f64 = commit.marks.iter().map(|mark| mark.ms).sum();
        assert!((sum - commit.ms).abs() < 1e-9);
    }

    #[test]
    fn one_fixed_snapshot_formats_a_byte_identical_line_with_telemetry_on_and_off() {
        let timings = fixed_line_c();
        let off = timings.summary();
        let hub = std::sync::Arc::new(postretro_telemetry::Hub::start(
            postretro_telemetry::HubConfig {
                memory: true,
                ..postretro_telemetry::HubConfig::new(postretro_telemetry::Source::Engine, "t")
            },
        ));
        let _scope = postretro_telemetry::scoped::install(std::sync::Arc::clone(&hub));
        emit_load_phases(&timings);
        assert_eq!(timings.summary(), off);
        assert_eq!(hub.lines().len(), 2);
    }
}
