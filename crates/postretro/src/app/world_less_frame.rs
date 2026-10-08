// The world-less frame: no level geometry, one published UI snapshot, a clear
// color. Frontend, first-launch-hold, and Loading frames all present through it,
// and so do Settling frames, which hold an installed level behind it.
// See: context/lib/boot_sequence.md §1, §4 · context/lib/ui.md §5

use std::time::Instant;

use glam::{Mat4, Vec3};
use postretro_level_loader::ShDrainBatch;
use postretro_visibility::VisibleCells;
use winit::event_loop::ActiveEventLoop;

use crate::render_preparation::VisibleRenderPreparation;
use crate::{App, cpu_timing, render};

/// The view a held level streams toward while a world-less frame covers it.
#[derive(Debug, Clone, Copy)]
pub(crate) struct HeldLevelView {
    pub(crate) eye: Vec3,
    pub(crate) view_proj: Mat4,
}

impl App {
    /// Render and present one world-less frame showing `ui_snapshot` over
    /// `clear_color`. Releases any level streaming: nothing is installed to
    /// stream for. Returns false without drawing when the session or a
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
        self.present_world_less_scene(
            event_loop,
            frame_start,
            ui_snapshot,
            clear_color,
            export_focus_rects,
            None,
        )
    }

    /// A world-less frame over the installed level: runs the level's
    /// visibility and streaming step from `view`, composes the SH it made
    /// resident, and draws none of the world. The level's streaming sessions
    /// survive the frame. Same return and error contract as
    /// [`Self::present_world_less_frame`]; also false without a level.
    pub(crate) fn present_held_level_frame(
        &mut self,
        event_loop: &ActiveEventLoop,
        frame_start: Instant,
        ui_snapshot: postretro_ui::UiReadSnapshot,
        clear_color: render::ClearColor,
        view: HeldLevelView,
    ) -> bool {
        if self.level.is_none() {
            return false;
        }
        self.present_world_less_scene(
            event_loop,
            frame_start,
            ui_snapshot,
            clear_color,
            false,
            Some(view),
        )
    }

    fn present_world_less_scene(
        &mut self,
        event_loop: &ActiveEventLoop,
        frame_start: Instant,
        ui_snapshot: postretro_ui::UiReadSnapshot,
        clear_color: render::ClearColor,
        export_focus_rects: bool,
        held: Option<HeldLevelView>,
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

        let (visible_render, camera_cell, sh_drain_batch, scene) =
            match (held, self.level.as_ref()) {
                (Some(view), Some(world)) => {
                    {
                        let registry = session.scripting.script_ctx.registry.borrow();
                        crate::rebuild_blocked_portals(
                            &mut self.blocked_portals,
                            Some(world),
                            &registry,
                        );
                    }
                    let visible_render = VisibleRenderPreparation::for_level(
                        world,
                        view.eye,
                        view.view_proj,
                        &self.blocked_portals,
                        false,
                        &mut self.scratch_cells,
                        self.cpu_timer.gate(),
                    );
                    renderer.update_per_frame_uniforms(
                        view.view_proj,
                        view.eye,
                        self.script_time as f32,
                    );
                    let streaming_cpu = postretro_stage_timing::StageFrame::<
                        cpu_timing::StreamingStage,
                    >::new(self.cpu_timer.gate());
                    let batch = match session.run_level_streaming_step(
                        world.sh_stream_manifest(),
                        Some(world),
                        renderer,
                        crate::session::level_streaming::StreamingFrame {
                            visible_cells: &visible_render.visible_cells,
                            camera_cell: Some(visible_render.stats.camera_cell as usize),
                            path: visible_render.stats.path,
                            monotonic_seconds: self.script_time,
                            cpu: &streaming_cpu,
                        },
                    ) {
                        Ok(batch) => batch,
                        Err(err) => {
                            self.exit_result = Err(err);
                            event_loop.exit();
                            return true;
                        }
                    };
                    let camera_cell = Some(visible_render.stats.camera_cell);
                    (
                        visible_render,
                        camera_cell,
                        batch,
                        render::FrameScene::ComposeOnly,
                    )
                }
                _ => {
                    session.clear_level_streaming();
                    (
                        VisibleRenderPreparation::empty_world(),
                        None,
                        ShDrainBatch::default(),
                        render::FrameScene::Empty,
                    )
                }
            };
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
            camera_cell,
            held.map_or(Mat4::IDENTITY, |view| view.view_proj),
            &[],
            self.script_time,
            clear_color,
            scene,
            sh_drain_batch,
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
        if let VisibleCells::Culled(mut cells) = visible_render.visible_cells {
            cells.clear();
            self.scratch_cells = cells;
        }

        let frame_cpu = Instant::now().duration_since(frame_start);
        self.frame_rate_meter.record(frame_cpu);
        true
    }
}
