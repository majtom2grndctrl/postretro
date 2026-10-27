// Planar pass recording: mask, mirror world, movers, skinned meshes, billboards, then the resolve.
// See: context/lib/rendering_pipeline.md §7.3a

use super::*;
use crate::render::cpu_stages::RenderStage;

fn color_attachment(
    view: &wgpu::TextureView,
    load: wgpu::LoadOp<wgpu::Color>,
) -> wgpu::RenderPassColorAttachment<'_> {
    wgpu::RenderPassColorAttachment {
        view,
        depth_slice: None,
        resolve_target: None,
        ops: wgpu::Operations {
            load,
            store: wgpu::StoreOp::Store,
        },
    }
}

fn depth_attachment(
    view: &wgpu::TextureView,
    load: wgpu::LoadOp<f32>,
) -> wgpu::RenderPassDepthStencilAttachment<'_> {
    wgpu::RenderPassDepthStencilAttachment {
        view,
        depth_ops: Some(wgpu::Operations {
            load,
            store: wgpu::StoreOp::Store,
        }),
        stencil_ops: None,
    }
}

impl PlanarReflectionPass {
    /// The mask and resolve uniform for `camera`, as the shaders read it.
    /// The style and the offset multiplier are integers stored by their bits.
    #[allow(clippy::too_many_arguments)] // One argument per field group of the fixed uniform.
    fn inputs(
        camera: &MirrorCamera,
        preset: PlanarPreset,
        main_vp: Mat4,
        main_eye: Vec3,
        extent: (u32, u32),
        scene: (u32, u32),
        style: FluidReflectionStyle,
        offset_scale: i32,
    ) -> [f32; PLANAR_INPUT_FLOATS] {
        let mut inputs = [0.0f32; PLANAR_INPUT_FLOATS];
        inputs[..16].copy_from_slice(&main_vp.to_cols_array());
        inputs[16..32].copy_from_slice(&camera.raster_vp.to_cols_array());
        inputs[32..48].copy_from_slice(&camera.raster_vp.inverse().to_cols_array());
        inputs[48..51].copy_from_slice(&main_eye.to_array());
        inputs[51] = camera.h;
        inputs[52..55].copy_from_slice(&camera.eye.to_array());
        inputs[56..60].copy_from_slice(&[
            preset.d.unwrap_or(0.0),
            preset.band_d,
            preset.r.unwrap_or(0.0),
            preset.band_r,
        ]);
        inputs[60..64].copy_from_slice(&[
            extent.0 as f32,
            extent.1 as f32,
            scene.0 as f32,
            scene.1 as f32,
        ]);
        inputs[64] = f32::from_bits(style.uniform_value());
        inputs[65] = f32::from_bits(offset_scale as u32);
        inputs
    }

    /// Write the inputs uniform where it changed.
    fn upload_inputs(
        &mut self,
        queue: &crate::render::uploads::UploadQueue,
        inputs: &[f32; PLANAR_INPUT_FLOATS],
    ) {
        if self.uploaded_inputs.as_ref() == Some(inputs) {
            return;
        }
        queue.write_buffer(&self.inputs_buffer, 0, bytemuck::cast_slice(inputs));
        self.uploaded_inputs = Some(*inputs);
    }
}

impl Renderer {
    /// The planar reflection pass: after the ghost pass, before the fluid
    /// gate, on the frame's one encoder, so the fluid pass samples this
    /// frame's mirror. Records nothing, and routes no draw to the planar
    /// layer, below very high, with no selected plane, without a planar
    /// layer, or on a frame whose reflecting draws hold none of the plane.
    pub(in crate::render) fn record_planar_reflection_pass(
        &mut self,
        encoder: &mut wgpu::CommandEncoder,
        render_world: bool,
        meshes: Option<&mesh_instances::MeshFramePlan>,
    ) {
        let quality = self.reflection_quality;
        let light_term_mask = self.frame_light_term_mask();
        let style = self.fluid_reflection_style;
        let cpu = std::rc::Rc::clone(&self.cpu_frame);
        let Self { queue, full, .. } = self;
        let full = full
            .as_mut()
            .expect("renderer full-init must complete before full-ready paths run");
        full.fluid_surface.set_planar_route(None);
        let pass = &full.planar_reflection;
        let (Some(preset), Some(camera), Some(extent)) = (
            pass.preset(quality),
            pass.camera,
            pass.targets.as_ref().map(|targets| targets.extent),
        ) else {
            return;
        };
        if !render_world
            || !full.fluid_surface.has_draws()
            || full.fluid_surface.planar_layer_extent() != Some(extent)
            || !full
                .fluid_surface
                .has_planar_parts(light_term_mask, quality, camera.h)
        {
            return;
        }
        // Only a recording frame opens the stage, so a frame with no planar
        // pass reports it absent, never as zero.
        let _planar_scope = cpu.scope(RenderStage::PlanarReflection);

        // Uploads and the world draw list, before the passes borrow the targets.
        let scene = full.depth_view.texture().size();
        let inputs = PlanarReflectionPass::inputs(
            &camera,
            preset,
            full.last_view_proj,
            full.last_camera_position,
            extent,
            (scene.width, scene.height),
            style,
            pass.offset_scale(),
        );
        full.planar_reflection.upload_inputs(queue, &inputs);
        full.fluid_surface.upload_planar_inputs(queue);
        let buckets = full
            .compute_cull
            .as_ref()
            .map_or(&[][..], |cull| cull.bucket_ranges());
        let PlanarReflectionPass { draws, visible, .. } = &mut full.planar_reflection;
        draws.rebuild(
            &full.bvh_leaves,
            buckets,
            full.cell_draw_index.as_ref(),
            visible,
            &camera.cull_planes(),
        );
        let sh = full.sh_bind_group().clone();
        let sh_mesh = full.sh_mesh_bind_group().clone();

        let frame = PlanarFrame {
            camera,
            light_term_mask,
            quality,
            meshes,
            sh: &sh,
            sh_mesh: &sh_mesh,
        };
        if record_planar_passes(full, encoder, &frame) {
            full.fluid_surface.set_planar_route(Some(camera.h));
            #[cfg(test)]
            {
                full.planar_reflection.records += 1;
            }
        }
    }
}

