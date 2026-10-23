// Skinned ghost pipelines (blend and screen-door, each with its depth-only draw) and
// their draw recording.
// See: context/lib/rendering_pipeline.md §7.9

use postretro_render_cpu::ghost_transparency::GhostTransparency;
use postretro_render_cpu::mesh_instances::MeshFramePlan;

use super::ghost_depth::ghost_depth_pipeline_state;
use super::ghost_order::{GhostDrawStep, SkinnedGhostSpan};
use super::mesh_pass::MeshPass;

/// Unlit ghost shader: skins like the skinned pass, shades base color by the
/// instance's packed look, and has a colour and a depth-only fragment entry
/// per transparency mode. Only the shared ghost look helper is appended: it
/// binds no lights, shadows or SH.
pub(super) const SKINNED_GHOST_SHADER_SOURCE: &str = concat!(
    include_str!("../shaders/skinned_ghost.wgsl"),
    "\n",
    include_str!("../shaders/ghost_look.wgsl"),
);

/// The fixed-function state one ghost pipeline draws with: a mode's colour
/// draw here, its depth-only draw in [`ghost_depth_pipeline_state`]. Pure
/// data, so the mode → state mapping is testable without a device.
#[derive(Debug, Clone, PartialEq)]
pub(super) struct GhostPipelineState {
    pub(super) fragment_entry: &'static str,
    pub(super) blend: Option<wgpu::BlendState>,
    pub(super) write_mask: wgpu::ColorWrites,
    pub(super) depth: wgpu::DepthStencilState,
    pub(super) cull_mode: Option<wgpu::Face>,
}

/// Blend: straight alpha over the scene, the fluid surface's blend.
/// Dither: blend off; the fragment discards pixels the screen-door pattern
/// drops. Both test `Equal` against the depth the ghost's own depth-only
/// draw just wrote (`ghost_depth.rs`) and never write depth, so each pixel
/// shows only the ghost's nearest surface, scene depth stays untouched and a
/// ghost hides nothing drawn after it. `Equal` is exact: both draws run one
/// `@invariant` vertex stage over the same triangles. A ghost face exactly
/// coplanar with the opaque scene draws, and z-fights with it either way.
/// Both cull back faces like the skinned pass.
pub(super) fn ghost_pipeline_state(
    mode: GhostTransparency,
    depth_format: wgpu::TextureFormat,
) -> GhostPipelineState {
    let (fragment_entry, blend) = match mode {
        GhostTransparency::Blend => (
            "fs_blend",
            Some(wgpu::BlendState {
                color: wgpu::BlendComponent {
                    src_factor: wgpu::BlendFactor::SrcAlpha,
                    dst_factor: wgpu::BlendFactor::OneMinusSrcAlpha,
                    operation: wgpu::BlendOperation::Add,
                },
                alpha: wgpu::BlendComponent {
                    src_factor: wgpu::BlendFactor::One,
                    dst_factor: wgpu::BlendFactor::OneMinusSrcAlpha,
                    operation: wgpu::BlendOperation::Add,
                },
            }),
        ),
        GhostTransparency::Dither => ("fs_dither", None),
    };
    GhostPipelineState {
        fragment_entry,
        blend,
        write_mask: wgpu::ColorWrites::ALL,
        depth: wgpu::DepthStencilState {
            format: depth_format,
            depth_write_enabled: Some(false),
            depth_compare: Some(wgpu::CompareFunction::Equal),
            stencil: wgpu::StencilState::default(),
            bias: wgpu::DepthBiasState::default(),
        },
        cull_mode: Some(wgpu::Face::Back),
    }
}

/// The skinned vertex stream at the skinned pass's offsets; unlit, so normal
/// (16) and tangent (20) are not declared.
static SKINNED_GHOST_VERTEX_ATTRIBUTES: [wgpu::VertexAttribute; 4] = [
    wgpu::VertexAttribute {
        offset: 0,
        shader_location: 0,
        format: wgpu::VertexFormat::Float32x3,
    },
    wgpu::VertexAttribute {
        offset: 12,
        shader_location: 1,
        format: wgpu::VertexFormat::Unorm16x2,
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
];

fn skinned_ghost_vertex_layout() -> wgpu::VertexBufferLayout<'static> {
    wgpu::VertexBufferLayout {
        array_stride: std::mem::size_of::<postretro_model::mesh::SkinnedVertex>()
            as wgpu::BufferAddress,
        step_mode: wgpu::VertexStepMode::Vertex,
        attributes: &SKINNED_GHOST_VERTEX_ATTRIBUTES,
    }
}

/// A colour and a depth-only pipeline per transparency mode, built once; a
/// frame draws every ghost through the mode the mod's render profile picks.
pub(super) struct GhostPipelines {
    blend: wgpu::RenderPipeline,
    dither: wgpu::RenderPipeline,
    depth_blend: wgpu::RenderPipeline,
    depth_dither: wgpu::RenderPipeline,
}

