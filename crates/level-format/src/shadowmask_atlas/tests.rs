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
            let len = group_plane_len(u32::from(b.width), u32::from(b.height)).unwrap() as usize;
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
    assert_eq!(&bytes[0..4], b"SMB8");
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
        message.contains("shadowmask face 1 owner 10 is not below the static spec-light count 10"),
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
    assert!(message.contains("outside the section blobs"), "{message}");
}

#[test]
fn rejects_retired_smb7_with_a_rebake_hint() {
    let lm = lightmap(&[(8, 4)]);
    let mut bytes = section_for(&lm, vec![NO_FACE_OWNERS]).to_bytes();
    bytes[0..4].copy_from_slice(b"SMB7");
    let message = error_message(parse(&bytes, &lm, 1));
    assert!(
        message.contains("retired every-plane-stored SMB7") && message.contains("re-bake"),
        "{message}"
    );
    // A streaming reader meets the tag at the fixed header, before any record.
    let message = error_message(ShadowmaskOwnerBounds::from_section_header(&bytes[..8]));
    assert!(
        message.contains("SMB7") && message.contains("re-bake"),
        "{message}"
    );
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
    // Far too few blocks for id 22's records fails the prefix bound itself.
    let many = lightmap(&[(8, 4); 8]);
    let message = error_message(shadowmask_prefix_len_through_block_count(
        &bytes[..8],
        bytes.len() as u64,
        &many.index(),
        None,
    ));
    assert!(
        message.contains("or the block count disagrees with id 22"),
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
        message.contains("group A blob is 32 bytes, expected 48"),
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
fn rejects_an_owner_table_that_cannot_fit_beside_the_block_records() {
    let lm = lightmap(&[(8, 4)]);
    let index = lm.index();
    let mut bytes = section_for(&lm, vec![NO_FACE_OWNERS]).to_bytes();
    let section_len = bytes.len() as u64;
    assert_eq!(
        shadowmask_prefix_len_through_block_count(&bytes[..8], section_len, &index, None).unwrap(),
        8 + 8 + 4
    );
    bytes[4..8].copy_from_slice(&20u32.to_le_bytes());
    let message = error_message(shadowmask_prefix_len_through_block_count(
        &bytes[..8],
        section_len,
        &index,
        None,
    ));
    assert!(
        message.contains("cannot hold a 160-byte owner table"),
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

/// Group A at record field offset 0, group B at 12, for `block` of a section
/// with one owner entry.
fn record_at(block: usize) -> usize {
    8 + 8 + 4 + block * SHADOWMASK_BLOCK_RECORD_BYTES
}

fn with_absent(
    mut section: ShadowmaskAtlasSection,
    absent: &[(usize, usize)],
) -> ShadowmaskAtlasSection {
    for &(block, plane) in absent {
        section.blocks[block][plane] = Vec::new();
    }
    section
}

// The all-zero plane's own bytes never reach the section: this pins the wire
// shape the compiler writes for each presence combination.
#[test]
fn absent_planes_store_no_blob_in_every_presence_combination() {
    let lm = lightmap(&[(8, 4), (8, 4), (8, 4), (8, 4)]);
    // Block 0 keeps both, 1 misses A, 2 misses B, 3 misses both.
    let section = with_absent(
        section_for(&lm, vec![NO_FACE_OWNERS]),
        &[(1, 0), (2, 1), (3, 0), (3, 1)],
    );
    let bytes = section.to_bytes();
    let plane = group_plane_len(8, 4).unwrap();
    let index = section.index();
    let start = index.index_byte_len() as u64;
    assert_eq!(bytes.len() as u64, start + 4 * plane, "four stored planes");
    assert_eq!(bytes.len(), section.byte_len());
    let range = |offset, len| SectionByteRange { offset, len };
    let (stored, absent) = (plane as u32, range(0, 0));
    assert_eq!(
        index.records,
        [
            ShadowmaskBlockRecord {
                group_a: range(start, stored),
                group_b: range(start + plane, stored),
            },
            ShadowmaskBlockRecord {
                group_a: absent,
                group_b: range(start + 2 * plane, stored),
            },
            ShadowmaskBlockRecord {
                group_a: range(start + 3 * plane, stored),
                group_b: absent,
            },
            ShadowmaskBlockRecord {
                group_a: absent,
                group_b: absent,
            },
        ]
    );
    assert_eq!(parse(&bytes, &lm, 1).unwrap(), section);
    let mut streamed = Vec::new();
    section.write_to(&mut streamed).unwrap();
    assert_eq!(streamed, bytes);
}

#[test]
fn a_section_of_only_absent_planes_ends_at_its_index() {
    let lm = lightmap(&[(8, 4), (4, 12)]);
    let section = with_absent(
        section_for(&lm, vec![NO_FACE_OWNERS]),
        &[(0, 0), (0, 1), (1, 0), (1, 1)],
    );
    let bytes = section.to_bytes();
    assert_eq!(bytes.len(), section.index().index_byte_len());
    assert_eq!(parse(&bytes, &lm, 1).unwrap(), section);
}

#[test]
fn stored_planes_bound_each_blocks_one_read() {
    let lm = lightmap(&[(8, 4), (8, 4), (8, 4), (8, 4)]);
    let index = with_absent(
        section_for(&lm, vec![NO_FACE_OWNERS]),
        &[(1, 0), (2, 1), (3, 0), (3, 1)],
    )
    .index();
    let [both, only_b, only_a, none] = index.records[..] else {
        panic!("four records");
    };
    assert_eq!(both.stored_planes(), Some((both.group_a, both.group_b)));
    assert_eq!(
        only_b.stored_planes(),
        Some((only_b.group_b, only_b.group_b))
    );
    assert_eq!(
        only_a.stored_planes(),
        Some((only_a.group_a, only_a.group_a))
    );
    assert_eq!(none.stored_planes(), None);
}

#[test]
fn rejects_malformed_plane_records_by_name() {
    let lm = lightmap(&[(8, 4), (4, 12)]);
    let section = with_absent(section_for(&lm, vec![NO_FACE_OWNERS]), &[(0, 1)]);
    let good = section.to_bytes();
    let index = section.index();
    parse(&good, &lm, 1).expect("block 0 keeps A alone, block 1 both");
    let (a0, a1, b1) = (record_at(0), record_at(1), record_at(1) + 12);

    // A nonzero length that is not the block's.
    let mut wrong_len = good.clone();
    wrong_len[a0 + 8..a0 + 12].copy_from_slice(&16u32.to_le_bytes());
    let message = error_message(parse(&wrong_len, &lm, 1));
    assert!(
        message.contains("shadowmask block 0 group A blob is 16 bytes, expected 32"),
        "{message}"
    );

    // An absent plane that keeps an offset.
    let mut kept_offset = good.clone();
    let held = index.records[0].group_a.offset;
    kept_offset[a0 + 12..a0 + 20].copy_from_slice(&held.to_le_bytes());
    let message = error_message(parse(&kept_offset, &lm, 1));
    assert!(
        message.contains("shadowmask block 0 absent group B plane keeps offset"),
        "{message}"
    );

    // Two present planes of one block, apart.
    let mut split_pair = good.clone();
    let moved = index.records[1].group_b.offset + 1;
    split_pair[b1..b1 + 8].copy_from_slice(&moved.to_le_bytes());
    split_pair.push(0);
    let message = error_message(parse(&split_pair, &lm, 1));
    assert!(
        message.contains(&format!(
            "shadowmask block 1 group B blob at {moved} does not start where its group A blob ends"
        )),
        "{message}"
    );

    // A block's planes apart from the previous block's.
    let mut gap = good.clone();
    let after_gap = index.records[1].group_a.offset + 1;
    gap[a1..a1 + 8].copy_from_slice(&after_gap.to_le_bytes());
    gap[b1..b1 + 8].copy_from_slice(&(index.records[1].group_b.offset + 1).to_le_bytes());
    gap.push(0);
    let message = error_message(parse(&gap, &lm, 1));
    assert!(
        message.contains(&format!(
            "shadowmask block 1 group A blob at {after_gap} leaves a gap"
        )),
        "{message}"
    );
}

// With group A absent, group B alone carries the block's order: the checks
// must follow the plane that is there.
#[test]
fn plane_order_follows_the_present_plane_when_group_a_is_absent() {
    let lm = lightmap(&[(8, 4), (8, 4)]);
    let section = with_absent(section_for(&lm, vec![NO_FACE_OWNERS]), &[(1, 0)]);
    let good = section.to_bytes();
    parse(&good, &lm, 1).expect("block 1 keeps B alone");
    let b1 = record_at(1) + 12;
    let index = section.index();

    let mut overlap = good.clone();
    overlap[b1..b1 + 8].copy_from_slice(&index.records[0].group_b.offset.to_le_bytes());
    let message = error_message(parse(&overlap, &lm, 1));
    assert!(
        message.contains("shadowmask block 1 group B blob at")
            && message.contains("starts before the previous block's blobs end"),
        "{message}"
    );

    let mut trailing = good;
    trailing.push(0);
    let message = error_message(parse(&trailing, &lm, 1));
    assert!(
        message.contains("but its index and group planes end at"),
        "{message}"
    );
}

#[test]
fn section_id_is_pinned() {
    assert_eq!(SectionId::ShadowmaskAtlas as u32, 42);
    assert_eq!(SectionId::from_u32(42), Some(SectionId::ShadowmaskAtlas));
}
