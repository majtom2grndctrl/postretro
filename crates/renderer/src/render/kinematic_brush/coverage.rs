// Mover coverage: a view's movers drawn through the depth pre-pass's own clip
// path into a caller's colour target, and the walk both recordings share.
// See: context/lib/rendering_pipeline.md §7.3, §7.3a (Glossy world faces)

use crate::render::gpu_create::GpuCreate;

use super::{
    KinematicBrushPass, KinematicLevelState, MoverView, mover_primitive_state, mover_vertex_layout,
};

impl KinematicBrushPass {
    /// A pipeline that rasterizes the movers exactly as the depth pre-pass
    /// does (`vs_depth` from the same module, same layout, vertex layout and
    /// culling), tests `Equal` against that depth without writing it, and
    /// writes `fragment`'s outputs. So it covers exactly the pixels the mover
    /// colour pass shades, a face coplanar with a world face included. Built
    /// at full init.
    pub(crate) fn coverage_pipeline(
        &self,
        device: &wgpu::Device,
        label: &str,
        depth_format: wgpu::TextureFormat,
        fragment: wgpu::FragmentState<'_>,
    ) -> wgpu::RenderPipeline {
        device.new_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some(label),
            layout: Some(&self.clip_layout),
            vertex: wgpu::VertexState {
                module: &self.clip_module,
                entry_point: Some("vs_depth"),
                buffers: &[Some(mover_vertex_layout())],
                compilation_options: wgpu::PipelineCompilationOptions::default(),
            },
            primitive: mover_primitive_state(),
            depth_stencil: Some(wgpu::DepthStencilState {
                format: depth_format,
                depth_write_enabled: Some(false),
                depth_compare: Some(wgpu::CompareFunction::Equal),
                stencil: wgpu::StencilState::default(),
                bias: wgpu::DepthBiasState::default(),
            }),
            multisample: wgpu::MultisampleState::default(),
            fragment: Some(fragment),
            multiview_mask: None,
            cache: None,
        })
    }

    /// The main view's movers through `pipeline`, a [`Self::coverage_pipeline`],
    /// walking the list the main depth pre-pass and colour pass walk. The
    /// caller binds group 0 (the main camera).
    pub(crate) fn record_coverage(
        &self,
        pass: &mut wgpu::RenderPass<'_>,
        level: &KinematicLevelState,
        pipeline: &wgpu::RenderPipeline,
    ) {
        self.record_clip(pass, level, MoverView::Main, pipeline);
    }

    /// `view`'s movers through `pipeline`, a pipeline over the clip layout:
    /// camera at group 0, instance transforms at group 3.
    pub(super) fn record_clip(
        &self,
        pass: &mut wgpu::RenderPass<'_>,
        level: &KinematicLevelState,
        view: MoverView,
        pipeline: &wgpu::RenderPipeline,
    ) {
        if !self.has_draws(level) {
            return;
        }
        pass.set_pipeline(pipeline);
        pass.set_bind_group(3, &self.instance_bind_group, &[]);
        pass.set_vertex_buffer(0, level.vertex_buffer.slice(..));
        pass.set_index_buffer(level.index_buffer.slice(..), wgpu::IndexFormat::Uint32);
        for active in self.view_draws(level, view) {
            let range = level.mover_index_ranges[active.mover_draw_index];
            if range.index_count == 0 {
                continue;
            }
            pass.draw_indexed(
                range.index_start..range.index_start + range.index_count,
                0,
                active.instance_index..active.instance_index + 1,
            );
        }
    }
}