impl GhostPipelines {
    /// Colour layout: group 0 camera, group 1 material, group 3 the skinned
    /// pass's palette + instance data (the same bind group, at the same
    /// index); group 2 stays empty. The depth-only layout drops the material.
    /// No light, shadow or SH group, so ghosts add nothing to any fragment
    /// binding budget. Every pipeline targets the scene colour, so both draws
    /// of a ghost share one pass; the depth-only one writes no colour.
    pub(super) fn new(
        device: &wgpu::Device,
        depth_format: wgpu::TextureFormat,
        camera_bgl: &wgpu::BindGroupLayout,
        material_bgl: &wgpu::BindGroupLayout,
        instance_bgl: &wgpu::BindGroupLayout,
    ) -> Self {
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("Skinned Ghost Shader"),
            source: wgpu::ShaderSource::Wgsl(SKINNED_GHOST_SHADER_SOURCE.into()),
        });
        let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("Skinned Ghost Pipeline Layout"),
            bind_group_layouts: &[
                Some(camera_bgl),
                Some(material_bgl),
                None,
                Some(instance_bgl),
            ],
            immediate_size: 0,
        });
        let depth_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("Skinned Ghost Depth Pipeline Layout"),
            bind_group_layouts: &[Some(camera_bgl), None, None, Some(instance_bgl)],
            immediate_size: 0,
        });
        let build = |layout: &wgpu::PipelineLayout, state: GhostPipelineState, label: &str| {
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some(label),
                layout: Some(layout),
                vertex: wgpu::VertexState {
                    module: &shader,
                    entry_point: Some("vs_main"),
                    buffers: &[Some(skinned_ghost_vertex_layout())],
                    compilation_options: wgpu::PipelineCompilationOptions::default(),
                },
                primitive: wgpu::PrimitiveState {
                    topology: wgpu::PrimitiveTopology::TriangleList,
                    front_face: wgpu::FrontFace::Ccw,
                    cull_mode: state.cull_mode,
                    ..Default::default()
                },
                depth_stencil: Some(state.depth),
                multisample: wgpu::MultisampleState::default(),
                fragment: Some(wgpu::FragmentState {
                    module: &shader,
                    entry_point: Some(state.fragment_entry),
                    targets: &[Some(wgpu::ColorTargetState {
                        format: super::SCENE_COLOR_FORMAT,
                        blend: state.blend,
                        write_mask: state.write_mask,
                    })],
                    compilation_options: wgpu::PipelineCompilationOptions::default(),
                }),
                multiview_mask: None,
                cache: None,
            })
        };
        let colour = |mode| ghost_pipeline_state(mode, depth_format);
        let depth = |mode| ghost_depth_pipeline_state(mode, depth_format);
        Self {
            blend: build(
                &layout,
                colour(GhostTransparency::Blend),
                "Skinned Ghost Blend Pipeline",
            ),
            dither: build(
                &layout,
                colour(GhostTransparency::Dither),
                "Skinned Ghost Dither Pipeline",
            ),
            depth_blend: build(
                &depth_layout,
                depth(GhostTransparency::Blend),
                "Skinned Ghost Blend Depth Pipeline",
            ),
            depth_dither: build(
                &depth_layout,
                depth(GhostTransparency::Dither),
                "Skinned Ghost Dither Depth Pipeline",
            ),
        }
    }

    fn for_step(&self, mode: GhostTransparency, step: GhostDrawStep) -> &wgpu::RenderPipeline {
        match (step, mode) {
            (GhostDrawStep::Depth, GhostTransparency::Blend) => &self.depth_blend,
            (GhostDrawStep::Depth, GhostTransparency::Dither) => &self.depth_dither,
            (GhostDrawStep::Colour, GhostTransparency::Blend) => &self.blend,
            (GhostDrawStep::Colour, GhostTransparency::Dither) => &self.dither,
        }
    }
}

