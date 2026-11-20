//! `generate-camera-route` argument parsing.
//!
//! Every refusal that needs no level lives here, so a bad parameter fails
//! before the level is read or anything is written.
//!
//! See: context/lib/rendering_pipeline.md §12

use std::ffi::OsString;
use std::path::{Path, PathBuf};

use postretro_camera_route::{
    DEFAULT_HFOV_DEGREES, DEFAULT_STEPS, GeneratedKind, POSE_FILE_MAGIC, Smoothing, WALK_SEED,
    WalkKind,
};

use crate::flag::match_flag;

/// The engine's accepted horizontal FOV range, degrees: `MIN_FOV_DEG` and
/// `MAX_FOV_DEG` in `crates/postretro/src/camera.rs`. The tool cannot depend
/// on the engine, so the range is repeated here; playback refuses a file
/// outside it.
const MIN_HFOV_DEGREES: f32 = 60.0;
const MAX_HFOV_DEGREES: f32 = 130.0;
/// The engine camera's pitch bound, degrees, inclusive: `PITCH_LIMIT` in
/// `crates/postretro/src/camera.rs`, which the recorder can write exactly.
const MAX_PITCH_DEGREES: f32 = 89.0;

#[derive(Debug, Clone, PartialEq)]
pub(super) struct GenerateArgs {
    /// The level path as typed: the bake identity records it verbatim.
    pub(super) level: String,
    pub(super) kind: GeneratedKind,
    /// The walk the route follows: an exact kind's own, or `--walk` for a
    /// smoothed route.
    pub(super) walk: WalkKind,
    /// A smoothed route's limits; `None` for the exact kinds.
    pub(super) smoothing: Option<Smoothing>,
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
    let mut walk = None;
    let mut speed = None;
    let mut turn_cap = None;
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
        } else if let Some(value) = value_of("--walk")? {
            set_once(&mut walk, parse_walk(&value)?, "--walk")?;
        } else if let Some(value) = value_of("--speed")? {
            let value = parse_limit(&value, "--speed", "metres per second")?;
            set_once(&mut speed, value, "--speed")?;
        } else if let Some(value) = value_of("--turn-cap")? {
            let value = parse_limit(&value, "--turn-cap", "degrees per second")?;
            set_once(&mut turn_cap, value, "--turn-cap")?;
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

    let kind = kind.ok_or_else(|| usage("generate-camera-route requires --kind <kind>"))?;
    let (walk, smoothing) = match kind {
        GeneratedKind::FarPoint | GeneratedKind::Random => {
            let smoothed_only = [
                ("--walk", walk.is_some()),
                ("--speed", speed.is_some()),
                ("--turn-cap", turn_cap.is_some()),
            ];
            if let Some((flag, _)) = smoothed_only.into_iter().find(|&(_, given)| given) {
                return Err(usage(&format!("{flag} applies only to --kind smoothed")));
            }
            let walk = if kind == GeneratedKind::Random {
                WalkKind::Random
            } else {
                WalkKind::FarPointTour
            };
            (walk, None)
        }
        GeneratedKind::Smoothed => {
            let defaults = Smoothing::default();
            let smoothing = Smoothing {
                speed: speed.unwrap_or(defaults.speed),
                turn_cap_degrees: turn_cap.unwrap_or(defaults.turn_cap_degrees),
            };
            (walk.unwrap_or(WalkKind::FarPointTour), Some(smoothing))
        }
    };

    let out = out.ok_or_else(|| usage("generate-camera-route requires -o <out-file>"))?;
    check_output_folder(&out)?;
    check_existing_output(&out, level.as_deref())?;

    Ok(GenerateArgs {
        level: level.ok_or_else(|| usage("generate-camera-route requires a level path"))?,
        kind,
        walk,
        smoothing,
        seed: seed.unwrap_or(WALK_SEED),
        steps: steps.unwrap_or(DEFAULT_STEPS),
        pitch_degrees: pitch.unwrap_or(0.0),
        hfov_degrees: hfov.unwrap_or(DEFAULT_HFOV_DEGREES),
        out,
    })
}

/// `-o` names a file whose folder exists: the tool creates no folder, and
/// refuses before the level is read rather than after the route is built.
/// A trailing separator, `.` or `..` names a folder: `Path::parent` drops
/// or steps past it, so the folder check would test another folder.
fn check_output_folder(out: &Path) -> Result<(), String> {
    let text = out.to_string_lossy();
    let dot_segment = matches!(text.rsplit(['/', '\\']).next(), Some("." | ".."));
    if text.ends_with(['/', '\\']) || dot_segment || out.is_dir() {
        return Err(format!(
            "-o {}: name a file to write, not a folder",
            out.display()
        ));
    }
    // A bare file name's folder is the working directory.
    match out.parent() {
        Some(folder) if folder.as_os_str().is_empty() || folder.is_dir() => Ok(()),
        Some(folder) => Err(format!(
            "-o {}: its folder {} does not exist, and the tool creates none",
            out.display(),
            folder.display(),
        )),
        None => Err(format!("-o {} names no file", out.display())),
    }
}

/// Bytes read to identify an existing `-o` file: enough for a pose file's
/// magic line, so a level named by mistake is never read whole.
const IDENTIFY_BYTES: u64 = 256;

