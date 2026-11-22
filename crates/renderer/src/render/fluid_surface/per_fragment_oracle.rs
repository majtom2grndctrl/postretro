// The build before's reflecting pipeline, for the live-lighting sums' GPU
// gates: the reflecting variant rebuilt with its fragment evaluating every
// stored light itself, swapped in for production's to draw the same frame.
// See: context/lib/rendering_pipeline.md §7.3a (Live lighting)

use super::*;

/// The composed fluid source with the reflecting fragment's one call of
/// `fluid_reflection_ratio_from_sums(` restored to the kept per-fragment
/// `fluid_reflection_ratio(`: the build before's fragment exactly. Panics
/// unless exactly one call site (the definition excluded) is substituted.
pub(crate) fn per_fragment_ratio_oracle_source() -> String {
    const FROM_SUMS: &str = "fluid_reflection_ratio_from_sums(";
    let calls: Vec<usize> = SHADER_SOURCE
        .match_indices(FROM_SUMS)
        .map(|(at, _)| at)
        .filter(|&at| !SHADER_SOURCE[..at].ends_with("fn "))
        .collect();
    assert_eq!(
        calls.len(),
        1,
        "the composed fluid source calls `{FROM_SUMS}` exactly once"
    );
    let at = calls[0];
    format!(
        "{}fluid_reflection_ratio({}",
        &SHADER_SOURCE[..at],
        &SHADER_SOURCE[at + FROM_SUMS.len()..]
    )
}

impl FluidSurfacePass {
    /// The build before's reflecting pipeline: the reflecting variant built
    /// from [`per_fragment_ratio_oracle_source`] over the production
    /// pipeline's own group layouts and the production constructor.
    pub(crate) fn build_per_fragment_oracle_pipeline(
        &self,
        device: &wgpu::Device,
        depth_format: wgpu::TextureFormat,
    ) -> wgpu::RenderPipeline {
        let reflecting = self.pipeline(FluidShaderVariant::Reflecting);
        let groups: Vec<wgpu::BindGroupLayout> = (0..FLUID_BIND_GROUP_COUNT as u32)
            .map(|group| reflecting.get_bind_group_layout(group))
            .collect();
        let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("Fluid Surface Oracle Pipeline Layout"),
            bind_group_layouts: &groups.iter().map(Some).collect::<Vec<_>>(),
            immediate_size: 0,
        });
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("Fluid Surface Oracle Shader"),
            source: wgpu::ShaderSource::Wgsl(per_fragment_ratio_oracle_source().into()),
        });
        create_fluid_pipeline(
            device,
            &layout,
            &shader,
            depth_format,
            FluidShaderVariant::Reflecting,
        )
    }

    /// Put `pipeline` in the reflecting variant's place and return the one
    /// it replaces.
    pub(crate) fn swap_reflecting_pipeline(
        &mut self,
        pipeline: wgpu::RenderPipeline,
    ) -> wgpu::RenderPipeline {
        std::mem::replace(
            &mut self.pipelines[FluidShaderVariant::Reflecting as usize],
            pipeline,
        )
    }
}

impl InstalledFluidSurfaces {
    /// The level's live-lighting sums buffer, for readback and prefill.
    pub(crate) fn sums_buffer_for_test(&self) -> &wgpu::Buffer {
        self.probes.sums_buffer()
    }
}
