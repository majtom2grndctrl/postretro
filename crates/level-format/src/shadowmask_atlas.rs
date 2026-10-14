// ShadowmaskAtlas PRL section (ID 42): each lightmapped face's owner lights
// and their baked visibility masks, stored per lightmap cell block.
// Governing context: context/lib/build_pipeline.md §PRL section IDs

use crate::lightmap::{
    LightmapBlockIndex, SectionByteRange, check_adjacent, check_after_previous_block, check_blob,
    eof, invalid, read_u32, read_u64, slice_range,
};

/// An empty channel in a face's owner entry.
pub const SHADOWMASK_OWNER_EMPTY: u16 = 0xFFFF;

/// Owner channels per face: one per mask channel, two per BC5 group.
pub const SHADOWMASK_OWNER_CHANNELS: usize = 4;

/// A face's owners: compact static spec-light indices (the `!is_dynamic`
/// order `pack_spec_lights` and id 23 use), ascending, empties trailing.
/// Position `s` is channel `s`: group `s / 2`, BC5 channel `s % 2`.
pub type FaceOwners = [u16; SHADOWMASK_OWNER_CHANNELS];

/// A face no light owns.
pub const NO_FACE_OWNERS: FaceOwners = [SHADOWMASK_OWNER_EMPTY; SHADOWMASK_OWNER_CHANNELS];

/// Per-face owner table, then BC5 `.rg` per cell block with two mask groups:
/// group 0 (channels 0/1) and group 1 (channels 2/3), each its own BC5 plane
/// at the block's lightmap extent. The pool layer places them side by side.
/// ASCII `SMB7`.
pub const SHADOWMASK_FORMAT_OWNED_BC5_RG_BLOCKS: u32 = u32::from_le_bytes(*b"SMB7");

/// Retired layouts, named so load can say "re-bake" instead of "unknown tag":
/// SMB5 held whole bake layers; SMB6 held one map-wide slot per selected light.
const RETIRED_SHADOWMASK_FORMAT_SMB5: u32 = u32::from_le_bytes(*b"SMB5");
const RETIRED_SHADOWMASK_FORMAT_SMB6: u32 = u32::from_le_bytes(*b"SMB6");

/// Mask groups per block.
pub const SHADOWMASK_GROUP_COUNT: u32 = 2;

/// One block record: group_a_offset u64, group_a_len u32, group_b_offset u64,
/// group_b_len u32, reserved u32.
pub const SHADOWMASK_BLOCK_RECORD_BYTES: usize = 28;

const BC5_BLOCK_BYTES: u64 = 16;
/// format tag + face_count.
const FIXED_HEADER_BYTES: usize = 8;
/// Four u16 owners.
const OWNER_ENTRY_BYTES: usize = 2 * SHADOWMASK_OWNER_CHANNELS;

/// What the owner table is checked against: id 17's face count and the
/// level's static spec-light count (`!is_dynamic` AlphaLights).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ShadowmaskOwnerBounds {
    pub face_count: u32,
    pub static_spec_light_count: u32,
}

