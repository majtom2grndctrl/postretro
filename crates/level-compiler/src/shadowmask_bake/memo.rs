// Whole-section shadowmask memo: input hash, cache read/write with the drop
// report, and stale-entry validation.
// See: context/lib/build_pipeline.md §Build Cache

use postretro_level_format::lightmap::{
    LightmapBlockIndex, LightmapBlockRecord, LightmapHeader, LightmapMode, SectionByteRange,
};
use postretro_level_format::shadowmask_atlas::{ShadowmaskAtlasSection, ShadowmaskOwnerBounds};

use super::eligibility::EligibleLight;
use super::ownership::{DropSite, LIT_TEXEL_FLOOR, OwnerDrop, OwnerReport};
#[cfg(test)]
use super::{
    RAW_FILL_LIVE_BYTES_AT_CACHE_WRITE, SHADOWMASK_STREAMED_CACHE_WRITE_COUNT, raw_fill_live_bytes,
};
use crate::cache::{CacheKey, StageCache};
use crate::lightmap_layer::{self, SharedAtlas};

/// Whole-section input hash for the `"shadowmask_atlas"` memo entry.
///
/// Fixed, order-sensitive byte layout: `LAYER_FORMAT_VERSION`, the lit-texel
/// floor, the face count, the eligible count, then per eligible light (in
/// ascending spec index) its spec index, its id-40 membership and its
/// `(light, atlas layer)` layer hashes in ascending layer order, then atlas
/// width/height/layer-count, then `estimate_params` verbatim. The spec indices
/// are hashed because inserting a dynamic light shifts them without changing
/// any layer hash.
pub(crate) fn shadowmask_atlas_input_hash(
    eligible: &[EligibleLight<'_>],
    layer_input_hashes: &[[u8; 32]],
    face_count: usize,
    atlas_width: u32,
    atlas_height: u32,
    layer_count: u32,
    estimate_params: &[u8],
) -> [u8; 32] {
    debug_assert_eq!(
        eligible.len().saturating_mul(layer_count as usize),
        layer_input_hashes.len(),
        "eligible light/layer hash slices must align"
    );
    let mut hasher = blake3::Hasher::new();
    hasher.update(&lightmap_layer::LAYER_FORMAT_VERSION.to_le_bytes());
    hasher.update(&LIT_TEXEL_FLOOR.to_le_bytes());
    hasher.update(&(face_count as u64).to_le_bytes());
    hasher.update(&(eligible.len() as u32).to_le_bytes());
    for (light, hashes) in eligible
        .iter()
        .zip(layer_input_hashes.chunks(layer_count.max(1) as usize))
    {
        hasher.update(&light.spec_index.to_le_bytes());
        hasher.update(&[u8::from(light.promoted)]);
        for hash in hashes {
            hasher.update(hash);
        }
    }
    hasher.update(&atlas_width.to_le_bytes());
    hasher.update(&atlas_height.to_le_bytes());
    hasher.update(&layer_count.to_le_bytes());
    hasher.update(&(estimate_params.len() as u32).to_le_bytes());
    hasher.update(estimate_params);
    *hasher.finalize().as_bytes()
}

pub(super) struct ShadowmaskMemo {
    pub(super) section: Option<ShadowmaskAtlasSection>,
    pub(super) report: OwnerReport,
}

/// Memo entry: the drop report, a presence byte, then (when present) the
/// section's wire bytes. The report shares the entry so a warm build repeats
/// the cold build's warnings and can never lose them apart from the section.
const DROP_RECORD_BYTES: usize = 24;

fn report_bytes(report: &OwnerReport) -> Vec<u8> {
    let mut out = Vec::with_capacity(8 + report.drops.len() * DROP_RECORD_BYTES);
    out.extend_from_slice(&report.peak_face_demand.to_le_bytes());
    out.extend_from_slice(&(report.drops.len() as u32).to_le_bytes());
    for drop in &report.drops {
        let (kind, site, first, last) = match drop.site {
            DropSite::Face(face) => (0u32, face, face, face),
            DropSite::CutFace {
                parent,
                first_face,
                last_face,
            } => (1u32, parent, first_face, last_face),
        };
        for word in [
            kind,
            site,
            first,
            last,
            u32::from(drop.spec_index),
            drop.lit_texels,
        ] {
            out.extend_from_slice(&word.to_le_bytes());
        }
    }
    out
}

/// The report and the bytes after it, or `None` when the entry is malformed.
fn parse_report(bytes: &[u8]) -> Option<(OwnerReport, &[u8])> {
    let word = |bytes: &[u8], at: usize| -> Option<u32> {
        Some(u32::from_le_bytes(bytes.get(at..at + 4)?.try_into().ok()?))
    };
    let peak_face_demand = word(bytes, 0)?;
    let count = word(bytes, 4)? as usize;
    let end = count.checked_mul(DROP_RECORD_BYTES)?.checked_add(8)?;
    let records = bytes.get(8..end)?;
    let mut drops = Vec::with_capacity(count);
    for record in records.chunks_exact(DROP_RECORD_BYTES) {
        let w = |i: usize| word(record, i * 4);
        let site = match w(0)? {
            0 => DropSite::Face(w(1)?),
            1 => DropSite::CutFace {
                parent: w(1)?,
                first_face: w(2)?,
                last_face: w(3)?,
            },
            _ => return None,
        };
        drops.push(OwnerDrop {
            site,
            spec_index: u16::try_from(w(4)?).ok()?,
            lit_texels: w(5)?,
        });
    }
    Some((
        OwnerReport {
            peak_face_demand,
            drops,
        },
        &bytes[end..],
    ))
}

/// A memo entry usable for this atlas and owner bounds, or `None` (logged)
/// when it is absent, truncated, or undecodable.
pub(super) fn read_shadowmask_memo(
    cache: &StageCache,
    key: &CacheKey,
    shared: &SharedAtlas<'_>,
    bounds: ShadowmaskOwnerBounds,
) -> Option<ShadowmaskMemo> {
    let bytes = cache.get(key)?;
    let Some((report, rest)) = parse_report(&bytes) else {
        log::warn!("[Compiler] corrupt shadowmask atlas, re-baking: memo entry has no drop report");
        return None;
    };
    let section = match rest.split_first() {
        Some((0, [])) => None,
        Some((1, section_bytes)) => {
            match ShadowmaskAtlasSection::from_bytes(section_bytes, &extent_index(shared), bounds) {
                Ok(section) => Some(section),
                Err(err) => {
                    log::warn!("[Compiler] corrupt shadowmask atlas, re-baking: {err}");
                    return None;
                }
            }
        }
        _ => {
            log::warn!("[Compiler] corrupt shadowmask atlas, re-baking: bad presence byte");
            return None;
        }
    };
    Some(ShadowmaskMemo { section, report })
}

/// Store the report and section as one entry, streaming the section's own
/// byte order with no second contiguous copy of the payload.
pub(super) fn cache_shadowmask_section(
    cache: &StageCache,
    section_key: &CacheKey,
    section: Option<&ShadowmaskAtlasSection>,
    report: &OwnerReport,
) {
    let report = report_bytes(report);
    #[cfg(test)]
    {
        SHADOWMASK_STREAMED_CACHE_WRITE_COUNT.with(|count| count.set(count.get() + 1));
        RAW_FILL_LIVE_BYTES_AT_CACHE_WRITE
            .with(|at_write| at_write.set(Some(raw_fill_live_bytes())));
    }
    let entry_len = report.len() + 1 + section.map_or(0, ShadowmaskAtlasSection::byte_len);
    cache.put_streamed(section_key, entry_len as u64, |writer| {
        writer.write_all(&report)?;
        let Some(section) = section else {
            return writer.write_all(&[0]);
        };
        writer.write_all(&[1])?;
        writer.write_all(&section.index_bytes())?;
        for [group_a, group_b] in &section.blocks {
            writer.write_all(group_a)?;
            writer.write_all(group_b)?;
        }
        Ok(())
    });
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
