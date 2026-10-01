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

    fn position(haystack: &str, needle: &str, from: usize) -> usize {
        from + haystack[from..]
            .find(needle)
            .unwrap_or_else(|| panic!("`{needle}` not found after offset {from}"))
    }

    // P6, P17: an option write in this frame's logic lands in the same commit
    // as a resize, and the camera is built from the committed extent.
    #[test]
    fn gameplay_frame_commits_extents_after_option_writes_and_before_the_camera() {
        let options = position(
            MAIN,
            "self.update_player_options(frame_dt, options_menu_was_open);",
            0,
        );
        let commit = position(MAIN, "self.commit_render_extents();", options);
        let eye = position(MAIN, "frame_eye::assemble_frame_eye(", commit);
        let viewmodel = position(MAIN, "renderer.update_viewmodel_view_projection(", eye);
        assert!(
            MAIN[viewmodel..]
                .trim_start_matches("renderer.update_viewmodel_view_projection(")
                .trim_start()
                .starts_with("self.camera.aspect()"),
            "the viewmodel projects at the camera's scene aspect"
        );
    }

    #[test]
    fn frontend_frame_commits_extents_after_option_writes() {
        let logic = position(MAIN, "fn run_frontend_ui_logic(", 0);
        let options = position(
            MAIN,
            "self.update_player_options(frame_dt, options_menu_was_open);",
            logic,
        );
        let commit = position(MAIN, "self.commit_render_extents();", options);
        let end = position(MAIN, "\n    }\n", options);
        assert!(
            commit < end,
            "the frontend commit belongs to the same frame logic"
        );
    }

    // P1, P3–P5: window events only record; nothing rebuilds inside a handler.
    #[test]
    fn window_events_record_size_and_scale_without_rebuilding() {
        let resized = position(MAIN, "WindowEvent::Resized(size) =>", 0);
        let arm_end = position(MAIN, "WindowEvent::CloseRequested", resized);
        let arms = &MAIN[resized..arm_end];
        assert!(arms.contains("renderer.record_surface_size(size.width, size.height)"));
        assert!(arms.contains("WindowEvent::ScaleFactorChanged"));
        assert!(arms.contains("renderer.record_scale_factor(scale_factor)"));
        assert!(!arms.contains("commit_extents"));
        assert!(!arms.contains("update_aspect"));
        assert!(!MAIN.contains("renderer.resize("));
    }
}
