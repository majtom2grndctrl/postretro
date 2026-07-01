// Dynamic direct-light slot assignment, spot/cube shadow-pool packing, and the
// shadow-debug trace.
// See: context/lib/rendering_pipeline.md §4

use super::*;

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
    /// The **candidate set** is `self.shadow_candidate_lights`
    /// (full level lights filtered by `is_dynamic`), which is the same set as
    /// `self.level_lights` (also `is_dynamic`-filtered) modulo ordering.
    /// `effective_brightness` is keyed on `level_lights` indices, so candidate
    /// brightness is translated through the original full level-light index.
    pub fn update_dynamic_light_slots(
        &mut self,
        camera_position: Vec3,
        camera_near_clip: f32,
        effective_brightness: &[f32],
        reachable_cell_aabbs: &[(Vec3, Vec3)],
    ) {
        // Candidate set is `is_dynamic`-filtered; if the map has no dynamic
        // lights the pool stays empty — early-return without disturbing
        // previous slots.
        if self.full().shadow_candidate_lights.is_empty() {
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
            for (i, light) in full.shadow_candidate_lights.iter().enumerate() {
                let reaches_view = shadow_candidate_reaches_visible_cell(
                    light,
                    full.shadow_candidate_influences.get(i),
                    reachable_cell_aabbs,
                );
                if !reaches_view {
                    continue;
                }
                // Brightness suppression is indexed by `level_lights` (the
                // forward / scripted-bridge index space). For candidates not in
                // `level_lights` we have no per-frame brightness — treat as 1.0.
                let b = full
                    .shadow_candidate_source_indices
                    .get(i)
                    .and_then(|&source_index| {
                        level_brightness_for_candidate(
                            &full.level_light_source_indices,
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

        let slot_assignment = SpotShadowPool::rank_lights(
            &self.full().shadow_candidate_lights,
            camera_position,
            camera_near_clip,
            &visible_lights,
        );

        // Rank dynamic POINT lights into the cube pool and upload their per-face
        // matrices. Returns the candidate-indexed cube slot assignment (empty
        // when the pool is disabled), which is patched into the light buffer
        // below alongside the spot slots. Runs before the patch block so both
        // slot fields land in one upload.
        let stride = self.full().shadow_vs_stride as usize;
        let cube_slot_assignment = self.update_cube_light_slots(
            camera_position,
            camera_near_clip,
            &visible_lights,
            stride,
        );

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
        let expected_len = full.level_lights.len() * crate::lighting::GPU_LIGHT_SIZE;
        if full.last_lights_upload.len() == expected_len {
            let spot_changed =
                crate::lighting::patch_shadow_slots(&mut full.last_lights_upload, &level_slots);
            let cube_changed =
                crate::lighting::patch_cube_slots(&mut full.last_lights_upload, &level_cube_slots);
            if spot_changed || cube_changed {
                queue.write_buffer(&full.lights_buffer, 0, &full.last_lights_upload);
            }
        } else {
            // Mirror not yet sized to the current light set (before the first
            // bridge upload, or the light count changed): full static pack so
            // frame-zero still uploads valid lights + slots and seeds the mirror.
            let mut scratch = std::mem::take(&mut full.lights_pack_scratch);
            pack_lights_with_slots_into(&mut scratch, &full.level_lights, &level_slots);
            crate::lighting::patch_cube_slots(&mut scratch, &level_cube_slots);
            if scratch != full.last_lights_upload {
                queue.write_buffer(&full.lights_buffer, 0, &scratch);
                full.last_lights_upload.clear();
                full.last_lights_upload.extend_from_slice(&scratch);
            }
            full.lights_pack_scratch = scratch;
        }

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
            let m = crate::lighting::spot_shadow::light_space_matrix(candidate);
            // Stash the SAME light-space matrix uploaded to bind-group-5 below —
            // the shadow-depth render loop reads it to build this slot's cone
            // cull frustum planes (one source of truth, no recomputation).
            full.spot_shadow_pool.slot_cone_matrices[slot as usize] = Some(m);
            // Record whether this slot's occupant renders entity occluders. The
            // shadow-depth loop draws skinned occluders into the slot only when
            // this is set; an ineligible (e.g. toggle-off dynamic) slot keeps its
            // world shadow but draws none.
            full.spot_shadow_pool.slot_entity_eligible[slot as usize] =
                crate::lighting::entity_occluder_eligible(candidate);
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
    ///   the reach+brightness gate feeding `rank_lights`/`rank_point_lights`), and
    ///   `slot` (assigned SPOT shadow slot or `NONE:<reason>`) plus `cube`
    ///   (assigned POINT cube shadow slot or `NONE:<reason>` — closes the prior
    ///   blind spot where point lights only ever showed `NONE:not_spot`). NOTE
    ///   these read the STATIC load-time `shadow_candidate_lights` — a scripted
    ///   sweep light's animated position/cone is NOT reflected here.
    /// - `casters`: `in_pvs` vs `off_pvs` from collected mesh draw inputs. The
    ///   strict collector should keep `off_pvs=0`.
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
                    level_brightness_for_candidate(
                        &self.full().level_light_source_indices,
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
                        let ent_ok = crate::lighting::entity_occluder_eligible(light);
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

        // Mesh visibility split. The strict collector emits visible-only draw
        // inputs, so `off_pvs` should stay zero outside synthetic/future callers.
        let in_pvs = self
            .full()
            .mesh_draws
            .iter()
            .filter(|m| m.forward_visible)
            .count() as u32;
        let off_pvs = self
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
        let fingerprint = (slot_occupancy, cube_occupancy, in_pvs, off_pvs);
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
            "[shadow_dbg f={f}{}] cam: pitch={:.1}deg fwd({:.2},{:.2},{:.2}) eye({:.0},{:.0},{:.0}) cell={cell_str} vis_cells={vis_cells} | pools: spot={spot_used}/{} cube={cube_used}/{cube_pool_size} elig_spot={elig_spot} elig_cube={elig_cube} spot_overflow={spot_overflow} cube_overflow={cube_overflow} | casters: in_pvs={in_pvs} off_pvs={off_pvs} total={} | occupied_spot_slots={} occupied_cube_slots={} | lights[{}]: {}",
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
    /// drift. Cube faces are ENTITY-ONLY in v1 — `slot_entity_eligible` decides
    /// whether the depth loop draws anything into a slot at all.
    ///
    /// The RETURNED (shader-facing) assignment masks any light that owns a ranked
    /// slot but is not `entity_occluder_eligible` back to the sentinel: its cube
    /// faces are never cleared/rendered (the depth loop skips `None` matrices), so
    /// the shader must not sample that slot. See `cube_shadow::shader_facing_cube_slot`.
    /// The pool's internal `slot_assignment` keeps the raw rank for diagnostics.
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

        let Some(pool) = full.cube_shadow_pool.as_mut() else {
            return Vec::new();
        };

        let slot_assignment = cube_shadow::rank_point_lights(
            &full.shadow_candidate_lights,
            camera_position,
            camera_near_clip,
            visible_lights,
        );

        // Shader-facing slot assignment, returned to the caller and patched into
        // each point light's `cone_angles_and_pad.w`. It DIVERGES from the
        // internal `slot_assignment` for ineligible lights: see the per-light
        // masking below. Starts as a copy of the rank and is downgraded to the
        // sentinel for any light whose cube faces will not be rendered.
        let mut shader_slot_assignment = slot_assignment.clone();

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
            // Cube faces are entity-only: an ineligible point light draws
            // nothing, so it needs no per-face matrices either.
            let eligible = crate::lighting::entity_occluder_eligible(candidate);
            pool.slot_entity_eligible[slot as usize] = eligible;
            // CRITICAL: a cube slot's faces are only CLEARED + rendered when the
            // light is entity-eligible (the depth loop skips `None` face matrices).
            // An ineligible slot's faces hold stale/uninitialized depth, so the
            // shader must NOT sample them — `shader_facing_cube_slot` downgrades
            // those to the sentinel (unshadowed). Unlike the spot path, where every
            // occupied slot always renders a Clear(1.0)+world-depth baseline, a cube
            // face carries no world geometry and no clear, so sampling an
            // occluder-free face would read garbage (often fully shadowed) and ZERO
            // the light when its origin is on-screen (slots are only assigned to
            // visible lights — hence the view-dependence of the original bug).
            shader_slot_assignment[light_idx] =
                cube_shadow::shader_facing_cube_slot(slot, eligible);
            if !eligible {
                continue;
            }
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

        pool.slot_assignment = slot_assignment;
        // Return the SHADER-facing assignment (ineligible lights masked to the
        // sentinel), not the raw rank — the caller patches this into the light
        // buffer, and only slots with rendered occluders may be sampled.
        shader_slot_assignment
    }
}
