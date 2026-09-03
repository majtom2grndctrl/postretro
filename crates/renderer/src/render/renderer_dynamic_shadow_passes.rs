// Spot/cube dynamic and promoted shadow-depth pass recording.
// See: context/lib/rendering_pipeline.md §7.1

use super::*;
use crate::render::mesh_depth::MeshDepthInstanceFilter;

/// Adds actual entity-depth draws to the pool total and, for a promoted slot,
/// to its promoted subset.
pub(super) fn tally_entity_occluder_submissions(
    pool_total: &mut u32,
    promoted_total: Option<&mut u32>,
    submitted: u32,
) {
    *pool_total += submitted;
    if let Some(promoted_total) = promoted_total {
        *promoted_total += submitted;
    }
}

impl Renderer {
    pub(super) fn record_spot_shadow_depth(
        &mut self,
        encoder: &mut wgpu::CommandEncoder,
        mesh_frame_plan: Option<&mesh_instances::MeshFramePlan>,
    ) {
        let Self { queue, full, .. } = self;
        let full = full
            .as_mut()
            .expect("renderer full-init must complete before full-ready paths run");
        let stride = full.shadow_vs_stride;
        let draw_world = full.has_geometry && full.index_count > 0;
        let cache_plan = &full.promoted_depth_cache_frame_plan;
        let dynamic_cache_plan = full.dynamic_depth_cache_frame_plan;
        full.dynamic_depth_cache_diagnostics.frame = Default::default();
        full.dynamic_depth_cache_diagnostics.frame.cached_spots =
            dynamic_cache_plan.spot().len() as u32;
        // One pair spans the whole pool loop because cached and uncached slots
        // interleave. The label reports an upper bound, including entity passes
        // and promoted work; counters identify the world/cull work actually saved.
        if !dynamic_cache_plan.spot().is_empty() {
            if let Some(timing) = &full.frame_timing {
                timing.write_encoder_start(encoder, TIMING_PAIR_DYNAMIC_SPOT_DEPTH);
            }
        }
        let occupied_slots = full
            .spot_shadow_pool
            .slot_cone_matrices
            .map(|matrix| matrix.is_some());

        // Reset the per-frame entity-occluder counter; the per-slot cull
        // tallies into it below. Mirrors `shadow-cone-cull`'s submitted
        // counter — pure CPU, no GPU readback.
        full.spot_entity_occluders_submitted = 0;

        // Per-slot GPU cone cull dispatches only slots that need static-world
        // depth: uncached slots and cold cache fills. Warm dynamic-cache slots
        // skip this traversal: cached world depth is copied into the pool before
        // current entity occluders draw.
        if draw_world {
            if let Some(shadow_cull) = &full.shadow_cull {
                full.promoted_depth_cache_cull_dispatch_skips +=
                    cache_plan.skipped_spot_cull_dispatches(&occupied_slots);
                full.dynamic_depth_cache_diagnostics
                    .frame
                    .cull_dispatch_skips += occupied_slots
                    .iter()
                    .enumerate()
                    .filter(|&(slot, occupied)| {
                        *occupied && !dynamic_cache_plan.should_dispatch_spot_cull(slot)
                    })
                    .count() as u32;
                shadow_cull.dispatch_occupied_slots_filtered(
                    queue,
                    encoder,
                    &full.spot_shadow_pool.slot_cone_matrices,
                    |slot| {
                        cache_plan.should_dispatch_spot_cull(slot)
                            && dynamic_cache_plan.should_dispatch_spot_cull(slot)
                    },
                );
            }
        }

        for (slot, occupied) in occupied_slots.into_iter().enumerate() {
            if !occupied {
                continue;
            }
            let slot = slot as u32;
            let promoted_plan = cache_plan.spot_for_slot(slot);
            if let Some(plan) = promoted_plan {
                // Open the coarse promoted-depth-cache GPU-timing pair lazily on the
                // first promoted slot; it closes at the end of the cube loop.
                // This span is an UPPER BOUND: interleaved dynamic spot slots
                // and the whole cube dynamic-shadow loop fall inside it. A tight
                // bracket over only the promoted cache-render + entity
                // regions would need per-region accumulation — promoted and
                // dynamic slots interleave in the sorted slot order, and one
                // timestamp pair (`[2i, 2i+1]`) cannot sum disjoint spans (a
                // second open/close overwrites the first). So the pair stays
                // coarse by design.
                if !full.promoted_depth_cache_timing_open {
                    if let Some(timing) = &full.frame_timing {
                        timing.write_encoder_start(encoder, TIMING_PAIR_PROMOTED_DEPTH_CACHE);
                    }
                    full.promoted_depth_cache_timing_open = true;
                }
                if plan.needs_world_render {
                    {
                        let cache_view = full
                            .promoted_depth_cache
                            .as_ref()
                            .expect("promoted spot plan implies cache allocated")
                            .spot_view(plan);
                        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                            label: Some("Promoted Spot World Depth Cache Pass"),
                            color_attachments: &[],
                            depth_stencil_attachment: Some(
                                wgpu::RenderPassDepthStencilAttachment {
                                    view: cache_view,
                                    depth_ops: Some(wgpu::Operations {
                                        load: wgpu::LoadOp::Clear(1.0),
                                        store: wgpu::StoreOp::Store,
                                    }),
                                    stencil_ops: None,
                                },
                            ),
                            timestamp_writes: None,
                            ..Default::default()
                        });
                        if draw_world {
                            pass.set_pipeline(&full.shadow_depth_pipeline);
                            pass.set_bind_group(0, &full.shadow_vs_bind_group, &[slot * stride]);
                            pass.set_vertex_buffer(0, full.vertex_buffer.slice(..));
                            pass.set_index_buffer(
                                full.index_buffer.slice(..),
                                wgpu::IndexFormat::Uint32,
                            );
                            if let Some(shadow_cull) = &full.shadow_cull {
                                shadow_cull.draw_slot_indirect(&mut pass, slot, None);
                            } else {
                                pass.draw_indexed(0..full.index_count, 0, 0..1);
                            }
                        }
                    }
                    full.promoted_depth_cache
                        .as_mut()
                        .expect("promoted spot plan implies cache allocated")
                        .mark_spot_world_rendered(plan);
                }

                let view = &full.spot_shadow_pool.views[slot as usize];
                let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                    label: Some("Promoted Spot Entity Shadow Depth Pass"),
                    color_attachments: &[],
                    depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                        view,
                        depth_ops: Some(wgpu::Operations {
                            load: wgpu::LoadOp::Clear(1.0),
                            store: wgpu::StoreOp::Store,
                        }),
                        stencil_ops: None,
                    }),
                    timestamp_writes: None,
                    ..Default::default()
                });
                if full.spot_shadow_pool.slot_entity_eligible[slot as usize]
                    && (mesh_frame_plan.is_some() || !full.mover_occluder_aabbs.is_empty())
                {
                    if let Some(cone_matrix) =
                        full.spot_shadow_pool.slot_cone_matrices[slot as usize]
                    {
                        let cone_planes =
                            postretro_render_data::cone_frustum::cone_frustum_planes(&cone_matrix);
                        if let Some(mesh_plan) = &mesh_frame_plan {
                            let submitted = full.mesh_pass.record_skinned_depth(
                                &mut pass,
                                mesh_plan,
                                MeshDepthInstanceFilter::AllRetained,
                                &full.shadow_vs_bind_group,
                                slot * stride,
                                &cone_planes,
                            );
                            tally_entity_occluder_submissions(
                                &mut full.spot_entity_occluders_submitted,
                                Some(&mut full.promoted_entity_occluders_submitted),
                                submitted,
                            );
                        }
                        let submitted = full.rigid_occluder_depth.record_kinematic_movers(
                            &mut pass,
                            &full.kinematic_brush,
                            &full.mover_occluder_aabbs,
                            &full.shadow_vs_bind_group,
                            slot * stride,
                            &cone_planes,
                        );
                        tally_entity_occluder_submissions(
                            &mut full.spot_entity_occluders_submitted,
                            Some(&mut full.promoted_entity_occluders_submitted),
                            submitted,
                        );
                    }
                }
                continue;
            }

            if let Some(plan) = dynamic_cache_plan.spot_for_slot(slot) {
                if plan.needs_world_render {
                    {
                        let cache_view = full.dynamic_depth_cache.spot_view(plan);
                        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                            label: Some("Dynamic Spot World Depth Cache Pass"),
                            color_attachments: &[],
                            depth_stencil_attachment: Some(
                                wgpu::RenderPassDepthStencilAttachment {
                                    view: cache_view,
                                    depth_ops: Some(wgpu::Operations {
                                        load: wgpu::LoadOp::Clear(1.0),
                                        store: wgpu::StoreOp::Store,
                                    }),
                                    stencil_ops: None,
                                },
                            ),
                            timestamp_writes: None,
                            ..Default::default()
                        });
                        if draw_world {
                            pass.set_pipeline(&full.shadow_depth_pipeline);
                            pass.set_bind_group(0, &full.shadow_vs_bind_group, &[slot * stride]);
                            pass.set_vertex_buffer(0, full.vertex_buffer.slice(..));
                            pass.set_index_buffer(
                                full.index_buffer.slice(..),
                                wgpu::IndexFormat::Uint32,
                            );
                            if let Some(shadow_cull) = &full.shadow_cull {
                                shadow_cull.draw_slot_indirect(&mut pass, slot, None);
                            } else {
                                pass.draw_indexed(0..full.index_count, 0, 0..1);
                            }
                        }
                    }
                    full.dynamic_depth_cache
                        .state
                        .mark_spot_world_rendered(plan);
                } else {
                    full.dynamic_depth_cache_diagnostics.frame.world_pass_skips += 1;
                }

                dynamic_depth_cache::copy_cached_world_depth(
                    encoder,
                    full.dynamic_depth_cache
                        .spot_texture
                        .as_ref()
                        .expect("dynamic spot cache allocated"),
                    plan.cache_layer as u32,
                    &full.spot_shadow_pool.array_texture,
                    slot,
                    crate::lighting::spot_shadow::SHADOW_MAP_RESOLUTION,
                );
                let view = &full.spot_shadow_pool.views[slot as usize];
                let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                    label: Some("Dynamic Spot Entity Shadow Depth Pass"),
                    color_attachments: &[],
                    depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                        view,
                        depth_ops: Some(wgpu::Operations {
                            load: wgpu::LoadOp::Load,
                            store: wgpu::StoreOp::Store,
                        }),
                        stencil_ops: None,
                    }),
                    timestamp_writes: None,
                    ..Default::default()
                });
                if full.spot_shadow_pool.slot_entity_eligible[slot as usize] {
                    if let Some(cone_matrix) =
                        full.spot_shadow_pool.slot_cone_matrices[slot as usize]
                    {
                        let cone_planes =
                            postretro_render_data::cone_frustum::cone_frustum_planes(&cone_matrix);
                        if let Some(mesh_plan) = &mesh_frame_plan {
                            let submitted = full.mesh_pass.record_skinned_depth(
                                &mut pass,
                                mesh_plan,
                                MeshDepthInstanceFilter::DynamicCasters,
                                &full.shadow_vs_bind_group,
                                slot * stride,
                                &cone_planes,
                            );
                            tally_entity_occluder_submissions(
                                &mut full.spot_entity_occluders_submitted,
                                None,
                                submitted,
                            );
                        }
                        let submitted = full.rigid_occluder_depth.record_kinematic_movers(
                            &mut pass,
                            &full.kinematic_brush,
                            &full.mover_occluder_aabbs,
                            &full.shadow_vs_bind_group,
                            slot * stride,
                            &cone_planes,
                        );
                        tally_entity_occluder_submissions(
                            &mut full.spot_entity_occluders_submitted,
                            None,
                            submitted,
                        );
                    }
                }
                continue;
            }

            let view = &full.spot_shadow_pool.views[slot as usize];
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("Spot Shadow Depth Pass"),
                color_attachments: &[],
                depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                    view,
                    depth_ops: Some(wgpu::Operations {
                        load: wgpu::LoadOp::Clear(1.0),
                        store: wgpu::StoreOp::Store,
                    }),
                    stencil_ops: None,
                }),
                timestamp_writes: None,
                ..Default::default()
            });
            if draw_world {
                pass.set_pipeline(&full.shadow_depth_pipeline);
                pass.set_bind_group(0, &full.shadow_vs_bind_group, &[slot * stride]);
                pass.set_vertex_buffer(0, full.vertex_buffer.slice(..));
                pass.set_index_buffer(full.index_buffer.slice(..), wgpu::IndexFormat::Uint32);
                // Indirect cone-culled draw from this slot's sub-region. The
                // depth-only shadow pipeline has no group-1 material slot, so
                // `None` skips the texture bind (matching the depth pre-pass).
                // Fall back to the full unconditional draw if the shadow cull
                // owner is absent (no BVH).
                if let Some(shadow_cull) = &full.shadow_cull {
                    shadow_cull.draw_slot_indirect(&mut pass, slot, None);
                } else {
                    pass.draw_indexed(0..full.index_count, 0, 0..1);
                }
            }

            // Entity occluders into the SAME slot. The skinned and rigid
            // depth paths share the slot's light-space bind group, dynamic
            // offset, and CPU cone planes.
            //
            // TWO gates (kept separate from pool-slot eligibility):
            //   1. `slot_entity_eligible[slot]` — the slot's light passes
            //      `entity_occluder_eligible` (dynamic + toggle on). An
            //      ineligible slot keeps its world shadow (already drawn
            //      above) but draws ZERO entity occluders.
            //   2. per-occluder cone cull — only bounds intersecting this
            //      slot's cone are submitted.
            if full.spot_shadow_pool.slot_entity_eligible[slot as usize] {
                if let Some(cone_matrix) = full.spot_shadow_pool.slot_cone_matrices[slot as usize] {
                    let cone_planes =
                        postretro_render_data::cone_frustum::cone_frustum_planes(&cone_matrix);
                    if let Some(plan) = &mesh_frame_plan {
                        let submitted = full.mesh_pass.record_skinned_depth(
                            &mut pass,
                            plan,
                            MeshDepthInstanceFilter::DynamicCasters,
                            &full.shadow_vs_bind_group,
                            slot * stride,
                            &cone_planes,
                        );
                        tally_entity_occluder_submissions(
                            &mut full.spot_entity_occluders_submitted,
                            None,
                            submitted,
                        );
                    }
                    let submitted = full.rigid_occluder_depth.record_kinematic_movers(
                        &mut pass,
                        &full.kinematic_brush,
                        &full.mover_occluder_aabbs,
                        &full.shadow_vs_bind_group,
                        slot * stride,
                        &cone_planes,
                    );
                    tally_entity_occluder_submissions(
                        &mut full.spot_entity_occluders_submitted,
                        None,
                        submitted,
                    );
                }
            }
        }
        if !dynamic_cache_plan.spot().is_empty() {
            if let Some(timing) = &full.frame_timing {
                timing.write_encoder_end(encoder, TIMING_PAIR_DYNAMIC_SPOT_DEPTH);
            }
        }
    }

    /// Cube point-light shadow depth loop: clear every occupied live-pool face
    /// to the far plane. Uncached slots draw static world plus eligible entity
    /// occluders. A cold dynamic-cache slot draws static world into its cache,
    /// then copies it to the live pool before entity occluders draw. Warm slots
    /// perform only the copy and entity draw. Caller gates on `render_world`.
    pub(super) fn record_cube_shadow_depth(
        &mut self,
        encoder: &mut wgpu::CommandEncoder,
        mesh_frame_plan: Option<&mesh_instances::MeshFramePlan>,
    ) {
        let Self { queue, full, .. } = self;
        let full = full
            .as_mut()
            .expect("renderer full-init must complete before full-ready paths run");
        let cache_plan = &full.promoted_depth_cache_frame_plan;
        let dynamic_cache_plan = full.dynamic_depth_cache_frame_plan;
        full.dynamic_depth_cache_diagnostics.frame.cached_cubes =
            dynamic_cache_plan.cube().len() as u32;
        if !dynamic_cache_plan.cube().is_empty() {
            if let Some(timing) = &full.frame_timing {
                timing.write_encoder_start(encoder, TIMING_PAIR_DYNAMIC_CUBE_DEPTH);
            }
        }
        if let Some(pool) = &full.cube_shadow_pool {
            let stride = full.shadow_vs_stride;
            let draw_world = full.has_geometry && full.index_count > 0;

            // Per-face GPU frustum cull dispatches layers that need static-world
            // depth: uncached layers and cold cache fills. Warm dynamic-cache
            // layers skip it because cached world depth is copied into the live pool
            // before current entity occluders draw. `pool.face_matrices` remains the source of truth
            // for both cull and VS uniforms.
            if draw_world {
                if let Some(cube_cull) = &full.cube_shadow_cull {
                    let occupied_layers: [bool;
                        crate::lighting::cube_shadow::CUBE_COUNT
                            * crate::lighting::cube_shadow::CUBE_FACES] =
                        std::array::from_fn(|layer| pool.face_matrices[layer].is_some());
                    full.promoted_depth_cache_cull_dispatch_skips +=
                        cache_plan.skipped_cube_cull_dispatches(&occupied_layers);
                    full.dynamic_depth_cache_diagnostics
                        .frame
                        .cull_dispatch_skips += occupied_layers
                        .iter()
                        .enumerate()
                        .filter(|&(layer, occupied)| {
                            *occupied && !dynamic_cache_plan.should_dispatch_cube_cull(layer)
                        })
                        .count() as u32;
                    cube_cull.dispatch_occupied_slots_filtered(
                        queue,
                        encoder,
                        &pool.face_matrices,
                        |layer| {
                            cache_plan.should_dispatch_cube_cull(layer)
                                && dynamic_cache_plan.should_dispatch_cube_cull(layer)
                        },
                    );
                }
            }

            for layer in 0..pool.face_matrices.len() {
                let face_matrix_opt = pool.face_matrices[layer];
                // Only occupied faces are touched; every occupied live-pool face
                // gets its Clear(1.0) far-plane baseline this frame, mesh plan or
                // not. A warm dynamic-cache face does not draw static world here;
                // it redraws only live entity occluders. See
                // `cube_shadow::cube_face_needs_clear` for why the clear must not
                // be gated on the plan.
                if !crate::lighting::cube_shadow::cube_face_needs_clear(face_matrix_opt.is_some()) {
                    continue;
                }
                let face_matrix = face_matrix_opt.expect("face_needs_clear implies occupied");
                let slot = layer / crate::lighting::cube_shadow::CUBE_FACES;
                let face = layer % crate::lighting::cube_shadow::CUBE_FACES;
                let promoted_plan = cache_plan.cube_for_slot(slot as u32);

                if let Some(plan) = promoted_plan {
                    // Same coarse promoted-depth-cache timing pair as the spot loop
                    // (see its open site): opens here only when no promoted spot
                    // opened it first. Still the same upper-bound span.
                    if !full.promoted_depth_cache_timing_open {
                        if let Some(timing) = &full.frame_timing {
                            timing.write_encoder_start(encoder, TIMING_PAIR_PROMOTED_DEPTH_CACHE);
                        }
                        full.promoted_depth_cache_timing_open = true;
                    }
                    if plan.needs_world_render {
                        {
                            let cache_view = full
                                .promoted_depth_cache
                                .as_ref()
                                .expect("promoted cube plan implies cache allocated")
                                .cube_face_view(plan, face);
                            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                                label: Some("Promoted Cube World Depth Cache Pass"),
                                color_attachments: &[],
                                depth_stencil_attachment: Some(
                                    wgpu::RenderPassDepthStencilAttachment {
                                        view: cache_view,
                                        depth_ops: Some(wgpu::Operations {
                                            load: wgpu::LoadOp::Clear(1.0),
                                            store: wgpu::StoreOp::Store,
                                        }),
                                        stencil_ops: None,
                                    },
                                ),
                                timestamp_writes: None,
                                ..Default::default()
                            });

                            if draw_world {
                                pass.set_pipeline(&full.shadow_depth_pipeline);
                                pass.set_bind_group(
                                    0,
                                    &full.cube_shadow_vs_bind_group,
                                    &[layer as u32 * stride],
                                );
                                pass.set_vertex_buffer(0, full.vertex_buffer.slice(..));
                                pass.set_index_buffer(
                                    full.index_buffer.slice(..),
                                    wgpu::IndexFormat::Uint32,
                                );
                                if let Some(cube_cull) = &full.cube_shadow_cull {
                                    cube_cull.draw_slot_indirect(&mut pass, layer as u32, None);
                                } else {
                                    pass.draw_indexed(0..full.index_count, 0, 0..1);
                                }
                            }
                        }
                        if face + 1 == crate::lighting::cube_shadow::CUBE_FACES {
                            full.promoted_depth_cache
                                .as_mut()
                                .expect("promoted cube plan implies cache allocated")
                                .mark_cube_world_rendered(plan);
                        }
                    }

                    let view = &pool.face_views[layer];
                    let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                        label: Some("Promoted Cube Entity Shadow Depth Pass"),
                        color_attachments: &[],
                        depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                            view,
                            depth_ops: Some(wgpu::Operations {
                                load: wgpu::LoadOp::Clear(1.0),
                                store: wgpu::StoreOp::Store,
                            }),
                            stencil_ops: None,
                        }),
                        timestamp_writes: None,
                        ..Default::default()
                    });
                    if pool.slot_entity_eligible[slot]
                        && (mesh_frame_plan.is_some() || !full.mover_occluder_aabbs.is_empty())
                    {
                        let face_planes =
                            postretro_render_data::cone_frustum::cone_frustum_planes(&face_matrix);
                        if let Some(mesh_plan) = &mesh_frame_plan {
                            let submitted = full.mesh_pass.record_skinned_depth(
                                &mut pass,
                                mesh_plan,
                                MeshDepthInstanceFilter::AllRetained,
                                &full.cube_shadow_vs_bind_group,
                                layer as u32 * stride,
                                &face_planes,
                            );
                            tally_entity_occluder_submissions(
                                &mut full.cube_entity_occluders_submitted,
                                Some(&mut full.promoted_entity_occluders_submitted),
                                submitted,
                            );
                        }
                        let submitted = full.rigid_occluder_depth.record_kinematic_movers(
                            &mut pass,
                            &full.kinematic_brush,
                            &full.mover_occluder_aabbs,
                            &full.cube_shadow_vs_bind_group,
                            layer as u32 * stride,
                            &face_planes,
                        );
                        tally_entity_occluder_submissions(
                            &mut full.cube_entity_occluders_submitted,
                            Some(&mut full.promoted_entity_occluders_submitted),
                            submitted,
                        );
                    }
                    continue;
                }

                if let Some(plan) = dynamic_cache_plan.cube_for_slot(slot as u32) {
                    if plan.needs_world_render {
                        {
                            let cache_view = full.dynamic_depth_cache.cube_view(plan, face);
                            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                                label: Some("Dynamic Cube World Depth Cache Pass"),
                                color_attachments: &[],
                                depth_stencil_attachment: Some(
                                    wgpu::RenderPassDepthStencilAttachment {
                                        view: cache_view,
                                        depth_ops: Some(wgpu::Operations {
                                            load: wgpu::LoadOp::Clear(1.0),
                                            store: wgpu::StoreOp::Store,
                                        }),
                                        stencil_ops: None,
                                    },
                                ),
                                timestamp_writes: None,
                                ..Default::default()
                            });
                            if draw_world {
                                pass.set_pipeline(&full.shadow_depth_pipeline);
                                pass.set_bind_group(
                                    0,
                                    &full.cube_shadow_vs_bind_group,
                                    &[layer as u32 * stride],
                                );
                                pass.set_vertex_buffer(0, full.vertex_buffer.slice(..));
                                pass.set_index_buffer(
                                    full.index_buffer.slice(..),
                                    wgpu::IndexFormat::Uint32,
                                );
                                if let Some(cube_cull) = &full.cube_shadow_cull {
                                    cube_cull.draw_slot_indirect(&mut pass, layer as u32, None);
                                } else {
                                    pass.draw_indexed(0..full.index_count, 0, 0..1);
                                }
                            }
                        }
                        if face + 1 == crate::lighting::cube_shadow::CUBE_FACES {
                            full.dynamic_depth_cache
                                .state
                                .mark_cube_world_rendered(plan);
                        }
                    } else {
                        full.dynamic_depth_cache_diagnostics.frame.world_pass_skips += 1;
                    }

                    dynamic_depth_cache::copy_cached_world_depth(
                        encoder,
                        full.dynamic_depth_cache
                            .cube_texture
                            .as_ref()
                            .expect("dynamic cube cache allocated"),
                        dynamic_depth_cache::DynamicDepthCache::cube_face_layer(plan, face) as u32,
                        &pool.array_texture,
                        layer as u32,
                        crate::lighting::cube_shadow::CUBE_FACE_RESOLUTION,
                    );
                    let view = &pool.face_views[layer];
                    let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                        label: Some("Dynamic Cube Entity Shadow Depth Pass"),
                        color_attachments: &[],
                        depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                            view,
                            depth_ops: Some(wgpu::Operations {
                                load: wgpu::LoadOp::Load,
                                store: wgpu::StoreOp::Store,
                            }),
                            stencil_ops: None,
                        }),
                        timestamp_writes: None,
                        ..Default::default()
                    });
                    if pool.slot_entity_eligible[slot] {
                        let face_planes =
                            postretro_render_data::cone_frustum::cone_frustum_planes(&face_matrix);
                        if let Some(mesh_plan) = &mesh_frame_plan {
                            let submitted = full.mesh_pass.record_skinned_depth(
                                &mut pass,
                                mesh_plan,
                                MeshDepthInstanceFilter::DynamicCasters,
                                &full.cube_shadow_vs_bind_group,
                                layer as u32 * stride,
                                &face_planes,
                            );
                            tally_entity_occluder_submissions(
                                &mut full.cube_entity_occluders_submitted,
                                None,
                                submitted,
                            );
                        }
                        let submitted = full.rigid_occluder_depth.record_kinematic_movers(
                            &mut pass,
                            &full.kinematic_brush,
                            &full.mover_occluder_aabbs,
                            &full.cube_shadow_vs_bind_group,
                            layer as u32 * stride,
                            &face_planes,
                        );
                        tally_entity_occluder_submissions(
                            &mut full.cube_entity_occluders_submitted,
                            None,
                            submitted,
                        );
                    }
                    continue;
                }

                let view = &pool.face_views[layer];
                let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                    label: Some("Cube Shadow Depth Pass"),
                    color_attachments: &[],
                    depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                        view,
                        depth_ops: Some(wgpu::Operations {
                            load: wgpu::LoadOp::Clear(1.0),
                            store: wgpu::StoreOp::Store,
                        }),
                        stencil_ops: None,
                    }),
                    timestamp_writes: None,
                    ..Default::default()
                });

                // WORLD geometry into this face, so static occluders (crates,
                // pillars) shadow under dynamic point lights exactly as they do
                // under pooled dynamic spots. Same depth-only pipeline; the
                // face's light-space matrix is selected via the cube VS
                // uniform's dynamic offset. Indirect from this layer's culled
                // sub-region; full unconditional draw when no BVH cull exists
                // (no-BVH maps), matching the spot fallback.
                if draw_world {
                    pass.set_pipeline(&full.shadow_depth_pipeline);
                    pass.set_bind_group(
                        0,
                        &full.cube_shadow_vs_bind_group,
                        &[layer as u32 * stride],
                    );
                    pass.set_vertex_buffer(0, full.vertex_buffer.slice(..));
                    pass.set_index_buffer(full.index_buffer.slice(..), wgpu::IndexFormat::Uint32);
                    if let Some(cube_cull) = &full.cube_shadow_cull {
                        cube_cull.draw_slot_indirect(&mut pass, layer as u32, None);
                    } else {
                        pass.draw_indexed(0..full.index_count, 0, 0..1);
                    }
                }

                // Entity occluders into the SAME face, gated on the
                // slot's entity eligibility (the per-light
                // `casts_entity_shadows` toggle) — the same occluder split as
                // the spot path: an ineligible slot keeps its world shadow but
                // draws zero entity occluders. With neither draw the face still
                // holds its Clear(1.0) baseline, so an occluder-free cube reads
                // as fully lit (shadow factor 1.0).
                if pool.slot_entity_eligible[slot] {
                    // Face frustum planes from the same matrix uploaded to the cube
                    // VS uniform buffer — one source of truth for cull + projection.
                    let face_planes =
                        postretro_render_data::cone_frustum::cone_frustum_planes(&face_matrix);
                    if let Some(plan) = &mesh_frame_plan {
                        let submitted = full.mesh_pass.record_skinned_depth(
                            &mut pass,
                            plan,
                            MeshDepthInstanceFilter::DynamicCasters,
                            &full.cube_shadow_vs_bind_group,
                            layer as u32 * stride,
                            &face_planes,
                        );
                        tally_entity_occluder_submissions(
                            &mut full.cube_entity_occluders_submitted,
                            None,
                            submitted,
                        );
                    }
                    let submitted = full.rigid_occluder_depth.record_kinematic_movers(
                        &mut pass,
                        &full.kinematic_brush,
                        &full.mover_occluder_aabbs,
                        &full.cube_shadow_vs_bind_group,
                        layer as u32 * stride,
                        &face_planes,
                    );
                    tally_entity_occluder_submissions(
                        &mut full.cube_entity_occluders_submitted,
                        None,
                        submitted,
                    );
                }
            }
        }
        if !dynamic_cache_plan.cube().is_empty() {
            if let Some(timing) = &full.frame_timing {
                timing.write_encoder_end(encoder, TIMING_PAIR_DYNAMIC_CUBE_DEPTH);
            }
        }
        full.dynamic_depth_cache_diagnostics
            .finish_frame(full.frame_timing.is_some());
        // Close the coarse promoted-depth-cache timing pair opened in either shadow
        // loop. The span is an upper bound over promoted work (see the spot
        // open site) — interleaved dynamic-shadow work is attributed here too.
        if full.promoted_depth_cache_timing_open {
            if let Some(timing) = &full.frame_timing {
                timing.write_encoder_end(encoder, TIMING_PAIR_PROMOTED_DEPTH_CACHE);
            }
            full.promoted_depth_cache_timing_open = false;
        }
    }
}
