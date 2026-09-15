// Shadowmask channel fill over already-assigned selected lights.
// See: context/lib/build_pipeline.md §PRL section IDs

use postretro_level_format::shadowmask_atlas::{
    SHADOWMASK_CHANNEL_DROPPED, ShadowmaskAtlasSection,
};

use crate::bake_control::BakeControl;
use crate::lightmap_layer::{LayerTexel, LightmapLayer};
use crate::map_data::MapLight;

use super::assignment::*;
use super::{ResidentLayerTracker, SHADOWMASK_FILL_CHECKPOINT_TEXELS, allocate_shadowmask_output};

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
    data: Vec<u8>,
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
            data: allocate_shadowmask_output(data_len),
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

    pub(super) fn finish(self) -> ShadowmaskAtlasSection {
        ShadowmaskAtlasSection {
            width: self.width,
            height: self.height,
            layer_count: self.layer_count,
            channels: self.channels,
            data: self.data,
        }
    }
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
