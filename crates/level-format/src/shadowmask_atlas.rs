// ShadowmaskAtlas PRL section (ID 42): each lightmapped face's owner lights
// and their baked visibility masks, stored per lightmap cell block.
// Governing context: context/lib/build_pipeline.md §PRL section IDs

use crate::coded_plane::{self, BlockCodec};
use crate::lightmap::{LightmapBlockIndex, SectionByteRange, eof, invalid, read_u32, slice_range};

mod owners;
mod planes;
mod record;

use owners::OWNER_ENTRY_BYTES;
pub use owners::{
    FaceOwners, MaskOffset, NO_FACE_OWNERS, SHADOWMASK_OWNER_CHANNELS, SHADOWMASK_OWNER_EMPTY,
    SpecOwnerVertex, validate_face_owners,
};
pub use record::{MaskExtents, SHADOWMASK_BLOCK_RECORD_BYTES};

/// Per-face owner table with each face's lightmap-to-mask texel offset, then
/// BC5 `.rg` per cell block with two mask groups: group 0 (channels 0/1) at
/// the mask block's own extent and group 1 (channels 2/3) at its plane-B
/// sub-rect, each plane stored coded. A plane whose encoded bytes are all
/// zero has no blob, and reads as the zero it would have decoded to. ASCII
/// `SMB9`.
pub const SHADOWMASK_FORMAT_OWNED_BC5_RG_BLOCKS: u32 = u32::from_le_bytes(*b"SMB9");

/// Retired layouts, named so load can say "re-bake" instead of "unknown tag":
/// SMB5 held whole bake layers; SMB6 held one map-wide slot per selected light;
/// SMB7 stored every block's two planes, all-zero ones included; SMB8 stored
/// each plane at its lightmap block's extent, in the lightmap layout.
const RETIRED_SHADOWMASK_FORMAT_SMB5: u32 = u32::from_le_bytes(*b"SMB5");
const RETIRED_SHADOWMASK_FORMAT_SMB6: u32 = u32::from_le_bytes(*b"SMB6");
const RETIRED_SHADOWMASK_FORMAT_SMB7: u32 = u32::from_le_bytes(*b"SMB7");
const RETIRED_SHADOWMASK_FORMAT_SMB8: u32 = u32::from_le_bytes(*b"SMB8");

/// Mask groups per block.
pub const SHADOWMASK_GROUP_COUNT: u32 = 2;

const BC5_BLOCK_BYTES: u64 = 16;
/// format tag + face_count.
const FIXED_HEADER_BYTES: usize = 8;

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

/// Bytes of one BC5 group plane at a `width × height` plane extent.
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

/// One block's stored shadowmask ranges, codec, raw lengths and mask extents,
/// in id-22 block order. An absent plane's range is empty, at offset 0, with
/// raw length 0.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ShadowmaskBlockRecord {
    pub group_a: SectionByteRange,
    pub group_b: SectionByteRange,
    /// How both present planes are stored.
    pub codec: BlockCodec,
    /// Group A then group B bytes once decoded: each present plane's BC5
    /// length at its extent, 0 for an absent plane.
    pub raw_len: [u32; 2],
    pub extents: MaskExtents,
}

