// Camera-route playback's launch flags.
// See: context/lib/rendering_pipeline.md §12

use std::path::{Path, PathBuf};
use std::time::Duration;

use postretro_camera_route::POSE_FILE_MAGIC;
use postretro_renderer::PresentChoice;

use crate::startup::camera_route_flags::{
    CAMERA_ROUTE_FLAG, CAMERA_ROUTE_HOLD_FLAG, CAMERA_ROUTE_PACING_FLAG, CAMERA_ROUTE_REPORT_FLAG,
    CAMERA_ROUTE_STALL_TIMEOUT_FLAG, PRESENT_MODE_FLAG, RECORD_CAMERA_ROUTE_FLAG,
};
use crate::startup::cli_flags::CONNECT_FLAG;

use super::record::SCHEMA;

/// Hold frames when `--camera-route-hold` is absent: about 10 s at 60 Hz,
/// time to attach a GPU trace before the route starts.
pub(crate) const DEFAULT_HOLD_FRAMES: u32 = 600;

/// The stall timeout when `--camera-route-stall-timeout` is absent. It must
/// outlast a load plus Settling's own 10 s timeout: campaign-test takes about
/// 72 s to load in a `dev` build, and levels grow, so the margin is wide.
pub(crate) const DEFAULT_STALL_TIMEOUT: Duration = Duration::from_secs(300);

/// The longest `--camera-route-hold`: about 28 minutes at 60 Hz. The record
/// reserves a row per hold frame at launch, so the flag alone would otherwise
/// size that reservation, and a huge one aborts the process.
pub(crate) const MAX_HOLD_FRAMES: u32 = 100_000;

