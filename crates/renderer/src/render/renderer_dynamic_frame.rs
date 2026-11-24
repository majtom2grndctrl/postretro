// The frame's opaque dynamic objects after world forward: the kinematic
// brush mover pass, then the skinned-mesh pass.
// See: context/lib/rendering_pipeline.md §7.3, §9

use super::cpu_stages::RenderStage;
use super::glossy_floor::ColourOutputs;
use super::*;

impl Renderer {
    /// Kinematic brush movers: local-space PRL brush payloads drawn as
    /// dynamic objects after opaque world geometry and before skinned meshes.
    /// Their depth is already in the pre-pass, so they test `Equal` with the
    /// depth attachment read-only, as the world forward pass does. They use
    /// the mesh dynamic-object lighting bindings (baked SH indirect/static
    /// direct + runtime dynamic direct). On a frame that runs the glossy
    /// floor layer (`floor_layer`) they also write the highlight-free colour,
    /// overwriting forward's at every pixel they draw.
    pub(super) fn record_mover_pass(
        &mut self,
        encoder: &mut wgpu::CommandEncoder,
        scene_color: &wgpu::TextureView,
        render_world: bool,
        floor_layer: bool,
    ) {
        if !render_world
            || !self
                .full()
                .kinematic_brush
                .has_draws(&self.level().kinematic)
        {
            return;
        }
        let cpu = std::rc::Rc::clone(&self.cpu_frame);
        let _kinematic_scope = cpu.scope(RenderStage::KinematicBrush);
        let frame_light_term_mask = self.frame_light_term_mask();
        {
            let Self { queue, full, .. } = self;
            let full = full
                .as_mut()
                .expect("renderer full-init must complete before full-ready paths run");
            let level = full
                .level
                .as_mut()
                .expect("a level-gated renderer path requires an active level set");
            // Total records bound the loop; the dynamic-tier count marks the promoted specular tail.
            full.kinematic_brush.write_light_params(
                queue,
                level.total_light_count,
                level.light_count,
                level.light_count + level.animated_baked_light_count as u32,
                full.mesh_dynamic_time,
                frame_light_term_mask.bits(),
                full.ambient_floor,
                full.frame_indirect_scale,
            );
        }
        let kinematic_ts = self
            .full()
            .frame_timing
            .as_ref()
            .map(|t| t.render_pass_writes(TIMING_PAIR_KINEMATIC_BRUSH));
        let highlight_free = self.glossy_highlight_free_view(floor_layer);
        let attachments = [Some(scene_color), highlight_free].map(load_and_store);
        let mut mover_pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("Kinematic Brush Pass"),
            color_attachments: &attachments[..1 + usize::from(highlight_free.is_some())],
            depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                view: &self.full().depth_view,
                depth_ops: None,
                stencil_ops: None,
            }),
            timestamp_writes: kinematic_ts,
            ..Default::default()
        });
        mover_pass.set_bind_group(0, &self.full().uniform_bind_group, &[]);
        mover_pass.set_bind_group(4, self.level().sh_mesh_bind_group(), &[]);
        self.full().kinematic_brush.record_draws(
            &mut mover_pass,
            &self.level().kinematic,
            &self.level().gpu_textures,
            MoverView::Main,
            ColourOutputs::of(highlight_free.is_some()),
            self.full().frame_specular_style,
        );
    }

    /// Skinned-mesh forward pass — after the opaque world forward, before
    /// billboards. Its own render pass so it can WRITE depth (the forward
    /// pass holds the depth attachment read-only). Loads the existing color
    /// + depth so the mesh composites over the world and depth-tests (`Less`).
    ///
    /// Reads the `plan` PLANNED + UPLOADED earlier in this frame (the
    /// pose/upload hoist, before the shadow loop). NO re-plan, NO re-upload
    /// here — `record_draws` only records draws against the buffers the
    /// hoist populated, the SAME buffers the skinned-depth shadow pass read,
    /// so an entity and its shadow share one pose (no one-frame lag). On a
    /// frame that runs the glossy floor layer (`floor_layer`) the mesh, which
    /// draws no highlight, writes its colour as the highlight-free colour
    /// too, overwriting forward's and the movers' where it draws.
    pub(super) fn record_skinned_mesh_pass(
        &self,
        encoder: &mut wgpu::CommandEncoder,
        scene_color: &wgpu::TextureView,
        render_world: bool,
        plan: Option<&mesh_instances::MeshFramePlan>,
        floor_layer: bool,
    ) {
        let Some(plan) = plan.filter(|plan| render_world && plan.has_forward_draws()) else {
            return;
        };
        let cpu = std::rc::Rc::clone(&self.cpu_frame);
        let _skinned_scope = cpu.scope(RenderStage::SkinnedMesh);
        let skinned_ts = self
            .full()
            .frame_timing
            .as_ref()
            .map(|t| t.render_pass_writes(TIMING_PAIR_SKINNED_MESH));
        let highlight_free = self.glossy_highlight_free_view(floor_layer);
        let attachments = [Some(scene_color), highlight_free].map(load_and_store);
        let mut mesh_enc = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("Skinned Mesh Pass"),
            color_attachments: &attachments[..1 + usize::from(highlight_free.is_some())],
            depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                view: &self.full().depth_view,
                depth_ops: Some(wgpu::Operations {
                    load: wgpu::LoadOp::Load,
                    store: wgpu::StoreOp::Store,
                }),
                stencil_ops: None,
            }),
            timestamp_writes: skinned_ts,
            ..Default::default()
        });
        mesh_enc.set_bind_group(0, &self.full().uniform_bind_group, &[]);
        // Group 4 = SH irradiance volume (baked indirect) + the mesh-only
        // dynamic-direct params uniform (binding 16). The mesh SUPERSET bind
        // group: shared SH entries the forward/billboard/fog passes hold PLUS
        // the dynamic-direct knobs (group 3 = instance data; group 2
        // unallocated).
        mesh_enc.set_bind_group(4, self.level().sh_mesh_bind_group(), &[]);
        self.full().mesh_pass.record_draws(
            &mut mesh_enc,
            &self.level().models,
            &self.level().mesh_lights,
            plan,
            ColourOutputs::of(highlight_free.is_some()),
        );
    }
}

/// A colour attachment that keeps what earlier passes wrote.
fn load_and_store(view: Option<&wgpu::TextureView>) -> Option<wgpu::RenderPassColorAttachment<'_>> {
    view.map(|view| wgpu::RenderPassColorAttachment {
        view,
        depth_slice: None,
        resolve_target: None,
        ops: wgpu::Operations {
            load: wgpu::LoadOp::Load,
            store: wgpu::StoreOp::Store,
        },
    })
}
