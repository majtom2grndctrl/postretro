// GPU creation chokepoint: every renderer buffer, texture, bind group and pipeline, counted per thread.
// See: context/plans/in-progress/level-load-pipeline/index.md (Acceptance, Off the main thread)

use std::cell::Cell;
use wgpu::util::DeviceExt;

/// Running totals of GPU objects created through [`GpuCreate`] on one thread.
/// Render and compute pipelines share `pipelines`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct GpuCreationCounts {
    pub buffers: u64,
    pub textures: u64,
    pub bind_groups: u64,
    pub pipelines: u64,
}

impl GpuCreationCounts {
    pub fn total(&self) -> u64 {
        self.buffers + self.textures + self.bind_groups + self.pipelines
    }

    /// What was created between `earlier` and `self`.
    pub fn since(&self, earlier: GpuCreationCounts) -> GpuCreationCounts {
        GpuCreationCounts {
            buffers: self.buffers.saturating_sub(earlier.buffers),
            textures: self.textures.saturating_sub(earlier.textures),
            bind_groups: self.bind_groups.saturating_sub(earlier.bind_groups),
            pipelines: self.pipelines.saturating_sub(earlier.pipelines),
        }
    }
}

// Per thread, not global: a question about one thread's work (the commit
// frame's, say) must not see the upload thread's creations or a parallel
// test's.
thread_local! {
    static COUNTS: Cell<GpuCreationCounts> = const {
        Cell::new(GpuCreationCounts {
            buffers: 0,
            textures: 0,
            bind_groups: 0,
            pipelines: 0,
        })
    };
}

/// The calling thread's running totals. Counts only creations routed through
/// [`GpuCreate`] on this thread; a grep gate proves every non-test renderer
/// creation of these kinds is.
pub fn gpu_creation_counts() -> GpuCreationCounts {
    COUNTS.with(Cell::get)
}

fn count(bump: impl FnOnce(&mut GpuCreationCounts)) {
    COUNTS.with(|counts| {
        let mut next = counts.get();
        bump(&mut next);
        counts.set(next);
    });
}

/// The renderer's only way to create the counted kinds. Each method counts on
/// the calling thread, then calls the matching `wgpu` method unchanged.
pub(crate) trait GpuCreate {
    fn new_buffer(&self, desc: &wgpu::BufferDescriptor) -> wgpu::Buffer;
    fn new_buffer_init(&self, desc: &wgpu::util::BufferInitDescriptor) -> wgpu::Buffer;
    fn new_texture(&self, desc: &wgpu::TextureDescriptor) -> wgpu::Texture;
    fn new_texture_with_data(
        &self,
        queue: &wgpu::Queue,
        desc: &wgpu::TextureDescriptor,
        order: wgpu::util::TextureDataOrder,
        data: &[u8],
    ) -> wgpu::Texture;
    fn new_bind_group(&self, desc: &wgpu::BindGroupDescriptor) -> wgpu::BindGroup;
    fn new_render_pipeline(&self, desc: &wgpu::RenderPipelineDescriptor) -> wgpu::RenderPipeline;
    fn new_compute_pipeline(&self, desc: &wgpu::ComputePipelineDescriptor)
    -> wgpu::ComputePipeline;
}

impl GpuCreate for wgpu::Device {
    fn new_buffer(&self, desc: &wgpu::BufferDescriptor) -> wgpu::Buffer {
        count(|counts| counts.buffers += 1);
        self.create_buffer(desc)
    }

    fn new_buffer_init(&self, desc: &wgpu::util::BufferInitDescriptor) -> wgpu::Buffer {
        count(|counts| counts.buffers += 1);
        self.create_buffer_init(desc)
    }

    fn new_texture(&self, desc: &wgpu::TextureDescriptor) -> wgpu::Texture {
        count(|counts| counts.textures += 1);
        self.create_texture(desc)
    }

    fn new_texture_with_data(
        &self,
        queue: &wgpu::Queue,
        desc: &wgpu::TextureDescriptor,
        order: wgpu::util::TextureDataOrder,
        data: &[u8],
    ) -> wgpu::Texture {
        count(|counts| counts.textures += 1);
        self.create_texture_with_data(queue, desc, order, data)
    }

    fn new_bind_group(&self, desc: &wgpu::BindGroupDescriptor) -> wgpu::BindGroup {
        count(|counts| counts.bind_groups += 1);
        self.create_bind_group(desc)
    }

    fn new_render_pipeline(&self, desc: &wgpu::RenderPipelineDescriptor) -> wgpu::RenderPipeline {
        count(|counts| counts.pipelines += 1);
        self.create_render_pipeline(desc)
    }

    fn new_compute_pipeline(
        &self,
        desc: &wgpu::ComputePipelineDescriptor,
    ) -> wgpu::ComputePipeline {
        count(|counts| counts.pipelines += 1);
        self.create_compute_pipeline(desc)
    }
}

#[cfg(test)]
#[path = "gpu_create_tests.rs"]
mod tests;
