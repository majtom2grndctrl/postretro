// Renderer-owned offscreen capture recording and RGBA8 readback.
// See: context/lib/rendering_pipeline.md §7.8

use super::*;

impl Renderer {
    /// Plain adapter identity retained for a capture measurement report.
    pub fn capture_measurement_adapter_identity(&self) -> &CaptureAdapterIdentity {
        &self.capture_adapter_identity
    }

    /// Timestamp-query setup state for a capture measurement report.
    pub fn capture_measurement_timing_state(&self) -> CaptureGpuTimingState {
        self.capture_gpu_timing_state
    }

    /// Drop timestamp-query accumulation at the warmup/sample boundary.
    ///
    /// Prepared capture calls this only after each warmup submission has
    /// completed, so `FrameTiming` has no pending map to preserve.
    pub fn reset_capture_measurement_timing(&mut self) {
        if let Some(timing) = self.full_mut().frame_timing.as_mut() {
            timing.reset_window_state();
        }
    }

    /// Submit one prepared static capture scene and wait until the GPU has
    /// completed it. This deliberately records no PNG/readback copy; callers
    /// use it for warmup and sample work, then use `capture_frame_indirect`
    /// once to produce the inspectable PNG.
    #[allow(clippy::too_many_arguments)]
    pub fn capture_measurement_frame_indirect(
        &mut self,
        cam_vis: CameraCullVisibility<'_>,
        light_reachable_cell_mask: &[bool],
        reachable_cell_aabbs: &[(Vec3, Vec3)],
        fog_reachable: &[u32],
        camera_cell: Option<u32>,
        view_proj: Mat4,
        camera_position: Vec3,
        particle_collections: &[(&str, &[u8])],
        capture_animated_promotion_weights: &[(usize, f32)],
        clear_color: ClearColor,
        render_world: bool,
    ) -> Result<Option<CaptureGpuTimingWindow>> {
        self.update_per_frame_uniforms(view_proj, camera_position, 0.0);

        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("Capture Measurement Encoder"),
            });
        self.record_scene_passes(
            &mut encoder,
            None,
            None,
            cam_vis,
            light_reachable_cell_mask,
            reachable_cell_aabbs,
            fog_reachable,
            camera_cell,
            view_proj,
            particle_collections,
            capture_animated_promotion_weights,
            0.0,
            clear_color,
            render_world,
        )?;

        if self.capture_gpu_timing_state == CaptureGpuTimingState::Active {
            if let Some(timing) = self.full_mut().frame_timing.as_mut() {
                timing.encode_resolve(&mut encoder);
            }
        }
        self.queue.submit(std::iter::once(encoder.finish()));
        self.complete_capture_measurement_submission()
    }

    fn complete_capture_measurement_submission(
        &mut self,
    ) -> Result<Option<CaptureGpuTimingWindow>> {
        self.device
            .poll(wgpu::PollType::wait_indefinitely())
            .context("waiting for capture measurement submission failed")?;

        if self.capture_gpu_timing_state != CaptureGpuTimingState::Active {
            return Ok(None);
        }

        // `FrameTiming::post_submit` starts its async map after a submitted
        // resolve. A second renderer-owned wait then makes that map complete
        // before this method returns, keeping CPU samples completion-based.
        {
            let Self { device, full, .. } = self;
            let full = full
                .as_mut()
                .expect("offscreen capture always has a full renderer");
            if let Some(timing) = full.frame_timing.as_mut() {
                timing.post_submit(device);
            }
        }
        self.device
            .poll(wgpu::PollType::wait_indefinitely())
            .context("waiting for capture measurement timing readback failed")?;

        let snapshot = {
            let Self { device, full, .. } = self;
            let full = full
                .as_mut()
                .expect("offscreen capture always has a full renderer");
            full.frame_timing.as_mut().and_then(|timing| {
                timing.post_submit(device);
                timing.take_completed_window()
            })
        };
        Ok(snapshot.map(capture_timing_window))
    }

    /// Render the world scene into the renderer-owned pre-resolve target and
    /// return tight RGBA8 pixels. The supplied camera updates both culling and
    /// forward-pass uniforms at the fixed capture time. This path has no UI,
    /// debug overlay, resolve, swapchain acquisition, or present step.
    #[allow(clippy::too_many_arguments)]
    pub fn capture_frame_indirect(
        &mut self,
        cam_vis: CameraCullVisibility<'_>,
        light_reachable_cell_mask: &[bool],
        reachable_cell_aabbs: &[(Vec3, Vec3)],
        fog_reachable: &[u32],
        camera_cell: Option<u32>,
        view_proj: Mat4,
        camera_position: Vec3,
        particle_collections: &[(&str, &[u8])],
        capture_animated_promotion_weights: &[(usize, f32)],
        clear_color: ClearColor,
        render_world: bool,
    ) -> Result<Vec<u8>> {
        self.update_per_frame_uniforms(view_proj, camera_position, 0.0);

        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("Frame Capture Encoder"),
            });
        self.record_scene_passes(
            &mut encoder,
            None,
            None,
            cam_vis,
            light_reachable_cell_mask,
            reachable_cell_aabbs,
            fog_reachable,
            camera_cell,
            view_proj,
            particle_collections,
            capture_animated_promotion_weights,
            0.0,
            clear_color,
            render_world,
        )?;

        let width = self.surface_config.width;
        let height = self.surface_config.height;
        // The PNG path reads tightly packed RGBA8, so resolve HDR scene color to
        // a capture-only LDR target first. Capture shares the window tonemap but
        // uses an at-rest effect uniform instead of transient screen effects.
        let capture_color = self.full().screen_effects.encode_capture_tonemap(
            &self.device,
            &self.queue,
            &mut encoder,
            width,
            height,
        );
        self.read_texture_rgba8(&capture_color, width, height, encoder)
    }
}

fn capture_timing_window(snapshot: frame_timing::FrameTimingSnapshot) -> CaptureGpuTimingWindow {
    CaptureGpuTimingWindow {
        passes: snapshot
            .passes
            .into_iter()
            .map(|(label, average_ms, skipped_frames)| CaptureGpuTimingPass {
                label,
                average_ms,
                skipped_frames,
            })
            .collect(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    #[ignore = "on-demand GPU coverage"]
    fn offscreen_measurement_exposes_plain_adapter_identity_without_leaking_wgpu() {
        let renderer = match Renderer::new_offscreen(1, 1) {
            Ok(renderer) => renderer,
            Err(error) if error.to_string().contains("requires a GPU adapter") => return,
            Err(error) => panic!("offscreen measurement renderer must initialize: {error:#}"),
        };

        let adapter = renderer.capture_measurement_adapter_identity();
        assert!(!adapter.name.trim().is_empty());
        assert!(!adapter.backend.trim().is_empty());
        assert!(!adapter.device_type.trim().is_empty());
    }
}
