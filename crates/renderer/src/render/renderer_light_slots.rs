// Runtime direct-light slot assignment, spot/cube shadow-pool packing, and the
// shadow-debug trace.
// See: context/lib/rendering_pipeline.md §4

use super::renderer_types::{
    FullRenderer, MAX_PROMOTED_CUBE, MAX_PROMOTED_SPOT, PromotedShadowPoolKind,
    PromotedStaticLightRecord,
};
use super::*;
use postretro_render_cpu::frame_uniforms::TOTAL_LIGHT_COUNT_OFFSET;

impl Renderer {
    /// Sub-0.01 lights excluded from slot ranking — animated-dark lights don't waste a shadow slot.
    /// Short/empty `effective_brightness` = all-1.0 (first frame runs before bridge).
    ///
    /// `reachable_cell_aabbs` are the AABBs of the fog/light-reachable cells —
    /// the WIDER portal-reachable set (same source as `light_reachable_cell_mask`)
    /// that deliberately includes empty `face_count == 0` cells, NOT the narrower
    /// drawable `VisibleCells` set. A light is shadow-eligible when its runtime
    /// `LightInfluence` sphere reaches one of these reachable cells — NOT when
    /// the light's OWN cell is in the camera PVS, and NOT from origin+range
    /// reconstruction. Empty = DrawAll sentinel (fallback visibility paths):
    /// every cell-assigned light stays eligible.
    ///
    /// The **candidate set** is `self.shadow_candidate_lights`: dynamic-tier
    /// lights plus compiler-selected static lights eligible for entity-shadow
    /// promotion. `effective_brightness` is keyed on the dynamic-tier
    /// `level_lights` array, so dynamic candidate brightness is translated
    /// through the original full level-light index.
    pub fn update_dynamic_light_slots(
        &mut self,
        camera_position: Vec3,
        camera_near_clip: f32,
        effective_brightness: &[f32],
        reachable_cell_aabbs: &[(Vec3, Vec3)],
        now_seconds: f64,
        promotion_mesh_frame_plan: Option<&mesh_instances::MeshFramePlan>,
    ) {
        // No runtime shadow candidates this frame. Clear both pools' occupancy:
        // a stale `Some` cone/face matrix carried over from a previous level
        // would keep its depth passes — full world rasterizations — running
        // every frame against slots no light samples.
        if self.full().shadow_candidate_lights.is_empty() {
            let Self { queue, full, .. } = self;
            let full = full
                .as_mut()
                .expect("renderer full-init must complete before full-ready paths run");
            full.spot_shadow_pool.clear_occupancy();
            if let Some(pool) = &mut full.cube_shadow_pool {
                pool.clear_occupancy();
            }
            full.promoted_static_records.clear();
            full.promoted_static_cache_layers.clear();
            full.promoted_static_weights.fill(0.0);
            full.total_light_count = full.light_count;
            queue.write_buffer(
                &full.uniform_buffer,
                TOTAL_LIGHT_COUNT_OFFSET,
                &full.total_light_count.to_ne_bytes(),
            );
            full.dynamic_depth_cache.state.reset_level();
            full.dynamic_depth_cache_frame_plan = DynamicDepthCachePlan::default();
            queue.write_buffer(
                &full.dynamic_depth_cache.spot_layers_buffer,
                0,
                bytemuck::cast_slice(&[-1i32; crate::lighting::spot_shadow::SHADOW_POOL_SIZE]),
            );
            queue.write_buffer(
                &full.dynamic_depth_cache.cube_layers_buffer,
                0,
                bytemuck::cast_slice(&dynamic_depth_cache::cube_layer_channel(
                    &DynamicDepthCachePlan::default(),
                )),
            );
            return;
        }

        // Shadow-slot eligibility: a light is eligible when its runtime influence
        // volume reaches a fog/light-reachable cell (`reachable_cell_aabbs` =
        // AABBs of the WIDER portal-reachable set, including empty
        // `face_count == 0` cells) — NOT when the light's own cell is in the
        // camera PVS. The light is a shadow caster (onto receivers the camera
        // sees); like a world occluder (`shadow_cull.rs`) it need not sit in the
        // camera PVS itself. The prior own-cell-PVS gate dropped a light whose
        // cell left the shrinking PVS on pitch-down even though it still lit and
        // shadowed geometry in view, so entity shadows vanished.
        //
        // Empty `reachable_cell_aabbs` = DrawAll sentinel (fallback visibility
        // paths) → all cell-assigned lights eligible. ALPHA_LIGHT_LEAF_UNASSIGNED
        // = degenerate (couldn't assign to a non-solid cell) → always cull.
        const BRIGHTNESS_SUPPRESSION_THRESHOLD: f32 = 0.01;
        let mut visible_lights = vec![false; self.full().shadow_candidate_lights.len()];
        {
            let full = self.full();
            // Fixed within the frame — build the source→level reverse lookup once
            // so each candidate's brightness read is O(1) instead of scanning
            // `level_light_source_indices`.
            let level_light_index_lookup =
                build_level_light_index_lookup(&full.level_light_source_indices);
            for (i, light) in full.shadow_candidate_lights.iter().enumerate() {
                let reaches_view = shadow_candidate_reaches_visible_cell(
                    light,
                    full.shadow_candidate_influences.get(i),
                    reachable_cell_aabbs,
                );
                if !reaches_view {
                    continue;
                }
                if full
                    .shadow_candidate_selection_indices
                    .get(i)
                    .and_then(|selection| *selection)
                    .is_some()
                {
                    let has_shadow_mesh = selected_static_light_has_shadow_entity(
                        light,
                        full.shadow_candidate_influences.get(i),
                        promotion_mesh_frame_plan,
                    );
                    let has_shadow_mover = selected_static_light_has_mover_occluder(
                        light,
                        full.shadow_candidate_influences.get(i),
                        &full.mover_occluder_aabbs,
                    );
                    if !has_shadow_mesh && !has_shadow_mover {
                        continue;
                    }
                }
                // Brightness suppression is indexed by `level_lights` (the
                // forward / scripted-bridge index space). For candidates not in
                // `level_lights` we have no per-frame brightness — treat as 1.0.
                let b = full
                    .shadow_candidate_source_indices
                    .get(i)
                    .and_then(|&source_index| {
                        level_brightness_for_candidate_indexed(
                            &level_light_index_lookup,
                            source_index,
                            effective_brightness,
                        )
                    })
                    .unwrap_or(1.0);
                if b < BRIGHTNESS_SUPPRESSION_THRESHOLD {
                    continue;
                }
                visible_lights[i] = true;
            }
        }

        let mut slot_assignment = assign_shadow_pool_slots_with_promoted_static(
            self.full(),
            PromotedShadowPoolKind::Spot,
            camera_position,
            camera_near_clip,
            &visible_lights,
            crate::lighting::spot_shadow::SHADOW_POOL_SIZE,
            MAX_PROMOTED_SPOT,
        );

        // Rank dynamic POINT lights into the cube pool and upload their per-face
        // matrices. Returns the candidate-indexed cube slot assignment (empty
        // when the pool is disabled), which is patched into the light buffer
        // below alongside the spot slots. Runs before the patch block so both
        // slot fields land in one upload.
        let stride = self.full().shadow_vs_stride as usize;
        let mut cube_slot_assignment = self.update_cube_light_slots(
            camera_position,
            camera_near_clip,
            &visible_lights,
            stride,
        );

        let frame_dt = {
            let full = self.full_mut();
            let previous = full.promoted_static_last_update_time.replace(now_seconds);
            previous
                .map(|t| (now_seconds - t).clamp(0.0, 0.25) as f32)
                .unwrap_or(1.0 / 60.0)
        };
        self.update_promoted_static_weights_and_records(
            &slot_assignment,
            &cube_slot_assignment,
            &visible_lights,
            camera_position,
            camera_near_clip,
            frame_dt,
        );
        {
            let full = self.full();
            clear_zero_weight_promoted_assignments(
                &full.shadow_candidate_selection_indices,
                &full.promoted_static_weights,
                &mut slot_assignment,
            );
            clear_zero_weight_promoted_assignments(
                &full.shadow_candidate_selection_indices,
                &full.promoted_static_weights,
                &mut cube_slot_assignment,
            );
        }
        if !cube_slot_assignment.is_empty() {
            if let Some(pool) = self.full_mut().cube_shadow_pool.as_mut() {
                let mut live_slots = [false; crate::lighting::cube_shadow::CUBE_COUNT];
                for &slot in &cube_slot_assignment {
                    if slot != postretro_lighting::NO_SHADOW_SLOT {
                        if let Some(live) = live_slots.get_mut(slot as usize) {
                            *live = true;
                        }
                    }
                }
                for (slot, live) in live_slots.iter().copied().enumerate() {
                    if live {
                        continue;
                    }
                    pool.slot_entity_eligible[slot] = false;
                    for face in 0..crate::lighting::cube_shadow::CUBE_FACES {
                        let layer = crate::lighting::cube_shadow::CubeShadowPool::face_layer(
                            slot as u32,
                            face,
                        );
                        pool.face_matrices[layer] = None;
                    }
                }
                pool.slot_assignment = cube_slot_assignment.clone();
            }
        }

        // The GPU lights buffer is keyed on `level_lights`. Translate slot
        // assignments from candidate-index space into `level_lights`-index
        // space via each light's original full level-light index. This keeps
        // duplicate dynamic lights with identical origin/type independent.
        let level_slots = slot_assignment_for_level_lights(
            &self.full().level_light_source_indices,
            &self.full().shadow_candidate_source_indices,
            &slot_assignment,
        );
        let level_cube_slots = if cube_slot_assignment.is_empty() {
            vec![crate::lighting::spot_shadow::NO_SHADOW_SLOT; self.full().level_lights.len()]
        } else {
            slot_assignment_for_level_lights(
                &self.full().level_light_source_indices,
                &self.full().shadow_candidate_source_indices,
                &cube_slot_assignment,
            )
        };

        // Patch the per-light spot AND cube shadow-slot fields onto the CPU
        // mirror of the light buffer, then re-upload only if a slot changed. The
        // mirror holds whatever was last uploaded — the animated bridge's base
        // bytes once it has run, otherwise this fn's static pack. Patching
        // (rather than re-packing static `level_lights`) is what lets the slots
        // and the bridge's animated base data coexist: the two writers share one
        // buffer, so a full re-pack here would clobber the animation, and the
        // bridge's sentinel slot would clobber the shadow. The spot slot rides
        // `cone_angles_and_pad.z` and the cube slot rides `.w` — disjoint bytes,
        // so the two patches compose. See `upload_bridge_lights`.
        let Self { queue, full, .. } = self;
        let full = full
            .as_mut()
            .expect("renderer full-init must complete before full-ready paths run");
        let mut scratch = std::mem::take(&mut full.lights_pack_scratch);
        build_count_split_light_upload(full, &level_slots, &level_cube_slots, &mut scratch);
        if scratch != full.last_lights_upload {
            queue.write_buffer(&full.lights_buffer, 0, &scratch);
            full.last_lights_upload.clear();
            full.last_lights_upload.extend_from_slice(&scratch);
        }
        full.lights_pack_scratch = scratch;

        // The bridge owns the complete dynamic influence prefix, including
        // runtime-spawned lights. Promotion appends its static tail after the
        // current dynamic count. Gate the combined re-upload on promoted records;
        // without promotion the bridge's prefix upload is already complete.
        // The `entity_shadow_light_influences` vector is raw-length N and
        // index-parallel to the selection index, so `[selection_index]` is a
        // direct aligned lookup for every promoted record.
        if !full.promoted_static_records.is_empty() {
            debug_assert_eq!(
                full.entity_shadow_light_influences.len(),
                full.promoted_static_states.len(),
                "selected-static influence vector must be raw-length N, index-parallel to selection index",
            );
            let mut influence_bytes = std::mem::take(&mut full.influence_pack_scratch);
            influence_bytes.clear();
            let dynamic_bytes = full.light_count as usize * 16;
            if full.last_influence_upload.len() >= dynamic_bytes {
                influence_bytes.extend_from_slice(&full.last_influence_upload[..dynamic_bytes]);
            } else {
                influence::pack_influence_into(&mut influence_bytes, &full.level_light_influences);
                influence_bytes.resize(dynamic_bytes, 0);
            }
            for record in &full.promoted_static_records {
                let influence =
                    &full.entity_shadow_light_influences[record.selection_index as usize];
                influence::pack_influence_into(
                    &mut influence_bytes,
                    std::slice::from_ref(influence),
                );
            }
            shadowmask::pack_forward_shadowmask_metadata(
                &full.promoted_static_records,
                &full.promoted_static_cache_layers,
                &full.entity_shadow_spec_light_indices,
                &full.shadowmask_channels,
                full.shadowmask_present,
                &mut full.forward_shadowmask_metadata_scratch,
            );
            influence_bytes.extend_from_slice(&full.forward_shadowmask_metadata_scratch);
            if influence_bytes.is_empty() {
                influence_bytes.resize(16, 0);
            }
            queue.write_buffer(&full.influence_buffer, 0, &influence_bytes);
            full.influence_pack_scratch = influence_bytes;
        }
        queue.write_buffer(
            &full.uniform_buffer,
            TOTAL_LIGHT_COUNT_OFFSET,
            &full.total_light_count.to_ne_bytes(),
        );

        // Upload slot matrices to both fragment-side storage (group 5 binding 2)
        // and vertex-side dynamic-offset uniform buffer. Matrices come from
        // the candidate list — that's the index space `slot_assignment` is
        // keyed on.
        const MAT_BYTES: usize = 64;
        let mut fragment_matrices =
            vec![0u8; MAT_BYTES * crate::lighting::spot_shadow::SHADOW_POOL_SIZE];
        let mut vertex_uniforms =
            vec![0u8; stride * crate::lighting::spot_shadow::SHADOW_POOL_SIZE];
        // Reset the per-slot cone-matrix stash; reoccupied slots overwrite, the
        // rest stay `None` so the GPU cone cull skips them this frame. The
        // entity-occluder gate resets to `false` in lockstep.
        full.spot_shadow_pool.slot_cone_matrices =
            [None; crate::lighting::spot_shadow::SHADOW_POOL_SIZE];
        full.spot_shadow_pool.slot_entity_eligible =
            [false; crate::lighting::spot_shadow::SHADOW_POOL_SIZE];
        for (light_idx, &slot) in slot_assignment.iter().enumerate() {
            if slot == crate::lighting::spot_shadow::NO_SHADOW_SLOT {
                continue;
            }
            let candidate = &full.shadow_candidate_lights[light_idx];
            let m = postretro_lighting::light_space_matrix(candidate);
            // Stash the SAME light-space matrix uploaded to bind-group-5 below —
            // the shadow-depth render loop reads it to build this slot's cone
            // cull frustum planes (one source of truth, no recomputation).
            full.spot_shadow_pool.slot_cone_matrices[slot as usize] = Some(m);
            // Record whether this slot's occupant renders entity occluders. The
            // shadow-depth loop draws skinned meshes and rigid movers into the slot only when
            // this is set; an ineligible (e.g. toggle-off dynamic) slot keeps its
            // world shadow but draws none.
            let promoted_static = full
                .shadow_candidate_selection_indices
                .get(light_idx)
                .and_then(|selection| *selection)
                .is_some();
            full.spot_shadow_pool.slot_entity_eligible[slot as usize] =
                postretro_lighting::entity_occluder_eligible(candidate, promoted_static);
            let cols = m.to_cols_array();
            let mut bytes = [0u8; MAT_BYTES];
            for (i, v) in cols.iter().enumerate() {
                bytes[i * 4..i * 4 + 4].copy_from_slice(&v.to_ne_bytes());
            }
            let slot_usize = slot as usize;
            fragment_matrices[slot_usize * MAT_BYTES..(slot_usize + 1) * MAT_BYTES]
                .copy_from_slice(&bytes);
            vertex_uniforms[slot_usize * stride..slot_usize * stride + MAT_BYTES]
                .copy_from_slice(&bytes);
        }
        queue.write_buffer(
            &full.spot_shadow_pool.matrices_buffer,
            0,
            &fragment_matrices,
        );
        queue.write_buffer(&full.shadow_vs_uniform_buffer, 0, &vertex_uniforms);

        full.spot_shadow_pool.slot_assignment = slot_assignment;

        // Dynamic-tier cache planning follows the existing rank result, but its
        // key is the source identity plus the frozen projection — never the
        // pool slot. Build the per-slot channel from scratch every frame so a
        // retained layer cannot leak through a slot that was vacated or
        // re-tenanted this frame.
        let spot_inputs: Vec<_> = full
            .spot_shadow_pool
            .slot_assignment
            .iter()
            .enumerate()
            .filter_map(|(candidate_index, &slot)| {
                (slot != crate::lighting::spot_shadow::NO_SHADOW_SLOT
                    && full.shadow_candidate_selection_indices[candidate_index].is_none())
                .then(|| {
                    let matrix = full.spot_shadow_pool.slot_cone_matrices[slot as usize]?;
                    Some((
                        slot,
                        full.shadow_candidate_source_indices[candidate_index],
                        matrix,
                    ))
                })?
            })
            .collect();
        let cube_inputs: Vec<_> = full
            .cube_shadow_pool
            .as_ref()
            .map(|pool| {
                pool.slot_assignment
                    .iter()
                    .enumerate()
                    .filter_map(|(candidate_index, &slot)| {
                        (slot != crate::lighting::spot_shadow::NO_SHADOW_SLOT
                            && full.shadow_candidate_selection_indices[candidate_index].is_none())
                        .then(|| {
                            let mut matrices =
                                [Mat4::IDENTITY; crate::lighting::cube_shadow::CUBE_FACES];
                            for (face, matrix) in matrices.iter_mut().enumerate() {
                                let layer =
                                    crate::lighting::cube_shadow::CubeShadowPool::face_layer(
                                        slot, face,
                                    );
                                *matrix = pool.face_matrices[layer]?;
                            }
                            Some((
                                slot,
                                full.shadow_candidate_source_indices[candidate_index],
                                matrices,
                            ))
                        })?
                    })
                    .collect()
            })
            .unwrap_or_default();
        let plan = full
            .dynamic_depth_cache
            .state
            .plan_frame(&spot_inputs, &cube_inputs);
        let spot_layers = dynamic_depth_cache::spot_layer_channel(&plan);
        let cube_layers = dynamic_depth_cache::cube_layer_channel(&plan);
        queue.write_buffer(
            &full.dynamic_depth_cache.spot_layers_buffer,
            0,
            bytemuck::cast_slice(&spot_layers),
        );
        queue.write_buffer(
            &full.dynamic_depth_cache.cube_layers_buffer,
            0,
            bytemuck::cast_slice(&cube_layers),
        );
        full.dynamic_depth_cache_frame_plan = plan;
    }

