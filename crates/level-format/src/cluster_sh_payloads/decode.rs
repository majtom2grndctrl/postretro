//! Positional chunk verification for id-50 worker reads.
//! See: context/lib/build_pipeline.md §PRL section IDs.

use super::*;

pub(super) fn validate_chunk_bytes(
    cluster_id: u32,
    bytes: &[u8],
    expected: &ExpectedChunk,
) -> Result<Vec<DecodedClusterShBlock>, ClusterShPayloadsError> {
    if expected.blocks.is_empty() {
        if !bytes.is_empty() {
            return invalid(format!("empty cluster {cluster_id} has chunk bytes"));
        }
        return Ok(Vec::new());
    }
    if bytes.len() < CLUSTER_SH_CHUNK_HEADER_SIZE {
        return invalid(format!(
            "cluster {cluster_id} chunk is shorter than its header"
        ));
    }
    let block_count = read_u32(bytes, 8);
    let expected_block_count = u32::try_from(expected.blocks.len())
        .map_err(|_| ClusterShPayloadsError::SizeOverflow("chunk block count exceeds u32"))?;
    if read_u32(bytes, 0) != CLUSTER_SH_PAYLOADS_VERSION
        || read_u32(bytes, 4) != cluster_id
        || block_count != expected_block_count
        || read_u32(bytes, 12) != 0
    {
        return invalid(format!("cluster {cluster_id} chunk header is invalid"));
    }
    let table_len = (CLUSTER_SH_CHUNK_HEADER_SIZE as u64)
        .checked_add(
            (expected.blocks.len() as u64)
                .checked_mul(CLUSTER_SH_BLOCK_RECORD_SIZE as u64)
                .ok_or(ClusterShPayloadsError::SizeOverflow("chunk block table"))?,
        )
        .ok_or(ClusterShPayloadsError::SizeOverflow("chunk table end"))?;
    let table_len = usize::try_from(table_len)
        .map_err(|_| ClusterShPayloadsError::SizeOverflow("chunk table exceeds usize"))?;
    if table_len > bytes.len() {
        return invalid(format!(
            "cluster {cluster_id} block table exceeds chunk bytes"
        ));
    }
    let mut next_offset = table_len as u64;
    let mut blocks = try_vec(expected.blocks.len(), "decoded chunk blocks")?;
    for (block_index, expected_block) in expected.blocks.iter().enumerate() {
        let offset = CLUSTER_SH_CHUNK_HEADER_SIZE + block_index * CLUSTER_SH_BLOCK_RECORD_SIZE;
        let section_id = read_u32(bytes, offset);
        let kind = read_u32(bytes, offset + 4);
        let element_count = read_u32(bytes, offset + 8);
        let flags = read_u32(bytes, offset + 12);
        let body_offset = read_u64(bytes, offset + 16);
        let body_len = read_u64(bytes, offset + 24);
        if section_id != expected_block.section_id
            || kind != expected_block.kind
            || element_count != expected_block.element_count
            || flags != 0
            || body_offset != next_offset
            || body_len != expected_block.byte_len
        {
            return invalid(format!(
                "cluster {cluster_id} block {block_index} disagrees with canonical layout"
            ));
        }
        let end = body_offset
            .checked_add(body_len)
            .ok_or(ClusterShPayloadsError::SizeOverflow("chunk block range"))?;
        if end
            > u64::try_from(bytes.len()).map_err(|_| {
                ClusterShPayloadsError::SizeOverflow("chunk byte length exceeds u64")
            })?
        {
            return Err(ClusterShPayloadsError::RangeOutOfBounds(format!(
                "cluster {cluster_id} block {block_index} exceeds chunk bytes"
            )));
        }
        let body = usize::try_from(body_offset)
            .map_err(|_| ClusterShPayloadsError::SizeOverflow("chunk body offset exceeds usize"))?
            ..usize::try_from(end).map_err(|_| {
                ClusterShPayloadsError::SizeOverflow("chunk body end exceeds usize")
            })?;
        match kind {
            BLOCK_KIND_PROBE_PATCHES => validate_probe_patches(&bytes[body.clone()], expected)?,
            BLOCK_KIND_ISOLATED_ATLAS => validate_isolated_atlas(
                &bytes[body.clone()],
                expected_block.element_count,
                expected_block.irradiance_format.ok_or_else(|| {
                    ClusterShPayloadsError::InvalidData(
                        "isolated atlas lacks a validated source format".into(),
                    )
                })?,
            )?,
            BLOCK_KIND_SPARSE_ROWS => {
                validate_sparse_rows(&bytes[body.clone()], section_id, expected)?
            }
            _ => unreachable!("expected blocks use only known kinds"),
        }
        blocks.push(DecodedClusterShBlock {
            section_id,
            kind,
            element_count,
            body,
        });
        next_offset = end;
    }
    if next_offset
        != u64::try_from(bytes.len())
            .map_err(|_| ClusterShPayloadsError::SizeOverflow("chunk byte length exceeds u64"))?
    {
        return invalid(format!(
            "cluster {cluster_id} has trailing bytes or a block gap"
        ));
    }
    Ok(blocks)
}