impl ShadowmaskOwnerBounds {
    /// Bounds taken from the section's own header: the face count comes from
    /// the header and the light count is the empty sentinel, so every
    /// non-sentinel owner index passes. That checks only the table's shape
    /// (length, ascending order, no duplicates, empties trailing), never
    /// whether an owner names a real light. The residency dry run uses it
    /// because it measures bytes and never resolves owners to lights.
    pub fn from_section_header(fixed_header: &[u8]) -> crate::Result<Self> {
        let (face_count, _) = owner_table(fixed_header)?;
        Ok(Self {
            face_count,
            static_spec_light_count: u32::from(SHADOWMASK_OWNER_EMPTY),
        })
    }
}

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
/// Rejects an owner table that cannot fit the `section_len`-byte section beside
/// the block count, id 22's block count of records, and each block's two
/// group planes. A bogus `face_count` then never makes a reader fetch more
/// than the index.
///
/// The bound is sized from id 22, so an id 42 with fewer blocks or smaller
/// group planes fails it too. `block_count` is id 42's count when the reader
/// already holds it (see [`shadowmask_block_count_offset`]); a count that
/// differs from id 22's is then named in the error.
pub fn shadowmask_prefix_len_through_block_count(
    fixed_header: &[u8],
    section_len: u64,
    lightmap: &LightmapBlockIndex,
    block_count: Option<u32>,
) -> crate::Result<usize> {
    let (faces, table) = owner_table(fixed_header)?;
    let room = section_len.saturating_sub(min_bytes_beside_the_owner_table(lightmap)?);
    if table <= room {
        return Ok(FIXED_HEADER_BYTES + table as usize + 4);
    }
    let lightmap_blocks = lightmap.records.len();
    Err(invalid(match block_count {
        Some(count) if count as usize != lightmap_blocks => format!(
            "shadowmask block count {count} does not match the lightmap's {lightmap_blocks}, \
             or its {table}-byte owner table (face_count {faces}) is too large \
             for the {section_len}-byte section"
        ),
        known => format!(
            "shadowmask section of {section_len} bytes cannot hold a {table}-byte owner table \
             (face_count {faces}) beside the records and group planes of id 22's \
             {lightmap_blocks} blocks: owner table too large, or the {} disagree with id 22",
            if known.is_some() {
                "group planes"
            } else {
                "block count or group planes"
            }
        ),
    }))
}

/// Where id 42's block count sits, past the fixed header and owner table, when
/// the section holds all four of its bytes. A reader whose owner-table bound
/// failed can fetch the count alone to diagnose it.
pub fn shadowmask_block_count_offset(fixed_header: &[u8], section_len: u64) -> Option<u64> {
    let (_, table) = owner_table(fixed_header).ok()?;
    let offset = FIXED_HEADER_BYTES as u64 + table;
    (offset + 4 <= section_len).then_some(offset)
}

/// The fixed header's `face_count` and its owner table's length, once the
/// format tag checks out.
fn owner_table(fixed_header: &[u8]) -> crate::Result<(u32, u64)> {
    if fixed_header.len() < FIXED_HEADER_BYTES {
        return Err(eof("shadowmask section too short for header"));
    }
    check_format_tag(read_u32(fixed_header, 0))?;
    let faces = read_u32(fixed_header, 4);
    Ok((faces, u64::from(faces) * OWNER_ENTRY_BYTES as u64))
}

/// Fixed header, block count, and id 22's block count of records and group
/// plane pairs: every byte of a valid section except its owner table.
fn min_bytes_beside_the_owner_table(lightmap: &LightmapBlockIndex) -> crate::Result<u64> {
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

/// Rejects an owner at or past `static_spec_light_count`, a duplicate owner,
/// owners out of ascending order, and an owner after an empty channel.
pub fn validate_face_owners(
    face: usize,
    owners: &FaceOwners,
    static_spec_light_count: u32,
) -> crate::Result<()> {
    let mut previous: Option<u16> = None;
    let mut ended = false;
    for &owner in owners {
        if owner == SHADOWMASK_OWNER_EMPTY {
            ended = true;
            continue;
        }
        if ended {
            return Err(invalid(format!(
                "shadowmask face {face} has owner {owner} after an empty channel"
            )));
        }
        if u32::from(owner) >= static_spec_light_count {
            return Err(invalid(format!(
                "shadowmask face {face} owner {owner} is not below the static spec-light count \
                 {static_spec_light_count}"
            )));
        }
        match previous {
            Some(p) if p == owner => {
                return Err(invalid(format!(
                    "shadowmask face {face} lists owner {owner} twice"
                )));
            }
            Some(p) if p > owner => {
                return Err(invalid(format!(
                    "shadowmask face {face} owners {owners:?} are not ascending"
                )));
            }
            _ => {}
        }
        previous = Some(owner);
    }
    Ok(())
}

/// One block's shadowmask ranges, in id-22 block order.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ShadowmaskBlockRecord {
    pub group_a: SectionByteRange,
    pub group_b: SectionByteRange,
}