/// The longest `--camera-route-stall-timeout`: a day. The deadline is the
/// launch instant plus the timeout, and an `Instant` that far out overflows
/// and panics on some platforms.
pub(crate) const MAX_STALL_TIMEOUT: Duration = Duration::from_secs(86_400);

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
            // A recording takes the stall timeout too.
            let recording = names_flag(args, RECORD_CAMERA_ROUTE_FLAG);
            for flag in [
                CAMERA_ROUTE_HOLD_FLAG,
                CAMERA_ROUTE_REPORT_FLAG,
                CAMERA_ROUTE_PACING_FLAG,
                CAMERA_ROUTE_STALL_TIMEOUT_FLAG,
            ] {
                if (recording && flag == CAMERA_ROUTE_STALL_TIMEOUT_FLAG) || !names_flag(args, flag)
                {
                    continue;
                }
                log::warn!("[Playback] {flag} given without {CAMERA_ROUTE_FLAG}; ignored");
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
    if names_flag(args, CONNECT_FLAG) {
        return Err(format!(
            "{CAMERA_ROUTE_FLAG} refuses a session launched with --connect: playback measures a local level, not a connected client's"
        ));
    }
    let hold = match flag_value(args, CAMERA_ROUTE_HOLD_FLAG)? {
        Some(value) => value
            .parse::<u32>()
            .ok()
            .filter(|hold| *hold <= MAX_HOLD_FRAMES)
            .ok_or_else(|| {
                format!(
                    "{CAMERA_ROUTE_HOLD_FLAG} {value:?} is not a whole number of frames from 0 to {MAX_HOLD_FRAMES}"
                )
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

/// A recording launch, as the flags name it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct RecordArgs {
    /// Where the pose file is written, once.
    pub(crate) path: PathBuf,
    pub(crate) stall_timeout: Duration,
}

/// Reads `--record-camera-route <path>` and the stall timeout it arms.
/// `Ok(None)` when no recording is named. Call it only when
/// [`playback_args`] found no route: that refuses the two together.
pub(crate) fn record_args(args: &[String]) -> Result<Option<RecordArgs>, String> {
    let Some(path) = flag_value(args, RECORD_CAMERA_ROUTE_FLAG)? else {
        return Ok(None);
    };
    Ok(Some(RecordArgs {
        path: PathBuf::from(path),
        stall_timeout: stall_timeout(args)?,
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

/// `--camera-route-stall-timeout <seconds>`: a positive number up to
/// [`MAX_STALL_TIMEOUT`].
pub(crate) fn stall_timeout(args: &[String]) -> Result<Duration, String> {
    let Some(value) = flag_value(args, CAMERA_ROUTE_STALL_TIMEOUT_FLAG)? else {
        return Ok(DEFAULT_STALL_TIMEOUT);
    };
    value
        .parse::<f64>()
        .ok()
        .filter(|seconds| *seconds > 0.0 && *seconds <= MAX_STALL_TIMEOUT.as_secs_f64())
        .and_then(|seconds| Duration::try_from_secs_f64(seconds).ok())
        .ok_or_else(|| {
            format!(
                "{CAMERA_ROUTE_STALL_TIMEOUT_FLAG} {value:?} is not a positive number of seconds up to {}",
                MAX_STALL_TIMEOUT.as_secs()
            )
        })
}

/// Where a playback run writes, checked at launch before any window: the
/// record's folder exists, the record is not the route file itself, which
/// its one write would overwrite, and an existing file there is an older
/// playback record. Nothing is created; an existing file is only read.
/// Relative paths resolve against `cwd`, as the write will.
pub(crate) fn check_report_path(report: &Path, poses: &Path, cwd: &Path) -> Result<(), String> {
    check_output_folder(CAMERA_ROUTE_REPORT_FLAG, report, cwd)?;
    if names_one_file(report, poses, cwd) {
        return Err(format!(
            "{CAMERA_ROUTE_REPORT_FLAG} {} is the route file {}: the record would overwrite the route it plays",
            report.display(),
            poses.display(),
        ));
    }
    check_existing_output(
        CAMERA_ROUTE_REPORT_FLAG,
        report,
        cwd,
        Output::PlaybackRecord,
    )
}

/// Where a recording writes, checked at launch like the record's path: an
/// existing file there must be an older pose file.
pub(crate) fn check_record_path(path: &Path, cwd: &Path) -> Result<(), String> {
    check_output_folder(RECORD_CAMERA_ROUTE_FLAG, path, cwd)?;
    check_existing_output(RECORD_CAMERA_ROUTE_FLAG, path, cwd, Output::PoseFile)
}

/// `path` names a file whose folder exists; the engine creates no folder.
/// A trailing separator, `.` or `..` names a folder: `Path::parent` and
/// `Path::components` drop or step past it, so the folder check would test
/// another folder. The write would then fail at the run's end, or on
/// Windows land in a file named after the folder.
fn check_output_folder(flag: &str, path: &Path, cwd: &Path) -> Result<(), String> {
    let text = path.to_string_lossy();
    if text.ends_with(['/', '\\']) {
        return Err(format!(
            "{flag} {} ends in a separator: name the file to write",
            path.display()
        ));
    }
    if matches!(text.rsplit(['/', '\\']).next(), Some("." | "..")) {
        return Err(format!(
            "{flag} {} ends in `.` or `..`, which name a folder: name the file to write",
            path.display()
        ));
    }
    let absolute = cwd.join(path);
    if absolute.is_dir() {
        return Err(format!(
            "{flag} {} is a folder: name the file to write",
            path.display()
        ));
    }
    match absolute.parent() {
        Some(folder) if folder.is_dir() => Ok(()),
        Some(folder) => Err(format!(
            "{flag} {}: its folder {} does not exist, and the engine creates none",
            path.display(),
            folder.display(),
        )),
        None => Err(format!("{flag} {} names no file", path.display())),
    }
}

/// The kind of file an output flag writes, and so the only kind it may
/// overwrite.
#[derive(Debug, Clone, Copy)]
enum Output {
    /// JSON whose header names a playback record schema of any version.
    PlaybackRecord,
    /// A camera-route pose file of any version, by its magic line.
    PoseFile,
}

/// Bytes read to identify an existing output: enough for a pose file's
/// magic line or a record's opening `schema` field. Nothing past them is
/// read, so a level or other large file named by mistake costs no launch
/// time.
const IDENTIFY_BYTES: u64 = 256;

/// An existing file at `path` must already be of `kind`: an older record or
/// recording is overwritten, but a level or any other file named by mistake
/// (`resolve_map_path` steps over a flag's value) is refused, never replaced.
///
/// The opening bytes alone identify the kind, any version of it. The check
/// catches a wrong file named by mistake, and those bytes settle that; an
/// older or hand-edited file the current parser refuses is still the
/// person's own output to replace.
fn check_existing_output(flag: &str, path: &Path, cwd: &Path, kind: Output) -> Result<(), String> {
    let absolute = cwd.join(path);
    let metadata = match std::fs::metadata(&absolute) {
        Ok(metadata) => metadata,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(err) => return Err(format!("{flag} {}: unreadable: {err}", path.display())),
    };
    let refuse = |what: &str| {
        format!(
            "{flag} {} exists and is not {what}: the write would overwrite it; name another file",
            path.display()
        )
    };
    if !metadata.is_file() {
        return Err(refuse("a file"));
    }
    let head = read_head(&absolute)
        .map_err(|err| format!("{flag} {}: unreadable: {err}", path.display()))?;
    // An editor's "UTF-8 with BOM" save: the pose parser strips it too.
    let head = head.strip_prefix('\u{feff}').unwrap_or(&head);
    match kind {
        Output::PlaybackRecord => {
            // serde writes the header first and the schema as its first
            // field, as the record test
            // `playback_record_written_file_passes_the_launch_overwrite_check`
            // pins.
            let family = record_schema_family();
            let compact: String = head.chars().filter(|c| !c.is_ascii_whitespace()).collect();
            let opening = format!("{{\"header\":{{\"schema\":\"{family}");
            if compact.starts_with(&opening) {
                Ok(())
            } else {
                Err(refuse(&format!("a playback record ({family}*)")))
            }
        }
        Output::PoseFile => {
            if is_pose_file_head(head) {
                Ok(())
            } else {
                Err(refuse("a camera-route pose file"))
            }
        }
    }
}

/// [`SCHEMA`] up to and including its last `.`, the version dropped: a
/// record of an older version still identifies as a playback record.
fn record_schema_family() -> &'static str {
    SCHEMA.rfind('.').map_or(SCHEMA, |dot| &SCHEMA[..=dot])
}

/// Whether `head`, a file's opening text past any BOM, opens with a pose
/// file's magic line at any version: [`POSE_FILE_MAGIC`] up to its last
/// space, then digits. Trailing whitespace and a CR are allowed, as the
/// parser allows them.
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

/// Whether `a` and `b` name one file. When both exist they compare as the
/// file system resolves them: by case on a case-insensitive volume, through
/// symlinks, and on Windows past trailing dots and spaces. Otherwise they
/// compare as spelled, folder canonicalized.
fn names_one_file(a: &Path, b: &Path, cwd: &Path) -> bool {
    let (a, b) = (cwd.join(a), cwd.join(b));
    if let (Ok(canonical_a), Ok(canonical_b)) = (a.canonicalize(), b.canonicalize()) {
        return canonical_a == canonical_b || same_inode(&canonical_a, &canonical_b);
    }
    resolved(&a) == resolved(&b)
}

/// One inode under two names: macOS `canonicalize` may keep a name's case
/// as typed on a case-insensitive volume.
#[cfg(unix)]
fn same_inode(a: &Path, b: &Path) -> bool {
    use std::os::unix::fs::MetadataExt;
    match (std::fs::metadata(a), std::fs::metadata(b)) {
        (Ok(a), Ok(b)) => a.dev() == b.dev() && a.ino() == b.ino(),
        _ => false,
    }
}

/// Windows' `canonicalize` returns the on-disk final path, which already
/// resolves case and links.
#[cfg(not(unix))]
fn same_inode(_a: &Path, _b: &Path) -> bool {
    false
}

/// An absolute path with its folder canonicalized and, on Windows, the
/// trailing dots and spaces the file system drops stripped from its name:
/// two spellings of a file not yet written compare equal.
fn resolved(absolute: &Path) -> PathBuf {
    match (absolute.parent(), absolute.file_name()) {
        (Some(folder), Some(name)) => {
            let name = win32_file_name(name);
            folder
                .canonicalize()
                .map_or_else(|_| folder.join(&name), |folder| folder.join(&name))
        }
        _ => absolute.to_path_buf(),
    }
}

/// Windows opens `run.json.` and `run.json ` as `run.json`.
#[cfg(windows)]
fn win32_file_name(name: &std::ffi::OsStr) -> std::ffi::OsString {
    let text = name.to_string_lossy();
    let trimmed = text.trim_end_matches(['.', ' ']);
    if trimmed.is_empty() {
        name.to_os_string()
    } else {
        trimmed.into()
    }
}

#[cfg(not(windows))]
fn win32_file_name(name: &std::ffi::OsStr) -> std::ffi::OsString {
    name.to_os_string()
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

/// The value of `flag`: `Ok(None)` when absent, an error when it carries no
/// value or is given twice, since silently taking either would read as a
/// result of the other.
fn flag_value<'a>(args: &'a [String], flag: &str) -> Result<Option<&'a str>, String> {
    let mut found = None;
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
        if found.is_some() {
            return Err(format!("{flag} is given twice: name it once"));
        }
        found = Some(value.ok_or_else(|| format!("{flag} needs a value"))?);
    }
    Ok(found)
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
    fn playback_args_bound_the_hold_and_the_stall_timeout_naming_the_flag() {
        let route = |extra: &[&str]| {
            let mut list = vec!["--camera-route", "r.txt"];
            list.extend_from_slice(extra);
            playback_args(&args(&list))
        };
        let max_hold = MAX_HOLD_FRAMES.to_string();
        assert_eq!(
            route(&["--camera-route-hold", &max_hold])
                .unwrap()
                .unwrap()
                .hold,
            MAX_HOLD_FRAMES
        );
        for bad in [(MAX_HOLD_FRAMES + 1).to_string(), u32::MAX.to_string()] {
            let error = route(&["--camera-route-hold", &bad]).unwrap_err();
            assert!(
                error.contains(CAMERA_ROUTE_HOLD_FLAG) && error.contains(&max_hold),
                "{bad}: {error}"
            );
        }
        assert_eq!(
            route(&["--camera-route-stall-timeout", "86400"])
                .unwrap()
                .unwrap()
                .stall_timeout,
            MAX_STALL_TIMEOUT
        );
        for bad in ["86400.5", "1e19", "NaN"] {
            let error = route(&["--camera-route-stall-timeout", bad]).unwrap_err();
            assert!(
                error.contains(CAMERA_ROUTE_STALL_TIMEOUT_FLAG) && error.contains("86400"),
                "{bad}: {error}"
            );
        }
        let error = record_args(&args(&[
            "--record-camera-route",
            "rec.txt",
            "--camera-route-stall-timeout=1e19",
        ]))
        .unwrap_err();
        assert!(error.contains(CAMERA_ROUTE_STALL_TIMEOUT_FLAG), "{error}");
    }

    #[test]
    fn camera_route_flags_given_twice_are_refused_naming_the_flag() {
        for (list, flag) in [
            (
                &["--camera-route", "a.txt", "--camera-route=b.txt"][..],
                CAMERA_ROUTE_FLAG,
            ),
            (
                &[
                    "--camera-route",
                    "r.txt",
                    "--camera-route-hold",
                    "5",
                    "--camera-route-hold=6",
                ][..],
                CAMERA_ROUTE_HOLD_FLAG,
            ),
            (
                &[
                    "--camera-route",
                    "r.txt",
                    "--camera-route-report",
                    "a.json",
                    "--camera-route-report",
                    "a.json",
                ][..],
                CAMERA_ROUTE_REPORT_FLAG,
            ),
            (
                &[
                    "--camera-route",
                    "r.txt",
                    "--camera-route-pacing=wall-clock",
                    "--camera-route-pacing=frame-locked",
                ][..],
                CAMERA_ROUTE_PACING_FLAG,
            ),
            (
                &[
                    "--camera-route",
                    "r.txt",
                    "--camera-route-stall-timeout=3",
                    "--camera-route-stall-timeout=4",
                ][..],
                CAMERA_ROUTE_STALL_TIMEOUT_FLAG,
            ),
            (
                &[
                    "--camera-route",
                    "r.txt",
                    "--present-mode=vsync",
                    "--present-mode",
                    "no-vsync",
                ][..],
                PRESENT_MODE_FLAG,
            ),
        ] {
            let error = playback_args(&args(list)).unwrap_err();
            assert!(
                error.contains(flag) && error.contains("twice"),
                "{list:?}: {error}"
            );
        }
        let error = record_args(&args(&[
            "--record-camera-route=a.txt",
            "--record-camera-route",
            "b.txt",
        ]))
        .unwrap_err();
        assert!(
            error.contains(RECORD_CAMERA_ROUTE_FLAG) && error.contains("twice"),
            "{error}"
        );
    }

    #[test]
    fn output_paths_need_an_existing_folder_and_never_name_the_route() {
        let dir = tempfile::tempdir().unwrap();
        let cwd = dir.path();
        std::fs::create_dir(cwd.join("routes")).unwrap();
        let route = Path::new("routes/cp.txt");
        std::fs::write(cwd.join(route), "").unwrap();

        assert_eq!(
            check_report_path(Path::new("cp.playback.json"), route, cwd),
            Ok(()),
            "a bare name writes into the working directory"
        );
        assert_eq!(
            check_report_path(&cwd.join("routes/run.json"), route, cwd),
            Ok(())
        );
        let missing = check_report_path(Path::new("out/run.json"), route, cwd).unwrap_err();
        assert!(
            missing.contains(CAMERA_ROUTE_REPORT_FLAG) && missing.contains("does not exist"),
            "{missing}"
        );
        let folder = check_report_path(Path::new("routes"), route, cwd).unwrap_err();
        assert!(folder.contains("is a folder"), "{folder}");
        for same in [
            PathBuf::from("routes/cp.txt"),
            PathBuf::from("routes/../routes/cp.txt"),
            cwd.join("routes").join("cp.txt"),
        ] {
            let error = check_report_path(&same, route, cwd).unwrap_err();
            assert!(
                error.contains(CAMERA_ROUTE_REPORT_FLAG) && error.contains("overwrite"),
                "{}: {error}",
                same.display()
            );
        }
        assert!(
            !cwd.join("cp.playback.json").exists() && !cwd.join("out").exists(),
            "the checks create nothing"
        );

        assert_eq!(check_record_path(Path::new("routes/rec.txt"), cwd), Ok(()));
        let error = check_record_path(Path::new("missing/rec.txt"), cwd).unwrap_err();
        assert!(
            error.contains(RECORD_CAMERA_ROUTE_FLAG) && error.contains("does not exist"),
            "{error}"
        );
        assert!(!cwd.join("missing").exists());
    }

    // Regression: `Path::parent` ignores a trailing separator, so the folder
    // check passed and the write failed at the run's end.
    #[test]
    fn output_paths_ending_in_a_separator_are_refused_naming_the_flag() {
        let dir = tempfile::tempdir().unwrap();
        let cwd = dir.path();
        let route = Path::new("cp.txt");
        for path in ["out/", "out\\", "rec/"] {
            let error = check_report_path(Path::new(path), route, cwd).unwrap_err();
            assert!(
                error.contains(CAMERA_ROUTE_REPORT_FLAG) && error.contains("separator"),
                "{path}: {error}"
            );
            let error = check_record_path(Path::new(path), cwd).unwrap_err();
            assert!(
                error.contains(RECORD_CAMERA_ROUTE_FLAG) && error.contains("separator"),
                "{path}: {error}"
            );
        }
        assert!(!cwd.join("out").exists() && !cwd.join("rec").exists());
    }

    // Regression: `Path::components` drops a trailing `.`, so `missing/.`
    // checked the working directory and passed; the write then failed at
    // the run's end, or on Windows wrote a file named `missing`.
    #[test]
    fn output_paths_ending_in_a_dot_segment_are_refused_naming_the_flag() {
        let dir = tempfile::tempdir().unwrap();
        let cwd = dir.path();
        std::fs::create_dir(cwd.join("out")).unwrap();
        let route = Path::new("cp.txt");
        for path in [
            "missing/.",
            "missing\\.",
            "missing/..",
            "out/.",
            "out\\..",
            ".",
            "..",
        ] {
            let error = check_report_path(Path::new(path), route, cwd).unwrap_err();
            assert!(
                error.contains(CAMERA_ROUTE_REPORT_FLAG) && error.contains("`.` or `..`"),
                "{path}: {error}"
            );
            let error = check_record_path(Path::new(path), cwd).unwrap_err();
            assert!(
                error.contains(RECORD_CAMERA_ROUTE_FLAG) && error.contains("`.` or `..`"),
                "{path}: {error}"
            );
        }
        assert!(!cwd.join("missing").exists(), "the checks create nothing");
        assert_eq!(
            check_record_path(Path::new("out/.rec.txt"), cwd),
            Ok(()),
            "a name merely starting with a dot is a file"
        );
    }

    /// The route the spelling tests play, relative to their working
    /// directory.
    const ROUTE: &str = "routes/cp.txt";

    /// A working directory holding [`ROUTE`].
    fn with_route() -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir(dir.path().join("routes")).unwrap();
        std::fs::write(dir.path().join(ROUTE), "route").unwrap();
        dir
    }

    /// Whether the record check refuses `report` as the route itself.
    fn overwrites_route(report: &str, cwd: &Path) -> bool {
        check_report_path(Path::new(report), Path::new(ROUTE), cwd)
            .is_err_and(|error| error.contains("overwrite the route"))
    }

    fn assert_route_unwritten(cwd: &Path) {
        assert_eq!(
            std::fs::read_to_string(cwd.join(ROUTE)).unwrap(),
            "route",
            "the checks never write"
        );
    }

    // Regression: only the folder was canonicalized, so another spelling of
    // the route's own name passed and the record overwrote the route.
    //
    // A case-insensitive volume (Windows NTFS, macOS default APFS) exercises
    // the refusal. A case-sensitive one (Linux) exercises the accept: there
    // the spelling names another file.
    #[test]
    fn report_path_naming_the_route_in_another_case_is_refused() {
        let dir = with_route();
        let cwd = dir.path();
        let upper = "routes/CP.TXT";
        if cwd.join(upper).exists() {
            assert!(
                overwrites_route(upper, cwd),
                "a case-folded spelling is the route"
            );
        } else {
            assert_eq!(
                check_report_path(Path::new(upper), Path::new(ROUTE), cwd),
                Ok(())
            );
        }
        assert_route_unwritten(cwd);
    }

    // Regression: as above, by trailing dots or spaces. Windows only: it
    // opens such a name as the name without them; other platforms keep
    // them, so the spelling names another file.
    #[cfg(windows)]
    #[test]
    fn report_path_naming_the_route_with_trailing_dots_or_spaces_is_refused() {
        let dir = with_route();
        let cwd = dir.path();
        for spelling in ["routes/cp.txt.", "routes/cp.txt ", "routes/cp.txt. ."] {
            assert!(overwrites_route(spelling, cwd), "{spelling}");
        }
        assert_route_unwritten(cwd);
    }

    // Regression: as above, through a symlink. Unix always makes the link.
    // Windows needs the symlink privilege (Developer Mode or an elevated
    // shell); without it the test prints a skip and checks nothing.
    #[test]
    fn report_path_naming_the_route_through_a_symlink_is_refused() {
        let dir = with_route();
        let cwd = dir.path();
        let link = cwd.join("routes/link.json");
        #[cfg(unix)]
        let linked = std::os::unix::fs::symlink(cwd.join(ROUTE), &link);
        #[cfg(windows)]
        let linked = std::os::windows::fs::symlink_file(cwd.join(ROUTE), &link);
        if let Err(err) = linked {
            eprintln!(
                "SKIPPED report_path_naming_the_route_through_a_symlink_is_refused: cannot make a symlink here: {err}"
            );
            return;
        }
        assert!(overwrites_route("routes/link.json", cwd));
        assert_route_unwritten(cwd);
    }

    // Regression: a level typed as the flag's value (`resolve_map_path`
    // steps over it) passed the launch check, and the run's one write
    // replaced the level.
    #[test]
    fn an_existing_output_is_refused_unless_it_is_an_older_file_of_its_kind() {
        let dir = tempfile::tempdir().unwrap();
        let cwd = dir.path();
        std::fs::create_dir(cwd.join("maps")).unwrap();
        let route = Path::new("cp.txt");
        let level = Path::new("maps/test.prl");
        let level_bytes = b"PRL\x00\x01not a route or a record";
        std::fs::write(cwd.join(level), level_bytes).unwrap();
        let empty = Path::new("empty.txt");
        std::fs::write(cwd.join(empty), "").unwrap();

        for path in [level, empty] {
            let error = check_record_path(path, cwd).unwrap_err();
            assert!(
                error.contains(RECORD_CAMERA_ROUTE_FLAG) && error.contains("pose file"),
                "{}: {error}",
                path.display()
            );
            let error = check_report_path(path, route, cwd).unwrap_err();
            assert!(
                error.contains(CAMERA_ROUTE_REPORT_FLAG) && error.contains("playback record"),
                "{}: {error}",
                path.display()
            );
        }

        // An older recording may be recorded over, known by its magic line
        // alone at any version, even where the current parser refuses it.
        // It is no record.
        let pose_text = postretro_camera_route::format_pose_file(
            &crate::playback::route::tests::pose_file(100.0, 3),
        );
        for (name, text) in [
            ("old.txt", pose_text.clone()),
            ("magic-only.txt", POSE_FILE_MAGIC.to_string()),
            (
                "older-version.txt",
                "postretro-camera-route 0\n".to_string(),
            ),
            ("hand-edited.txt", pose_text.replacen('\n', "\n\n", 2)),
            ("crlf.txt", pose_text.replace('\n', "\r\n")),
        ] {
            if name == "hand-edited.txt" {
                assert!(
                    postretro_camera_route::parse_pose_file(&text).is_err(),
                    "the current parser refuses a hand edit"
                );
            }
            std::fs::write(cwd.join(name), &text).unwrap();
            assert_eq!(check_record_path(Path::new(name), cwd), Ok(()), "{name}");
            assert!(
                check_report_path(Path::new(name), route, cwd).is_err(),
                "{name}: a pose file is no record"
            );
        }
        // Text that only resembles the magic line is not one.
        for (name, text) in [
            ("unversioned.txt", "postretro-camera-route\n"),
            ("word-version.txt", "postretro-camera-route one\n"),
            ("indented.txt", " postretro-camera-route 1\n"),
            ("magic-later.prl", "PRL\0\npostretro-camera-route 1\n"),
        ] {
            std::fs::write(cwd.join(name), text).unwrap();
            assert!(check_record_path(Path::new(name), cwd).is_err(), "{name}");
        }

        // An older record may be replaced at any version of its schema;
        // another schema family or other JSON may not.
        let record = |schema: &str| {
            format!(
                "{{\n  \"header\": {{\n    \"schema\": \"{schema}\",\n    \"revision\": null\n  }},\n  \"rows\": []\n}}"
            )
        };
        let family = record_schema_family();
        assert!(
            family.ends_with('.') && SCHEMA.starts_with(family) && family != SCHEMA,
            "{family}"
        );
        for (name, text) in [
            ("old.playback.json", record(SCHEMA)),
            ("v0.json", record(&format!("{family}v0"))),
        ] {
            std::fs::write(cwd.join(name), text).unwrap();
            assert_eq!(
                check_report_path(Path::new(name), route, cwd),
                Ok(()),
                "{name}"
            );
            assert!(check_record_path(Path::new(name), cwd).is_err(), "{name}");
        }
        for (name, text) in [
            ("other-family.json", record("postretro.residency_walk.v1")),
            ("no-version.json", record(family.trim_end_matches('.'))),
            ("other.json", "{\"rows\": []}".to_string()),
        ] {
            std::fs::write(cwd.join(name), text).unwrap();
            assert!(
                check_report_path(Path::new(name), route, cwd).is_err(),
                "{name}"
            );
        }

        assert_eq!(
            std::fs::read(cwd.join(level)).unwrap(),
            level_bytes,
            "the checks never write"
        );
    }

    // Regression: a byte-order mark hid the opening bytes, so an older
    // recording resaved "UTF-8 with BOM", which still plays, could not be
    // recorded over.
    #[test]
    fn an_existing_output_saved_with_a_byte_order_mark_is_still_its_kind() {
        let dir = tempfile::tempdir().unwrap();
        let cwd = dir.path();
        let pose_text = postretro_camera_route::format_pose_file(
            &crate::playback::route::tests::pose_file(100.0, 3),
        );
        let recording = format!("\u{feff}{pose_text}");
        assert!(
            postretro_camera_route::parse_pose_file(&recording).is_ok(),
            "it still plays"
        );
        std::fs::write(cwd.join("bom.txt"), recording).unwrap();
        assert_eq!(check_record_path(Path::new("bom.txt"), cwd), Ok(()));

        let record = format!("\u{feff}{{\"header\": {{\"schema\": \"{SCHEMA}\"}}, \"rows\": []}}");
        std::fs::write(cwd.join("bom.playback.json"), record).unwrap();
        assert_eq!(
            check_report_path(Path::new("bom.playback.json"), Path::new("cp.txt"), cwd),
            Ok(())
        );
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
    fn record_args_read_the_path_and_the_stall_timeout() {
        assert_eq!(record_args(&args(&["map.prl"])), Ok(None));
        let launch = args(&[
            "--record-camera-route",
            "out/rec.txt",
            "--camera-route-stall-timeout=30",
            "map.prl",
        ]);
        assert_eq!(
            playback_args(&launch),
            Ok(None),
            "a recording plays nothing"
        );
        assert_eq!(
            record_args(&launch),
            Ok(Some(RecordArgs {
                path: PathBuf::from("out/rec.txt"),
                stall_timeout: Duration::from_secs(30),
            }))
        );
        let defaulted = record_args(&args(&["--record-camera-route=rec.txt"]))
            .unwrap()
            .unwrap();
        assert_eq!(defaulted.stall_timeout, DEFAULT_STALL_TIMEOUT);
        for list in [
            &["--record-camera-route"][..],
            &["--record-camera-route", "--windowed"][..],
            &["--record-camera-route=", "map.prl"][..],
        ] {
            let error = record_args(&args(list)).unwrap_err();
            assert!(
                error.contains(RECORD_CAMERA_ROUTE_FLAG),
                "{list:?}: {error}"
            );
        }
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

    /// `--help` states these defaults and limits in prose.
    #[test]
    fn registry_help_states_playback_defaults_and_limits() {
        use crate::startup::cli_flags::lookup;
        let hold = lookup(CAMERA_ROUTE_HOLD_FLAG).unwrap().help;
        assert!(hold.contains(&DEFAULT_HOLD_FRAMES.to_string()), "{hold}");
        assert!(hold.contains(&MAX_HOLD_FRAMES.to_string()), "{hold}");
        let stall = lookup(CAMERA_ROUTE_STALL_TIMEOUT_FLAG).unwrap().help;
        assert!(
            stall.contains(&DEFAULT_STALL_TIMEOUT.as_secs().to_string()),
            "{stall}"
        );
        assert!(
            stall.contains(&MAX_STALL_TIMEOUT.as_secs().to_string()),
            "{stall}"
        );
    }
}
