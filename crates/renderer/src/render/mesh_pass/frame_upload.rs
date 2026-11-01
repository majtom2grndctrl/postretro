// Skinned-mesh per-frame uploads: instance SSBO, bone palette and viewmodel camera.
// See: context/lib/rendering_pipeline.md §9

use std::collections::HashMap;

use postretro_model::BonePaletteEntry;
use postretro_model::sample_params::{FadeSource, SnapshotTag};
use postretro_render_cpu::ghost_instance::GhostInstanceLook;
use postretro_render_cpu::mesh_instances::{MAX_PALETTE_ENTRIES, MeshFramePlan};

use super::pose::{InstancePoseSample, sample_instance};
use super::{BONE_PALETTE_ENTRY_SIZE, INSTANCE_ENTRY_SIZE, LevelModels, MeshPass};
use crate::render::UNIFORM_SIZE;

/// Pack one instance's SSBO bytes (model matrix column-major + base index +
/// receiver-bias scale). The scale occupies `Instance.base_and_pad.y` as f32
/// bits, preserving the WGSL record's 80-byte stride. A ghost's look packs into
/// the otherwise-zero `zw` lanes (`skinned_ghost.wgsl`).
pub(super) fn build_instance_entry(
    model: glam::Mat4,
    base_index: u32,
    shadow_bias_scale: f32,
    ghost_look: Option<GhostInstanceLook>,
) -> [u8; INSTANCE_ENTRY_SIZE] {
    let mut bytes = [0u8; INSTANCE_ENTRY_SIZE];
    let cols = model.to_cols_array();
    for (i, v) in cols.iter().enumerate() {
        let off = i * 4;
        bytes[off..off + 4].copy_from_slice(&v.to_ne_bytes());
    }
    // Base index at offset 64 (x) and bias scale bits at 68 (y). z/w carry a
    // ghost's look and stay zero otherwise.
    bytes[64..68].copy_from_slice(&base_index.to_ne_bytes());
    bytes[68..72].copy_from_slice(&shadow_bias_scale.to_bits().to_ne_bytes());
    if let Some(look) = ghost_look {
        let [z, w] = look.pack();
        bytes[72..76].copy_from_slice(&z.to_ne_bytes());
        bytes[76..80].copy_from_slice(&w.to_ne_bytes());
    }
    bytes
}

impl MeshPass {
    /// Initialize the shared bone palette to identity (bind pose) before the
    /// first sampled frame, so any un-sampled run renders in bind pose rather
    /// than reading uninitialized buffer memory.
    pub fn upload_identity_palette(&self, queue: &wgpu::Queue) {
        let identity = BonePaletteEntry {
            matrix: glam::Mat4::IDENTITY.to_cols_array_2d(),
        };
        let entries = vec![identity; MAX_PALETTE_ENTRIES];
        queue.write_buffer(&self.palette_buffer, 0, bytemuck::cast_slice(&entries));
    }