/// Owner table plus every block record: what a loaded level keeps. The owner
/// table loads whole at install and never streams.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ShadowmaskBlockIndex {
    /// One entry per id-17 face, in face order.
    pub owners: Vec<FaceOwners>,
    pub records: Vec<ShadowmaskBlockRecord>,
}

impl ShadowmaskBlockIndex {
    /// Bytes of header, owner table, block count and records.
    pub fn index_byte_len(&self) -> usize {
        index_len(self.owners.len(), self.records.len())
    }

    /// Parse and validate the index against the section length, the id-22
    /// index it pairs with, and the level's owner bounds. Rejects an unknown or
    /// retired format tag; a face count that differs from id 17's; an owner
    /// table too large for the section; an invalid owner entry
    /// ([`validate_face_owners`]); a block count that differs from id 22's; a
    /// group blob whose length disagrees with its lightmap block's extent; a
    /// blob range outside the section blobs; a group B blob that does not start
    /// where group A ends; a block whose blobs start before the previous
    /// block's end; a nonzero reserved field; and a section whose length
    /// differs from where its last blob ends.
    pub fn from_prefix(
        prefix: &[u8],
        section_len: u64,
        lightmap: &LightmapBlockIndex,
        bounds: ShadowmaskOwnerBounds,
    ) -> crate::Result<Self> {
        let held_count = shadowmask_block_count_offset(prefix, section_len)
            .and_then(|at| prefix.get(at as usize..at as usize + 4))
            .map(|count| read_u32(count, 0));
        let (faces, _) = owner_table(prefix)?;
        if faces != bounds.face_count {
            return Err(invalid(format!(
                "shadowmask face count {faces} does not match id 17's {}",
                bounds.face_count
            )));
        }
        let through_count =
            shadowmask_prefix_len_through_block_count(prefix, section_len, lightmap, held_count)?;
        if prefix.len() < through_count {
            return Err(eof("shadowmask section truncated in owner table"));
        }
        let owners: Vec<FaceOwners> = prefix[FIXED_HEADER_BYTES..through_count - 4]
            .as_chunks::<OWNER_ENTRY_BYTES>()
            .0
            .iter()
            .map(|entry| {
                std::array::from_fn(|s| u16::from_le_bytes([entry[2 * s], entry[2 * s + 1]]))
            })
            .collect();
        for (face, entry) in owners.iter().enumerate() {
            validate_face_owners(face, entry, bounds.static_spec_light_count)?;
        }
        let block_count = read_u32(prefix, through_count - 4) as usize;
        if block_count != lightmap.records.len() {
            return Err(invalid(format!(
                "shadowmask block count {block_count} does not match the lightmap's {}",
                lightmap.records.len()
            )));
        }
        let index_len = index_len(owners.len(), block_count);
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
        let mut blobs_end = index_len;
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
            blobs_end = check_after_previous_block(
                "shadowmask",
                block,
                ("group A", record.group_a),
                record.group_b,
                blobs_end,
            )?;
            records.push(record);
        }
        if blobs_end != section_len {
            return Err(invalid(format!(
                "shadowmask section is {section_len} bytes, but its index and group planes end \
                 at {blobs_end}"
            )));
        }
        Ok(Self { owners, records })
    }
}

