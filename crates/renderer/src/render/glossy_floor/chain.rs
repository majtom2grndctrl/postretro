// The scene-colour chain's recording: one downsample pass per level, level 0
// from the highlight-free scene colour, each further level from the one above.
// See: context/lib/rendering_pipeline.md §7.3a (Glossy world faces, Floors)

use super::{GlossyFloorPass, GlossyFloorTargets};

impl GlossyFloorPass {
    /// Record `targets`' chain into `encoder`: every level, top first, each
    /// cleared and written whole, so it holds this frame's highlight-free
    /// scene alone. The caller records it after every opaque pass that
    /// writes the highlight-free colour and before the overlay.
    pub(crate) fn record_chain(
        &self,
        encoder: &mut wgpu::CommandEncoder,
        targets: &GlossyFloorTargets,
    ) {
        #[cfg(test)]
        self.chains.set(self.chains.get() + 1);
        for (target, source) in targets.levels() {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("Glossy Floor Chain Level"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: target,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                ..Default::default()
            });
            pass.set_pipeline(&self.downsample);
            pass.set_bind_group(0, source, &[]);
            pass.draw(0..3, 0..1);
        }
    }
}
