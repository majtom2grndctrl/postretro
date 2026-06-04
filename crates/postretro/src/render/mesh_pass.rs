// Skinned-mesh render pass: forward draw of skinned models with a shared bone palette.
// See: context/lib/rendering_pipeline.md §5
//
// Mirrors the shape of `crate::fx::smoke::SmokePass`. Task 3 fills the pipeline
// construction and draw recording; the signatures here are the contract Tasks
// 3/5 build against.

use crate::model::mesh::SkinnedMesh;

/// GPU resources for the skinned-mesh forward pass.
///
/// **Binding plan** (forward, non-shadow):
/// * group 0 = camera (shared renderer-owned camera bind group)
/// * group 1 = material (skinned model's texture set)
/// * the shared bone-palette storage buffer is bound on its own group; the
///   per-instance base index addresses an instance's run within it.
///
/// The **lighting bind group is a provisional additive slot** chosen later by
/// the lighting-interface broadening task (SH ambient + dynamic direct). It is
/// deliberately NOT allocated here — the pass leaves room for it so the future
/// task adds a slot rather than renumbering existing groups.
#[allow(dead_code)] // Task 3 constructs and wires this; pre-emptive structure.
pub struct MeshPass {
    pipeline: wgpu::RenderPipeline,
}

#[allow(dead_code)] // Task 3 calls these.
impl MeshPass {
    /// Build the skinned-mesh pipeline. `camera_bgl` and `material_bgl` are the
    /// renderer-owned layouts shared with the forward pass. Mirrors
    /// `SmokePass::new`'s shape (renderer hands in the shared layouts).
    ///
    /// Task 3 fills the body (shader module, bone-palette bind group layout,
    /// pipeline layout with the provisional lighting slot left open, pipeline).
    #[allow(unused_variables)]
    pub fn new(
        device: &wgpu::Device,
        surface_format: wgpu::TextureFormat,
        depth_format: wgpu::TextureFormat,
        camera_bgl: &wgpu::BindGroupLayout,
        material_bgl: &wgpu::BindGroupLayout,
    ) -> Self {
        todo!(
            "Task 3: build skinned-mesh pipeline (camera + material groups; lighting slot left provisional)"
        )
    }

    /// Upload one instance's bone palette + record its skinned draw on `pass`.
    /// `base_index` is the instance's offset into the shared palette buffer (see
    /// `crate::model::BonePaletteEntry`). Mirrors `SmokePass::record_draw`'s
    /// shape (queue + pass + per-draw payload).
    ///
    /// Task 3 fills the body (write palette run, set bind groups, set vertex /
    /// index buffers, indexed draw).
    #[allow(unused_variables)]
    pub fn record_draw<'a>(
        &'a self,
        queue: &wgpu::Queue,
        pass: &mut wgpu::RenderPass<'a>,
        mesh: &SkinnedMesh,
        base_index: u32,
    ) {
        todo!("Task 3: record skinned draw for one instance")
    }
}
