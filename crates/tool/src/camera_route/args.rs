//! `generate-camera-route` argument parsing.
//!
//! Every refusal that needs no level lives here, so a bad parameter fails
//! before the level is read or anything is written.
//!
//! See: context/lib/rendering_pipeline.md §12

use std::ffi::OsString;
use std::path::PathBuf;

use postretro_camera_route::{DEFAULT_HFOV_DEGREES, DEFAULT_STEPS, WALK_SEED, WalkKind};

use crate::flag::match_flag;

/// The engine's accepted horizontal FOV range, degrees: `MIN_FOV_DEG` and
/// `MAX_FOV_DEG` in `crates/postretro/src/camera.rs`. The tool cannot depend
/// on the engine, so the range is repeated here; playback refuses a file
/// outside it.
const MIN_HFOV_DEGREES: f32 = 60.0;
const MAX_HFOV_DEGREES: f32 = 130.0;
/// The walk's own pitch bound (`POSTRETRO_LIGHTMAP_WALK_PITCH`), degrees.
const MAX_PITCH_DEGREES: f32 = 89.0;

#[derive(Debug, Clone, PartialEq)]
pub(super) struct GenerateArgs {
    /// The level path as typed: the bake identity records it verbatim.
    pub(super) level: String,
    pub(super) kind: WalkKind,
    /// The path's seed; the eyes draw from `seed ^ EYE_STREAM_SALT`.
    pub(super) seed: u64,
    pub(super) steps: usize,
    pub(super) pitch_degrees: f32,
    pub(super) hfov_degrees: f32,
    pub(super) out: PathBuf,
}

pub(super) fn parse_args(args: Vec<OsString>) -> Result<GenerateArgs, String> {
    let mut level = None;
    let mut kind = None;
    let mut seed = None;
    let mut steps = None;
    let mut pitch = None;
    let mut hfov = None;
    let mut out = None;
    let mut index = 0;

    while index < args.len() {
        let token = args[index]
            .to_str()
            .ok_or_else(|| usage("generate-camera-route arguments must be valid UTF-8"))?;
        let next = args.get(index + 1);
        let mut value_of = |flag: &str| -> Result<Option<String>, String> {
            let Some(matched) = match_flag(token, next, flag) else {
                return Ok(None);
            };
            index += matched.tokens;
            let value = matched
                .value
                .ok_or_else(|| usage(&format!("{flag} requires a value")))?;
            value
                .to_str()
                .map(|v| Some(v.to_string()))
                .ok_or_else(|| usage(&format!("{flag} must be valid UTF-8")))
        };
        if let Some(value) = value_of("--kind")? {
            set_once(&mut kind, parse_kind(&value)?, "--kind")?;
        } else if let Some(value) = value_of("--seed")? {
            set_once(&mut seed, parse_seed(&value)?, "--seed")?;
        } else if let Some(value) = value_of("--steps")? {
            set_once(&mut steps, parse_steps(&value)?, "--steps")?;
        } else if let Some(value) = value_of("--pitch")? {
            set_once(&mut pitch, parse_pitch(&value)?, "--pitch")?;
        } else if let Some(value) = value_of("--hfov")? {
            set_once(&mut hfov, parse_hfov(&value)?, "--hfov")?;
        } else if let Some(value) = value_of("-o")? {
            set_once(&mut out, PathBuf::from(value), "-o")?;
        } else if let Some(value) = value_of("--out")? {
            set_once(&mut out, PathBuf::from(value), "-o")?;
        } else if token.starts_with('-') {
            return Err(usage(&format!(
                "unknown generate-camera-route option {token:?}"
            )));
        } else {
            if level.replace(token.to_string()).is_some() {
                return Err(usage(
                    "generate-camera-route accepts exactly one level path",
                ));
            }
            index += 1;
        }
    }

    Ok(GenerateArgs {
        level: level.ok_or_else(|| usage("generate-camera-route requires a level path"))?,
        kind: kind.ok_or_else(|| usage("generate-camera-route requires --kind <kind>"))?,
        seed: seed.unwrap_or(WALK_SEED),
        steps: steps.unwrap_or(DEFAULT_STEPS),
        pitch_degrees: pitch.unwrap_or(0.0),
        hfov_degrees: hfov.unwrap_or(DEFAULT_HFOV_DEGREES),
        out: out.ok_or_else(|| usage("generate-camera-route requires -o <out-file>"))?,
    })
}

/// The kinds this verb generates.
const KINDS: [WalkKind; 2] = [WalkKind::FarPointTour, WalkKind::Random];

fn parse_kind(value: &str) -> Result<WalkKind, String> {
    WalkKind::from_key(value)
        .filter(|kind| KINDS.contains(kind))
        .ok_or_else(|| {
            let valid: Vec<&str> = KINDS.iter().map(|kind| kind.key()).collect();
            usage(&format!(
                "unknown --kind {value:?}; valid kinds: {}",
                valid.join(", ")
            ))
        })
}

fn parse_seed(value: &str) -> Result<u64, String> {
    let parsed = match value
        .strip_prefix("0x")
        .or_else(|| value.strip_prefix("0X"))
    {
        Some(hex) => u64::from_str_radix(hex, 16),
        None => value.parse::<u64>(),
    };
    parsed.map_err(|_| {
        usage(&format!(
            "--seed expects a u64, decimal or 0x hex, got {value:?}"
        ))
    })
}

fn parse_steps(value: &str) -> Result<usize, String> {
    match value.parse::<usize>() {
        Ok(0) => Err(usage("--steps must be at least 1, got 0")),
        Ok(steps) => Ok(steps),
        Err(_) => Err(usage(&format!(
            "--steps expects a step count, got {value:?}"
        ))),
    }
}

fn parse_degrees(value: &str, flag: &str) -> Result<f32, String> {
    value
        .parse::<f32>()
        .ok()
        .filter(|degrees| degrees.is_finite())
        .ok_or_else(|| usage(&format!("{flag} expects finite degrees, got {value:?}")))
}

fn parse_pitch(value: &str) -> Result<f32, String> {
    let degrees = parse_degrees(value, "--pitch")?;
    if degrees.abs() >= MAX_PITCH_DEGREES {
        return Err(usage(&format!(
            "--pitch must lie in (-{MAX_PITCH_DEGREES}, {MAX_PITCH_DEGREES}) degrees, got {value}"
        )));
    }
    Ok(degrees)
}

fn parse_hfov(value: &str) -> Result<f32, String> {
    let degrees = parse_degrees(value, "--hfov")?;
    if !(MIN_HFOV_DEGREES..=MAX_HFOV_DEGREES).contains(&degrees) {
        return Err(usage(&format!(
            "--hfov must lie in the engine's {MIN_HFOV_DEGREES}-{MAX_HFOV_DEGREES} degrees, got {value}"
        )));
    }
    Ok(degrees)
}

fn set_once<T>(slot: &mut Option<T>, value: T, flag: &str) -> Result<(), String> {
    if slot.replace(value).is_some() {
        return Err(usage(&format!(
            "generate-camera-route accepts only one {flag}"
        )));
    }
    Ok(())
}

pub(super) fn usage(message: &str) -> String {
    format!(
        "{message}\n\nUsage: postretro-tool generate-camera-route <level.prl> \\
         --kind far-point|random [--seed <u64>] [--steps <n>] [--pitch <degrees>] \\
         [--hfov <degrees>] -o <out-file>"
    )
}
