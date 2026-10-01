// The per-frame indirect render orchestration: depth pre-pass, shadow passes,
// forward pass, mesh/smoke/fog passes, and submission.
// See: context/lib/rendering_pipeline.md §1

use super::cpu_stages::RenderStage;
use super::*;
use postretro_level_loader::{ShDrainBatch, ShDrainOutcome};

/// One frame entry's renderer-owned SH admission outcome and its subsequent
/// scene result. The outcome remains available when a later surface or scene
/// failure occurs, so the application can release or retain loader permits
/// exactly once before it propagates that failure.
#[derive(Debug)]
pub struct ShDrainFrameResult<T> {
    pub outcome: ShDrainOutcome,
    /// True only after all streamed compose encodes succeeded and their
    /// command buffer was submitted. The application uses this renderer fact,
    /// not surface acquisition or later readback success, to gate promotion.
    pub compose_submitted: bool,
    pub frame: std::result::Result<T, anyhow::Error>,
    /// Time spent in the surface texture request, when CPU timing is on and a
    /// request was made. The binary moves it out of render work into wait.
    pub acquire_nanos: Option<u64>,
}

// Must match the near/far the caller bakes into `view_proj`
// (`postretro::camera::{NEAR, FAR}`) — the fog pass reconstructs
// view-space depth by inverting that projection.
const RENDERER_NEAR_CLIP: f32 = 0.1;
const RENDERER_FAR_CLIP: f32 = 4096.0;

/// Select the non-empty mesh plans for their structurally separate pass paths.
/// Shadow-depth recording receives the world plan; the viewmodel plan is only
/// consumed by its dedicated forward pass.
pub(super) fn mesh_frame_plans_for_passes(
    plans: Option<&mesh_instances::MeshFramePlans>,
) -> (
    Option<&mesh_instances::MeshFramePlan>,
    Option<&mesh_instances::MeshFramePlan>,
) {
    let world = plans
        .filter(|plans| !plans.world.groups.is_empty())
        .map(|plans| &plans.world);
    let viewmodel = plans
        .filter(|plans| !plans.viewmodel.groups.is_empty())
        .map(|plans| &plans.viewmodel);
    (world, viewmodel)
}

