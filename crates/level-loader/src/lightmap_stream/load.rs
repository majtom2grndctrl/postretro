// Load-time reads of ids 22/42 for the chosen residency: whole sections, or index prefixes only.
// See: context/lib/rendering_pipeline.md §4 (Lightmap cell-block residency)

use std::sync::Arc;

use postretro_level_format::lightmap::{LIGHTMAP_HEADER_BYTES, LightmapBlockIndex, LightmapHeader};
use postretro_level_format::shadowmask_atlas::{
    SHADOWMASK_BLOCK_RECORD_BYTES, ShadowmaskBlockIndex, shadowmask_prefix_len_through_block_count,
};
use postretro_level_format::{ContainerMeta, SectionEntry, SectionId};

use super::boundary::LightmapStreamingMode;
use super::lightmap_stream_error;
use super::manifest::LightmapStreamManifest;
use super::storage::{
    LightmapResidencyInputs, LightmapStorage, log_lightmap_residency, select_lightmap_residency,
};
use crate::prl::PrlLoadError;
use crate::prl_container::PrlContainer;
use crate::prl_file::PrlFile;
use crate::prl_lightmap::{LoadedLightmap, LoadedShadowmask};
use crate::prl_loader::{section_validation, section_validation_from_error};
use crate::sh_stream::{ShStreamManifest, read_vec_at, validate_positional_entry_bounds};

/// The mode chosen while reading id 22, carried to id 42 and to the
/// storage built once the load has settled which sections it keeps.
pub(crate) struct LightmapResidencyRead {
    mode: LightmapStreamingMode,
    /// Raw index prefixes read in streaming mode, digested into the content
    /// tag when SH has none to share.
    lightmap_prefix: Vec<u8>,
    shadowmask_prefix: Option<Vec<u8>>,
}

/// Read id 22 for the residency `inputs` allow. When nothing but the block
/// count stands between the level and streaming, only the index prefix is
/// read positionally and the payloads stay on disk; otherwise the section is
/// read whole, as the all-resident path always has.
pub(crate) fn read_lightmap_for_residency(
    container: &PrlContainer,
    inputs: LightmapResidencyInputs,
) -> Result<(Option<LoadedLightmap>, LightmapResidencyRead), PrlLoadError> {
    let file = container
        .retained_file()
        .filter(|_| inputs.blocker().is_none());
    let Some(file) = file else {
        let lightmap = crate::prl_lightmap::read_lightmap(container)?;
        let block_count = lightmap.as_ref().map_or(0, |l| l.index.header.block_count);
        let (mode, reason) = select_lightmap_residency(inputs, block_count);
        log_lightmap_residency(mode, reason, block_count);
        return Ok((lightmap, residency_read(mode, Vec::new())));
    };
    let Some((index, prefix)) = read_lightmap_index_prefix(file, container.metadata())? else {
        log::warn!("[PRL] Lightmap section missing — static direct lighting disabled for this map");
        let (mode, reason) = select_lightmap_residency(inputs, 0);
        log_lightmap_residency(mode, reason, 0);
        return Ok((None, residency_read(mode, Vec::new())));
    };
    let block_count = index.header.block_count;
    let (mode, reason) = select_lightmap_residency(inputs, block_count);
    log_lightmap_residency(mode, reason, block_count);
    log::info!(
        "[PRL] Lightmap: {block_count} cell block(s), index only ({} B read)",
        prefix.len()
    );
    // Zero blocks leaves nothing past the index, so the prefix is the whole
    // all-resident read too.
    let loaded = LoadedLightmap {
        index,
        blocks: Vec::new(),
    };
    Ok((Some(loaded), residency_read(mode, prefix)))
}

fn residency_read(mode: LightmapStreamingMode, lightmap_prefix: Vec<u8>) -> LightmapResidencyRead {
    LightmapResidencyRead {
        mode,
        lightmap_prefix,
        shadowmask_prefix: None,
    }
}

