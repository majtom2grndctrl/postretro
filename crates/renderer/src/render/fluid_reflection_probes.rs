// Fluid reflection probe resources: the probe array, share table, distance
// array and live-lighting sums that join the fluid pass's level-scoped group 4,
// and the sums pass's group over the sums and share table, created at level
// install.
// See: context/lib/rendering_pipeline.md §7.3a, §10 · context/lib/resource_management.md §5

use crate::render::gpu_create::GpuCreate;
use std::num::NonZeroU64;

use postretro_level_format::reflection_probes::ReflectionProbesSection;
use postretro_render_cpu::fluid_surface::prepared::ProbeArrayFormat;
pub(crate) use postretro_render_cpu::fluid_surface::prepared::installable_probe_section;
use postretro_render_cpu::fluid_surface::reflection::{
    FLUID_PROBE_SUMS_BYTES, FLUID_PROBE_TABLE_BYTES, pack_probe_table,
};

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
/// The sums pass's own group: the sums it writes, then the share table it
/// reads, over the buffers group 4 binds.
pub(crate) const FLUID_SUMS_PASS_SUMS_BINDING: u32 = 0;
pub(crate) const FLUID_SUMS_PASS_TABLE_BINDING: u32 = 1;

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

/// The sums pass's own group: the sums, its one writable binding, and the
/// share table it reads. IP rule (owner review to change,
/// rendering_pipeline.md §7.3a): the pass writes nothing but the sums.
pub(crate) fn fluid_sums_pass_bind_group_layout_entries() -> [wgpu::BindGroupLayoutEntry; 2] {
    [
        wgpu::BindGroupLayoutEntry {
            binding: FLUID_SUMS_PASS_SUMS_BINDING,
            visibility: wgpu::ShaderStages::COMPUTE,
            ty: wgpu::BindingType::Buffer {
                ty: wgpu::BufferBindingType::Storage { read_only: false },
                has_dynamic_offset: false,
                min_binding_size: NonZeroU64::new(FLUID_PROBE_SUMS_BYTES as u64),
            },
            count: None,
        },
        uniform_entry(
            FLUID_SUMS_PASS_TABLE_BINDING,
            wgpu::ShaderStages::COMPUTE,
            FLUID_PROBE_TABLE_BYTES,
        ),
    ]
}