impl Renderer {
    #[allow(clippy::too_many_arguments)]
    pub fn render_frame_indirect(
        &mut self,
        font_system: &mut postretro_ui::text::FontSystem,
        cam_vis: CameraCullVisibility<'_>,
        light_reachable_cell_mask: &[bool],
        reachable_cell_aabbs: &[(Vec3, Vec3)],
        fog_reachable: &[u32],
        sh_sample_regions: ShSampleRegionSets<'_>,
        camera_cell: Option<u32>,
        view_proj: Mat4,
        particle_collections: &[(&str, &[u8])],
        now_seconds: f64,
        clear_color: ClearColor,
        render_world: bool,
        sh_drain_batch: ShDrainBatch,
    ) -> std::result::Result<ShDrainFrameResult<Option<PresentHandle>>, ShResidencyDrainError> {
        // The binary commits after its option writes and before building the
        // camera, so this is always a no-op on gameplay and frontend frames. A
        // change here means an extent was recorded after the camera was built.
        if self.commit_extents().is_some() {
            log::warn!(
                "[Renderer] render extents changed after the camera was built; this frame's projection uses the previous aspect"
            );
        }
        // This is the sole loader→renderer admission point for a windowed
        // frame. It precedes surface acquisition so even a skipped frame
        // returns the ownership outcome to the session controller.
        self.cpu_frame.clear();
        // A splash acquire leaves its time behind; only this frame's counts.
        self.last_acquire_nanos = None;
        let cpu = std::rc::Rc::clone(&self.cpu_frame);
        let drain_scope = cpu.scope(RenderStage::ShDrain);
        let outcome = self.drain_sh_residency(sh_drain_batch)?;
        drop(drain_scope);
        let mut compose_submitted = false;
        let frame = (|| -> Result<Option<PresentHandle>> {
            let Some(handle) = self.acquire_present_handle("gameplay frame")? else {
                self.prepare_streamed_sh_compose(
                    sh_sample_regions,
                    None,
                    false,
                    fog_reachable.is_empty(),
                    false,
                )?;
                self.queue.flush_skipped_frame();
                return Ok(None);
            };
            let view = handle.surface_view();
            let mut encoder = self
                .device
                .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                    label: Some("Frame Encoder"),
                });

            compose_submitted = self.record_scene_passes(
                &mut encoder,
                Some(font_system),
                Some(&view),
                cam_vis,
                light_reachable_cell_mask,
                reachable_cell_aabbs,
                fog_reachable,
                sh_sample_regions,
                camera_cell,
                view_proj,
                particle_collections,
                &[],
                now_seconds,
                clear_color,
                render_world,
            )?;
            {
                let _submit = cpu.scope(RenderStage::Submit);
                self.submit_windowed_frame(encoder);
            }

            // Caller (`App`) presents after optionally appending the egui overlay
            // pass via `render_debug_ui`.
            Ok(Some(handle))
        })();
        if frame.is_ok() {
            self.queue.complete_frame();
        }
        Ok(ShDrainFrameResult {
            outcome,
            compose_submitted,
            frame,
            acquire_nanos: self.last_acquire_nanos.take(),
        })
    }

    /// Record the world-scene passes shared by windowed gameplay and offscreen
    /// capture. Window-only UI/debug/resolve work runs only when both windowed
    /// inputs are supplied.
    #[allow(clippy::too_many_arguments)]
    pub(super) fn record_scene_passes(
        &mut self,
        encoder: &mut wgpu::CommandEncoder,
        font_system: Option<&mut postretro_ui::text::FontSystem>,
        swapchain_view: Option<&wgpu::TextureView>,
        cam_vis: CameraCullVisibility<'_>,
        light_reachable_cell_mask: &[bool],
        reachable_cell_aabbs: &[(Vec3, Vec3)],
        fog_reachable: &[u32],
        sh_sample_regions: ShSampleRegionSets<'_>,
        camera_cell: Option<u32>,
        view_proj: Mat4,
        particle_collections: &[(&str, &[u8])],
        capture_animated_promotion_weights: &[(usize, f32)],
        now_seconds: f64,
        clear_color: ClearColor,
        render_world: bool,
    ) -> Result<bool> {
        // The drawable visible-cell set; candidate-cull eligibility derives
        // from `cam_vis` (set + path provenance) inside `record_pre_scene_compute`.
        let visible: &VisibleCells = cam_vis.cells;
        // One CPU scope per pass; the record scope spans the whole call,
        // including capture's early return.
        let cpu = std::rc::Rc::clone(&self.cpu_frame);
        let _record_scope = cpu.scope(RenderStage::Record);
        let mesh_plan_scope = cpu.scope(RenderStage::MeshPlan);

        // Before any pass requests timestamps: the frame's resolve reads
        // every query slot, so every slot must be written in this encoder.
        if let Some(timing) = self.full_mut().frame_timing.as_mut() {
            timing.begin_frame(encoder);
        }

        self.full_mut().debug_frame = self.full().debug_frame.wrapping_add(1);
        let frame_light_term_mask = self.frame_light_term_mask();
        let mut compose_succeeded = true;

        // The readback copy is deliberately not encoded here. A
        // `copy_texture_to_buffer` in the same command buffer as the compose
        // dispatch reads the `total` atlas texture before its storage writes
        // are visible, flickering garbage into the markers. It runs after a
        // blocking `poll(Wait)` below, once the compose submit has retired.

        // Selected-static promotion gates on the renderable/planned mesh set, not
        // raw collected inputs: uncached models and budget overflow cannot promote
        // a light that would have no entity depth draw this frame.
        let selected_static_needs_mesh_gate = if render_world {
            let full = self.full();
            full.shadow_candidate_lights
                .iter()
                .enumerate()
                .any(|(candidate_index, light)| {
                    shadow_candidate_is_promoted_baked(
                        &full.shadow_candidate_selection_indices,
                        &full.shadow_candidate_animated_baked_indices,
                        candidate_index,
                    ) && (light.light_type == postretro_level_loader::LightType::Spot
                        || (light.light_type == postretro_level_loader::LightType::Point
                            && full.cube_shadow_pool.is_some()))
                })
        } else {
            false
        };
        // Plan both control-flow partitions together before shadow-slot
        // assignment. They share one palette/instance SSBO allocation, so two
        // independent plans would overlap their bases. Static-light promotion
        // deliberately sees only the world partition.
        let mesh_frame_plans: Option<mesh_instances::MeshFramePlans> = if render_world
            && self.full().mesh_pass.has_model()
            && !self.full().mesh_draws.is_empty()
        {
            Some(mesh_instances::plan_mesh_frame_plans(
                &self.full().mesh_draws,
                &self.full().mesh_pass,
            ))
        } else {
            None
        };
        let promotion_mesh_frame_plan = selected_static_needs_mesh_gate
            .then(|| mesh_frame_plans.as_ref().map(|plans| &plans.world))
            .flatten();
        drop(mesh_plan_scope);
        if render_world {
            let light_slots_scope = cpu.scope(RenderStage::LightSlots);
            // mem::take avoids a simultaneous borrow of self; returned after call
            // to reuse the allocation.
            let eff_brightness = std::mem::take(&mut self.full_mut().light_effective_brightness);
            let animated_window_brightness =
                std::mem::take(&mut self.full_mut().animated_light_window_brightness);
            let last_camera_position = self.full().last_camera_position;
            self.update_dynamic_light_slots_with_capture_overrides(
                last_camera_position,
                crate::lighting::spot_shadow::SHADOW_NEAR_CLIP,
                &eff_brightness,
                &animated_window_brightness,
                reachable_cell_aabbs,
                now_seconds,
                promotion_mesh_frame_plan,
                capture_animated_promotion_weights,
            )?;
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
                    &animated_window_brightness,
                    camera_cell,
                );
            }
            self.full_mut().light_effective_brightness = eff_brightness;
            self.full_mut().animated_light_window_brightness = animated_window_brightness;
            drop(light_slots_scope);

            let _pre_scene_scope = cpu.scope(RenderStage::PreScene);
            {
                let _prep_scope = cpu.scope(RenderStage::ShComposePrep);
                self.prepare_streamed_sh_compose(
                    sh_sample_regions,
                    mesh_frame_plans.as_ref(),
                    swapchain_view.is_some(),
                    fog_reachable.is_empty(),
                    true,
                )?;
            }
            compose_succeeded &= self.record_pre_scene_compute(
                encoder,
                cam_vis,
                view_proj,
                true,
                frame_light_term_mask,
            );
            let direct_scope = cpu.scope(RenderStage::DirectShCompose);
            compose_succeeded &= self.record_direct_sh_pre_scene_compute(encoder);
            drop(direct_scope);
        } else {
            let _pre_scene_scope = cpu.scope(RenderStage::PreScene);
            compose_succeeded &= self.record_pre_scene_compute(
                encoder,
                cam_vis,
                view_proj,
                false,
                frame_light_term_mask,
            );
        }

        // --- Skinned-mesh pose/upload HOIST ----------------------------------
        // Plan + sample + upload the skinned-mesh palette/instance buffers HERE —
        // after `update_dynamic_light_slots`, BEFORE the spot-shadow depth loop —
        // so the skinned-depth shadow occluder pass and the forward mesh draw both
        // read the SAME already-posed buffers. Nothing rewrites `palette_buffer`/
        // `instance_buffer` between this point and the forward `record_draws`, so
        // an entity and its shadow are sampled at the identical pose (no one-frame
        // lag). The world and viewmodel plans are held in `mesh_frame_plans`;
        // both upload into the same buffers, but only the world partition reaches
        // shadow depth below.
        if let Some(plans) = &mesh_frame_plans {
            let _mesh_upload_scope = cpu.scope(RenderStage::MeshUpload);
            // Overflow drops excess instances rather than corrupting the
            // palette or panicking — rate-limited warning. Covers BOTH the
            // palette-slot cap and the instance-count cap. Rigid / zero-joint
            // props still reserve one identity palette entry, so either cap
            // can reject them.
            let dropped = plans.world.dropped.saturating_add(plans.viewmodel.dropped);
            if dropped > 0 {
                let now = now_seconds as f32;
                if now - self.full().mesh_overflow_last_warn >= 1.0 {
                    log::warn!(
                        "[Renderer] skinned-mesh budget exceeded: dropped {} instance(s) \
                             (budget {} palette slots / {} instances); excess not drawn",
                        dropped,
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
                full.mesh_pass.plan_and_upload(
                    queue,
                    &[&plans.world, &plans.viewmodel],
                    &mut full.bone_palette_scratch,
                    &cpu,
                );
            }
        }
        let (world_mesh_frame_plan, viewmodel_mesh_frame_plan) =
            mesh_frame_plans_for_passes(mesh_frame_plans.as_ref());

        // Both mesh passes share one params buffer and the same frame values.
        if render_world && (world_mesh_frame_plan.is_some() || viewmodel_mesh_frame_plan.is_some())
        {
            {
                let frame_light_term_mask = self.frame_light_term_mask();
                let Self { queue, full, .. } = self;
                let full = full
                    .as_mut()
                    .expect("renderer full-init must complete before full-ready paths run");
                full.mesh_pass.write_light_params(
                    queue,
                    full.total_light_count,
                    full.light_count,
                    full.light_count + full.animated_baked_light_count as u32,
                    full.mesh_dynamic_time,
                    frame_light_term_mask.bits(),
                    full.ambient_floor,
                );
            }
        }

        if !render_world {
            let full = self.full_mut();
            full.spot_entity_occluders_submitted = 0;
            full.dynamic_depth_cache_diagnostics.frame = Default::default();
            full.promoted_depth_cache_frame_plan = PromotedDepthCacheFramePlan::default();
            full.promoted_depth_cache_promoted_count = 0;
            full.promoted_depth_cache_world_render_skips = 0;
            full.promoted_depth_cache_cull_dispatch_skips = 0;
            full.promoted_entity_occluders_submitted = 0;
            full.promoted_depth_cache_timing_open = false;
        }

        let shadow_scope = cpu.scope(RenderStage::ShadowDepth);
        if render_world {
            self.record_spot_shadow_depth(encoder, world_mesh_frame_plan);
        }

        // --- Cube point-light shadow depth loop -------------------------------
        // For each occupied cube slot, CLEAR all 6 live-pool faces to the far
        // plane (1.0). Uncached slots render cone-culled WORLD geometry plus
        // eligible entity occluders. Cached slots render static world only into
        // a cold cache layer, then clear and redraw live entity occluders in the
        // pool. Per face: a depth render pass into the
        // `slot*6 + face` D2Array view, projecting by that face's light-space
        // matrix (group 0, dynamic offset into the cube VS uniform buffer). The
        // world draw pulls from that face's `cube_shadow_cull` indirect
        // sub-region (per-face 90° frustum, all-cells); entity instances are
        // CPU-culled inside `record_skinned_depth` against the same planes.
        // Reuses the SAME depth pipeline as the spot path.
        //
        // CRITICAL: the per-face Clear(1.0) baseline must run for EVERY occupied
        // face regardless of whether any occluder draws happen this frame.
        // Gating the whole loop on `mesh_frame_plan` being `Some` (the prior
        // bug) meant that when no mesh entity was in the PVS — e.g. a combat
        // arena whose meshes are all off-screen — the occupied faces were NEVER
        // cleared and held stale/uninitialized depth (~0.0). An on-screen point
        // light then sampled that garbage and read fully shadowed
        // (CompareFunction::Less: reference >= 0 is never < 0), zeroing its
        // world illumination. Off-screen lights own no slot (sentinel), so they
        // stayed lit — the view-dependent symptom. The clear stays
        // unconditional; the world and entity draws are the only gated steps.
        // Cached slots retain the same live-pool clear baseline while their
        // warm cache skips static-world work.
        self.full_mut().cube_entity_occluders_submitted = 0;
        if render_world {
            self.record_cube_shadow_depth(encoder, world_mesh_frame_plan);
        }

        drop(shadow_scope);

        {
            let _depth_sdf_scope = cpu.scope(RenderStage::DepthSdf);
            self.record_depth_and_sdf_passes(encoder, view_proj, render_world);
        }

        // Post-scene compositor seam: every gameplay scene pass renders into
        // `scene_color` (the offscreen target) instead of the swapchain `view`;
        // UI records separately into its own native-res layer.
        // The resolve pass below is the sole swapchain writer for the gameplay
        // path. The view is cloned (wgpu `TextureView` is `Arc`-backed) into an
        // OWNED handle so it no longer borrows `self.full()` — the post-split
        // `full()`/`full_mut()` accessors borrow ALL of `self`, so holding a
        // borrow of `screen_effects` across the later `&mut self` pass/helper
        // calls (debug_lines, wireframe overlay) would conflict. The owned
        // clone preserves the disjoint-borrow behavior the inline-field layout
        // had. The splash path is unaffected — it writes the swapchain directly
        // and never touches this target.
        let scene_color = self.full().screen_effects.scene_color_view().clone();

        {
            let _forward_scope = cpu.scope(RenderStage::Forward);
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
                render_pass.set_bind_group(3, self.full().sh_bind_group(), &[]);
                render_pass.set_bind_group(4, &self.full().lightmap_resources.bind_group, &[]);
                render_pass.set_bind_group(5, &self.full().spot_shadow_pool.bind_group, &[]);
                render_pass.set_bind_group(
                    LIGHTMAP_BLOCK_TABLE_GROUP,
                    &self.full().lightmap_resources.block_table_bind_group,
                    &[],
                );
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

        // Kinematic brush movers: local-space PRL brush payloads drawn as
        // dynamic objects after opaque world geometry and before skinned meshes.
        // They write depth and use the mesh dynamic-object lighting bindings
        // (baked SH indirect/static direct + runtime dynamic direct).
        if render_world && self.full().kinematic_brush.has_draws() {
            let _kinematic_scope = cpu.scope(RenderStage::KinematicBrush);
            let frame_light_term_mask = self.frame_light_term_mask();
            {
                let Self { queue, full, .. } = self;
                let full = full
                    .as_mut()
                    .expect("renderer full-init must complete before full-ready paths run");
                // Total records bound the loop; the dynamic-tier count marks the promoted specular tail.
                full.kinematic_brush.write_light_params(
                    queue,
                    full.total_light_count,
                    full.light_count,
                    full.light_count + full.animated_baked_light_count as u32,
                    full.mesh_dynamic_time,
                    frame_light_term_mask.bits(),
                    full.ambient_floor,
                );
            }
            let mut mover_pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("Kinematic Brush Pass"),
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
            mover_pass.set_bind_group(0, &self.full().uniform_bind_group, &[]);
            mover_pass.set_bind_group(4, self.full().sh_mesh_bind_group(), &[]);
            self.full()
                .kinematic_brush
                .record_draws(&mut mover_pass, &self.full().gpu_textures);
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
            if let Some(plan) = world_mesh_frame_plan {
                let _skinned_scope = cpu.scope(RenderStage::SkinnedMesh);
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
                mesh_enc.set_bind_group(4, self.full().sh_mesh_bind_group(), &[]);
                self.full_mut().mesh_pass.record_draws(&mut mesh_enc, plan);
            }
        }

        // After opaque forward, before wireframe. Alpha additive; depth test on, write off.
        if render_world
            && self.full().smoke_pass.has_any_sheet()
            && !particle_collections.is_empty()
        {
            let _smoke_scope = cpu.scope(RenderStage::Smoke);
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
            smoke_pass_enc.set_bind_group(3, self.full().sh_bind_group(), &[]);
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
        let fog_scope = cpu.scope(RenderStage::Fog);
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
                    RENDERER_NEAR_CLIP,
                    RENDERER_FAR_CLIP,
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
                raymarch.set_bind_group(3, self.full().sh_bind_group(), &[]);
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

        drop(fog_scope);

        // Bloom samples HDR scene color after fog and adds the blurred bright
        // contribution back before capture. The capture return below therefore
        // sees bloom, while wireframe/debug/viewmodel/UI remain un-bloomed.
        if self.full().bloom.enabled() {
            let _bloom_scope = cpu.scope(RenderStage::Bloom);
            let full = self
                .full
                .as_ref()
                .expect("renderer full-init must complete before full-ready paths run");
            if let Some(timing) = &full.frame_timing {
                timing.write_encoder_start(encoder, TIMING_PAIR_BLOOM);
            }
            full.bloom.record(encoder, &scene_color);
            if let Some(timing) = &full.frame_timing {
                timing.write_encoder_end(encoder, TIMING_PAIR_BLOOM);
            }
        }

        // Offscreen capture stops after fog and bloom. The windowed-only
        // wireframe/debug/viewmodel/UI/resolve tail must not enter capture bytes.
        let Some(view) = swapchain_view else {
            // Capture still records the streamed SH compose work above; its
            // separate submission path uses this result to decide whether that
            // compose may be promoted after the command buffer retires.
            return Ok(compose_succeeded);
        };
        let font_system =
            font_system.expect("windowed gameplay rendering requires a UI font system");

        let overlay_scope = cpu.scope(RenderStage::Overlay);
        self.record_wireframe_overlay(encoder, &scene_color, render_world, visible);

        #[cfg(feature = "dev-tools")]
        if render_world {
            let Self { queue, full, .. } = self;
            let full = full
                .as_mut()
                .expect("renderer full-init must complete before full-ready paths run");
            full.debug_lines.render(
                queue,
                encoder,
                &scene_color,
                &full.depth_view,
                &full.uniform_bind_group,
            );
            // Buffer is cleared by the frame loop (via `clear_debug_lines`)
            // before the next frame's emit call — that single owner handles
            // surface Timeout/Occluded/Outdated early-returns above without
            // leaking segments across frames.
        }

        drop(overlay_scope);

        // First-person weapon presentation is deliberately last among scene
        // geometry: clear the shared depth attachment so nearby world surfaces
        // cannot clip it, then draw only the structurally separate viewmodel
        // plan. Running after fog/wireframe/debug also preserves their world-depth
        // interpretation; UI remains above this pass as usual.
        if render_world {
            if let Some(plan) = viewmodel_mesh_frame_plan {
                let _viewmodel_scope = cpu.scope(RenderStage::Viewmodel);
                let mut viewmodel_pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                    label: Some("Skinned Viewmodel Pass"),
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
                            load: wgpu::LoadOp::Clear(1.0),
                            store: wgpu::StoreOp::Store,
                        }),
                        stencil_ops: None,
                    }),
                    timestamp_writes: None,
                    ..Default::default()
                });
                viewmodel_pass.set_bind_group(
                    0,
                    self.full().mesh_pass.viewmodel_uniform_bind_group(),
                    &[],
                );
                viewmodel_pass.set_bind_group(4, self.full().sh_mesh_bind_group(), &[]);
                self.full_mut()
                    .mesh_pass
                    .record_draws(&mut viewmodel_pass, plan);
            }
        }

        // Game UI records into its native-res layer, never into `scene_color`,
        // so the tonemap and scene-only effects leave it untouched.
        {
            let _ui_scope = cpu.scope(RenderStage::Ui);
            self.record_ui_layer(encoder, font_system);
        }

        // Resolve: upscale + tonemap + scene effects, then composite the UI
        // layer. This is the gameplay path's sole swapchain writer and runs
        // even when screen effects are at rest; timing query resolution
        // follows it.
        let _resolve_scope = cpu.scope(RenderStage::Resolve);
        let scene_divisor = self.render_extents().divisor;
        let Self { queue, full, .. } = self;
        let full = full
            .as_mut()
            .expect("renderer full-init must complete before full-ready paths run");
        let resolve_timestamps = full
            .frame_timing
            .as_ref()
            .map(|t| t.render_pass_writes(TIMING_PAIR_RESOLVE));
        full.screen_effects.encode_resolve(
            queue,
            encoder,
            view,
            &full.ui_snapshot.slot_values,
            full.limiter_frame.take(),
            scene_divisor,
            resolve_timestamps,
        );

        if let Some(timing) = &mut full.frame_timing {
            timing.encode_resolve(encoder);
        }

        Ok(compose_succeeded)
    }

    /// Submit a windowed frame after its scene, UI, and resolve commands have
    /// been recorded. Capture owns a separate submit/readback sequence.
    pub(super) fn submit_windowed_frame(&mut self, encoder: wgpu::CommandEncoder) {
        self.queue.submit(std::iter::once(encoder.finish()));
        self.queue.assert_empty("drawn frame submit");
        self.full_mut().ui.mark_submitted();

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
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn drain_outcome_survives_a_later_frame_failure() {
        let result: ShDrainFrameResult<()> = ShDrainFrameResult {
            outcome: ShDrainOutcome {
                accepted: vec![4],
                dropped: vec![9],
                deferred: Vec::new(),
                evicted: Vec::new(),
            },
            compose_submitted: false,
            frame: Err(anyhow::anyhow!("surface acquisition failed")),
            acquire_nanos: None,
        };

        assert_eq!(result.outcome.accepted, vec![4]);
        assert_eq!(result.outcome.dropped, vec![9]);
        assert!(!result.compose_submitted);
        assert!(result.frame.is_err());
    }

    #[test]
    fn windowed_entry_drains_once_before_surface_or_scene_work() {
        let source = include_str!("renderer_render_frame.rs");
        let entry = source
            .find("pub fn render_frame_indirect(")
            .expect("windowed renderer entry must remain present");
        let end = source[entry..]
            .find("    /// Record the world-scene")
            .map(|offset| entry + offset)
            .expect("windowed renderer entry must end before scene helper");
        let body = &source[entry..end];
        let drain = body
            .find("self.drain_sh_residency(sh_drain_batch)")
            .expect("windowed entry must accept its drain batch");
        let acquire = body
            .find("self.acquire_present_handle")
            .expect("windowed entry must acquire its surface");
        let scene = body
            .find("self.record_scene_passes(")
            .expect("windowed entry must record scene passes");

        assert_eq!(
            body.matches("self.drain_sh_residency(sh_drain_batch)")
                .count(),
            1,
            "the loader batch has exactly one renderer admission point"
        );
        assert!(
            drain < acquire && acquire < scene,
            "draining must precede surface acquisition and scene composition"
        );
    }

    // Regression: resolving timestamp queries no pass wrote that frame
    // device-lost NVIDIA/Vulkan under POSTRETRO_GPU_TIMING=1.
    #[test]
    fn scene_recording_prefills_timing_queries_before_any_pass_or_resolve() {
        let source = include_str!("renderer_render_frame.rs");
        let entry = source
            .find("pub(super) fn record_scene_passes(")
            .expect("scene recording helper must remain present");
        // Production code only, so a call removed from the helper cannot be
        // found in this test's own string literals instead.
        let tests = source
            .find("#[cfg(test)]")
            .expect("test module must remain present");
        let body = &source[entry..tests];
        let prefill = body
            .find("timing.begin_frame(encoder)")
            .expect("scene recording must prefill the timing query set");
        for pass in [
            "self.record_pre_scene_compute(",
            "self.record_direct_sh_pre_scene_compute(",
            "self.record_spot_shadow_depth(",
            "self.record_cube_shadow_depth(",
            "self.record_depth_and_sdf_passes(",
            "render_pass_writes(",
            "write_encoder_start(",
            "render_pass_writes(TIMING_PAIR_RESOLVE)",
            "timing.encode_resolve(encoder)",
        ] {
            let at = body
                .find(pass)
                .unwrap_or_else(|| panic!("scene recording must still call `{pass}`"));
            assert!(
                prefill < at,
                "the timing prefill must precede `{pass}`: every timestamped pass and the resolve"
            );
        }
    }

    /// The resolve reports its own GPU timing entry under
    /// `POSTRETRO_GPU_TIMING=1`.
    #[test]
    fn the_resolve_owns_a_labeled_timing_pair() {
        const { assert!(TIMING_PAIR_RESOLVE < TIMING_PAIR_COUNT) }
        let labels = include_str!("renderer_init_resources.rs");
        let line = "pass_labels[TIMING_PAIR_RESOLVE] = \"resolve\"";
        assert!(labels.contains(line), "missing timing label `{line}`");
    }

    #[test]
    fn billboard_scatter_compose_is_after_shared_descriptor_flush_and_before_sprite_draw() {
        let frame_update = include_str!("renderer_frame.rs");
        assert_eq!(
            frame_update
                .matches("upload_descriptors_if_dirty(queue)")
                .count(),
            1,
            "animated direct-SH and direct scatter must share one descriptor flush"
        );

        let pre_scene = include_str!("renderer_pre_scene.rs");
        let direct = pre_scene
            .find("full.direct_sh_compose.dispatch_if_needed(")
            .expect("direct-SH compose dispatch must remain present");
        let scatter = pre_scene
            .find("full.billboard_direct_scatter_compose.dispatch_if_needed(")
            .expect("scatter compose dispatch must be recorded");
        assert!(
            direct < scatter,
            "direct-SH composition must precede billboard scatter composition"
        );

        let render = include_str!("renderer_render_frame.rs");
        let slot_assignment = render
            .find("self.update_dynamic_light_slots_with_capture_overrides(")
            .expect("dynamic-light slot assignment must remain present");
        let direct_pre_scene = render
            .find("self.record_direct_sh_pre_scene_compute(encoder)")
            .expect("direct/scatter pre-scene helper must be recorded");
        let sprites = render
            .find("label: Some(\"Billboard Sprite Pass\")")
            .expect("billboard draw pass must remain present");
        assert!(
            slot_assignment < direct_pre_scene && direct_pre_scene < sprites,
            "shared descriptors must flush, then direct/scatter composition must finish after slot assignment and before the first billboard draw"
        );
    }
}
