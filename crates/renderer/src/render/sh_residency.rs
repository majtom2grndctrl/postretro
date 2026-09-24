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
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ShResidencyReport {
    pub allocations: Vec<ShResidencyAllocation>,
    pub total_bytes: u64,
    /// Streaming-only live pool accounting. It intentionally remains separate
    /// from descriptor rows: logical occupancy is not another GPU allocation,
    /// and a temporary retiring generation is not fixed metadata.
    pub streaming: Option<ShStreamingAllocationSummary>,
    /// Controller-owned policy and host-payload state captured for the same
    /// frame as `streaming`. The renderer leaves this absent because workers
    /// and permits intentionally live above the renderer boundary.
    pub streaming_lifecycle: Option<ShStreamingLifecycleSummary>,
}

/// Live streamed-SH accounting captured after renderer pool initialization.
///
/// The values describe distinct resource lifetimes. In particular, logical
/// occupancy is a sub-ledger of active capacity, while replacement peak is a
/// temporary active-plus-retiring measurement rather than an additional
/// allocation. Whole-resident billboard scatter (ids 47/48) stays separate so
/// it cannot be claimed as a saving from streamed probe pools.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ShStreamingAllocationSummary {
    pub fixed_metadata_bytes: u64,
    pub whole_resident_scatter_bytes: u64,
    pub active_capacity_bytes: u64,
    pub logical_occupancy_bytes: u64,
    pub retiring_capacity_bytes: u64,
    pub replacement_peak_bytes: u64,
}

/// App-side state paired with the renderer's live streamed-pool snapshot.
/// Values are deliberately plain data so capture can serialize them without
/// retaining a renderer, worker, or controller handle. CPU current values
/// are exact frame snapshots. Their high-water upper bounds sum independently
/// checked worker and controller maxima, so the report cannot understate a
/// transfer but does not claim that sum was one simultaneous peak.
///
/// The fields after `retries` mirror [`super::ShStreamingLiveDiagnostics`]:
/// controller, I/O worker, and renderer install counters for the same frame.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct ShStreamingLifecycleSummary {
    pub non_evictable_overshoot_bytes: u64,
    pub encoded_current_bytes: u64,
    pub encoded_high_water_upper_bound_bytes: u64,
    pub decoding_current_bytes: u64,
    pub decoding_high_water_upper_bound_bytes: u64,
    pub ready_current_bytes: u64,
    pub ready_high_water_upper_bound_bytes: u64,
    pub permits_in_use: u64,
    pub target_clusters: u64,
    pub absent_clusters: u64,
    pub queued_clusters: u64,
    pub ready_clusters: u64,
    pub installed_uncomposed_clusters: u64,
    pub sampleable_clusters: u64,
    pub failed_clusters: u64,
    pub misses: u64,
    pub installs: u64,
    pub evictions: u64,
    pub retries: u64,
    pub warm_clusters: u64,
    pub cancelled_requests: u64,
    pub discarded_reads: u64,
    pub discarded_read_bytes: u64,
    pub decoded_bytes_installed: u64,
    pub last_drain_decoded_bytes: u64,
    pub max_drain_decoded_bytes: u64,
    pub budget_limited_drains: u64,
    pub reads_issued: u64,
    pub coalesced_reads: u64,
    pub read_bytes: u64,
    pub gap_bytes: u64,
    pub read_latency_p50_ms: f32,
    pub read_latency_p95_ms: f32,
    pub read_latency_max_ms: f32,
    pub decode_latency_max_ms: f32,
    pub install_cpu_total_micros: u64,
    pub install_cpu_max_drain_micros: u64,
    pub install_cpu_last_drain_micros: u64,
    pub pool_growth_events: u64,
    pub pool_growth_bytes: u64,
}

impl ShResidencyReport {
    /// Overlay a current streamed-pool snapshot on immutable install rows.
    ///
    /// A streaming pool may grow or retain a generation after the level's
    /// descriptor rows have been recorded. Rebuild the physical total from
    /// those static rows each time so a later snapshot replaces, rather than
    /// accumulates with, the install-time live values. Whole-resident scatter
    /// is already one of the static rows and must not be added from the
    /// streaming summary a second time.
    pub fn with_streaming_summary(mut self, summary: ShStreamingAllocationSummary) -> Self {
        self.streaming = Some(summary);
        self.total_bytes = allocation_total_bytes(&self.allocations)
            .checked_add(summary.fixed_metadata_bytes)
            .expect("SH residency physical total overflow adding fixed metadata")
            .checked_add(summary.active_capacity_bytes)
            .expect("SH residency physical total overflow adding active capacity")
            .checked_add(summary.retiring_capacity_bytes)
            .expect("SH residency physical total overflow adding retiring capacity");
        self
    }

    /// Overlay app-owned worker/policy state on the immutable install ledger
    /// and renderer-owned live capacity snapshot. This intentionally changes
    /// no physical total: host phases, counters, and logical overshoot are
    /// diagnostic ledgers rather than GPU allocations.
    pub fn with_streaming_lifecycle_summary(
        mut self,
        summary: ShStreamingLifecycleSummary,
    ) -> Self {
        self.streaming_lifecycle = Some(summary);
        self
    }
}

/// Mutable collector passed only through level-owned SH constructors.
pub(super) struct ShAllocationLedger {
    allocations: Vec<ShResidencyAllocation>,
    streaming: Option<ShStreamingAllocationSummary>,
}