impl ShadowmaskBlockRecord {
    /// Both planes' bytes once decoded: what the block's mask half costs in
    /// hand and to upload.
    pub fn decoded_bytes(&self) -> u64 {
        u64::from(self.raw_len[0]) + u64::from(self.raw_len[1])
    }

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
    /// Each face's lightmap-to-mask texel offset, in face order; zero for a
    /// face with no owner.
    pub mask_offsets: Vec<MaskOffset>,
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
    /// ([`validate_face_owners`]) or a mask offset on an unowned face; a block
    /// count that differs from id 22's; a record whose codec, extents or raw
    /// lengths are invalid (`record::check_record`); an absent plane that
    /// keeps an offset; a present plane whose range leaves the section blobs;
    /// present planes not adjacent in record order; and a section whose
    /// length differs from where its last blob ends.
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
        let (owners, mask_offsets): (Vec<FaceOwners>, Vec<MaskOffset>) = prefix
            [FIXED_HEADER_BYTES..through_count - 4]
            .as_chunks::<OWNER_ENTRY_BYTES>()
            .0
            .iter()
            .map(owners::read_owner_entry)
            .unzip();
        for (face, (entry, &offset)) in owners.iter().zip(&mask_offsets).enumerate() {
            validate_face_owners(face, entry, bounds.static_spec_light_count)?;
            owners::validate_mask_offset(face, entry, offset)?;
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
        for block in 0..block_count {
            let record = record::read_record(
                prefix,
                through_count + block * SHADOWMASK_BLOCK_RECORD_BYTES,
                block,
            )?;
            record::check_record(block, &record)?;
            blobs_end = planes::check_block_planes(
                block,
                &record,
                planes::PlaneBounds {
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
        Ok(Self {
            owners,
            mask_offsets,
            records,
        })
    }

    /// Decodes block `block`'s stored group A and group B planes to their BC5
    /// bytes, as the pool uploads them; an absent plane stays empty. A raw
    /// plane moves through uncopied. Fails by name on a frame that does not
    /// decode, checksum verified, to exactly its declared raw length.
    pub fn decode_planes(
        &self,
        block: usize,
        [group_a, group_b]: [Vec<u8>; 2],
    ) -> crate::Result<[Vec<u8>; 2]> {
        let record = self
            .records
            .get(block)
            .ok_or_else(|| invalid(format!("shadowmask block {block} is past the index")))?;
        let decode = |plane: usize, what: &str, stored: Vec<u8>| {
            if stored.is_empty() && record.raw_len[plane] == 0 {
                return Ok(stored);
            }
            coded_plane::decode_plane(record.codec, stored, record.raw_len[plane] as usize).map_err(
                |reason| {
                    invalid(format!(
                        "shadowmask block {block} {what} plane failed to decode: {reason}"
                    ))
                },
            )
        };
        Ok([
            decode(0, "group A", group_a)?,
            decode(1, "group B", group_b)?,
        ])
    }
}

/// A whole id-42 section in memory. Block `i` pairs with id-22 block `i`; the
/// pair installs together or not at all. Its mask block holds only the charts
/// of mask-owning faces, each a pure translation of its lightmap chart.
///
/// On-disk layout (little-endian; offsets from the payload start):
///
/// ```text
///   u32 format                (= SHADOWMASK_FORMAT_OWNED_BC5_RG_BLOCKS, "SMB9")
///   u32 face_count            (= id 17's face count)
///   face_count entries of 4 × u16 owners, then i16 dx, i16 dy (mask texel =
///     lightmap texel + delta; 0 for a face with no owner), in id-17 face order
///   u32 block_count           (= id 22's block_count)
///   block_count records (44 bytes each), in id-22 block order:
///     u64 group_a_offset, u32 group_a_stored_len
///     u64 group_b_offset, u32 group_b_stored_len
///     u32 codec (0 = stored raw, 1 = one zstd frame per plane)
///     u32 group_a_raw_len, u32 group_b_raw_len
///     u16 mask_width, u16 mask_height, u16 plane_b_width, u16 plane_b_height
///   Blobs, in record order: each present plane in its stored form, group A
///   before group B, each starting where the one before ends; group A decodes
///   to BC5 at the mask extent, group B at the plane-B extent, which lies at
///   the mask block's origin. A plane of stored length 0 is absent: offset 0,
///   raw length 0, no blob.
/// ```
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ShadowmaskAtlasSection {
    /// One entry per id-17 face, in face order.
    pub owners: Vec<FaceOwners>,
    /// Each face's lightmap-to-mask texel offset, in face order.
    pub mask_offsets: Vec<MaskOffset>,
    /// Per block: group A then group B BC5 planes in their stored form;
    /// empty when absent.
    pub blocks: Vec<[Vec<u8>; 2]>,
    /// Per block: how its planes are stored.
    pub codecs: Vec<BlockCodec>,
    /// Per block: its mask block's extents.
    pub extents: Vec<MaskExtents>,
}

impl ShadowmaskAtlasSection {
    /// A section whose mask blocks are their lightmap blocks: every plane at
    /// its block's extent, stored raw, and every offset zero. `block_extents`
    /// is id 22's, one per block.
    pub fn whole_block(
        owners: Vec<FaceOwners>,
        blocks: Vec<[Vec<u8>; 2]>,
        block_extents: impl IntoIterator<Item = (u16, u16)>,
    ) -> Self {
        let extents: Vec<MaskExtents> = block_extents
            .into_iter()
            .map(|(width, height)| MaskExtents::whole_block(width, height))
            .collect();
        assert_eq!(extents.len(), blocks.len(), "one extent per block");
        Self {
            mask_offsets: vec![[0; 2]; owners.len()],
            owners,
            codecs: vec![BlockCodec::Raw; blocks.len()],
            blocks,
            extents,
        }
    }

    /// The index this section serializes with.
    pub fn index(&self) -> ShadowmaskBlockIndex {
        let mut cursor = index_len(self.owners.len(), self.blocks.len()) as u64;
        let records = self
            .blocks
            .iter()
            .zip(&self.extents)
            .zip(&self.codecs)
            .map(|(([a, b], &extents), &codec)| {
                let raw_len = |plane: usize, stored: &[u8]| {
                    if stored.is_empty() {
                        0
                    } else {
                        u32::try_from(extents.plane_len(plane)).expect("a mask plane fits u32")
                    }
                };
                ShadowmaskBlockRecord {
                    group_a: planes::plane_range(&mut cursor, a),
                    group_b: planes::plane_range(&mut cursor, b),
                    codec,
                    raw_len: [raw_len(0, a), raw_len(1, b)],
                    extents,
                }
            })
            .collect();
        ShadowmaskBlockIndex {
            owners: self.owners.clone(),
            mask_offsets: self.mask_offsets.clone(),
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
        for (owners, &offset) in self.owners.iter().zip(&self.mask_offsets) {
            owners::write_owner_entry(&mut out, owners, offset);
        }
        out.extend_from_slice(&(self.blocks.len() as u32).to_le_bytes());
        for record in &index.records {
            record::write_record(&mut out, record);
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
    /// level's owner bounds. Planes stay in their stored form.
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
            mask_offsets: index.mask_offsets,
            codecs: index.records.iter().map(|r| r.codec).collect(),
            extents: index.records.iter().map(|r| r.extents).collect(),
            blocks,
        })
    }

    /// This section with every block's planes decoded and stored raw.
    pub fn decoded(&self) -> crate::Result<Self> {
        let blocks = (0..self.blocks.len())
            .map(|block| self.decode_block(block))
            .collect::<crate::Result<_>>()?;
        Ok(Self {
            owners: self.owners.clone(),
            mask_offsets: self.mask_offsets.clone(),
            blocks,
            codecs: vec![BlockCodec::Raw; self.codecs.len()],
            extents: self.extents.clone(),
        })
    }

    /// Block `block`'s group A and group B BC5 bytes, decoded from their
    /// stored form; an absent plane is empty.
    pub fn decode_block(&self, block: usize) -> crate::Result<[Vec<u8>; 2]> {
        let stored = self
            .blocks
            .get(block)
            .ok_or_else(|| invalid(format!("shadowmask block {block} is past the section")))?;
        let (codec, extents) = (self.codecs[block], self.extents[block]);
        let mut planes = [Vec::new(), Vec::new()];
        for (plane, (what, bytes)) in [("group A", &stored[0]), ("group B", &stored[1])]
            .into_iter()
            .enumerate()
        {
            if bytes.is_empty() {
                continue;
            }
            planes[plane] =
                coded_plane::decode_plane_from(codec, bytes, extents.plane_len(plane) as usize)
                    .map_err(|reason| {
                        invalid(format!(
                            "shadowmask block {block} {what} plane failed to decode: {reason}"
                        ))
                    })?;
        }
        Ok(planes)
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
        RETIRED_SHADOWMASK_FORMAT_SMB8 => Err(invalid(
            "shadowmask section uses the retired lightmap-layout SMB8 layout; re-bake the level",
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
