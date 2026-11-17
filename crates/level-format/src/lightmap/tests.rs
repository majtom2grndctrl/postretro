use super::*;

/// Raw planes of a `width × height` BC6H block at direction scale `scale`.
fn raw_planes(width: u16, height: u16, seed: u8, scale: u32) -> (Vec<u8>, Vec<u8>) {
    let header = LightmapHeader {
        block_count: 0,
        direction_texel_scale: scale,
        irradiance_format: IRRADIANCE_FORMAT_BC6H,
        mode: LightmapMode::Shadowed,
    };
    let (w, h) = (u32::from(width), u32::from(height));
    let irr_len = header.irradiance_len(w, h).unwrap() as usize;
    let dir_len = header.direction_len(w, h).unwrap() as usize;
    (
        (0..irr_len)
            .map(|i| seed.wrapping_add((i / 7) as u8))
            .collect(),
        (0..dir_len).map(|i| seed ^ ((i / 5) as u8)).collect(),
    )
}

fn block(cell_id: u32, width: u16, height: u16, seed: u8, scale: u32) -> LightmapBlock {
    let header = LightmapHeader {
        block_count: 0,
        direction_texel_scale: scale,
        irradiance_format: IRRADIANCE_FORMAT_BC6H,
        mode: LightmapMode::Shadowed,
    };
    let (w, h) = (u32::from(width), u32::from(height));
    let irr_len = header.irradiance_len(w, h).unwrap() as usize;
    let dir_len = header.direction_len(w, h).unwrap() as usize;
    LightmapBlock {
        cell_id,
        width,
        height,
        codec: BlockCodec::Raw,
        irradiance: (0..irr_len).map(|i| seed.wrapping_add(i as u8)).collect(),
        direction: (0..dir_len).map(|i| seed ^ (i as u8)).collect(),
    }
}

fn coded_block(cell_id: u32, width: u16, height: u16, seed: u8) -> LightmapBlock {
    let (irradiance, direction) = raw_planes(width, height, seed, 2);
    LightmapBlock::coded(
        cell_id,
        width,
        height,
        BlockCodec::Zstd,
        &irradiance,
        &direction,
    )
}

/// Two zstd-coded blocks, large enough that coding shrinks them.
fn coded_section() -> LightmapSection {
    LightmapSection {
        direction_texel_scale: 2,
        irradiance_format: IRRADIANCE_FORMAT_BC6H,
        mode: LightmapMode::Shadowed,
        blocks: vec![coded_block(3, 64, 32, 5), coded_block(4, 32, 64, 9)],
    }
}

fn two_block_section() -> LightmapSection {
    LightmapSection {
        direction_texel_scale: 2,
        irradiance_format: IRRADIANCE_FORMAT_BC6H,
        mode: LightmapMode::Unshadowed,
        blocks: vec![block(7, 8, 4, 3, 2), block(2, 4, 12, 9, 2)],
    }
}

fn error_message(result: crate::Result<impl std::fmt::Debug>) -> String {
    format!("{:?}", result.expect_err("expected a rejection"))
}

/// Byte offset of block `block`'s record field at `field` within it.
fn record_at(block: usize, field: usize) -> usize {
    LIGHTMAP_HEADER_BYTES + block * LIGHTMAP_BLOCK_RECORD_BYTES + field
}

#[test]
fn lightmap_blocks_round_trip_with_header_mode_and_scale() {
    let section = two_block_section();
    let bytes = section.to_bytes();
    assert_eq!(section.byte_len(), bytes.len());
    assert_eq!(LightmapSection::from_bytes(&bytes).unwrap(), section);
}

#[test]
fn zero_block_section_is_a_bare_header() {
    let section = LightmapSection::empty(2);
    let bytes = section.to_bytes();
    assert_eq!(bytes.len(), LIGHTMAP_HEADER_BYTES);
    let index = LightmapBlockIndex::from_prefix(&bytes, bytes.len() as u64).unwrap();
    assert!(index.records.is_empty());
    assert_eq!(LightmapSection::from_bytes(&bytes).unwrap(), section);
}

#[test]
fn index_parses_from_the_prefix_alone() {
    let section = two_block_section();
    let bytes = section.to_bytes();
    let index_len = section.header().index_byte_len().unwrap() as usize;
    let index = LightmapBlockIndex::from_prefix(&bytes[..index_len], bytes.len() as u64)
        .expect("the index needs no blob bytes");
    assert_eq!(index, section.index());
    // Blobs follow the index in record order: irradiance, then direction.
    let first = index.records[0];
    assert_eq!(first.irradiance.offset, index_len as u64);
    assert_eq!(
        first.direction.offset,
        first.irradiance.offset + u64::from(first.irradiance.len)
    );
    assert_eq!(index.extent(1), Some((4, 12)));
}

