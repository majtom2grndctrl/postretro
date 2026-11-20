// Camera-route playback's launch flags.
// See: context/lib/rendering_pipeline.md §12

use std::path::{Path, PathBuf};
use std::time::Duration;

use postretro_renderer::PresentChoice;

use crate::startup::camera_route_flags::{
    CAMERA_ROUTE_FLAG, CAMERA_ROUTE_HOLD_FLAG, CAMERA_ROUTE_PACING_FLAG, CAMERA_ROUTE_REPORT_FLAG,
    CAMERA_ROUTE_STALL_TIMEOUT_FLAG, PRESENT_MODE_FLAG, RECORD_CAMERA_ROUTE_FLAG,
};

/// Hold frames when `--camera-route-hold` is absent: about 10 s at 60 Hz,
/// time to attach a GPU trace before the route starts.
pub(crate) const DEFAULT_HOLD_FRAMES: u32 = 600;

/// The stall timeout when `--camera-route-stall-timeout` is absent. It must
/// outlast a load plus Settling's own 10 s timeout: campaign-test takes about
/// 72 s to load in a `dev` build, and levels grow, so the margin is wide.
pub(crate) const DEFAULT_STALL_TIMEOUT: Duration = Duration::from_secs(300);

/// How route frames map to poses.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Pacing {
    /// Route frame k draws pose k, at any frame rate.
    FrameLocked,
    /// A route frame draws the pose at its elapsed time since route frame 0,
    /// at the file's cadence, interpolated between poses.
    WallClock,
}

impl Pacing {
    pub(crate) fn key(self) -> &'static str {
        match self {
            Self::FrameLocked => "frame-locked",
            Self::WallClock => "wall-clock",
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
    pub(crate) stall_timeout: Duration,
    /// The launch present mode the run draws under from its first frame.
    pub(crate) present: PresentChoice,
}

/// Reads `--camera-route <poses>` and its options, each also in
/// `--flag=value` form. `Ok(None)` when no route is named; a malformed or
/// missing value is an error naming the flag, since a run that silently
/// falls back to a default reads as a result. A route in a session launched
/// with `--connect`, or beside `--record-camera-route`, is refused.
pub(crate) fn playback_args(args: &[String]) -> Result<Option<PlaybackArgs>, String> {
    let poses = match flag_value(args, CAMERA_ROUTE_FLAG)? {
        Some(poses) => PathBuf::from(poses),
        None => {
            if names_flag(args, RECORD_CAMERA_ROUTE_FLAG) {
                log::warn!("[Playback] {RECORD_CAMERA_ROUTE_FLAG} is not built yet; ignored");
            }
            for flag in [
                CAMERA_ROUTE_HOLD_FLAG,
                CAMERA_ROUTE_REPORT_FLAG,
                CAMERA_ROUTE_PACING_FLAG,
                CAMERA_ROUTE_STALL_TIMEOUT_FLAG,
            ] {
                if flag_value(args, flag).is_ok_and(|value| value.is_some()) {
                    log::warn!("[Playback] {flag} given without {CAMERA_ROUTE_FLAG}; ignored");
                }
            }
            return Ok(None);
        }
    };
    if names_flag(args, RECORD_CAMERA_ROUTE_FLAG) {
        return Err(format!(
            "{CAMERA_ROUTE_FLAG} and {RECORD_CAMERA_ROUTE_FLAG} cannot run together: a launch either plays a route or records one"
        ));
    }
    // The netcode parses `--connect` later; a client's camera follows its
    // host's world, which a route cannot measure.
    if names_flag(args, "--connect") {
        return Err(format!(
            "{CAMERA_ROUTE_FLAG} refuses a session launched with --connect: playback measures a local level, not a connected client's"
        ));
    }
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
        Some("wall-clock") => Pacing::WallClock,
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
        stall_timeout: stall_timeout(args)?,
        present: present_choice(args)?,
    }))
}

/// `--present-mode vsync|no-vsync`, vsync when absent. It applies to any
/// session, a route or not, so recording and ordinary measurement runs can
/// take it too.
pub(crate) fn present_choice(args: &[String]) -> Result<PresentChoice, String> {
    match flag_value(args, PRESENT_MODE_FLAG)? {
        None | Some("vsync") => Ok(PresentChoice::Vsync),
        Some("no-vsync") => Ok(PresentChoice::NoVsync),
        Some(other) => Err(format!(
            "{PRESENT_MODE_FLAG} {other:?} is not vsync or no-vsync"
        )),
    }
}

