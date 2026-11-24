// The fluid pass's group-4 reflection probe bindings: the probe array, share
// table, distance array and live-lighting sums it borrows from the level's
// probe records, and the fluid mesh's per-vertex probe index stream.
// See: context/lib/rendering_pipeline.md §7.3a, §10 · context/lib/resource_management.md §5

use crate::render::gpu_create::GpuCreate;
use std::num::NonZeroU64;

use postretro_render_cpu::fluid_surface::reflection::{
    FLUID_PROBE_SUMS_BYTES, FLUID_PROBE_TABLE_BYTES,
};

use crate::render::reflection_probe_resources::ReflectionProbeResources;

/// Group 4 binding of the probe array: one capture tile per layer.
pub(crate) const FLUID_PROBE_ARRAY_BINDING: u32 = 1;
/// Group 4 binding of the share table: one non-dynamic uniform at the cap.
pub(crate) const FLUID_PROBE_TABLE_BINDING: u32 = 2;
/// Group 4 binding of the distance array: one R16Float distance tile per
/// probe, in the probe array's layers.
pub(crate) const FLUID_PROBE_DISTANCE_BINDING: u32 = 3;
/// Group 4 binding of the live-lighting sums, read as a uniform by the
/// reflecting fragment entry alone: fragment storage is spent (§10).
pub(crate) const FLUID_PROBE_SUMS_BINDING: u32 = 4;

/// A filterable, fragment-visible 2D-array texture entry of group 4.
fn filterable_array_entry(binding: u32) -> wgpu::BindGroupLayoutEntry {
    wgpu::BindGroupLayoutEntry {
        binding,
        visibility: wgpu::ShaderStages::FRAGMENT,
        ty: wgpu::BindingType::Texture {
            sample_type: wgpu::TextureSampleType::Float { filterable: true },
            view_dimension: wgpu::TextureViewDimension::D2Array,
            multisampled: false,
        },
        count: None,
    }
}

/// A non-dynamic uniform entry of `size` bytes visible to `visibility`.
fn uniform_entry(
    binding: u32,
    visibility: wgpu::ShaderStages,
    size: usize,
) -> wgpu::BindGroupLayoutEntry {
    wgpu::BindGroupLayoutEntry {
        binding,
        visibility,
        ty: wgpu::BindingType::Buffer {
            ty: wgpu::BufferBindingType::Uniform,
            has_dynamic_offset: false,
            min_binding_size: NonZeroU64::new(size as u64),
        },
        count: None,
    }
}

/// The probe array, share table, distance array and sums entries of group 4.
/// Both arrays sample through the SH atlas's linear-clamp sampler (group 3
/// binding 2), so the pass adds no sampler.
pub(crate) fn fluid_probe_bind_group_layout_entries() -> [wgpu::BindGroupLayoutEntry; 4] {
    [
        filterable_array_entry(FLUID_PROBE_ARRAY_BINDING),
        uniform_entry(
            FLUID_PROBE_TABLE_BINDING,
            wgpu::ShaderStages::FRAGMENT,
            FLUID_PROBE_TABLE_BYTES,
        ),
        filterable_array_entry(FLUID_PROBE_DISTANCE_BINDING),
        uniform_entry(
            FLUID_PROBE_SUMS_BINDING,
            wgpu::ShaderStages::FRAGMENT,
            FLUID_PROBE_SUMS_BYTES,
        ),
    ]
}

/// The fluid mesh's third vertex buffer: the prepared per-vertex probe
/// indices (`PreparedFluidSurfaces::probe_indices`), written once here and
/// never again.
pub(crate) fn create_probe_index_buffer(device: &wgpu::Device, indices: &[u32]) -> wgpu::Buffer {
    device.new_buffer_init(&wgpu::util::BufferInitDescriptor {
        label: Some("Fluid Surface Probe Index Buffer"),
        contents: bytemuck::cast_slice(indices),
        usage: wgpu::BufferUsages::VERTEX,
    })
}

/// Group 4's probe array, share table, distance array and sums entries over
/// `probes`: the level's own, or the fluid install's dummies.
pub(crate) fn fluid_probe_bind_group_entries(
    probes: &ReflectionProbeResources,
) -> [wgpu::BindGroupEntry<'_>; 4] {
    [
        wgpu::BindGroupEntry {
            binding: FLUID_PROBE_ARRAY_BINDING,
            resource: wgpu::BindingResource::TextureView(probes.colour_view()),
        },
        wgpu::BindGroupEntry {
            binding: FLUID_PROBE_TABLE_BINDING,
            resource: probes.table_binding(),
        },
        wgpu::BindGroupEntry {
            binding: FLUID_PROBE_DISTANCE_BINDING,
            resource: wgpu::BindingResource::TextureView(probes.distance_view()),
        },
        wgpu::BindGroupEntry {
            binding: FLUID_PROBE_SUMS_BINDING,
            resource: probes.sums_binding(),
        },
    ]
}
