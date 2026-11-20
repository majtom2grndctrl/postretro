// Camera-route playback's launch flags.
// See: context/lib/rendering_pipeline.md §12

use std::path::{Path, PathBuf};

use crate::startup::session::{
    CAMERA_ROUTE_FLAG, CAMERA_ROUTE_HOLD_FLAG, CAMERA_ROUTE_PACING_FLAG, CAMERA_ROUTE_REPORT_FLAG,
};

/// Hold frames when `--camera-route-hold` is absent: about 10 s at 60 Hz,
/// time to attach a GPU trace before the route starts.
pub(crate) const DEFAULT_HOLD_FRAMES: u32 = 600;

/// How route frames map to poses.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Pacing {
    /// Route frame k draws pose k, at any frame rate.
    FrameLocked,
}

impl Pacing {
    pub(crate) fn key(self) -> &'static str {
        match self {
            Self::FrameLocked => "frame-locked",
        }
    }
}

/// A playback launch, as the flags name it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct PlaybackArgs {
    pub(crate) poses: PathBuf,
    pub(crate) hold: u32,
    pub(crate) report: PathBuf,
    pub(crate) pacing: Pacing,
}

/// Reads `--camera-route <poses>` and its options, each also in
/// `--flag=value` form. `Ok(None)` when no route is named; a malformed or
/// missing value is an error naming the flag, since a run that silently
/// falls back to a default reads as a result.
pub(crate) fn playback_args(args: &[String]) -> Result<Option<PlaybackArgs>, String> {
    let poses = match flag_value(args, CAMERA_ROUTE_FLAG)? {
        Some(poses) => PathBuf::from(poses),
        None => {
            for flag in [
                CAMERA_ROUTE_HOLD_FLAG,
                CAMERA_ROUTE_REPORT_FLAG,
                CAMERA_ROUTE_PACING_FLAG,
            ] {
                if flag_value(args, flag).is_ok_and(|value| value.is_some()) {
                    log::warn!("[Playback] {flag} given without {CAMERA_ROUTE_FLAG}; ignored");
                }
            }
            return Ok(None);
        }
    };
    let hold = match flag_value(args, CAMERA_ROUTE_HOLD_FLAG)? {
        Some(value) => value.parse::<u32>().map_err(|_| {
            format!("{CAMERA_ROUTE_HOLD_FLAG} {value:?} is not a whole number of frames")
        })?,
        None => DEFAULT_HOLD_FRAMES,
    };
    let report = match flag_value(args, CAMERA_ROUTE_REPORT_FLAG)? {
        Some(value) => PathBuf::from(value),
        None => default_report_path(&poses),
    };
    let pacing = match flag_value(args, CAMERA_ROUTE_PACING_FLAG)? {
        None | Some("frame-locked") => Pacing::FrameLocked,
        Some("wall-clock") => {
            return Err(format!(
                "{CAMERA_ROUTE_PACING_FLAG} wall-clock is not supported yet; use frame-locked"
            ));
        }
        Some(other) => {
            return Err(format!(
                "{CAMERA_ROUTE_PACING_FLAG} {other:?} is not frame-locked or wall-clock"
            ));
        }
    };
    Ok(Some(PlaybackArgs {
        poses,
        hold,
        report,
        pacing,
    }))
}

/// `<poses stem>.playback.json` in the working directory.
fn default_report_path(poses: &Path) -> PathBuf {
    let stem = poses
        .file_stem()
        .map_or_else(|| "camera-route".into(), |stem| stem.to_string_lossy());
    PathBuf::from(format!("{stem}.playback.json"))
}

/// The first occurrence of `flag`: `Ok(None)` when absent, an error when it
/// carries no value.
fn flag_value<'a>(args: &'a [String], flag: &str) -> Result<Option<&'a str>, String> {
    let mut iter = args.iter().skip(1).peekable();
    while let Some(arg) = iter.next() {
        let value = if arg == flag {
            iter.next_if(|value| !value.is_empty() && !value.starts_with("--"))
                .map(String::as_str)
        } else if let Some(value) = arg
            .strip_prefix(flag)
            .and_then(|rest| rest.strip_prefix('='))
        {
            Some(value).filter(|value| !value.is_empty())
        } else {
            continue;
        };
        return value
            .map(Some)
            .ok_or_else(|| format!("{flag} needs a value"));
    }
    Ok(None)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(list: &[&str]) -> Vec<String> {
        std::iter::once("postretro")
            .chain(list.iter().copied())
            .map(str::to_string)
            .collect()
    }

    #[test]
    fn playback_args_absent_without_a_route_flag() {
        assert_eq!(playback_args(&args(&["map.prl"])), Ok(None));
        assert_eq!(
            playback_args(&args(&["--camera-route-hold", "5", "map.prl"])),
            Ok(None),
            "options alone start no playback"
        );
    }

    #[test]
    fn playback_args_default_hold_report_and_pacing() {
        let parsed = playback_args(&args(&["--camera-route", "routes/cp.txt", "map.prl"]))
            .unwrap()
            .unwrap();
        assert_eq!(
            parsed,
            PlaybackArgs {
                poses: PathBuf::from("routes/cp.txt"),
                hold: DEFAULT_HOLD_FRAMES,
                report: PathBuf::from("cp.playback.json"),
                pacing: Pacing::FrameLocked,
            }
        );
    }

    #[test]
    fn playback_args_accept_split_and_equals_forms() {
        let parsed = playback_args(&args(&[
            "--camera-route=cp.txt",
            "--camera-route-hold",
            "0",
            "--camera-route-report=out/run.json",
            "--camera-route-pacing",
            "frame-locked",
        ]))
        .unwrap()
        .unwrap();
        assert_eq!(parsed.poses, PathBuf::from("cp.txt"));
        assert_eq!(parsed.hold, 0);
        assert_eq!(parsed.report, PathBuf::from("out/run.json"));
        assert_eq!(parsed.pacing, Pacing::FrameLocked);
    }

    #[test]
    fn playback_args_refuse_missing_or_malformed_values() {
        for (list, flag) in [
            (&["--camera-route"][..], CAMERA_ROUTE_FLAG),
            (&["--camera-route", "--windowed"][..], CAMERA_ROUTE_FLAG),
            (
                &["--camera-route", "r.txt", "--camera-route-hold", "-3"][..],
                CAMERA_ROUTE_HOLD_FLAG,
            ),
            (
                &["--camera-route", "r.txt", "--camera-route-hold", "ten"][..],
                CAMERA_ROUTE_HOLD_FLAG,
            ),
            (
                &["--camera-route", "r.txt", "--camera-route-report"][..],
                CAMERA_ROUTE_REPORT_FLAG,
            ),
            (
                &["--camera-route", "r.txt", "--camera-route-pacing", "fast"][..],
                CAMERA_ROUTE_PACING_FLAG,
            ),
        ] {
            let error = playback_args(&args(list)).unwrap_err();
            assert!(error.contains(flag), "{list:?}: {error}");
        }
    }

    #[test]
    fn playback_args_refuse_wall_clock_pacing_until_it_lands() {
        let error = playback_args(&args(&[
            "--camera-route",
            "r.txt",
            "--camera-route-pacing=wall-clock",
        ]))
        .unwrap_err();
        assert!(error.contains("wall-clock is not supported yet"), "{error}");
    }
}
