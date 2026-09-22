// Per-frame renderer pass recording factored out of frame orchestration.
// See: context/lib/rendering_pipeline.md §7.1, §7.6

#[cfg(test)]
use super::renderer_dynamic_shadow_passes::tally_entity_occluder_submissions;
use super::*;

fn wireframe_draws_leaf(
    mode: WorldWireframeMode,
    visible: &VisibleCells,
    leaf: &postretro_render_data::geometry::BvhLeaf,
) -> bool {
    match mode {
        WorldWireframeMode::Off => false,
        WorldWireframeMode::CullStatusTrianglesAlwaysOnTop => true,
        WorldWireframeMode::VisibleTrianglesDepthTested => match visible {
            VisibleCells::DrawAll => true,
            VisibleCells::Culled(cells) => cells.contains(&leaf.cell_id),
        },
    }
}

impl Renderer {
    /// Depth pre-pass (writes the scene depth buffer for the forward Equal test)
    /// followed by the half-res SDF shadow dispatch. Both run before `scene_color`
    /// is bound and before the forward pass that consumes them.
    pub(super) fn record_depth_and_sdf_passes(
        &mut self,
        encoder: &mut wgpu::CommandEncoder,
        view_proj: Mat4,
        render_world: bool,
    ) {
        let Self { queue, full, .. } = self;
        let full = full
            .as_mut()
            .expect("renderer full-init must complete before full-ready paths run");
        if render_world {
            let depth_ts = full
                .frame_timing
                .as_ref()
                .map(|t| t.render_pass_writes(TIMING_PAIR_DEPTH_PREPASS));
            let mut depth_pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("Depth Pre-Pass"),
                // Vertex-only: depth attachment only. The lightmap-UV gbuffer
                // MRT was removed with the animated dominant-direction trace.
                color_attachments: &[],
                depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                    view: &full.depth_view,
                    depth_ops: Some(wgpu::Operations {
                        load: wgpu::LoadOp::Clear(1.0),
                        store: wgpu::StoreOp::Store,
                    }),
                    stencil_ops: None,
                }),
                timestamp_writes: depth_ts,
                ..Default::default()
            });

            if full.has_geometry && full.index_count > 0 {
                depth_pass.set_pipeline(&full.depth_prepass_pipeline);
                depth_pass.set_bind_group(0, &full.uniform_bind_group, &[]);
                depth_pass.set_vertex_buffer(0, full.vertex_buffer.slice(..));
                depth_pass.set_index_buffer(full.index_buffer.slice(..), wgpu::IndexFormat::Uint32);

                if let Some(cull) = &full.compute_cull {
                    cull.draw_indirect(&mut depth_pass, None); // None = no texture bind (group 0 only)
                }
            }
        }

        // SDF half-res shadow pass. Runs after the depth pre-pass because it
        // consumes scene depth, and before the forward pass that samples the
        // shadow factor. Skipped when no SDF atlas is loaded; forward-side
        // atlas/mode flags gate consumption so stale target contents are ignored.
        if render_world && full.sdf_atlas_resources.present {
            let sdf_ts = full
                .frame_timing
                .as_ref()
                .map(|t| t.compute_pass_writes(TIMING_PAIR_SDF_SHADOW));
            let inv_view_proj = view_proj.inverse();
            // TEMP DEBUG: SDF shadow path visualization. When a debug-viz mode is
            // selected, the pass writes a debug RGB code into slot 0 instead of
            // per-light visibility floats. The mode value (3 = debug paths,
            // 4 = normals) is threaded so the shader picks the right encoding;
            // 0 means "not a debug mode" (production path).
            let sdf_debug_mode = match full.sdf_shadow_mode {
                SdfShadowMode::VisualizeDebugPaths => SdfShadowMode::VisualizeDebugPaths as u32,
                SdfShadowMode::VisualizeNormals => SdfShadowMode::VisualizeNormals as u32,
                _ => 0,
            };
            full.sdf_shadow_pass.dispatch(
                queue,
                encoder,
                &full.sdf_atlas_resources,
                SdfShadowFrameInputs {
                    inv_view_proj,
                    camera_position: full.last_camera_position.into(),
                },
                sdf_ts,
                sdf_debug_mode,
            );
        }
    }
}

#[cfg(feature = "dev-tools")]
impl Renderer {
    /// Encode + submit the SH atlas readback copy after the frame submit. A no-op
    /// unless the diagnostics irradiance overlay requested a copy this frame.
    pub(super) fn encode_sh_probe_readback(&mut self) {
        let Self {
            device,
            queue,
            full,
            ..
        } = self;
        let full = full
            .as_mut()
            .expect("renderer full-init must complete before full-ready paths run");
        // Capture the just-composed SH atlas for the live irradiance overlay.
        // Separate submission so the boundary orders this copy after the compose
        // storage writes (see the note at the compose dispatch above). Skipped
        // unless the overlay is active.
        if full.sh_probe_readback.wants_copy() {
            // Block until the compose submit above has fully retired before the
            // copy reads `total`. A submission boundary alone does not hard-sync
            // the compute storage writes against the copy on the Metal backend:
            // when the in-room compose runs longer (active delta lights), the
            // copy catches the last-written (high-z) texels mid-flight and reads
            // foreign/zero garbage. Only reached while the overlay is active, so
            // the per-readback stall is confined to debug sessions.
            let _ = device.poll(wgpu::PollType::wait_indefinitely());
            let mut readback_encoder =
                device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
                    label: Some("SH Readback Encoder"),
                });
            full.sh_probe_readback.encode_copy(
                &mut readback_encoder,
                &full.sh_volume_resources.total_atlas_texture,
            );
            queue.submit(std::iter::once(readback_encoder.finish()));
        }
    }
}

