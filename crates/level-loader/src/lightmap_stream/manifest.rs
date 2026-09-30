// Streaming-mode lightmap manifest: id-22/42 block indexes, section offsets, retained file.
// See: context/lib/rendering_pipeline.md §4 (Lightmap cell-block residency)

use std::ops::Range;
use std::sync::Arc;

use postretro_level_format::SectionEntry;
use postretro_level_format::lightmap::{
    LightmapBlockIndex, LightmapBlockPayload, SectionByteRange,
};
use postretro_level_format::shadowmask_atlas::ShadowmaskBlockIndex;

use super::boundary::LightmapDrainBatch;
use super::lightmap_stream_error;
use crate::prl::PrlLoadError;
use crate::prl_file::{PrlFile, PrlReadCounters};
use crate::sh_stream::read_vec_at;

/// Absolute PRL file ranges of one block pair, in the order its bytes are
/// handed back: id 22's irradiance then direction, and id 42's group A then
/// group B when the level keeps id 42. Each is one contiguous range, the shape
/// the shared issuer's two-range request carries.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LightmapBlockFileRanges {
    pub lightmap: Range<u64>,
    pub shadowmask: Option<Range<u64>>,
}

/// Immutable handle for a level whose lightmap blocks stream.
///
/// Holds only the id-22/42 indexes (header, slot table, records), each
/// section's absolute file range, the level's content tag, and the retained
/// file. Never a block blob.
///
/// Owner: `LevelWorld::lightmap_storage`, shared by `Arc` with the residency
/// controller and the issuer's lightmap route. The indexes are a copy of the
/// ones `LevelWorld` keeps (tens of bytes per block). Released, and with it
/// the file handle when no SH manifest shares it, when the level's storage and
/// every clone drop; level unload must drop the controller's clones.
#[derive(Debug)]
pub struct LightmapStreamManifest {
    file: Arc<PrlFile>,
    lightmap: LightmapBlockIndex,
    shadowmask: Option<ShadowmaskBlockIndex>,
    lightmap_section: Range<u64>,
    shadowmask_section: Option<Range<u64>>,
    content_tag: [u8; 32],
}

impl LightmapStreamManifest {
    /// The id-22/42 index parsers already hold each block's blobs adjacent
    /// (irradiance then direction; group A then group B), so a pair is
    /// exactly two reads.
    pub(crate) fn new(
        file: Arc<PrlFile>,
        lightmap: LightmapBlockIndex,
        shadowmask: Option<ShadowmaskBlockIndex>,
        lightmap_entry: &SectionEntry,
        shadowmask_entry: Option<&SectionEntry>,
        content_tag: [u8; 32],
    ) -> Result<Self, PrlLoadError> {
        let lightmap_section = entry_range(lightmap_entry)?;
        let shadowmask_section = match (&shadowmask, shadowmask_entry) {
            (Some(_), Some(entry)) => Some(entry_range(entry)?),
            (None, _) => None,
            (Some(_), None) => {
                return Err(lightmap_stream_error(
                    "a shadowmask index needs its id-42 table entry",
                ));
            }
        };
        if let Some(shadowmask) = &shadowmask {
            if shadowmask.records.len() != lightmap.records.len() {
                return Err(lightmap_stream_error(
                    "shadowmask block count differs from the lightmap's",
                ));
            }
        }
        Ok(Self {
            file,
            lightmap,
            shadowmask,
            lightmap_section,
            shadowmask_section,
            content_tag,
        })
    }

    pub fn block_count(&self) -> u32 {
        self.lightmap.records.len() as u32
    }

    pub fn lightmap_index(&self) -> &LightmapBlockIndex {
        &self.lightmap
    }

    pub fn shadowmask_index(&self) -> Option<&ShadowmaskBlockIndex> {
        self.shadowmask.as_ref()
    }

    /// The level's content identity for stale-completion checks: the SH
    /// manifest's tag when SH streams from id 50, so both resources share one
    /// identity; otherwise a digest of the table and the id-22/42 indexes.
    pub fn content_tag(&self) -> [u8; 32] {
        self.content_tag
    }

    /// The level's per-section read counters (shared with the loader and the
    /// SH manifest through the one retained file).
    pub fn read_counters(&self) -> &Arc<PrlReadCounters> {
        self.file.read_counters()
    }

    /// Absolute file range of the id-22 section.
    pub fn lightmap_section_range(&self) -> Range<u64> {
        self.lightmap_section.clone()
    }

    /// Absolute file range of the id-42 section, when the level keeps it.
    pub fn shadowmask_section_range(&self) -> Option<Range<u64>> {
        self.shadowmask_section.clone()
    }

    /// The file region a span read may cover: from the first to the last
    /// byte of ids 22 and 42 together. The issuer merges lightmap reads only
    /// when their ranges are byte-contiguous, which can join id 22's tail to
    /// id 42's head when the two sections touch.
    pub fn span_region(&self) -> Range<u64> {
        match &self.shadowmask_section {
            Some(shadowmask) => {
                self.lightmap_section.start.min(shadowmask.start)
                    ..self.lightmap_section.end.max(shadowmask.end)
            }
            None => self.lightmap_section.clone(),
        }
    }

