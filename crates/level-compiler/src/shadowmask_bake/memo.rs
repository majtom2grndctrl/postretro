// Whole-section shadowmask memo: input hash, cache read/write, and stale-entry validation.
// See: context/lib/build_pipeline.md §Build Cache

use postretro_level_format::entity_shadow_lights::EntityShadowLightsSection;
use postretro_level_format::lightmap::{
    LightmapBlockIndex, LightmapBlockRecord, LightmapHeader, LightmapMode, SectionByteRange,
};
use postretro_level_format::shadowmask_atlas::ShadowmaskAtlasSection;

#[cfg(test)]
use super::{
    RAW_FILL_LIVE_BYTES_AT_CACHE_WRITE, SHADOWMASK_STREAMED_CACHE_WRITE_COUNT, raw_fill_live_bytes,
};
use crate::bake_control::BakeControl;
use crate::cache::{CacheKey, StageCache};
use crate::lightmap_layer::{self, SharedAtlas};

/// Whole-section input hash for the `"shadowmask_atlas"` memo entry.
///
/// The byte layout is fixed and order-sensitive:
/// `LAYER_FORMAT_VERSION`, selected-light count, selected `AlphaLights` indices,
/// then every selected `(light, atlas layer)`
/// `lightmap_layer::layer_input_hash` in selected-light-major, ascending-layer
/// order, followed by atlas width/height/layer-count. The caller supplies that
/// exact order; the helper does no sorting.
pub fn shadowmask_atlas_input_hash(
    selection: &EntityShadowLightsSection,
    layer_input_hashes: &[[u8; 32]],
    atlas_width: u32,
    atlas_height: u32,
    layer_count: u32,
) -> [u8; 32] {
    debug_assert_eq!(
        selection
            .light_indices
            .len()
            .saturating_mul(layer_count as usize),
        layer_input_hashes.len(),
        "shadowmask selected light/layer hash slices must align"
    );

    let mut hasher = blake3::Hasher::new();
    hasher.update(&lightmap_layer::LAYER_FORMAT_VERSION.to_le_bytes());
    hasher.update(&(selection.light_indices.len() as u32).to_le_bytes());
    for &alpha_index in &selection.light_indices {
        hasher.update(&alpha_index.to_le_bytes());
    }
    for hash in layer_input_hashes {
        hasher.update(hash);
    }
    hasher.update(&atlas_width.to_le_bytes());
    hasher.update(&atlas_height.to_le_bytes());
    hasher.update(&layer_count.to_le_bytes());
    *hasher.finalize().as_bytes()
}

/// Whole-section memo entry: the miss's peak per-texel overlap (`u32` LE),
/// then the section's wire bytes. The count shares the entry so it can never
/// be evicted apart from the section it describes.
pub(super) const MEMO_OVERLAP_PREFIX_BYTES: usize = 4;

pub(super) struct ShadowmaskMemo {
    pub(super) section: ShadowmaskAtlasSection,
    pub(super) peak_texel_overlap: u32,
}

/// A memo entry usable for this selection and atlas, or `None` (logged) when
/// it is absent, truncated, undecodable or describes another atlas.
pub(super) fn read_shadowmask_memo(
    cache: &StageCache,
    key: &CacheKey,
    selection: &EntityShadowLightsSection,
    shared: &SharedAtlas<'_>,
) -> Option<ShadowmaskMemo> {
    let bytes = cache.get(key)?;
    let Some((prefix, section_bytes)) = bytes.split_first_chunk::<MEMO_OVERLAP_PREFIX_BYTES>()
    else {
        log::warn!(
            "[Compiler] corrupt shadowmask atlas, re-baking: memo entry has no overlap count"
        );
        return None;
    };
    let section = match ShadowmaskAtlasSection::from_bytes(section_bytes, &extent_index(shared)) {
        Ok(section) => section,
        Err(err) => {
            log::warn!("[Compiler] corrupt shadowmask atlas, re-baking: {err}");
            return None;
        }
    };
    if let Err(reason) = validate_cached_shadowmask_section(&section, selection) {
        log::warn!(
            "[Compiler] shadowmask_atlas cache entry does not match current atlas ({reason}), re-baking"
        );
        return None;
    }
    Some(ShadowmaskMemo {
        section,
        peak_texel_overlap: u32::from_le_bytes(*prefix),
    })
}

pub(super) fn cache_shadowmask_section_then_complete(
    cache: &StageCache,
    section_key: &CacheKey,
    section: &ShadowmaskAtlasSection,
    peak_texel_overlap: u32,
    control: &BakeControl,
    has_valid_selection: bool,
    after_cache_write: impl FnOnce(),
) {
    let section_index = section.index_bytes();

    #[cfg(test)]
    {
        SHADOWMASK_STREAMED_CACHE_WRITE_COUNT.with(|count| count.set(count.get() + 1));
        RAW_FILL_LIVE_BYTES_AT_CACHE_WRITE
            .with(|at_write| at_write.set(Some(raw_fill_live_bytes())));
    }
    let entry_len = MEMO_OVERLAP_PREFIX_BYTES + section.byte_len();
    // The index, then each block's groups: the section's own byte order, with
    // no second contiguous copy of the payload.
    cache.put_streamed(section_key, entry_len as u64, |writer| {
        writer.write_all(&peak_texel_overlap.to_le_bytes())?;
        writer.write_all(&section_index)?;
        for [group_a, group_b] in &section.blocks {
            writer.write_all(group_a)?;
            writer.write_all(group_b)?;
        }
        Ok(())
    });
    after_cache_write();
    if has_valid_selection {
        // Keep the final fill unit pending through whole-section memo storage.
        control.advance(1);
    }
}

pub(super) fn invalid_selected_light_hash(alpha_index: u32, target_layer: u32) -> [u8; 32] {
    let mut hasher = blake3::Hasher::new();
    hasher.update(b"shadowmask_atlas_invalid_selected_alpha_light");
    hasher.update(&alpha_index.to_le_bytes());
    hasher.update(&target_layer.to_le_bytes());
    *hasher.finalize().as_bytes()
}

/// The id-22 index the memo's id-42 parse checks group lengths against. The
/// shadowmask parse reads only block count and extents, and the lightmap
/// section is not built yet when the memo resolves, so the blob ranges are
/// empty.
pub(super) fn extent_index(shared: &SharedAtlas<'_>) -> LightmapBlockIndex {
    let empty = SectionByteRange { offset: 0, len: 0 };
    LightmapBlockIndex {
        header: LightmapHeader {
            block_count: shared.layout.blocks.len() as u32,
            direction_texel_scale: shared.layout.direction_texel_scale,
            irradiance_format: 0,
            mode: LightmapMode::Shadowed,
        },
        records: shared
            .layout
            .blocks
            .iter()
            .map(|block| LightmapBlockRecord {
                cell_id: block.cell_id,
                width: block.width as u16,
                height: block.height as u16,
                irradiance: empty,
                direction: empty,
            })
            .collect(),
    }
}

/// Block count and group extents are already checked by the parse against
/// [`extent_index`]; what remains is the selection the section was built for.
fn validate_cached_shadowmask_section(
    section: &ShadowmaskAtlasSection,
    selection: &EntityShadowLightsSection,
) -> Result<(), String> {
    if section.channels.len() != selection.light_indices.len() {
        return Err(format!(
            "channel count {} != selected light count {}",
            section.channels.len(),
            selection.light_indices.len()
        ));
    }
    Ok(())
}
