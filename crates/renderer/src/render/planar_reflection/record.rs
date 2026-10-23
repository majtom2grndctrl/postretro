use super::*;

fn color_attachment(view: &wgpu::TextureView, clear: bool) -> wgpu::RenderPassColorAttachment<'_> {
    wgpu::RenderPassColorAttachment {
        view,
        depth_slice: None,
        resolve_target: None,
        ops: wgpu::Operations {
            load: if clear {
                wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT)
            } else {
                wgpu::LoadOp::Load
            },
            store: wgpu::StoreOp::Store,
        },
    }
}
fn depth_attachment(
    view: &wgpu::TextureView,
    clear: bool,
) -> wgpu::RenderPassDepthStencilAttachment<'_> {
    wgpu::RenderPassDepthStencilAttachment {
        view,
        depth_ops: Some(wgpu::Operations {
            load: if clear {
                wgpu::LoadOp::Clear(0.0)
            } else {
                wgpu::LoadOp::Load
            },
            store: wgpu::StoreOp::Store,
        }),
        stencil_ops: None,
    }
}

impl Renderer {
    pub(in crate::render) fn record_planar_reflection_pass(
        &mut self,
        encoder: &mut wgpu::CommandEncoder,
        meshes: Option<&mesh_instances::MeshFramePlan>,
    ) {
        let Self {
            device,
            queue,
            has_multi_draw_indirect,
            full,
            ..
        } = self;
        let full = full.as_mut().expect("planar recording requires full init");
        let sh = full.sh_bind_group().clone();
        let sh_mesh = full.sh_mesh_bind_group().clone();
        let planar = &mut full.planar_reflection;
        let Some(targets) = &planar.targets else {
            return;
        };
        if let Some(timing) = &full.frame_timing {
            timing.write_encoder_start(encoder, TIMING_PAIR_PLANAR_REFLECTION);
        }
        if let Some(camera) = planar.camera {
            let mut inputs = [0.0f32; 64];
            inputs[..16].copy_from_slice(&full.last_view_proj.to_cols_array());
            inputs[16..32].copy_from_slice(&full.last_view_proj.inverse().to_cols_array());
            inputs[32..48].copy_from_slice(&camera.raster_vp.inverse().to_cols_array());
            inputs[48..51].copy_from_slice(&full.last_camera_position.to_array());
            inputs[51] = camera.h;
            inputs[52..55].copy_from_slice(&camera.eye.to_array());
            inputs[56..60].copy_from_slice(&[
                planar.settings.d.unwrap_or(0.0),
                planar.settings.band_d,
                planar.settings.r.unwrap_or(0.0),
                planar.settings.band_r,
            ]);
            let scene = full.depth_view.texture().size();
            inputs[60..64].copy_from_slice(&[
                targets.extent.0 as f32,
                targets.extent.1 as f32,
                scene.width as f32,
                scene.height as f32,
            ]);
            queue.write_buffer(&planar.inputs_buffer, 0, bytemuck::cast_slice(&inputs));
            if let (Some(main_cull), Some(index)) = (&full.compute_cull, &full.cell_draw_index) {
                let count = main_cull.total_leaves();
                if planar.cull.as_ref().is_none_or(|c| c.leaf_count != count) {
                    let buffer = |label, size, usage| {
                        device.create_buffer(&wgpu::BufferDescriptor {
                            label: Some(label),
                            size,
                            usage,
                            mapped_at_creation: false,
                        })
                    };
                    planar.cull = Some(MirrorCull {
                        candidate: crate::candidate_cull::CandidateCullPipeline::new(device, count),
                        indirect: buffer(
                            "Planar Indirect Draws",
                            u64::from(count.max(1)) * crate::compute_cull::DRAW_INDIRECT_SIZE,
                            wgpu::BufferUsages::STORAGE
                                | wgpu::BufferUsages::INDIRECT
                                | wgpu::BufferUsages::COPY_DST,
                        ),
                        status: buffer(
                            "Planar Cull Status",
                            u64::from(count.max(1)) * 4,
                            wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
                        ),
                        ranges: Default::default(),
                        leaf_count: count,
                        all_cells: Vec::new(),
                    });
                }
                let cull = planar.cull.as_mut().expect("created above");
                cull.candidate.set_pass_label("Planar Candidate Cull");
                let cells = match &planar.visible {
                    VisibleCells::Culled(cells) => cells.as_slice(),
                    VisibleCells::DrawAll => {
                        cull.all_cells.clear();
                        cull.all_cells.extend(0..index.cell_count);
                        &cull.all_cells
                    }
                };
                cull.ranges
                    .rebuild(Some(index), &planar.visible, main_cull.bucket_ranges());
                if matches!(
                    cull.candidate.gather(index, cells),
                    crate::candidate_cull::GatherStatus::Ok
                ) {
                    encoder.push_debug_group("Planar Candidate Cull");
                    cull.candidate.dispatch(
                        device,
                        queue,
                        encoder,
                        main_cull.leaf_buffer(),
                        &cull.indirect,
                        &cull.status,
                        &camera.cull_vp,
                        None,
                    );
                    encoder.pop_debug_group();
                } else {
                    encoder.clear_buffer(&cull.indirect, 0, None);
                }
            }
        }
        {
            let mut mask = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("Planar Water Mask"),
                color_attachments: &[Some(color_attachment(&targets.coverage, true))],
                depth_stencil_attachment: Some(depth_attachment(&targets.depth, true)),
                ..Default::default()
            });
            if planar.camera.is_some()
                && let Some(vertices) = &planar.vertices
            {
                mask.set_pipeline(&planar.mask_pipeline);
                mask.set_bind_group(0, &targets.mask_inputs, &[]);
                mask.set_vertex_buffer(0, vertices.slice(..));
                mask.draw(0..planar.vertex_count, 0..1);
            }
        }
        if planar.camera.is_some()
            && let Some(cull) = &planar.cull
        {
            let mut depth = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("Planar World Prepass"),
                color_attachments: &[],
                depth_stencil_attachment: Some(depth_attachment(&targets.depth, false)),
                ..Default::default()
            });
            depth.set_pipeline(&full.depth_prepass_pipeline);
            depth.set_bind_group(0, &planar.uniform_bind_group, &[]);
            depth.set_vertex_buffer(0, full.vertex_buffer.slice(..));
            depth.set_index_buffer(full.index_buffer.slice(..), wgpu::IndexFormat::Uint32);
            crate::compute_cull::draw_indirect_buckets(
                &mut depth,
                &cull.indirect,
                cull.ranges.ranges(),
                *has_multi_draw_indirect,
                None,
            );
        }
        {
            let mut forward = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("Planar World Forward"),
                color_attachments: &[Some(color_attachment(&targets.color, true))],
                depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                    view: &targets.depth,
                    depth_ops: None,
                    stencil_ops: None,
                }),
                timestamp_writes: full
                    .frame_timing
                    .as_ref()
                    .filter(|_| planar.camera.is_some())
                    .map(|t| t.render_pass_writes(TIMING_PAIR_PLANAR_FORWARD)),
                ..Default::default()
            });
            if planar.camera.is_some()
                && let Some(cull) = &planar.cull
            {
                forward.set_pipeline(&full.pipeline);
                forward.set_bind_group(0, &planar.uniform_bind_group, &[]);
                forward.set_bind_group(2, &full.lighting_bind_group, &[]);
                forward.set_bind_group(3, &sh, &[]);
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
                crate::compute_cull::draw_indirect_buckets(
                    &mut forward,
                    &cull.indirect,
                    cull.ranges.ranges(),
                    *has_multi_draw_indirect,
                    Some(&|pass, bucket| {
                        let material = full
                            .gpu_textures
                            .get(bucket as usize)
                            .unwrap_or(&full.gpu_textures[0]);
                        pass.set_bind_group(1, &material.bind_group, &[]);
                    }),
                );
            }
        }
        if planar.camera.is_some() && full.kinematic_brush.has_draws() {
            let mut movers = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("Planar Movers"),
                color_attachments: &[Some(color_attachment(&targets.color, false))],
                depth_stencil_attachment: Some(depth_attachment(&targets.depth, false)),
                ..Default::default()
            });
            movers.set_bind_group(0, &planar.uniform_bind_group, &[]);
            movers.set_bind_group(4, &sh_mesh, &[]);
            full.kinematic_brush
                .record_draws(&mut movers, &full.gpu_textures);
        }
        if planar.camera.is_some()
            && let Some(plan) = meshes
        {
            let mut skinned = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("Planar Skinned Meshes"),
                color_attachments: &[Some(color_attachment(&targets.color, false))],
                depth_stencil_attachment: Some(depth_attachment(&targets.depth, false)),
                ..Default::default()
            });
            skinned.set_bind_group(0, &planar.uniform_bind_group, &[]);
            skinned.set_bind_group(4, &sh_mesh, &[]);
            full.mesh_pass.record_draws(&mut skinned, plan);
        }
        {
            let mut resolve = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("Planar Resolve"),
                color_attachments: &[Some(color_attachment(&targets.resolved, true))],
                ..Default::default()
            });
            if planar.camera.is_some() {
                resolve.set_pipeline(&planar.resolve_pipeline);
                resolve.set_bind_group(0, &targets.resolve_inputs, &[]);
                resolve.draw(0..3, 0..1);
            }
        }
        if let Some(timing) = &full.frame_timing {
            timing.write_encoder_end(encoder, TIMING_PAIR_PLANAR_REFLECTION);
        }
    }
}
