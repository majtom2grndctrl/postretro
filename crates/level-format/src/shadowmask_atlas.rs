// ShadowmaskAtlas PRL section (ID 42): per-selected-light baked visibility
// masks, stored per lightmap cell block.
// Governing context: context/lib/build_pipeline.md

use crate::lightmap::{
    LightmapBlockIndex, SectionByteRange, check_adjacent, check_blob, eof, invalid, read_u32,
    read_u64, slice_range,
};

pub const SHADOWMASK_CHANNEL_DROPPED: u8 = 0xFF;

/// BC5 `.rg` per cell block, two mask groups per block: group 0 (slots 0/1)
/// and group 1 (slots 2/3), each its own BC5 plane at the block's lightmap
/// extent. The pool layer places them side by side. ASCII `SMB6`.
pub const SHADOWMASK_FORMAT_BC5_RG_BLOCKS: u32 = u32::from_le_bytes(*b"SMB6");

/// The retired whole-layer layout. Named so load can say "re-bake" instead
/// of "unknown tag".
const RETIRED_SHADOWMASK_FORMAT_SMB5: u32 = u32::from_le_bytes(*b"SMB5");

/// Mask groups per block.
pub const SHADOWMASK_GROUP_COUNT: u32 = 2;

/// One block record: group_a_offset u64, group_a_len u32, group_b_offset u64,
/// group_b_len u32, reserved u32.
pub const SHADOWMASK_BLOCK_RECORD_BYTES: usize = 28;

const BC5_BLOCK_BYTES: u64 = 16;
/// format tag + selected_light_count.
const FIXED_HEADER_BYTES: usize = 8;

/// Bytes of one BC5 group plane at a `width × height` block extent.
pub fn group_plane_len(width: u32, height: u32) -> Option<u64> {
    u64::from(width)
        .div_ceil(4)
        .checked_mul(u64::from(height).div_ceil(4))?
        .checked_mul(BC5_BLOCK_BYTES)
}

/// Prefix byte counts a streaming loader reads in steps: the fixed header
/// first, then through the block count, then the records.
///
/// Rejects a slot table that cannot fit the `section_len`-byte section beside
/// the block count, id 22's block count of records, and each block's two
/// group planes. A bogus `selected_light_count` then never makes a reader
/// fetch more than the index.
pub fn shadowmask_prefix_len_through_block_count(
    fixed_header: &[u8],
    section_len: u64,
    lightmap: &LightmapBlockIndex,
) -> crate::Result<usize> {
    if fixed_header.len() < FIXED_HEADER_BYTES {
        return Err(eof("shadowmask section too short for header"));
    }
    check_format_tag(read_u32(fixed_header, 0))?;
    let selected = read_u32(fixed_header, 4);
    let table = u64::from(selected) + padding_to_4(selected as usize) as u64;
    let room = section_len
        .checked_sub(min_bytes_beside_the_slot_table(lightmap)?)
        .unwrap_or(0);
    if table > room {
        return Err(invalid(format!(
            "shadowmask section of {section_len} bytes cannot hold a {table}-byte slot table \
             (selected_light_count {selected}) beside the records and group planes of the \
             lightmap's {} blocks",
            lightmap.records.len()
        )));
    }
    Ok(FIXED_HEADER_BYTES + table as usize + 4)
}

/// Fixed header, block count, and id 22's block count of records and group
/// plane pairs: every byte of a section except its slot table.
fn min_bytes_beside_the_slot_table(lightmap: &LightmapBlockIndex) -> crate::Result<u64> {
    let overflow = || invalid("shadowmask section size overflows");
    let mut total = (lightmap.records.len() as u64)
        .checked_mul(SHADOWMASK_BLOCK_RECORD_BYTES as u64)
        .and_then(|records| records.checked_add(FIXED_HEADER_BYTES as u64 + 4))
        .ok_or_else(overflow)?;
    for record in &lightmap.records {
        let planes = group_plane_len(u32::from(record.width), u32::from(record.height))
            .and_then(|plane| plane.checked_mul(u64::from(SHADOWMASK_GROUP_COUNT)))
            .ok_or_else(overflow)?;
        total = total.checked_add(planes).ok_or_else(overflow)?;
    }
    Ok(total)
}

/// One block's shadowmask ranges, in id-22 block order.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ShadowmaskBlockRecord {
    pub group_a: SectionByteRange,
    pub group_b: SectionByteRange,
}

