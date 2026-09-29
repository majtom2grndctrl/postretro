// Whole-section shadowmask memo: input hash, cache read/write, and stale-entry validation.
// See: context/lib/build_pipeline.md §Build Cache

use postretro_level_format::entity_shadow_lights::EntityShadowLightsSection;
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
    layer_count: u32,
) -> Option<ShadowmaskMemo> {
    let bytes = cache.get(key)?;
    let Some((prefix, section_bytes)) = bytes.split_first_chunk::<MEMO_OVERLAP_PREFIX_BYTES>()
    else {
        log::warn!(
            "[Compiler] corrupt shadowmask atlas, re-baking: memo entry has no overlap count"
        );
        return None;
    };
    let section = match ShadowmaskAtlasSection::from_bytes(section_bytes) {
        Ok(section) => section,
        Err(err) => {
            log::warn!("[Compiler] corrupt shadowmask atlas, re-baking: {err}");
            return None;
        }
    };
    if let Err(reason) =
        validate_cached_shadowmask_section(&section, selection, shared, layer_count)
    {
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
    let section_header = section.header_bytes();

    #[cfg(test)]
    {
        SHADOWMASK_STREAMED_CACHE_WRITE_COUNT.with(|count| count.set(count.get() + 1));
        RAW_FILL_LIVE_BYTES_AT_CACHE_WRITE
            .with(|at_write| at_write.set(Some(raw_fill_live_bytes())));
    }
    let entry_len = MEMO_OVERLAP_PREFIX_BYTES + section.byte_len();
    cache.put_streamed(section_key, entry_len as u64, |writer| {
        writer.write_all(&peak_texel_overlap.to_le_bytes())?;
        writer.write_all(&section_header)?;
        writer.write_all(&section.data)
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

fn validate_cached_shadowmask_section(
    section: &ShadowmaskAtlasSection,
    selection: &EntityShadowLightsSection,
    shared: &SharedAtlas<'_>,
    layer_count: u32,
) -> Result<(), String> {
    if section.width != shared.atlas_width || section.height != shared.atlas_height {
        return Err(format!(
            "dimensions {}x{} != {}x{}",
            section.width, section.height, shared.atlas_width, shared.atlas_height
        ));
    }
    if section.layer_count != layer_count {
        return Err(format!(
            "layer_count {} != {}",
            section.layer_count, layer_count
        ));
    }
    if section.channels.len() != selection.light_indices.len() {
        return Err(format!(
            "channel count {} != selected light count {}",
            section.channels.len(),
            selection.light_indices.len()
        ));
    }
    Ok(())
}
