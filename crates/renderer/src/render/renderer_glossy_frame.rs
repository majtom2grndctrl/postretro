// The frame's glossy world overlay: its draw gate, its one pass after the
// skinned-mesh pass and before the Ghost Pass, and its D_static lifecycle.
// See: context/lib/rendering_pipeline.md §7.3a (Glossy world faces)

use super::glossy_overlay::{GLOSSY_OVERLAY_MATERIAL_GROUP, glossy_overlay_draws};
use super::*;
use postretro_render_cpu::fluid_surface::reflection_quality::FluidReflectionQuality;

impl Renderer {
    /// Whether this frame records the overlay under `frame_light_term_mask`
    /// and `frame_reflection_quality`, the frame's one mask snapshot and
    /// tier: the active level's glossy faces are ready and a camera range
    /// draws a glossy bucket. Also the overlay's input to the sums' dispatch.
    pub(super) fn glossy_overlay_probe_term_draws(
        &self,
        render_world: bool,
        frame_light_term_mask: LightTermMask,
        frame_reflection_quality: FluidReflectionQuality,
    ) -> bool {
        let Some(level) = self.full().level.as_ref().filter(|_| render_world) else {
            return false;
        };
        let ready = level.glossy.as_ref().is_some_and(|glossy| glossy.ready());
        // Cheapest first: the range scan runs only on a glossy level.
        glossy_overlay_draws(frame_reflection_quality, frame_light_term_mask, ready, true)
            && level
                .compute_cull
                .as_ref()
                .is_some_and(|cull| cull.draws_bucket(|bucket| level.bucket_is_glossy(bucket)))
    }

    /// Record the overlay over the frame's glossy camera ranges into
    /// `scene_color`, when [`Self::glossy_overlay_probe_term_draws`] holds.
    /// Scene colour loads and blends additively; scene depth is attached
    /// read-only for the Equal test.
    pub(super) fn record_glossy_overlay_pass(
        &self,
        encoder: &mut wgpu::CommandEncoder,
        scene_color: &wgpu::TextureView,
        render_world: bool,
        frame_light_term_mask: LightTermMask,
        frame_reflection_quality: FluidReflectionQuality,
    ) {
        if !self.glossy_overlay_probe_term_draws(
            render_world,
            frame_light_term_mask,
            frame_reflection_quality,
        ) {
            return;
        }
        let full = self.full();
        let level = self.level();
        let (Some(glossy), Some(cull)) = (level.glossy.as_ref(), level.compute_cull.as_ref())
        else {
            return;
        };
        let timestamps = full
            .frame_timing
            .as_ref()
            .map(|timing| timing.render_pass_writes(TIMING_PAIR_GLOSSY_OVERLAY));
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("Glossy Overlay Pass"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: scene_color,
                depth_slice: None,
                resolve_target: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Load,
                    store: wgpu::StoreOp::Store,
                },
            })],
            depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                view: &full.depth_view,
                depth_ops: None,
                stencil_ops: None,
            }),
            timestamp_writes: timestamps,
            ..Default::default()
        });
        pass.set_bind_group(0, &full.uniform_bind_group, &[]);
        // Forward's material bind group per bucket: the overlay reads the
        // texel forward's highlight read, and a live uniform rewrite reaches
        // both on the same frame.
        let bind_material = |pass: &mut wgpu::RenderPass<'_>, bucket: u32| {
            pass.set_bind_group(
                GLOSSY_OVERLAY_MATERIAL_GROUP,
                level.material_bind_group(bucket),
                &[],
            );
        };
        // The committed reflection style, read each frame, so the first
        // frame after a style commit draws in the new style (pin P9).
        full.glossy_overlay.record(
            &mut pass,
            self.fluid_reflection_style,
            glossy,
            &level.vertex_buffer,
            &level.index_buffer,
            cull,
            &|bucket| level.bucket_is_glossy(bucket),
            &bind_material,
        );
    }

    /// Bring the active level's D_static target in line with the kept
    /// reflection tier at the current scene extent: allocated above off,
    /// released at off. Compiles nothing.
    pub(super) fn sync_glossy_target(&mut self) {
        let quality = self.reflection_quality;
        let extent = self.scene_extent();
        let Self { device, full, .. } = self;
        let Some(full) = full.as_mut() else {
            return;
        };
        let layout = full.glossy_overlay.level_layout();
        if let Some(glossy) = full.level.as_mut().and_then(|level| level.glossy.as_mut()) {
            layout.sync_target(device, glossy, quality, extent);
        }
    }
}