/// Slot table plus every block record: what a loaded level keeps.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ShadowmaskBlockIndex {
    /// One entry per EntityShadowLights selection index: slot `0..3`, or
    /// 0xFF when that selected light was globally dropped from the mask.
    /// Slot `s` addresses group `s / 2`, channel `s % 2`.
    pub channels: Vec<u8>,
    pub records: Vec<ShadowmaskBlockRecord>,
}

impl ShadowmaskBlockIndex {
    /// Bytes of header, slot table, block count and records.
    pub fn index_byte_len(&self) -> usize {
        index_len(self.channels.len(), self.records.len())
    }

    /// Parse and validate the index against the section length and the
    /// id-22 index it pairs with. Rejects an unknown or retired format tag,
    /// a slot table too large for the section, an invalid slot, nonzero slot
    /// table padding, a block count that differs from id 22's, a group blob
    /// whose length disagrees with its lightmap block's extent, a blob range
    /// outside the section blobs, a group B blob that does not start where
    /// group A ends, and a nonzero reserved field.
    pub fn from_prefix(
        prefix: &[u8],
        section_len: u64,
        lightmap: &LightmapBlockIndex,
    ) -> crate::Result<Self> {
        let through_count =
            shadowmask_prefix_len_through_block_count(prefix, section_len, lightmap)?;
        if prefix.len() < through_count {
            return Err(eof("shadowmask section truncated in channel table"));
        }
        let selected = read_u32(prefix, 4) as usize;
        let table_end = FIXED_HEADER_BYTES + selected;
        let channels = prefix[FIXED_HEADER_BYTES..table_end].to_vec();
        if let Some(&channel) = channels.iter().find(|&&c| !is_valid_slot(c)) {
            return Err(invalid(format!(
                "shadowmask slot {channel} is not 0..3 or 0xFF"
            )));
        }
        let padding = &prefix[table_end..through_count - 4];
        if padding.iter().any(|&byte| byte != 0) {
            return Err(invalid(format!(
                "shadowmask slot table padding {padding:02x?} is not zero"
            )));
        }
        let block_count = read_u32(prefix, through_count - 4) as usize;
        if block_count != lightmap.records.len() {
            return Err(invalid(format!(
                "shadowmask block count {block_count} does not match the lightmap's {}",
                lightmap.records.len()
            )));
        }
        let index_len = index_len(selected, block_count);
        if prefix.len() < index_len {
            return Err(eof(format!(
                "shadowmask index truncated: need {index_len} bytes, got {}",
                prefix.len()
            )));
        }
        let index_len = index_len as u64;
        if section_len < index_len {
            return Err(eof(format!(
                "shadowmask section of {section_len} bytes cannot hold its {index_len}-byte index"
            )));
        }
        let mut records = Vec::with_capacity(block_count);
        for (block, lightmap_record) in lightmap.records.iter().enumerate() {
            let at = through_count + block * SHADOWMASK_BLOCK_RECORD_BYTES;
            let record = ShadowmaskBlockRecord {
                group_a: SectionByteRange {
                    offset: read_u64(prefix, at),
                    len: read_u32(prefix, at + 8),
                },
                group_b: SectionByteRange {
                    offset: read_u64(prefix, at + 12),
                    len: read_u32(prefix, at + 20),
                },
            };
            let reserved = read_u32(prefix, at + 24);
            if reserved != 0 {
                return Err(invalid(format!(
                    "shadowmask block {block} has nonzero reserved field {reserved:#x}"
                )));
            }
            let expected = group_plane_len(
                u32::from(lightmap_record.width),
                u32::from(lightmap_record.height),
            )
            .ok_or_else(|| invalid("shadowmask group length overflows"))?;
            for (what, range) in [("group A", record.group_a), ("group B", record.group_b)] {
                check_blob(
                    "shadowmask",
                    block,
                    what,
                    range,
                    expected,
                    index_len,
                    section_len,
                )?;
            }
            check_adjacent(
                "shadowmask",
                block,
                ("group A", record.group_a),
                ("group B", record.group_b),
            )?;
            records.push(record);
        }
        Ok(Self { channels, records })
    }
}