impl Renderer {
    /// Wireframe BVH-leaf overlay. The cull-status mode draws every loaded leaf
    /// always-on-top with GPU cull-status tinting. The visible mode draws only
    /// leaves from the frame's CPU `VisibleCells` set, depth-tested, with a flat
    /// color so it does not imply final GPU BVH/frustum survivors.
    pub(super) fn record_wireframe_overlay(
        &self,
        encoder: &mut wgpu::CommandEncoder,
        scene_color: &wgpu::TextureView,
        render_world: bool,
        visible: &VisibleCells,
    ) {
        let Self { device, full, .. } = self;
        let full = full
            .as_ref()
            .expect("renderer full-init must complete before full-ready paths run");
        if render_world
            && full.wireframe_enabled
            && full.has_geometry
            && full.wireframe_index_count > 0
            && !full.bvh_leaves.is_empty()
        {
            if let Some(cull) = &full.compute_cull {
                let cull_status_bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
                    label: Some("Wireframe Cull Status BG"),
                    layout: &full.wireframe_cull_status_bgl,
                    entries: &[wgpu::BindGroupEntry {
                        binding: 0,
                        resource: cull.cull_status_buffer().as_entire_binding(),
                    }],
                });

                let mut overlay_pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                    label: Some("Wireframe Overlay Pass"),
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
                        depth_ops: Some(wgpu::Operations {
                            load: wgpu::LoadOp::Load,
                            store: wgpu::StoreOp::Store,
                        }),
                        stencil_ops: None,
                    }),
                    ..Default::default()
                });

                let pipeline = match full.world_wireframe_mode {
                    WorldWireframeMode::Off => return,
                    WorldWireframeMode::CullStatusTrianglesAlwaysOnTop => {
                        &full.wireframe_cull_status_pipeline
                    }
                    WorldWireframeMode::VisibleTrianglesDepthTested => {
                        &full.wireframe_visible_pipeline
                    }
                };

                overlay_pass.set_pipeline(pipeline);
                overlay_pass.set_bind_group(0, &full.uniform_bind_group, &[]);
                overlay_pass.set_bind_group(1, &cull_status_bind_group, &[]);
                overlay_pass.set_vertex_buffer(0, full.vertex_buffer.slice(..));
                overlay_pass.set_index_buffer(
                    full.wireframe_index_buffer.slice(..),
                    wgpu::IndexFormat::Uint32,
                );

                // instance_index = leaf index so shader looks up per-leaf cull status.
                for (leaf_idx, leaf) in full.bvh_leaves.iter().enumerate() {
                    if !wireframe_draws_leaf(full.world_wireframe_mode, visible, leaf) {
                        continue;
                    }
                    let wire_offset = leaf.index_offset * 2;
                    let wire_count = leaf.index_count * 2;
                    let li = leaf_idx as u32;
                    overlay_pass.draw_indexed(wire_offset..wire_offset + wire_count, 0, li..li + 1);
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn entity_occluder_tally_counts_movers_in_dynamic_and_promoted_paths() {
        let mut spot = 0;
        let mut cube = 0;
        let mut promoted = 0;

        // Skinned submissions arrive first; mover recorder returns must join
        // the same pool total and only promoted slots join the subset.
        tally_entity_occluder_submissions(&mut spot, None, 2);
        tally_entity_occluder_submissions(&mut spot, None, 3);
        tally_entity_occluder_submissions(&mut spot, Some(&mut promoted), 5);
        tally_entity_occluder_submissions(&mut spot, Some(&mut promoted), 7);
        tally_entity_occluder_submissions(&mut cube, None, 11);
        tally_entity_occluder_submissions(&mut cube, None, 13);
        tally_entity_occluder_submissions(&mut cube, Some(&mut promoted), 17);
        tally_entity_occluder_submissions(&mut cube, Some(&mut promoted), 19);

        assert_eq!(spot, 17);
        assert_eq!(cube, 60);
        assert_eq!(promoted, 48);
    }

    fn leaf(cell_id: u32) -> postretro_render_data::geometry::BvhLeaf {
        postretro_render_data::geometry::BvhLeaf {
            aabb_min: [0.0; 3],
            material_bucket_id: 0,
            aabb_max: [1.0; 3],
            index_offset: 0,
            index_count: 3,
            cell_id,
            chunk_range_start: 0,
            chunk_range_count: 0,
        }
    }

    #[test]
    fn cull_status_wireframe_draws_every_leaf() {
        let visible = VisibleCells::Culled(vec![2]);

        assert!(wireframe_draws_leaf(
            WorldWireframeMode::CullStatusTrianglesAlwaysOnTop,
            &visible,
            &leaf(1),
        ));
    }

    #[test]
    fn visible_wireframe_draws_only_cpu_visible_cells() {
        let visible = VisibleCells::Culled(vec![2, 4]);

        assert!(wireframe_draws_leaf(
            WorldWireframeMode::VisibleTrianglesDepthTested,
            &visible,
            &leaf(2),
        ));
        assert!(!wireframe_draws_leaf(
            WorldWireframeMode::VisibleTrianglesDepthTested,
            &visible,
            &leaf(3),
        ));
    }

    #[test]
    fn visible_wireframe_draws_all_for_draw_all_visibility() {
        assert!(wireframe_draws_leaf(
            WorldWireframeMode::VisibleTrianglesDepthTested,
            &VisibleCells::DrawAll,
            &leaf(999),
        ));
    }
}