/// One planar frame's inputs, gathered before the passes borrow the targets.
struct PlanarFrame<'a> {
    camera: MirrorCamera,
    light_term_mask: LightTermMask,
    quality: FluidReflectionQuality,
    meshes: Option<&'a mesh_instances::MeshFramePlan>,
    sh: &'a wgpu::BindGroup,
    sh_mesh: &'a wgpu::BindGroup,
}

/// Record the mask, the mirror's world, movers, skinned meshes and
/// billboards, and the resolve, all on `encoder`. False, recording nothing,
/// without targets or a planar layer. Ghosts and the viewmodel never enter:
/// only the world mesh plan and the mirror's own sprites reach the mirror.
fn record_planar_passes(
    full: &crate::render::renderer_types::FullRenderer,
    encoder: &mut wgpu::CommandEncoder,
    frame: &PlanarFrame<'_>,
) -> bool {
    let PlanarFrame {
        camera,
        light_term_mask,
        quality,
        meshes,
        sh,
        sh_mesh,
    } = *frame;
    let pass = &full.planar_reflection;
    let Some(targets) = pass.targets.as_ref() else {
        return false;
    };
    let Some((layer, layer_depth)) = full.fluid_surface.planar_layer_attachments() else {
        return false;
    };
    if let Some(timing) = &full.frame_timing {
        timing.write_encoder_start(encoder, TIMING_PAIR_PLANAR_REFLECTION);
    }
    {
        let mut mask = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("Planar Water Mask"),
            color_attachments: &[],
            depth_stencil_attachment: Some(depth_attachment(
                &targets.depth,
                wgpu::LoadOp::Clear(0.0),
            )),
            ..Default::default()
        });
        mask.set_pipeline(&pass.pipelines.mask);
        mask.set_bind_group(0, &targets.mask_inputs, &[]);
        full.fluid_surface
            .record_planar_parts(&mut mask, light_term_mask, quality, camera.h, None);
    }
    let world = full.has_geometry && full.index_count > 0 && !pass.draws.draws().is_empty();
    if world {
        let mut depth = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("Planar World Prepass"),
            color_attachments: &[],
            depth_stencil_attachment: Some(depth_attachment(&targets.depth, wgpu::LoadOp::Load)),
            ..Default::default()
        });
        depth.set_pipeline(&full.depth_prepass_pipeline);
        depth.set_bind_group(0, &pass.uniform_bind_group, &[]);
        depth.set_vertex_buffer(0, full.vertex_buffer.slice(..));
        depth.set_index_buffer(full.index_buffer.slice(..), wgpu::IndexFormat::Uint32);
        for run in pass.draws.depth_runs() {
            depth.draw_indexed(run.clone(), 0, 0..1);
        }
    }
    {
        // Opens even without world draws: it clears the mirror colour the
        // movers and skinned meshes load.
        let mut forward = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("Planar World Forward"),
            color_attachments: &[Some(color_attachment(
                &targets.color,
                wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT),
            ))],
            depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                view: &targets.depth,
                depth_ops: None,
                stencil_ops: None,
            }),
            timestamp_writes: full
                .frame_timing
                .as_ref()
                .map(|t| t.render_pass_writes(TIMING_PAIR_PLANAR_FORWARD)),
            ..Default::default()
        });
        if world {
            forward.set_pipeline(&full.pipeline);
            forward.set_bind_group(0, &pass.uniform_bind_group, &[]);
            // Group 2 swaps to the mirror's light records while a mirror-only
            // light holds a shadow slot; only the reflection shows that shadow.
            forward.set_bind_group(
                2,
                full.mirror_lighting
                    .lighting_bind_group(&full.lighting_bind_group),
                &[],
            );
            forward.set_bind_group(3, sh, &[]);
            forward.set_bind_group(4, &full.lightmap_resources.bind_group, &[]);
            forward.set_bind_group(5, &full.spot_shadow_pool.bind_group, &[]);
            forward.set_bind_group(
                LIGHTMAP_BLOCK_TABLE_GROUP,
                &full.lightmap_resources.block_table_bind_group,
                &[],
            );
            forward.set_vertex_buffer(0, full.vertex_buffer.slice(..));
            forward.set_vertex_buffer(1, full.spec_owner_buffer.slice(..));
            forward.set_index_buffer(full.index_buffer.slice(..), wgpu::IndexFormat::Uint32);
            let mut bound_bucket = None;
            for draw in pass.draws.draws() {
                if bound_bucket != Some(draw.bucket) {
                    let material = full
                        .gpu_textures
                        .get(draw.bucket as usize)
                        .unwrap_or(&full.gpu_textures[0]);
                    forward.set_bind_group(1, &material.bind_group, &[]);
                    bound_bucket = Some(draw.bucket);
                }
                forward.draw_indexed(draw.first_index..draw.end_index, 0, 0..1);
            }
        }
    }
    if full.kinematic_brush.has_draws() {
        let mut movers = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("Planar Movers"),
            color_attachments: &[Some(color_attachment(&targets.color, wgpu::LoadOp::Load))],
            depth_stencil_attachment: Some(depth_attachment(&targets.depth, wgpu::LoadOp::Load)),
            ..Default::default()
        });
        movers.set_bind_group(0, &pass.uniform_bind_group, &[]);
        movers.set_bind_group(4, sh_mesh, &[]);
        full.kinematic_brush.record_mirror_draws(
            &mut movers,
            &full.gpu_textures,
            &camera.cull_planes(),
            camera.clip_plane,
        );
    }
    if let Some(plan) = meshes {
        let mut skinned = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("Planar Skinned Meshes"),
            color_attachments: &[Some(color_attachment(&targets.color, wgpu::LoadOp::Load))],
            depth_stencil_attachment: Some(depth_attachment(&targets.depth, wgpu::LoadOp::Load)),
            ..Default::default()
        });
        skinned.set_bind_group(0, &pass.uniform_bind_group, &[]);
        skinned.set_bind_group(4, sh_mesh, &[]);
        full.mesh_pass.record_mirror_draws(&mut skinned, plan);
    }
    if pass.sprites.has_draws() && full.smoke_pass.has_any_sheet() {
        // After the opaque mirror, as the billboard pass follows the opaque
        // scene: depth-tested, never written, additive. The mirror's group 0
        // turns every quad to the mirror camera.
        let mut sprites = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("Planar Billboards"),
            color_attachments: &[Some(color_attachment(&targets.color, wgpu::LoadOp::Load))],
            depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                view: &targets.depth,
                depth_ops: None,
                stencil_ops: None,
            }),
            ..Default::default()
        });
        sprites.set_bind_group(0, &pass.uniform_bind_group, &[]);
        sprites.set_bind_group(2, &full.lighting_bind_group, &[]);
        sprites.set_bind_group(3, sh, &[]);
        pass.sprites.record_draws(&mut sprites, &full.smoke_pass);
    }
    {
        // Cleared every frame it records, colour and depth, so a texel no
        // selected-plane fragment wrote reads c = 0.
        let mut resolve = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("Planar Resolve"),
            color_attachments: &[Some(color_attachment(
                layer,
                wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT),
            ))],
            depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                view: layer_depth,
                depth_ops: Some(wgpu::Operations {
                    load: wgpu::LoadOp::Clear(1.0),
                    store: wgpu::StoreOp::Discard,
                }),
                stencil_ops: None,
            }),
            ..Default::default()
        });
        resolve.set_pipeline(&pass.pipelines.resolve);
        // The main camera's group 0, as the reflection pass binds it: the
        // eye the reflection law reads and the animation clock the offset
        // steps on.
        resolve.set_bind_group(
            pipelines::RESOLVE_CAMERA_GROUP,
            &full.uniform_bind_group,
            &[],
        );
        resolve.set_bind_group(
            pipelines::RESOLVE_INPUTS_GROUP,
            &targets.resolve_inputs,
            &[],
        );
        full.fluid_surface.record_planar_parts(
            &mut resolve,
            light_term_mask,
            quality,
            camera.h,
            Some((pipelines::RESOLVE_MATERIAL_GROUP, &full.gpu_textures)),
        );
    }
    if let Some(timing) = &full.frame_timing {
        timing.write_encoder_end(encoder, TIMING_PAIR_PLANAR_REFLECTION);
    }
    true
}
