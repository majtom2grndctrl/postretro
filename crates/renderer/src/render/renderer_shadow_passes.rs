// Per-frame renderer pass recording factored out of frame orchestration.
// See: context/lib/rendering_pipeline.md §7.1, §7.6

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

/// Whether a leaf AABB survives the frustum, mirroring `is_aabb_outside_frustum`
/// in both cull shaders (p-vertex test, inside-sign `dot(n, p) + d >= 0`).
/// `planes` come from `extract_frustum_planes_for_gpu` — the exact CPU source
/// the GPU uniform is serialized from — so the CPU diagnostics submitted count
/// matches what the GPU writes.
fn leaf_passes_frustum(
    leaf: &postretro_render_data::geometry::BvhLeaf,
    planes: &[[f32; 4]; 6],
) -> bool {
    for plane in planes {
        let n = Vec3::new(plane[0], plane[1], plane[2]);
        let d = plane[3];
        let p = Vec3::new(
            if n.x >= 0.0 {
                leaf.aabb_max[0]
            } else {
                leaf.aabb_min[0]
            },
            if n.y >= 0.0 {
                leaf.aabb_max[1]
            } else {
                leaf.aabb_min[1]
            },
            if n.z >= 0.0 {
                leaf.aabb_max[2]
            } else {
                leaf.aabb_min[2]
            },
        );
        if n.dot(p) + d < 0.0 {
            return false;
        }
    }
    true
}

/// CPU-derived submitted-leaf count for the tree walk: drawable leaves whose
/// cell is visible and whose AABB passes the frustum, over the whole leaf
/// array. Mirrors `bvh_cull.wgsl::cull_main`'s submit branch. Diagnostic only.
fn count_submitted_tree_walk(
    leaves: &[postretro_render_data::geometry::BvhLeaf],
    visible: &VisibleCells,
    view_proj: &Mat4,
) -> u32 {
    let planes = postretro_render_data::cone_frustum::extract_frustum_planes_for_gpu(view_proj);
    leaves
        .iter()
        .filter(|leaf| {
            // `!is_solid && face_count > 0` drawability is not checked here: non-drawable
            // cells' BVH leaves always have `index_count == 0`, so the early return above
            // already excludes them.
            if leaf.index_count == 0 {
                return false;
            }
            let cell_visible = match visible {
                VisibleCells::DrawAll => true,
                VisibleCells::Culled(cells) => cells.contains(&leaf.cell_id),
            };
            cell_visible && leaf_passes_frustum(leaf, &planes)
        })
        .count() as u32
}

/// CPU-derived submitted-leaf count for the candidate path: gathered candidate
/// leaves whose AABB passes the frustum. Candidate gather already applies the
/// visible-cell constraint, so this only mirrors the shader's frustum submit
/// branch. Diagnostic only.
fn count_submitted_candidates(
    leaves: &[postretro_render_data::geometry::BvhLeaf],
    candidate_leaves: &[u32],
    view_proj: &Mat4,
) -> u32 {
    let planes = postretro_render_data::cone_frustum::extract_frustum_planes_for_gpu(view_proj);
    candidate_leaves
        .iter()
        .filter_map(|&leaf| leaves.get(leaf as usize))
        .filter(|leaf| leaf.index_count != 0 && leaf_passes_frustum(leaf, &planes))
        .count() as u32
}