impl LightmapResidencyRead {
    /// Read id 42 to match: its index prefix only when the level streams.
    pub(crate) fn read_shadowmask(
        &mut self,
        container: &PrlContainer,
        lightmap: Option<&LightmapBlockIndex>,
    ) -> Result<Option<LoadedShadowmask>, PrlLoadError> {
        let (LightmapStreamingMode::Stream, Some(file), Some(lightmap)) =
            (self.mode, container.retained_file(), lightmap)
        else {
            return crate::prl_lightmap::read_shadowmask_atlas(container, lightmap);
        };
        let Some((index, prefix)) =
            read_shadowmask_index_prefix(file, container.metadata(), lightmap)?
        else {
            return Ok(None);
        };
        log::info!(
            "[PRL] ShadowmaskAtlas: {} cell block(s), {} selected channel entr(y/ies), index only ({} B read)",
            index.records.len(),
            index.channels.len(),
            prefix.len(),
        );
        self.shadowmask_prefix = Some(prefix);
        Ok(Some(LoadedShadowmask {
            index,
            groups: Vec::new(),
        }))
    }

    /// The level's storage once the load has settled which indexes it keeps
    /// (id 42 may have been dropped against EntityShadowLights). The manifest
    /// reuses the SH manifest's content tag when SH streams from id 50.
    pub(crate) fn into_storage(
        self,
        container: &PrlContainer,
        lightmap: Option<&LightmapBlockIndex>,
        shadowmask: Option<&ShadowmaskBlockIndex>,
        sh_manifest: Option<&ShStreamManifest>,
    ) -> Result<LightmapStorage, PrlLoadError> {
        let (LightmapStreamingMode::Stream, Some(file), Some(lightmap)) =
            (self.mode, container.retained_file(), lightmap)
        else {
            return Ok(LightmapStorage::AllResident);
        };
        let meta = container.metadata();
        let lightmap_entry = required_entry(meta, SectionId::Lightmap)?;
        let shadowmask_entry = match shadowmask {
            Some(_) => Some(required_entry(meta, SectionId::ShadowmaskAtlas)?),
            None => None,
        };
        let content_tag = match sh_manifest {
            Some(manifest) => {
                debug_assert!(
                    Arc::ptr_eq(manifest.retained_file(), file),
                    "SH and lightmap manifests share one retained file"
                );
                manifest.content_tag()
            }
            None => lightmap_content_tag(
                meta,
                &self.lightmap_prefix,
                self.shadowmask_prefix.as_deref(),
            ),
        };
        let manifest = LightmapStreamManifest::new(
            file.clone(),
            lightmap.clone(),
            shadowmask.cloned(),
            lightmap_entry,
            shadowmask_entry,
            content_tag,
        )?;
        Ok(LightmapStorage::Streaming(Arc::new(manifest)))
    }
}

/// Id 22's header, then its records: exactly the index prefix, never a blob.
fn read_lightmap_index_prefix(
    file: &PrlFile,
    meta: &ContainerMeta,
) -> Result<Option<(LightmapBlockIndex, Vec<u8>)>, PrlLoadError> {
    let Some(entry) = meta.find_section(SectionId::Lightmap as u32) else {
        return Ok(None);
    };
    validate_positional_entry_bounds(file, meta, entry)?;
    let mut prefix = Vec::new();
    extend_prefix(file, entry, &mut prefix, LIGHTMAP_HEADER_BYTES as u64)?;
    let header = LightmapHeader::from_bytes(&prefix)
        .map_err(|err| section_validation_from_error("Lightmap", err))?;
    let index_len = header
        .index_byte_len()
        .ok_or_else(|| section_validation("Lightmap", "index length overflows"))?;
    extend_prefix(file, entry, &mut prefix, index_len)?;
    let index = LightmapBlockIndex::from_prefix(&prefix, entry.size)
        .map_err(|err| section_validation_from_error("Lightmap", err))?;
    Ok(Some((index, prefix)))
}

