// Skinned-mesh shadow-depth pipeline and draw recording.
// See: context/lib/rendering_pipeline.md §7.1, §9

use postretro_model::ModelHandle;
use postretro_render_cpu::mesh_instances::{JointCounts, MeshFramePlan, instance_casts_into_cone};

use super::mesh_pass::MeshPass;

/// Depth-only skinned shader: position + joints + weights, skinned by the shared
/// `skin_matrix` kernel and projected by a per-render light-space matrix (group
/// 0). Renders animated entity occluders into a shadow map. Standalone (no
/// helper append) — it declares only the buffers it reads.
pub(super) const SKINNED_DEPTH_SHADER_SOURCE: &str = include_str!("../shaders/skinned_depth.wgsl");

/// Which planned mesh instances a shadow-depth pass may consume.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum MeshDepthInstanceFilter {
    /// Dynamic slots include portal-visible casters plus explicit
    /// descriptor-authored shadow-only casters, including those outside PVS.
    DynamicCasters,
    /// Promoted-static slots consume the broader collection retained by static
    /// light relevance, including off-PVS meshes that are not dynamic casters.
    AllRetained,
}

impl MeshDepthInstanceFilter {
    fn includes(self, instance: &postretro_render_cpu::mesh_instances::PlannedInstance) -> bool {
        match self {
            Self::DynamicCasters => instance.dynamic_shadow_visible,
            Self::AllRetained => true,
        }
    }
}

/// Build the depth-only skinned pipeline used by spot slots and cube faces.
/// Its layout keeps the forward pass's group-3 instance bind group reusable
/// without a second palette or instance upload.
pub(super) fn create_skinned_depth_pipeline(
    device: &wgpu::Device,
    light_space_bgl: &wgpu::BindGroupLayout,
    instance_bind_group_layout: &wgpu::BindGroupLayout,
    shadow_depth_format: wgpu::TextureFormat,
) -> wgpu::RenderPipeline {
    // Its OWN layout: group 0 = the per-render light-space matrix BGL
    // (dynamic-offset 64-byte mat4x4, shared with the world spot-shadow depth
    // pipeline), group 3 = the SAME instance BGL as the forward pass (palette
    // + per-instance SSBO). Groups 1, 2, 4 are omitted — depth-only reads no
    // material, lighting, or SH. Forcing group 3 to index 3 keeps the forward
    // pass's group-3 bind group reusable here without re-upload.
    let depth_shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some("Skinned Depth Shader"),
        source: wgpu::ShaderSource::Wgsl(SKINNED_DEPTH_SHADER_SOURCE.into()),
    });
    let depth_pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
        label: Some("Skinned Depth Pipeline Layout"),
        bind_group_layouts: &[
            Some(light_space_bgl),
            None,
            None,
            Some(instance_bind_group_layout),
        ],
        immediate_size: 0,
    });
    device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some("Skinned Depth Pipeline"),
        layout: Some(&depth_pipeline_layout),
        vertex: wgpu::VertexState {
            module: &depth_shader,
            entry_point: Some("vs_main"),
            // Position (loc 0) + joints (loc 4) + weights (loc 5) only — the
            // color attributes are dropped. Offsets match the forward layout so
            // the SAME vertex buffer binds: joints at byte 24, weights at 28;
            // stride is the full `SkinnedVertex` (the skipped attributes still
            // occupy the stride, they are simply not declared).
            buffers: &[wgpu::VertexBufferLayout {
                array_stride: std::mem::size_of::<postretro_model::mesh::SkinnedVertex>()
                    as wgpu::BufferAddress,
                step_mode: wgpu::VertexStepMode::Vertex,
                attributes: &[
                    wgpu::VertexAttribute {
                        offset: 0,
                        shader_location: 0,
                        format: wgpu::VertexFormat::Float32x3,
                    },
                    wgpu::VertexAttribute {
                        offset: 24,
                        shader_location: 4,
                        format: wgpu::VertexFormat::Uint8x4,
                    },
                    wgpu::VertexAttribute {
                        offset: 28,
                        shader_location: 5,
                        format: wgpu::VertexFormat::Unorm8x4,
                    },
                ],
            }],
            compilation_options: wgpu::PipelineCompilationOptions::default(),
        },
        primitive: wgpu::PrimitiveState {
            topology: wgpu::PrimitiveTopology::TriangleList,
            front_face: wgpu::FrontFace::Ccw,
            cull_mode: Some(wgpu::Face::Back),
            ..Default::default()
        },
        // Depth-only into the shadow map: write depth, no color target, with
        // the same acne-suppressing bias the world spot-shadow pass uses.
        depth_stencil: Some(wgpu::DepthStencilState {
            format: shadow_depth_format,
            depth_write_enabled: Some(true),
            depth_compare: Some(wgpu::CompareFunction::Less),
            stencil: wgpu::StencilState::default(),
            bias: wgpu::DepthBiasState {
                constant: 2,
                slope_scale: 1.5,
                clamp: 0.0,
            },
        }),
        multisample: wgpu::MultisampleState::default(),
        fragment: None,
        multiview_mask: None,
        cache: None,
    })
}

