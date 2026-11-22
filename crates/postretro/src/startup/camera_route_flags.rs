// Camera-route launch flags every build knows: their names, which the flag
// registry (`cli_flags`) declares, and the warning a build without playback gives.
// See: context/lib/rendering_pipeline.md §12 · context/lib/development_guide.md §6.4

pub(crate) const CAMERA_ROUTE_FLAG: &str = "--camera-route";
pub(crate) const CAMERA_ROUTE_HOLD_FLAG: &str = "--camera-route-hold";
pub(crate) const CAMERA_ROUTE_REPORT_FLAG: &str = "--camera-route-report";
pub(crate) const CAMERA_ROUTE_PACING_FLAG: &str = "--camera-route-pacing";
pub(crate) const CAMERA_ROUTE_STALL_TIMEOUT_FLAG: &str = "--camera-route-stall-timeout";
pub(crate) const RECORD_CAMERA_ROUTE_FLAG: &str = "--record-camera-route";
/// `vsync` or `no-vsync`: the swapchain's present mode from the first frame,
/// for any session of a build with playback's feature, a route or not.
pub(crate) const PRESENT_MODE_FLAG: &str = "--present-mode";

/// The feature playback and recording build behind. It must match the
/// feature gating `mod playback;` in `main.rs` and the feature the SDK's
/// authoring engine builds with, or the warning names a feature that enables
/// nothing. Tests in this file and in xtask's `dist.rs` check both. Keep the
/// declaration on one line: xtask reads it by source text.
pub(crate) const CAMERA_ROUTE_FEATURE: &str = "dev-tools";

/// In a build without playback's feature, warns once for each of the
/// playback, recording and present-mode flags the launch names; the run then
/// behaves as if it were unset. A build with the feature warns nothing.
pub(crate) fn warn_camera_route_flags_without_feature(args: &[String], feature_enabled: bool) {
    if feature_enabled {
        return;
    }
    for flag in [
        CAMERA_ROUTE_FLAG,
        RECORD_CAMERA_ROUTE_FLAG,
        PRESENT_MODE_FLAG,
    ] {
        if names_flag(args, flag) {
            log::warn!(
                "[Playback] {flag} needs the `{CAMERA_ROUTE_FEATURE}` feature, which this build lacks; ignored"
            );
        }
    }
}

/// Whether the launch names a camera route to play, as telemetry's
/// `run_start` reports playback's producer.
pub(crate) fn names_camera_route(args: &[String]) -> bool {
    names_flag(args, CAMERA_ROUTE_FLAG)
}

impl crate::App {
    /// The present mode the renderer builds its surface with: the launch's
    /// `--present-mode` in a build with playback's feature, else vsync.
    pub(crate) fn launch_present_choice(&self) -> postretro_renderer::PresentChoice {
        #[cfg(feature = "dev-tools")]
        {
            self.playback.present_choice()
        }
        #[cfg(not(feature = "dev-tools"))]
        {
            postretro_renderer::PresentChoice::Vsync
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
    const PRESENT_WARNING: &str =
        "[Playback] --present-mode needs the `dev-tools` feature, which this build lacks; ignored";

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
        capture.assert_not_logged(log::Level::Warn, PRESENT_WARNING);
    }

    #[test]
    fn present_mode_flag_warns_once_without_the_feature_and_nothing_with_it() {
        let capture = LogCapture::start();
        let launch = args(&["--present-mode", "no-vsync", "map.prl"]);
        warn_camera_route_flags_without_feature(&launch, true);
        capture.assert_not_logged(log::Level::Warn, PRESENT_WARNING);
        warn_camera_route_flags_without_feature(&launch, false);
        capture.assert_logged_once(log::Level::Warn, PRESENT_WARNING);
        capture.assert_not_logged(log::Level::Warn, PLAY_WARNING);
    }

    #[test]
    fn camera_route_feature_names_the_gate_playback_builds_behind() {
        let main = include_str!("../main.rs");
        let gate = format!("#[cfg(feature = \"{CAMERA_ROUTE_FEATURE}\")]\nmod playback;");
        assert!(
            main.contains(&gate),
            "main.rs gates playback on {CAMERA_ROUTE_FEATURE}"
        );
    }

    #[test]
    fn names_camera_route_reads_either_form_and_not_its_options() {
        assert!(names_camera_route(&args(&["--camera-route", "cp.txt"])));
        assert!(names_camera_route(&args(&["--camera-route=cp.txt"])));
        assert!(!names_camera_route(&args(&["--camera-route-hold", "5"])));
        assert!(!names_camera_route(&args(&["map.prl"])));
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
            "--present-mode",
            "no-vsync",
            "maps/x.prl",
        ]);
        assert_eq!(
            crate::startup::session::resolve_map_path(&launch).as_deref(),
            Some("maps/x.prl")
        );
    }
}
