// Shadowmask channel fill over already-assigned selected lights.
// See: context/lib/build_pipeline.md §PRL section IDs

use postretro_level_format::shadowmask_atlas::{
    SHADOWMASK_CHANNEL_DROPPED, SHADOWMASK_FORMAT_BC5_RG_SIDE_BY_SIDE, ShadowmaskAtlasSection,
};

use crate::bake_control::BakeControl;
use crate::lightmap_layer::{LayerTexel, LightmapLayer};
use crate::map_data::MapLight;

use super::assignment::*;
use super::encode::encode_side_by_side_bc5;
use super::{
    ResidentLayerTracker, SHADOWMASK_FILL_CHECKPOINT_TEXELS, allocate_shadowmask_raw_fill,
};

pub(super) fn raw_visibility_is_covered(raw_visibility: f32) -> bool {
    raw_visibility.partial_cmp(&0.0) != Some(std::cmp::Ordering::Less)
}

pub(super) struct ShadowmaskFill<'a> {
    width: u32,
    height: u32,
    layer_count: u32,
    plane: usize,
    compact_channels: Vec<u8>,
    channels: Vec<u8>,
    data: RawFillBuffer,
    control: Option<&'a BakeControl>,
    resident_layers: Option<&'a ResidentLayerTracker>,
}

impl<'a> ShadowmaskFill<'a> {
    #[allow(clippy::too_many_arguments)]
    pub(super) fn new(
        width: u32,
        height: u32,
        layer_count: u32,
        selected_light_count: usize,
        selected: &[(usize, u32, &MapLight)],
        graph: &OverlapGraph,
        control: Option<&'a BakeControl>,
        resident_layers: Option<&'a ResidentLayerTracker>,
    ) -> Self {
        Self::new_with_assignment_checkpoint(
            width,
            height,
            layer_count,
            selected_light_count,
            selected,
            graph,
            control,
            resident_layers,
            || {
                if let Some(control) = control {
                    control.governor().checkpoint();
                }
            },
        )
    }

    #[allow(clippy::too_many_arguments)]
    pub(super) fn new_with_assignment_checkpoint(
        width: u32,
        height: u32,
        layer_count: u32,
        selected_light_count: usize,
        selected: &[(usize, u32, &MapLight)],
        graph: &OverlapGraph,
        control: Option<&'a BakeControl>,
        resident_layers: Option<&'a ResidentLayerTracker>,
        assignment_checkpoint: impl FnOnce(),
    ) -> Self {
        debug_assert_eq!(selected.len(), graph.light_count());
        assignment_checkpoint();
        let assignment = assign_channels_with_drops_controlled(
            graph,
            selected,
            SHADOWMASK_COLOR_SEARCH_NODE_BUDGET,
            || {
                if let Some(control) = control {
                    control.governor().checkpoint();
                }
            },
        );
        let compact_channels = assignment.channels;
        if let Some(control) = control.filter(|_| !selected.is_empty()) {
            control.advance(1);
        }
        let mut channels = vec![SHADOWMASK_CHANNEL_DROPPED; selected_light_count];
        for (compact_index, &(selection_index, _, _)) in selected.iter().enumerate() {
            if selection_index < channels.len() {
                channels[selection_index] = compact_channels[compact_index];
            }
        }
        let plane = texel_plane_len(width, height);
        let data_len = plane
            .checked_mul(layer_count as usize)
            .and_then(|texels| texels.checked_mul(4))
            .expect("shadowmask atlas byte count exceeds addressable memory");

        Self {
            width,
            height,
            layer_count,
            plane,
            compact_channels,
            channels,
            data: allocate_shadowmask_raw_fill(data_len),
            control,
            resident_layers,
        }
    }

    pub(super) fn write_partition(
        &mut self,
        compact_light_index: usize,
        partition: &LightmapLayer,
    ) {
        let _resident_partition = self.resident_layers.map(ResidentLayerTracker::acquire);
        self.write_texels(
            compact_light_index,
            partition.target_layer,
            &partition.texels,
        );
    }