impl ShAllocationLedger {
    pub(super) fn new() -> Self {
        Self {
            allocations: Vec::new(),
            streaming: None,
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

    /// Record the one live streaming summary after its renderer-owned pools
    /// exist. This must be called at most once for a level install; report
    /// reads overlay later per-frame snapshots rather than mutating this
    /// completed install ledger.
    pub(super) fn record_streaming_summary(&mut self, summary: ShStreamingAllocationSummary) {
        debug_assert!(self.streaming.is_none(), "streaming summary recorded twice");
        self.streaming = Some(summary);
    }

    fn record(&mut self, allocation: ShResidencyAllocation) {
        self.allocations.push(allocation);
    }

    pub(super) fn finish(self) -> ShResidencyReport {
        let allocation_total_bytes = allocation_total_bytes(&self.allocations);
        let report = ShResidencyReport {
            allocations: self.allocations,
            total_bytes: allocation_total_bytes,
            streaming: self.streaming,
            streaming_lifecycle: None,
        };
        match report.streaming {
            Some(summary) => report.with_streaming_summary(summary),
            None => report,
        }
    }
}

fn allocation_total_bytes(allocations: &[ShResidencyAllocation]) -> u64 {
    allocations
        .iter()
        .try_fold(0_u64, |total, row| total.checked_add(row.bytes))
        .expect("SH residency static allocation total overflow")
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

    fn test_allocation(bytes: u64) -> ShResidencyAllocation {
        ShResidencyAllocation {
            name: "test_allocation",
            sources: vec![ShResidencySource::Derived],
            bytes,
            state: ShResidencyAllocationState::Data,
            shape: ShResidencyAllocationShape::Buffer {
                binding_bytes: bytes,
            },
        }
    }

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

    #[test]
    fn live_streaming_summary_replaces_install_values_after_growth_and_retirement() {
        let mut ledger = ShAllocationLedger::new();
        ledger.record_buffer(
            BufferAllocation {
                kind: ShAllocationKind::BillboardComposeGrid,
                byte_len: 13,
                usage: wgpu::BufferUsages::UNIFORM,
            },
            &[47],
            false,
            ShResidencyAllocationState::Data,
        );
        ledger.record_streaming_summary(ShStreamingAllocationSummary {
            fixed_metadata_bytes: 11,
            whole_resident_scatter_bytes: 13,
            active_capacity_bytes: 17,
            logical_occupancy_bytes: 5,
            retiring_capacity_bytes: 0,
            replacement_peak_bytes: 17,
        });

        let installed = ledger.finish();
        assert_eq!(
            installed.total_bytes, 41,
            "static id47 bytes plus fixed and active streamed allocations"
        );
        let report = installed.with_streaming_summary(ShStreamingAllocationSummary {
            fixed_metadata_bytes: 11,
            whole_resident_scatter_bytes: 13,
            active_capacity_bytes: 23,
            logical_occupancy_bytes: 7,
            retiring_capacity_bytes: 19,
            replacement_peak_bytes: 42,
        });
        assert_eq!(
            report.total_bytes, 66,
            "refresh uses live fixed + active + retiring capacity and does not double-count id47"
        );
        assert_eq!(
            report.streaming,
            Some(ShStreamingAllocationSummary {
                fixed_metadata_bytes: 11,
                whole_resident_scatter_bytes: 13,
                active_capacity_bytes: 23,
                logical_occupancy_bytes: 7,
                retiring_capacity_bytes: 19,
                replacement_peak_bytes: 42,
            })
        );
    }

    #[test]
    fn non_evictable_logical_overshoot_is_not_temporary_replacement_capacity() {
        let report = ShResidencyReport {
            allocations: Vec::new(),
            total_bytes: 0,
            streaming: None,
            streaming_lifecycle: None,
        }
        .with_streaming_summary(ShStreamingAllocationSummary {
            fixed_metadata_bytes: 11,
            active_capacity_bytes: 17,
            retiring_capacity_bytes: 19,
            replacement_peak_bytes: 36,
            ..ShStreamingAllocationSummary::default()
        })
        .with_streaming_lifecycle_summary(ShStreamingLifecycleSummary {
            non_evictable_overshoot_bytes: 4,
            ..ShStreamingLifecycleSummary::default()
        });

        assert_eq!(report.total_bytes, 47);
        assert_eq!(
            report
                .streaming
                .expect("streaming pool summary")
                .replacement_peak_bytes,
            36,
            "replacement peak is active plus retiring pool capacity only"
        );
        assert_eq!(
            report
                .streaming_lifecycle
                .expect("controller lifecycle summary")
                .non_evictable_overshoot_bytes,
            4,
            "logical policy demand is reported separately from replacement backing"
        );
    }

    #[test]
    #[should_panic(expected = "SH residency static allocation total overflow")]
    fn static_allocation_total_rejects_overflow() {
        let _ = allocation_total_bytes(&[test_allocation(u64::MAX), test_allocation(1)]);
    }

    #[test]
    #[should_panic(expected = "SH residency physical total overflow adding fixed metadata")]
    fn live_streaming_total_rejects_overflow() {
        let report = ShResidencyReport {
            allocations: vec![test_allocation(u64::MAX)],
            total_bytes: u64::MAX,
            streaming: None,
            streaming_lifecycle: None,
        };
        let _ = report.with_streaming_summary(ShStreamingAllocationSummary {
            fixed_metadata_bytes: 1,
            ..Default::default()
        });
    }
}