/// A whole id-42 section in memory. Block `i` pairs with id-22 block `i` and
/// shares its extent; the pair installs together or not at all.
///
/// On-disk layout (little-endian; offsets from the payload start):
///
/// ```text
///   u32 format                (= SHADOWMASK_FORMAT_OWNED_BC5_RG_BLOCKS, "SMB7")
///   u32 face_count            (= id 17's face count)
///   face_count entries of 4 × u16 owners, in id-17 face order
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
    /// One entry per id-17 face, in face order.
    pub owners: Vec<FaceOwners>,
    /// Per block: group A then group B BC5 planes.
    pub blocks: Vec<[Vec<u8>; 2]>,
}

impl ShadowmaskAtlasSection {
    /// The index this section serializes with.
    pub fn index(&self) -> ShadowmaskBlockIndex {
        let mut cursor = index_len(self.owners.len(), self.blocks.len()) as u64;
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
            owners: self.owners.clone(),
            records,
        }
    }

    pub fn byte_len(&self) -> usize {
        index_len(self.owners.len(), self.blocks.len())
            + self
                .blocks
                .iter()
                .map(|[a, b]| a.len() + b.len())
                .sum::<usize>()
    }

    /// Header, owner table, block count and records: everything before the
    /// blobs. A streamed writer can emit this first, then each block's groups.
    pub fn index_bytes(&self) -> Vec<u8> {
        let index = self.index();
        let mut out = Vec::with_capacity(index.index_byte_len());
        out.extend_from_slice(&SHADOWMASK_FORMAT_OWNED_BC5_RG_BLOCKS.to_le_bytes());
        out.extend_from_slice(&(self.owners.len() as u32).to_le_bytes());
        for owner in self.owners.iter().flatten() {
            out.extend_from_slice(&owner.to_le_bytes());
        }
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

    /// Stream the index followed by each block's two encoded group planes.
    pub fn write_to(&self, writer: &mut (impl std::io::Write + ?Sized)) -> std::io::Result<()> {
        writer.write_all(&self.index_bytes())?;
        for [a, b] in &self.blocks {
            writer.write_all(a)?;
            writer.write_all(b)?;
        }
        Ok(())
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

    /// Parse a whole section against the id-22 index it pairs with and the
    /// level's owner bounds.
    pub fn from_bytes(
        data: &[u8],
        lightmap: &LightmapBlockIndex,
        bounds: ShadowmaskOwnerBounds,
    ) -> crate::Result<Self> {
        let index = ShadowmaskBlockIndex::from_prefix(data, data.len() as u64, lightmap, bounds)?;
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
            owners: index.owners,
            blocks,
        })
    }
}

fn check_format_tag(format: u32) -> crate::Result<()> {
    match format {
        SHADOWMASK_FORMAT_OWNED_BC5_RG_BLOCKS => Ok(()),
        RETIRED_SHADOWMASK_FORMAT_SMB5 => Err(invalid(
            "shadowmask section uses the retired whole-layer SMB5 layout; re-bake the level",
        )),
        RETIRED_SHADOWMASK_FORMAT_SMB6 => Err(invalid(
            "shadowmask section uses the retired per-light-slot SMB6 layout; re-bake the level",
        )),
        other => Err(invalid(format!(
            "shadowmask format tag {other:#010x} is unknown"
        ))),
    }
}