    /// Env-gated shadow-pipeline diagnostics (`POSTRETRO_SHADOW_DEBUG=1`).
    ///
    /// READ-ONLY: logs the per-frame shadow decisions so a non-author can watch
    /// which one flips as the camera pitches down until an entity shadow vanishes.
    /// It changes no culling/selection state — it re-reads the values
    /// `update_dynamic_light_slots` just computed (the pool's `slot_assignment`,
    /// the candidate lights, the live `effective_brightness`, and the mesh
    /// visibility split) and renders them human-readable.
    ///
    /// Throttled: emits the full per-light table only when the decision
    /// fingerprint changes (spot/cube slot occupancy or the mesh visibility split),
    /// plus a heartbeat every ~120 frames, so normal play with the flag on still
    /// stays quiet between transitions. Off by default → zero overhead.
    ///
    /// Field guide (match these against the symptom):
    /// - `pitch` / `fwd` — camera look direction; `pitch` negative = looking down.
    /// - `cell` / `vis_cells` — camera cell + portal-reachable
    ///   (fog/light-reachable) cells, or `fallback_all_active` when the reachable
    ///   AABB list is empty and every cell-assigned light stays eligible.
    ///   `vis_cells` shrinking on pitch-down used to drop lights via the old
    ///   own-cell gate; the fix decouples eligibility from it (see `reach` below).
    /// - Per light `Lk`: `pos`, `range`, `dyn`, `cell`, `cell_ok` (legacy: its own
    ///   cell is in the portal-reachable set — NO LONGER the eligibility criterion,
    ///   kept for diagnosis), `reach` (THE criterion: its runtime
    ///   `LightInfluence` sphere reaches a fog/light-reachable cell), `bright`
    ///   (live animated brightness), `elig`
    ///   (passed
    ///   the reach+brightness gate feeding the spot/point shadow rankers), and
    ///   `slot` (assigned SPOT shadow slot or `NONE:<reason>`) plus `cube`
    ///   (assigned POINT cube shadow slot or `NONE:<reason>` — closes the prior
    ///   blind spot where point lights only ever showed `NONE:not_spot`). NOTE
    ///   these read the STATIC load-time `shadow_candidate_lights` — a scripted
    ///   sweep light's animated position/cone is NOT reflected here.
    /// - `casters`: `forward` vs `non_forward` collected mesh draw inputs.
    ///   `non_forward` includes explicit dynamic `shadowOnly` casters and the
    ///   broader promoted-static shadow-relevance set; `forward` is a mesh-pass
    ///   classification, not a raw PVS count.
    pub(super) fn emit_shadow_debug(
        &mut self,
        view_proj: Mat4,
        visible: &VisibleCells,
        light_reachable_cell_mask: &[bool],
        reachable_cell_aabbs: &[(Vec3, Vec3)],
        effective_brightness: &[f32],
        camera_cell: Option<u32>,
    ) {
        use crate::lighting::spot_shadow::NO_SHADOW_SLOT;
        use postretro_level_loader::LightType;

        const BRIGHTNESS_SUPPRESSION_THRESHOLD: f32 = 0.01;
        let f = self.full().debug_frame;

        // Camera forward = -Z row of the view matrix recovered from view_proj is
        // awkward; instead read the cached eye + derive a forward proxy from the
        // inverse view-projection (project a point down the -Z clip axis). Cheap
        // and only runs under the flag.
        let eye = self.full().last_camera_position;
        let inv = view_proj.inverse();
        let near_pt = inv.project_point3(glam::Vec3::new(0.0, 0.0, 0.0));
        let far_pt = inv.project_point3(glam::Vec3::new(0.0, 0.0, 1.0));
        let fwd = (far_pt - near_pt).normalize_or_zero();
        let pitch_deg = fwd.y.clamp(-1.0, 1.0).asin().to_degrees();

        let reachable_cell_count = light_reachable_cell_mask.iter().filter(|&&b| b).count();
        let vis_cells = if reachable_cell_aabbs.is_empty() {
            let mode = match visible {
                VisibleCells::DrawAll => "draw_all",
                VisibleCells::Culled(_) => "culled_empty",
            };
            format!("fallback_all_active({mode},mask={reachable_cell_count})")
        } else {
            reachable_cell_count.to_string()
        };

        // Per-candidate-light shadow status. Mirrors the eligibility logic in
        // `update_dynamic_light_slots` WITHOUT mutating anything — pure read.
        let mut slot_occupancy: u128 = 0;
        let mut cube_occupancy: u128 = 0;
        // Pool-saturation tallies (read-only): how many candidates passed the
        // reach/eligibility gate per pool, and how many of those were dropped
        // by ranking because the pool was full (the over-inclusion signal from
        // the looser shadow-eligibility gate, commit 3fef618).
        let mut elig_spot: usize = 0;
        let mut elig_cube: usize = 0;
        let mut spot_overflow: usize = 0;
        let mut cube_overflow: usize = 0;
        let mut light_lines: Vec<String> = Vec::new();
        // Fixed within the frame — build the source→level reverse lookup once so
        // the per-light brightness read is O(1).
        let level_light_index_lookup =
            build_level_light_index_lookup(&self.full().level_light_source_indices);
        for (i, light) in self.full().shadow_candidate_lights.iter().enumerate() {
            // Legacy own-cell-PVS membership (no longer the gate; kept so a reader
            // can SEE it diverge from `reach` — the whole point of the fix).
            let cell_ok = if light.cell_index == ALPHA_LIGHT_LEAF_UNASSIGNED {
                false
            } else if light_reachable_cell_mask.is_empty() {
                true
            } else {
                let cell = light.cell_index as usize;
                cell < light_reachable_cell_mask.len() && light_reachable_cell_mask[cell]
            };
            // THE eligibility criterion: runtime influence sphere reaches a
            // fog/light-reachable cell. Mirrors `update_dynamic_light_slots`
            // exactly (pure read).
            let reach = shadow_candidate_reaches_visible_cell(
                light,
                self.full().shadow_candidate_influences.get(i),
                reachable_cell_aabbs,
            );
            let bright = self
                .full()
                .shadow_candidate_source_indices
                .get(i)
                .and_then(|&source_index| {
                    level_brightness_for_candidate_indexed(
                        &level_light_index_lookup,
                        source_index,
                        effective_brightness,
                    )
                })
                .unwrap_or(1.0);
            let is_spot = light.light_type == LightType::Spot;
            let is_point = light.light_type == LightType::Point;
            let elig = reach && bright >= BRIGHTNESS_SUPPRESSION_THRESHOLD;

            // SPOT slot assigned to this candidate (slot_assignment is
            // candidate-indexed). Reason codes explain a NONE.
            let slot = self
                .full()
                .spot_shadow_pool
                .slot_assignment
                .get(i)
                .copied()
                .unwrap_or(NO_SHADOW_SLOT);
            let slot_str = if slot != NO_SHADOW_SLOT {
                if (slot as usize) < 128 {
                    slot_occupancy |= 1u128 << slot;
                }
                format!("slot={slot}")
            } else if !light.is_dynamic {
                "NONE:baked".to_string()
            } else if !is_spot {
                "NONE:not_spot".to_string()
            } else if !reach {
                "NONE:no_reach_to_view".to_string()
            } else if bright < BRIGHTNESS_SUPPRESSION_THRESHOLD {
                "NONE:dark".to_string()
            } else {
                spot_overflow += 1;
                "NONE:pool_overflow_or_unranked".to_string()
            };
            // A spot that passed the gate is an eligible candidate for the spot
            // pool whether or not it won a slot.
            if elig && is_spot {
                elig_spot += 1;
            }

            // CUBE (point-light) slot — closes the prior blind spot. The cube
            // pool's `slot_assignment` is candidate-indexed (same as spot).
            // `None` pool = adapter lacks CUBE_ARRAY_TEXTURES (point shadows off).
            let cube_str = match self.full().cube_shadow_pool.as_ref() {
                None if is_point => "NONE:cube_pool_off".to_string(),
                None => "NONE:not_point".to_string(),
                Some(pool) => {
                    let cslot = pool
                        .slot_assignment
                        .get(i)
                        .copied()
                        .unwrap_or(NO_SHADOW_SLOT);
                    if cslot != NO_SHADOW_SLOT {
                        if (cslot as usize) < 128 {
                            cube_occupancy |= 1u128 << cslot;
                        }
                        let promoted_static = self
                            .full()
                            .shadow_candidate_selection_indices
                            .get(i)
                            .and_then(|selection| *selection)
                            .is_some();
                        let ent_ok =
                            postretro_lighting::entity_occluder_eligible(light, promoted_static);
                        format!("cube={cslot}{}", if ent_ok { "" } else { "(no_ent)" })
                    } else if !light.is_dynamic {
                        "NONE:baked".to_string()
                    } else if !is_point {
                        "NONE:not_point".to_string()
                    } else if !reach {
                        "NONE:no_reach_to_view".to_string()
                    } else if bright < BRIGHTNESS_SUPPRESSION_THRESHOLD {
                        "NONE:dark".to_string()
                    } else {
                        cube_overflow += 1;
                        "NONE:pool_overflow_or_unranked".to_string()
                    }
                }
            };
            // A point light that passed the gate is an eligible candidate for the
            // cube pool (only counted when the pool exists; with the pool off the
            // reason is `cube_pool_off`, not overflow).
            if elig && is_point && self.full().cube_shadow_pool.is_some() {
                elig_cube += 1;
            }

            light_lines.push(format!(
                "L{i}[pos({:.0},{:.0},{:.0}) range={:.0} dyn={} ent={} cell={} cell_ok={} reach={} bright={:.2} elig={} {} {}]",
                light.origin[0],
                light.origin[1],
                light.origin[2],
                light.falloff_range,
                light.is_dynamic as u8,
                light.casts_entity_shadows as u8,
                light.cell_index as i64,
                cell_ok as u8,
                reach as u8,
                bright,
                elig as u8,
                slot_str,
                cube_str,
            ));
        }

        // Mesh presentation split. Non-forward inputs include explicit
        // shadow-only casters and the broader promoted-static relevance set;
        // `forward_visible` intentionally is not a raw PVS signal.
        let forward_visible = self
            .full()
            .mesh_draws
            .iter()
            .filter(|m| m.forward_visible)
            .count() as u32;
        let non_forward = self
            .full()
            .mesh_draws
            .iter()
            .filter(|m| !m.forward_visible)
            .count() as u32;

        // Throttle: emit on a decision change, plus a ~2s heartbeat. Spot and
        // cube occupancy are carried as separate fields (not XOR-folded) so a
        // POINT-light slot flip (the path that most likely casts the monster
        // shadows) always triggers a re-emit and can never XOR-cancel against a
        // simultaneous spot flip.
        let fingerprint = (slot_occupancy, cube_occupancy, forward_visible, non_forward);
        let heartbeat = f % 120 == 0;
        if fingerprint == self.full().shadow_debug_prev && !heartbeat {
            return;
        }
        let changed = fingerprint != self.full().shadow_debug_prev;
        self.full_mut().shadow_debug_prev = fingerprint;

        let cell_str = camera_cell
            .map(|l| l.to_string())
            .unwrap_or_else(|| "?".to_string());
        // Compact pool-saturation summary. `spot_overflow`/`cube_overflow` are
        // THE over-inclusion signal: > 0 means more lights cleared the reach gate
        // than the capped pool can shadow, so some were dropped by ranking.
        let spot_used = slot_occupancy.count_ones() as usize;
        let cube_used = cube_occupancy.count_ones() as usize;
        let cube_pool_size = if self.full().cube_shadow_pool.is_some() {
            crate::lighting::cube_shadow::CUBE_COUNT
        } else {
            0
        };
        log::info!(
            "[shadow_dbg f={f}{}] cam: pitch={:.1}deg fwd({:.2},{:.2},{:.2}) eye({:.0},{:.0},{:.0}) cell={cell_str} vis_cells={vis_cells} | pools: spot={spot_used}/{} cube={cube_used}/{cube_pool_size} elig_spot={elig_spot} elig_cube={elig_cube} spot_overflow={spot_overflow} cube_overflow={cube_overflow} | casters: forward={forward_visible} non_forward={non_forward} total={} | occupied_spot_slots={} occupied_cube_slots={} | lights[{}]: {}",
            if changed { " CHANGED" } else { " (hb)" },
            pitch_deg,
            fwd.x,
            fwd.y,
            fwd.z,
            eye.x,
            eye.y,
            eye.z,
            crate::lighting::spot_shadow::SHADOW_POOL_SIZE,
            self.full().mesh_draws.len(),
            slot_occupancy.count_ones(),
            cube_occupancy.count_ones(),
            light_lines.len(),
            light_lines.join(" "),
        );
    }

