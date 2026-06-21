// Dev-tools debug-UI bootstrap and the texture-dimension limit accessor.
// See: context/lib/ui.md

use super::*;

impl Renderer {
    /// The adapter's `max_texture_dimension_2d` limit. Exposed for callers that
    /// need it to construct CPU-side helpers (e.g. egui-winit's `State::new`
    /// caps emitted texture sizes against this). Keeps wgpu types from leaking
    /// across the renderer boundary; only the scalar limit escapes.
    #[cfg(feature = "dev-tools")]
    pub fn max_texture_dimension_2d(&self) -> u32 {
        self.device.limits().max_texture_dimension_2d
    }

    /// Lazily constructs the egui-wgpu renderer on first panel open. Idempotent:
    /// subsequent calls are no-ops. The init log fires exactly once per
    /// session, used by the acceptance criteria to verify lazy init.
    #[cfg(feature = "dev-tools")]
    pub fn ensure_debug_ui_gpu(&mut self) {
        if self.debug_ui_gpu.is_none() {
            self.debug_ui_gpu = Some(debug_ui::DebugUiGpu::new(
                &self.device,
                self.surface_config.format,
            ));
            log::info!("[DebugUi] GPU renderer initialized");
        }
    }

    /// Records the egui overlay pass against the surface texture. Caller
    /// (`App`) has already tessellated the frame's shapes into `paint_jobs`;
    /// the view + screen descriptor are built here so the wgpu boundary stays
    /// inside the renderer module. Loads the existing swapchain color and
    /// stores it back — no depth attachment.
    ///
    /// Egui overlay runs in a separate command encoder submission after the
    /// world draw, using LoadOp::Load to composite on top. This deviates from
    /// the spec's "before frame_timing.encode_resolve" placement — threading a
    /// shared encoder across the renderer/App boundary was more complex than
    /// the benefit justified.
    #[cfg(feature = "dev-tools")]
    pub fn render_debug_ui(
        &mut self,
        surface_texture: &wgpu::SurfaceTexture,
        textures_delta: egui::TexturesDelta,
        paint_jobs: Vec<egui::ClippedPrimitive>,
        pixels_per_point: f32,
    ) -> Result<()> {
        let surface_view = surface_texture
            .texture
            .create_view(&wgpu::TextureViewDescriptor::default());
        let screen_desc = egui_wgpu::ScreenDescriptor {
            size_in_pixels: [self.surface_config.width, self.surface_config.height],
            pixels_per_point,
        };

        self.ensure_debug_ui_gpu();
        let gpu = self
            .debug_ui_gpu
            .as_mut()
            .expect("ensure_debug_ui_gpu populated debug_ui_gpu");

        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("egui Encoder"),
            });

        for (id, image_delta) in &textures_delta.set {
            gpu.renderer
                .update_texture(&self.device, &self.queue, *id, image_delta);
        }
        let user_cmd_bufs = gpu.renderer.update_buffers(
            &self.device,
            &self.queue,
            &mut encoder,
            &paint_jobs,
            &screen_desc,
        );

        {
            let pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("egui Overlay Pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &surface_view,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Load,
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: None,
                ..Default::default()
            });
            gpu.renderer
                .render(&mut pass.forget_lifetime(), &paint_jobs, &screen_desc);
        }

        for id in &textures_delta.free {
            gpu.renderer.free_texture(id);
        }

        self.queue.submit(
            user_cmd_bufs
                .into_iter()
                .chain(std::iter::once(encoder.finish())),
        );
        Ok(())
    }
}
