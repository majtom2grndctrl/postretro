// The per-frame indirect render orchestration: depth pre-pass, shadow passes,
// forward pass, mesh/smoke/fog passes, and submission.
// See: context/lib/rendering_pipeline.md §1

use super::*;

impl Renderer {
    #[allow(clippy::too_many_arguments)]
    pub fn render_frame_indirect(
        &mut self,
        cam_vis: CameraCullVisibility<'_>,
        light_reachable_cell_mask: &[bool],
        reachable_cell_aabbs: &[(Vec3, Vec3)],
        fog_reachable: &[u32],
        camera_cell: Option<u32>,
        view_proj: Mat4,
        particle_collections: &[(&str, &[u8])],
        now_seconds: f64,
        clear_color: ClearColor,
        render_world: bool,
    ) -> Result<Option<wgpu::SurfaceTexture>> {
        // The drawable visible-cell set; candidate-cull eligibility derives
        // from `cam_vis` (set + path provenance) inside `record_pre_scene_compute`.
        let visible: &VisibleCells = cam_vis.cells;

        self.full_mut().debug_frame = self.full().debug_frame.wrapping_add(1);
        let output = match self.surface.get_current_texture() {
            wgpu::CurrentSurfaceTexture::Success(tex) => tex,
            wgpu::CurrentSurfaceTexture::Suboptimal(tex) => {
                self.surface.configure(&self.device, &self.surface_config);
                tex
            }
            wgpu::CurrentSurfaceTexture::Timeout | wgpu::CurrentSurfaceTexture::Occluded => {
                return Ok(None);
            }
            wgpu::CurrentSurfaceTexture::Outdated => {
                self.surface.configure(&self.device, &self.surface_config);
                return Ok(None);
            }
            wgpu::CurrentSurfaceTexture::Lost => {
                anyhow::bail!("surface lost");
            }
            wgpu::CurrentSurfaceTexture::Validation => {
                anyhow::bail!("surface validation error");
            }
        };

        let view = output
            .texture
            .create_view(&wgpu::TextureViewDescriptor::default());

        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("Frame Encoder"),
            });

        self.record_pre_scene_compute(&mut encoder, cam_vis, view_proj, render_world);

        // The readback copy is deliberately not encoded here. A
        // `copy_texture_to_buffer` in the same command buffer as the compose
        // dispatch reads the `total` atlas texture before its storage writes
        // are visible, flickering garbage into the markers. It runs after a
        // blocking `poll(Wait)` below, once the compose submit has retired.

        // mem::take avoids a simultaneous borrow of self; returned after call to reuse the allocation.
        if render_world {
            let eff_brightness = std::mem::take(&mut self.full_mut().light_effective_brightness);
            let last_camera_position = self.full().last_camera_position;
            self.update_dynamic_light_slots(
                last_camera_position,
                crate::lighting::spot_shadow::SHADOW_NEAR_CLIP,
                &eff_brightness,
                reachable_cell_aabbs,
            );
            // Env-gated diagnostics (POSTRETRO_SHADOW_DEBUG=1) — read-only, runs
            // right after slot assignment so it sees this frame's decisions. No
            // effect on culling/selection. Skipped entirely when disabled.
            if self.full().shadow_debug_enabled {
                self.emit_shadow_debug(
                    view_proj,
                    visible,
                    light_reachable_cell_mask,
                    reachable_cell_aabbs,
                    &eff_brightness,
                    camera_cell,
                );
            }
            self.full_mut().light_effective_brightness = eff_brightness;
        }

        // --- Skinned-mesh pose/upload HOIST ----------------------------------
        // Plan + sample + upload the skinned-mesh palette/instance buffers HERE —
        // after `update_dynamic_light_slots`, BEFORE the spot-shadow depth loop —
        // so the skinned-depth shadow occluder pass and the forward mesh draw both
        // read the SAME already-posed buffers. Nothing rewrites `palette_buffer`/
        // `instance_buffer` between this point and the forward `record_draws`, so
        // an entity and its shadow are sampled at the identical pose (no one-frame
        // lag). The plan is held in `mesh_frame_plan` and consumed by both passes.
        let mesh_frame_plan: Option<mesh_instances::MeshFramePlan> = if render_world
            && self.full().mesh_pass.has_model()
            && !self.full().mesh_draws.is_empty()
        {
            // Plan: group instances by model, assign each a contiguous palette
            // run, drop any overflow past the fixed budget. GPU-free.
            let plan =
                mesh_instances::plan_mesh_frame(&self.full().mesh_draws, &self.full().mesh_pass);

            // Overflow drops excess instances rather than corrupting the
            // palette or panicking — rate-limited warning. Covers BOTH the
            // palette-slot cap and the instance-count cap (the latter is what
            // fires for rigid / zero-joint props, which consume no slots).
            if plan.dropped > 0 {
                let now = now_seconds as f32;
                if now - self.full().mesh_overflow_last_warn >= 1.0 {
                    log::warn!(
                        "[Renderer] skinned-mesh budget exceeded: dropped {} instance(s) \
                             (budget {} palette slots / {} instances); excess not drawn",
                        plan.dropped,
                        mesh_instances::MAX_PALETTE_ENTRIES,
                        mesh_instances::MAX_INSTANCES,
                    );
                    self.full_mut().mesh_overflow_last_warn = now;
                }
            }

            // Sample every instance's clip into its palette run + write the
            // per-instance SSBO. The ONLY per-frame write to these buffers —
            // both the shadow loop and the forward draw read them unchanged.
            {
                let Self { queue, full, .. } = self;
                let full = full
                    .as_mut()
                    .expect("renderer full-init must complete before full-ready paths run");
                full.mesh_pass
                    .plan_and_upload(queue, &plan, &mut full.bone_palette_scratch);
            }
            (!plan.groups.is_empty()).then_some(plan)
        } else {
            None
        };

        if render_world && self.full().has_geometry && self.full().index_count > 0 {
            self.record_spot_shadow_depth(&mut encoder, mesh_frame_plan.as_ref());
        }

        // --- Cube point-light shadow depth loop (entity-only) ----------------
        // For each occupied cube slot whose light is `entity_occluder_eligible`,
        // CLEAR all 6 faces to the far plane (1.0) and render entity occluders
        // into them. Cube faces carry NO world geometry in v1, so this loop is
        // independent of `has_geometry`; an ineligible point light (which has no
        // per-face matrices) is skipped entirely. Per face: a depth render pass
        // into the `slot*6 + face` D2Array view, projecting by that face's
        // light-space matrix (group 0, dynamic offset into the cube VS uniform
        // buffer), with the per-instance cone cull inside `record_skinned_depth`
        // testing each bound against the face's 90° frustum planes. Reuses the
        // SAME cube-ready depth pipeline as the spot path.
        //
        // CRITICAL: the per-face Clear(1.0) baseline must run for EVERY occupied
        // eligible face regardless of whether any skinned-mesh occluders exist
        // this frame. Gating the whole loop on `mesh_frame_plan` being `Some`
        // (the prior bug) meant that when no mesh entity was in the PVS — e.g. a
        // combat arena whose meshes are all off-screen — the occupied faces were
        // NEVER cleared and held stale/uninitialized depth (~0.0). An on-screen
        // eligible point light then sampled that garbage and read fully shadowed
        // (CompareFunction::Less: reference >= 0 is never < 0), zeroing its world
        // illumination. Off-screen lights own no slot (sentinel), so they stayed
        // lit — the view-dependent symptom. The clear is now unconditional and
        // the occluder draw is the only mesh-plan-gated step, mirroring the spot
        // path's "every occupied slot gets a Clear(1.0) baseline" invariant.
        self.full_mut().cube_entity_occluders_submitted = 0;
        if render_world {
            self.record_cube_shadow_depth(&mut encoder, mesh_frame_plan.as_ref());
        }

        self.record_depth_and_sdf_passes(&mut encoder, view_proj, render_world);

        // Post-scene compositor seam: every gameplay scene + UI pass renders into
        // `scene_color` (the offscreen target) instead of the swapchain `view`.
        // The resolve pass below is the sole swapchain writer for the gameplay
        // path. The view is cloned (wgpu `TextureView` is `Arc`-backed) into an
        // OWNED handle so it no longer borrows `self.full()` — the post-split
        // `full()`/`full_mut()` accessors borrow ALL of `self`, so holding a
        // borrow of `screen_effects` across the later `&mut self` pass/helper
        // calls (ui, debug_lines, wireframe overlay) would conflict. The owned
        // clone preserves the disjoint-borrow behavior the inline-field layout
        // had. The splash path is unaffected — it writes the swapchain directly
        // and never touches this target.
        let scene_color = self.full().screen_effects.scene_color_view().clone();

        {
            let forward_ts = self
                .full()
                .frame_timing
                .as_ref()
                .map(|t| t.render_pass_writes(TIMING_PAIR_FORWARD));
            let mut render_pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("Textured Pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &scene_color,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(clear_color.into()),
                        store: wgpu::StoreOp::Store,
                    },
                })],
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

            if render_world && self.full().has_geometry && self.full().index_count > 0 {
                render_pass.set_pipeline(&self.full().pipeline);
                render_pass.set_bind_group(0, &self.full().uniform_bind_group, &[]);
                render_pass.set_bind_group(2, &self.full().lighting_bind_group, &[]);
                render_pass.set_bind_group(3, &self.full().sh_volume_resources.bind_group, &[]);
                render_pass.set_bind_group(4, &self.full().lightmap_resources.bind_group, &[]);
                render_pass.set_bind_group(5, &self.full().spot_shadow_pool.bind_group, &[]);
                render_pass.set_vertex_buffer(0, self.full().vertex_buffer.slice(..));
                render_pass.set_index_buffer(
                    self.full().index_buffer.slice(..),
                    wgpu::IndexFormat::Uint32,
                );

                if let Some(cull) = &self.full().compute_cull {
                    let gpu_textures = &self.full().gpu_textures;
                    cull.draw_indirect(
                        &mut render_pass,
                        Some(&|pass, bucket| {
                            let bind_group = if (bucket as usize) < gpu_textures.len() {
                                &gpu_textures[bucket as usize].bind_group
                            } else {
                                &gpu_textures[0].bind_group
                            };
                            pass.set_bind_group(1, bind_group, &[]);
                        }),
                    );
                }
            }
        }

        // Skinned-mesh forward pass — after the opaque world forward, before
        // billboards. Its own render pass so it can WRITE depth (the forward pass
        // holds the depth attachment read-only). Loads the existing color + depth
        // so the mesh composites over the world and depth-tests (`Less`).
        //
        // Reads the `mesh_frame_plan` PLANNED + UPLOADED earlier in this frame
        // (the pose/upload hoist, before the shadow loop). NO re-plan, NO
        // re-upload here — `record_draws` only records draws against the buffers
        // the hoist populated, the SAME buffers the skinned-depth shadow pass
        // read, so an entity and its shadow share one pose (no one-frame lag).
        if render_world {
            if let Some(plan) = &mesh_frame_plan {
                // Mesh group-2 params uniform (binding 4): the dynamic-light count, the
                // frame's render-clock time (the SAME value written to forward
                // `Uniforms.time` this frame — cached in `update_per_frame_uniforms` —
                // so the scripted-light curves the mesh loop evaluates stay
                // phase-coherent), and the SAME `lighting_isolation` value written to
                // forward `Uniforms.lighting_isolation` this frame, so the mesh
                // dynamic-direct term participates in the lighting-isolation debug
                // modes exactly as the world dynamic term does (the shader derives
                // `use_dynamic` from it, mirroring forward.wgsl).
                {
                    let Self { queue, full, .. } = self;
                    let full = full
                        .as_mut()
                        .expect("renderer full-init must complete before full-ready paths run");
                    full.mesh_pass.write_light_params(
                        queue,
                        full.light_count,
                        full.mesh_dynamic_time,
                        full.lighting_isolation as u32,
                        full.ambient_floor,
                    );
                }
                let mut mesh_enc = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                    label: Some("Skinned Mesh Pass"),
                    color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                        view: &scene_color,
                        depth_slice: None,
                        resolve_target: None,
                        ops: wgpu::Operations {
                            load: wgpu::LoadOp::Load,
                            store: wgpu::StoreOp::Store,
                        },
                    })],
                    depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                        view: &self.full().depth_view,
                        depth_ops: Some(wgpu::Operations {
                            load: wgpu::LoadOp::Load,
                            store: wgpu::StoreOp::Store,
                        }),
                        stencil_ops: None,
                    }),
                    timestamp_writes: None,
                    ..Default::default()
                });
                mesh_enc.set_bind_group(0, &self.full().uniform_bind_group, &[]);
                // Group 4 = SH irradiance volume (baked indirect) + the mesh-only
                // dynamic-direct params uniform (binding 16). The mesh SUPERSET bind
                // group: shared SH entries the forward/billboard/fog passes hold PLUS
                // the dynamic-direct knobs (group 3 = instance data; group 2
                // unallocated).
                mesh_enc.set_bind_group(4, &self.full().sh_volume_resources.mesh_bind_group, &[]);
                self.full_mut().mesh_pass.record_draws(&mut mesh_enc, plan);
            }
        }

        // After opaque forward, before wireframe. Alpha additive; depth test on, write off.
        if render_world
            && self.full().smoke_pass.has_any_sheet()
            && !particle_collections.is_empty()
        {
            let smoke_ts = self
                .full()
                .frame_timing
                .as_ref()
                .map(|t| t.render_pass_writes(TIMING_PAIR_SMOKE));
            let mut smoke_pass_enc = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("Billboard Sprite Pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &scene_color,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Load,
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                    view: &self.full().depth_view,
                    depth_ops: Some(wgpu::Operations {
                        load: wgpu::LoadOp::Load,
                        store: wgpu::StoreOp::Store,
                    }),
                    stencil_ops: None,
                }),
                timestamp_writes: smoke_ts,
                ..Default::default()
            });
            smoke_pass_enc.set_bind_group(0, &self.full().uniform_bind_group, &[]);
            smoke_pass_enc.set_bind_group(2, &self.full().lighting_bind_group, &[]);
            smoke_pass_enc.set_bind_group(3, &self.full().sh_volume_resources.bind_group, &[]);
            // One shared instance buffer, drawn per collection from its own
            // 256-byte-aligned dynamic offset.
            {
                let Self {
                    device,
                    queue,
                    full,
                    ..
                } = self;
                let full = full
                    .as_mut()
                    .expect("renderer full-init must complete before full-ready paths run");
                full.smoke_pass.record_draws(
                    device,
                    queue,
                    &mut smoke_pass_enc,
                    particle_collections,
                );
            }
        }

        // Volumetric fog: low-res compute raymarch + additive composite.
        // Skipped when no active volumes — scatter target need not be cleared.
        // See: context/lib/rendering_pipeline.md §7.5
        if render_world {
            let cell_mask = compute_fog_cell_mask(
                fog_reachable,
                self.full().fog_cell_masks.as_deref(),
                self.full().fog.canonical_volume_count(),
                camera_cell,
            );
            {
                let Self { queue, full, .. } = self;
                let full = full
                    .as_mut()
                    .expect("renderer full-init must complete before full-ready paths run");
                full.fog.repack_active(queue, cell_mask, now_seconds);
            }
        }
        if render_world && self.full().fog.active() {
            // Spots before params so FogParams.spot_count reflects this frame's count.
            let fog_spots = self.collect_fog_spot_lights();
            {
                let Self { queue, full, .. } = self;
                let full = full
                    .as_mut()
                    .expect("renderer full-init must complete before full-ready paths run");
                full.fog.upload_spots(queue, &fog_spots);
            }

            let inv_view_proj = view_proj.inverse();
            {
                let Self { queue, full, .. } = self;
                let full = full
                    .as_mut()
                    .expect("renderer full-init must complete before full-ready paths run");
                full.fog.upload_params(
                    queue,
                    inv_view_proj,
                    full.last_camera_position,
                    crate::camera::NEAR,
                    crate::camera::FAR,
                );
            }

            let (scatter_w, scatter_h) = self.full().fog.scatter_dims();
            // 8×8 matches @workgroup_size(8,8); div_ceil covers edge pixels.
            let groups_x = scatter_w.div_ceil(8);
            let groups_y = scatter_h.div_ceil(8);
            {
                let mut raymarch = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
                    label: Some("Fog Raymarch Pass"),
                    timestamp_writes: None,
                });
                raymarch.set_pipeline(&self.full().fog.raymarch_pipeline);
                raymarch.set_bind_group(0, &self.full().uniform_bind_group, &[]);
                raymarch.set_bind_group(3, &self.full().sh_volume_resources.bind_group, &[]);
                raymarch.set_bind_group(5, &self.full().spot_shadow_pool.bind_group, &[]);
                raymarch.set_bind_group(6, &self.full().fog.bind_group, &[]);
                raymarch.dispatch_workgroups(groups_x, groups_y, 1);
            }

            let mut composite = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("Fog Composite Pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &scene_color,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Load,
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: None,
                ..Default::default()
            });
            composite.set_pipeline(&self.full().fog.composite_pipeline);
            composite.set_bind_group(0, &self.full().fog.composite_bind_group, &[]);
            composite.draw(0..3, 0..1); // fullscreen triangle from vertex_index — no vertex buffer
        }

        self.record_wireframe_overlay(&mut encoder, &scene_color, render_world, visible);

        #[cfg(feature = "dev-tools")]
        if render_world {
            let Self { queue, full, .. } = self;
            let full = full
                .as_mut()
                .expect("renderer full-init must complete before full-ready paths run");
            full.debug_lines.render(
                queue,
                &mut encoder,
                &scene_color,
                &full.depth_view,
                &full.uniform_bind_group,
            );
            // Buffer is cleared by the frame loop (via `clear_debug_lines`)
            // before the next frame's emit call — that single owner handles
            // surface Timeout/Occluded/Outdated early-returns above without
            // leaking segments across frames.
        }

        // UI pass: records into `scene_color` (offscreen) with `LoadOp::Load`
        // after the world/fog/wireframe/debug-line passes, before the timing
        // resolve and submit — beneath the egui overlay (which draws in the
        // caller's separate submission).
        //
        // The gameplay path lays out the snapshot's descriptor tree (renderer
        // owns layout) and records its draw data. EMPTY-TREE EARLY-OUT: when the
        // snapshot carries no tree, or the tree lays out empty, the pass is
        // skipped entirely — no `begin_render_pass`. This is the gameplay-path-
        // only early-out (A follow-up #3); the boot splash is a separate
        // renderer-owned pass (`BootSplashPass`) that always clears the swapchain.
        let ui_viewport = [self.surface_config.width, self.surface_config.height];
        // Destructure boot (`device`/`queue`) + `full` once for the whole UI
        // region: the layout/focus-ring/encode/resolve statements interleave a
        // `&mut full.ui` (or `&mut full.screen_effects`) borrow with disjoint
        // `&full.ui_snapshot` / `&full.ui_theme` reads in single statements — the
        // `full_mut()` accessor borrows ALL of `self`, so it cannot coexist with
        // those argument reads. The destructure restores the disjoint-field
        // borrows the inline layout had. Closed before the submit/readback tail,
        // which calls `&mut self` helpers (`encode_sh_probe_readback`) and the
        // boot `queue.submit` that need `self` intact.
        let Self {
            device,
            queue,
            full,
            ..
        } = self;
        let full = full
            .as_mut()
            .expect("renderer full-init must complete before full-ready paths run");
        // Modal stack: lay out and record each layer bottom→top (`trees[0]` is the
        // bottom HUD, the last entry the top/active modal). Each layer keeps its
        // own retained tree + dirty gate, so a frozen lower layer recomputes
        // nothing while the top animates. Painter's order is the stack order: a
        // later layer's quads composite over the earlier ones into the same view
        // (LoadOp::Load). Empty/empty-laying-out layers early-out individually.
        let stack: Vec<ui::descriptor::AnchoredTree> = full
            .ui_snapshot
            .trees
            .iter()
            .map(|entry| entry.descriptor.clone())
            .collect();

        // Lay out EVERY layer first into owned draw data, THEN compose all layers
        // into a SINGLE `encode` call. The glyphon text half (`UiTextRenderer`) is
        // shared across layers and holds ONE vertex buffer it overwrites at offset
        // 0 on each `prepare`; `queue.write_buffer` resolves on the queue timeline
        // (last write wins) regardless of recording order, so issuing a separate
        // `encode` per layer makes EVERY layer's text draw read the LAST layer's
        // shaped glyphs — the readout-aliasing bug (a lower layer's text rendered
        // the top layer's glyphs). This mirrors the multi-batch quad-buffer clobber
        // already documented in `UiPass::encode`: one `prepare`/`render` per frame,
        // with all layers' glyphs concatenated in painter order, sidesteps it.
        let mut layer_draws: Vec<ui::tree::UiDrawData> = Vec::with_capacity(stack.len());
        for (layer, tree) in stack.iter().enumerate() {
            // Image sizes are optional for gameplay layers — an `image` node with
            // no size entry measures to zero. The boot splash sizes its logo in
            // its own `BootSplashPass`, independent of this gameplay path.
            // Bound text/panel nodes resolve against the snapshot's slot values
            // (disjoint field borrow from `&mut self.ui`). The cloned `stack`
            // above already released the snapshot, so this borrow is clean.
            let mut draw = full.ui.layout_gameplay_tree(
                layer,
                tree,
                ui_viewport,
                &ui::tree::ImageSizes::new(),
                &full.ui_snapshot.slot_values,
                &full.ui_snapshot.cell_values,
                &full.ui_theme,
                full.ui_theme_generation,
                full.ui_snapshot.time_seconds,
            );
            // Focus ring (M13 Goal F, Task 3): only the TOP layer takes focus, so
            // draw the engine ring around the focused node's rect on it. The
            // focused id rode in on the snapshot (resolved app-side last frame, so
            // it may trail a focus change by one frame). The ring is a `focus.ring`
            // bordered frame inset by the `xs` spacing token; appended to this
            // layer's quad list so it composites over the layer's own quads.
            let is_top = layer + 1 == stack.len();
            if is_top {
                if let Some(focused) = full.ui_snapshot.focused_id.as_deref() {
                    let focus_rects = full.ui.export_top_focus_rects(
                        ui_viewport,
                        &full.ui_snapshot.slot_values,
                        &full.ui_snapshot.cell_values,
                    );
                    if let Some(fr) = focus_rects.rects.iter().find(|r| r.id == focused) {
                        let inset = full.ui_theme.spacing("xs").unwrap_or(0.0)
                            * ui::layout::device_scale(ui_viewport);
                        let ring_color = full
                            .ui_theme
                            .color("focus.ring")
                            .unwrap_or([1.0, 0.0, 1.0, 1.0]);
                        ui::push_focus_ring(&mut draw.quads, fr.rect, inset, ring_color);
                    }
                }
            }
            layer_draws.push(draw);
        }

        // Fold every laid-out layer into ONE whole-frame composition (bottom→top
        // painter order) and record a SINGLE UI pass. The composition is the unit
        // of encoding — `encode` takes the whole composition, never one layer — so
        // the cross-layer glyphon clobber (every layer's text reading the last
        // layer's shaped glyphs) is unrepresentable. The white bind group is cloned
        // out first so the `&self.ui_images` borrow the fold takes can coexist with
        // the `&mut self.ui` encode call below.
        let white_bg = full.ui.white_bind_group().clone();
        let composition =
            ui::UiComposition::from_layer_draws(&layer_draws, &white_bg, &full.ui_images);
        if !composition.is_empty() {
            full.ui.encode(
                device,
                queue,
                &mut encoder,
                &scene_color,
                ui_viewport,
                wgpu::LoadOp::Load,
                &composition,
            );
        }
        // Drop retained state for any layers popped since last frame (stack
        // shrank), so freed modal trees release their layout cache.
        full.ui.truncate_gameplay_stack(stack.len());

        // Post-scene compositor resolve: blit `scene_color` into the swapchain
        // `view`, composing flash/vignette/shake from the frame's UI slot
        // snapshot on top. Encoded AFTER the UI pass and BEFORE the timing
        // resolve — the sole swapchain writer for the gameplay path, run every
        // frame (never skipped at rest). At-rest slot values pack to the identity
        // uniform, so the output stays byte-identical to the pre-SE blit.
        full.screen_effects.encode_resolve(
            queue,
            &mut encoder,
            &view,
            &full.ui_snapshot.slot_values,
        );

        if let Some(timing) = &full.frame_timing {
            timing.encode_resolve(&mut encoder);
        }

        // Last use of the UI-region destructure: the boot `queue` local submits.
        // After this statement NLL releases the `&mut self` reborrow, so the
        // submit/readback tail below may touch `self` again (the
        // `encode_sh_probe_readback` helper takes `&mut self`).
        queue.submit(std::iter::once(encoder.finish()));

        #[cfg(feature = "dev-tools")]
        self.encode_sh_probe_readback();

        {
            let Self { device, full, .. } = self;
            let full = full
                .as_mut()
                .expect("renderer full-init must complete before full-ready paths run");
            if let Some(timing) = full.frame_timing.as_mut() {
                timing.post_submit(device);
            }
        }

        // Drive the SH readback map and, when a frame's data has landed, swap it
        // into the probe-marker source so the next overlay frame shows live
        // (base + animated-delta) irradiance instead of the static bake.
        #[cfg(feature = "dev-tools")]
        {
            let Self { device, full, .. } = self;
            let full = full
                .as_mut()
                .expect("renderer full-init must complete before full-ready paths run");
            if let Some(live_irradiance) = full.sh_probe_readback.post_submit(device) {
                full.sh_volume_resources.probe_irradiance = live_irradiance;
            }
        }

        // Drive the candidate cull's deferred submitted-leaf counter readback so
        // the Spatial-tab "Submitted leaves" count reflects the GPU's own tally
        // (a few frames stale by design) instead of a per-frame CPU recompute.
        #[cfg(feature = "dev-tools")]
        {
            let Self { device, full, .. } = self;
            let full = full
                .as_mut()
                .expect("renderer full-init must complete before full-ready paths run");
            if let Some(candidate) = full.candidate_cull.as_mut() {
                candidate.post_submit(device);
            }
        }

        // Caller (`App`) presents after optionally appending the egui overlay
        // pass via `render_debug_ui`.
        Ok(Some(output))
    }
}