/// The sums buffer's usage: written by the sums pass as storage, read by the
/// fluid fragment as a uniform; one buffer, so no copy sits between them.
/// Tests also read it back and prefill it.
fn sums_usage() -> wgpu::BufferUsages {
    let usage = wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::UNIFORM;
    #[cfg(any(test, feature = "test-seams"))]
    let usage = usage | wgpu::BufferUsages::COPY_SRC | wgpu::BufferUsages::COPY_DST;
    usage
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

/// BC6H tiles stay compressed through upload, as the SH atlases do: the
/// device requires `TEXTURE_COMPRESSION_BC` at init (see
/// `renderer_init_resources.rs`), so no adapter reaches here without it. The
/// uncompressed debug tag uploads as `Rgba16Float` through the same binding.
fn probe_array_format(section: &ReflectionProbesSection) -> Option<wgpu::TextureFormat> {
    postretro_render_cpu::fluid_surface::prepared::probe_array_format(section).map(|format| {
        match format {
            ProbeArrayFormat::Bc6hRgbUfloat => wgpu::TextureFormat::Bc6hRgbUfloat,
            ProbeArrayFormat::Rgba16Float => wgpu::TextureFormat::Rgba16Float,
        }
    })
}

/// One level's probe array, share table, distance array and live-lighting
/// sums, with the sums pass's group over them. Owned by the installed fluid
/// surfaces, so all drop with them at clear and unload; no pass or
/// renderer-lifetime state holds the sums or a group over them.
pub(crate) struct FluidProbeResources {
    // Held so the level, not the bind group alone, owns the textures' life.
    _array: wgpu::Texture,
    array_view: wgpu::TextureView,
    table: wgpu::Buffer,
    _distance: wgpu::Texture,
    distance_view: wgpu::TextureView,
    /// One RGB sum per lobe per probe at the cap, rewritten by the sums pass
    /// on every frame with a reflecting draw. Never the table or a tile.
    sums: wgpu::Buffer,
    /// The installed probes the sums pass evaluates; 0 for the dummies.
    probe_count: u32,
    /// The sums pass's group 2, over `sums` and `table`; `None` with no
    /// installed probe, which never dispatches.
    sums_bind_group: Option<wgpu::BindGroup>,
}

impl FluidProbeResources {
    /// Upload `section` (already [`installable_probe_section`]); `None`
    /// builds the dummies group 4's fixed layout needs: a 1×1 one-layer
    /// zero array, a zeroed table and a 1×1 one-layer zero distance array,
    /// read by no fragment since every probe index is then the no-probe
    /// value.
    pub(crate) fn new(
        device: &wgpu::Device,
        queue: &dyn crate::render::queue_writes::QueueWrites,
        section: Option<&ReflectionProbesSection>,
        sums_layout: &wgpu::BindGroupLayout,
    ) -> Self {
        let array = match section.and_then(|section| Some((section, probe_array_format(section)?)))
        {
            Some((section, format)) => {
                let array = device.new_texture_with_data(
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
                     ({} B), R16Float distance tiles ({} B), {} bound faces, share table {} B",
                    section.probes.len(),
                    format_label(format),
                    section.tile_dimension,
                    section.tiles.len(),
                    section.distances.len(),
                    section.bindings.len(),
                    FLUID_PROBE_TABLE_BYTES,
                );
                array
            }
            None => device.new_texture_with_data(
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
        let table = device.new_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("Fluid Reflection Probe Table"),
            contents: &pack_probe_table(section),
            usage: wgpu::BufferUsages::UNIFORM,
        });
        // One distance tile per probe in the colour tiles' layers, uploaded
        // as stored (little-endian f16); 0 is no surface.
        let distance = match section.filter(|section| probe_array_format(section).is_some()) {
            Some(section) => device.new_texture_with_data(
                queue,
                &array_descriptor(
                    "Fluid Reflection Probe Distance Array",
                    wgpu::TextureFormat::R16Float,
                    section.tile_dimension,
                    section.probes.len() as u32,
                ),
                wgpu::util::TextureDataOrder::LayerMajor,
                &section.distances,
            ),
            None => device.new_texture_with_data(
                queue,
                &array_descriptor(
                    "Fluid Reflection Probe Distance Array Dummy",
                    wgpu::TextureFormat::R16Float,
                    1,
                    1,
                ),
                wgpu::util::TextureDataOrder::LayerMajor,
                &[0u8; 2],
            ),
        };
        let distance_view = distance.create_view(&wgpu::TextureViewDescriptor {
            label: Some("Fluid Reflection Probe Distance Array View"),
            dimension: Some(wgpu::TextureViewDimension::D2Array),
            ..Default::default()
        });
        // Zeroed at creation; the sums pass writes every installed probe's
        // sums before any fragment reads them.
        let sums = device.new_buffer(&wgpu::BufferDescriptor {
            label: Some("Fluid Reflection Probe Sums"),
            size: FLUID_PROBE_SUMS_BYTES as u64,
            usage: sums_usage(),
            mapped_at_creation: false,
        });
        let probe_count = section
            .filter(|section| probe_array_format(section).is_some())
            .map_or(0, |section| section.probes.len() as u32);
        let sums_bind_group = (probe_count > 0).then(|| {
            device.new_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("Fluid Reflection Sums Pass Bind Group"),
                layout: sums_layout,
                entries: &[
                    wgpu::BindGroupEntry {
                        binding: FLUID_SUMS_PASS_SUMS_BINDING,
                        resource: sums.as_entire_binding(),
                    },
                    wgpu::BindGroupEntry {
                        binding: FLUID_SUMS_PASS_TABLE_BINDING,
                        resource: table.as_entire_binding(),
                    },
                ],
            })
        });
        Self {
            _array: array,
            array_view,
            table,
            _distance: distance,
            distance_view,
            sums,
            probe_count,
            sums_bind_group,
        }
    }

    /// The sums pass's group and the probes it evaluates, or `None` with no
    /// installed probe.
    pub(crate) fn sums_dispatch(&self) -> Option<(&wgpu::BindGroup, u32)> {
        self.sums_bind_group
            .as_ref()
            .map(|group| (group, self.probe_count))
    }

    /// Group 4's sums entry.
    pub(crate) fn sums_bind_group_entry(&self) -> wgpu::BindGroupEntry<'_> {
        wgpu::BindGroupEntry {
            binding: FLUID_PROBE_SUMS_BINDING,
            resource: self.sums.as_entire_binding(),
        }
    }

    /// The sums buffer, for readback and prefill (tests).
    #[cfg(any(test, feature = "test-seams"))]
    pub(crate) fn sums_buffer(&self) -> &wgpu::Buffer {
        &self.sums
    }

    /// Group 4's probe array, share table and distance array entries.
    pub(crate) fn bind_group_entries(&self) -> [wgpu::BindGroupEntry<'_>; 3] {
        [
            wgpu::BindGroupEntry {
                binding: FLUID_PROBE_ARRAY_BINDING,
                resource: wgpu::BindingResource::TextureView(&self.array_view),
            },
            wgpu::BindGroupEntry {
                binding: FLUID_PROBE_TABLE_BINDING,
                resource: self.table.as_entire_binding(),
            },
            wgpu::BindGroupEntry {
                binding: FLUID_PROBE_DISTANCE_BINDING,
                resource: wgpu::BindingResource::TextureView(&self.distance_view),
            },
        ]
    }

    /// The probe array's tile width, height and layer count.
    #[cfg(test)]
    pub(crate) fn array_size(&self) -> wgpu::Extent3d {
        self._array.size()
    }

    /// The distance array's tile width, height and layer count.
    #[cfg(test)]
    pub(crate) fn distance_size(&self) -> wgpu::Extent3d {
        self._distance.size()
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