    /// Rank dynamic POINT lights into the cube pool and write each occupied
    /// slot's 6 per-face light-space matrices into the cube VS uniform buffer.
    /// Returns the candidate-indexed cube slot assignment so the caller can
    /// patch each point light's cube slot into the forward light buffer
    /// (`cone_angles_and_pad.w`). An EMPTY return means the pool is disabled
    /// (adapter lacks `CUBE_ARRAY_TEXTURES`) — every point light then keeps the
    /// sentinel and does unshadowed attenuation.
    ///
    /// Shares the spot path's per-light eligibility (`visible_lights`) and the
    /// SHARED scoring/drop ranking core, so cube and spot slot assignment cannot
    /// drift. Cube faces render WORLD geometry (per-face cone-culled, mirroring
    /// the spot depth pass) plus entity occluders; `slot_entity_eligible` gates
    /// only the entity draw, exactly like the spot path's per-slot entity gate.
    ///
    /// Every ranked slot's faces get matrices — occupancy is independent of
    /// `entity_occluder_eligible`. Each occupied face receives a Clear(1.0) +
    /// world-depth baseline every frame (the invariant that makes it safe for
    /// the shader to sample any ranked slot), so the returned assignment is the
    /// raw rank with no shader-facing masking.
    fn update_cube_light_slots(
        &mut self,
        camera_position: Vec3,
        camera_near_clip: f32,
        visible_lights: &[bool],
        stride: usize,
    ) -> Vec<u32> {
        use crate::lighting::cube_shadow;

        let Self { queue, full, .. } = self;
        let full = full
            .as_mut()
            .expect("renderer full-init must complete before full-ready paths run");

        if full.cube_shadow_pool.is_none() {
            return Vec::new();
        }

        let slot_assignment = assign_shadow_pool_slots_with_promoted_static(
            full,
            PromotedShadowPoolKind::Cube,
            camera_position,
            camera_near_clip,
            visible_lights,
            cube_shadow::CUBE_COUNT,
            MAX_PROMOTED_CUBE,
        );

        let pool = full
            .cube_shadow_pool
            .as_mut()
            .expect("cube pool presence checked above");

        // Reset per-face matrices + per-slot entity gate; reoccupied faces
        // overwrite, the rest stay `None`/`false` so the render loop skips them.
        let face_count = cube_shadow::CUBE_COUNT * cube_shadow::CUBE_FACES;
        for m in pool.face_matrices.iter_mut() {
            *m = None;
        }
        for e in pool.slot_entity_eligible.iter_mut() {
            *e = false;
        }

        let mut vertex_uniforms = vec![0u8; stride * face_count];
        for (light_idx, &slot) in slot_assignment.iter().enumerate() {
            if slot == crate::lighting::spot_shadow::NO_SHADOW_SLOT {
                continue;
            }
            let candidate = &full.shadow_candidate_lights[light_idx];
            // EVERY ranked slot gets face matrices: the depth loop clears each
            // occupied face to the far plane and renders cone-culled WORLD
            // geometry into it every frame (same Clear(1.0)+world baseline as
            // an occupied spot slot), so the shader may sample any ranked slot.
            // `slot_entity_eligible` gates only whether skinned ENTITY
            // occluders are additionally drawn into the faces — the same
            // occluder split as the spot path.
            let promoted_static = full
                .shadow_candidate_selection_indices
                .get(light_idx)
                .and_then(|selection| *selection)
                .is_some();
            pool.slot_entity_eligible[slot as usize] =
                postretro_lighting::entity_occluder_eligible(candidate, promoted_static);
            let face_mats = cube_shadow::cube_face_matrices(candidate);
            for (face, m) in face_mats.iter().enumerate() {
                let layer = cube_shadow::CubeShadowPool::face_layer(slot, face);
                pool.face_matrices[layer] = Some(*m);
                let cols = m.to_cols_array();
                let off = layer * stride;
                for (i, v) in cols.iter().enumerate() {
                    vertex_uniforms[off + i * 4..off + i * 4 + 4].copy_from_slice(&v.to_ne_bytes());
                }
            }
        }
        queue.write_buffer(&full.cube_shadow_vs_uniform_buffer, 0, &vertex_uniforms);

        pool.slot_assignment = slot_assignment.clone();
        // The raw rank IS the shader-facing assignment: every ranked slot's
        // faces carry a rendered world-depth baseline, so no masking is needed.
        slot_assignment
    }