fn index_len(faces: usize, block_count: usize) -> usize {
    FIXED_HEADER_BYTES + faces * OWNER_ENTRY_BYTES + 4 + block_count * SHADOWMASK_BLOCK_RECORD_BYTES
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::SectionId;
    use crate::lightmap::{IRRADIANCE_FORMAT_BC6H, LightmapBlock, LightmapMode, LightmapSection};

    const E: u16 = SHADOWMASK_OWNER_EMPTY;

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

    fn section_for(lightmap: &LightmapSection, owners: Vec<FaceOwners>) -> ShadowmaskAtlasSection {
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
        ShadowmaskAtlasSection { owners, blocks }
    }

    fn bounds(face_count: usize) -> ShadowmaskOwnerBounds {
        ShadowmaskOwnerBounds {
            face_count: face_count as u32,
            static_spec_light_count: 10,
        }
    }

    fn error_message(result: crate::Result<impl std::fmt::Debug>) -> String {
        format!("{:?}", result.expect_err("expected a rejection"))
    }

    fn parse(
        bytes: &[u8],
        lm: &LightmapSection,
        faces: usize,
    ) -> crate::Result<ShadowmaskAtlasSection> {
        ShadowmaskAtlasSection::from_bytes(bytes, &lm.index(), bounds(faces))
    }

    #[test]
    fn shadowmask_blocks_round_trip_with_empty_partial_and_full_owner_entries() {
        let lm = lightmap(&[(8, 4), (4, 12)]);
        for owners in [
            vec![],
            vec![NO_FACE_OWNERS],
            vec![[2, E, E, E], [0, 1, 2, 9], NO_FACE_OWNERS],
        ] {
            let faces = owners.len();
            let section = section_for(&lm, owners);
            let bytes = section.to_bytes();
            assert_eq!(bytes.len(), section.byte_len());
            assert_eq!(parse(&bytes, &lm, faces).unwrap(), section);
        }
    }

    #[test]
    fn owner_entry_is_four_little_endian_u16_in_channel_order() {
        let lm = lightmap(&[(8, 4)]);
        let bytes = section_for(&lm, vec![[3, 0x0102, E, E]]).to_bytes();
        assert_eq!(&bytes[0..4], b"SMB7");
        assert_eq!(read_u32(&bytes, 4), 1, "face count follows the format tag");
        assert_eq!(&bytes[8..16], &[3, 0, 2, 1, 0xFF, 0xFF, 0xFF, 0xFF]);
        assert_eq!(
            read_u32(&bytes, 16),
            1,
            "block count follows the owner table"
        );
    }

    #[test]
    fn index_parses_in_steps_from_the_prefix() {
        let lm = lightmap(&[(8, 4), (4, 12)]);
        let section = section_for(&lm, vec![[0, 3, E, E], [1, E, E, E], NO_FACE_OWNERS]);
        let bytes = section.to_bytes();
        let (section_len, lm_index) = (bytes.len() as u64, lm.index());
        let through =
            shadowmask_prefix_len_through_block_count(&bytes[..8], section_len, &lm_index, None)
                .unwrap();
        assert_eq!(through, 8 + 3 * 8 + 4);
        assert_eq!(
            shadowmask_block_count_offset(&bytes[..8], section_len),
            Some((8 + 3 * 8) as u64)
        );
        let index_len = section.index().index_byte_len();
        assert_eq!(index_len, through + 2 * SHADOWMASK_BLOCK_RECORD_BYTES);
        let index = ShadowmaskBlockIndex::from_prefix(
            &bytes[..index_len],
            bytes.len() as u64,
            &lm.index(),
            bounds(3),
        )
        .unwrap();
        assert_eq!(index, section.index());
    }

    #[test]
    fn rejects_an_owner_at_or_past_the_static_spec_light_count() {
        let lm = lightmap(&[(8, 4)]);
        let bytes = section_for(&lm, vec![NO_FACE_OWNERS, [4, 10, E, E]]).to_bytes();
        let message = error_message(parse(&bytes, &lm, 2));
        assert!(
            message
                .contains("shadowmask face 1 owner 10 is not below the static spec-light count 10"),
            "{message}"
        );
        let below = section_for(&lm, vec![NO_FACE_OWNERS, [4, 9, E, E]]).to_bytes();
        parse(&below, &lm, 2).expect("an owner below the count is valid");
    }

    #[test]
    fn rejects_a_duplicate_owner_within_a_face() {
        let lm = lightmap(&[(8, 4)]);
        let bytes = section_for(&lm, vec![[2, 2, E, E]]).to_bytes();
        let message = error_message(parse(&bytes, &lm, 1));
        assert!(message.contains("face 0 lists owner 2 twice"), "{message}");
    }

    #[test]
    fn rejects_owners_out_of_order_or_after_an_empty_channel() {
        let lm = lightmap(&[(8, 4)]);
        let descending = section_for(&lm, vec![[5, 3, E, E]]).to_bytes();
        let message = error_message(parse(&descending, &lm, 1));
        assert!(message.contains("are not ascending"), "{message}");
        let gap = section_for(&lm, vec![[1, E, 3, E]]).to_bytes();
        let message = error_message(parse(&gap, &lm, 1));
        assert!(
            message.contains("face 0 has owner 3 after an empty channel"),
            "{message}"
        );
    }

    #[test]
    fn rejects_a_face_count_that_disagrees_with_id_17() {
        let lm = lightmap(&[(8, 4)]);
        let bytes = section_for(&lm, vec![NO_FACE_OWNERS; 3]).to_bytes();
        let message = error_message(parse(&bytes, &lm, 4));
        assert!(
            message.contains("shadowmask face count 3 does not match id 17's 4"),
            "{message}"
        );
    }

    #[test]
    fn rejects_a_payload_length_that_disagrees_with_the_block_arithmetic() {
        let lm = lightmap(&[(8, 4)]);
        let mut bytes = section_for(&lm, vec![NO_FACE_OWNERS]).to_bytes();
        bytes.push(0);
        let message = error_message(parse(&bytes, &lm, 1));
        assert!(
            message.contains("but its index and group planes end at"),
            "{message}"
        );
        bytes.truncate(bytes.len() - 2);
        let message = error_message(parse(&bytes, &lm, 1));
        assert!(message.contains("owner table too large"), "{message}");
    }

    #[test]
    fn rejects_retired_smb5_and_smb6_tags_with_a_rebake_hint_and_unknown_tags() {
        let lm = lightmap(&[(8, 4)]);
        let mut bytes = section_for(&lm, vec![NO_FACE_OWNERS]).to_bytes();
        for (tag, layout) in [
            (b"SMB5", "whole-layer SMB5"),
            (b"SMB6", "per-light-slot SMB6"),
        ] {
            bytes[0..4].copy_from_slice(tag);
            let message = error_message(parse(&bytes, &lm, 1));
            assert!(
                message.contains(layout) && message.contains("re-bake"),
                "{message}"
            );
        }
        bytes[0..4].copy_from_slice(b"XXXX");
        let message = error_message(parse(&bytes, &lm, 1));
        assert!(message.contains("unknown"), "{message}");
    }

    #[test]
    fn rejects_block_count_mismatch_with_the_lightmap() {
        let lm = lightmap(&[(8, 4), (4, 12)]);
        let bytes = section_for(&lm, vec![NO_FACE_OWNERS]).to_bytes();
        let fewer = lightmap(&[(8, 4)]);
        let message = error_message(parse(&bytes, &fewer, 1));
        assert!(message.contains("does not match the lightmap"), "{message}");
    }

    #[test]
    fn rejects_fewer_blocks_than_the_lightmap_as_a_count_mismatch() {
        let bytes = section_for(&lightmap(&[(8, 4)]), vec![NO_FACE_OWNERS]).to_bytes();
        let more = lightmap(&[(8, 4), (4, 12)]);
        let message = error_message(parse(&bytes, &more, 1));
        assert!(
            message.contains("shadowmask block count 1 does not match the lightmap's 2"),
            "{message}"
        );
        let message = error_message(shadowmask_prefix_len_through_block_count(
            &bytes[..8],
            bytes.len() as u64,
            &more.index(),
            None,
        ));
        assert!(
            message.contains("the block count or group planes disagree with id 22"),
            "{message}"
        );
    }

    #[test]
    fn rejects_group_length_that_disagrees_with_the_lightmap_extent() {
        let lm = lightmap(&[(8, 4)]);
        let bytes = section_for(&lm, vec![NO_FACE_OWNERS]).to_bytes();
        let message = error_message(parse(&bytes, &lightmap(&[(4, 4)]), 1));
        assert!(message.contains("group A blob"), "{message}");
        let message = error_message(parse(&bytes, &lightmap(&[(12, 4)]), 1));
        assert!(
            message.contains("owner table too large, or the group planes disagree with id 22"),
            "{message}"
        );
    }

    #[test]
    fn rejects_group_blobs_that_overlap_the_previous_block() {
        let lm = lightmap(&[(8, 4), (4, 12)]);
        let section = section_for(&lm, vec![NO_FACE_OWNERS]);
        let mut bytes = section.to_bytes();
        let first = section.index().records[0];
        let at = 8 + 8 + 4 + SHADOWMASK_BLOCK_RECORD_BYTES;
        let group_len = u64::from(section.index().records[1].group_a.len);
        bytes[at..at + 8].copy_from_slice(&first.group_a.offset.to_le_bytes());
        bytes[at + 12..at + 20].copy_from_slice(&(first.group_a.offset + group_len).to_le_bytes());
        let message = error_message(parse(&bytes, &lm, 1));
        let expected = format!(
            "shadowmask block 1 group A blob at {} starts before the previous block's blobs end at {}",
            first.group_a.offset,
            first.group_b.end().unwrap()
        );
        assert!(message.contains(&expected), "{message}");
    }

    #[test]
    fn rejects_group_b_that_does_not_start_where_group_a_ends() {
        let lm = lightmap(&[(8, 4), (4, 12)]);
        let mut bytes = section_for(&lm, vec![NO_FACE_OWNERS]).to_bytes();
        let at = 8 + 8 + 4 + 12;
        let offset = read_u64(&bytes, at) + 1;
        bytes[at..at + 8].copy_from_slice(&offset.to_le_bytes());
        let message = error_message(parse(&bytes, &lm, 1));
        let expected = format!(
            "shadowmask block 0 group B blob at {offset} does not start where its group A blob ends"
        );
        assert!(message.contains(&expected), "{message}");
    }

    #[test]
    fn rejects_an_owner_table_that_cannot_fit_beside_the_block_records_and_planes() {
        let lm = lightmap(&[(8, 4)]);
        let index = lm.index();
        let mut bytes = section_for(&lm, vec![NO_FACE_OWNERS]).to_bytes();
        let section_len = bytes.len() as u64;
        assert_eq!(
            shadowmask_prefix_len_through_block_count(&bytes[..8], section_len, &index, None)
                .unwrap(),
            8 + 8 + 4
        );
        bytes[4..8].copy_from_slice(&2u32.to_le_bytes());
        let message = error_message(shadowmask_prefix_len_through_block_count(
            &bytes[..8],
            section_len,
            &index,
            None,
        ));
        assert!(
            message.contains("cannot hold a 16-byte owner table"),
            "{message}"
        );
        assert!(message.contains("owner table too large"), "{message}");
    }

    #[test]
    fn rejects_nonzero_reserved_and_out_of_section_range() {
        let lm = lightmap(&[(8, 4)]);
        let good = section_for(&lm, vec![NO_FACE_OWNERS]).to_bytes();
        let records = 8 + 8 + 4;
        let mut reserved = good.clone();
        reserved[records + 24..records + 28].copy_from_slice(&1u32.to_le_bytes());
        assert!(error_message(parse(&reserved, &lm, 1)).contains("reserved"));

        let mut outside = good.clone();
        outside[records + 12..records + 20].copy_from_slice(&(good.len() as u64).to_le_bytes());
        assert!(error_message(parse(&outside, &lm, 1)).contains("outside the section"));
    }

    #[test]
    fn section_id_is_pinned() {
        assert_eq!(SectionId::ShadowmaskAtlas as u32, 42);
        assert_eq!(SectionId::from_u32(42), Some(SectionId::ShadowmaskAtlas));
    }
}
