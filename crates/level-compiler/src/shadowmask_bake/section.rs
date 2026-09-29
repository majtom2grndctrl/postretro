// Shadowmask section assembly: atlas layer count, empty sections, and sections from preloaded layers.
// See: context/lib/build_pipeline.md §PRL section IDs

use postretro_level_format::entity_shadow_lights::EntityShadowLightsSection;
use postretro_level_format::shadowmask_atlas::{
    SHADOWMASK_CHANNEL_DROPPED, SHADOWMASK_FORMAT_BC5_RG_SIDE_BY_SIDE, ShadowmaskAtlasSection,
};

#[cfg(test)]
use super::SHADOWMASK_OUTPUT_ALLOCATION_COUNT;
use super::assignment::OverlapGraph;
use super::encode;
#[cfg(test)]
use super::fill::RawShadowmaskFill;
use super::fill::{ShadowmaskFill, raw_visibility_is_covered, texel_plane_len};
use crate::lightmap_layer::{LightmapLayer, SharedAtlas};
use crate::map_data::MapLight;

pub(super) fn layer_count_from_shared(shared: &SharedAtlas<'_>) -> u32 {
    shared
        .placements
        .iter()
        .map(|placement| placement.layer + 1)
        .max()
        .unwrap_or(1)
}

pub(super) fn empty_section_for_selection(
    shared: &SharedAtlas<'_>,
    selected_light_count: usize,
) -> ShadowmaskAtlasSection {
    let layer_count = layer_count_from_shared(shared);
    empty_section_for_dimensions(
        shared.atlas_width,
        shared.atlas_height,
        layer_count,
        selected_light_count,
    )
}

fn empty_section_for_dimensions(
    width: u32,
    height: u32,
    layer_count: u32,
    selected_light_count: usize,
) -> ShadowmaskAtlasSection {
    let data = encode::all_visible_bc5_payload(width, height, layer_count);
    #[cfg(test)]
    SHADOWMASK_OUTPUT_ALLOCATION_COUNT.with(|count| count.set(count.get() + 1));
    ShadowmaskAtlasSection {
        format: SHADOWMASK_FORMAT_BC5_RG_SIDE_BY_SIDE,
        width,
        height,
        layer_count,
        channels: vec![SHADOWMASK_CHANNEL_DROPPED; selected_light_count],
        data,
    }
}

/// Build the shadowmask section from preloaded per-light layers.
///
/// `selected` and `layers` must be in the same order as
/// `selection.light_indices`, after dropping any out-of-range selected
/// `AlphaLights` entries the same way the uncached path does. Each `selected`
/// entry carries its original selection index so invalid earlier selections do
/// not shift the channel table. Every preloaded texel must fit within the
/// supplied atlas layer and plane dimensions.
pub fn bake_shadowmask_atlas_from_layers(
    selection: &EntityShadowLightsSection,
    atlas_width: u32,
    atlas_height: u32,
    layer_count: u32,
    selected: &[(usize, u32, &MapLight)],
    layers: &[LightmapLayer],
) -> Option<ShadowmaskAtlasSection> {
    if selection.light_indices.is_empty() {
        return None;
    }

    assert_eq!(
        selected.len(),
        layers.len(),
        "shadowmask selected light/layer slices must align"
    );

    validate_preloaded_layer_texels(atlas_width, atlas_height, layer_count, layers);

    if selected.is_empty() {
        return Some(empty_section_for_dimensions(
            atlas_width,
            atlas_height,
            layer_count,
            selection.light_indices.len(),
        ));
    }

    Some(build_shadowmask_from_layers(
        atlas_width,
        atlas_height,
        layer_count as usize,
        selection.light_indices.len(),
        selected,
        layers,
    ))
}

fn validate_preloaded_layer_texels(
    atlas_width: u32,
    atlas_height: u32,
    layer_count: u32,
    layers: &[LightmapLayer],
) {
    let plane = texel_plane_len(atlas_width, atlas_height);
    for layer in layers {
        assert!(
            layer.target_layer < layer_count,
            "preloaded shadowmask partition layer {} exceeds atlas layer count {layer_count}",
            layer.target_layer
        );
        for texel in &layer.texels {
            assert!(
                (texel.idx as usize) < plane,
                "preloaded shadowmask texel index {} exceeds atlas plane length {plane}",
                texel.idx
            );
        }
    }
}

fn build_shadowmask_from_layers(
    width: u32,
    height: u32,
    layer_count: usize,
    selected_light_count: usize,
    selected: &[(usize, u32, &MapLight)],
    layers: &[LightmapLayer],
) -> ShadowmaskAtlasSection {
    fill_shadowmask_from_layers(
        width,
        height,
        layer_count,
        selected_light_count,
        selected,
        layers,
    )
    .finish()
}

#[cfg(test)]
pub(super) fn build_raw_shadowmask_from_layers(
    width: u32,
    height: u32,
    layer_count: usize,
    selected_light_count: usize,
    selected: &[(usize, u32, &MapLight)],
    layers: &[LightmapLayer],
) -> RawShadowmaskFill {
    fill_shadowmask_from_layers(
        width,
        height,
        layer_count,
        selected_light_count,
        selected,
        layers,
    )
    .finish_raw()
}

fn fill_shadowmask_from_layers(
    width: u32,
    height: u32,
    layer_count: usize,
    selected_light_count: usize,
    selected: &[(usize, u32, &MapLight)],
    layers: &[LightmapLayer],
) -> ShadowmaskFill<'static> {
    debug_assert_eq!(selected.len(), layers.len());
    let graph = overlap_graph_from_layers(layers);
    let mut fill = ShadowmaskFill::new(
        width,
        height,
        layer_count as u32,
        selected_light_count,
        selected,
        &graph,
        None,
        None,
    );
    for (compact_light_index, layer) in layers.iter().enumerate() {
        fill.write_partition(compact_light_index, layer);
    }
    fill
}

pub(super) fn overlap_graph_from_layers(layers: &[LightmapLayer]) -> OverlapGraph {
    let graph = OverlapGraph::new(layers.len());
    for a in 0..layers.len() {
        for b in a + 1..layers.len() {
            if layers_overlap(&layers[a], &layers[b]) {
                graph.mark_overlap(a, b);
            }
        }
    }
    graph
}

pub(super) fn layers_overlap(a: &LightmapLayer, b: &LightmapLayer) -> bool {
    if a.target_layer != b.target_layer {
        return false;
    }
    a.texels.iter().any(|a_texel| {
        raw_visibility_is_covered(a_texel.raw_visibility)
            && b.texels.iter().any(|b_texel| {
                raw_visibility_is_covered(b_texel.raw_visibility) && a_texel.idx == b_texel.idx
            })
    })
}