    fn update_promoted_static_weights_and_records(
        &mut self,
        spot_assignment: &[u32],
        cube_assignment: &[u32],
        visible_lights: &[bool],
        camera_position: Vec3,
        camera_near_clip: f32,
        frame_dt: f32,
    ) {
        const PROMOTE_SECONDS: f32 = 0.3;
        const STICKY_SECONDS: f32 = 0.5;
        const DEMOTE_SECONDS: f32 = 0.3;

        let Self { queue, full, .. } = self;
        let full = full
            .as_mut()
            .expect("renderer full-init must complete before full-ready paths run");

        full.promoted_static_records.clear();
        if full.promoted_static_weights.len() != full.promoted_static_states.len() {
            full.promoted_static_weights
                .resize(full.promoted_static_states.len(), 0.0);
        }
        // The selected-static vectors are raw-length N and index-parallel to the
        // selection index, so every `[selection_index]` below is a direct aligned
        // lookup into the same N-length space the weight buffer and baked delta
        // `affinity_lights` use.
        debug_assert_eq!(
            full.entity_shadow_light_source_indices.len(),
            full.promoted_static_states.len(),
            "selected-static source-index vector must be raw-length N, index-parallel to selection index",
        );

        for (selection_index, state) in full.promoted_static_states.iter_mut().enumerate() {
            let candidate_index = full
                .shadow_candidate_selection_indices
                .iter()
                .position(|idx| *idx == Some(selection_index));

            let assigned = candidate_index.and_then(|candidate_index| {
                let spot = spot_assignment
                    .get(candidate_index)
                    .copied()
                    .unwrap_or(postretro_lighting::NO_SHADOW_SLOT);
                if spot != postretro_lighting::NO_SHADOW_SLOT {
                    return Some((PromotedShadowPoolKind::Spot, spot, candidate_index));
                }
                let cube = cube_assignment
                    .get(candidate_index)
                    .copied()
                    .unwrap_or(postretro_lighting::NO_SHADOW_SLOT);
                (cube != postretro_lighting::NO_SHADOW_SLOT).then_some((
                    PromotedShadowPoolKind::Cube,
                    cube,
                    candidate_index,
                ))
            });

            if let Some((pool_kind, slot, candidate_index)) = assigned {
                state.pool_kind = Some(pool_kind);
                state.slot = slot;
                if let Some(light) = full.shadow_candidate_lights.get(candidate_index) {
                    state.last_score =
                        candidate_slot_score(light, camera_position, camera_near_clip);
                }
                let gate_passed = visible_lights.get(candidate_index).copied().unwrap_or(true);
                if gate_passed {
                    state.sticky_remaining = STICKY_SECONDS;
                    state.weight = step_toward(state.weight, 1.0, frame_dt / PROMOTE_SECONDS);
                } else if state.sticky_remaining > 0.0 {
                    state.sticky_remaining = (state.sticky_remaining - frame_dt).max(0.0);
                } else {
                    state.weight = step_toward(state.weight, 0.0, frame_dt / DEMOTE_SECONDS);
                }
                if state.weight > 0.0 {
                    let global_light_index =
                        full.entity_shadow_light_source_indices[selection_index];
                    full.promoted_static_records
                        .push(PromotedStaticLightRecord {
                            global_light_index: global_light_index as u32,
                            selection_index: selection_index as u32,
                            pool_kind,
                            slot,
                            weight: state.weight.clamp(0.0, 1.0),
                        });
                }
            } else {
                state.pool_kind = None;
                state.sticky_remaining = 0.0;
                state.weight = step_toward(state.weight, 0.0, frame_dt / DEMOTE_SECONDS);
            }

            full.promoted_static_weights[selection_index] = state.weight.clamp(0.0, 1.0);
        }

        let record_count_before_cache_layers = full.promoted_static_records.len();
        if let Some(cache) = &mut full.promoted_depth_cache {
            let mut plan = cache.plan_frame(&full.promoted_static_records);
            full.promoted_static_cache_layers = apply_promoted_cache_layers(
                &mut full.promoted_static_records,
                &mut full.promoted_static_weights,
                &mut plan,
            );
            if full.promoted_static_records.len() != record_count_before_cache_layers
                && !full.promoted_depth_cache_missing_layer_warned
            {
                log::warn!(
                    "[Renderer] promoted shadow record had no depth-cache plan layer; dropping it for this frame"
                );
                full.promoted_depth_cache_missing_layer_warned = true;
            }
            full.promoted_depth_cache_promoted_count = plan.counters.promoted_count;
            full.promoted_depth_cache_world_render_skips = plan.counters.cached_world_render_skips;
            // The shadow passes accumulate these after planning their world/cache work.
            full.promoted_depth_cache_cull_dispatch_skips = 0;
            full.promoted_entity_occluders_submitted = 0;
            full.promoted_depth_cache_frame_plan = plan;
        } else {
            assert!(
                full.promoted_static_records.is_empty(),
                "promoted records require a promoted depth cache"
            );
            full.promoted_static_cache_layers.clear();
            full.promoted_depth_cache_frame_plan = PromotedDepthCacheFramePlan::default();
            full.promoted_depth_cache_promoted_count = 0;
            full.promoted_depth_cache_world_render_skips = 0;
            full.promoted_depth_cache_cull_dispatch_skips = 0;
            full.promoted_entity_occluders_submitted = 0;
        }

        full.promoted_static_weight_scratch.clear();
        full.promoted_static_weight_scratch
            .reserve(full.promoted_static_weights.len().max(1).saturating_mul(4));
        if full.promoted_static_weights.is_empty() {
            full.promoted_static_weight_scratch
                .extend_from_slice(&0.0f32.to_ne_bytes());
        } else {
            for &weight in &full.promoted_static_weights {
                full.promoted_static_weight_scratch
                    .extend_from_slice(&weight.clamp(0.0, 1.0).to_ne_bytes());
            }
        }
        queue.write_buffer(
            &full.promoted_static_weight_buffer,
            0,
            &full.promoted_static_weight_scratch,
        );
        full.total_light_count = full.light_count + full.promoted_static_records.len() as u32;
    }
}

