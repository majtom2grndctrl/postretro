// Plain-Rust accounting for level-owned SH GPU allocations.
// See: context/lib/rendering_pipeline.md §7.8

use super::sh_allocation::{
    BufferAllocation, ShAllocationKind, TextureAllocation, texture_allocation_bytes,
};

/// A PRL section or renderer-derived input cited by an SH residency row.
///
/// The report intentionally carries no `wgpu` handles or descriptors: capture
/// and other non-renderer consumers only need the allocation decision.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ShResidencySource {
    Section(u16),
    Derived,
}

/// Whether the allocation backs accepted source data or a required valid GPU
/// binding for an unavailable source.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ShResidencyAllocationState {
    Data,
    Dummy,
    Fallback,
}

/// Requested physical shape of a level-owned SH allocation.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ShResidencyAllocationShape {
    Texture {
        format: &'static str,
        dimension: &'static str,
        extent: [u32; 3],
    },
    Buffer {
        binding_bytes: u64,
    },
}

/// One physical texture or buffer created for the installed level.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ShResidencyAllocation {
    pub name: &'static str,
    pub sources: Vec<ShResidencySource>,
    pub bytes: u64,
    pub state: ShResidencyAllocationState,
    pub shape: ShResidencyAllocationShape,
}

/// Renderer-accounted requested SH residency for one completed level install.
///
/// This is allocation accounting rather than a portable driver-memory query:
/// opaque driver padding and resources outside the SH install boundary are not
/// included.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ShResidencyReport {
    pub allocations: Vec<ShResidencyAllocation>,
    pub total_bytes: u64,
}

/// Mutable collector passed only through level-owned SH constructors.
pub(super) struct ShAllocationLedger {
    allocations: Vec<ShResidencyAllocation>,
}

impl ShAllocationLedger {
    pub(super) fn new() -> Self {
        Self {
            allocations: Vec::new(),
        }
    }

    pub(super) fn record_texture(
        &mut self,
        allocation: TextureAllocation,
        section_ids: &[u16],
        derived: bool,
        state: ShResidencyAllocationState,
    ) {
        let bytes = texture_allocation_bytes(allocation);
        self.record(ShResidencyAllocation {
            name: allocation_name(allocation.kind),
            sources: sources(section_ids, derived),
            bytes,
            state,
            shape: ShResidencyAllocationShape::Texture {
                format: texture_format_name(allocation.format),
                dimension: texture_dimension_name(allocation.dimension),
                extent: [
                    allocation.extent.width,
                    allocation.extent.height,
                    allocation.extent.depth_or_array_layers,
                ],
            },
        });
    }

    pub(super) fn record_buffer(
        &mut self,
        allocation: BufferAllocation,
        section_ids: &[u16],
        derived: bool,
        state: ShResidencyAllocationState,
    ) {
        let bytes = allocation.byte_len as u64;
        self.record(ShResidencyAllocation {
            name: allocation_name(allocation.kind),
            sources: sources(section_ids, derived),
            bytes,
            state,
            shape: ShResidencyAllocationShape::Buffer {
                binding_bytes: bytes,
            },
        });
    }

    fn record(&mut self, allocation: ShResidencyAllocation) {
        self.allocations.push(allocation);
    }

    pub(super) fn finish(self) -> ShResidencyReport {
        let total_bytes = self.allocations.iter().map(|row| row.bytes).sum();
        ShResidencyReport {
            allocations: self.allocations,
            total_bytes,
        }
    }
}

pub(super) fn source_ids<const N: usize>(ids: [Option<u16>; N]) -> Vec<u16> {
    ids.into_iter().flatten().collect()
}

fn sources(section_ids: &[u16], derived: bool) -> Vec<ShResidencySource> {
    let mut sources = section_ids
        .iter()
        .copied()
        .map(ShResidencySource::Section)
        .collect::<Vec<_>>();
    if derived || sources.is_empty() {
        sources.push(ShResidencySource::Derived);
    }
    sources
}

fn texture_format_name(format: wgpu::TextureFormat) -> &'static str {
    match format {
        wgpu::TextureFormat::Bc6hRgbUfloat => "Bc6hRgbUfloat",
        wgpu::TextureFormat::Rgba16Float => "Rgba16Float",
        wgpu::TextureFormat::Rgba16Uint => "Rgba16Uint",
        format => panic!("unexpected SH allocation format {format:?}"),
    }
}

