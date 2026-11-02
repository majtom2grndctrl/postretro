// Install-time queue writes: direct on the main thread, metered into bounded submits on the upload thread.
// See: context/plans/done/level-load-pipeline/index.md (Off-main GPU creation) · context/plans/done/per-frame-upload-batching

/// The queue writes a level build issues. The raw queue implements it
/// directly (main-thread installs and in-play growth); the upload thread's
/// [`ChunkedWrites`](super::upload_thread::ChunkedWrites) meters every write
/// and submits them in batches of at most
/// [`UPLOAD_SUBMIT_CHUNK_BYTES`](super::upload_thread::UPLOAD_SUBMIT_CHUNK_BYTES).
/// Every caller still needs a lifecycle class in the upload drift guard: the
/// trait routes writes, it never exempts them.
pub(crate) trait QueueWrites {
    fn write_buffer(&self, buffer: &wgpu::Buffer, offset: wgpu::BufferAddress, data: &[u8]);

    fn write_texture(
        &self,
        texture: wgpu::TexelCopyTextureInfo<'_>,
        data: &[u8],
        data_layout: wgpu::TexelCopyBufferLayout,
        size: wgpu::Extent3d,
    );

    /// The queue a `create_texture_with_data` stages its `bytes` of initial
    /// data on, metered as one write. Only the creation chokepoint
    /// (`GpuCreate::new_texture_with_data`) calls it.
    fn queue_for_initial_data(&self, bytes: u64) -> &wgpu::Queue;
}

impl QueueWrites for wgpu::Queue {
    fn write_buffer(&self, buffer: &wgpu::Buffer, offset: wgpu::BufferAddress, data: &[u8]) {
        wgpu::Queue::write_buffer(self, buffer, offset, data);
    }

    fn write_texture(
        &self,
        texture: wgpu::TexelCopyTextureInfo<'_>,
        data: &[u8],
        data_layout: wgpu::TexelCopyBufferLayout,
        size: wgpu::Extent3d,
    ) {
        wgpu::Queue::write_texture(self, texture, data, data_layout, size);
    }

    fn queue_for_initial_data(&self, _bytes: u64) -> &wgpu::Queue {
        self
    }
}
