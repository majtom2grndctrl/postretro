// One level's reflection probe records: the colour and distance arrays, the
// share table and the live-lighting sums, with the sums pass's group over
// them, built at level install whatever surfaces read them.
// See: context/lib/rendering_pipeline.md §7.3a (Live lighting) · context/lib/resource_management.md §5.3

use crate::render::gpu_create::GpuCreate;
use std::num::NonZeroU64;

use postretro_level_format::SectionId;
use postretro_level_format::reflection_probes::ReflectionProbesSection;
use postretro_render_cpu::fluid_surface::prepared::ProbeArrayFormat;
pub(crate) use postretro_render_cpu::fluid_surface::prepared::installable_probe_section;
use postretro_render_cpu::fluid_surface::reflection::{
    FLUID_PROBE_SUMS_BYTES, FLUID_PROBE_TABLE_BYTES, pack_probe_table,
};

use super::reflection_probe_residency::{
    REFLECTION_PROBE_COLOUR, REFLECTION_PROBE_DISTANCE, REFLECTION_PROBE_SHARE_TABLE,
    REFLECTION_PROBE_SUMS, ReflectionProbeResidencyReport,
};
use super::residency::{
    ResidencyAllocation, ResidencyAllocationShape, ResidencyAllocationState, sources, texture_row,
};

/// The sums pass's own group: the sums it writes, then the share table it
/// reads.
pub(crate) const PROBE_SUMS_PASS_SUMS_BINDING: u32 = 0;
pub(crate) const PROBE_SUMS_PASS_TABLE_BINDING: u32 = 1;

/// The sums pass's own group layout entries: the sums, its one writable
/// binding, and the share table it reads. IP rule (owner review to change,
/// rendering_pipeline.md §7.3a): the pass writes nothing but the sums.
pub(crate) fn probe_sums_pass_bind_group_layout_entries() -> [wgpu::BindGroupLayoutEntry; 2] {
    [
        wgpu::BindGroupLayoutEntry {
            binding: PROBE_SUMS_PASS_SUMS_BINDING,
            visibility: wgpu::ShaderStages::COMPUTE,
            ty: wgpu::BindingType::Buffer {
                ty: wgpu::BufferBindingType::Storage { read_only: false },
                has_dynamic_offset: false,
                min_binding_size: NonZeroU64::new(FLUID_PROBE_SUMS_BYTES as u64),
            },
            count: None,
        },
        wgpu::BindGroupLayoutEntry {
            binding: PROBE_SUMS_PASS_TABLE_BINDING,
            visibility: wgpu::ShaderStages::COMPUTE,
            ty: wgpu::BindingType::Buffer {
                ty: wgpu::BufferBindingType::Uniform,
                has_dynamic_offset: false,
                min_binding_size: NonZeroU64::new(FLUID_PROBE_TABLE_BYTES as u64),
            },
            count: None,
        },
    ]
}

/// The section a level installs probes from: an installable one that binds
/// some face, fluid or world. Anything else allocates no probe resource.
pub(crate) fn level_probe_section(
    section: Option<&ReflectionProbesSection>,
) -> Option<&ReflectionProbesSection> {
    installable_probe_section(section)
        .filter(|section| !section.fluid_bindings.is_empty() || !section.world_bindings.is_empty())
}

/// What a level's probe build binds from the renderer: the sums pass's own
/// group layout, cloned for a level build on any thread.
#[derive(Clone)]
pub(crate) struct ReflectionProbeLevelLayout {
    pub(super) sums_layout: wgpu::BindGroupLayout,
}

impl ReflectionProbeLevelLayout {
    /// Upload the level's probes: `None` (no [`level_probe_section`])
    /// allocates nothing. An install-time upload takes the raw queue, as
    /// the SH atlases do.
    pub(crate) fn build(
        &self,
        device: &wgpu::Device,
        queue: &dyn crate::render::queue_writes::QueueWrites,
        section: Option<&ReflectionProbesSection>,
    ) -> Option<ReflectionProbeResources> {
        let section = level_probe_section(section)?;
        let probes = ReflectionProbeResources::new(device, queue, section, &self.sums_layout);
        log::info!("{}", probes.residency_report().log_line());
        Some(probes)
    }
}

