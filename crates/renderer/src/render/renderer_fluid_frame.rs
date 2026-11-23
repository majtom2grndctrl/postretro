// The frame's fluid passes: the scene-input upload, the screen-space
// reflection pass and the fluid surface pass, in that order. The sums they
// read dispatch earlier (`renderer_probe_sums_frame.rs`).
// See: context/lib/rendering_pipeline.md §7.3a

use super::*;
use postretro_render_cpu::fluid_surface::reflection_quality::FluidReflectionQuality;

impl Renderer {
    /// Record the fluid surface pass and the reflection work it reads this
    /// frame. The caller gates on the pass's own visible draws;
    /// `frame_light_term_mask` and `frame_reflection_quality` are the frame's
    /// one mask snapshot and tier, which pick every draw's shader variant.
    pub(super) fn record_fluid_surface_passes(
        &mut self,
        encoder: &mut wgpu::CommandEncoder,
        scene_color: &wgpu::TextureView,
        frame_light_term_mask: LightTermMask,
        frame_reflection_quality: FluidReflectionQuality,
    ) {
        // Only a drawing frame reads the scene inputs (fog weight,
        // reflection style, target scale). The weight is stored every
        // frame; all upload here lazily, only when changed, before the
        // reflection pass, so both passes draw this frame's style; the
        // batched write copies ahead of this frame's submission.
        {
            let Self { queue, full, .. } = self;
            let full = full
                .as_mut()
                .expect("renderer full-init must complete before full-ready paths run");
            full.fluid_surface.upload_scene_inputs(queue);
        }
        // The screen-space reflection layer, resolved into its own target
        // before the fluid pass samples it: the frame's reflecting draws,
        // by the same mask snapshot and tier the fluid pass picks variants
        // by. Off, Low and a frame with no reflecting draw record none.
        if self.full().fluid_surface.has_reflecting_draws(
            self.fluid_surfaces(),
            frame_light_term_mask,
            frame_reflection_quality,
        ) {
            self.record_fluid_reflection_pass(
                encoder,
                frame_light_term_mask,
                frame_reflection_quality,
            );
        }
        let fluid_ts = self
            .full()
            .frame_timing
            .as_ref()
            .map(|t| t.render_pass_writes(TIMING_PAIR_FLUID_SURFACE));
        let mut fluid_pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("Fluid Surface Pass"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: scene_color,
                depth_slice: None,
                resolve_target: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Load,
                    store: wgpu::StoreOp::Store,
                },
            })],
            // Read-only: the pass tests depth without writing it, and its
            // group 5 loads the same texture for depth fog.
            depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                view: &self.full().depth_view,
                depth_ops: None,
                stencil_ops: None,
            }),
            timestamp_writes: fluid_ts,
            ..Default::default()
        });
        fluid_pass.set_bind_group(0, &self.full().uniform_bind_group, &[]);
        fluid_pass.set_bind_group(2, &self.level().lighting_bind_group, &[]);
        fluid_pass.set_bind_group(3, self.level().sh_bind_group(), &[]);
        // The frame's group-0 mask and tier pick each draw's shader
        // variant.
        self.full().fluid_surface.record_draws(
            &mut fluid_pass,
            self.fluid_surfaces(),
            &self.level().gpu_textures,
            frame_light_term_mask,
            frame_reflection_quality,
        );
    }
}