/// Id 42's fixed header, then its slot table through the block count, then
/// its records, each step reading only the bytes the last one proved needed.
fn read_shadowmask_index_prefix(
    file: &PrlFile,
    meta: &ContainerMeta,
    lightmap: &LightmapBlockIndex,
) -> Result<Option<(ShadowmaskBlockIndex, Vec<u8>)>, PrlLoadError> {
    let Some(entry) = meta.find_section(SectionId::ShadowmaskAtlas as u32) else {
        return Ok(None);
    };
    validate_positional_entry_bounds(file, meta, entry)?;
    let invalid = |err| section_validation_from_error("ShadowmaskAtlas", err);
    let mut prefix = Vec::new();
    extend_prefix(file, entry, &mut prefix, 8)?;
    let through_count = shadowmask_prefix_len_through_block_count(&prefix).map_err(invalid)?;
    extend_prefix(file, entry, &mut prefix, through_count as u64)?;
    if prefix.len() == through_count {
        let at = through_count - 4;
        let block_count = u32::from_le_bytes(prefix[at..through_count].try_into().unwrap());
        let index_len = u64::from(block_count)
            .checked_mul(SHADOWMASK_BLOCK_RECORD_BYTES as u64)
            .and_then(|records| records.checked_add(through_count as u64))
            .ok_or_else(|| section_validation("ShadowmaskAtlas", "index length overflows"))?;
        // A count that disagrees with id 22 fails in `from_prefix` below;
        // never read records for it.
        if block_count as usize == lightmap.records.len() {
            extend_prefix(file, entry, &mut prefix, index_len)?;
        }
    }
    let index =
        ShadowmaskBlockIndex::from_prefix(&prefix, entry.size, lightmap).map_err(invalid)?;
    Ok(Some((index, prefix)))
}

/// Grow `prefix` to `want` bytes of the section, clamped to its size so a
/// short section fails in the format parser rather than reading past it.
fn extend_prefix(
    file: &PrlFile,
    entry: &SectionEntry,
    prefix: &mut Vec<u8>,
    want: u64,
) -> Result<(), PrlLoadError> {
    let have = prefix.len() as u64;
    let want = want.min(entry.size);
    if want <= have {
        return Ok(());
    }
    let bytes = read_vec_at(file, entry.offset + have, want - have, "id-22/42 index")?;
    prefix.extend_from_slice(&bytes);
    Ok(())
}

fn required_entry(meta: &ContainerMeta, section: SectionId) -> Result<&SectionEntry, PrlLoadError> {
    meta.find_section(section as u32).ok_or_else(|| {
        lightmap_stream_error(format!(
            "section {} disappeared from the validated table",
            section as u32
        ))
    })
}

/// Content identity for a streaming level without id 50: the loader has no
/// whole-file digest, so this digests what identifies the streamed bytes'
/// layout, as SH's tag does: the section table and the id-22/42 indexes.
fn lightmap_content_tag(
    meta: &ContainerMeta,
    lightmap_prefix: &[u8],
    shadowmask_prefix: Option<&[u8]>,
) -> [u8; 32] {
    let mut hasher = blake3::Hasher::new();
    hasher.update(b"postretro.lightmap-stream.v1\0");
    hasher.update(&postretro_level_format::MAGIC);
    hasher.update(&meta.header.version.to_le_bytes());
    hasher.update(&meta.header.section_count.to_le_bytes());
    for entry in &meta.sections {
        hasher.update(&entry.section_id.to_le_bytes());
        hasher.update(&entry.offset.to_le_bytes());
        hasher.update(&entry.size.to_le_bytes());
        hasher.update(&entry.version.to_le_bytes());
    }
    hasher.update(&(lightmap_prefix.len() as u64).to_le_bytes());
    hasher.update(lightmap_prefix);
    if let Some(prefix) = shadowmask_prefix {
        hasher.update(&(prefix.len() as u64).to_le_bytes());
        hasher.update(prefix);
    }
    *hasher.finalize().as_bytes()
}
