// The device limits CPU install plans read, copied from `wgpu::Limits` by the
// renderer so prep can run on a thread with no device.
// See: context/lib/rendering_pipeline.md §4

/// The subset of `wgpu::Limits` the install plans check against. Plain data:
/// the renderer copies it once from its device and hands it to prep.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DeviceLimits {
    pub max_texture_dimension_2d: u32,
    pub max_texture_dimension_3d: u32,
    pub max_texture_array_layers: u32,
    pub max_compute_workgroups_per_dimension: u32,
    pub min_uniform_buffer_offset_alignment: u32,
    pub max_storage_buffer_binding_size: u64,
    pub max_buffer_size: u64,
}