impl MeshPass {
    /// Record one skinned ghost's `spans` of the ghost plan as `step` of its
    /// draw pair, through `mode`'s pipelines, from the buffers
    /// [`MeshPass::plan_and_upload`] already filled this frame. Group 0
    /// (camera) must be set by the caller; this sets the instance data and,
    /// for the colour step, the material per submesh. Every ghost is
    /// forward-visible by construction (the collector culls at the mapped
    /// position). Projectile billboard ghosts are not here: they draw through
    /// the billboard ghost variant and always blend.
    pub(super) fn record_ghost_spans(
        &self,
        pass: &mut wgpu::RenderPass<'_>,
        plan: &MeshFramePlan,
        spans: &[SkinnedGhostSpan],
        mode: GhostTransparency,
        step: GhostDrawStep,
    ) {
        if spans.is_empty() {
            return;
        }
        pass.set_pipeline(self.ghost_pipelines.for_step(mode, step));
        pass.set_bind_group(3, &self.instance_bind_group, &[]);
        for span in spans {
            let Some(group) = plan.groups.get(span.group as usize) else {
                continue;
            };
            let Some(model) = self.models.get(&group.model) else {
                continue;
            };
            pass.set_vertex_buffer(0, model.vertex_buffer.slice(..));
            pass.set_index_buffer(model.index_buffer.slice(..), wgpu::IndexFormat::Uint32);
            for (material_bind_group, indices) in &model.submeshes {
                if indices.is_empty() {
                    continue;
                }
                if step == GhostDrawStep::Colour {
                    pass.set_bind_group(1, material_bind_group, &[]);
                }
                pass.draw_indexed(indices.clone(), 0, span.instances());
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const DEPTH: wgpu::TextureFormat = wgpu::TextureFormat::Depth32Float;

    // Each mode's colour draw writes colour, tests `Equal` against the ghost
    // depth target and never writes depth, so it draws only the ghost's
    // nearest surface per pixel and leaves scene depth to the opaque passes.
    #[test]
    fn ghost_mode_selects_its_fragment_and_blend_and_tests_ghost_depth_equal_without_writing() {
        let blend = ghost_pipeline_state(GhostTransparency::Blend, DEPTH);
        assert_eq!(blend.fragment_entry, "fs_blend");
        let color = blend.blend.expect("blend mode blends").color;
        assert_eq!(color.src_factor, wgpu::BlendFactor::SrcAlpha);
        assert_eq!(color.dst_factor, wgpu::BlendFactor::OneMinusSrcAlpha);

        let dither = ghost_pipeline_state(GhostTransparency::Dither, DEPTH);
        assert_eq!(dither.fragment_entry, "fs_dither");
        assert_eq!(dither.blend, None, "screen-door draws opaque survivors");

        for state in [blend, dither] {
            assert_eq!(state.write_mask, wgpu::ColorWrites::ALL);
            assert_eq!(state.depth.depth_write_enabled, Some(false));
            assert_eq!(
                state.depth.depth_compare,
                Some(wgpu::CompareFunction::Equal)
            );
            assert_eq!(state.cull_mode, Some(wgpu::Face::Back));
        }
    }

    #[test]
    fn skinned_ghost_wgsl_passes_naga_validation_with_both_fragment_entries() {
        let module = naga::front::wgsl::parse_str(SKINNED_GHOST_SHADER_SOURCE)
            .expect("skinned_ghost.wgsl must parse");
        naga::valid::Validator::new(
            naga::valid::ValidationFlags::all(),
            naga::valid::Capabilities::all(),
        )
        .validate(&module)
        .expect("skinned_ghost.wgsl must pass naga validation");
        for mode in GhostTransparency::ALL {
            let entry = ghost_pipeline_state(mode, DEPTH).fragment_entry;
            assert!(
                module
                    .entry_points
                    .iter()
                    .any(|ep| ep.name == entry && ep.stage == naga::ShaderStage::Fragment),
                "{mode:?} entry {entry} must exist"
            );
        }
        // No light, shadow or SH binding: only camera, material and instances.
        let groups: std::collections::BTreeSet<u32> = module
            .global_variables
            .iter()
            .filter_map(|(_, var)| var.binding.as_ref().map(|binding| binding.group))
            .collect();
        assert_eq!(groups, [0, 1, 3].into_iter().collect());
    }

    /// Unlit ghost colors stay at or under the bloom threshold: base color and
    /// tint both lie in `[0, 1]`, so the ghost's own pixels never feed the
    /// bright-pass and bloom cannot smear the screen-door pattern.
    #[test]
    fn unlit_ghost_colors_never_exceed_the_bloom_threshold() {
        use postretro_render_cpu::ghost_instance::{GHOST_LUMA, GhostInstanceLook, ghost_shade};
        let steps = [0.0, 0.25, 0.5, 0.75, 1.0];
        let mut brightest = 0.0f32;
        for &r in &steps {
            for &g in &steps {
                for &b in &steps {
                    for &saturation in &steps {
                        let look = GhostInstanceLook {
                            opacity: 1.0,
                            saturation,
                            tint: [1.0; 3],
                        };
                        let shaded = ghost_shade([r, g, b], &look);
                        let luminance = shaded[0] * GHOST_LUMA[0]
                            + shaded[1] * GHOST_LUMA[1]
                            + shaded[2] * GHOST_LUMA[2];
                        brightest = brightest.max(luminance);
                    }
                }
            }
        }
        assert!(
            brightest <= super::super::bloom::BLOOM_THRESHOLD + 1e-6,
            "brightest unlit ghost luminance {brightest}"
        );
    }
}