#[test]
fn block_alignment_is_lcm_of_bc_edge_and_direction_scale() {
    let mut header = two_block_section().header();
    for (scale, align) in [(1, 4), (2, 4), (4, 4), (8, 8), (3, 12)] {
        header.direction_texel_scale = scale;
        assert_eq!(header.block_alignment(), align, "scale {scale}");
    }
}

#[test]
fn rejects_older_section_versions_with_a_rebake_hint() {
    // v3 is the raw per-cell layout every level baked before coded planes
    // carries; v2 the whole-atlas one.
    for version in [2u32, 3] {
        let mut bytes = two_block_section().to_bytes();
        bytes[0..4].copy_from_slice(&version.to_le_bytes());
        let message = error_message(LightmapSection::from_bytes(&bytes));
        assert!(
            message.contains(&format!("version: {version}")),
            "{message}"
        );
        assert!(message.contains("re-bake"), "{message}");
    }
}

#[test]
fn coded_blocks_round_trip_stored_and_decode_to_their_raw_planes() {
    let section = coded_section();
    let bytes = section.to_bytes();
    assert_eq!(section.byte_len(), bytes.len());
    let parsed = LightmapSection::from_bytes(&bytes).unwrap();
    assert_eq!(parsed, section);
    let index = LightmapBlockIndex::from_prefix(&bytes, bytes.len() as u64).unwrap();
    for (block, (seed, (w, h))) in [(5, (64, 32)), (9, (32, 64))].into_iter().enumerate() {
        let raw = raw_planes(w, h, seed, 2);
        let record = index.records[block];
        assert_eq!(record.codec, BlockCodec::Zstd);
        assert_eq!(
            record.raw_len,
            [raw.0.len() as u32, raw.1.len() as u32],
            "raw lengths are the extent's"
        );
        assert!(
            record.irradiance.len < record.raw_len[0],
            "coding shrinks the plane"
        );
        assert_eq!(parsed.decode_block(block).unwrap(), raw);
        let stored = (
            slice_range(&bytes, record.irradiance).to_vec(),
            slice_range(&bytes, record.direction).to_vec(),
        );
        assert_eq!(index.decode_block(block, stored.0, stored.1).unwrap(), raw);
    }
}

#[test]
fn a_raw_block_decodes_by_moving_its_planes() {
    let section = two_block_section();
    let index = section.index();
    let block = section.blocks[0].clone();
    let pointers = (block.irradiance.as_ptr(), block.direction.as_ptr());
    let (irradiance, direction) = index
        .decode_block(0, block.irradiance, block.direction)
        .unwrap();
    assert_eq!((irradiance.as_ptr(), direction.as_ptr()), pointers);
}

#[test]
fn a_flipped_byte_in_a_coded_plane_fails_its_decode_by_name() {
    let section = coded_section();
    let index = section.index();
    let mut irradiance = section.blocks[1].irradiance.clone();
    let last = irradiance.len() - 1;
    irradiance[last] ^= 0x01;
    let message =
        error_message(index.decode_block(1, irradiance, section.blocks[1].direction.clone()));
    assert!(
        message.contains("lightmap block 1 irradiance plane failed to decode"),
        "{message}"
    );
}

#[test]
fn rejects_a_declared_raw_length_that_disagrees_with_the_extent() {
    let mut bytes = coded_section().to_bytes();
    let at = record_at(0, 40);
    let declared = read_u32(&bytes, at) + 2;
    bytes[at..at + 4].copy_from_slice(&declared.to_le_bytes());
    let message = error_message(LightmapSection::from_bytes(&bytes));
    assert!(
        message.contains(&format!(
            "lightmap block 0 direction raw length {declared} disagrees with its 64x32 extent"
        )),
        "{message}"
    );
}

#[test]
fn rejects_a_stored_length_that_breaks_adjacency() {
    // Block 0's coded irradiance claims one byte more: its direction plane
    // no longer starts where it ends.
    let mut bytes = coded_section().to_bytes();
    let at = record_at(0, 16);
    let stored = read_u32(&bytes, at) + 1;
    bytes[at..at + 4].copy_from_slice(&stored.to_le_bytes());
    let message = error_message(LightmapSection::from_bytes(&bytes));
    assert!(
        message.contains("direction blob") && message.contains("does not start where its"),
        "{message}"
    );
}

