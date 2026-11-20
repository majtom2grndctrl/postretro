// Camera-route launch flags every build knows: their names, so the map scan
// steps over their values, and the warning a build without playback gives.
// See: context/lib/rendering_pipeline.md §12 · context/lib/development_guide.md §6.4

pub(crate) const CAMERA_ROUTE_FLAG: &str = "--camera-route";
pub(crate) const CAMERA_ROUTE_HOLD_FLAG: &str = "--camera-route-hold";
pub(crate) const CAMERA_ROUTE_REPORT_FLAG: &str = "--camera-route-report";
pub(crate) const CAMERA_ROUTE_PACING_FLAG: &str = "--camera-route-pacing";
pub(crate) const CAMERA_ROUTE_STALL_TIMEOUT_FLAG: &str = "--camera-route-stall-timeout";
pub(crate) const RECORD_CAMERA_ROUTE_FLAG: &str = "--record-camera-route";

/// Every camera-route flag that takes a value.
pub(crate) const CAMERA_ROUTE_VALUE_FLAGS: [&str; 6] = [
    CAMERA_ROUTE_FLAG,
    CAMERA_ROUTE_HOLD_FLAG,
    CAMERA_ROUTE_REPORT_FLAG,
    CAMERA_ROUTE_PACING_FLAG,
    CAMERA_ROUTE_STALL_TIMEOUT_FLAG,
    RECORD_CAMERA_ROUTE_FLAG,
];

/// The feature playback and recording build behind.
pub(crate) const CAMERA_ROUTE_FEATURE: &str = "dev-tools";

/// In a build without playback's feature, warns once for each of the
/// playback and recording flags the launch names; the run then behaves as if
/// it were unset. A build with the feature warns nothing.
pub(crate) fn warn_camera_route_flags_without_feature(args: &[String], feature_enabled: bool) {
    if feature_enabled {
        return;
    }
    for flag in [CAMERA_ROUTE_FLAG, RECORD_CAMERA_ROUTE_FLAG] {
        if names_flag(args, flag) {
            log::warn!(
                "[Playback] {flag} needs the `{CAMERA_ROUTE_FEATURE}` feature, which this build lacks; ignored"
            );
        }
    }
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

#[cfg(test)]
mod tests {
    use postretro_test_log_capture::LogCapture;

    use super::*;

    fn args(list: &[&str]) -> Vec<String> {
        std::iter::once("postretro")
            .chain(list.iter().copied())
            .map(str::to_string)
            .collect()
    }

    const PLAY_WARNING: &str =
        "[Playback] --camera-route needs the `dev-tools` feature, which this build lacks; ignored";
    const RECORD_WARNING: &str = "[Playback] --record-camera-route needs the `dev-tools` feature, which this build lacks; ignored";

    #[test]
    fn camera_route_flags_warn_once_each_without_the_feature() {
        let capture = LogCapture::start();
        warn_camera_route_flags_without_feature(
            &args(&[
                "--camera-route",
                "cp.txt",
                "--record-camera-route=rec.txt",
                "--camera-route-hold",
                "5",
                "map.prl",
            ]),
            false,
        );
        capture.assert_logged_once(log::Level::Warn, PLAY_WARNING);
        capture.assert_logged_once(log::Level::Warn, RECORD_WARNING);
    }

    #[test]
    fn camera_route_flags_warn_nothing_with_the_feature_or_without_the_flags() {
        let capture = LogCapture::start();
        warn_camera_route_flags_without_feature(&args(&["--camera-route", "cp.txt"]), true);
        warn_camera_route_flags_without_feature(
            &args(&["--camera-route-hold", "5", "--camera-routes", "map.prl"]),
            false,
        );
        capture.assert_not_logged(log::Level::Warn, "needs the `dev-tools` feature");
    }

    #[test]
    fn camera_route_flags_without_the_feature_leave_the_map_argument() {
        // The run behaves as if the flags were unset: their values are never
        // taken for the map.
        let launch = args(&[
            "--camera-route",
            "cp.txt",
            "--record-camera-route",
            "rec.txt",
            "--camera-route-stall-timeout",
            "30",
            "maps/x.prl",
        ]);
        assert_eq!(
            crate::startup::session::resolve_map_path(&launch).as_deref(),
            Some("maps/x.prl")
        );
    }
}