    /// Absolute file ranges of block `block`'s pair.
    pub fn block_file_ranges(&self, block: u32) -> Result<LightmapBlockFileRanges, PrlLoadError> {
        let index = block as usize;
        let record = self.lightmap.records.get(index).ok_or_else(|| {
            lightmap_stream_error(format!(
                "block {block} is past the {}-block index",
                self.lightmap.records.len()
            ))
        })?;
        let lightmap = absolute(
            &self.lightmap_section,
            record.irradiance.offset,
            record.direction,
        )?;
        let shadowmask = match (&self.shadowmask, &self.shadowmask_section) {
            (Some(shadowmask), Some(section)) => {
                let groups = shadowmask.records[index];
                Some(absolute(section, groups.group_a.offset, groups.group_b)?)
            }
            _ => None,
        };
        Ok(LightmapBlockFileRanges {
            lightmap,
            shadowmask,
        })
    }

    /// One counted positional read of an absolute span, which may cover
    /// several byte-contiguous block ranges, across the id-22/42 boundary
    /// when the sections touch. A span reaching outside [`Self::span_region`] is rejected
    /// before allocation.
    pub fn read_file_span(&self, range: Range<u64>) -> Result<Vec<u8>, PrlLoadError> {
        let region = self.span_region();
        if range.start > range.end || range.start < region.start || range.end > region.end {
            return Err(lightmap_stream_error(format!(
                "file span {}..{} is outside the id-22/42 region {}..{}",
                range.start, range.end, region.start, region.end
            )));
        }
        read_vec_at(
            &self.file,
            range.start,
            range.end - range.start,
            "id-22/42 span",
        )
    }

    /// Read one block pair synchronously: its id-22 range, then its id-42
    /// range. For install preload and capture; in play the issuer reads.
    pub fn read_block_pair(&self, block: u32) -> Result<LightmapBlockPayload, PrlLoadError> {
        let ranges = self.block_file_ranges(block)?;
        let lightmap = self.read_file_span(ranges.lightmap)?;
        let shadowmask = ranges
            .shadowmask
            .map(|range| self.read_file_span(range))
            .transpose()?;
        self.payload_from_pair_bytes(block, lightmap, shadowmask)
    }

    /// Split a pair's two read buffers into the payload the pool installs.
    /// Rejects buffers whose lengths disagree with the block's ranges.
    pub fn payload_from_pair_bytes(
        &self,
        block: u32,
        lightmap: Vec<u8>,
        shadowmask: Option<Vec<u8>>,
    ) -> Result<LightmapBlockPayload, PrlLoadError> {
        let ranges = self.block_file_ranges(block)?;
        let record = self.lightmap.records[block as usize];
        let (irradiance, direction) = split_pair(
            block,
            "lightmap",
            lightmap,
            &ranges.lightmap,
            record.irradiance.len,
        )?;
        let shadowmask = match (ranges.shadowmask, shadowmask) {
            (Some(range), Some(bytes)) => {
                let group_a_len = self
                    .shadowmask
                    .as_ref()
                    .map_or(0, |index| index.records[block as usize].group_a.len);
                Some(split_pair(block, "shadowmask", bytes, &range, group_a_len)?)
            }
            (None, None) => None,
            (Some(_), None) | (None, Some(_)) => {
                return Err(lightmap_stream_error(format!(
                    "block {block} shadowmask bytes do not match the level's id-42 presence"
                )));
            }
        };
        Ok(LightmapBlockPayload {
            irradiance,
            direction,
            shadowmask: shadowmask.map(|(group_a, group_b)| [group_a, group_b]),
        })
    }

    /// Validate a drain batch against this level's block count and identity
    /// before either side mutates state.
    pub fn validate_drain_batch(&self, batch: &LightmapDrainBatch) -> Result<(), PrlLoadError> {
        batch.validate_contract(self.block_count(), self.content_tag)
    }

    #[cfg(test)]
    pub(crate) fn retained_file(&self) -> &Arc<PrlFile> {
        &self.file
    }
}

fn entry_range(entry: &SectionEntry) -> Result<Range<u64>, PrlLoadError> {
    let end = entry.offset.checked_add(entry.size).ok_or_else(|| {
        lightmap_stream_error(format!(
            "section {} end overflows the file offset space",
            entry.section_id
        ))
    })?;
    Ok(entry.offset..end)
}

/// `first_offset..second.end()` inside `section`, made absolute. The index
/// validated both blobs against the section length.
fn absolute(
    section: &Range<u64>,
    first_offset: u64,
    second: SectionByteRange,
) -> Result<Range<u64>, PrlLoadError> {
    let overflow = || lightmap_stream_error("block file range overflows");
    let end = second.end().ok_or_else(overflow)?;
    let start = section
        .start
        .checked_add(first_offset)
        .ok_or_else(overflow)?;
    let end = section.start.checked_add(end).ok_or_else(overflow)?;
    Ok(start..end)
}

fn split_pair(
    block: u32,
    what: &str,
    mut bytes: Vec<u8>,
    range: &Range<u64>,
    first_len: u32,
) -> Result<(Vec<u8>, Vec<u8>), PrlLoadError> {
    if bytes.len() as u64 != range.end - range.start {
        return Err(lightmap_stream_error(format!(
            "block {block} {what} read returned {} bytes, expected {}",
            bytes.len(),
            range.end - range.start
        )));
    }
    let second = bytes.split_off(first_len as usize);
    Ok((bytes, second))
}
