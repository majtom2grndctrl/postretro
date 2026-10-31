//! Streamed SH compose pipelines: built once per renderer, cloned into each level's pools.
//! See: context/plans/in-progress/level-load-pipeline/index.md (Decisions: Pipelines once per renderer)

use crate::render::install_pipelines::new_install_compute_pipeline;
use crate::render::sh_indirection::WGSL_DECODE_HELPER;

/// One compose pass's group layout and pipeline.
#[derive(Clone)]
pub(in crate::render::sh_streaming) struct ComposeKernel {
    pub(in crate::render::sh_streaming) bind_group_layout: wgpu::BindGroupLayout,
    pub(in crate::render::sh_streaming) pipeline: wgpu::ComputePipeline,
}

struct KernelSpec<'a> {
    label: &'a str,
    layout_label: &'a str,
    bgl_label: &'a str,
    shader_label: &'a str,
    entries: &'a [wgpu::BindGroupLayoutEntry],
    /// The renderer's group-0 layout, for passes that bind frame uniforms.
    uniform_bind_group_layout: Option<&'a wgpu::BindGroupLayout>,
    source: String,
    entry_point: &'a str,
}

impl ComposeKernel {
    fn new(device: &wgpu::Device, spec: KernelSpec<'_>) -> Self {
        let bind_group_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some(spec.bgl_label),
            entries: spec.entries,
        });
        let bind_group_layouts: Vec<Option<&wgpu::BindGroupLayout>> =
            match spec.uniform_bind_group_layout {
                Some(uniform) => vec![Some(uniform), Some(&bind_group_layout)],
                None => vec![Some(&bind_group_layout)],
            };
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some(spec.layout_label),
            bind_group_layouts: &bind_group_layouts,
            immediate_size: 0,
        });
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some(spec.shader_label),
            source: wgpu::ShaderSource::Wgsl(spec.source.into()),
        });
        let pipeline = new_install_compute_pipeline(
            device,
            &wgpu::ComputePipelineDescriptor {
                label: Some(spec.label),
                layout: Some(&pipeline_layout),
                module: &shader,
                entry_point: Some(spec.entry_point),
                compilation_options: wgpu::PipelineCompilationOptions::default(),
                cache: None,
            },
        );
        Self {
            bind_group_layout,
            pipeline,
        }
    }
}

/// The streamed indirect compose and the streamed direct promotion and
/// animated-add passes. Each level's pools keep a clone, so in-play sparse
/// growth rebinds against these instead of rebuilding a pipeline.
#[derive(Clone)]
pub(crate) struct StreamingShPipelines {
    pub(in crate::render::sh_streaming) indirect: ComposeKernel,
    pub(in crate::render::sh_streaming) promotion: ComposeKernel,
    pub(in crate::render::sh_streaming) animated: ComposeKernel,
}

impl StreamingShPipelines {
    pub(crate) fn new(
        device: &wgpu::Device,
        uniform_bind_group_layout: &wgpu::BindGroupLayout,
    ) -> Self {
        let indirect = ComposeKernel::new(
            device,
            KernelSpec {
                label: "Streamed SH Compose Pipeline",
                layout_label: "Streamed SH Compose Pipeline Layout",
                bgl_label: "Streamed SH Compose BGL",
                shader_label: "Streamed SH Compose Shader",
                entries: &super::gpu::compose_bgl_entries(),
                uniform_bind_group_layout: Some(uniform_bind_group_layout),
                source: [
                    include_str!("../../shaders/sh_compose.wgsl"),
                    "\n",
                    include_str!("../../shaders/curve_eval.wgsl"),
                    "\n",
                    WGSL_DECODE_HELPER,
                ]
                .concat(),
                entry_point: "compose_main",
            },
        );
        let promotion = ComposeKernel::new(
            device,
            KernelSpec {
                label: "Streamed Direct SH Promotion Pipeline",
                layout_label: "Streamed Direct SH Promotion Pipeline Layout",
                bgl_label: "Streamed Direct SH Promotion BGL",
                shader_label: "Streamed Direct SH Promotion Shader",
                entries: &super::direct_compose::promotion_bgl_entries(),
                uniform_bind_group_layout: None,
                source: [
                    include_str!("../../shaders/direct_sh_compose.wgsl"),
                    "\n",
                    WGSL_DECODE_HELPER,
                ]
                .concat(),
                entry_point: "compose_main",
            },
        );
        let animated = ComposeKernel::new(
            device,
            KernelSpec {
                label: "Streamed Animated Direct SH Pipeline",
                layout_label: "Streamed Animated Direct SH Pipeline Layout",
                bgl_label: "Streamed Animated Direct SH BGL",
                shader_label: "Streamed Animated Direct SH Shader",
                entries: &super::direct_compose::animated_bgl_entries(),
                uniform_bind_group_layout: Some(uniform_bind_group_layout),
                source: [
                    include_str!("../../shaders/animated_direct_sh_compose.wgsl"),
                    "\n",
                    include_str!("../../shaders/curve_eval.wgsl"),
                    "\n",
                    WGSL_DECODE_HELPER,
                ]
                .concat(),
                entry_point: super::direct_compose::ANIMATED_DIRECT_COMPOSE_ENTRY_POINT,
            },
        );
        Self {
            indirect,
            promotion,
            animated,
        }
    }
}