impl MeshPass {
    /// Record skinned ENTITY occluders into a shadow map through the
    /// parameterized depth-only path, culled per-slot by the slot's cone frustum.
    /// `filter` keeps ordinary dynamic slots to portal-visible or explicitly
    /// shadow-only casters while promoted-static slots may use the broader
    /// static-relevance collection.
    /// `light_space_bind_group` + `dynamic_offset` select the per-render
    /// light-space matrix at group 0 (the spot path passes the renderer's
    /// `shadow_vs_bind_group` and the per-slot offset; a cube path would pass a
    /// per-face uniform) — nothing here assumes one slot per light or a 2D target,
    /// proving the cube-ready contract.
    ///
    /// `cone_planes` are the slot's 6 cone-frustum planes (from the slot's
    /// light-space matrix). Each planned instance's local bound is transformed by
    /// its world matrix and tested against the cone; only intersecting instances
    /// are drawn into the slot. Entities are not in the world BVH, so this cull is
    /// per-instance CPU (distinct from the GPU world cull). Returns the count of
    /// instances actually submitted into this slot, so the caller can tally the
    /// per-frame submitted-occluder counter that verifies the out-of-cone
    /// acceptance criterion — no GPU readback.
    ///
    /// The caller owns the target view (it begins the render pass against the
    /// slot's depth attachment) and supplies the light-space matrix via the bind
    /// group; this method binds the depth pipeline + the SHARED group-3 instance
    /// data and records the draws from the SAME palette/instance buffers
    /// [`MeshPass::plan_and_upload`] populated. No per-frame buffer writes here —
    /// it reads the already-posed data.
    ///
    /// Surviving instances are drawn as per-instance `draw_indexed` calls
    /// (`instance_index..+1`) because the cone cull selects an arbitrary subset of
    /// each group's contiguous SSBO range; wave counts are small (a few dozen), so
    /// per-instance draws stay cheap. The base instance is the absolute index into
    /// the dense SSBO, so `@builtin(instance_index)` selects this occluder's entry —
    /// the SAME `first_instance`-borne addressing the forward path uses, with the
    /// SAME documented DX12 exposure (gfx-rs/wgpu#2471). See the per-draw comment at
    /// the `draw_indexed` site below; the per-instance palette base still travels in
    /// the SSBO entry, never in `first_instance`.
    pub fn record_skinned_depth(
        &self,
        pass: &mut wgpu::RenderPass<'_>,
        plan: &MeshFramePlan,
        filter: MeshDepthInstanceFilter,
        light_space_bind_group: &wgpu::BindGroup,
        dynamic_offset: u32,
        cone_planes: &[glam::Vec4; 6],
    ) -> u32 {
        if plan.groups.is_empty() {
            return 0;
        }

        pass.set_pipeline(&self.depth_pipeline);
        pass.set_bind_group(0, light_space_bind_group, &[dynamic_offset]);
        // Same shared group-3 instance data as the forward pass — the depth
        // layout forces it to index 3 so the bind group is reusable verbatim.
        pass.set_bind_group(3, &self.instance_bind_group, &[]);

        let mut submitted: u32 = 0;
        for group in &plan.groups {
            let Some(model) = self.models.get(&group.model) else {
                continue;
            };
            if model.submeshes.is_empty() {
                continue;
            }
            pass.set_vertex_buffer(0, model.vertex_buffer.slice(..));
            pass.set_index_buffer(model.index_buffer.slice(..), wgpu::IndexFormat::Uint32);
            for (i, inst) in group.instances.iter().enumerate() {
                if !filter.includes(inst) {
                    continue;
                }
                // Per-light caster cull: skip instances whose transformed bound
                // does not intersect this slot's cone. An enemy outside the cone
                // is not drawn into the slot.
                if !instance_casts_into_cone(inst, cone_planes) {
                    continue;
                }
                let instance_index = group.instance_offset + i as u32;
                let instance_range = instance_index..instance_index + 1;
                // The draw's `first_instance` is the absolute SSBO index, so the
                // shader reads `instances[instance_index]` / `bone_palette[base]`
                // for THIS occluder via `@builtin(instance_index)`. This shares the
                // forward path's `@builtin(instance_index)` assumption (record_draws
                // above, file header §"Per-instance addressing"): the SSBO ENTRY is
                // selected through `first_instance`, and a backend that zeroes it
                // (the documented DX12 quirk, gfx-rs/wgpu#2471 — we do not assume
                // `INDIRECT_FIRST_INSTANCE`) would read entry 0 for every occluder,
                // projecting all of them with the first instance's pose. Known DX12
                // exposure, correct on Vulkan/Metal; it is NOT unique to the depth
                // path — both paths route the entry index through `first_instance`
                // identically, so a future DX12-robust fix (per-instance index via a
                // vertex-stepped buffer or per-draw dynamic offset) must change both
                // in lock-step, not just here. Only the per-instance palette BASE
                // (`base_and_pad.x`) is kept out of `first_instance` today.
                // Depth-only: one draw per submesh range, no material bind (the
                // depth layout omits group 1).
                for (_material_bind_group, indices) in &model.submeshes {
                    if indices.is_empty() {
                        continue;
                    }
                    pass.draw_indexed(indices.clone(), 0, instance_range.clone());
                }
                submitted += 1;
            }
        }
        submitted
    }
}