#[test]
fn rejects_an_unknown_codec_and_an_empty_coded_plane() {
    let mut bytes = coded_section().to_bytes();
    let at = record_at(1, 32);
    bytes[at..at + 4].copy_from_slice(&2u32.to_le_bytes());
    let message = error_message(LightmapSection::from_bytes(&bytes));
    assert!(
        message.contains("lightmap block 1 codec 2 is unknown"),
        "{message}"
    );

    let mut section = coded_section();
    section.blocks[0].direction.clear();
    let message = error_message(LightmapSection::from_bytes(&section.to_bytes()));
    assert!(
        message.contains("lightmap block 0 direction is coded but stores no frame"),
        "{message}"
    );
}

#[test]
fn rejects_nonzero_reserved_field() {
    let mut bytes = two_block_section().to_bytes();
    let at = record_at(1, 44);
    bytes[at..at + 4].copy_from_slice(&1u32.to_le_bytes());
    let message = error_message(LightmapSection::from_bytes(&bytes));
    assert!(message.contains("reserved"), "{message}");
}

#[test]
fn rejects_blob_range_outside_the_section() {
    let mut bytes = two_block_section().to_bytes();
    let at = record_at(1, 20);
    let past_end = bytes.len() as u64;
    bytes[at..at + 8].copy_from_slice(&past_end.to_le_bytes());
    let message = error_message(LightmapSection::from_bytes(&bytes));
    assert!(message.contains("outside the section"), "{message}");
}

#[test]
fn rejects_blob_range_overlapping_the_index() {
    let mut bytes = two_block_section().to_bytes();
    let at = record_at(0, 8);
    bytes[at..at + 8].copy_from_slice(&0u64.to_le_bytes());
    let message = error_message(LightmapSection::from_bytes(&bytes));
    assert!(message.contains("outside the section"), "{message}");
}

#[test]
fn rejects_block_larger_than_a_pool_layer() {
    let edge = LIGHTMAP_POOL_LAYER_EDGE as u16 + 4;
    let section = LightmapSection {
        blocks: vec![block(0, edge, 4, 1, 2)],
        ..two_block_section()
    };
    let message = error_message(LightmapSection::from_bytes(&section.to_bytes()));
    assert!(message.contains("pool layer"), "{message}");
}

#[test]
fn accepts_a_block_exactly_one_pool_layer() {
    let edge = LIGHTMAP_POOL_LAYER_EDGE as u16;
    let section = LightmapSection {
        blocks: vec![block(0, edge, 4, 1, 2)],
        ..two_block_section()
    };
    assert!(LightmapSection::from_bytes(&section.to_bytes()).is_ok());
}

#[test]
fn rejects_misaligned_extent() {
    let mut bytes = two_block_section().to_bytes();
    let at = record_at(0, 4);
    bytes[at..at + 2].copy_from_slice(&6u16.to_le_bytes());
    let message = error_message(LightmapSection::from_bytes(&bytes));
    assert!(message.contains("not a multiple of 4"), "{message}");
}

#[test]
fn rejects_a_raw_plane_stored_at_another_length() {
    let mut section = two_block_section();
    section.blocks[1].direction.pop();
    let message = error_message(LightmapSection::from_bytes(&section.to_bytes()));
    assert!(
        message.contains("lightmap block 1 direction is stored raw in"),
        "{message}"
    );
}

#[test]
fn rejects_block_count_past_the_vertex_id_limit() {
    let mut bytes = LightmapSection::empty(2).to_bytes();
    bytes[4..8].copy_from_slice(&(MAX_LIGHTMAP_BLOCKS + 1).to_le_bytes());
    let message = error_message(LightmapHeader::from_bytes(&bytes));
    assert!(message.contains("vertex id limit"), "{message}");
    bytes[4..8].copy_from_slice(&MAX_LIGHTMAP_BLOCKS.to_le_bytes());
    assert!(LightmapHeader::from_bytes(&bytes).is_ok());
}

#[test]
fn block_limit_is_u16_max_so_the_last_block_id_plus_one_fits_a_vertex() {
    assert_eq!(MAX_LIGHTMAP_BLOCKS, 65_535);
    let last_block_id = MAX_LIGHTMAP_BLOCKS - 1;
    assert_eq!(u16::try_from(last_block_id + 1), Ok(u16::MAX));
}

