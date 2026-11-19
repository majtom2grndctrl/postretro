// The versioned envelope every telemetry event travels in.
// See: context/lib/development_guide.md §6.5

use serde::{Deserialize, Serialize};

/// Bumped when a field changes meaning or an existing field's type changes.
/// Adding a kind or a payload field is no break: readers skip kinds they do
/// not know (`Envelope::parse_line` reads only this build's kinds). A reader
/// refuses any other version.
pub const ENVELOPE_VERSION: u32 = 1;

/// The process that wrote an event.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Source {
    #[serde(rename = "engine")]
    Engine,
    #[serde(rename = "prl-build")]
    PrlBuild,
}

/// The running level an event belongs to: the load generation and map
/// captured at commit. `None` between levels. Wire: `{"gen": u64, "map": string}`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Level {
    #[serde(rename = "gen")]
    pub generation: u64,
    pub map: String,
}

/// Every event kind. The wire name is the snake_case variant name.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Kind {
    RunStart,
    ProducerState,
    RunEnd,
    Log,
    BootPhases,
    ModPhases,
    LoadPhases,
    Unload,
    GeometryInstall,
    ShStreamingWindow,
    LightmapStreamingWindow,
    CpuTimingWindow,
    GpuTimingWindow,
    BakeStage,
    BakeSummary,
}

impl Kind {
    pub const ALL: [Kind; 15] = [
        Kind::RunStart,
        Kind::ProducerState,
        Kind::RunEnd,
        Kind::Log,
        Kind::BootPhases,
        Kind::ModPhases,
        Kind::LoadPhases,
        Kind::Unload,
        Kind::GeometryInstall,
        Kind::ShStreamingWindow,
        Kind::LightmapStreamingWindow,
        Kind::CpuTimingWindow,
        Kind::GpuTimingWindow,
        Kind::BakeStage,
        Kind::BakeSummary,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            Kind::RunStart => "run_start",
            Kind::ProducerState => "producer_state",
            Kind::RunEnd => "run_end",
            Kind::Log => "log",
            Kind::BootPhases => "boot_phases",
            Kind::ModPhases => "mod_phases",
            Kind::LoadPhases => "load_phases",
            Kind::Unload => "unload",
            Kind::GeometryInstall => "geometry_install",
            Kind::ShStreamingWindow => "sh_streaming_window",
            Kind::LightmapStreamingWindow => "lightmap_streaming_window",
            Kind::CpuTimingWindow => "cpu_timing_window",
            Kind::GpuTimingWindow => "gpu_timing_window",
            Kind::BakeStage => "bake_stage",
            Kind::BakeSummary => "bake_summary",
        }
    }

    pub fn parse(name: &str) -> Option<Kind> {
        Kind::ALL.into_iter().find(|kind| kind.as_str() == name)
    }

    /// The load family: the latest of each stays pinned in the live ring so
    /// a late client still reads the latest load.
    pub fn is_load_family(self) -> bool {
        matches!(
            self,
            Kind::BootPhases
                | Kind::ModPhases
                | Kind::LoadPhases
                | Kind::Unload
                | Kind::GeometryInstall
        )
    }
}

/// One parsed line. The payload stays untyped: its struct lives with the
/// producer that wrote it.
#[derive(Debug, Clone, PartialEq)]
pub struct Envelope {
    pub v: u32,
    pub run: String,
    pub seq: u64,
    pub t_ms: u64,
    pub source: Source,
    pub level: Option<Level>,
    pub kind: Kind,
    pub payload: serde_json::Value,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct EnvelopeIn {
    v: u32,
    run: String,
    seq: u64,
    t_ms: u64,
    source: Source,
    level: Option<Level>,
    kind: Kind,
    payload: serde_json::Value,
}

/// Why a line is not a current envelope.
#[derive(Debug)]
pub enum ParseError {
    Json(serde_json::Error),
    Version(u32),
}

impl std::fmt::Display for ParseError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ParseError::Json(error) => write!(f, "not an envelope: {error}"),
            ParseError::Version(v) => {
                write!(f, "envelope version {v}, expected {ENVELOPE_VERSION}")
            }
        }
    }
}

impl std::error::Error for ParseError {}