/// The sums buffer's usage: written by the sums pass as storage, read by
/// probe-term fragments as a uniform; one buffer, so no copy sits between
/// them. Tests also read it back and prefill it.
fn sums_usage() -> wgpu::BufferUsages {
    let usage = wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::UNIFORM;
    #[cfg(test)]
    let usage = usage | wgpu::BufferUsages::COPY_SRC | wgpu::BufferUsages::COPY_DST;
    usage
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
/// sums, with the sums pass's group over them. The level set owns them, so
/// they drop with it at release; no pass or renderer-lifetime state holds the
/// sums or a group over them. Probe-term surfaces borrow them into their own
/// groups. A level with fluid surfaces and no probes binds
/// [`Self::dummies`], which the fluid install owns.
pub(crate) struct ReflectionProbeResources {
    array: wgpu::Texture,
    array_view: wgpu::TextureView,
    table: wgpu::Buffer,
    distance: wgpu::Texture,
    distance_view: wgpu::TextureView,
    /// One RGB sum per lobe per probe at the cap, rewritten by the sums pass
    /// on every frame with a probe-term draw. Never the table or a tile.
    sums: wgpu::Buffer,
    /// The installed probes the sums pass evaluates; 0 for the dummies.
    probe_count: u32,
    /// The sums pass's group 2, over `sums` and `table`; `None` for the
    /// dummies, which never dispatch.
    sums_bind_group: Option<wgpu::BindGroup>,
}

impl ReflectionProbeResources {
    /// Upload `section` (already a [`level_probe_section`]). Only
    /// [`ReflectionProbeLevelLayout::build`] calls this.
    fn new(
        device: &wgpu::Device,
        queue: &dyn crate::render::queue_writes::QueueWrites,
        section: &ReflectionProbesSection,
        sums_layout: &wgpu::BindGroupLayout,
    ) -> Self {
        Self::create(device, queue, Some(section), Some(sums_layout))
    }

    /// The dummies a fixed probe layout needs where the level has no probes:
    /// a 1×1 one-layer zero array, a zeroed table, a 1×1 one-layer zero
    /// distance array and zeroed sums, with no sums group, so the sums pass
    /// never dispatches over them. No fragment reads any of them, since every
    /// probe index is then the no-probe value.
    pub(crate) fn dummies(
        device: &wgpu::Device,
        queue: &dyn crate::render::queue_writes::QueueWrites,
    ) -> Self {
        Self::create(device, queue, None, None)
    }

    fn create(
        device: &wgpu::Device,
        queue: &dyn crate::render::queue_writes::QueueWrites,
        section: Option<&ReflectionProbesSection>,
        sums_layout: Option<&wgpu::BindGroupLayout>,
    ) -> Self {
        // The table packs this same section, so the arrays' layers, the
        // table's probe count (the sums shader's bound) and the dispatch's
        // probe count share one source.
        let array = match section.and_then(|section| Some((section, probe_array_format(section)?)))
        {
            Some((section, format)) => {
                let array = device.new_texture_with_data(
                    queue,
                    &array_descriptor(
                        "Reflection Probe Array",
                        format,
                        section.tile_dimension,
                        section.colour_mip_count,
                        section.probes.len() as u32,
                    ),
                    wgpu::util::TextureDataOrder::LayerMajor,
                    &section.tiles,
                );
                log::info!(
                    "[Renderer] Reflection probes installed: {} probes, {} {}² tiles × {} mip(s) \
                     ({} B), R16Float distance tiles ({} B), {} fluid and {} world bound faces, \
                     share table {} B",
                    section.probes.len(),
                    format_label(format),
                    section.tile_dimension,
                    section.colour_mip_count,
                    section.tiles.len(),
                    section.distances.len(),
                    section.fluid_bindings.len(),
                    section.world_bindings.len(),
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
                    1,
                ),
                wgpu::util::TextureDataOrder::LayerMajor,
                &[0u8; 8],
            ),
        };
        let array_view = array.create_view(&wgpu::TextureViewDescriptor {
            label: Some("Reflection Probe Array View"),
            dimension: Some(wgpu::TextureViewDimension::D2Array),
            ..Default::default()
        });
        let table = device.new_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("Reflection Probe Table"),
            contents: &pack_probe_table(section),
            usage: wgpu::BufferUsages::UNIFORM,
        });
        // One distance tile per probe in the colour tiles' layers, uploaded
        // as stored (little-endian f16); 0 is no surface.
        let distance = match section {
            Some(section) => device.new_texture_with_data(
                queue,
                &array_descriptor(
                    "Reflection Probe Distance Array",
                    wgpu::TextureFormat::R16Float,
                    section.tile_dimension,
                    1,
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
                    1,
                ),
                wgpu::util::TextureDataOrder::LayerMajor,
                &[0u8; 2],
            ),
        };
        let distance_view = distance.create_view(&wgpu::TextureViewDescriptor {
            label: Some("Reflection Probe Distance Array View"),
            dimension: Some(wgpu::TextureViewDimension::D2Array),
            ..Default::default()
        });
        // Zeroed at creation; the sums pass writes every installed probe's
        // sums before any fragment reads them.
        let sums = device.new_buffer(&wgpu::BufferDescriptor {
            label: Some("Reflection Probe Sums"),
            size: FLUID_PROBE_SUMS_BYTES as u64,
            usage: sums_usage(),
            mapped_at_creation: false,
        });
        let probe_count = section.map_or(0, |section| section.probes.len() as u32);
        let sums_bind_group = sums_layout.filter(|_| probe_count > 0).map(|sums_layout| {
            device.new_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("Reflection Probe Sums Pass Bind Group"),
                layout: sums_layout,
                entries: &[
                    wgpu::BindGroupEntry {
                        binding: PROBE_SUMS_PASS_SUMS_BINDING,
                        resource: sums.as_entire_binding(),
                    },
                    wgpu::BindGroupEntry {
                        binding: PROBE_SUMS_PASS_TABLE_BINDING,
                        resource: table.as_entire_binding(),
                    },
                ],
            })
        });
        Self {
            array,
            array_view,
            table,
            distance,
            distance_view,
            sums,
            probe_count,
            sums_bind_group,
        }
    }

    /// The sums pass's group and the probes it evaluates, or `None` for the
    /// dummies.
    pub(crate) fn sums_dispatch(&self) -> Option<(&wgpu::BindGroup, u32)> {
        self.sums_bind_group
            .as_ref()
            .map(|group| (group, self.probe_count))
    }

    /// The colour array, one capture tile chain per layer.
    pub(crate) fn colour_view(&self) -> &wgpu::TextureView {
        &self.array_view
    }

    /// The share table, a uniform at the cap.
    pub(crate) fn table_binding(&self) -> wgpu::BindingResource<'_> {
        self.table.as_entire_binding()
    }

    /// The distance array, in the colour array's layers.
    pub(crate) fn distance_view(&self) -> &wgpu::TextureView {
        &self.distance_view
    }

    /// The live-lighting sums, read as a uniform by probe-term fragments.
    pub(crate) fn sums_binding(&self) -> wgpu::BindingResource<'_> {
        self.sums.as_entire_binding()
    }

    /// The memory ledger's probe rows: every colour mip, the distance
    /// array, the share table and the sums, read back from what is bound.
    pub(crate) fn residency_report(&self) -> ReflectionProbeResidencyReport {
        let section = [SectionId::ReflectionProbes as u16];
        let state = ResidencyAllocationState::Data;
        ReflectionProbeResidencyReport::new([
            texture_row(REFLECTION_PROBE_COLOUR, &self.array, &section, state),
            texture_row(REFLECTION_PROBE_DISTANCE, &self.distance, &section, state),
            buffer_row(REFLECTION_PROBE_SHARE_TABLE, &self.table, &section, false),
            buffer_row(REFLECTION_PROBE_SUMS, &self.sums, &section, true),
        ])
    }

    /// The sums buffer, for readback and prefill (tests).
    #[cfg(test)]
    pub(crate) fn sums_buffer(&self) -> &wgpu::Buffer {
        &self.sums
    }

    /// The probe array's tile width, height and layer count.
    #[cfg(test)]
    pub(crate) fn array_size(&self) -> wgpu::Extent3d {
        self.array.size()
    }

    /// The distance array's tile width, height and layer count.
    #[cfg(test)]
    pub(crate) fn distance_size(&self) -> wgpu::Extent3d {
        self.distance.size()
    }

    /// The colour and distance arrays themselves, for their usage and mip
    /// count (tests).
    #[cfg(test)]
    pub(crate) fn arrays(&self) -> [&wgpu::Texture; 2] {
        [&self.array, &self.distance]
    }
}

fn buffer_row(
    name: &'static str,
    buffer: &wgpu::Buffer,
    section_ids: &[u16],
    derived: bool,
) -> ResidencyAllocation {
    ResidencyAllocation {
        name,
        sources: sources(section_ids, derived),
        bytes: buffer.size(),
        state: ResidencyAllocationState::Data,
        shape: ResidencyAllocationShape::Buffer {
            binding_bytes: buffer.size(),
        },
    }
}

/// `mip_levels` is the colour chain's count (id 54 stores each probe's mips
/// in sequence, which is `LayerMajor` order), 1 for the distance tiles.
fn array_descriptor(
    label: &'static str,
    format: wgpu::TextureFormat,
    dimension: u32,
    mip_levels: u32,
    layers: u32,
) -> wgpu::TextureDescriptor<'static> {
    wgpu::TextureDescriptor {
        label: Some(label),
        size: wgpu::Extent3d {
            width: dimension,
            height: dimension,
            depth_or_array_layers: layers,
        },
        mip_level_count: mip_levels,
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

#[cfg(test)]
#[path = "reflection_probe_resources_tests.rs"]
mod tests;
