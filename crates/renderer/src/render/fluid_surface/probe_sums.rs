// The live-lighting sums pass: once per frame with a reflecting fluid draw, a
// compute dispatch over every installed reflection probe writes its per-lobe
// light sums, which the reflecting fluid fragment blends.
// See: context/lib/rendering_pipeline.md §7.3a (Live lighting), §10

use postretro_render_cpu::fluid_surface::reflection::reflection_probe_sums_workgroups;

use super::InstalledFluidSurfaces;
use crate::render::fluid_reflection_probes::fluid_sums_pass_bind_group_layout_entries;
use crate::render::frame_timing::FrameTiming;
use crate::render::gpu_create::GpuCreate;
use crate::render::pipeline_layout::TIMING_PAIR_FLUID_REFLECTION_SUMS;

/// The sums shader: its own bindings and entry, then the light helpers it
/// shares with the fluid shader, then the curve helpers they read.
const SUMS_SHADER_SOURCE: &str = concat!(
    include_str!("../../shaders/fluid_reflection_sums.wgsl"),
    "\n",
    include_str!("../../shaders/fluid_reflection_lights.wgsl"),
    "\n",
    include_str!("../../shaders/curve_eval.wgsl"),
    "\n",
);

/// The sums pass's groups: 0 the shared camera group (the frame's animation
/// clock), 1 the level's SH group (the descriptors and curve samples script
/// writes land in), 2 its own (the sums and the share table).
pub(crate) const FLUID_SUMS_CAMERA_GROUP: u32 = 0;
pub(crate) const FLUID_SUMS_SH_GROUP: u32 = 1;
pub(crate) const FLUID_SUMS_OWN_GROUP: u32 = 2;
#[cfg(test)]
pub(crate) const FLUID_SUMS_BIND_GROUP_COUNT: usize = 3;

/// Every BGL entry set the sums pipeline layout composes, in group order.
/// GPU-free, so the budget tests derive the compute stage's inventory from
/// the builders the layout is created from.
#[cfg(test)]
pub(crate) fn fluid_sums_pipeline_bind_group_layout_entries()
-> [Vec<wgpu::BindGroupLayoutEntry>; FLUID_SUMS_BIND_GROUP_COUNT] {
    [
        crate::render::pipeline_layout::uniform_bind_group_layout_entries().to_vec(),
        crate::render::sh_volume::sh_bind_group_layout_entries(),
        fluid_sums_pass_bind_group_layout_entries().to_vec(),
    ]
}

/// The sums pass's pipeline and its own group's layout, built at full init.
/// It holds no sums and no group over them: those belong to the level.
pub(crate) struct FluidProbeSumsPass {
    pipeline: wgpu::ComputePipeline,
    layout: wgpu::BindGroupLayout,
    /// Dispatches recorded so far; tests read it.
    #[cfg(test)]
    dispatches: std::cell::Cell<u32>,
}

impl FluidProbeSumsPass {
    pub(crate) fn new(
        device: &wgpu::Device,
        camera_bgl: &wgpu::BindGroupLayout,
        sh_bgl: &wgpu::BindGroupLayout,
    ) -> Self {
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("Fluid Reflection Sums BGL"),
            entries: &fluid_sums_pass_bind_group_layout_entries(),
        });
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("Fluid Reflection Sums Shader"),
            source: wgpu::ShaderSource::Wgsl(SUMS_SHADER_SOURCE.into()),
        });
        // Same group order as `fluid_sums_pipeline_bind_group_layout_entries`.
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("Fluid Reflection Sums Pipeline Layout"),
            bind_group_layouts: &[Some(camera_bgl), Some(sh_bgl), Some(&layout)],
            immediate_size: 0,
        });
        let pipeline = device.new_compute_pipeline(&wgpu::ComputePipelineDescriptor {
            label: Some("Fluid Reflection Sums Pipeline"),
            layout: Some(&pipeline_layout),
            module: &shader,
            entry_point: Some("fluid_reflection_sums_main"),
            compilation_options: Default::default(),
            cache: None,
        });
        Self {
            pipeline,
            layout,
            #[cfg(test)]
            dispatches: std::cell::Cell::new(0),
        }
    }

    /// The sums pass's own group layout, for a level build's group over its
    /// sums and share table.
    pub(crate) fn layout(&self) -> &wgpu::BindGroupLayout {
        &self.layout
    }

    /// Record one dispatch over every probe `installed` holds, into the
    /// frame's encoder, timed inside the frame's prefilled query set.
    /// `camera` and `sh` are the frame's group-0 and level SH groups, the
    /// ones the fluid pass binds. A level with no installed probe records
    /// nothing; no reflecting draw can bind one.
    pub(crate) fn record(
        &self,
        encoder: &mut wgpu::CommandEncoder,
        timing: Option<&FrameTiming>,
        camera: &wgpu::BindGroup,
        sh: &wgpu::BindGroup,
        installed: &InstalledFluidSurfaces,
    ) {
        let Some((sums, probe_count)) = installed.probes.sums_dispatch() else {
            return;
        };
        #[cfg(test)]
        self.dispatches.set(self.dispatches.get() + 1);
        let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
            label: Some("Fluid Reflection Sums Pass"),
            timestamp_writes: timing
                .map(|timing| timing.compute_pass_writes(TIMING_PAIR_FLUID_REFLECTION_SUMS)),
        });
        pass.set_pipeline(&self.pipeline);
        pass.set_bind_group(FLUID_SUMS_CAMERA_GROUP, camera, &[]);
        pass.set_bind_group(FLUID_SUMS_SH_GROUP, sh, &[]);
        pass.set_bind_group(FLUID_SUMS_OWN_GROUP, sums, &[]);
        pass.dispatch_workgroups(reflection_probe_sums_workgroups(probe_count), 1, 1);
    }

    /// Dispatches recorded so far (tests).
    #[cfg(test)]
    pub(crate) fn dispatches(&self) -> u32 {
        self.dispatches.get()
    }
}

#[cfg(test)]
pub(crate) fn composed_sums_shader_source() -> &'static str {
    SUMS_SHADER_SOURCE
}