    pub(super) fn write_texels(
        &mut self,
        compact_light_index: usize,
        target_layer: u32,
        texels: &[LayerTexel],
    ) {
        let channel = self.compact_channels[compact_light_index];
        if channel == SHADOWMASK_CHANNEL_DROPPED {
            return;
        }
        for chunk in texels.chunks(SHADOWMASK_FILL_CHECKPOINT_TEXELS) {
            if let Some(control) = self.control {
                control.governor().checkpoint();
            }
            for texel in chunk {
                let global_texel_index = (target_layer as usize)
                    .checked_mul(self.plane)
                    .and_then(|offset| offset.checked_add(texel.idx as usize))
                    .expect("shadowmask global texel index exceeds addressable memory");
                let offset = shadowmask_data_offset(global_texel_index, channel)
                    .expect("shadowmask texel byte offset exceeds addressable memory");
                let visibility = (texel.raw_visibility.clamp(0.0, 1.0) * 255.0 + 0.5) as u8;
                self.data[offset] = visibility;
            }
        }
    }

    /// Encodes the raw fill and drops it before the section exists, so no
    /// cached or returned section coexists with the raw buffer.
    pub(super) fn finish(self) -> ShadowmaskAtlasSection {
        let Self {
            width,
            height,
            layer_count,
            channels,
            data: raw,
            ..
        } = self;
        // Test-only full copy of the raw fill for exact-value assertions. It
        // is not a `RawFillBuffer`, so the residency counters never see it.
        #[cfg(test)]
        super::record_raw_fill(&raw);
        let data = encode_side_by_side_bc5(&raw, width, height, layer_count);
        drop(raw);
        ShadowmaskAtlasSection {
            format: SHADOWMASK_FORMAT_BC5_RG_SIDE_BY_SIDE,
            width,
            height,
            layer_count,
            channels,
            data,
        }
    }

    #[cfg(test)]
    pub(super) fn finish_raw(self) -> RawShadowmaskFill {
        RawShadowmaskFill {
            width: self.width,
            height: self.height,
            layer_count: self.layer_count,
            channels: self.channels,
            data: self.data.into_vec(),
        }
    }
}

/// The raw fill buffer. Test builds count its live bytes when it is dropped,
/// so the compile-time bound observes the release itself.
pub(super) struct RawFillBuffer {
    bytes: Vec<u8>,
}

impl RawFillBuffer {
    pub(super) fn new(bytes: Vec<u8>) -> Self {
        #[cfg(test)]
        super::raw_fill_live_bytes_add(bytes.len());
        Self { bytes }
    }

    /// Hands the bytes out of the tracked buffer, so they stop counting as
    /// live raw fill here; `Drop` then sees an empty buffer.
    #[cfg(test)]
    fn into_vec(mut self) -> Vec<u8> {
        super::raw_fill_live_bytes_sub(self.bytes.len());
        std::mem::take(&mut self.bytes)
    }
}

impl std::ops::Deref for RawFillBuffer {
    type Target = [u8];

    fn deref(&self) -> &[u8] {
        &self.bytes
    }
}

impl std::ops::DerefMut for RawFillBuffer {
    fn deref_mut(&mut self) -> &mut [u8] {
        &mut self.bytes
    }
}

#[cfg(test)]
impl Drop for RawFillBuffer {
    fn drop(&mut self) {
        super::raw_fill_live_bytes_sub(self.bytes.len());
    }
}

/// The completed fill before encoding: layer-major `Rgba8Unorm`, one mask
/// slot per channel, 255 fully visible.
#[cfg(test)]
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct RawShadowmaskFill {
    pub(super) width: u32,
    pub(super) height: u32,
    pub(super) layer_count: u32,
    pub(super) channels: Vec<u8>,
    pub(super) data: Vec<u8>,
}

pub(super) fn texel_plane_len(width: u32, height: u32) -> usize {
    (width as usize)
        .checked_mul(height as usize)
        .expect("shadowmask atlas plane exceeds addressable memory")
}

pub(super) fn shadowmask_data_offset(global_texel_index: usize, channel: u8) -> Option<usize> {
    global_texel_index
        .checked_mul(4)
        .and_then(|base| base.checked_add(channel as usize))
}
