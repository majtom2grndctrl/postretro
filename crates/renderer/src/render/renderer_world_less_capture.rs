// Offscreen readback of a world-less frame: the clear, the UI layer, and the
// resolve that composites them, exactly as a windowed Frontend or Loading
// frame records them, written to a capture target instead of a swapchain.
// See: context/lib/rendering_pipeline.md §7.8 · context/lib/ui.md §5

use super::*;

impl Renderer {
    /// Render the stored UI snapshot over `clear_color` with no world, through
    /// the windowed UI and resolve passes, into an `Rgba8UnormSrgb` target the
    /// size of the capture extent, and return its tight RGBA8 pixels.
    ///
    /// Offscreen renderers only. Unlike `capture_frame_indirect` this keeps
    /// the UI and the resolve, so it shows what a player sees on a world-less
    /// frame; it admits no SH drain, since a world-less frame streams nothing.
    pub fn capture_world_less_frame(
        &mut self,
        font_system: &mut postretro_ui::text::FontSystem,
        clear_color: ClearColor,
        now_seconds: f64,
    ) -> Result<Vec<u8>> {
        self.cpu_frame.clear();
        self.commit_native_capture_extents();
        let extent = self.render_extents().surface;
        let (width, height) = (extent.width, extent.height);
        let target = self.device.create_texture(&wgpu::TextureDescriptor {
            label: Some("World-less Capture Target"),
            size: wgpu::Extent3d {
                width,
                height,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            // The offscreen renderer's resolve pipeline targets this format.
            format: self.surface_config.format,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
            view_formats: &[],
        });
        let view = target.create_view(&wgpu::TextureViewDescriptor::default());
        let visible = VisibleCells::DrawAll;
        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("World-less Capture Encoder"),
            });
        self.record_scene_passes(
            &mut encoder,
            Some(font_system),
            Some(&view),
            CameraCullVisibility {
                cells: &visible,
                path: postretro_visibility::VisibilityPath::EmptyWorldFallback,
            },
            &[],
            &[],
            &[],
            ShSampleRegionSets {
                visible_cells: &visible,
                fog_cells: &[],
                movers: &[],
            },
            None,
            Mat4::IDENTITY,
            &[],
            &[],
            now_seconds,
            clear_color,
            FrameScene::Empty,
        )?;
        self.queue.submit(std::iter::once(encoder.finish()));
        self.queue.assert_empty("world-less capture submit");
        self.full_mut().ui.mark_submitted();
        let readback_encoder =
            self.device
                .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                    label: Some("World-less Readback Encoder"),
                });
        let pixels = self.read_texture_rgba8(&target, width, height, readback_encoder)?;
        self.queue.complete_frame();
        Ok(pixels)
    }
}
