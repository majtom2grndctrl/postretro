// Lightmap-family PRL section decoding (ids 22, 42, 24, 25), the vertex
// cell-block check, and the all-resident id-22/42 GPU payload.
// See: context/lib/build_pipeline.md §PRL section IDs

use postretro_level_format::SectionId;
use postretro_level_format::animated_light_chunks::AnimatedLightChunksSection;
use postretro_level_format::animated_light_weight_maps::AnimatedLightWeightMapsSection;
use postretro_level_format::lightmap::{
    LightmapBlockIndex, LightmapBlockPayload, SectionByteRange,
};
use postretro_level_format::shadowmask_atlas::ShadowmaskBlockIndex;
use postretro_render_data::geometry::WorldVertex;

use crate::prl::{LightmapMode, PrlLoadError};
use crate::prl_container::PrlContainer;
use crate::prl_loader::{section_validation, section_validation_from_error};

/// Id 22 as loaded: the block index the level keeps, and each block's texels
/// in block-id order. The shadowmask groups join the payloads at the split.
pub(crate) struct LoadedLightmap {
    pub(crate) index: LightmapBlockIndex,
    pub(crate) blocks: Vec<LightmapBlockPayload>,
}

/// Id 42 as loaded: the slot table and block records the level keeps, and
/// each block's group A then group B planes in id-22 block order.
pub(crate) struct LoadedShadowmask {
    pub(crate) index: ShadowmaskBlockIndex,
    pub(crate) groups: Vec<[Vec<u8>; 2]>,
}

/// Absent id 22 is placeholder mode: no cell blocks, and every vertex must
/// carry block 0. Present, it parses strictly against the level's
/// `cell_count` cells; any fault fails the load.
pub(crate) fn read_lightmap(
    container: &PrlContainer,
    cell_count: usize,
) -> Result<Option<LoadedLightmap>, PrlLoadError> {
    let Some(data) = container.read_section(SectionId::Lightmap as u32)? else {
        log::warn!("[PRL] Lightmap section missing — static direct lighting disabled for this map");
        return Ok(None);
    };
    let index = LightmapBlockIndex::from_prefix(&data, data.len() as u64)
        .map_err(|err| section_validation_from_error("Lightmap", err))?;
    validate_lightmap_block_cells(&index, cell_count)?;
    let blocks: Vec<LightmapBlockPayload> = index
        .records
        .iter()
        .map(|record| LightmapBlockPayload {
            irradiance: blob(&data, record.irradiance),
            direction: blob(&data, record.direction),
            shadowmask: None,
        })
        .collect();
    log::info!(
        "[PRL] Lightmap: {} cell block(s), {} B irradiance, {} B direction",
        blocks.len(),
        blocks.iter().map(|b| b.irradiance.len()).sum::<usize>(),
        blocks.iter().map(|b| b.direction.len()).sum::<usize>(),
    );
    Ok(Some(LoadedLightmap { index, blocks }))
}

/// Reject a block whose cell lies past the Cells table, and a cell whose
/// blocks are not contiguous: a cell's charts pack into one or more blocks in
/// block-id order, and residency maps a cell to that contiguous run. A cell
/// may own no block. Both load modes run this.
pub(crate) fn validate_lightmap_block_cells(
    index: &LightmapBlockIndex,
    cell_count: usize,
) -> Result<(), PrlLoadError> {
    // First block of each cell seen so far.
    let mut first_block: Vec<Option<usize>> = vec![None; cell_count];
    let mut previous_cell = None;
    for (block, record) in index.records.iter().enumerate() {
        let cell = record.cell_id;
        let Some(slot) = first_block.get_mut(cell as usize) else {
            return Err(section_validation(
                "Lightmap",
                format!(
                    "block {block} names cell {cell} past the {cell_count}-cell Cells table; recompile with `prl-build`"
                ),
            ));
        };
        match *slot {
            None => *slot = Some(block),
            Some(first) if previous_cell != Some(cell) => {
                return Err(section_validation(
                    "Lightmap",
                    format!(
                        "block {block} names cell {cell}, whose blocks began at block {first} and were interrupted by block {}; a cell's blocks must be contiguous; recompile with `prl-build`",
                        block - 1
                    ),
                ));
            }
            Some(_) => {}
        }
        previous_cell = Some(cell);
    }
    Ok(())
}