/// Associate each promoted record with its static-depth cache layer before the
/// renderer uploads weights and appends shadowmask metadata. A missing layer is
/// recoverable defensive degradation: remove the record and zero its selection
/// weight so every downstream count and tail starts from the same surviving set.
///
fn apply_promoted_cache_layers(
    records: &mut Vec<PromotedStaticLightRecord>,
    weights: &mut [f32],
    plan: &mut PromotedDepthCacheFramePlan,
) -> Vec<i32> {
    let mut cache_layers = Vec::with_capacity(records.len());
    records.retain(|record| {
        let layer = match record.pool_kind {
            PromotedShadowPoolKind::Spot => plan
                .spot_for_slot(record.slot)
                .map(|spot| spot.cache_layer as i32),
            PromotedShadowPoolKind::Cube => plan.cube_for_slot(record.slot).map(|cube| {
                (cube.cache_layer_base / crate::lighting::cube_shadow::CUBE_FACES as u32) as i32
            }),
        };
        let Some(layer) = layer else {
            if let Some(weight) = weights.get_mut(record.selection_index as usize) {
                *weight = 0.0;
            }
            return false;
        };
        cache_layers.push(layer);
        true
    });
    plan.counters.promoted_count = records.len() as u32;
    cache_layers
}

/// Adapts a `MapLight` + camera to the shared shadow-slot score. The score
/// FORMULA lives once in [`postretro_lighting::shadow_ranking::slot_score`];
/// this only supplies the light→camera distance from the renderer's light type.
fn candidate_slot_score(light: &MapLight, camera_position: Vec3, camera_near_clip: f32) -> f32 {
    let light_pos = Vec3::new(
        light.origin[0] as f32,
        light.origin[1] as f32,
        light.origin[2] as f32,
    );
    let dist = (light_pos - camera_position).length();
    postretro_lighting::shadow_ranking::slot_score(light.falloff_range, dist, camera_near_clip)
}

