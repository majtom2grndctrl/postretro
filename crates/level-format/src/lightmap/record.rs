// Id 22's v4 block record: a block's two stored plane ranges, their codec and
// raw lengths, read, written and validated.
// See: context/lib/build_pipeline.md §PRL section IDs

use super::{
    LIGHTMAP_POOL_LAYER_EDGE, LightmapBlockRecord, LightmapHeader, SectionByteRange,
    check_adjacent, check_blob_range, invalid, read_u16, read_u32, read_u64,
};
use crate::coded_plane::BlockCodec;

/// One block record: cell_id u32, width u16, height u16, irradiance_offset
/// u64, irradiance_stored_len u32, direction_offset u64, direction_stored_len
/// u32, codec u32, irradiance_raw_len u32, direction_raw_len u32, reserved u32.
pub const LIGHTMAP_BLOCK_RECORD_BYTES: usize = 48;

/// Reads the record at `at`. Refuses an unknown codec and a nonzero reserved
/// field; [`validate_record`] checks the rest.
pub(super) fn read_record(
    data: &[u8],
    at: usize,
    block: usize,
) -> crate::Result<LightmapBlockRecord> {
    let raw_codec = read_u32(data, at + 32);
    let codec = BlockCodec::from_u32(raw_codec).ok_or_else(|| {
        invalid(format!(
            "lightmap block {block} codec {raw_codec} is unknown; this build reads 0 (stored raw) \
             and 1 (zstd)"
        ))
    })?;
    let reserved = read_u32(data, at + 44);
    if reserved != 0 {
        return Err(invalid(format!(
            "lightmap block {block} has nonzero reserved field {reserved:#x}"
        )));
    }
    Ok(LightmapBlockRecord {
        cell_id: read_u32(data, at),
        width: read_u16(data, at + 4),
        height: read_u16(data, at + 6),
        irradiance: SectionByteRange {
            offset: read_u64(data, at + 8),
            len: read_u32(data, at + 16),
        },
        direction: SectionByteRange {
            offset: read_u64(data, at + 20),
            len: read_u32(data, at + 28),
        },
        codec,
        raw_len: [read_u32(data, at + 36), read_u32(data, at + 40)],
    })
}

pub(super) fn write_record(out: &mut Vec<u8>, record: &LightmapBlockRecord) {
    out.extend_from_slice(&record.cell_id.to_le_bytes());
    out.extend_from_slice(&record.width.to_le_bytes());
    out.extend_from_slice(&record.height.to_le_bytes());
    out.extend_from_slice(&record.irradiance.offset.to_le_bytes());
    out.extend_from_slice(&record.irradiance.len.to_le_bytes());
    out.extend_from_slice(&record.direction.offset.to_le_bytes());
    out.extend_from_slice(&record.direction.len.to_le_bytes());
    out.extend_from_slice(&record.codec.as_u32().to_le_bytes());
    out.extend_from_slice(&record.raw_len[0].to_le_bytes());
    out.extend_from_slice(&record.raw_len[1].to_le_bytes());
    out.extend_from_slice(&0u32.to_le_bytes());
}

/// Refuses a zero, misaligned or pool-layer-oversize extent; a raw length
/// that is not its plane's at the block's extent and format; a raw plane
/// stored at another length, or a coded plane stored empty; a stored range
/// that overlaps the index or leaves the section; and a direction plane that
/// does not start where its irradiance plane ends.
pub(super) fn validate_record(
    header: &LightmapHeader,
    block: usize,
    record: &LightmapBlockRecord,
    align: u32,
    index_len: u64,
    section_len: u64,
) -> crate::Result<()> {
    let (w, h) = (u32::from(record.width), u32::from(record.height));
    if w == 0 || h == 0 {
        return Err(invalid(format!(
            "lightmap block {block} has zero extent {w}x{h}"
        )));
    }
    if w > LIGHTMAP_POOL_LAYER_EDGE || h > LIGHTMAP_POOL_LAYER_EDGE {
        return Err(invalid(format!(
            "lightmap block {block} extent {w}x{h} exceeds the {LIGHTMAP_POOL_LAYER_EDGE}² pool layer"
        )));
    }
    if w % align != 0 || h % align != 0 {
        return Err(invalid(format!(
            "lightmap block {block} extent {w}x{h} is not a multiple of {align}"
        )));
    }
    let expected = [
        header
            .irradiance_len(w, h)
            .ok_or_else(|| invalid("lightmap irradiance length overflows"))?,
        header
            .direction_len(w, h)
            .ok_or_else(|| invalid("lightmap direction length overflows"))?,
    ];
    let planes = [
        ("irradiance", record.irradiance),
        ("direction", record.direction),
    ];
    for (plane, (what, range)) in planes.into_iter().enumerate() {
        let raw_len = record.raw_len[plane];
        if u64::from(raw_len) != expected[plane] {
            return Err(invalid(format!(
                "lightmap block {block} {what} raw length {raw_len} disagrees with its {w}x{h} \
                 extent, expected {}",
                expected[plane]
            )));
        }
        match record.codec {
            BlockCodec::Raw if range.len != raw_len => {
                return Err(invalid(format!(
                    "lightmap block {block} {what} is stored raw in {} bytes, but its raw length \
                     is {raw_len}",
                    range.len
                )));
            }
            BlockCodec::Zstd if range.len == 0 => {
                return Err(invalid(format!(
                    "lightmap block {block} {what} is coded but stores no frame"
                )));
            }
            _ => {}
        }
        check_blob_range("lightmap", block, what, range, index_len, section_len)?;
    }
    check_adjacent("lightmap", block, planes[0], planes[1])
}