impl Renderer {
    /// Refresh dev-tools camera-cull diagnostics from the current frame's CPU
    /// visibility inputs before the debug UI reads them. The tree-walk baseline
    /// and candidate counts are both computed here so the Spatial tab does not
    /// mix current cell visibility with later render-pass diagnostics.
    #[cfg(feature = "dev-tools")]
    pub fn refresh_camera_cull_diagnostics(
        &mut self,
        cam_vis: CameraCullVisibility<'_>,
        view_proj: Mat4,
    ) {
        let visible: &VisibleCells = cam_vis.cells;
        let full = self.full_mut();
        let Some(total_leaves) = full.compute_cull.as_ref().map(|cull| cull.total_leaves()) else {
            full.camera_cull_diagnostics = CameraCullDiagnostics::default();
            full.bvh_cull_diagnostics = None;
            return;
        };
        full.bvh_cull_diagnostics = full
            .compute_cull
            .as_ref()
            .map(|cull| cull.estimate_diagnostics(visible, &view_proj));

        let candidate_counts = match (
            full.cell_draw_index.as_ref(),
            full.candidate_cull.as_mut(),
            visible,
            cam_vis.path,
        ) {
            (
                Some(index),
                Some(candidate),
                VisibleCells::Culled(cells),
                VisibilityPath::PrlPortal { .. },
            ) => match candidate.gather(index, cells) {
                crate::candidate_cull::GatherStatus::Ok => Some((
                    candidate.candidates().len() as u32,
                    count_submitted_candidates(
                        &full.bvh_leaves,
                        candidate.candidates(),
                        &view_proj,
                    ),
                )),
                crate::candidate_cull::GatherStatus::OutOfRange { cell_id } => {
                    if !full.candidate_cull_oor_logged {
                        log::warn!(
                            "[Renderer] candidate cull: visible cell id {} out of \
                             CellDrawIndex range ({} cells); using whole-BVH tree walk \
                             for this frame",
                            cell_id,
                            index.cell_count,
                        );
                        full.candidate_cull_oor_logged = true;
                    }
                    None
                }
            },
            _ => None,
        };

        full.camera_cull_diagnostics = if let Some((candidate_leaves, submitted_leaves)) =
            candidate_counts
        {
            CameraCullDiagnostics {
                path: CameraCullPath::Candidate { candidate_leaves },
                total_leaves,
                submitted_leaves,
            }
        } else {
            CameraCullDiagnostics {
                path: CameraCullPath::TreeWalk,
                total_leaves,
                submitted_leaves: count_submitted_tree_walk(&full.bvh_leaves, visible, &view_proj),
            }
        };
    }

    /// Spot-shadow depth loop: per occupied slot, render world geometry (indirect,
    /// cone-culled) then skinned-entity occluders into that slot's depth map.
    /// Caller gates on `render_world && self.has_geometry && self.index_count > 0`.
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
        let slot_assignment = full.spot_shadow_pool.slot_assignment.clone();
        let mut used_slots: Vec<u32> = slot_assignment
            .iter()
            .copied()
            .filter(|&s| s != crate::lighting::spot_shadow::NO_SHADOW_SLOT)
            .collect();
        used_slots.sort_unstable();
        used_slots.dedup();

        // Reset the per-frame entity-occluder counter; the per-slot cull
        // tallies into it below. Mirrors `shadow-cone-cull`'s submitted
        // counter — pure CPU, no GPU readback.
        full.spot_entity_occluders_submitted = 0;

        // Per-slot GPU cone cull: one compute pass loops the occupied slots,
        // dispatching BVH traversal into each slot's indirect sub-region
        // gated by that slot's cone frustum planes. Runs after the camera
        // BVH cull and before the per-slot depth render passes below, so the
        // sub-regions are populated when each slot draws indirect.
        if let Some(shadow_cull) = &full.shadow_cull {
            shadow_cull.dispatch_occupied_slots(
                queue,
                encoder,
                &full.spot_shadow_pool.slot_cone_matrices,
            );
        }