/// A whole id-42 section in memory. Block `i` pairs with id-22 block `i` and
/// shares its extent; the pair installs together or not at all.
///
/// On-disk layout (little-endian; offsets from the payload start):
///
/// ```text
///   u32 format                (= SHADOWMASK_FORMAT_BC5_RG_BLOCKS, "SMB6")
///   u32 selected_light_count
///   u8  channels[selected_light_count], zero-padded to 4
///   u32 block_count           (= id 22's block_count)
///   block_count records (28 bytes each), in id-22 block order:
///     u64 group_a_offset, u32 group_a_len
///     u64 group_b_offset, u32 group_b_len
///     u32 reserved (= 0)
///   Blobs, in record order: group A then group B, adjacent, each BC5 at the
///   block's width × height.
/// ```
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ShadowmaskAtlasSection {
    pub channels: Vec<u8>,
    /// Per block: group A then group B BC5 planes.
    pub blocks: Vec<[Vec<u8>; 2]>,
}

impl ShadowmaskAtlasSection {
    /// The index this section serializes with.
    pub fn index(&self) -> ShadowmaskBlockIndex {
        let mut cursor = index_len(self.channels.len(), self.blocks.len()) as u64;
        let records = self
            .blocks
            .iter()
            .map(|[a, b]| {
                let group_a = SectionByteRange {
                    offset: cursor,
                    len: a.len() as u32,
                };
                cursor += a.len() as u64;
                let group_b = SectionByteRange {
                    offset: cursor,
                    len: b.len() as u32,
                };
                cursor += b.len() as u64;
                ShadowmaskBlockRecord { group_a, group_b }
            })
            .collect();
        ShadowmaskBlockIndex {
            channels: self.channels.clone(),
            records,
        }
    }

    pub fn byte_len(&self) -> usize {
        index_len(self.channels.len(), self.blocks.len())
            + self
                .blocks
                .iter()
                .map(|[a, b]| a.len() + b.len())
                .sum::<usize>()
    }

    /// Header, slot table, block count and records: everything before the
    /// blobs. A streamed writer can emit this first, then each block's groups.
    pub fn index_bytes(&self) -> Vec<u8> {
        let index = self.index();
        let mut out = Vec::with_capacity(index.index_byte_len());
        out.extend_from_slice(&SHADOWMASK_FORMAT_BC5_RG_BLOCKS.to_le_bytes());
        out.extend_from_slice(&(self.channels.len() as u32).to_le_bytes());
        out.extend_from_slice(&self.channels);
        out.extend(std::iter::repeat_n(0u8, padding_to_4(self.channels.len())));
        out.extend_from_slice(&(self.blocks.len() as u32).to_le_bytes());
        for record in &index.records {
            out.extend_from_slice(&record.group_a.offset.to_le_bytes());
            out.extend_from_slice(&record.group_a.len.to_le_bytes());
            out.extend_from_slice(&record.group_b.offset.to_le_bytes());
            out.extend_from_slice(&record.group_b.len.to_le_bytes());
            out.extend_from_slice(&0u32.to_le_bytes());
        }
        out
    }

    pub fn to_bytes(&self) -> Vec<u8> {
        let mut out = self.index_bytes();
        out.reserve_exact(self.byte_len() - out.len());
        for [a, b] in &self.blocks {
            out.extend_from_slice(a);
            out.extend_from_slice(b);
        }
        out
    }

    /// Parse a whole section against the id-22 index it pairs with.
    pub fn from_bytes(data: &[u8], lightmap: &LightmapBlockIndex) -> crate::Result<Self> {
        let index = ShadowmaskBlockIndex::from_prefix(data, data.len() as u64, lightmap)?;
        let blocks = index
            .records
            .iter()
            .map(|r| {
                [
                    slice_range(data, r.group_a).to_vec(),
                    slice_range(data, r.group_b).to_vec(),
                ]
            })
            .collect();
        Ok(Self {
            channels: index.channels,
            blocks,
        })
    }
}

fn check_format_tag(format: u32) -> crate::Result<()> {
    match format {
        SHADOWMASK_FORMAT_BC5_RG_BLOCKS => Ok(()),
        RETIRED_SHADOWMASK_FORMAT_SMB5 => Err(invalid(
            "shadowmask section uses the retired whole-layer SMB5 layout; re-bake the level",
        )),
        other => Err(invalid(format!(
            "shadowmask format tag {other:#010x} is unknown"
        ))),
    }
}

fn index_len(selected: usize, block_count: usize) -> usize {
    FIXED_HEADER_BYTES
        + selected
        + padding_to_4(selected)
        + 4
        + block_count * SHADOWMASK_BLOCK_RECORD_BYTES
}

