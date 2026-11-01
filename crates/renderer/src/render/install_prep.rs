// The renderer's side of the CPU install plans: the device limits prep plans
// against, read once from the device so prep needs no GPU handle.
// See: context/lib/rendering_pipeline.md §4

use postretro_render_cpu::device_limits::DeviceLimits;

use super::Renderer;

/// The plan-relevant subset of `limits`.
pub(crate) fn device_limits(limits: &wgpu::Limits) -> DeviceLimits {
    DeviceLimits {
        max_texture_dimension_2d: limits.max_texture_dimension_2d,
        max_texture_dimension_3d: limits.max_texture_dimension_3d,
        max_texture_array_layers: limits.max_texture_array_layers,
        max_compute_workgroups_per_dimension: limits.max_compute_workgroups_per_dimension,
        min_uniform_buffer_offset_alignment: limits.min_uniform_buffer_offset_alignment,
        max_storage_buffer_binding_size: limits.max_storage_buffer_binding_size,
        max_buffer_size: limits.max_buffer_size,
    }
}

impl Renderer {
    /// The device limits every CPU install plan checks against. Plain data:
    /// copy it to prep, which plans with no device.
    pub fn install_device_limits(&self) -> DeviceLimits {
        device_limits(&self.device.limits())
    }
}