        for slot in used_slots {
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

            // Skinned ENTITY occluders into the SAME slot, through the
            // parameterized depth-only path: target view = this slot's depth
            // attachment (the `pass` above), light-space matrix = the
            // per-slot `shadow_vs_bind_group` + dynamic offset. This proves
            // the cube-ready contract — the pipeline takes the view + matrix
            // as per-render parameters, with no slot-count or 2D-target
            // assumption baked in. Reads the already-posed buffers from the
            // hoist (no rewrite since), so the occluder pose matches the
            // forward draw with no one-frame lag.
            //
            // TWO gates (kept separate from pool-slot eligibility):
            //   1. `slot_entity_eligible[slot]` — the slot's light passes
            //      `entity_occluder_eligible` (dynamic + toggle on). An
            //      ineligible slot keeps its world shadow (already drawn
            //      above) but draws ZERO entity occluders.
            //   2. per-instance cone cull inside `record_skinned_depth` —
            //      only instances whose transformed bound intersects this
            //      slot's cone are submitted.
            if let Some(plan) = &mesh_frame_plan {
                if full.spot_shadow_pool.slot_entity_eligible[slot as usize] {
                    if let Some(cone_matrix) =
                        full.spot_shadow_pool.slot_cone_matrices[slot as usize]
                    {
                        let cone_planes =
                            postretro_render_data::cone_frustum::cone_frustum_planes(&cone_matrix);
                        full.spot_entity_occluders_submitted +=
                            full.mesh_pass.record_skinned_depth(
                                &mut pass,
                                plan,
                                &full.shadow_vs_bind_group,
                                slot * stride,
                                &cone_planes,
                            );
                    }
                }
            }
        }
    }

    /// Cube point-light shadow depth loop: clear every occupied eligible face to
    /// the far plane and render skinned-entity occluders into it. Caller gates on
    /// `render_world`.
    pub(super) fn record_cube_shadow_depth(
        &mut self,
        encoder: &mut wgpu::CommandEncoder,
        mesh_frame_plan: Option<&mesh_instances::MeshFramePlan>,
    ) {
        let full = self.full_mut();
        if let Some(pool) = &full.cube_shadow_pool {
            let stride = full.shadow_vs_stride;
            for layer in 0..pool.face_matrices.len() {
                let face_matrix_opt = pool.face_matrices[layer];
                // Only occupied faces are touched; an occupied face ALWAYS gets
                // its Clear(1.0) far-plane baseline this frame, mesh plan or not
                // (the occluder draw below is the only mesh-plan-gated step). See
                // `cube_shadow::cube_face_needs_clear` for why the clear must not
                // be gated on the plan.
                if !crate::lighting::cube_shadow::cube_face_needs_clear(face_matrix_opt.is_some()) {
                    continue;
                }
                let face_matrix = face_matrix_opt.expect("face_needs_clear implies occupied");
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
                // Occluders are entity-only: submit skinned meshes ONLY when a
                // mesh frame plan exists. With no plan the face still receives its
                // Clear(1.0) far-plane baseline above, so an occluder-free eligible
                // cube reads as fully lit (shadow factor 1.0) — matching the spot
                // path and the off-camera (no-slot) path.
                if let Some(plan) = &mesh_frame_plan {
                    // Face frustum planes from the same matrix uploaded to the cube
                    // VS uniform buffer — one source of truth for cull + projection.
                    let face_planes =
                        postretro_render_data::cone_frustum::cone_frustum_planes(&face_matrix);
                    full.cube_entity_occluders_submitted += full.mesh_pass.record_skinned_depth(
                        &mut pass,
                        plan,
                        &full.cube_shadow_vs_bind_group,
                        layer as u32 * stride,
                        &face_planes,
                    );
                }
            }
        }
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

impl Renderer {
    /// Pre-scene compute work encoded before any render pass: BVH/visibility cull,
    /// animated-lightmap compose, and SH compose. All write storage the forward
    /// pass later samples, so they precede the depth pre-pass.
    pub(super) fn record_pre_scene_compute(
        &mut self,
        encoder: &mut wgpu::CommandEncoder,
        cam_vis: CameraCullVisibility<'_>,
        view_proj: Mat4,
        render_world: bool,
    ) {
        let visible: &VisibleCells = cam_vis.cells;
        let Self {
            device,
            queue,
            full,
            ..
        } = self;
        let full = full
            .as_mut()
            .expect("renderer full-init must complete before full-ready paths run");

        // Same submission as render passes — no readback or GPU sync between cull and draw.
        if render_world {
            // Keep the pre-UI tree-walk baseline mirrored after pass recording
            // for non-egui diagnostic readers. This remains independent of the
            // active GPU cull strategy, so candidate frames never starve the
            // baseline to zero.
            #[cfg(feature = "dev-tools")]
            {
                full.bvh_cull_diagnostics = full
                    .compute_cull
                    .as_ref()
                    .map(|cull| cull.estimate_diagnostics(visible, &view_proj));
            }

            // Candidate-cull routing. Eligible iff ALL hold:
            //   * a valid loaded `CellDrawIndex`,
            //   * `VisibleCells::Culled` (a concrete visible-cell set), AND
            //   * portal-traversal provenance (`VisibilityPath::PrlPortal`).
            // The gather may still bail to the tree walk for THIS frame if a
            // visible cell id is out of the index's range. DrawAll and
            // non-portal Culled fallbacks also route to the unchanged tree walk.
            // `None` for the installed index means no installed level, an empty
            // BVH map, or released resources; missing or invalid required PRL
            // indexes fail at load time. Gathered into the pipeline's reused
            // scratch (no per-frame allocation): `cell_draw_index` borrowed
            // immutably and `candidate_cull` mutably — disjoint fields. The
            // returned flag only signals readiness; the gathered leaves live in
            // the pipeline (`candidate.candidates()`), read after this borrow
            // ends in the dispatch match below.
            let candidates_ready: bool = match (
                full.cell_draw_index.as_ref(),
                full.candidate_cull.as_mut(),
                visible,
                cam_vis.path,
            ) {
                (
                    Some(index),
                    Some(candidate),
                    VisibleCells::Culled(cells),
                    VisibilityPath::PrlPortal { .. },
                ) => match candidate.gather(index, cells) {
                    crate::candidate_cull::GatherStatus::Ok => true,
                    crate::candidate_cull::GatherStatus::OutOfRange { cell_id } => {
                        if !full.candidate_cull_oor_logged {
                            log::warn!(
                                "[Renderer] candidate cull: visible cell id {} out of \
                                 CellDrawIndex range ({} cells); using whole-BVH tree walk \
                                 for this frame",
                                cell_id,
                                index.cell_count,
                            );
                            full.candidate_cull_oor_logged = true;
                        }
                        false
                    }
                },
                _ => false,
            };

            let cull_ts = full
                .frame_timing
                .as_ref()
                .map(|t| t.compute_pass_writes(TIMING_PAIR_CULL));

            // Single dispatch selection, consuming `cull_ts` (not `Copy`) in
            // exactly one arm. The candidate arm uses disjoint-field borrows:
            // `compute_cull` immutably (for its shared BVH leaf/indirect/status
            // buffer accessors) and `candidate_cull` mutably — distinct struct
            // fields, so both are live at once. The candidate path writes the
            // SAME global indirect/status slots as the tree-walk fallback arm.
            match (
                candidates_ready,
                full.compute_cull.as_ref(),
                full.candidate_cull.as_mut(),
            ) {
                (true, Some(cull), Some(candidate)) => {
                    // CPU-derived Spatial diagnostics: candidate count vs total
                    // BVH leaves, and submitted = candidates passing the frustum
                    // predicate. The gathered leaves live in the pipeline scratch
                    // (`candidate.candidates()`); read immutably here before the
                    // mutable `dispatch` borrow below.
                    let candidates = candidate.candidates();
                    let submitted_leaves =
                        count_submitted_candidates(&full.bvh_leaves, candidates, &view_proj);
                    full.camera_cull_diagnostics = CameraCullDiagnostics {
                        path: CameraCullPath::Candidate {
                            candidate_leaves: candidates.len() as u32,
                        },
                        total_leaves: cull.total_leaves(),
                        submitted_leaves,
                    };
                    candidate.dispatch(
                        device,
                        queue,
                        encoder,
                        cull.leaf_buffer(),
                        cull.indirect_buffer(),
                        cull.cull_status_buffer(),
                        &view_proj,
                        cull_ts,
                    );
                }
                // Tree-walk fallback (DrawAll, non-portal Culled, out-of-range
                // cell id, no installed level/empty BVH/released resources, or
                // no candidate pipeline).
                _ => {
                    if let Some(cull) = &mut full.compute_cull {
                        cull.dispatch(device, queue, encoder, visible, &view_proj, cull_ts);
                    }
                    // Tree-walk diagnostics: submitted = drawable, visible-cell,
                    // frustum-passing leaves over the WHOLE leaf array.
                    if let Some(cull) = full.compute_cull.as_ref() {
                        full.camera_cull_diagnostics = CameraCullDiagnostics {
                            path: CameraCullPath::TreeWalk,
                            total_leaves: cull.total_leaves(),
                            submitted_leaves: count_submitted_tree_walk(
                                &full.bvh_leaves,
                                visible,
                                &view_proj,
                            ),
                        };
                    }
                }
            }

            if let Some(cull) = &full.compute_cull {
                if log::log_enabled!(log::Level::Debug) {
                    let f = full.debug_frame;

                    let bm = cull.debug_bitmask_fingerprint();
                    if bm != full.debug_prev_bitmask {
                        log::debug!(
                            "[cull f={f}] visible-cell bitmask changed: pop={} hash={:#010x} (was pop={} hash={:#010x})",
                            bm.0,
                            bm.1,
                            full.debug_prev_bitmask.0,
                            full.debug_prev_bitmask.1,
                        );
                        full.debug_prev_bitmask = bm;
                    }

                    let mut vp_hash = 0u32;
                    for i in 0..4 {
                        let col = view_proj.col(i);
                        vp_hash ^= col.x.to_bits();
                        vp_hash ^= col.y.to_bits().rotate_left(7);
                        vp_hash ^= col.z.to_bits().rotate_left(13);
                        vp_hash ^= col.w.to_bits().rotate_left(19);
                    }
                    if vp_hash != full.debug_prev_vp_hash {
                        log::debug!("[cull f={f}] view_proj changed: hash={:#010x}", vp_hash);
                        full.debug_prev_vp_hash = vp_hash;
                    }

                    let cur_vis = match visible {
                        VisibleCells::Culled(cells) => ("Culled", cells.len()),
                        VisibleCells::DrawAll => ("DrawAll", 0),
                    };
                    if cur_vis != full.debug_prev_visible {
                        log::debug!(
                            "[cull f={f}] VisibleCells changed: {}(n={}) (was {}(n={}))",
                            cur_vis.0,
                            cur_vis.1,
                            full.debug_prev_visible.0,
                            full.debug_prev_visible.1,
                        );
                        full.debug_prev_visible = cur_vis;
                    }
                }
            }
        }

        // Before depth pre-pass: storage→sampled barrier must resolve before forward sampling.
        if render_world && full.animated_lightmap.is_active() {
            let animated_ts = full
                .frame_timing
                .as_ref()
                .map(|t| t.compute_pass_writes(TIMING_PAIR_ANIMATED_LM_COMPOSE));
            full.animated_lightmap.dispatch(
                queue,
                encoder,
                &full.uniform_bind_group,
                visible,
                animated_ts,
            );
        }

        // Before depth pre-pass: storage-write → sampled-read barrier for SH.
        if render_world {
            let sh_compose_ts = full
                .frame_timing
                .as_ref()
                .map(|t| t.compute_pass_writes(TIMING_PAIR_SH_COMPOSE));
            full.sh_compose
                .dispatch(encoder, &full.uniform_bind_group, sh_compose_ts);
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