/// The id-22 bake mode, `Shadowed` without id 22. The forward pass never
/// multiplies SDF visibility into the static term, so an `Unshadowed` level
/// is recorded, not honoured, and warns once per load.
pub(crate) fn lightmap_mode(lightmap: Option<&LoadedLightmap>) -> LightmapMode {
    let mode = lightmap.map_or(LightmapMode::default(), |lightmap| {
        LightmapMode::from(lightmap.index.header.mode)
    });
    if mode == LightmapMode::Unshadowed {
        log::warn!(
            "[PRL] lightmap mode Unshadowed is recorded but not honoured; static light renders without its shadow term"
        );
    }
    mode
}

/// Id 42 pairs with id 22 block for block, so every fault fails the load:
/// a malformed section, a block count that differs from id 22's, or a
/// shadowmask with no lightmap to pair with.
pub(crate) fn read_shadowmask_atlas(
    container: &PrlContainer,
    lightmap: Option<&LightmapBlockIndex>,
) -> Result<Option<LoadedShadowmask>, PrlLoadError> {
    let Some(data) = container.read_section(SectionId::ShadowmaskAtlas as u32)? else {
        return Ok(None);
    };
    let lightmap = lightmap.ok_or_else(|| {
        section_validation(
            "ShadowmaskAtlas",
            "present without a Lightmap section (id 22) to pair its blocks with; recompile with `prl-build`",
        )
    })?;
    let index = ShadowmaskBlockIndex::from_prefix(&data, data.len() as u64, lightmap)
        .map_err(|err| section_validation_from_error("ShadowmaskAtlas", err))?;
    let groups: Vec<[Vec<u8>; 2]> = index
        .records
        .iter()
        .map(|record| [blob(&data, record.group_a), blob(&data, record.group_b)])
        .collect();
    log::info!(
        "[PRL] ShadowmaskAtlas: {} cell block(s), {} selected channel entr(y/ies), {} payload byte(s)",
        groups.len(),
        index.channels.len(),
        groups.iter().map(|[a, b]| a.len() + b.len()).sum::<usize>(),
    );
    Ok(Some(LoadedShadowmask { index, groups }))
}

/// A range the index validated against this section's length.
fn blob(data: &[u8], range: SectionByteRange) -> Vec<u8> {
    let start = range.offset as usize;
    data[start..start + range.len as usize].to_vec()
}

/// Reject a vertex whose cell block (`id + 1`, 0 meaning none) lies past the
/// id-22 table. With id 22 absent the table is empty, so any nonzero id is
/// past it.
pub(crate) fn validate_vertex_lightmap_blocks(
    vertices: &[WorldVertex],
    lightmap: Option<&LightmapBlockIndex>,
) -> Result<(), PrlLoadError> {
    let block_count = lightmap.map_or(0, |index| index.records.len());
    match vertices
        .iter()
        .enumerate()
        .find(|(_, vertex)| usize::from(vertex.lightmap_block) > block_count)
    {
        Some((index, vertex)) => Err(section_validation(
            "Geometry",
            format!(
                "vertex {index} names lightmap block {} past the {block_count}-block Lightmap (id 22) table; recompile with `prl-build`",
                vertex.lightmap_block - 1,
            ),
        )),
        None => Ok(()),
    }
}

// Optional — cross-checked against weight-map chunk count at runtime.
pub(crate) fn read_animated_light_chunks(
    container: &PrlContainer,
) -> Result<Option<AnimatedLightChunksSection>, PrlLoadError> {
    Ok(
        match container.read_section(SectionId::AnimatedLightChunks as u32)? {
            Some(data) => {
                let section = AnimatedLightChunksSection::from_bytes(&data)?;
                log::info!(
                    "[PRL] AnimatedLightChunks: {} chunks, {} flat indices",
                    section.chunks.len(),
                    section.light_indices.len(),
                );
                Some(section)
            }
            None => None,
        },
    )
}