    /// Plan-sample-upload step: write this frame's per-instance SSBO entries and
    /// sample every instance's clip into its bone-palette run. NO draws recorded.
    ///
    /// This is the pose/upload HOIST: the renderer runs it AFTER
    /// `update_dynamic_light_slots` and BEFORE the spot-shadow depth loop, so the
    /// skinned-depth pass (shadow occluders) and the forward mesh draw both read
    /// the SAME already-posed `palette_buffer`/`instance_buffer`. Nothing rewrites
    /// these buffers between the shadow loop and the forward draw, so there is no
    /// one-frame pose lag between an entity and its shadow.
    ///
    /// For each planned instance: pack its SSBO entry (model matrix + palette
    /// base), evaluate any one-time snapshot-capture instruction into the
    /// per-entity snapshot store, then sample its pose into the palette at that
    /// base per the instance's resolved [`MeshSampleParams`] — a single clip
    /// ([`postretro_model::anim::sample_clip_looped`]), a clip→clip blend, or a
    /// snapshot→clip blend. All sample times arrive in the params (the collector
    /// computed them from the animation clock), so the pass holds no render-clock
    /// of its own. The optional pose-sampling measurement uses an `Instant`, not
    /// the render clock.
    ///
    /// Snapshot-store lifecycle: a capture installs/refreshes an entry (idempotent
    /// by tag), then frame-end retention keeps only entries for entities with an
    /// active matching snapshot fade in the current plan. A snapshot fade whose
    /// store entry is missing (its capture frame was culled / budget-dropped)
    /// degrades to the fallback clip — a discontinuity no one saw because the
    /// entity was not drawn at the interrupt instant.
    ///
    /// Cull is the caller's job — see [`postretro_render_cpu::mesh_pass::mesh_visible`];
    /// the plan already holds only surviving, in-budget instances.
    pub fn plan_and_upload(
        &mut self,
        queue: &crate::render::uploads::UploadQueue,
        models: &LevelModels,
        plans: &[&MeshFramePlan],
        scratch: &mut Vec<BonePaletteEntry>,
        cpu: &postretro_stage_timing::StageFrame<crate::render::cpu_stages::RenderStage>,
    ) {
        if plans.iter().all(|plan| plan.groups.is_empty()) {
            self.snapshot_store
                .retain_active_snapshot_fades(&HashMap::new());
            self.palette_cache.end_frame();
            return;
        }

        // Disjoint field borrows: the capture step mutates `snapshot_store` +
        // `capture_scratch` while reading the level's models; the sample step
        // reads `snapshot_store`. Destructuring lets the borrow checker see they
        // are distinct fields (a `self.method` call would borrow all of `self`).
        let Self {
            snapshot_store,
            palette_cache,
            capture_scratch,
            instance_buffer,
            palette_buffer,
            ..
        } = self;

        let mut active_snapshot_fades: HashMap<u32, SnapshotTag> = HashMap::new();

        for plan in plans {
            for group in &plan.groups {
                let Some(entry) = models.entry(&group.model) else {
                    // Planner only emits groups for cached models, but guard anyway.
                    continue;
                };
                let model = &entry.uploaded;
                let resolve_clip = |idx: usize| entry.clips.get(idx);

                for (i, inst) in group.instances.iter().enumerate() {
                    let instance_index = group.instance_offset as usize + i;
                    let entry = build_instance_entry(
                        inst.transform,
                        inst.palette_base,
                        inst.shadow_bias_scale,
                        inst.ghost_look,
                    );
                    queue.write_buffer(
                        instance_buffer,
                        (instance_index * INSTANCE_ENTRY_SIZE) as u64,
                        &entry,
                    );

                    // Evaluate the one-time `"smooth"` capture (if any) into the store
                    // BEFORE sampling, so this frame's snapshot fade resolves against
                    // it. Idempotent by tag — a re-emission evaluates nothing.
                    if let Some(capture) = &inst.capture {
                        snapshot_store.apply_capture(
                            capture,
                            &model.skeleton,
                            resolve_clip,
                            capture_scratch,
                        );
                    }

                    // Retention mark: the capture above just installed the matching
                    // entry on a capture frame, so this frame's snapshot fade can
                    // sample it below and keep it at frame end. Missing/stale tags are
                    // left unmarked and will fall back during sampling, then evict.
                    if let Some(FadeSource::Snapshot { tag, .. }) = inst.sample.fade.map(|f| f.from)
                        && snapshot_store.matching(inst.phase_seed, tag).is_some()
                    {
                        active_snapshot_fades.insert(inst.phase_seed, tag);
                    }

                    // Time-slicing decision. Sample when the collector asked
                    // for a resample OR the cache misses (a re-entering instance with
                    // no cached run must sample, never show a stale pose). Otherwise
                    // re-upload the cached run with no sampling.
                    if palette_cache.must_sample(inst.palette_cache_key, inst.resample) {
                        // RESAMPLE: sample this instance's pose, upload it, and refresh
                        // the cache with the freshly sampled run.
                        let sample_scope =
                            cpu.scope(crate::render::cpu_stages::RenderStage::MeshPoseSampling);
                        let sampled = sample_instance(
                            InstancePoseSample {
                                params: &inst.sample,
                                pose_inputs: inst.pose_inputs.as_ref(),
                                seed: inst.phase_seed,
                            },
                            &model.skeleton,
                            &model.pose_stack,
                            snapshot_store,
                            &resolve_clip,
                            scratch,
                        );
                        drop(sample_scope);
                        cpu.add_count(crate::render::cpu_stages::RenderStage::MeshPoseSamples, 1);
                        if sampled && !scratch.is_empty() {
                            queue.write_buffer(
                                palette_buffer,
                                inst.palette_base as u64 * BONE_PALETTE_ENTRY_SIZE as u64,
                                bytemuck::cast_slice(scratch),
                            );
                            // Refresh the cache so a future skipped frame re-uploads
                            // THIS pose. Reuses the entry's `Vec` storage in place.
                            palette_cache.store(inst.palette_cache_key, scratch);
                        }
                    } else if let Some(cached) = palette_cache.touch_cached(inst.palette_cache_key)
                    {
                        // SKIP: re-upload the cached run at this frame's palette base
                        // (the base can move frame to frame as the dense plan repacks).
                        // No sampling, no allocation.
                        if !cached.is_empty() {
                            queue.write_buffer(
                                palette_buffer,
                                inst.palette_base as u64 * BONE_PALETTE_ENTRY_SIZE as u64,
                                bytemuck::cast_slice(cached),
                            );
                        }
                    }
                }
            }
        }

        // Evict cache entries not touched this frame, so the cache holds exactly
        // this frame's planned instances (bounded by MAX_INSTANCES / the palette
        // budget) and a culled-out entity's stale run does not linger.
        palette_cache.end_frame();

        // Evict snapshots after sampling, not before: a capture frame's snapshot
        // fade must resolve against the capture that landed earlier in this pass.
        snapshot_store.retain_active_snapshot_fades(&active_snapshot_fades);
    }

    /// Upload the tight view-projection used exclusively by the viewmodel pass.
    /// `skinned_mesh.wgsl` reads only the leading `mat4x4`, while the buffer keeps
    /// the existing group-0 allocation size/layout for bind-group compatibility.
    pub(in crate::render) fn write_viewmodel_view_projection(
        &self,
        queue: &crate::render::uploads::UploadQueue,
        view_projection: glam::Mat4,
    ) {
        let mut data = [0u8; UNIFORM_SIZE];
        for (index, value) in view_projection.to_cols_array().iter().enumerate() {
            let offset = index * std::mem::size_of::<f32>();
            data[offset..offset + std::mem::size_of::<f32>()].copy_from_slice(&value.to_ne_bytes());
        }
        queue.write_buffer(&self.viewmodel_uniform_buffer, 0, &data);
    }

    pub(in crate::render) fn viewmodel_uniform_bind_group(&self) -> &wgpu::BindGroup {
        &self.viewmodel_uniform_bind_group
    }
}