/// Eviction hysteresis margin: a challenger takes an incumbent's slot only when
/// it out-scores the incumbent by this factor. Renderer-owned tuning policy fed
/// to the wgpu-free ranker (see the static-light-entity-shadows plan, Task 4).
const EVICTION_MARGIN: f32 = 1.25;

/// Rank this pool's lights (dynamic-tier + promoted-static) into slots through
/// the shared [`postretro_lighting::shadow_ranking`] core. The renderer-specific
/// state — which lights are promoted-static, and each slot's prior occupant for
/// hysteresis — is assembled here; the pure score sort, cap enforcement, and
/// tier-neutral eviction live in the lighting crate so the spot and cube pools
/// cannot drift and no scoring/assignment formula is duplicated.
fn assign_shadow_pool_slots_with_promoted_static(
    full: &FullRenderer,
    pool_kind: PromotedShadowPoolKind,
    camera_position: Vec3,
    camera_near_clip: f32,
    eligible_lights: &[bool],
    capacity: usize,
    promoted_cap: usize,
) -> Vec<u32> {
    let light_type = match pool_kind {
        PromotedShadowPoolKind::Spot => postretro_level_loader::LightType::Spot,
        PromotedShadowPoolKind::Cube => postretro_level_loader::LightType::Point,
    };

    // Unified candidate set: every eligible light of this pool's type competes on
    // score alone — no tier is reserved ahead of the sort (Resolution 2).
    let mut candidates = Vec::new();
    for (candidate_index, light) in full.shadow_candidate_lights.iter().enumerate() {
        if light.light_type != light_type {
            continue;
        }
        if eligible_lights
            .get(candidate_index)
            .is_some_and(|eligible| !eligible)
        {
            continue;
        }
        let is_promoted_static = full
            .shadow_candidate_selection_indices
            .get(candidate_index)
            .and_then(|selection| *selection)
            .is_some();
        // A baked, non-selected light never earns a runtime slot.
        if !is_promoted_static && !light.is_dynamic {
            continue;
        }
        candidates.push(postretro_lighting::shadow_ranking::SlotCandidate {
            candidate_index,
            score: candidate_slot_score(light, camera_position, camera_near_clip),
            is_promoted_static,
        });
    }

    let incumbents = build_slot_incumbents(
        full,
        pool_kind,
        capacity,
        camera_position,
        camera_near_clip,
        eligible_lights,
    );

    postretro_lighting::shadow_ranking::assign_slots_with_hysteresis(
        &candidates,
        &incumbents,
        capacity,
        promoted_cap,
        full.shadow_candidate_lights.len(),
        EVICTION_MARGIN,
    )
}