/// Joint-count and model-bound lookup over the mesh cache, so the GPU-free
/// frame planner can assign palette runs and stamp per-light caster bounds
/// without a wgpu reference. Missing models keep the existing zero-bound
/// degradation path.
impl JointCounts for MeshPass {
    fn joint_count(&self, model: &ModelHandle) -> Option<u32> {
        self.models
            .get(model)
            .map(|model| model.skeleton.joints.len() as u32)
    }

    fn model_bounds(&self, model: &ModelHandle) -> postretro_render_data::cone_frustum::Aabb {
        self.model_bounds.get(model).copied().unwrap_or_default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use postretro_model::sample_params::MeshSampleParams;
    use postretro_render_cpu::mesh_instances::{MeshPaletteCacheKey, PlannedInstance};

    #[test]
    fn skinned_depth_wgsl_parses_and_is_vertex_only() {
        // The depth-only skinned shader must parse, export `@vertex vs_main`, and
        // carry NO fragment stage (depth-only) — mirroring depth_prepass.wgsl's
        // relationship to forward.wgsl.
        let module = naga::front::wgsl::parse_str(SKINNED_DEPTH_SHADER_SOURCE)
            .expect("skinned_depth.wgsl should parse as WGSL");
        let has_vs = module
            .entry_points
            .iter()
            .any(|ep| ep.name == "vs_main" && ep.stage == naga::ShaderStage::Vertex);
        assert!(has_vs, "skinned_depth.wgsl must export @vertex vs_main");
        let has_fs = module
            .entry_points
            .iter()
            .any(|ep| ep.stage == naga::ShaderStage::Fragment);
        assert!(
            !has_fs,
            "skinned_depth.wgsl is depth-only — it must declare no fragment stage"
        );
    }

    #[test]
    fn skinned_depth_wgsl_passes_naga_validation() {
        let module = naga::front::wgsl::parse_str(SKINNED_DEPTH_SHADER_SOURCE)
            .expect("skinned_depth.wgsl must parse");
        naga::valid::Validator::new(
            naga::valid::ValidationFlags::all(),
            naga::valid::Capabilities::all(),
        )
        .validate(&module)
        .expect("skinned_depth.wgsl must pass naga validation");
    }

    #[test]
    fn depth_instance_filter_separates_dynamic_and_promoted_static_relevance() {
        let visible = PlannedInstance {
            transform: glam::Mat4::IDENTITY,
            shadow_bias_scale: 1.0,
            palette_base: 0,
            phase_seed: 1,
            palette_cache_key: MeshPaletteCacheKey::Entity(1),
            bounds: postretro_render_data::cone_frustum::Aabb::default(),
            sample: MeshSampleParams::stateless(0.0),
            pose_inputs: None,
            capture: None,
            resample: true,
            forward_visible: true,
            dynamic_shadow_visible: true,
        };
        let shadow_only = PlannedInstance {
            forward_visible: false,
            ..visible.clone()
        };
        let static_relevance_only = PlannedInstance {
            dynamic_shadow_visible: false,
            ..shadow_only.clone()
        };

        assert!(MeshDepthInstanceFilter::DynamicCasters.includes(&visible));
        assert!(MeshDepthInstanceFilter::DynamicCasters.includes(&shadow_only));
        assert!(!MeshDepthInstanceFilter::DynamicCasters.includes(&static_relevance_only));
        assert!(MeshDepthInstanceFilter::AllRetained.includes(&static_relevance_only));
    }
}
