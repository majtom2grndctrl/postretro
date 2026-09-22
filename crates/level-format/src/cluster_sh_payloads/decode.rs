//! Positional chunk verification for id-50 worker reads.
//! See: context/plans/in-progress/sh-probe-streaming--cluster-residency/index.md

use super::*;

pub(super) fn validate_chunk_bytes(
    cluster_id: u32,
    bytes: &[u8],
    expected: &ExpectedChunk,
    inputs: ClusterShPayloadsValidationInputs<'_>,
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
            BLOCK_KIND_PROBE_PATCHES => {
                validate_probe_patches(&bytes[body.clone()], expected, inputs)?
            }
            BLOCK_KIND_ISOLATED_ATLAS => validate_isolated_atlas(
                &bytes[body.clone()],
                expected_block.element_count,
                dense_source(inputs.sources, section_id)?,
            )?,
            BLOCK_KIND_SPARSE_ROWS => validate_sparse_rows(
                cluster_id,
                &bytes[body.clone()],
                section_id,
                expected,
                inputs,
            )?,
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
    inputs: ClusterShPayloadsValidationInputs<'_>,
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
    for (patch, expected_index) in expected.dense_indices.iter().enumerate() {
        let offset = patch * 16;
        let dense_index = read_u32(bytes, offset);
        let word = read_u32(bytes, offset + 4);
        let mean_distance = read_u16(bytes, offset + 8);
        let mean_sq_distance = read_u16(bytes, offset + 10);
        let reserved = read_u32(bytes, offset + 12);
        if dense_index != *expected_index || reserved != 0 {
            return invalid("probe patches are not strictly canonical".into());
        }
        let probe = &inputs.base.probes[dense_index as usize];
        let node = node_for_probe(
            dense_index,
            inputs.base,
            affinity_dimensions(inputs.base.grid_dimensions)?,
        )?
        .ok_or_else(|| {
            ClusterShPayloadsError::InvalidData("valid probe has no stored node".into())
        })?;
        let rank = expected.node_base_ranks[&node]
            .checked_add(local_probe_slot_offset(dense_index, inputs.base)?)
            .ok_or(ClusterShPayloadsError::SizeOverflow(
                "chunk-local probe slot rank",
            ))?;
        checked_probe_slot_rank(rank)?;
        let expected_word = PROBE_INDIRECTION_VALID_BIT
            | (u32::from(probe.density_level) & PROBE_INDIRECTION_LEVEL_MASK)
            | (u32::from(probe.node_scale) << PROBE_INDIRECTION_SCALE_SHIFT)
            | (rank << PROBE_INDIRECTION_SLOT_SHIFT);
        if word != expected_word
            || mean_distance != probe.mean_distance
            || mean_sq_distance != probe.mean_sq_distance
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
    source: ClusterShPayloadsSourceMetadata<'_>,
) -> Result<(), ClusterShPayloadsError> {
    if bytes.len() < 20 {
        return invalid("isolated atlas body is shorter than its header".into());
    }
    let format = read_u32(bytes, 0);
    let slot_count = read_u32(bytes, 4);
    let width = read_u32(bytes, 8);
    let height = read_u32(bytes, 12);
    let layers = read_u32(bytes, 16);
    if format != dense_format(source)? || slot_count != expected_slots {
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
    cluster_id: u32,
    bytes: &[u8],
    section_id: u32,
    expected: &ExpectedChunk,
    inputs: ClusterShPayloadsValidationInputs<'_>,
) -> Result<(), ClusterShPayloadsError> {
    if bytes.len() < 16 {
        return invalid("sparse row body is shorter than its header".into());
    }
    let rows = expected.sparse_rows.get(&section_id).ok_or_else(|| {
        ClusterShPayloadsError::InvalidData(format!("unexpected sparse source {section_id}"))
    })?;
    let row_count = read_u32(bytes, 0);
    let entry_count = read_u32(bytes, 4);
    let tile_f16_count = read_u32(bytes, 8);
    if read_u32(bytes, 12) != 0
        || row_count
            != u32::try_from(rows.len()).map_err(|_| {
                ClusterShPayloadsError::SizeOverflow("sparse row count exceeds u32 wire field")
            })?
    {
        return invalid("sparse row header is invalid".into());
    }
    let source = sparse_source_from_inputs(inputs.sources, section_id)?;
    let (expected_entries, expected_tiles) = sparse_counts(
        rows,
        source.valid_probe_masks,
        source.cell_levels,
        source.affinity_offsets,
        section_id,
    )?;
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
    let mut entry_cursor = 0u32;
    let mut tile_cursor = 0u32;
    for (row_index, &row) in rows.iter().enumerate() {
        let offset = 16 + row_index * 16;
        let source_row = read_u32(bytes, offset);
        let first_entry = read_u32(bytes, offset + 4);
        let count = read_u32(bytes, offset + 8);
        let role = read_u32(bytes, offset + 12);
        let expected_role = sparse_row_role(inputs.directory, cluster_id, section_id, row)?;
        let source_count =
            source.affinity_offsets[row as usize + 1] - source.affinity_offsets[row as usize];
        if source_row != row
            || first_entry != entry_cursor
            || count != source_count
            || role != expected_role as u32
        {
            return invalid("sparse row record disagrees with id 49/source metadata".into());
        }
        entry_cursor = entry_cursor
            .checked_add(count)
            .ok_or(ClusterShPayloadsError::SizeOverflow("sparse entry cursor"))?;
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
        let (row, source_entry) =
            sparse_entry_location(rows, entry as u32, source.affinity_offsets)?;
        let expected_light = source.affinity_lights[source_entry as usize];
        let level = Level::from_u8(source.cell_levels[row as usize]).ok_or_else(|| {
            ClusterShPayloadsError::SourceMismatch("sparse level became invalid".into())
        })?;
        let expected_tile_count = stored_delta_tiles(level, source.valid_probe_masks[row as usize])
            .checked_mul(delta_probe_f16_stride(CLUSTER_SH_LOGICAL_TILE_DIMENSION))
            .ok_or(ClusterShPayloadsError::SizeOverflow(
                "sparse entry tile count",
            ))?;
        if light != expected_light
            || tile_count
                != u32::try_from(expected_tile_count).map_err(|_| {
                    ClusterShPayloadsError::SizeOverflow(
                        "sparse entry tile count exceeds u32 wire field",
                    )
                })?
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

fn sparse_entry_location(
    rows: &[u32],
    local_entry: u32,
    offsets: &[u32],
) -> Result<(u32, u32), ClusterShPayloadsError> {
    let mut cursor = 0u32;
    for &row in rows {
        let start = offsets[row as usize];
        let end = offsets[row as usize + 1];
        let count = end - start;
        if local_entry < cursor + count {
            return Ok((row, start + local_entry - cursor));
        }
        cursor = cursor
            .checked_add(count)
            .ok_or(ClusterShPayloadsError::SizeOverflow(
                "sparse entry location",
            ))?;
    }
    invalid("sparse entry exceeds its row table".into())
}
