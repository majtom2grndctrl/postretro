// Pre-scene cull and compose orchestration before depth and forward passes.
// See: context/lib/rendering_pipeline.md §7.1

use super::*;

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
    pub(super) fn prepare_streamed_sh_compose(
        &mut self,
        region_sets: ShSampleRegionSets<'_>,
        fog_draw_all: bool,
        records_compose: bool,
    ) -> std::result::Result<(), ShResidencyDrainError> {
        #[cfg(feature = "dev-tools")]
        let promotion_override = self.full().direct_sh_debug_override;
        #[cfg(not(feature = "dev-tools"))]
        let promotion_override = DirectShDebugOverride::default();
        #[cfg(feature = "dev-tools")]
        let animated_override = self.full().animated_direct_sh_debug_override;
        #[cfg(not(feature = "dev-tools"))]
        let animated_override = AnimatedDirectShDebugOverride::default();

        let frame_light_term_mask = self.frame_light_term_mask();
        let full = self.full_mut();
        let indirect_active = full.sh_streaming.as_ref().is_some_and(|streaming| {
            streaming.indirect_has_active_animation(&full.sh_volume_resources.animation)
        });
        let animated_direct_active = full.sh_streaming.as_ref().is_some_and(|streaming| {
            streaming.direct_has_active_animation(&full.sh_volume_resources.animation)
        }) || animated_override.active();
        let force_full_resident = full.force_full_resident_sh_compose;
        let Some(streaming) = full.sh_streaming.as_mut() else {
            return Ok(());
        };
        streaming.prepare_compose_frame(
            region_sets,
            fog_draw_all,
            records_compose,
            force_full_resident,
            indirect_active,
            animated_direct_active,
            frame_light_term_mask,
            promotion_override,
            animated_override,
            &full.promoted_static_weights,
            &full.promoted_animated_states,
        )
    }

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
                            "[Renderer] candidate cull: visible cell id {} out of \\
                             CellDrawIndex range ({} cells); using whole-BVH tree walk \\
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
}