fn texture_dimension_name(dimension: wgpu::TextureDimension) -> &'static str {
    match dimension {
        wgpu::TextureDimension::D2 => "D2",
        wgpu::TextureDimension::D3 => "D3",
        wgpu::TextureDimension::D1 => "D1",
    }
}

fn allocation_name(kind: ShAllocationKind) -> &'static str {
    match kind {
        ShAllocationKind::IndirectBaseAtlas => "indirect_base_atlas",
        ShAllocationKind::IndirectTotalAtlas => "indirect_total_atlas",
        ShAllocationKind::DepthMoments => "depth_moments",
        ShAllocationKind::GridInfo => "grid_info",
        ShAllocationKind::AnimatedLightDescriptors => "animated_light_descriptors",
        ShAllocationKind::AnimatedLightSamples => "animated_light_samples",
        ShAllocationKind::ScriptedLightDescriptors => "scripted_light_descriptors",
        ShAllocationKind::DirectBaseAtlas => "direct_base_atlas",
        ShAllocationKind::DirectDynamicParams => "direct_dynamic_params",
        ShAllocationKind::DirectComposedAtlas => "direct_composed_atlas",
        ShAllocationKind::DirectIntermediateAtlas => "direct_intermediate_atlas",
        ShAllocationKind::IndirectComposeDeltaSubblocks => "indirect_compose_delta_subblocks",
        ShAllocationKind::IndirectComposeAffinityOffsets => "indirect_compose_affinity_offsets",
        ShAllocationKind::IndirectComposeAffinityLights => "indirect_compose_affinity_lights",
        ShAllocationKind::IndirectComposeDescriptorIndices => "indirect_compose_descriptor_indices",
        ShAllocationKind::IndirectComposeProbeIndirection => "indirect_compose_probe_indirection",
        ShAllocationKind::IndirectComposeCompactionMetadata => {
            "indirect_compose_compaction_metadata"
        }
        ShAllocationKind::IndirectComposeGrid => "indirect_compose_grid",
        ShAllocationKind::IndirectComposeOrigin => "indirect_compose_origin",
        ShAllocationKind::DirectComposeDeltaSubblocks => "direct_compose_delta_subblocks",
        ShAllocationKind::DirectComposeCompactionMetadata => "direct_compose_compaction_metadata",
        ShAllocationKind::DirectComposeAffinityOffsets => "direct_compose_affinity_offsets",
        ShAllocationKind::DirectComposeAffinityLights => "direct_compose_affinity_lights",
        ShAllocationKind::DirectComposeProbeIndirection => "direct_compose_probe_indirection",
        ShAllocationKind::DirectComposeGrid => "direct_compose_grid",
        ShAllocationKind::DirectComposeDebugOverride => "direct_compose_debug_override",
        ShAllocationKind::DirectComposeLightTermMask => "direct_compose_light_term_mask",
        ShAllocationKind::AnimatedDirectComposeDeltaSubblocks => {
            "animated_direct_compose_delta_subblocks"
        }
        ShAllocationKind::AnimatedDirectComposeCompactionMetadata => {
            "animated_direct_compose_compaction_metadata"
        }
        ShAllocationKind::AnimatedDirectComposeAffinityOffsets => {
            "animated_direct_compose_affinity_offsets"
        }
        ShAllocationKind::AnimatedDirectComposeAffinityLights => {
            "animated_direct_compose_affinity_lights"
        }
        ShAllocationKind::AnimatedDirectComposeDescriptorIndices => {
            "animated_direct_compose_descriptor_indices"
        }
        ShAllocationKind::AnimatedDirectComposeProbeIndirection => {
            "animated_direct_compose_probe_indirection"
        }
        ShAllocationKind::AnimatedDirectComposeGrid => "animated_direct_compose_grid",
        ShAllocationKind::AnimatedDirectComposeLightScale => "animated_direct_compose_light_scale",
        ShAllocationKind::BillboardDirectScatterBaseVolume => {
            "billboard_direct_scatter_base_volume"
        }
        ShAllocationKind::BillboardDirectScatterComposedVolume => {
            "billboard_direct_scatter_composed_volume"
        }
        ShAllocationKind::BillboardComposeGrid => "billboard_compose_grid",
        ShAllocationKind::BillboardComposeDeltas => "billboard_compose_deltas",
        ShAllocationKind::BillboardComposeOffsets => "billboard_compose_offsets",
        ShAllocationKind::BillboardComposeLights => "billboard_compose_lights",
        ShAllocationKind::BillboardComposeDescriptorIndices => {
            "billboard_compose_descriptor_indices"
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::render::sh_allocation::{ShAllocationKind, TextureAllocation};

    #[test]
    fn physical_bc6h_blocks_and_buffer_binding_bytes_drive_the_total() {
        let mut ledger = ShAllocationLedger::new();
        ledger.record_texture(
            TextureAllocation {
                kind: ShAllocationKind::IndirectBaseAtlas,
                format: wgpu::TextureFormat::Bc6hRgbUfloat,
                extent: wgpu::Extent3d {
                    width: 5,
                    height: 7,
                    depth_or_array_layers: 2,
                },
                dimension: wgpu::TextureDimension::D2,
                usage: wgpu::TextureUsages::TEXTURE_BINDING,
            },
            &[34],
            false,
            ShResidencyAllocationState::Data,
        );
        ledger.record_buffer(
            BufferAllocation {
                kind: ShAllocationKind::IndirectComposeAffinityOffsets,
                byte_len: 8,
                usage: wgpu::BufferUsages::STORAGE,
            },
            &[27],
            false,
            ShResidencyAllocationState::Data,
        );

        let report = ledger.finish();
        assert_eq!(report.allocations[0].bytes, 128);
        assert_eq!(report.total_bytes, 136);
        assert_eq!(
            report.allocations[0].shape,
            ShResidencyAllocationShape::Texture {
                format: "Bc6hRgbUfloat",
                dimension: "D2",
                extent: [5, 7, 2],
            }
        );
    }

    #[test]
    fn shared_allocation_is_recorded_once_with_all_of_its_sources() {
        let mut ledger = ShAllocationLedger::new();
        ledger.record_texture(
            TextureAllocation {
                kind: ShAllocationKind::DirectComposedAtlas,
                format: wgpu::TextureFormat::Rgba16Float,
                extent: wgpu::Extent3d {
                    width: 4,
                    height: 4,
                    depth_or_array_layers: 1,
                },
                dimension: wgpu::TextureDimension::D2,
                usage: wgpu::TextureUsages::TEXTURE_BINDING,
            },
            &[35, 41, 45],
            false,
            ShResidencyAllocationState::Data,
        );

        let report = ledger.finish();
        assert_eq!(report.allocations.len(), 1);
        assert_eq!(report.total_bytes, 128);
        assert_eq!(
            report.allocations[0].sources,
            vec![
                ShResidencySource::Section(35),
                ShResidencySource::Section(41),
                ShResidencySource::Section(45),
            ]
        );
    }

    #[test]
    fn absent_and_zero_sized_sources_remain_distinct_through_dummy_state() {
        let mut absent = ShAllocationLedger::new();
        absent.record_texture(
            TextureAllocation {
                kind: ShAllocationKind::IndirectBaseAtlas,
                format: wgpu::TextureFormat::Rgba16Float,
                extent: wgpu::Extent3d {
                    width: 1,
                    height: 1,
                    depth_or_array_layers: 1,
                },
                dimension: wgpu::TextureDimension::D2,
                usage: wgpu::TextureUsages::TEXTURE_BINDING,
            },
            &[],
            false,
            ShResidencyAllocationState::Dummy,
        );
        let mut accepted_empty = ShAllocationLedger::new();
        accepted_empty.record_texture(
            TextureAllocation {
                kind: ShAllocationKind::IndirectBaseAtlas,
                format: wgpu::TextureFormat::Rgba16Float,
                extent: wgpu::Extent3d {
                    width: 1,
                    height: 1,
                    depth_or_array_layers: 1,
                },
                dimension: wgpu::TextureDimension::D2,
                usage: wgpu::TextureUsages::TEXTURE_BINDING,
            },
            &[34],
            false,
            ShResidencyAllocationState::Data,
        );

        let absent = absent.finish();
        let accepted_empty = accepted_empty.finish();
        assert_eq!(
            absent.allocations[0].sources,
            vec![ShResidencySource::Derived]
        );
        assert_eq!(
            absent.allocations[0].state,
            ShResidencyAllocationState::Dummy
        );
        assert_eq!(
            accepted_empty.allocations[0].sources,
            vec![ShResidencySource::Section(34)]
        );
        assert_eq!(
            accepted_empty.allocations[0].state,
            ShResidencyAllocationState::Data
        );
    }
}
