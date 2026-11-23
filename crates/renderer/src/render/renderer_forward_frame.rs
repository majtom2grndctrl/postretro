// The world forward pass: the opaque world's one shaded draw over the
// frame's visible spans, into the HDR scene colour.
// See: context/lib/rendering_pipeline.md §7.3

use super::cpu_stages::RenderStage;
use super::*;
use postretro_render_cpu::fluid_surface::reflection_quality::FluidReflectionQuality;

impl Renderer {
    /// Record world forward into `scene_color`, cleared to `clear_color`, over
    /// the depth the pre-pass wrote. A frame that draws no world still clears.
    /// On a level whose glossy faces are ready at `frame_reflection_quality`
    /// above off, forward also writes D_static, cleared to 0, as a second
    /// output: glossy buckets through the static-diffuse entry, every other
    /// bucket with that output masked. Otherwise it keeps one output.
    pub(super) fn record_world_forward_pass(
        &self,
        encoder: &mut wgpu::CommandEncoder,
        scene_color: &wgpu::TextureView,
        clear_color: ClearColor,
        render_world: bool,
        frame_reflection_quality: FluidReflectionQuality,
    ) {
        let cpu = std::rc::Rc::clone(&self.cpu_frame);
        let _forward_scope = cpu.scope(RenderStage::Forward);
        let static_diffuse =
            self.forward_static_diffuse_view(render_world, frame_reflection_quality);
        let static_diffuse_attachment =
            static_diffuse.map(|view| wgpu::RenderPassColorAttachment {
                view,
                depth_slice: None,
                resolve_target: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT),
                    store: wgpu::StoreOp::Store,
                },
            });
        let forward_ts = self
            .full()
            .frame_timing
            .as_ref()
            .map(|t| t.render_pass_writes(TIMING_PAIR_FORWARD));
        let mut render_pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("Textured Pass"),
            color_attachments: &[
                Some(wgpu::RenderPassColorAttachment {
                    view: scene_color,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(clear_color.into()),
                        store: wgpu::StoreOp::Store,
                    },
                }),
                static_diffuse_attachment,
            ][..1 + usize::from(static_diffuse.is_some())],
            depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                view: &self.full().depth_view,
                // Forward pass uses `depth_compare: Equal` with depth
                // writes disabled — the depth buffer is read-only here.
                // Task 5 of sdf-static-occluder-shadows samples this
                // same depth texture via group 5 binding 4 (the
                // bilateral upsample's depth-aware weights); wgpu
                // requires `depth_ops: None` so the attachment doesn't
                // alias a writable resource with a sampled-texture
                // binding. The depth contents the pre-pass wrote
                // persist for the wireframe pass that follows.
                depth_ops: None,
                stencil_ops: None,
            }),
            timestamp_writes: forward_ts,
            ..Default::default()
        });

        if render_world && self.level().has_geometry && self.level().index_count > 0 {
            render_pass.set_bind_group(0, &self.full().uniform_bind_group, &[]);
            render_pass.set_bind_group(2, &self.level().lighting_bind_group, &[]);
            render_pass.set_bind_group(3, self.level().sh_bind_group(), &[]);
            render_pass.set_bind_group(4, &self.level().lightmap_resources.bind_group, &[]);
            render_pass.set_bind_group(5, &self.level().extent.spot_shadow, &[]);
            render_pass.set_bind_group(
                LIGHTMAP_BLOCK_TABLE_GROUP,
                &self.level().lightmap_resources.block_table_bind_group,
                &[],
            );
            render_pass.set_vertex_buffer(0, self.level().vertex_buffer.slice(..));
            render_pass.set_vertex_buffer(1, self.level().spec_owner_buffer.slice(..));
            render_pass.set_vertex_buffer(2, self.level().emissive_volumes.vertex.slice(..));
            render_pass.set_index_buffer(
                self.level().index_buffer.slice(..),
                wgpu::IndexFormat::Uint32,
            );

            if let Some(cull) = &self.level().compute_cull {
                let gpu_textures = &self.level().gpu_textures;
                let bind_material = |pass: &mut wgpu::RenderPass<'_>, bucket: u32| {
                    let bind_group = if (bucket as usize) < gpu_textures.len() {
                        &gpu_textures[bucket as usize].bind_group
                    } else {
                        &gpu_textures[0].bind_group
                    };
                    pass.set_bind_group(1, bind_group, &[]);
                };
                if static_diffuse.is_some() {
                    // Two sweeps, each binding its materials once per bucket
                    // as the one-output draw does: two pipeline switches.
                    let forms = self.full().glossy_overlay.forward_forms();
                    let glossy = |bucket: u32| self.level().bucket_is_glossy(bucket);
                    render_pass.set_pipeline(&forms.masked);
                    cull.draw_indirect_subset(&mut render_pass, Some(&bind_material), &|bucket| {
                        !glossy(bucket)
                    });
                    render_pass.set_pipeline(&forms.written);
                    cull.draw_indirect_subset(&mut render_pass, Some(&bind_material), &glossy);
                } else {
                    render_pass.set_pipeline(&self.full().pipeline);
                    cull.draw_indirect(&mut render_pass, Some(&bind_material));
                }
            }
        }
    }

    /// Forward's D_static attachment this frame: the active level's target
    /// when its glossy faces are ready at a tier above off, else `None` and
    /// forward keeps its one output.
    fn forward_static_diffuse_view(
        &self,
        render_world: bool,
        frame_reflection_quality: FluidReflectionQuality,
    ) -> Option<&wgpu::TextureView> {
        if !render_world || frame_reflection_quality == FluidReflectionQuality::Off {
            return None;
        }
        self.full()
            .level
            .as_ref()?
            .glossy
            .as_ref()
            .filter(|glossy| glossy.ready())?
            .static_diffuse_view()
    }
}
