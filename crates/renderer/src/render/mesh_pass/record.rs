// Skinned-mesh draw recording for the forward and planar-mirror passes.
// See: context/lib/rendering_pipeline.md §9

use postretro_render_cpu::mesh_instances::MeshFramePlan;

use super::{MeshPass, UploadedModel};

impl MeshPass {
    /// Record the forward skinned-mesh draws from the already-uploaded buffers.
    ///
    /// Must run AFTER [`MeshPass::plan_and_upload`] has populated the palette +
    /// instance buffers for this `plan` — this method records draws only, it does
    /// NOT touch the buffers, so the data it draws is the identical posed data the
    /// shadow loop read. One instanced `draw_indexed` per model per submesh.
    ///
    /// Group 0 (camera) and group 4 (SH irradiance volume) must be set by the
    /// caller before recording — the renderer owns those bind groups (camera is
    /// shared across passes; SH uses the mesh-superset `mesh_bind_group`).
    ///
    /// The mesh collector can include shadow-only instances for selected static
    /// lights. Filter `forward_visible` so those instances feed shadow depth but
    /// skip the color pass, batching contiguous visible runs so the common
    /// all-visible frame issues one draw per group/submesh. Instances only
    /// the planar mirror reaches never draw here.
    pub fn record_draws(&self, pass: &mut wgpu::RenderPass<'_>, plan: &MeshFramePlan) {
        self.record_draws_where(pass, plan, false, |instance| instance.forward_visible);
    }

    /// The planar mirror's skinned-mesh draws from the same uploaded buffers:
    /// every world instance the mirror reaches, shared with the main view or
    /// the mirror's alone (`rendering_pipeline.md` §7.3a). Group 0 carries
    /// the mirror camera. Only the world plan reaches the mirror, so ghosts
    /// and the viewmodel never draw here. Group 2 carries the mirror's light
    /// records on a frame a mirror-only light holds a shadow slot.
    pub fn record_mirror_draws(&self, pass: &mut wgpu::RenderPass<'_>, plan: &MeshFramePlan) {
        self.record_draws_where(pass, plan, self.mirror_light_records, |instance| {
            instance.mirror.drawn()
        });
    }

    /// One instanced draw per maximal contiguous run of instances `drawn`
    /// admits, per group and submesh.
    fn record_draws_where(
        &self,
        pass: &mut wgpu::RenderPass<'_>,
        plan: &MeshFramePlan,
        mirror_light_records: bool,
        drawn: impl Fn(&postretro_render_cpu::mesh_instances::PlannedInstance) -> bool,
    ) {
        if plan.groups.is_empty() {
            return;
        }

        pass.set_pipeline(&self.pipeline);
        // Group 2 (runtime direct lighting): the runtime light buffers + the
        // per-frame params uniform. Set once for the frame. The pipeline layout
        // declares group 2, so the bind group MUST be present before any mesh
        // draw — the renderer wires it (`rebuild_light_bind_group`) once geometry
        // installs, and the draw path is skipped until a model is uploaded, so
        // this is `Some` on every frame a mesh actually draws. The expect guards
        // against a future caller reordering that wiring after the draw.
        let light_bind_group = if mirror_light_records {
            self.mirror_light_bind_group.as_ref()
        } else {
            self.light_bind_group.as_ref()
        }
        .expect("mesh group-2 light bind group must be built before recording mesh draws");
        pass.set_bind_group(2, light_bind_group, &[]);
        // Group 3 (palette + instance SSBO) is shared across every group/submesh
        // this frame — set once. The shader selects each instance's run via
        // `@builtin(instance_index)` against the densely-packed SSBO.
        pass.set_bind_group(3, &self.instance_bind_group, &[]);

        for group in &plan.groups {
            let Some(model) = self.models.get(&group.model) else {
                continue;
            };
            if model.submeshes.is_empty() {
                continue;
            }

            pass.set_vertex_buffer(0, model.vertex_buffer.slice(..));
            pass.set_index_buffer(model.index_buffer.slice(..), wgpu::IndexFormat::Uint32);
            // One instanced draw per maximal contiguous run of drawn instances.
            // Undrawn entries break a run; an all-drawn group collapses to one
            // run. The base instance is
            // the run's absolute SSBO offset — the palette base still rides each
            // SSBO entry, never `first_instance` (DX12 reads it as 0,
            // gfx-rs/wgpu#2471), addressed by `@builtin(instance_index)`.
            let mut run_start: Option<u32> = None;
            for (i, inst) in group.instances.iter().enumerate() {
                let abs = group.instance_offset + i as u32;
                if drawn(inst) {
                    run_start.get_or_insert(abs);
                } else if let Some(start) = run_start.take() {
                    draw_forward_run(pass, model, start..abs);
                }
            }
            if let Some(start) = run_start.take() {
                let end = group.instance_offset + group.instances.len() as u32;
                draw_forward_run(pass, model, start..end);
            }
        }
    }
}

/// One instanced draw per submesh over a contiguous `range` of drawn
/// instances. Sets group 1 (material) per submesh; group 3 (palette + instance
/// SSBO) is bound once per frame by the caller.
fn draw_forward_run(
    pass: &mut wgpu::RenderPass<'_>,
    model: &UploadedModel,
    range: std::ops::Range<u32>,
) {
    if range.is_empty() {
        return;
    }
    for (material_bind_group, indices) in &model.submeshes {
        if indices.is_empty() {
            continue;
        }
        pass.set_bind_group(1, material_bind_group, &[]);
        pass.draw_indexed(indices.clone(), 0, range.clone());
    }
}