/// Prior-frame slot occupants for tier-neutral eviction hysteresis. Static
/// incumbents are the promoted lights still holding a slot with weight `w > 0`
/// (the demote sticky window keeps them here after their gate fails, so their
/// weight ramps down while the slot is held). Dynamic incumbents are the
/// still-eligible dynamic lights that held a slot last frame — they have no
/// sticky window, so an ineligible dynamic frees its slot immediately. Every
/// incumbent's `score` is the CURRENT-frame score so a camera jump is reflected
/// before the margin comparison.
fn build_slot_incumbents(
    full: &FullRenderer,
    pool_kind: PromotedShadowPoolKind,
    capacity: usize,
    camera_position: Vec3,
    camera_near_clip: f32,
    eligible_lights: &[bool],
) -> Vec<postretro_lighting::shadow_ranking::SlotIncumbent> {
    use postretro_lighting::shadow_ranking::SlotIncumbent;
    let mut incumbents = Vec::new();

    // Static incumbents: the promoted-static weight state owns their slot and
    // sticky window, independent of this frame's eligibility.
    for (selection_index, state) in full.promoted_static_states.iter().enumerate() {
        if state.pool_kind != Some(pool_kind) || state.weight <= 0.0 {
            continue;
        }
        let slot = state.slot as usize;
        if slot >= capacity {
            continue;
        }
        let Some(candidate_index) = full
            .shadow_candidate_selection_indices
            .iter()
            .position(|idx| *idx == Some(selection_index))
        else {
            continue;
        };
        let score = full
            .shadow_candidate_lights
            .get(candidate_index)
            .map(|light| candidate_slot_score(light, camera_position, camera_near_clip))
            .unwrap_or(state.last_score)
            .max(0.0);
        incumbents.push(SlotIncumbent {
            slot,
            candidate_index,
            score,
            is_promoted_static: true,
        });
    }

    // Dynamic incumbents: last frame's assignment for this pool, restricted to
    // still-eligible dynamic (non-selected) lights.
    let prior = match pool_kind {
        PromotedShadowPoolKind::Spot => full.spot_shadow_pool.slot_assignment.as_slice(),
        PromotedShadowPoolKind::Cube => full
            .cube_shadow_pool
            .as_ref()
            .map(|pool| pool.slot_assignment.as_slice())
            .unwrap_or(&[]),
    };
    for (candidate_index, &slot) in prior.iter().enumerate() {
        if slot == postretro_lighting::NO_SHADOW_SLOT {
            continue;
        }
        let slot = slot as usize;
        if slot >= capacity {
            continue;
        }
        let is_promoted_static = full
            .shadow_candidate_selection_indices
            .get(candidate_index)
            .and_then(|selection| *selection)
            .is_some();
        if is_promoted_static {
            // Statics come from the weight state above; skip so a slot is not
            // seeded twice.
            continue;
        }
        let Some(light) = full.shadow_candidate_lights.get(candidate_index) else {
            continue;
        };
        if !light.is_dynamic {
            continue;
        }
        if !eligible_lights
            .get(candidate_index)
            .copied()
            .unwrap_or(true)
        {
            continue;
        }
        incumbents.push(SlotIncumbent {
            slot,
            candidate_index,
            score: candidate_slot_score(light, camera_position, camera_near_clip).max(0.0),
            is_promoted_static: false,
        });
    }

    incumbents
}

fn clear_zero_weight_promoted_assignments(
    selection_indices: &[Option<usize>],
    weights: &[f32],
    assignment: &mut [u32],
) {
    for (candidate_index, slot) in assignment.iter_mut().enumerate() {
        if *slot == postretro_lighting::NO_SHADOW_SLOT {
            continue;
        }
        let Some(selection_index) = selection_indices
            .get(candidate_index)
            .and_then(|selection_index| *selection_index)
        else {
            continue;
        };
        if weights
            .get(selection_index)
            .is_none_or(|weight| *weight <= 0.0)
        {
            *slot = postretro_lighting::NO_SHADOW_SLOT;
        }
    }
}

fn step_toward(value: f32, target: f32, step: f32) -> f32 {
    if step <= 0.0 {
        return value.clamp(0.0, 1.0);
    }
    if value < target {
        (value + step).min(target)
    } else {
        (value - step).max(target)
    }
    .clamp(0.0, 1.0)
}

fn selected_static_light_has_shadow_entity(
    light: &MapLight,
    influence: Option<&LightInfluence>,
    plan: Option<&mesh_instances::MeshFramePlan>,
) -> bool {
    let Some(plan) = plan else {
        return false;
    };
    let influence = selected_static_light_shadow_influence(light, influence);
    plan.groups
        .iter()
        .flat_map(|group| group.instances.iter())
        .any(|instance| {
            let world = instance.bounds.transformed(&instance.transform);
            static_light_influence_intersects_aabb(light, &influence, &world)
        })
}

/// The mover counterpart to [`selected_static_light_has_shadow_entity`]. This
/// stays separate from `MeshFramePlan`: movers have their own renderer-owned
/// rigid-occluder data path and must share promotion capacity rather than mesh
/// planning or palette budgets.
fn selected_static_light_has_mover_occluder(
    light: &MapLight,
    influence: Option<&LightInfluence>,
    movers: &[rigid_occluder_depth::MoverOccluderAabb],
) -> bool {
    let influence = selected_static_light_shadow_influence(light, influence);
    movers
        .iter()
        .any(|mover| static_light_influence_intersects_aabb(light, &influence, &mover.world_aabb))
}

fn selected_static_light_shadow_influence(
    light: &MapLight,
    influence: Option<&LightInfluence>,
) -> LightInfluence {
    influence.cloned().unwrap_or(LightInfluence {
        center: Vec3::new(
            light.origin[0] as f32,
            light.origin[1] as f32,
            light.origin[2] as f32,
        ),
        radius: f32::MAX,
    })
}

fn static_light_influence_intersects_aabb(
    light: &MapLight,
    influence: &LightInfluence,
    world_aabb: &postretro_render_data::cone_frustum::Aabb,
) -> bool {
    let center = influence.center;
    let radius_sq = influence.radius.max(light.falloff_range).max(0.0).powi(2);
    let closest = center.clamp(world_aabb.min, world_aabb.max);
    closest.distance_squared(center) <= radius_sq
}

