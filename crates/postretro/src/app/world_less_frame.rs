// The world-less frame: no level geometry, one published UI snapshot, a clear
// color. Frontend, first-launch-hold, and Loading frames all present through it.
// See: context/lib/boot_sequence.md §1, §4 · context/lib/ui.md §5

use std::time::Instant;

use winit::event_loop::ActiveEventLoop;

use crate::{App, render, render_preparation};

impl App {
    /// Render and present one world-less frame showing `ui_snapshot` over
    /// `clear_color`. Returns false without drawing when the session or a
    /// full-ready renderer is missing, so a caller can fall back. A render or
    /// SH-streaming error stores the exit result and stops the event loop.
    ///
    /// `export_focus_rects` publishes the frame's focus and hit-test rects for
    /// the next frame's input. Display-only frames (Loading) pass false, so
    /// their trees never become reachable.
    pub(crate) fn present_world_less_frame(
        &mut self,
        event_loop: &ActiveEventLoop,
        frame_start: Instant,
        ui_snapshot: postretro_ui::UiReadSnapshot,
        clear_color: render::ClearColor,
        export_focus_rects: bool,
    ) -> bool {
        let (Some(session), Some(renderer)) = (self.session.as_mut(), self.renderer.as_mut())
        else {
            return false;
        };
        // The world-less frame renders through the full UI/scene path.
        if !renderer.is_full_ready() {
            return false;
        }

        #[cfg(feature = "dev-tools")]
        renderer.clear_debug_lines();

        renderer.set_ui_snapshot(ui_snapshot);
        let limiter_frame = Self::next_limiter_frame(&mut self.last_resolve_at, frame_start);
        renderer.set_limiter_frame(limiter_frame);
        let recycled_inputs = renderer.set_presentation_draw_inputs(Vec::new());
        session
            .presentation_pool
            .recycle_draw_inputs(recycled_inputs);
        let visible_render = render_preparation::VisibleRenderPreparation::empty_world();
        session.clear_level_streaming();
        let sh_frame_result = match renderer.render_frame_indirect(
            &mut session.font_system,
            visible_render.camera_cull(),
            &visible_render.light_reachable_cell_mask,
            &visible_render.reachable_cell_aabbs,
            &visible_render.fog_reachable,
            render::ShSampleRegionSets {
                visible_cells: &visible_render.visible_cells,
                fog_cells: &visible_render.fog_reachable,
                movers: &[],
            },
            None,
            glam::Mat4::IDENTITY,
            &[],
            self.script_time,
            clear_color,
            false,
            postretro_level_loader::ShDrainBatch::default(),
        ) {
            Ok(result) => result,
            Err(err) => {
                self.exit_result = Err(err.into());
                event_loop.exit();
                return true;
            }
        };
        let compose_submitted = sh_frame_result.compose_submitted;
        if let Err(err) = session.apply_sh_streaming_outcome(sh_frame_result.outcome, renderer) {
            self.exit_result = Err(err);
            event_loop.exit();
            return true;
        }
        let present_handle = match sh_frame_result.frame {
            Ok(present_handle) => present_handle,
            Err(err) => {
                self.exit_result = Err(err);
                event_loop.exit();
                return true;
            }
        };
        session.mark_sh_streaming_compose_submitted(compose_submitted);
        if export_focus_rects {
            session.ui_focus_rects = Some(renderer.export_ui_focus_rects());
        }
        if let Some(present_handle) = present_handle {
            renderer.present(present_handle);
        }

        let frame_cpu = Instant::now().duration_since(frame_start);
        self.frame_rate_meter.record(frame_cpu);
        true
    }
}
