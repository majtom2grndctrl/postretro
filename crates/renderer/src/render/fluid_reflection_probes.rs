// Fluid reflection probe resources: the probe array and share table that join
// the fluid pass's level-scoped group 4, created once at level install.
// See: context/lib/rendering_pipeline.md §7.3a, §10 · context/lib/resource_management.md §5

use std::num::NonZeroU64;

use wgpu::util::DeviceExt;

use postretro_level_format::environment_volumes::EnvironmentVolumeRecord;
use postretro_level_format::lightmap::{IRRADIANCE_FORMAT_BC6H, IRRADIANCE_FORMAT_RGBA16F};
use postretro_level_format::reflection_probes::{
    ReflectionProbesSection, reflection_probe_tile_bytes,
};
use postretro_render_cpu::fluid_surface::FluidFaceDriftMap;
use postretro_render_cpu::fluid_surface::reflection::{
    FLUID_PROBE_TABLE_BYTES, FLUID_REFLECTION_NO_PROBE, fill_probe_indices, pack_probe_table,
    usable_probe_section,
};

/// Group 4 binding of the probe array: one capture tile per layer.
pub(crate) const FLUID_PROBE_ARRAY_BINDING: u32 = 1;
/// Group 4 binding of the share table: one non-dynamic uniform at the cap.
pub(crate) const FLUID_PROBE_TABLE_BINDING: u32 = 2;

/// The probe array and share table entries of group 4. The array samples
/// through the SH atlas's linear-clamp sampler (group 3 binding 2), so the
/// pass adds no sampler.
pub(crate) fn fluid_probe_bind_group_layout_entries() -> [wgpu::BindGroupLayoutEntry; 2] {
    [
        wgpu::BindGroupLayoutEntry {
            binding: FLUID_PROBE_ARRAY_BINDING,
            visibility: wgpu::ShaderStages::FRAGMENT,
            ty: wgpu::BindingType::Texture {
                sample_type: wgpu::TextureSampleType::Float { filterable: true },
                view_dimension: wgpu::TextureViewDimension::D2Array,
                multisampled: false,
            },
            count: None,
        },
        wgpu::BindGroupLayoutEntry {
            binding: FLUID_PROBE_TABLE_BINDING,
            visibility: wgpu::ShaderStages::FRAGMENT,
            ty: wgpu::BindingType::Buffer {
                ty: wgpu::BufferBindingType::Uniform,
                has_dynamic_offset: false,
                min_binding_size: NonZeroU64::new(FLUID_PROBE_TABLE_BYTES as u64),
            },
            count: None,
        },
    ]
}

/// The section whose probes this renderer installs: a usable one whose tile
/// payload matches its header. The loader refuses anything else; a
/// hand-built section that slips past reflects nothing rather than failing
/// texture creation. Install feeds the same answer to the table, the probe
/// indices and the array, so a face never samples a probe that is not there.
pub(crate) fn installable_probe_section(
    section: Option<&ReflectionProbesSection>,
) -> Option<&ReflectionProbesSection> {
    usable_probe_section(section).filter(|section| {
        probe_array_format(section).is_some()
            && reflection_probe_tile_bytes(section.atlas_format, section.tile_dimension)
                .is_some_and(|bytes| bytes * section.probes.len() == section.tiles.len())
    })
}

/// The fluid mesh's third vertex buffer: one probe index per vertex of the
/// mesh `face_maps` describe, written once here and never again.
pub(crate) fn create_probe_index_buffer(
    device: &wgpu::Device,
    volumes: &[EnvironmentVolumeRecord],
    face_maps: &[Option<FluidFaceDriftMap>],
    vertex_count: usize,
    section: Option<&ReflectionProbesSection>,
) -> wgpu::Buffer {
    let mut indices = vec![FLUID_REFLECTION_NO_PROBE; vertex_count];
    fill_probe_indices(volumes, face_maps, section, &mut indices);
    device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
        label: Some("Fluid Surface Probe Index Buffer"),
        contents: bytemuck::cast_slice(&indices),
        usage: wgpu::BufferUsages::VERTEX,
    })
}

