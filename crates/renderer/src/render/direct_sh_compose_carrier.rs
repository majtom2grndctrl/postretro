// CPU-side carrier construction for the direct-SH promotion compose pass.
// See: context/lib/rendering_pipeline.md §4

use postretro_level_format::direct_sh_delta_volumes::DirectShDeltaVolumesSection;
#[cfg(feature = "dev-tools")]
use postretro_render_cpu::sh_compose::ComposeStorageFootprint;
use postretro_render_cpu::sh_compose::{DirectDeltaComposeBuffers, build_direct_delta_buffers};

use super::sh_allocation::{ComposeStoragePayloads, ShAllocationKind, compose_storage_payloads};

#[cfg(feature = "dev-tools")]
const DIRECT_PROMOTION_FOOTPRINT_LABEL: &str = "DIRECT SH compose id-41 promotion @group(0)";

/// Format-owned sparse data becomes one renderer carrier before GPU buffers are
/// created. The pass builder consumes this as a single ownership transfer, so
/// future streamed payloads can replace its source without perturbing bindings.
pub(super) struct DirectPromotionStorage {
    pub(super) buffers: DirectDeltaComposeBuffers,
    pub(super) payloads: ComposeStoragePayloads,
}

impl DirectPromotionStorage {
    pub(super) fn new(
        delta: Option<&DirectShDeltaVolumesSection>,
        grid_dimensions: [u32; 3],
    ) -> Self {
        let delta_subblocks: &[u16] = delta.map_or(&[], |delta| delta.delta_subblocks.as_slice());
        let buffers = build_direct_delta_buffers(delta, grid_dimensions);
        let payloads = compose_storage_payloads(
            ShAllocationKind::DirectComposeDeltaSubblocks,
            ShAllocationKind::DirectComposeCompactionMetadata,
            ShAllocationKind::DirectComposeAffinityOffsets,
            ShAllocationKind::DirectComposeAffinityLights,
            None,
            delta_subblocks,
            &buffers.compaction_meta_words(),
            &buffers.affinity_offsets,
            &buffers.affinity_lights,
            None,
        );
        Self { buffers, payloads }
    }

    #[cfg(feature = "dev-tools")]
    fn footprint(&self) -> ComposeStorageFootprint {
        ComposeStorageFootprint {
            delta_subblocks_bytes: self.payloads.delta_subblocks.allocation.byte_len,
            delta_compaction_meta_bytes: self.payloads.compaction_metadata.allocation.byte_len,
            affinity_offsets_bytes: self.payloads.affinity_offsets.allocation.byte_len,
            affinity_lights_bytes: self.payloads.affinity_lights.allocation.byte_len,
            // The id-41 promotion pass has no descriptor-index binding. Case
            // 2's id-45 animated-add storage belongs to its sibling pass.
            animation_descriptor_indices_bytes: 0,
        }
    }

    #[cfg(feature = "dev-tools")]
    pub(super) fn log_footprint(&self) {
        self.footprint().log(DIRECT_PROMOTION_FOOTPRINT_LABEL);
    }
}
