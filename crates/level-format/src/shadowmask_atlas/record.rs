// Id 42's SMB9 block record: a block's two plane ranges, their codec and raw
// lengths, and the mask block's own extents.
// Governing context: context/lib/build_pipeline.md §PRL section IDs

use super::{ShadowmaskBlockRecord, group_plane_len};
use crate::coded_plane::BlockCodec;
use crate::lightmap::{LIGHTMAP_POOL_LAYER_EDGE, SectionByteRange, invalid, read_u32, read_u64};

/// One block record: group_a_offset u64, group_a_stored_len u32,
/// group_b_offset u64, group_b_stored_len u32, codec u32, group_a_raw_len u32,
/// group_b_raw_len u32, mask_width u16, mask_height u16, plane_b_width u16,
/// plane_b_height u16. A stored length of 0 is an absent plane: offset 0, raw
/// length 0.
pub const SHADOWMASK_BLOCK_RECORD_BYTES: usize = 44;

/// BC5 block edge: every mask extent is a whole number of them.
const MASK_EXTENT_ALIGNMENT: u16 = 4;

/// A mask block's own extents, separate from its lightmap block's. Plane A
/// spans `mask`; plane B is the sub-rect at the mask block's origin holding
/// every chart that owns channel 2 or 3. `(0, 0)` for an extent with nothing
/// in it: a block with no owned face has no mask, a block whose owned faces
/// own only channels 0/1 has no plane B.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Hash)]
pub struct MaskExtents {
    pub mask: (u16, u16),
    pub plane_b: (u16, u16),
}

impl MaskExtents {
    /// A block with no mask-owning face.
    pub const NONE: Self = Self {
        mask: (0, 0),
        plane_b: (0, 0),
    };

    /// Both planes at their lightmap block's extent: the mask block is the
    /// lightmap block, as when every chart stays where the lightmap put it.
    pub fn whole_block(width: u16, height: u16) -> Self {
        Self {
            mask: (width, height),
            plane_b: (width, height),
        }
    }

    /// Plane `plane`'s extent: the mask block's for plane A (0), plane B's
    /// sub-rect for plane B (1).
    pub fn plane(&self, plane: usize) -> (u32, u32) {
        let (width, height) = if plane == 0 { self.mask } else { self.plane_b };
        (u32::from(width), u32::from(height))
    }

    /// Bytes of plane `plane` at its extent.
    pub fn plane_len(&self, plane: usize) -> u64 {
        let (width, height) = self.plane(plane);
        group_plane_len(width, height).expect("u16 extents cannot overflow a plane length")
    }
}

/// Reads the record at `at`, refusing an unknown codec; [`check_record`]
/// checks the rest.
pub(super) fn read_record(
    data: &[u8],
    at: usize,
    block: usize,
) -> crate::Result<ShadowmaskBlockRecord> {
    let u16_at = |at: usize| u16::from_le_bytes([data[at], data[at + 1]]);
    let raw_codec = read_u32(data, at + 24);
    let codec = BlockCodec::from_u32(raw_codec).ok_or_else(|| {
        invalid(format!(
            "shadowmask block {block} codec {raw_codec} is unknown; this build reads 0 (stored \
             raw) and 1 (zstd)"
        ))
    })?;
    Ok(ShadowmaskBlockRecord {
        group_a: SectionByteRange {
            offset: read_u64(data, at),
            len: read_u32(data, at + 8),
        },
        group_b: SectionByteRange {
            offset: read_u64(data, at + 12),
            len: read_u32(data, at + 20),
        },
        codec,
        raw_len: [read_u32(data, at + 28), read_u32(data, at + 32)],
        extents: MaskExtents {
            mask: (u16_at(at + 36), u16_at(at + 38)),
            plane_b: (u16_at(at + 40), u16_at(at + 42)),
        },
    })
}

