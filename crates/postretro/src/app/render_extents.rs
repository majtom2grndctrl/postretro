// The once-per-frame render-extent commit and the camera aspect it sets.
// See: context/lib/rendering_pipeline.md §7.8, §11

use crate::App;

impl App {
    /// Commit the frame's recorded resize, scale-factor and render-resolution
    /// changes, rebuilding at most once, then take the camera aspect from the
    /// committed scene extent. Runs after the frame's option writes and before
    /// the camera is built, so the first frame at a new extent already projects
    /// at its aspect, and the viewmodel follows through the camera.
    pub(crate) fn commit_render_extents(&mut self) {
        let Some(renderer) = self.renderer.as_mut() else {
            return;
        };
        renderer.commit_extents();
        let scene = renderer.scene_extent();
        self.camera.update_aspect(scene.width, scene.height);
    }
}

#[cfg(test)]
mod tests {
    const MAIN: &str = include_str!("../main.rs");
    const OPTIONS: &str = "self.update_player_options(frame_dt, options_menu_was_open);";
    const COMMIT: &str = "self.commit_render_extents();";
    const FRONTEND_FN: &str = "fn run_frontend_ui_logic(";

    fn position(haystack: &str, needle: &str, from: usize) -> usize {
        from + haystack[from..]
            .find(needle)
            .unwrap_or_else(|| panic!("`{needle}` not found after offset {from}"))
    }

    /// Byte range of `run_frontend_ui_logic`'s body: from its signature to the
    /// next method-level `fn` at four-space indent (any visibility), or EOF.
    fn frontend_logic_range() -> (usize, usize) {
        let start = position(MAIN, FRONTEND_FN, 0);
        let after = start + FRONTEND_FN.len();
        let end = ["\n    fn ", "\n    pub(crate) fn ", "\n    pub fn "]
            .iter()
            .filter_map(|marker| MAIN[after..].find(marker).map(|at| after + at))
            .min()
            .unwrap_or(MAIN.len());
        (start, end)
    }

    // P6, P17: an option write in this frame's logic lands in the same commit
    // as a resize, and the camera is built from the committed extent.
    #[test]
    fn gameplay_frame_commits_extents_after_option_writes_and_before_the_camera() {
        let (frontend_start, frontend_end) = frontend_logic_range();
        let options = MAIN
            .match_indices(OPTIONS)
            .map(|(at, _)| at)
            .find(|at| *at < frontend_start || *at >= frontend_end)
            .expect("gameplay frame calls update_player_options outside run_frontend_ui_logic");
        let commit = position(MAIN, COMMIT, options);
        let eye = position(MAIN, "frame_eye::assemble_frame_eye(", commit);
        let viewmodel = position(MAIN, "renderer.update_viewmodel_view_projection(", eye);

        assert!(
            options < commit && commit < eye && eye < viewmodel,
            "order must be options < commit < eye < viewmodel: an option write and a \
             resize in the same frame rebuild once (P6), and the camera and viewmodel \
             project at the committed scene aspect (P17)"
        );
        let commits_in_window = MAIN[options..eye].matches("commit_render_extents").count();
        assert_eq!(
            commits_in_window, 1,
            "exactly one commit_render_extents between the option writes and the eye (P6)"
        );
        assert!(
            MAIN[viewmodel..]
                .trim_start_matches("renderer.update_viewmodel_view_projection(")
                .trim_start()
                .starts_with("self.camera.aspect()"),
            "the viewmodel projects at the camera's committed scene aspect (P17)"
        );
    }

    #[test]
    fn frontend_frame_commits_extents_after_option_writes() {
        let (start, end) = frontend_logic_range();
        let body = &MAIN[start..end];
        let options = position(body, OPTIONS, 0);
        let commit = position(body, COMMIT, options);
        let pose = position(
            body,
            "self.apply_frontend_menu_camera_pose_if_present();",
            commit,
        );
        assert!(
            options < commit && commit < pose,
            "frontend frame order must be option writes < commit < menu camera pose (P6, P17)"
        );
    }

    // Regression: frontend Surface Depth writes preceded an empty-only reload commit.
    #[test]
    fn frontend_reload_commits_before_option_uploads_without_reordering_camera_setup() {
        let (start, end) = frontend_logic_range();
        let body = &MAIN[start..end];
        let commands = position(body, "self.dispatch_system_commands();", 0);
        let reload = position(body, "self.poll_staged_manifest_results();", 0);
        let options = position(body, OPTIONS, 0);
        let commit = position(body, COMMIT, 0);
        let focus = position(body, "self.reconcile_ui_focus();", commit);
        let pose = position(
            body,
            "self.apply_frontend_menu_camera_pose_if_present();",
            commit,
        );
        assert!(
            commands < reload
                && reload < options
                && options < commit
                && commit < focus
                && focus < pose,
            "frontend order must be commands < reload < options < extents < focus < camera: \
             reload requires the completed frame's empty upload boundary"
        );
        assert_eq!(
            body.matches("self.poll_staged_manifest_results();").count(),
            1
        );
    }

    // P1, P3–P5: window events only record; nothing rebuilds inside a handler.
    #[test]
    fn window_events_record_size_and_scale_without_rebuilding() {
        let resized = position(MAIN, "WindowEvent::Resized(", 0);
        let arm_end = position(MAIN, "WindowEvent::CloseRequested", resized);
        let arms = &MAIN[resized..arm_end];
        assert!(
            arms.contains("record_surface_size("),
            "Resized records size"
        );
        assert!(
            arms.contains("WindowEvent::ScaleFactorChanged"),
            "ScaleFactorChanged arm sits between Resized and CloseRequested"
        );
        assert!(
            arms.contains("record_scale_factor("),
            "ScaleFactorChanged records scale"
        );
        assert!(!arms.contains("commit_extents"), "handlers never commit");
        assert!(!arms.contains("update_aspect"), "handlers never set aspect");
        assert!(
            !MAIN.contains("renderer.resize("),
            "nothing rebuilds via renderer.resize"
        );
    }
}