/// BC6H tiles stay compressed through upload, as the SH atlases do: the
/// device requires `TEXTURE_COMPRESSION_BC` at init (see
/// `renderer_init_resources.rs`), so no adapter reaches here without it. The
/// uncompressed debug tag uploads as `Rgba16Float` through the same binding.
fn probe_array_format(section: &ReflectionProbesSection) -> Option<wgpu::TextureFormat> {
    match section.atlas_format {
        IRRADIANCE_FORMAT_BC6H if section.tile_dimension.is_multiple_of(4) => {
            Some(wgpu::TextureFormat::Bc6hRgbUfloat)
        }
        IRRADIANCE_FORMAT_RGBA16F => Some(wgpu::TextureFormat::Rgba16Float),
        _ => None,
    }
}

/// One level's probe array and share table. Owned by the installed fluid
/// surfaces, so both drop with them at clear and unload.
pub(crate) struct FluidProbeResources {
    // Held so the level, not the bind group alone, owns the texture's life.
    _array: wgpu::Texture,
    array_view: wgpu::TextureView,
    table: wgpu::Buffer,
}

impl FluidProbeResources {
    /// Upload `section` (already [`installable_probe_section`]); `None`
    /// builds the dummies group 4's fixed layout needs: a 1×1 one-layer
    /// zero array and a zeroed table, read by no fragment since every probe
    /// index is then the no-probe value.
    pub(crate) fn new(
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        section: Option<&ReflectionProbesSection>,
    ) -> Self {
        let array = match section.and_then(|section| Some((section, probe_array_format(section)?)))
        {
            Some((section, format)) => {
                let array = device.create_texture_with_data(
                    queue,
                    &array_descriptor(
                        "Fluid Reflection Probe Array",
                        format,
                        section.tile_dimension,
                        section.probes.len() as u32,
                    ),
                    wgpu::util::TextureDataOrder::LayerMajor,
                    &section.tiles,
                );
                log::info!(
                    "[Renderer] Fluid reflection probes installed: {} probes, {} {}² tiles \
                     ({} B), {} bound faces, share table {} B",
                    section.probes.len(),
                    format_label(format),
                    section.tile_dimension,
                    section.tiles.len(),
                    section.bindings.len(),
                    FLUID_PROBE_TABLE_BYTES,
                );
                array
            }
            None => device.create_texture_with_data(
                queue,
                &array_descriptor(
                    "Fluid Reflection Probe Array Dummy",
                    wgpu::TextureFormat::Rgba16Float,
                    1,
                    1,
                ),
                wgpu::util::TextureDataOrder::LayerMajor,
                &[0u8; 8],
            ),
        };
        let array_view = array.create_view(&wgpu::TextureViewDescriptor {
            label: Some("Fluid Reflection Probe Array View"),
            dimension: Some(wgpu::TextureViewDimension::D2Array),
            ..Default::default()
        });
        let table = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("Fluid Reflection Probe Table"),
            contents: &pack_probe_table(section),
            usage: wgpu::BufferUsages::UNIFORM,
        });
        Self {
            _array: array,
            array_view,
            table,
        }
    }

    /// Group 4's probe array and share table entries.
    pub(crate) fn bind_group_entries(&self) -> [wgpu::BindGroupEntry<'_>; 2] {
        [
            wgpu::BindGroupEntry {
                binding: FLUID_PROBE_ARRAY_BINDING,
                resource: wgpu::BindingResource::TextureView(&self.array_view),
            },
            wgpu::BindGroupEntry {
                binding: FLUID_PROBE_TABLE_BINDING,
                resource: self.table.as_entire_binding(),
            },
        ]
    }

    #[cfg(test)]
    pub(crate) fn array_layers(&self) -> u32 {
        self._array.depth_or_array_layers()
    }
}

fn array_descriptor(
    label: &'static str,
    format: wgpu::TextureFormat,
    dimension: u32,
    layers: u32,
) -> wgpu::TextureDescriptor<'static> {
    wgpu::TextureDescriptor {
        label: Some(label),
        size: wgpu::Extent3d {
            width: dimension,
            height: dimension,
            depth_or_array_layers: layers,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format,
        usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
        view_formats: &[],
    }
}

fn format_label(format: wgpu::TextureFormat) -> &'static str {
    match format {
        wgpu::TextureFormat::Bc6hRgbUfloat => "BC6H",
        _ => "Rgba16Float",
    }
}