#[test]
fn rejects_direction_scale_that_is_not_a_power_of_two_dividing_the_pool_layer() {
    for scale in [3u32, 6, 12, LIGHTMAP_POOL_LAYER_EDGE * 2] {
        let mut bytes = LightmapSection::empty(2).to_bytes();
        bytes[8..12].copy_from_slice(&scale.to_le_bytes());
        let message = error_message(LightmapHeader::from_bytes(&bytes));
        assert!(
            message.contains("power of two dividing"),
            "{scale}: {message}"
        );
    }
    for scale in [1u32, 2, 8, LIGHTMAP_POOL_LAYER_EDGE] {
        let mut bytes = LightmapSection::empty(2).to_bytes();
        bytes[8..12].copy_from_slice(&scale.to_le_bytes());
        assert!(LightmapHeader::from_bytes(&bytes).is_ok(), "{scale}");
    }
}

#[test]
fn rejects_direction_blob_that_does_not_start_where_its_irradiance_ends() {
    let mut bytes = two_block_section().to_bytes();
    // Block 0's direction moved one byte on: still inside the section
    // and the right length, but no longer adjacent to its irradiance.
    let at = record_at(0, 20);
    let offset = read_u64(&bytes, at) + 1;
    bytes[at..at + 8].copy_from_slice(&offset.to_le_bytes());
    let message = error_message(LightmapSection::from_bytes(&bytes));
    let expected = format!(
        "lightmap block 0 direction blob at {offset} does not start where its irradiance blob ends"
    );
    assert!(message.contains(&expected), "{message}");
}

#[test]
fn rejects_blobs_that_overlap_or_leave_record_order() {
    let section = two_block_section();
    let index = section.index();
    let (first, second) = (index.records[0], index.records[1]);
    let first_len = u64::from(first.irradiance.len + first.direction.len);
    let second_len = u64::from(second.irradiance.len + second.direction.len);
    let start = first.irradiance.offset;
    let place = |bytes: &mut Vec<u8>, block: usize, record: &LightmapBlockRecord, at: u64| {
        let irradiance = record_at(block, 8);
        bytes[irradiance..irradiance + 8].copy_from_slice(&at.to_le_bytes());
        let direction = record_at(block, 20);
        let direction_at = at + u64::from(record.irradiance.len);
        bytes[direction..direction + 8].copy_from_slice(&direction_at.to_le_bytes());
    };

    // Block 1's pair laid over block 0's: each pair adjacent and inside
    // the section, but the two share bytes.
    let mut overlapping = section.to_bytes();
    place(&mut overlapping, 1, &second, start);
    let message = error_message(LightmapSection::from_bytes(&overlapping));
    let expected = format!(
        "lightmap block 1 irradiance blob at {start} starts before the previous block's blobs end at {}",
        start + first_len
    );
    assert!(message.contains(&expected), "{message}");

    // The same bytes with the two pairs swapped: disjoint, but block 1's
    // blobs come first.
    let mut reversed = section.to_bytes();
    place(&mut reversed, 0, &first, start + second_len);
    place(&mut reversed, 1, &second, start);
    let message = error_message(LightmapSection::from_bytes(&reversed));
    assert!(
        message.contains("disjoint and in record order"),
        "{message}"
    );
}

#[test]
fn rejects_unknown_mode_format_and_zero_scale() {
    for (at, value, expect) in [
        (16, 9u32, "mode"),
        (12, 9u32, "irradiance format"),
        (8, 0u32, "scale"),
    ] {
        let mut bytes = LightmapSection::empty(2).to_bytes();
        bytes[at..at + 4].copy_from_slice(&value.to_le_bytes());
        let message = error_message(LightmapSection::from_bytes(&bytes));
        assert!(message.contains(expect), "{expect}: {message}");
    }
}

#[test]
fn rejects_truncated_index() {
    let bytes = two_block_section().to_bytes();
    let index_len = LIGHTMAP_HEADER_BYTES + 2 * LIGHTMAP_BLOCK_RECORD_BYTES;
    assert!(LightmapBlockIndex::from_prefix(&bytes[..index_len - 1], bytes.len() as u64).is_err());
}

#[test]
fn encode_direction_axis_round_trip() {
    let enc = encode_direction_oct([0.0, 1.0, 0.0]);
    assert_eq!(enc[0], 128);
    assert_eq!(enc[1], 255);
}

#[test]
fn f32_to_f16_known_values() {
    assert_eq!(f32_to_f16_bits(0.0), 0x0000);
    assert_eq!(f32_to_f16_bits(1.0), 0x3c00);
    assert_eq!(f32_to_f16_bits(-1.0), 0xbc00);
}
