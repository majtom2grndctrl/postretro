//! Compiler-owned chunk block assembly for the id-50 wire.
//! See: context/lib/build_pipeline.md §PRL section IDs

use postretro_level_format::cluster_sh_payloads::{
    CLUSTER_SH_BLOCK_RECORD_SIZE, CLUSTER_SH_CHUNK_HEADER_SIZE, CLUSTER_SH_PAYLOADS_VERSION,
};

pub(super) const BLOCK_KIND_PROBE_PATCHES: u32 = 0;
pub(super) const BLOCK_KIND_ISOLATED_ATLAS: u32 = 1;
pub(super) const BLOCK_KIND_SPARSE_ROWS: u32 = 3;

pub(super) struct EncodedBlock {
    pub(super) section_id: u32,
    pub(super) kind: u32,
    pub(super) element_count: u32,
    pub(super) body: Vec<u8>,
}

pub(super) struct EncodedSparse {
    pub(super) section_id: u32,
    pub(super) rows: Vec<u32>,
    pub(super) body: Vec<u8>,
}

impl EncodedSparse {
    pub(super) fn into_block(self) -> EncodedBlock {
        EncodedBlock {
            section_id: self.section_id,
            kind: BLOCK_KIND_SPARSE_ROWS,
            element_count: self.rows.len() as u32,
            body: self.body,
        }
    }
}

pub(super) fn encode_chunk_wire(
    cluster_id: u32,
    blocks: &[EncodedBlock],
) -> anyhow::Result<Vec<u8>> {
    if blocks.is_empty() {
        return Ok(Vec::new());
    }
    let table_len = CLUSTER_SH_CHUNK_HEADER_SIZE
        .checked_add(
            blocks
                .len()
                .checked_mul(CLUSTER_SH_BLOCK_RECORD_SIZE)
                .ok_or_else(|| anyhow::anyhow!("id-50 block table length overflow"))?,
        )
        .ok_or_else(|| anyhow::anyhow!("id-50 chunk table length overflow"))?;
    let body_len = blocks.iter().try_fold(0usize, |total, block| {
        total
            .checked_add(block.body.len())
            .ok_or_else(|| anyhow::anyhow!("id-50 chunk body length overflow"))
    })?;
    let mut bytes = Vec::with_capacity(table_len + body_len);
    push_u32(&mut bytes, CLUSTER_SH_PAYLOADS_VERSION);
    push_u32(&mut bytes, cluster_id);
    push_u32(&mut bytes, u32::try_from(blocks.len())?);
    push_u32(&mut bytes, 0);
    let mut offset = u64::try_from(table_len)?;
    for block in blocks {
        push_u32(&mut bytes, block.section_id);
        push_u32(&mut bytes, block.kind);
        push_u32(&mut bytes, block.element_count);
        push_u32(&mut bytes, 0);
        push_u64(&mut bytes, offset);
        push_u64(&mut bytes, u64::try_from(block.body.len())?);
        offset = offset
            .checked_add(u64::try_from(block.body.len())?)
            .ok_or_else(|| anyhow::anyhow!("id-50 chunk block offset overflow"))?;
    }
    for block in blocks {
        bytes.extend_from_slice(&block.body);
    }
    Ok(bytes)
}

pub(super) fn push_u32(bytes: &mut Vec<u8>, value: u32) {
    bytes.extend_from_slice(&value.to_le_bytes());
}

pub(super) fn push_u64(bytes: &mut Vec<u8>, value: u64) {
    bytes.extend_from_slice(&value.to_le_bytes());
}