impl Renderer {
    /// Direct SH and billboard-scatter composition encoded after dynamic
    /// light-slot assignment but before any scene draw. Slot assignment writes
    /// the promotion state these compose passes consume, so this intentionally
    /// remains separate from `record_pre_scene_compute` above.
    pub(super) fn record_direct_sh_pre_scene_compute(
        &mut self,
        encoder: &mut wgpu::CommandEncoder,
    ) -> bool {
        #[cfg(feature = "dev-tools")]
        let direct_sh_debug_override = self.full().direct_sh_debug_override;
        #[cfg(not(feature = "dev-tools"))]
        let direct_sh_debug_override = DirectShDebugOverride::default();
        #[cfg(feature = "dev-tools")]
        let animated_direct_sh_debug_override = self.full().animated_direct_sh_debug_override;
        #[cfg(not(feature = "dev-tools"))]
        let animated_direct_sh_debug_override = AnimatedDirectShDebugOverride::default();
        let full = self.full();
        let streamed_direct_animation_active = full.sh_streaming.as_ref().is_some_and(|state| {
            state.direct_has_active_animation(&full.sh_volume_resources.animation)
        });
        let direct_sh_active = full
            .promoted_static_weights
            .iter()
            .any(|weight| *weight > 0.0)
            || full
                .promoted_animated_states
                .iter()
                .any(|state| state.weight > 0.0)
            || if full.sh_streaming.is_some() {
                streamed_direct_animation_active
            } else {
                full.sh_volume_resources
                    .direct
                    .has_active_animated_descriptor(&full.sh_volume_resources.animation)
            }
            || direct_sh_debug_override.active()
            || animated_direct_sh_debug_override.active();
        // This intentionally keys on descriptor activity rather than the
        // curve's current evaluated scale: an active zero-valued curve
        // still needs composition for a later nonzero sample.
        let billboard_direct_scatter_active = full
            .sh_volume_resources
            .billboard_direct_scatter
            .has_active_animated_descriptor(&full.sh_volume_resources.animation);
        let frame_light_term_mask = self.frame_light_term_mask();
        let Self { queue, full, .. } = self;
        let full = full
            .as_mut()
            .expect("renderer full-init must complete before full-ready paths run");
        let direct_sh_ts = full
            .frame_timing
            .as_ref()
            .map(|t| t.compute_pass_writes(TIMING_PAIR_DIRECT_SH_COMPOSE));
        let animated_direct_sh_ts = full
            .frame_timing
            .as_ref()
            .map(|t| t.compute_pass_writes(TIMING_PAIR_ANIMATED_DIRECT_SH_COMPOSE));
        let billboard_direct_scatter_ts = full
            .frame_timing
            .as_ref()
            .map(|t| t.compute_pass_writes(TIMING_PAIR_BILLBOARD_DIRECT_SCATTER_COMPOSE));
        let compose_succeeded = if let Some(streaming) = full.sh_streaming.as_mut() {
            match streaming.dispatch_direct_compose(
                queue,
                encoder,
                &full.uniform_bind_group,
                direct_sh_active,
                frame_light_term_mask,
                direct_sh_debug_override,
                animated_direct_sh_debug_override,
                &full.promoted_animated_states,
                direct_sh_ts,
                animated_direct_sh_ts,
            ) {
                Ok(()) => true,
                Err(error) => {
                    // Keep this frame's sampled mirror miss-safe and report
                    // that the app must not publish newly accepted clusters.
                    log::error!("[Renderer] streamed direct SH compose failed: {error}");
                    false
                }
            }
        } else {
            full.direct_sh_compose.dispatch_if_needed(
                queue,
                encoder,
                DirectShComposeFrameInputs {
                    uniform_bind_group: &full.uniform_bind_group,
                    active: direct_sh_active,
                    light_term_mask: frame_light_term_mask,
                    debug_overrides: DirectShComposeDebugOverrides {
                        promotion: direct_sh_debug_override,
                        animated: animated_direct_sh_debug_override,
                    },
                    animated_promotion_states: &full.promoted_animated_states,
                    timestamp_writes: DirectShComposeTimestampWrites {
                        promotion: direct_sh_ts,
                        animated: animated_direct_sh_ts,
                    },
                },
            );
            true
        };
        // Shares the already-flushed descriptor/sample buffers with animated
        // direct SH. This stays before every billboard draw, so its initial
        // copy-through is visible on the first frame.
        full.billboard_direct_scatter_compose.dispatch_if_needed(
            encoder,
            &full.uniform_bind_group,
            billboard_direct_scatter_active,
            frame_light_term_mask,
            billboard_direct_scatter_ts,
        );
        compose_succeeded
    }

    /// Pre-scene compute work encoded before any render pass: BVH/visibility cull,
    /// animated-lightmap compose, and SH compose. All write storage the forward
    /// pass later samples, so they precede the depth pre-pass.
    pub(super) fn record_pre_scene_compute(
        &mut self,
        encoder: &mut wgpu::CommandEncoder,
        cam_vis: CameraCullVisibility<'_>,
        view_proj: Mat4,
        render_world: bool,
        frame_light_term_mask: LightTermMask,
    ) -> bool {
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
                                "[Renderer] candidate cull: visible cell id {} out of \\
                                 CellDrawIndex range ({} cells); using whole-BVH tree walk \\
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
            let streamed_indirect_active = full.sh_streaming.as_ref().is_some_and(|streaming| {
                streaming.indirect_has_active_animation(&full.sh_volume_resources.animation)
            });
            if let Some(streaming) = full.sh_streaming.as_mut() {
                if let Err(error) = streaming.dispatch_indirect_compose(
                    queue,
                    encoder,
                    &full.uniform_bind_group,
                    streamed_indirect_active,
                    frame_light_term_mask,
                    sh_compose_ts,
                ) {
                    log::error!("[Renderer] streamed indirect SH compose failed: {error}");
                    return false;
                }
            } else {
                let indirect_active = full
                    .sh_compose
                    .has_active_animated_descriptor(&full.sh_volume_resources.animation);
                full.sh_compose.dispatch_if_needed(
                    encoder,
                    &full.uniform_bind_group,
                    indirect_active,
                    frame_light_term_mask,
                    sh_compose_ts,
                );
            }
        }
        true
    }
}