/// `-o` may replace an earlier pose file and nothing else: never the level
/// the route is generated from, nor another level or file named by mistake.
/// A pose file is known by its magic line alone, at any version, as the
/// engine's `--record-camera-route` launch check knows it: an older or
/// hand-edited route is still the author's own output to replace.
fn check_existing_output(out: &Path, level: Option<&str>) -> Result<(), String> {
    let metadata = match std::fs::metadata(out) {
        Ok(metadata) => metadata,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(err) => return Err(format!("-o {}: unreadable: {err}", out.display())),
    };
    if let Some(level) = level {
        // Both exist, so the file system resolves case, links and spelling.
        if let (Ok(out_path), Ok(level_path)) =
            (out.canonicalize(), Path::new(level).canonicalize())
            && out_path == level_path
        {
            return Err(format!(
                "-o {} is the level {level}: the route would overwrite the level it is made from",
                out.display()
            ));
        }
    }
    let refuse = || {
        format!(
            "-o {} exists and is not a camera-route pose file: the write would overwrite it; name another file",
            out.display()
        )
    };
    if !metadata.is_file() {
        return Err(refuse());
    }
    let head = read_head(out).map_err(|err| format!("-o {}: unreadable: {err}", out.display()))?;
    // An editor's "UTF-8 with BOM" save: the pose parser strips it too.
    let head = head.strip_prefix('\u{feff}').unwrap_or(&head);
    if is_pose_file_head(head) {
        Ok(())
    } else {
        Err(refuse())
    }
}

/// Whether `head` opens with a pose file's magic line at any version:
/// [`POSE_FILE_MAGIC`] up to its last space, then digits. Trailing
/// whitespace and a CR are allowed, as the parser allows them.
fn is_pose_file_head(head: &str) -> bool {
    let Some((prefix, _)) = POSE_FILE_MAGIC.rsplit_once(' ') else {
        return false;
    };
    head.lines()
        .next()
        .and_then(|line| line.trim_end().strip_prefix(prefix))
        .and_then(|rest| rest.strip_prefix(' '))
        .is_some_and(|version| !version.is_empty() && version.bytes().all(|b| b.is_ascii_digit()))
}

/// The first [`IDENTIFY_BYTES`] of `path`, lossily decoded.
fn read_head(path: &Path) -> std::io::Result<String> {
    use std::io::Read;
    let mut head = Vec::new();
    std::fs::File::open(path)?
        .take(IDENTIFY_BYTES)
        .read_to_end(&mut head)?;
    Ok(String::from_utf8_lossy(&head).into_owned())
}

fn parse_kind(value: &str) -> Result<GeneratedKind, String> {
    GeneratedKind::from_key(value).ok_or_else(|| {
        let valid: Vec<&str> = GeneratedKind::ALL.iter().map(|kind| kind.key()).collect();
        usage(&format!(
            "unknown --kind {value:?}; valid kinds: {}",
            valid.join(", ")
        ))
    })
}

fn parse_walk(value: &str) -> Result<WalkKind, String> {
    WalkKind::from_key(value).ok_or_else(|| {
        let valid: Vec<&str> = WalkKind::ALL.iter().map(|walk| walk.key()).collect();
        usage(&format!(
            "unknown --walk {value:?}; valid walks: {}",
            valid.join(", ")
        ))
    })
}

/// A smoothed route's speed or turn cap: finite and above zero, never
/// clamped, since a mod moves at its own speeds.
fn parse_limit(value: &str, flag: &str, unit: &str) -> Result<f32, String> {
    value
        .parse::<f32>()
        .ok()
        .filter(|limit| limit.is_finite() && *limit > 0.0)
        .ok_or_else(|| {
            usage(&format!(
                "{flag} expects a finite positive value in {unit}, got {value:?}"
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
    if degrees.abs() > MAX_PITCH_DEGREES {
        return Err(usage(&format!(
            "--pitch must lie in -{MAX_PITCH_DEGREES} to {MAX_PITCH_DEGREES} degrees, got {value}"
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

/// Every option with its range and default. A raw literal, like the tool's
/// `HELP`, so the indentation survives.
const OPTIONS: &str = r"  --kind far-point|random|smoothed  the route to generate (required)
  -o, --out <out-file>              the pose file to write (required)
  --seed <u64>                      the path's seed, decimal or hex (0x...); default 0x5eedb10c
  --steps <n>                       walk steps, at least 1; default 2000
  --pitch <degrees>                 fixed camera pitch, -89 to 89; default 0
  --hfov <degrees>                  horizontal FOV, 60 to 130; default 100
  --walk far-point|random           the walk a smoothed route follows; default far-point
  --speed <m/s>                     flight speed, above 0; default 8
  --turn-cap <deg/s>                yaw turn limit, above 0; default 270
  --walk, --speed and --turn-cap apply only to --kind smoothed.";

pub(super) fn usage(message: &str) -> String {
    format!(
        "{message}\n\nUsage: postretro-tool generate-camera-route <level.prl> \
         --kind far-point|random|smoothed -o <out-file> [options]\n\nOptions:\n{OPTIONS}"
    )
}
