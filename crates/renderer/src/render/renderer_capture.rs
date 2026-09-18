// Renderer-owned offscreen capture recording and RGBA8 readback.
// See: context/lib/rendering_pipeline.md §7.8

use super::*;

impl Renderer {
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