fn validate_probe_patches(
    bytes: &[u8],
    expected: &ExpectedChunk,
) -> Result<(), ClusterShPayloadsError> {
    let expected_len = u64::from(expected.dense_patch_count)
        .checked_mul(16)
        .ok_or(ClusterShPayloadsError::SizeOverflow("probe patch bytes"))?;
    if u64::try_from(bytes.len())
        .map_err(|_| ClusterShPayloadsError::SizeOverflow("probe patch body length"))?
        != expected_len
    {
        return invalid("probe patch body length is invalid".into());
    }
    for (patch, expected_patch) in expected.probe_patches.iter().enumerate() {
        let offset = patch * 16;
        let dense_index = read_u32(bytes, offset);
        let word = read_u32(bytes, offset + 4);
        let mean_distance = read_u16(bytes, offset + 8);
        let mean_sq_distance = read_u16(bytes, offset + 10);
        let reserved = read_u32(bytes, offset + 12);
        if dense_index != expected_patch.dense_index || reserved != 0 {
            return invalid("probe patches are not strictly canonical".into());
        }
        if word != expected_patch.word
            || mean_distance != expected_patch.mean_distance
            || mean_sq_distance != expected_patch.mean_sq_distance
        {
            return invalid(format!(
                "probe patch {patch} disagrees with id 34 metadata/closure"
            ));
        }
    }
    Ok(())
}

fn validate_isolated_atlas(
    bytes: &[u8],
    expected_slots: u32,
    expected_format: u32,
) -> Result<(), ClusterShPayloadsError> {
    if bytes.len() < 20 {
        return invalid("isolated atlas body is shorter than its header".into());
    }
    let format = read_u32(bytes, 0);
    let slot_count = read_u32(bytes, 4);
    let width = read_u32(bytes, 8);
    let height = read_u32(bytes, 12);
    let layers = read_u32(bytes, 16);
    if format != expected_format || slot_count != expected_slots {
        return invalid("isolated atlas format or slot count is invalid".into());
    }
    let layout = isolated_layout(slot_count)?;
    if [width, height] != [layout.atlas_width, layout.atlas_height] || layers != layout.layer_count
    {
        return invalid("isolated atlas dimensions disagree with slot layout".into());
    }
    let payload_len = isolated_atlas_payload_len(slot_count, format)?;
    let expected_len =
        20u64
            .checked_add(payload_len)
            .ok_or(ClusterShPayloadsError::SizeOverflow(
                "isolated atlas body length",
            ))?;
    if u64::try_from(bytes.len())
        .map_err(|_| ClusterShPayloadsError::SizeOverflow("isolated atlas body length"))?
        != expected_len
    {
        return invalid("isolated atlas payload length is invalid".into());
    }
    Ok(())
}