impl Envelope {
    /// Parse one line (with or without its trailing newline) as a current
    /// envelope.
    pub fn parse_line(line: &str) -> Result<Envelope, ParseError> {
        let raw: EnvelopeIn = serde_json::from_str(line).map_err(ParseError::Json)?;
        if raw.v != ENVELOPE_VERSION {
            return Err(ParseError::Version(raw.v));
        }
        Ok(Envelope {
            v: raw.v,
            run: raw.run,
            seq: raw.seq,
            t_ms: raw.t_ms,
            source: raw.source,
            level: raw.level,
            kind: raw.kind,
            payload: raw.payload,
        })
    }
}

/// Format one envelope line, newline included, from pre-serialized pieces:
/// `run_json` and `payload_json` are JSON values already.
pub(crate) fn format_line(
    run_json: &str,
    seq: u64,
    t_ms: u64,
    source: Source,
    level: Option<&Level>,
    kind: Kind,
    payload_json: &str,
) -> String {
    let source = match source {
        Source::Engine => "engine",
        Source::PrlBuild => "prl-build",
    };
    let level_json = match level {
        // A `String` and a `u64` always serialize.
        Some(level) => serde_json::to_string(level).expect("a level serializes"),
        None => String::from("null"),
    };
    format!(
        "{{\"v\":{ENVELOPE_VERSION},\"run\":{run_json},\"seq\":{seq},\"t_ms\":{t_ms},\
         \"source\":\"{source}\",\"level\":{level_json},\"kind\":\"{kind}\",\"payload\":{payload_json}}}\n",
        kind = kind.as_str(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every variant's place in `Kind::ALL`. Exhaustive, so a new variant
    /// fails to compile here until it is placed in the list too.
    fn position(kind: Kind) -> usize {
        match kind {
            Kind::RunStart => 0,
            Kind::ProducerState => 1,
            Kind::RunEnd => 2,
            Kind::Log => 3,
            Kind::BootPhases => 4,
            Kind::ModPhases => 5,
            Kind::LoadPhases => 6,
            Kind::Unload => 7,
            Kind::GeometryInstall => 8,
            Kind::ShStreamingWindow => 9,
            Kind::LightmapStreamingWindow => 10,
            Kind::CpuTimingWindow => 11,
            Kind::GpuTimingWindow => 12,
            Kind::BakeStage => 13,
            Kind::BakeSummary => 14,
        }
    }

    #[test]
    fn kind_all_lists_every_variant_in_place() {
        for (index, kind) in Kind::ALL.into_iter().enumerate() {
            assert_eq!(position(kind), index);
        }
    }

    #[test]
    fn kind_wire_names_match_serde_for_every_kind() {
        for kind in Kind::ALL {
            let json = serde_json::to_string(&kind).unwrap();
            assert_eq!(json, format!("\"{}\"", kind.as_str()));
            assert_eq!(Kind::parse(kind.as_str()), Some(kind));
        }
        assert_eq!(Kind::parse("nope"), None);
    }

    #[test]
    fn formatted_line_parses_back_with_level_spelled_gen_and_map() {
        let level = Level {
            generation: 3,
            map: String::from("maps/a.prl"),
        };
        let line = format_line(
            "\"r1\"",
            7,
            12,
            Source::PrlBuild,
            Some(&level),
            Kind::Unload,
            "{\"x\":null}",
        );
        assert!(line.ends_with('\n'));
        assert!(line.contains("\"level\":{\"gen\":3,\"map\":\"maps/a.prl\"}"));
        let parsed = Envelope::parse_line(&line).unwrap();
        assert_eq!(parsed.seq, 7);
        assert_eq!(parsed.source, Source::PrlBuild);
        assert_eq!(parsed.level, Some(level));
        assert_eq!(parsed.kind, Kind::Unload);
    }

    #[test]
    fn parse_refuses_another_version() {
        let line = "{\"v\":99,\"run\":\"r\",\"seq\":0,\"t_ms\":0,\"source\":\"engine\",\"level\":null,\"kind\":\"log\",\"payload\":{}}";
        assert!(matches!(
            Envelope::parse_line(line),
            Err(ParseError::Version(99))
        ));
    }
}