pub(super) fn write_record(out: &mut Vec<u8>, record: &ShadowmaskBlockRecord) {
    out.extend_from_slice(&record.group_a.offset.to_le_bytes());
    out.extend_from_slice(&record.group_a.len.to_le_bytes());
    out.extend_from_slice(&record.group_b.offset.to_le_bytes());
    out.extend_from_slice(&record.group_b.len.to_le_bytes());
    out.extend_from_slice(&record.codec.as_u32().to_le_bytes());
    out.extend_from_slice(&record.raw_len[0].to_le_bytes());
    out.extend_from_slice(&record.raw_len[1].to_le_bytes());
    let MaskExtents { mask, plane_b } = record.extents;
    for edge in [mask.0, mask.1, plane_b.0, plane_b.1] {
        out.extend_from_slice(&edge.to_le_bytes());
    }
}

/// Refuses an extent that is not a whole number of BC5 blocks, exceeds a
/// pool layer, or is zero on one axis only; a plane B not inside the mask
/// block; a present plane with a zero extent; a raw length that is not its
/// plane's at its extent, or nonzero for an absent plane; and a raw plane
/// whose stored length differs from its raw length. The caller then checks
/// the stored ranges.
pub(super) fn check_record(block: usize, record: &ShadowmaskBlockRecord) -> crate::Result<()> {
    let extents = record.extents;
    for (what, (width, height)) in [("mask", extents.mask), ("plane B", extents.plane_b)] {
        check_extent(block, what, width, height)?;
    }
    if extents.plane_b.0 > extents.mask.0 || extents.plane_b.1 > extents.mask.1 {
        return Err(invalid(format!(
            "shadowmask block {block} plane B extent {:?} is not inside its mask extent {:?}",
            extents.plane_b, extents.mask
        )));
    }
    let ranges = [("group A", record.group_a), ("group B", record.group_b)];
    for (plane, (what, range)) in ranges.into_iter().enumerate() {
        let raw_len = record.raw_len[plane];
        if range.len == 0 {
            if raw_len != 0 {
                return Err(invalid(format!(
                    "shadowmask block {block} absent {what} plane declares raw length {raw_len}; \
                     an absent plane's raw length is 0"
                )));
            }
            continue;
        }
        let (width, height) = extents.plane(plane);
        if width == 0 {
            return Err(invalid(format!(
                "shadowmask block {block} stores a {what} plane at a zero extent"
            )));
        }
        let expected = extents.plane_len(plane);
        if u64::from(raw_len) != expected {
            return Err(invalid(format!(
                "shadowmask block {block} {what} raw length {raw_len} disagrees with its \
                 {width}x{height} extent, expected {expected}"
            )));
        }
        if record.codec == BlockCodec::Raw && range.len != raw_len {
            return Err(invalid(format!(
                "shadowmask block {block} {what} is stored raw in {} bytes, but its raw length \
                 is {raw_len}",
                range.len
            )));
        }
    }
    Ok(())
}

fn check_extent(block: usize, what: &str, width: u16, height: u16) -> crate::Result<()> {
    if (width == 0) != (height == 0) {
        return Err(invalid(format!(
            "shadowmask block {block} {what} extent {width}x{height} is zero on one axis only"
        )));
    }
    if !width.is_multiple_of(MASK_EXTENT_ALIGNMENT) || !height.is_multiple_of(MASK_EXTENT_ALIGNMENT)
    {
        return Err(invalid(format!(
            "shadowmask block {block} {what} extent {width}x{height} is not a multiple of \
             {MASK_EXTENT_ALIGNMENT}"
        )));
    }
    if u32::from(width) > LIGHTMAP_POOL_LAYER_EDGE || u32::from(height) > LIGHTMAP_POOL_LAYER_EDGE {
        return Err(invalid(format!(
            "shadowmask block {block} {what} extent {width}x{height} exceeds the \
             {LIGHTMAP_POOL_LAYER_EDGE}-texel pool layer"
        )));
    }
    Ok(())
}
