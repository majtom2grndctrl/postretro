// The world-less frame: no level geometry, one published UI snapshot, a clear
// color. Frontend, first-launch-hold, and Loading frames all present through it,
// and so do Settling frames, which hold an installed level behind it.
// See: context/lib/boot_sequence.md §1, §4 · context/lib/ui.md §5

use std::time::Instant;

use glam::Mat4;
use postretro_level_loader::ShDrainBatch;
use postretro_render_cpu::planar_reflection::PlanarPreset;
use postretro_visibility::VisibleCells;
use winit::event_loop::ActiveEventLoop;

use crate::camera::RenderCamera;
use crate::render_preparation::VisibleRenderPreparation;
use crate::{App, cpu_timing, render};

/// The view a held level streams toward while a world-less frame covers it:
/// the reveal frame's camera, so the planar mirror builds from it too.
#[derive(Debug, Clone, Copy)]
pub(crate) struct HeldLevelView {
    pub(crate) camera: RenderCamera,
}

/// A held level's prepared frame: its visibility from the held view and the
/// SH batch its streaming step produced, waiting to be presented.
pub(crate) struct HeldLevelFrame {
    view: HeldLevelView,
    visible_render: VisibleRenderPreparation,
    sh_drain_batch: ShDrainBatch,
}

impl HeldLevelFrame {
    /// The visibility path the held view's walk took.
    pub(crate) fn path(&self) -> postretro_visibility::VisibilityPath {
        self.visible_render.stats.path
    }

    /// The main view's visible cells from the held pose, without the mirror's.
    pub(crate) fn visible_cells(&self) -> &VisibleCells {
        &self.visible_render.visible_cells
    }
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

    /// The held level's half of a Settling frame, before its paint: the
    /// level's visibility and streaming step from `view`. Returns `None`,
    /// having done nothing, without a level, session or full-ready renderer.
    /// The level's streaming sessions survive; the frame that presents the
    /// result composes the SH it made resident.
    ///
    /// Visibility runs the play frame's order from the held pose at the tier
    /// the renderer draws with, the planar mirror included, and its drawn
    /// cells (main and mirror) are what Settling demands on any path, a main
    /// fallback included: the lightmap settles every block the pose draws.
    /// SH visible targeting takes the play frame's gate, and SH settles its
    /// mandatory tier only. From reveal the play frame's path rules apply.
    pub(crate) fn prepare_held_level_frame(
        &mut self,
        view: HeldLevelView,
    ) -> anyhow::Result<Option<HeldLevelFrame>> {
        let Some(renderer) = self.renderer.as_ref() else {
            return Ok(None);
        };
        if !renderer.is_full_ready() {
            return Ok(None);
        }
        let preset = renderer.planar_reflection_preset();
        let Some(visible_render) = self.held_level_visibility(&view, preset) else {
            return Ok(None);
        };
        let (Some(world), Some(session), Some(renderer)) = (
            self.level.as_ref(),
            self.session.as_mut(),
            self.renderer.as_mut(),
        ) else {
            return Ok(None);
        };
        let streaming_cpu = postretro_stage_timing::StageFrame::<cpu_timing::StreamingStage>::new(
            self.cpu_timer.gate(),
        );
        let sh_drain_batch = session.run_level_streaming_step(
            world.sh_stream_manifest(),
            Some(world),
            renderer,
            crate::session::level_streaming::StreamingFrame {
                visible_cells: self
                    .planar_render
                    .drawn_cells(&visible_render.visible_cells),
                sh_visible_cells: self
                    .planar_render
                    .sh_visible_cells(&visible_render.visible_cells),
                camera_cell: Some(visible_render.stats.camera_cell as usize),
                path: visible_render.stats.path,
                monotonic_seconds: self.script_time,
                settling: true,
                cpu: &streaming_cpu,
            },
        )?;
        Ok(Some(HeldLevelFrame {
            view,
            visible_render,
            sh_drain_batch,
        }))
    }