// Optional — absent → 1×1 zero atlas on animated-contribution slot.
pub(crate) fn read_animated_light_weight_maps(
    container: &PrlContainer,
    lightmap: Option<&LightmapBlockIndex>,
    vertices: &[WorldVertex],
) -> Result<Option<AnimatedLightWeightMapsSection>, PrlLoadError> {
    let animated_light_weight_maps: Option<AnimatedLightWeightMapsSection> =
        match container.read_section(SectionId::AnimatedLightWeightMaps as u32)? {
            Some(data) => {
                let section = AnimatedLightWeightMapsSection::from_bytes(&data)?;
                log::info!(
                    "[PRL] AnimatedLightWeightMaps: {} chunks, {} blocks on {} pages of {}², \
                 {} covered texels, {} weight entries",
                    section.chunk_rects.len(),
                    section.blocks.len(),
                    section.compact_layers,
                    section.page_size,
                    section.offset_counts.len(),
                    section.texel_lights.len(),
                );
                Some(section)
            }
            None => None,
        };
    // Both sections it depends on have decoded: check each animated block
    // against its cell block and cross-check every vertex's block id.
    crate::prl_animated_atlas::check_animated_atlas(animated_light_weight_maps, lightmap, vertices)
}

/// Drop id 42 when its channel table does not match the usable
/// EntityShadowLights selection.
pub(crate) fn reconcile_shadowmask_with_shadow_lights(
    shadowmask_atlas: &mut Option<LoadedShadowmask>,
    entity_shadow_lights: &[u32],
) {
    if let Some(section) = shadowmask_atlas.as_ref() {
        if entity_shadow_lights.is_empty() {
            log::warn!(
                "[PRL] ShadowmaskAtlas present without usable EntityShadowLights; ignoring section"
            );
            *shadowmask_atlas = None;
        } else if section.index.channels.len() != entity_shadow_lights.len() {
            log::warn!(
                "[PRL] ShadowmaskAtlas channel table has {} entr(y/ies), but EntityShadowLights has {}; ignoring section",
                section.index.channels.len(),
                entity_shadow_lights.len(),
            );
            *shadowmask_atlas = None;
        }
    }
}

/// The all-resident id-22/42 texels, which only the GPU upload reads: one
/// payload per lightmap cell block, in block-id order, each carrying its
/// shadowmask groups when the level keeps id 42. Empty in placeholder mode
/// (id 22 absent or holding no blocks).
///
/// A loaded level holds them until install moves them into the upload, which
/// drops them once the pool holds the texels; the level keeps only the block
/// indices. An install that uploads nothing leaves them here.
#[derive(Debug, Default, PartialEq)]
pub struct GpuLightingPayloads {
    pub blocks: Vec<LightmapBlockPayload>,
}

/// The block indices a loaded level keeps for ids 22 and 42, and their
/// payloads with each block's shadowmask groups moved in beside its texels.
pub(crate) fn split_gpu_lighting(
    lightmap: Option<LoadedLightmap>,
    shadowmask: Option<LoadedShadowmask>,
) -> (
    Option<LightmapBlockIndex>,
    Option<ShadowmaskBlockIndex>,
    GpuLightingPayloads,
) {
    let Some(LoadedLightmap { index, mut blocks }) = lightmap else {
        debug_assert!(
            shadowmask.is_none(),
            "id 42 without id 22 fails the load before the split"
        );
        return (None, None, GpuLightingPayloads::default());
    };
    let shadowmask_index = shadowmask.map(|LoadedShadowmask { index, groups }| {
        debug_assert_eq!(groups.len(), blocks.len(), "id 42 validated against id 22");
        for (block, groups) in blocks.iter_mut().zip(groups) {
            block.shadowmask = Some(groups);
        }
        index
    });
    (
        Some(index),
        shadowmask_index,
        GpuLightingPayloads { blocks },
    )
}

#[cfg(test)]
#[path = "prl_lightmap_tests.rs"]
mod tests;