/// `--camera-route-stall-timeout <seconds>`: a positive, finite number.
pub(crate) fn stall_timeout(args: &[String]) -> Result<Duration, String> {
    let Some(value) = flag_value(args, CAMERA_ROUTE_STALL_TIMEOUT_FLAG)? else {
        return Ok(DEFAULT_STALL_TIMEOUT);
    };
    value
        .parse::<f64>()
        .ok()
        .filter(|seconds| seconds.is_finite() && *seconds > 0.0)
        .and_then(|seconds| Duration::try_from_secs_f64(seconds).ok())
        .ok_or_else(|| {
            format!(
                "{CAMERA_ROUTE_STALL_TIMEOUT_FLAG} {value:?} is not a positive number of seconds"
            )
        })
}

/// `flag` appears as its own argument or in `--flag=value` form.
fn names_flag(args: &[String], flag: &str) -> bool {
    args.iter().skip(1).any(|arg| {
        arg == flag
            || arg
                .strip_prefix(flag)
                .is_some_and(|rest| rest.starts_with('='))
    })
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
                stall_timeout: DEFAULT_STALL_TIMEOUT,
                present: PresentChoice::Vsync,
            }
        );
    }

    #[test]
    fn present_choice_defaults_to_vsync_and_reads_either_form() {
        assert_eq!(
            present_choice(&args(&["map.prl"])),
            Ok(PresentChoice::Vsync)
        );
        assert_eq!(
            present_choice(&args(&["--present-mode", "no-vsync", "map.prl"])),
            Ok(PresentChoice::NoVsync)
        );
        assert_eq!(
            present_choice(&args(&["--present-mode=vsync"])),
            Ok(PresentChoice::Vsync)
        );
        let parsed = playback_args(&args(&[
            "--camera-route",
            "r.txt",
            "--present-mode=no-vsync",
        ]))
        .unwrap()
        .unwrap();
        assert_eq!(parsed.present, PresentChoice::NoVsync);
    }

    #[test]
    fn present_choice_refuses_a_missing_or_unknown_mode_naming_the_flag() {
        for list in [
            &["--present-mode"][..],
            &["--present-mode", "--windowed"][..],
            &["--present-mode", "fast"][..],
            &["--present-mode=off"][..],
        ] {
            let error = present_choice(&args(list)).unwrap_err();
            assert!(error.contains(PRESENT_MODE_FLAG), "{list:?}: {error}");
        }
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
    fn playback_args_read_wall_clock_pacing_and_the_stall_timeout() {
        let parsed = playback_args(&args(&[
            "--camera-route",
            "r.txt",
            "--camera-route-pacing=wall-clock",
            "--camera-route-stall-timeout",
            "2.5",
        ]))
        .unwrap()
        .unwrap();
        assert_eq!(parsed.pacing, Pacing::WallClock);
        assert_eq!(parsed.pacing.key(), "wall-clock");
        assert_eq!(parsed.stall_timeout, Duration::from_millis(2_500));
        for bad in ["0", "-1", "soon", "inf"] {
            let error = playback_args(&args(&[
                "--camera-route",
                "r.txt",
                "--camera-route-stall-timeout",
                bad,
            ]))
            .unwrap_err();
            assert!(
                error.contains(CAMERA_ROUTE_STALL_TIMEOUT_FLAG),
                "{bad}: {error}"
            );
        }
    }

    #[test]
    fn playback_args_refuse_a_connected_client() {
        for list in [
            &["--camera-route", "r.txt", "--connect", "127.0.0.1:27015"][..],
            &["--connect=127.0.0.1:27015", "--camera-route", "r.txt"][..],
        ] {
            let error = playback_args(&args(list)).unwrap_err();
            assert!(
                error.contains("--camera-route") && error.contains("--connect"),
                "{list:?}: {error}"
            );
        }
        assert!(
            playback_args(&args(&["--connect", "127.0.0.1:27015"]))
                .unwrap()
                .is_none(),
            "a client without a route is no playback"
        );
    }

    #[test]
    fn playback_args_refuse_recording_beside_playback_naming_both_flags() {
        for list in [
            &[
                "--camera-route",
                "r.txt",
                "--record-camera-route",
                "rec.txt",
            ][..],
            &["--record-camera-route=rec.txt", "--camera-route=r.txt"][..],
        ] {
            let error = playback_args(&args(list)).unwrap_err();
            assert!(
                error.contains("--camera-route ") && error.contains("--record-camera-route"),
                "{list:?}: {error}"
            );
        }
    }
}