fn is_valid_slot(channel: u8) -> bool {
    channel <= 3 || channel == SHADOWMASK_CHANNEL_DROPPED
}

fn padding_to_4(len: usize) -> usize {
    (4 - (len % 4)) % 4
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::SectionId;
    use crate::lightmap::{IRRADIANCE_FORMAT_BC6H, LightmapBlock, LightmapMode, LightmapSection};

    fn lightmap(extents: &[(u16, u16)]) -> LightmapSection {
        LightmapSection {
            direction_texel_scale: 2,
            irradiance_format: IRRADIANCE_FORMAT_BC6H,
            mode: LightmapMode::Shadowed,
            blocks: extents
                .iter()
                .enumerate()
                .map(|(i, &(w, h))| {
                    let (w32, h32) = (u32::from(w), u32::from(h));
                    LightmapBlock {
                        cell_id: i as u32,
                        width: w,
                        height: h,
                        irradiance: vec![0; (w32 / 4 * h32 / 4 * 16) as usize],
                        direction: vec![0; (w32 / 2 * h32 / 2 * 2) as usize],
                    }
                })
                .collect(),
        }
    }

    fn section_for(lightmap: &LightmapSection, channels: Vec<u8>) -> ShadowmaskAtlasSection {
        let blocks = lightmap
            .blocks
            .iter()
            .enumerate()
            .map(|(i, b)| {
                let len =
                    group_plane_len(u32::from(b.width), u32::from(b.height)).unwrap() as usize;
                [
                    (0..len).map(|t| (t + i) as u8).collect(),
                    (0..len).map(|t| (t * 3 + i) as u8).collect(),
                ]
            })
            .collect();
        ShadowmaskAtlasSection { channels, blocks }
    }

    fn error_message(result: crate::Result<impl std::fmt::Debug>) -> String {
        format!("{:?}", result.expect_err("expected a rejection"))
    }

    #[test]
    fn shadowmask_blocks_round_trip_with_empty_single_and_full_slot_tables() {
        let lm = lightmap(&[(8, 4), (4, 12)]);
        let index = lm.index();
        for channels in [
            vec![],
            vec![2],
            vec![0, 1, 2, 3, SHADOWMASK_CHANNEL_DROPPED],
        ] {
            let section = section_for(&lm, channels);
            let bytes = section.to_bytes();
            assert_eq!(bytes.len(), section.byte_len());
            assert_eq!(
                ShadowmaskAtlasSection::from_bytes(&bytes, &index).unwrap(),
                section
            );
        }
    }

    #[test]
    fn index_parses_in_steps_from_the_prefix() {
        let lm = lightmap(&[(8, 4), (4, 12)]);
        let section = section_for(&lm, vec![0, 3, 1]);
        let bytes = section.to_bytes();
        let (section_len, lm_index) = (bytes.len() as u64, lm.index());
        let through =
            shadowmask_prefix_len_through_block_count(&bytes[..8], section_len, &lm_index).unwrap();
        assert_eq!(through, 8 + 4 + 4);
        let index_len = section.index().index_byte_len();
        assert_eq!(index_len, through + 2 * SHADOWMASK_BLOCK_RECORD_BYTES);
        let index =
            ShadowmaskBlockIndex::from_prefix(&bytes[..index_len], bytes.len() as u64, &lm.index())
                .unwrap();
        assert_eq!(index, section.index());
    }

    #[test]
    fn rejects_block_count_mismatch_with_the_lightmap() {
        let lm = lightmap(&[(8, 4), (4, 12)]);
        let bytes = section_for(&lm, vec![0]).to_bytes();
        let fewer = lightmap(&[(8, 4)]);
        let message = error_message(ShadowmaskAtlasSection::from_bytes(&bytes, &fewer.index()));
        assert!(message.contains("does not match the lightmap"), "{message}");
    }

    #[test]
    fn rejects_group_length_that_disagrees_with_the_lightmap_extent() {
        let lm = lightmap(&[(8, 4)]);
        let bytes = section_for(&lm, vec![0]).to_bytes();
        let narrower = lightmap(&[(4, 4)]);
        let message = error_message(ShadowmaskAtlasSection::from_bytes(
            &bytes,
            &narrower.index(),
        ));
        assert!(message.contains("group A blob"), "{message}");
    }

    #[test]
    fn rejects_group_b_that_does_not_start_where_group_a_ends() {
        let lm = lightmap(&[(8, 4), (4, 12)]);
        let mut bytes = section_for(&lm, vec![0]).to_bytes();
        // Block 0's group B moved one byte on: still inside the section and
        // the right length, but no longer adjacent to its group A.
        let at = 8 + 4 + 4 + 12;
        let offset = read_u64(&bytes, at) + 1;
        bytes[at..at + 8].copy_from_slice(&offset.to_le_bytes());
        let message = error_message(ShadowmaskAtlasSection::from_bytes(&bytes, &lm.index()));
        let expected = format!(
            "shadowmask block 0 group B blob at {offset} does not start where its group A blob ends"
        );
        assert!(message.contains(&expected), "{message}");
    }

    #[test]
    fn rejects_nonzero_slot_table_padding() {
        let lm = lightmap(&[(8, 4)]);
        let mut bytes = section_for(&lm, vec![2]).to_bytes();
        // One slot at byte 8, then three padding bytes before the block count.
        bytes[10] = 7;
        let message = error_message(ShadowmaskAtlasSection::from_bytes(&bytes, &lm.index()));
        assert!(message.contains("padding"), "{message}");
    }

    #[test]
    fn rejects_a_slot_table_that_cannot_fit_beside_the_block_records_and_planes() {
        let lm = lightmap(&[(8, 4)]);
        let index = lm.index();
        let mut bytes = section_for(&lm, vec![0]).to_bytes();
        let section_len = bytes.len() as u64;
        // Header, count, one record and two 32-byte planes leave 4 bytes:
        // exactly the one-slot table padded to 4.
        assert_eq!(
            shadowmask_prefix_len_through_block_count(&bytes[..8], section_len, &index).unwrap(),
            8 + 4 + 4
        );
        // A count that still fits the section whole, as a bare section-length
        // bound would allow, but not beside the records and planes.
        let bogus = (bytes.len() - 16) as u32;
        bytes[4..8].copy_from_slice(&bogus.to_le_bytes());
        let message = error_message(shadowmask_prefix_len_through_block_count(
            &bytes[..8],
            section_len,
            &index,
        ));
        assert!(message.contains("cannot hold a"), "{message}");
        let message = error_message(ShadowmaskAtlasSection::from_bytes(&bytes, &index));
        assert!(message.contains("cannot hold a"), "{message}");
    }

    #[test]
    fn rejects_retired_smb5_tag_with_a_rebake_hint_and_unknown_tags() {
        let lm = lightmap(&[(8, 4)]);
        let mut bytes = section_for(&lm, vec![0]).to_bytes();
        bytes[0..4].copy_from_slice(b"SMB5");
        let message = error_message(ShadowmaskAtlasSection::from_bytes(&bytes, &lm.index()));
        assert!(message.contains("re-bake"), "{message}");
        bytes[0..4].copy_from_slice(b"XXXX");
        let message = error_message(ShadowmaskAtlasSection::from_bytes(&bytes, &lm.index()));
        assert!(message.contains("unknown"), "{message}");
    }

    #[test]
    fn rejects_invalid_slot_nonzero_reserved_and_out_of_section_range() {
        let lm = lightmap(&[(8, 4)]);
        let good = section_for(&lm, vec![0]).to_bytes();

        let mut bad_slot = good.clone();
        bad_slot[8] = 4;
        assert!(
            error_message(ShadowmaskAtlasSection::from_bytes(&bad_slot, &lm.index()))
                .contains("slot 4")
        );

        let records = 8 + 4 + 4;
        let mut reserved = good.clone();
        reserved[records + 24..records + 28].copy_from_slice(&1u32.to_le_bytes());
        assert!(
            error_message(ShadowmaskAtlasSection::from_bytes(&reserved, &lm.index()))
                .contains("reserved")
        );

        let mut outside = good.clone();
        outside[records + 12..records + 20].copy_from_slice(&(good.len() as u64).to_le_bytes());
        assert!(
            error_message(ShadowmaskAtlasSection::from_bytes(&outside, &lm.index()))
                .contains("outside the section")
        );
    }

    #[test]
    fn section_id_is_pinned() {
        assert_eq!(SectionId::ShadowmaskAtlas as u32, 42);
        assert_eq!(SectionId::from_u32(42), Some(SectionId::ShadowmaskAtlas));
    }
}