    /// The held pose's visibility in the play frame's order: the camera
    /// walk, then the planar mirror at `preset` (`None` below very high),
    /// which selects, crops and walks from the held pose and tags its
    /// mirror-only light reach. The mirror's state stays in `planar_render`,
    /// whose selection history carries into the reveal frame, so at the same
    /// pose the reveal selects the plane the held frames did. `None` without
    /// a level or session.
    pub(crate) fn held_level_visibility(
        &mut self,
        view: &HeldLevelView,
        preset: Option<PlanarPreset>,
    ) -> Option<VisibleRenderPreparation> {
        let (Some(world), Some(session)) = (self.level.as_ref(), self.session.as_ref()) else {
            return None;
        };
        {
            let registry = session.scripting.script_ctx.registry.borrow();
            crate::rebuild_blocked_portals(&mut self.blocked_portals, Some(world), &registry);
        }
        let visible_render = VisibleRenderPreparation::for_level(
            world,
            view.camera.eye_position,
            view.camera.view_projection,
            &self.blocked_portals,
            false,
            &mut self.portal_walk_scratch,
            self.cpu_timer.gate(),
        );
        self.prepare_planar_mirror_at(&view.camera, &visible_render, preset);
        Some(visible_render)
    }

    /// Present a world-less frame over the held level `held` prepared:
    /// composes the SH its drain made resident and draws none of the world.
    /// Same return and error contract as [`Self::present_world_less_frame`].
    pub(crate) fn present_held_level_frame(
        &mut self,
        event_loop: &ActiveEventLoop,
        frame_start: Instant,
        ui_snapshot: postretro_ui::UiReadSnapshot,
        clear_color: render::ClearColor,
        held: HeldLevelFrame,
    ) -> bool {
        self.present_world_less_scene(
            event_loop,
            frame_start,
            ui_snapshot,
            clear_color,
            false,
            Some(held),
        )
    }

    fn present_world_less_scene(
        &mut self,
        event_loop: &ActiveEventLoop,
        frame_start: Instant,
        ui_snapshot: postretro_ui::UiReadSnapshot,
        clear_color: render::ClearColor,
        export_focus_rects: bool,
        held: Option<HeldLevelFrame>,
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

        let (visible_render, camera_cell, view_proj, sh_drain_batch, scene) = match held {
            Some(held) => {
                // The mirror the held pose walked: compose covers its cells,
                // as the reveal frame's will. A compose-only frame records no
                // planar pass.
                renderer.set_planar_reflection_view(
                    self.planar_render.camera(),
                    self.planar_render.visible(),
                );
                // Set for symmetry with the play frame, and harmless: a
                // compose-only frame ranks no light, and the next world frame
                // sets its own reach before it ranks.
                renderer
                    .set_planar_mirror_light_reach(self.planar_render.mirror_light_cell_aabbs());
                renderer.update_per_frame_uniforms(
                    held.view.camera.view_projection,
                    held.view.camera.eye_position,
                    self.script_time as f32,
                );
                let camera_cell = Some(held.visible_render.stats.camera_cell);
                (
                    held.visible_render,
                    camera_cell,
                    held.view.camera.view_projection,
                    held.sh_drain_batch,
                    render::FrameScene::ComposeOnly,
                )
            }
            None => {
                session.clear_level_streaming();
                renderer.set_planar_mirror_light_reach(&[]);
                (
                    VisibleRenderPreparation::empty_world(),
                    None,
                    Mat4::IDENTITY,
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
            // A held level samples main and mirror, as the play frame does.
            if scene == render::FrameScene::ComposeOnly {
                render::ShSampleRegionSets {
                    visible_cells: self
                        .planar_render
                        .drawn_cells(&visible_render.visible_cells),
                    fog_cells: self
                        .planar_render
                        .sample_fog_cells(&visible_render.fog_reachable),
                    movers: &[],
                }
            } else {
                render::ShSampleRegionSets {
                    visible_cells: &visible_render.visible_cells,
                    fog_cells: &visible_render.fog_reachable,
                    movers: &[],
                }
            },
            camera_cell,
            view_proj,
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
        if let VisibleCells::Culled(cells) = visible_render.visible_cells {
            self.portal_walk_scratch.recycle_cells(cells);
        }

        let frame_cpu = Instant::now().duration_since(frame_start);
        self.frame_rate_meter.record(frame_cpu);
        true
    }
}
