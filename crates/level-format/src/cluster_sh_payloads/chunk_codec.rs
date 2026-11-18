//! Id-50 chunk storage: a chunk's 16-byte header stays plain and names the
//! codec in its word at offset 12; the block table and bodies after it are
//! stored raw or as one checksummed zstd frame.
//! See: context/lib/build_pipeline.md §PRL section IDs

use super::*;
use crate::coded_plane::{BlockCodec, check_stored_plane, decode_plane_into, store_plane};

/// Header word holding the chunk's codec. In the decoded (raw) form it is 0.
const CODEC_WORD: usize = 12;

/// The stored form of one raw-form chunk under `codec`. A raw-form chunk is
/// what the decoder hands out: its codec word is 0. An empty chunk (a
/// canonical empty cluster) stays empty.
pub fn store_chunk(codec: BlockCodec, mut raw: Vec<u8>) -> Vec<u8> {
    if raw.is_empty() {
        return raw;
    }
    assert!(
        raw.len() >= CLUSTER_SH_CHUNK_HEADER_SIZE,
        "an id-50 chunk holds at least its header"
    );
    debug_assert_eq!(read_u32(&raw, CODEC_WORD), 0, "raw-form chunk");
    if codec == BlockCodec::Raw {
        return raw;
    }
    let body = raw.split_off(CLUSTER_SH_CHUNK_HEADER_SIZE);
    raw[CODEC_WORD..CODEC_WORD + 4].copy_from_slice(&codec.as_u32().to_le_bytes());
    raw.extend_from_slice(&store_plane(codec, body));
    raw
}

/// Decodes one stored, non-empty chunk to its raw form of exactly `raw_len`
/// bytes, codec word 0. A raw chunk moves through without a copy; a coded one
/// decodes into one buffer that already holds its header.
pub(super) fn unstore_chunk(
    cluster_id: u32,
    stored: Vec<u8>,
    raw_len: usize,
) -> Result<Vec<u8>, ClusterShPayloadsError> {
    let failed = |reason: String| ClusterShPayloadsError::ChunkDecodeFailed { cluster_id, reason };
    if stored.len() < CLUSTER_SH_CHUNK_HEADER_SIZE || raw_len < CLUSTER_SH_CHUNK_HEADER_SIZE {
        return Err(failed(format!(
            "chunk of {} bytes (raw {raw_len}) is shorter than its header",
            stored.len()
        )));
    }
    let word = read_u32(&stored, CODEC_WORD);
    let codec =
        BlockCodec::from_u32(word).ok_or_else(|| failed(format!("codec {word} is unknown")))?;
    match codec {
        BlockCodec::Raw => {
            if stored.len() != raw_len {
                return Err(failed(format!(
                    "raw chunk is {} bytes, but its raw length is {raw_len}",
                    stored.len()
                )));
            }
            Ok(stored)
        }
        BlockCodec::Zstd => {
            let payload = &stored[CLUSTER_SH_CHUNK_HEADER_SIZE..];
            let payload_raw_len = raw_len - CLUSTER_SH_CHUNK_HEADER_SIZE;
            check_stored_plane(codec, payload, payload_raw_len).map_err(failed)?;
            let mut raw = Vec::new();
            raw.try_reserve_exact(raw_len)
                .map_err(|_| ClusterShPayloadsError::AllocationFailed("decoded chunk"))?;
            raw.resize(raw_len, 0);
            raw[..CLUSTER_SH_CHUNK_HEADER_SIZE]
                .copy_from_slice(&stored[..CLUSTER_SH_CHUNK_HEADER_SIZE]);
            raw[CODEC_WORD..CODEC_WORD + 4].copy_from_slice(&0u32.to_le_bytes());
            decode_plane_into(codec, payload, &mut raw[CLUSTER_SH_CHUNK_HEADER_SIZE..])
                .map_err(failed)?;
            Ok(raw)
        }
    }
}

/// A chunk's raw length from its index record and header, with no
/// validation plan: header, one record per block, then the block bodies the
/// index's `decoded_bytes` sums.
pub(super) fn raw_chunk_len(
    stored: &[u8],
    decoded_bytes: u64,
) -> Result<usize, ClusterShPayloadsError> {
    if stored.len() < CLUSTER_SH_CHUNK_HEADER_SIZE {
        return invalid("chunk is shorter than its header".into());
    }
    let table = u64::from(read_u32(stored, 8))
        .checked_mul(CLUSTER_SH_BLOCK_RECORD_SIZE as u64)
        .and_then(|table| table.checked_add(CLUSTER_SH_CHUNK_HEADER_SIZE as u64))
        .and_then(|table| table.checked_add(decoded_bytes))
        .ok_or(ClusterShPayloadsError::SizeOverflow("raw chunk length"))?;
    usize::try_from(table).map_err(|_| ClusterShPayloadsError::SizeOverflow("raw chunk length"))
}