fn build_count_split_light_upload(
    full: &FullRenderer,
    level_spot_slots: &[u32],
    level_cube_slots: &[u32],
    bytes: &mut Vec<u8>,
) {
    bytes.clear();
    let dynamic_bytes = full.light_count as usize * postretro_lighting::GPU_LIGHT_SIZE;
    if full.last_lights_upload.len() >= dynamic_bytes && dynamic_bytes > 0 {
        bytes.extend_from_slice(&full.last_lights_upload[..dynamic_bytes]);
        postretro_lighting::patch_shadow_slots(bytes, level_spot_slots);
        postretro_lighting::patch_cube_slots(bytes, level_cube_slots);
    } else if !full.level_lights.is_empty() {
        pack_lights_with_slots_into(bytes, &full.level_lights, level_spot_slots);
        postretro_lighting::patch_cube_slots(bytes, level_cube_slots);
        bytes.resize(dynamic_bytes, 0);
    }

    // `entity_shadow_lights` is raw-length N and index-parallel to the selection
    // index, so `[selection_index]` is a direct aligned lookup for every promoted
    // record — no compacted-vs-raw index divergence, and the light record it
    // appends stays in lock-step with the influence tail packed in
    // `update_dynamic_light_slots` (both key on the same selection index).
    debug_assert_eq!(
        full.entity_shadow_lights.len(),
        full.promoted_static_states.len(),
        "selected-static light vector must be raw-length N, index-parallel to selection index",
    );
    for record in &full.promoted_static_records {
        let light = &full.entity_shadow_lights[record.selection_index as usize];
        let mut weighted = light.clone();
        weighted.intensity *= record.weight.clamp(0.0, 1.0);
        let spot_slot = match record.pool_kind {
            PromotedShadowPoolKind::Spot => record.slot,
            PromotedShadowPoolKind::Cube => postretro_lighting::NO_SHADOW_SLOT,
        };
        bytes.extend_from_slice(&postretro_lighting::pack_light_with_slot(
            &weighted, spot_slot,
        ));
        if record.pool_kind == PromotedShadowPoolKind::Cube {
            let start = bytes.len() - postretro_lighting::GPU_LIGHT_SIZE;
            postretro_lighting::patch_cube_slots(&mut bytes[start..], &[record.slot]);
        }
    }

    if bytes.is_empty() {
        bytes.resize(postretro_lighting::GPU_LIGHT_SIZE, 0);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn promoted_static_record_schema_carries_task4_contract_fields() {
        let record = PromotedStaticLightRecord {
            global_light_index: 42,
            selection_index: 3,
            pool_kind: PromotedShadowPoolKind::Cube,
            slot: 1,
            weight: 0.75,
        };

        assert_eq!(record.global_light_index, 42);
        assert_eq!(record.selection_index, 3);
        assert_eq!(record.pool_kind, PromotedShadowPoolKind::Cube);
        assert_eq!(record.slot, 1);
        assert!((0.0..=1.0).contains(&record.weight));
    }

    #[test]
    fn promoted_static_caps_match_task4_budget() {
        assert_eq!(MAX_PROMOTED_SPOT, 8);
        assert_eq!(MAX_PROMOTED_CUBE, 2);
    }

    #[test]
    fn missing_cache_plan_layer_drops_record_and_zeros_weight_before_metadata_pack() {
        let mut records = vec![PromotedStaticLightRecord {
            global_light_index: 42,
            selection_index: 0,
            pool_kind: PromotedShadowPoolKind::Spot,
            slot: 3,
            weight: 0.75,
        }];
        let mut weights = [0.75];
        let mut plan = PromotedDepthCacheFramePlan::default();

        let cache_layers = apply_promoted_cache_layers(&mut records, &mut weights, &mut plan);

        assert!(
            records.is_empty(),
            "a record without a cache layer must not upload"
        );
        assert_eq!(
            weights,
            [0.0],
            "dropped records must zero their selection weight"
        );
        assert!(
            cache_layers.is_empty(),
            "dropped records must not pack a metadata tail"
        );
        assert_eq!(plan.counters.promoted_count, 0);
    }

    #[test]
    fn promoted_weight_step_is_reversible_and_clamped() {
        let w = step_toward(0.0, 1.0, 0.5);
        assert!((w - 0.5).abs() < 1.0e-6);
        let w = step_toward(w, 0.0, 0.2);
        assert!((w - 0.3).abs() < 1.0e-6);
        assert_eq!(step_toward(0.95, 1.0, 0.2), 1.0);
        assert_eq!(step_toward(0.05, 0.0, 0.2), 0.0);
    }

    // The pure score sort, promoted-static cap, and tier-neutral eviction
    // hysteresis now live in `postretro_lighting::shadow_ranking` — their
    // regression tests (cap-full static swap, dynamic⇄static eviction, the 1.25x
    // margin) moved there with the code. This module keeps the renderer-owned
    // state tests below.

    #[test]
    fn zero_weight_promoted_static_assignment_is_cleared_before_occupancy() {
        let selection_indices = [Some(0), None, Some(1)];
        let weights = [0.0, 0.25];
        let mut assignment = [2, 3, 4];

        clear_zero_weight_promoted_assignments(&selection_indices, &weights, &mut assignment);

        assert_eq!(assignment[0], postretro_lighting::NO_SHADOW_SLOT);
        assert_eq!(assignment[1], 3, "dynamic assignments are not touched");
        assert_eq!(assignment[2], 4, "nonzero promoted assignments survive");
    }

    #[test]
    fn selected_static_entity_gate_uses_planned_instances_only() {
        use postretro_level_loader::{FalloffModel, LightType, MapLight, ShadowType};
        use postretro_model::ModelHandle;
        use postretro_model::sample_params::MeshSampleParams;
        use postretro_render_cpu::mesh_instances::{
            MeshFramePlan, ModelDrawGroup, PlannedInstance,
        };
        use postretro_render_data::cone_frustum::Aabb;
        use postretro_render_data::influence::LightInfluence;

        let light = MapLight {
            origin: [0.0, 0.0, 0.0],
            light_type: LightType::Spot,
            intensity: 1.0,
            color: [1.0, 1.0, 1.0],
            falloff_model: FalloffModel::Linear,
            falloff_range: 4.0,
            cone_angle_inner: 0.3,
            cone_angle_outer: 0.4,
            cone_direction: [0.0, 0.0, -1.0],
            is_dynamic: false,
            casts_entity_shadows: true,
            animated_slot: None,
            tags: vec![],
            cell_index: 0,
            shadow_type: ShadowType::StaticLightMap,
        };
        let influence = LightInfluence {
            center: Vec3::ZERO,
            radius: 4.0,
        };
        let planned_inside = PlannedInstance {
            transform: glam::Mat4::from_translation(Vec3::new(1.0, 0.0, 0.0)),
            shadow_bias_scale: 1.0,
            palette_base: 0,
            phase_seed: 1,
            palette_cache_key: postretro_render_cpu::mesh_instances::MeshPaletteCacheKey::Entity(1),
            bounds: Aabb {
                min: Vec3::splat(-0.5),
                max: Vec3::splat(0.5),
            },
            sample: MeshSampleParams::stateless(0.0),
            pose_inputs: None,
            capture: None,
            resample: true,
            forward_visible: false,
            dynamic_shadow_visible: false,
        };
        let planned_outside = PlannedInstance {
            transform: glam::Mat4::from_translation(Vec3::new(20.0, 0.0, 0.0)),
            phase_seed: 2,
            palette_cache_key: postretro_render_cpu::mesh_instances::MeshPaletteCacheKey::Entity(2),
            ..planned_inside.clone()
        };
        let plan = MeshFramePlan {
            groups: vec![ModelDrawGroup {
                model: ModelHandle::from("grunt"),
                instance_offset: 0,
                instances: vec![planned_outside, planned_inside],
            }],
            instance_count: 2,
            dropped: 0,
        };

        assert!(
            !selected_static_light_has_shadow_entity(&light, Some(&influence), None),
            "without a planned/renderable mesh set, raw mesh inputs must not promote a static light",
        );
        assert!(
            selected_static_light_has_shadow_entity(&light, Some(&influence), Some(&plan)),
            "planned shadow-only instances may promote selected static lights",
        );
    }

    #[test]
    fn selected_static_mover_gate_uses_active_mover_aabbs() {
        use postretro_level_loader::{FalloffModel, LightType, MapLight, ShadowType};
        use postretro_render_data::cone_frustum::Aabb;
        use postretro_render_data::influence::LightInfluence;

        let light = MapLight {
            origin: [0.0, 0.0, 0.0],
            light_type: LightType::Spot,
            intensity: 1.0,
            color: [1.0, 1.0, 1.0],
            falloff_model: FalloffModel::Linear,
            falloff_range: 4.0,
            cone_angle_inner: 0.3,
            cone_angle_outer: 0.4,
            cone_direction: [0.0, 0.0, -1.0],
            is_dynamic: false,
            casts_entity_shadows: true,
            animated_slot: None,
            tags: vec![],
            cell_index: 0,
            shadow_type: ShadowType::StaticLightMap,
        };
        let influence = LightInfluence {
            center: Vec3::ZERO,
            radius: 4.0,
        };
        let inside = rigid_occluder_depth::MoverOccluderAabb {
            mover_id: 7,
            world_aabb: Aabb {
                min: Vec3::new(3.5, -0.5, -0.5),
                max: Vec3::new(4.5, 0.5, 0.5),
            },
        };
        let outside = rigid_occluder_depth::MoverOccluderAabb {
            mover_id: 8,
            world_aabb: Aabb {
                min: Vec3::new(5.0, -0.5, -0.5),
                max: Vec3::new(6.0, 0.5, 0.5),
            },
        };

        assert!(
            !selected_static_light_has_mover_occluder(&light, Some(&influence), &[]),
            "an empty per-frame mover set preserves the prior mesh-only result",
        );
        assert!(
            !selected_static_light_has_mover_occluder(&light, Some(&influence), &[outside]),
            "a mover beyond the influence radius cannot promote the static light",
        );
        assert!(
            selected_static_light_has_mover_occluder(&light, Some(&influence), &[inside]),
            "an active mover alone makes the static light promotion-relevant",
        );
    }
}
