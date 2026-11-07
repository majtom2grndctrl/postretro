// ShadowmaskAtlas PRL section (ID 42): each lightmapped face's owner lights
// and their baked visibility masks, stored per lightmap cell block.
// Governing context: context/lib/build_pipeline.md §PRL section IDs

use crate::lightmap::{
    LightmapBlockIndex, SectionByteRange, eof, invalid, read_u32, read_u64, slice_range,
};

mod planes;

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
/// at the block's lightmap extent. A plane whose encoded bytes are all zero
/// has no blob, and reads as the zero it would have decoded to. ASCII `SMB8`.
pub const SHADOWMASK_FORMAT_OWNED_BC5_RG_BLOCKS: u32 = u32::from_le_bytes(*b"SMB8");

/// Retired layouts, named so load can say "re-bake" instead of "unknown tag":
/// SMB5 held whole bake layers; SMB6 held one map-wide slot per selected light;
/// SMB7 stored every block's two planes, all-zero ones included.
const RETIRED_SHADOWMASK_FORMAT_SMB5: u32 = u32::from_le_bytes(*b"SMB5");
const RETIRED_SHADOWMASK_FORMAT_SMB6: u32 = u32::from_le_bytes(*b"SMB6");
const RETIRED_SHADOWMASK_FORMAT_SMB7: u32 = u32::from_le_bytes(*b"SMB7");

/// Mask groups per block.
pub const SHADOWMASK_GROUP_COUNT: u32 = 2;

/// One block record: group_a_offset u64, group_a_len u32, group_b_offset u64,
/// group_b_len u32, reserved u32. A length of 0 is an absent plane, offset 0.
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
/// the block count and id 22's block count of records. A bogus `face_count`
/// then never makes a reader fetch more than the index. Every plane may be
/// absent, so the bound counts none; [`ShadowmaskBlockIndex::from_prefix`]
/// checks the planes against the records.
///
/// The bound is sized from id 22, so an id 42 with far fewer blocks can fail
/// it too. `block_count` is id 42's count when the reader already holds it
/// (see [`shadowmask_block_count_offset`]); a count that differs from id 22's
/// is then named in the error.
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
             (face_count {faces}) beside the records of id 22's {lightmap_blocks} blocks: \
             owner table too large{}",
            if known.is_some() {
                ""
            } else {
                ", or the block count disagrees with id 22"
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

/// Fixed header, block count, and id 22's block count of records: every byte
/// a section must hold besides its owner table, with every plane absent.
fn min_bytes_beside_the_owner_table(lightmap: &LightmapBlockIndex) -> crate::Result<u64> {
    (lightmap.records.len() as u64)
        .checked_mul(SHADOWMASK_BLOCK_RECORD_BYTES as u64)
        .and_then(|records| records.checked_add(FIXED_HEADER_BYTES as u64 + 4))
        .ok_or_else(|| invalid("shadowmask section size overflows"))
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

/// One block's shadowmask ranges, in id-22 block order. An absent plane's
/// range is empty, at offset 0.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ShadowmaskBlockRecord {
    pub group_a: SectionByteRange,
    pub group_b: SectionByteRange,
}

impl ShadowmaskBlockRecord {
    /// The first and last planes this block stores, the same plane when it
    /// stores one. They bound one adjacent run, a single read. `None` for a
    /// block with no plane.
    pub fn stored_planes(&self) -> Option<(SectionByteRange, SectionByteRange)> {
        match (self.group_a.len != 0, self.group_b.len != 0) {
            (true, true) => Some((self.group_a, self.group_b)),
            (true, false) => Some((self.group_a, self.group_a)),
            (false, true) => Some((self.group_b, self.group_b)),
            (false, false) => None,
        }
    }
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
    /// nonzero reserved field; an absent plane that keeps an offset; a present
    /// plane whose length disagrees with its lightmap block's extent, or whose
    /// range leaves the section blobs; present planes not adjacent in record
    /// order; and a section whose length differs from where its last blob ends.
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
            blobs_end = planes::check_block_planes(
                block,
                &record,
                planes::PlaneBounds {
                    expected_len: expected,
                    index_len,
                    section_len,
                    previous_end: blobs_end,
                },
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
///   u32 format                (= SHADOWMASK_FORMAT_OWNED_BC5_RG_BLOCKS, "SMB8")
///   u32 face_count            (= id 17's face count)
///   face_count entries of 4 × u16 owners, in id-17 face order
///   u32 block_count           (= id 22's block_count)
///   block_count records (28 bytes each), in id-22 block order:
///     u64 group_a_offset, u32 group_a_len
///     u64 group_b_offset, u32 group_b_len
///     u32 reserved (= 0)
///   Blobs, in record order: each present plane, group A before group B, each
///   starting where the one before ends, each BC5 at the block's
///   width × height. A plane of length 0 is absent: offset 0, no blob.
/// ```
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ShadowmaskAtlasSection {
    /// One entry per id-17 face, in face order.
    pub owners: Vec<FaceOwners>,
    /// Per block: group A then group B BC5 planes; empty when absent.
    pub blocks: Vec<[Vec<u8>; 2]>,
}

impl ShadowmaskAtlasSection {
    /// The index this section serializes with.
    pub fn index(&self) -> ShadowmaskBlockIndex {
        let mut cursor = index_len(self.owners.len(), self.blocks.len()) as u64;
        let records = self
            .blocks
            .iter()
            .map(|[a, b]| ShadowmaskBlockRecord {
                group_a: planes::plane_range(&mut cursor, a),
                group_b: planes::plane_range(&mut cursor, b),
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

    /// Stream the index followed by each block's present group planes.
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
        RETIRED_SHADOWMASK_FORMAT_SMB7 => Err(invalid(
            "shadowmask section uses the retired every-plane-stored SMB7 layout; re-bake the level",
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
mod tests;