fn validate_sparse_rows(
    bytes: &[u8],
    section_id: u32,
    expected: &ExpectedChunk,
) -> Result<(), ClusterShPayloadsError> {
    if bytes.len() < 16 {
        return invalid("sparse row body is shorter than its header".into());
    }
    let sparse = expected.sparse_blocks.get(&section_id).ok_or_else(|| {
        ClusterShPayloadsError::InvalidData(format!("unexpected sparse source {section_id}"))
    })?;
    let row_count = read_u32(bytes, 0);
    let entry_count = read_u32(bytes, 4);
    let tile_f16_count = read_u32(bytes, 8);
    if read_u32(bytes, 12) != 0
        || row_count
            != u32::try_from(sparse.rows.len()).map_err(|_| {
                ClusterShPayloadsError::SizeOverflow("sparse row count exceeds u32 wire field")
            })?
    {
        return invalid("sparse row header is invalid".into());
    }
    let expected_entries = sparse.entries.len();
    let expected_tiles = usize::try_from(sparse.tile_f16_count)
        .map_err(|_| ClusterShPayloadsError::SizeOverflow("sparse f16 count exceeds usize"))?;
    if entry_count
        != u32::try_from(expected_entries).map_err(|_| {
            ClusterShPayloadsError::SizeOverflow("sparse entry count exceeds u32 wire field")
        })?
        || tile_f16_count
            != u32::try_from(expected_tiles).map_err(|_| {
                ClusterShPayloadsError::SizeOverflow("sparse f16 count exceeds u32 wire field")
            })?
    {
        return invalid("sparse row counts disagree with source metadata".into());
    }
    let row_count = usize::try_from(row_count)
        .map_err(|_| ClusterShPayloadsError::SizeOverflow("sparse row count exceeds usize"))?;
    let entry_count = usize::try_from(entry_count)
        .map_err(|_| ClusterShPayloadsError::SizeOverflow("sparse entry count exceeds usize"))?;
    let tile_f16_count = usize::try_from(tile_f16_count)
        .map_err(|_| ClusterShPayloadsError::SizeOverflow("sparse f16 count exceeds usize"))?;
    let expected_len = sparse_block_len(row_count, entry_count, tile_f16_count)?;
    if u64::try_from(bytes.len())
        .map_err(|_| ClusterShPayloadsError::SizeOverflow("sparse row body length exceeds u64"))?
        != expected_len
    {
        return invalid("sparse row body length is invalid".into());
    }
    let mut tile_cursor = 0u32;
    for (row_index, expected_row) in sparse.rows.iter().enumerate() {
        let offset = 16 + row_index * 16;
        let source_row = read_u32(bytes, offset);
        let first_entry = read_u32(bytes, offset + 4);
        let count = read_u32(bytes, offset + 8);
        let role = read_u32(bytes, offset + 12);
        if source_row != expected_row.global_affinity_index
            || first_entry != expected_row.first_entry
            || count != expected_row.entry_count
            || role != expected_row.role
        {
            return invalid("sparse row record disagrees with id 49/source metadata".into());
        }
    }
    let entries_base = 16 + row_count * 16;
    for entry in 0..entry_count {
        let offset = entries_base + entry * 16;
        let light = read_u32(bytes, offset);
        let first_tile = read_u32(bytes, offset + 4);
        let tile_count = read_u32(bytes, offset + 8);
        if read_u32(bytes, offset + 12) != 0 || first_tile != tile_cursor {
            return invalid("sparse entry offsets are invalid".into());
        }
        let expected_entry = &sparse.entries[entry];
        if light != expected_entry.light
            || first_tile != expected_entry.first_tile_f16
            || tile_count != expected_entry.tile_f16_count
        {
            return invalid("sparse entry disagrees with its source CSR namespace".into());
        }
        tile_cursor = tile_cursor
            .checked_add(tile_count)
            .ok_or(ClusterShPayloadsError::SizeOverflow("sparse tile cursor"))?;
    }
    if usize::try_from(tile_cursor)
        .map_err(|_| ClusterShPayloadsError::SizeOverflow("sparse tile cursor exceeds usize"))?
        != tile_f16_count
    {
        return invalid("sparse tile ranges do not consume the payload".into());
    }
    Ok(())
}
