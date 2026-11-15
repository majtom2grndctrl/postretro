use super::*;
use crate::SectionId;
use crate::delta_sh_volumes::AFFINITY_FACTOR;

const ALL_VALID: u64 = u64::MAX;
const CELL1_VALID: u64 = (1 << 1) | (1 << 5) | (1 << 40);

fn record(
    namespace: MoverLightNamespace,
    light_index: u32,
    carries_own_tile: bool,
    probe_mask: u64,
) -> MoverDirectRecord {
    MoverDirectRecord {
        namespace,
        carries_own_tile,
        light_index,
        probe_mask,
    }
}

fn tiles(probes: usize, first: u16) -> Vec<u16> {
    (0..probes * MOVER_IRRADIANCE_TILE_F16_COUNT)
        .map(|i| first + i as u16)
        .collect()
}

/// Two cells along x. Cell 0 is all valid; cell 1 holds three valid probes.
/// Movers 3 and 9 gate static light 2 at two probes of cell 0, which carries
/// its own tile once; mover 3 gates an animated light at one probe and a
/// bake-only light (with its own tile) at two probes of cell 1. Mover 3
/// changes bounce at two probes of cell 1: probe 1 at both termini, probe 40
/// only at terminus 0. Mover 9 carries depth at two probes: probe 4 at both
/// termini, probe 70 only at terminus 0. Parts arrive out of wire order.
fn parts() -> MoverProbeChannelParts {
    use MoverLightNamespace::{AnimatedBaked, BakeOnlyStaticLight, StaticLight};
    MoverProbeChannelParts {
        affinity_dims: [2, 1, 1],
        direct: vec![
            DirectRecordParts {
                cell: 1,
                record: record(BakeOnlyStaticLight, 0, true, (1 << 1) | (1 << 40)),
                movers: vec![(3, vec![1, 2, 3, 4])],
                own_tiles: tiles(2, 1000),
            },
            DirectRecordParts {
                cell: 0,
                record: record(StaticLight, 2, true, 0b1001),
                movers: vec![(9, vec![255, 255, 255, 20]), (3, vec![10, 255, 255, 255])],
                own_tiles: tiles(2, 0),
            },
            DirectRecordParts {
                cell: 1,
                record: record(AnimatedBaked, 0, false, 1 << 5),
                movers: vec![(3, vec![0, 128])],
                own_tiles: Vec::new(),
            },
        ],
        bounce: vec![BounceEntryParts {
            cell: 1,
            entry: MoverBounceEntry::termini(3, [(1 << 1) | (1 << 40), 1 << 1]),
            tiles: (0..3 * MOVER_IRRADIANCE_TILE_F16_COUNT)
                .map(|i| 0x3c00 + i as u16)
                .collect(),
        }],
        depth: [(70, [true, false]), (4, [true, true])]
            .into_iter()
            .map(|(probe_index, stored_termini)| {
                // Probe 4's tiles count from 0, probe 70's from 2 tiles on.
                let first = if probe_index == 4 {
                    0
                } else {
                    2 * PROBE_DEPTH_TILE_TEXELS
                };
                let record = MoverDepthRecord::termini(9, probe_index, stored_termini);
                DepthRecordParts {
                    tiles: (0..record.stored_tile_count() * PROBE_DEPTH_TILE_TEXELS)
                        .map(|i| [(first + i) as u16, 0x4000])
                        .collect(),
                    record,
                }
            })
            .collect(),
    }
}

fn termini_record(mover_id: u32) -> Result<MoverRecord, ()> {
    let (start, end) = if mover_id == 9 {
        (0.6, 0.9)
    } else {
        (0.0, 1.0)
    };
    Ok(MoverRecord::termini(mover_id, start, end))
}

fn assemble(parts: MoverProbeChannelParts) -> Option<MoverProbeChannelSection> {
    MoverProbeChannelSection::assemble(
        parts,
        |cell| if cell == 0 { ALL_VALID } else { CELL1_VALID },
        termini_record,
    )
    .unwrap()
}

fn sample() -> MoverProbeChannelSection {
    assemble(parts()).expect("the sample holds entries")
}

#[test]
fn section_id_is_57() {
    assert_eq!(SectionId::MoverProbeChannel as u32, 57);
    assert_eq!(SectionId::from_u32(57), Some(SectionId::MoverProbeChannel));
}

#[test]
fn assembly_lays_parts_out_in_wire_order_over_the_listed_cells() {
    let section = sample();
    assert_eq!(
        section.cells,
        [
            MoverChannelCell {
                cell: 0,
                valid_probe_mask: ALL_VALID
            },
            MoverChannelCell {
                cell: 1,
                valid_probe_mask: CELL1_VALID
            },
        ]
    );
    assert_eq!(section.direct_offsets, [0, 1, 3]);
    assert_eq!(
        section
            .direct_records
            .iter()
            .map(MoverDirectRecord::order_key)
            .collect::<Vec<_>>(),
        [
            (MoverLightNamespace::StaticLight, 2),
            (MoverLightNamespace::AnimatedBaked, 0),
            (MoverLightNamespace::BakeOnlyStaticLight, 0),
        ]
    );
    assert_eq!(section.mover_offsets, [0, 2, 3, 4]);
    assert_eq!(section.direct_movers, [3, 9, 3, 3], "movers ascend by id");
    assert_eq!(
        &section.visibility[..4],
        &[10, 255, 255, 255],
        "mover 3's pairs lead record 0"
    );
    assert_eq!(section.bounce_offsets, [0, 0, 1]);
    assert_eq!(
        section
            .depth_records
            .iter()
            .map(|r| r.probe_index)
            .collect::<Vec<_>>(),
        [4, 70]
    );
    assert_eq!(section.depth_tiles[0][0], 0, "probe 4's tiles lead");
    assert_eq!(section.mover_ids(), [3, 9]);
    assert_eq!(section.mover_records.len(), 2);

    let mut shuffled = parts();
    shuffled.direct.reverse();
    shuffled.depth.reverse();
    assert_eq!(
        assemble(shuffled),
        Some(section),
        "input order never matters"
    );
}

#[test]
fn every_block_round_trips_with_its_payload_reachable_by_record_and_mover() {
    let section = sample();
    let bytes = section.to_bytes();
    assert_eq!(section.encoded_len(), bytes.len());
    assert_eq!(section.byte_breakdown().total(), bytes.len());
    let restored = MoverProbeChannelSection::from_bytes(&bytes).unwrap();
    assert_eq!(restored, section);

    let cells: Vec<_> = restored.direct_cells().collect();
    assert_eq!(cells.iter().map(|c| c.cell()).collect::<Vec<_>>(), [0, 1]);
    let records: Vec<_> = restored.direct_record_views().collect();
    assert_eq!(records.len(), 3);
    assert_eq!(cells[1].records().count(), 2);

    // One tile per masked probe, once per record, whatever its mover count.
    let shared = &records[0];
    assert_eq!(shared.mover_ids, [3, 9]);
    assert_eq!(
        shared.own_tiles.map(|tiles| tiles.len()),
        Some(2 * MOVER_IRRADIANCE_TILE_F16_COUNT)
    );
    assert_eq!(
        shared.own_tile(1).unwrap().get(0),
        Some(MOVER_IRRADIANCE_TILE_F16_COUNT as u16)
    );
    let movers: Vec<_> = shared.movers().collect();
    assert_eq!(movers[0].mover_id, 3);
    assert_eq!(movers[0].probe_q(0), [10, 255]);
    assert_eq!(movers[1].mover_id, 9);
    assert_eq!(movers[1].probe_q(1), [255, 20]);

    assert!(records[1].own_tiles.is_none());
    assert_eq!(records[1].movers().next().unwrap().visibility, [0, 128]);
    assert_eq!(
        records[2].own_tiles.unwrap().get(0),
        Some(1000),
        "the bake-only record's tiles follow record 0's"
    );
    assert_eq!(restored.bake_only_light_count(), 1);
    assert_eq!(
        restored.direct_counts(),
        MoverDirectCounts {
            cells: 2,
            records: 3,
            mover_entries: 4,
            masked_probes: 5,
            own_tile_probes: 4,
            visibility_bytes: 14,
        }
    );

    let bounce: Vec<_> = restored.bounce_entry_views().collect();
    assert_eq!(bounce.len(), 1);
    assert_eq!(bounce[0].cell, 1);
    assert_eq!(
        bounce[0].tiles,
        Some(MoverHalves::Decoded(&section.bounce_tiles))
    );
    assert_eq!(
        restored.bounce_payload_bytes(),
        (section.bounce_tiles.len() * 2) as u64
    );

    // Probe 1 stores both states; probe 40 stores S^b₀ alone and composes
    // id 34 at terminus 1. Tiles run pose after pose, probe by stored rank.
    let tile_start = |pose: usize, local: u32| {
        bounce[0]
            .pose_tile(pose, local)
            .map(|tiles| tiles.get(0).unwrap())
    };
    let first = |tile: usize| Some(0x3c00 + (tile * MOVER_IRRADIANCE_TILE_F16_COUNT) as u16);
    assert_eq!([tile_start(0, 1), tile_start(1, 1)], [first(0), first(2)]);
    assert_eq!([tile_start(0, 40), tile_start(1, 40)], [first(1), None]);
    assert_eq!(bounce[0].entry.mark(1, 40), PoseMark::Unmarked);
    assert_eq!(
        [tile_start(0, 5), tile_start(1, 5)],
        [None, None],
        "an unaffected valid probe"
    );
    assert_eq!(bounce[0].entry.affected_probe_mask(), (1 << 1) | (1 << 40));
    assert_eq!(
        restored.bounce_payload_bytes(),
        (3 * MOVER_IRRADIANCE_TILE_F16_COUNT * 2) as u64
    );

    let depth: Vec<_> = restored.depth_record_views().collect();
    assert_eq!(depth[0].record.probe_index, 4);
    assert_eq!(depth[0].pose_tile(0).unwrap()[0], [0, 0x4000]);
    assert_eq!(
        depth[0].pose_tile(1).unwrap()[0],
        [PROBE_DEPTH_TILE_TEXELS as u16, 0x4000]
    );
    assert_eq!(
        depth[1].pose_tile(0).unwrap()[0],
        [(2 * PROBE_DEPTH_TILE_TEXELS) as u16, 0x4000]
    );
    assert!(
        depth[1].pose_tile(1).is_none(),
        "probe 70 reads its base at terminus 1"
    );
    assert_eq!(restored.depth_tiles.len(), 3 * PROBE_DEPTH_TILE_TEXELS);
    assert_eq!(restored.live_tile_count(), 2);
    assert_eq!(
        restored.mover_record(9).map(|record| (
            record.ramp_start,
            record.ramp_end,
            record.knots.clone()
        )),
        Some((0.6, 0.9, vec![0.0, 1.0]))
    );
    assert_eq!(restored.mover_record(4), None);
    assert!(restored.cell_has_entries(1) && !restored.cell_has_entries(2));
    assert!(restored.cell_has_direct_records(0));
}

#[test]
fn header_cell_index_and_first_record_sit_at_their_pinned_offsets() {
    let bytes = sample().to_bytes();
    assert_eq!(bytes[0], MOVER_PROBE_CHANNEL_EPOCH);
    assert_eq!(bytes[1], AFFINITY_FACTOR);
    assert_eq!(&bytes[2..14], &[2, 0, 0, 0, 1, 0, 0, 0, 1, 0, 0, 0]);
    assert_eq!(bytes[14], MoverPayloadPlacement::Resident as u8, "direct");
    assert_eq!(bytes[15], MoverPayloadPlacement::Resident as u8, "bounce");
    assert_eq!(&bytes[16..20], &2u32.to_le_bytes(), "two listed cells");
    assert_eq!(&bytes[20..24], &0u32.to_le_bytes());
    assert_eq!(&bytes[24..32], &ALL_VALID.to_le_bytes());
    assert_eq!(&bytes[32..36], &1u32.to_le_bytes());
    // Two cell records, then the mover records: movers 3 and 9, two poses.
    let movers = 16 + 4 + 2 * 12;
    assert_eq!(&bytes[movers..movers + 4], &2u32.to_le_bytes());
    let mover3 = [
        3u32.to_le_bytes(),
        0f32.to_le_bytes(),
        1f32.to_le_bytes(),
        2u32.to_le_bytes(),
        0f32.to_le_bytes(),
        1f32.to_le_bytes(),
    ]
    .concat();
    assert_eq!(&bytes[movers + 4..movers + 28], &mover3[..]);
    assert_eq!(&bytes[movers + 28..movers + 32], &9u32.to_le_bytes());
    assert_eq!(&bytes[movers + 32..movers + 36], &0.6f32.to_le_bytes());
    // Then three direct offsets, then the first record.
    let record = movers + 4 + 2 * 24 + 3 * 4;
    assert_eq!(
        &bytes[record..record + 16],
        &[
            0,
            MOVER_RECORD_FLAG_OWN_TILE,
            0,
            0,
            2,
            0,
            0,
            0,
            0b1001,
            0,
            0,
            0,
            0,
            0,
            0,
            0
        ]
    );
}

// Namespace 2 names lights no other section holds, so its records always
// carry their own tile: the decoder refuses one with the flag clear.
#[test]
fn a_bake_only_record_without_its_own_tile_is_refused_by_name() {
    let mut bytes = sample().to_bytes();
    // Header, cell index, two two-pose mover records, three direct offsets,
    // then record 2's flags byte.
    let flags = 16 + 4 + 2 * 12 + 4 + 2 * 24 + 3 * 4 + 2 * 16 + 1;
    assert_eq!(
        bytes[flags - 1],
        MoverLightNamespace::BakeOnlyStaticLight as u8
    );
    bytes[flags] = 0;
    assert_eq!(
        MoverProbeChannelSection::from_bytes(&bytes),
        Err(MoverProbeChannelError::OwnTileRequired {
            record: 2,
            namespace: MoverLightNamespace::BakeOnlyStaticLight,
        })
    );
}

/// `section` with each block placed as given.
fn placed(
    mut section: MoverProbeChannelSection,
    direct: MoverPayloadPlacement,
    bounce: MoverPayloadPlacement,
) -> MoverProbeChannelSection {
    section.direct_placement = direct;
    section.bounce_placement = bounce;
    section
}

/// `section` as a load decodes it: a streamed block's payload not held.
fn unheld(mut section: MoverProbeChannelSection) -> MoverProbeChannelSection {
    if section.direct_placement == MoverPayloadPlacement::Streamed {
        section.visibility.clear();
        section.own_tiles.clear();
    }
    if section.bounce_placement == MoverPayloadPlacement::Streamed {
        section.bounce_tiles.clear();
    }
    section
}

/// Every id-50 chunk's rows for `section`: one chunk per listed cell, plus
/// a second chunk covering every cell, as a cell two clusters share repeats.
fn chunk_rows(section: &MoverProbeChannelSection) -> Vec<Vec<u8>> {
    let slices = section.cell_slices();
    let listed: Vec<u32> = section.listed_cells().map(|cell| cell as u32).collect();
    let mut chunks: Vec<Vec<u8>> = listed
        .iter()
        .map(|&cell| {
            section.encode_streamed_rows(&slices, &section.streamed_rows_for(&slices, [cell]))
        })
        .collect();
    chunks.push(section.encode_streamed_rows(&slices, &section.streamed_rows_for(&slices, listed)));
    chunks
}

const ALL_PLACEMENTS: [(MoverPayloadPlacement, MoverPayloadPlacement); 4] = {
    use MoverPayloadPlacement::{Resident, Streamed};
    [
        (Resident, Resident),
        (Streamed, Resident),
        (Resident, Streamed),
        (Streamed, Streamed),
    ]
};

// Each block's payload round-trips resident or streamed, in every
// combination. A streamed payload leaves the section, which keeps its
// cells, records, masks and mover entries; the payload's weight does not
// change; and the rows id 50 carries attach back to the resident section's
// bytes exactly.
#[test]
fn both_placements_of_both_blocks_round_trip_and_attach_back_to_the_resident_payload() {
    for resident in [sample(), posed_sample()] {
        placements_round_trip_and_attach_back(&resident);
    }
}

fn placements_round_trip_and_attach_back(resident: &MoverProbeChannelSection) {
    for (direct, bounce) in ALL_PLACEMENTS {
        let section = placed(resident.clone(), direct, bounce);
        let bytes = section.to_bytes();
        assert_eq!(section.encoded_len(), bytes.len());
        let breakdown = section.byte_breakdown();
        let streamed = |placement| placement == MoverPayloadPlacement::Streamed;
        assert_eq!(
            resident.to_bytes().len() - bytes.len(),
            breakdown.streamed_direct + breakdown.streamed_bounce,
            "{direct:?}/{bounce:?}: only the streamed payload leaves the section"
        );
        assert_eq!(
            breakdown.streamed_direct as u64,
            if streamed(direct) {
                resident.direct_payload_bytes()
            } else {
                0
            }
        );
        assert_eq!(
            breakdown.streamed_bounce as u64,
            if streamed(bounce) {
                resident.bounce_payload_bytes()
            } else {
                0
            }
        );

        let mut restored = MoverProbeChannelSection::from_bytes(&bytes).unwrap();
        assert_eq!(restored, unheld(section.clone()));
        assert_eq!(restored.direct_placement, direct);
        assert_eq!(restored.bounce_placement, bounce);
        assert_eq!(
            restored.direct_payload_bytes(),
            resident.direct_payload_bytes()
        );
        assert_eq!(
            restored.bounce_payload_bytes(),
            resident.bounce_payload_bytes()
        );
        assert_eq!(restored.streams(), streamed(direct) || streamed(bounce));
        if streamed(bounce) {
            assert_eq!(restored.bounce_entry_views().next().unwrap().tiles, None);
        }
        if streamed(direct) {
            assert_eq!(
                restored.direct_cells().count(),
                0,
                "no held payload, no view"
            );
        }

        let chunks = chunk_rows(&section);
        let rows = chunks
            .iter()
            .flat_map(|chunk| parse_streamed_rows(chunk).unwrap())
            .collect::<Vec<_>>();
        if !restored.streams() {
            assert!(
                rows.is_empty(),
                "a resident section places no row with id 50"
            );
        }
        restored.attach_streamed_payload(rows).unwrap();
        assert_eq!(
            restored, section,
            "{direct:?}/{bounce:?}: attached equals resident"
        );
    }
}

// A renderer reads a cell through one view whatever holds each
// block: a streamed block from its id-50 row, a resident one from the
// section. Both give the resident section's records, movers, q, own tiles
// and bounce tiles.
#[test]
fn a_cells_payload_reads_the_same_whichever_placement_holds_it() {
    for resident in [sample(), posed_sample()] {
        a_cells_payload_reads_alike(&resident);
    }
}

fn a_cells_payload_reads_alike(resident: &MoverProbeChannelSection) {
    let slices = resident.cell_slices();
    for (direct, bounce) in ALL_PLACEMENTS {
        let section = placed(resident.clone(), direct, bounce);
        let loaded = MoverProbeChannelSection::from_bytes(&section.to_bytes()).unwrap();
        for (position, cell) in resident.listed_cells().enumerate() {
            let cell = cell as u32;
            let chunk =
                section.encode_streamed_rows(&slices, &section.streamed_rows_for(&slices, [cell]));
            let rows = parse_streamed_rows(&chunk).unwrap();
            let row = rows.first().copied();
            let expected = resident
                .cell_payload(position, &slices[position], None)
                .unwrap();
            let payload = loaded
                .cell_payload(position, &slices[position], row)
                .unwrap();
            let records = |section: &MoverProbeChannelSection, payload| {
                section
                    .direct_cell_at(position, payload)
                    .map(|cell| {
                        cell.records()
                            .map(|view| {
                                (
                                    *view.record,
                                    view.movers()
                                        .map(|mover| (mover.mover_id, mover.visibility.to_vec()))
                                        .collect::<Vec<_>>(),
                                    view.own_tiles.map(|tiles| tiles.to_vec()),
                                )
                            })
                            .collect::<Vec<_>>()
                    })
                    .unwrap_or_default()
            };
            assert_eq!(records(&loaded, payload), records(resident, expected));
            let bounce = |section: &MoverProbeChannelSection, payload| {
                section
                    .bounce_entries_at(position, payload)
                    .map(|view| (view.entry.clone(), view.tiles.map(|tiles| tiles.to_vec())))
                    .collect::<Vec<_>>()
            };
            assert_eq!(bounce(&loaded, payload), bounce(resident, expected));
        }
    }
}

// A streamed block's payload with no id-50 row is the named error,
// never an empty read; so is a row of the wrong length, a row two chunks
// disagree on, and a cell no chunk carries when the payload attaches.
#[test]
fn an_unresolved_or_malformed_streamed_payload_is_refused_by_name() {
    use MoverPayloadPlacement::{Resident, Streamed};
    let resident = sample();
    let slices = resident.cell_slices();
    let section = placed(resident.clone(), Streamed, Streamed);
    let loaded = MoverProbeChannelSection::from_bytes(&section.to_bytes()).unwrap();
    assert_eq!(
        loaded.cell_payload(0, &slices[0], None).unwrap_err(),
        MoverProbeChannelError::StreamedPayloadUnresolved {
            block: MoverChannelBlock::Direct
        }
    );
    let bounce_only = placed(resident.clone(), Resident, Streamed);
    let loaded_bounce = MoverProbeChannelSection::from_bytes(&bounce_only.to_bytes()).unwrap();
    assert_eq!(
        loaded_bounce.cell_payload(1, &slices[1], None).unwrap_err(),
        MoverProbeChannelError::StreamedPayloadUnresolved {
            block: MoverChannelBlock::Bounce
        }
    );

    let chunk = section.encode_streamed_rows(&slices, &section.streamed_rows_for(&slices, [0, 1]));
    let rows = parse_streamed_rows(&chunk).unwrap();
    let short = MoverStreamedRow {
        direct: &rows[0].direct[1..],
        ..rows[0]
    };
    assert!(matches!(
        loaded.cell_payload(0, &slices[0], Some(short)),
        Err(MoverProbeChannelError::StreamedPayloadMismatch {
            block: MoverChannelBlock::Direct,
            cell: 0,
            ..
        })
    ));
    assert!(matches!(
        loaded.clone().attach_streamed_payload([rows[1]]),
        Err(MoverProbeChannelError::StreamedPayloadMismatch { cell: 0, .. })
    ));
    let mut altered = rows[1].bounce.to_vec();
    altered[0] ^= 1;
    let disagreeing = MoverStreamedRow {
        bounce: &altered,
        ..rows[1]
    };
    assert!(matches!(
        loaded
            .clone()
            .attach_streamed_payload([rows[0], rows[1], disagreeing]),
        Err(MoverProbeChannelError::StreamedPayloadMismatch {
            block: MoverChannelBlock::Bounce,
            cell: 1,
            ..
        })
    ));
    let mut stray = chunk.clone();
    // Row 0's cell becomes 7, which the section does not list.
    stray[8..12].copy_from_slice(&7u32.to_le_bytes());
    let stray_rows = parse_streamed_rows(&stray);
    assert!(
        matches!(stray_rows, Err(MoverProbeChannelError::StreamedRows { .. })),
        "rows must ascend: {stray_rows:?}"
    );
    let mut trailing = chunk;
    trailing.push(0);
    assert!(matches!(
        parse_streamed_rows(&trailing),
        Err(MoverProbeChannelError::StreamedRows { .. })
    ));
}

// The header's two placement bytes are read per block; any other epoch,
// epoch 4's per-terminus layout, epoch 5's through-base depth rule and
// epoch 6's across marks included, is a named re-bake, an unknown placement
// names its block, and a streamed block with no entry is refused for that
// block.
#[test]
fn placements_are_read_per_block_and_any_other_epoch_is_refused_by_name() {
    use MoverPayloadPlacement::{Resident, Streamed};
    let bytes = sample().to_bytes();
    assert_eq!(bytes[0], 7);
    for epoch in [3, 4, 5, 6, 8] {
        let mut other = bytes.clone();
        other[0] = epoch;
        assert_eq!(
            MoverProbeChannelSection::from_bytes(&other),
            Err(MoverProbeChannelError::EpochMismatch {
                found: epoch,
                expected: 7
            })
        );
    }
    for (at, block) in [
        (14, MoverChannelBlock::Direct),
        (15, MoverChannelBlock::Bounce),
    ] {
        let mut unknown = bytes.clone();
        unknown[at] = 2;
        assert_eq!(
            MoverProbeChannelSection::from_bytes(&unknown),
            Err(MoverProbeChannelError::UnknownPlacement { block, value: 2 })
        );
    }
    let mut bounce_only = parts();
    bounce_only.direct.clear();
    let bounce_only = assemble(bounce_only).unwrap();
    assert_eq!(
        MoverProbeChannelSection::from_bytes(&placed(bounce_only, Streamed, Resident).to_bytes()),
        Err(MoverProbeChannelError::StreamedBlockWithoutEntries {
            block: MoverChannelBlock::Direct
        })
    );
    let mut direct_only = parts();
    direct_only.bounce.clear();
    let direct_only = assemble(direct_only).unwrap();
    assert_eq!(
        MoverProbeChannelSection::from_bytes(&placed(direct_only, Resident, Streamed).to_bytes()),
        Err(MoverProbeChannelError::StreamedBlockWithoutEntries {
            block: MoverChannelBlock::Bounce
        })
    );
}

// The direct payload's masks are checked tight when it attaches from id 50,
// as a resident one's are at decode: a masked probe every mover leaves at
// 255 is refused.
#[test]
fn an_attached_direct_payload_is_checked_tight() {
    use MoverPayloadPlacement::{Resident, Streamed};
    let mut section = placed(sample(), Streamed, Resident);
    // Record 1 (animated, one masked probe, mover 3): q₀ 0 → 255 leaves
    // its one probe ungated at both termini once q₁ is 255 too.
    let slices = section.cell_slices();
    let at = slices[1].visibility.start;
    section.visibility[at] = 255;
    section.visibility[at + 1] = 255;
    let mut loaded = MoverProbeChannelSection::from_bytes(&section.to_bytes()).unwrap();
    let chunk = section.encode_streamed_rows(&slices, &section.streamed_rows_for(&slices, [0, 1]));
    let refused = loaded.attach_streamed_payload(parse_streamed_rows(&chunk).unwrap());
    assert!(
        matches!(
            refused,
            Err(MoverProbeChannelError::MoverGatesNothing { .. }
                | MoverProbeChannelError::ProbeMaskNotTight { .. })
        ),
        "{refused:?}"
    );
}

#[test]
fn each_block_alone_round_trips_and_lists_only_its_own_cells() {
    let only = |direct: bool, bounce: bool, depth: bool| {
        let mut parts = parts();
        if !direct {
            parts.direct.clear();
        }
        if !bounce {
            parts.bounce.clear();
        }
        if !depth {
            parts.depth.clear();
        }
        assemble(parts).expect("one block holds entries")
    };
    let direct_only = only(true, false, false);
    assert_eq!(direct_only.listed_cells().collect::<Vec<_>>(), [0, 1]);
    let bounce_only = only(false, true, false);
    assert_eq!(bounce_only.listed_cells().collect::<Vec<_>>(), [1]);
    assert_eq!(bounce_only.direct_cells().count(), 0);
    assert!(!bounce_only.cell_has_direct_records(1));
    let depth_only = only(false, false, true);
    assert!(depth_only.cells.is_empty());
    assert_eq!(depth_only.direct_offsets, [0]);
    assert_eq!(depth_only.mover_records.len(), 1);
    for section in [direct_only, bounce_only, depth_only] {
        let restored = MoverProbeChannelSection::from_bytes(&section.to_bytes()).unwrap();
        assert_eq!(restored, section);
    }
    assert_eq!(assemble(only_nothing()), None, "never written empty");
}

fn only_nothing() -> MoverProbeChannelParts {
    MoverProbeChannelParts {
        direct: Vec::new(),
        bounce: Vec::new(),
        depth: Vec::new(),
        ..parts()
    }
}

/// Four movers of different pose counts. Mover 3 has four poses (knots 0,
/// 0.25, 0.6, 1), mover 9 three, mover 12 two and mover 20 seventeen, so its
/// depth marks span two words. Direct: in cell 0, movers 3 and 9 gate static
/// light 2 at probes 0 and 3; mover 9 only at its mid pose at probe 3, so
/// the mask is the union over poses. In cell 1, mover 12 gates an animated
/// light. Bounce, cell 1: mover 3 stores probe 1 at pose 0, probe 40 at
/// pose 1 where probe 1 reads the base, nothing at pose 2 (both blend
/// across), and both at pose 3; mover 9 stores probe 5 at its mid pose,
/// where probe 40 reads the base, and probe 40 at its last. Depth: mover 3
/// at probe 4 stores poses 0 and 3 and reads the base at 1; mover 9 at
/// probe 70 stores its mid pose alone; mover 20 at probe 6 reads the base
/// at pose 8 and stores pose 16.
fn posed_parts() -> MoverProbeChannelParts {
    use MoverLightNamespace::{AnimatedBaked, StaticLight};
    use PoseMark::{Base, Stored, Unmarked};
    let bounce = |mover_id: u32, poses: &[(u64, u64)], first: u16| {
        let entry = MoverBounceEntry {
            mover_id,
            poses: poses
                .iter()
                .map(|&(stored, base)| BouncePoseMasks { stored, base })
                .collect(),
        };
        BounceEntryParts {
            cell: 1,
            tiles: tiles(entry.stored_tile_count(), first),
            entry,
        }
    };
    let depth = |mover_id: u32, probe_index: u32, marks: Vec<PoseMark>, first: u16| {
        let record = MoverDepthRecord {
            mover_id,
            probe_index,
            marks,
        };
        DepthRecordParts {
            tiles: (0..record.stored_tile_count() * PROBE_DEPTH_TILE_TEXELS)
                .map(|i| [first + i as u16, 0x4000])
                .collect(),
            record,
        }
    };
    let mut mover20 = vec![Unmarked; 17];
    mover20[8] = Base;
    mover20[16] = Stored;
    MoverProbeChannelParts {
        affinity_dims: [2, 1, 1],
        direct: vec![
            DirectRecordParts {
                cell: 0,
                record: record(StaticLight, 2, true, 0b1001),
                movers: vec![
                    (9, vec![255, 255, 255, 255, 40, 255]),
                    (3, vec![10, 20, 255, 255, 255, 255, 255, 255]),
                ],
                own_tiles: tiles(2, 0),
            },
            DirectRecordParts {
                cell: 1,
                record: record(AnimatedBaked, 0, false, 1 << 5),
                movers: vec![(12, vec![0, 128])],
                own_tiles: Vec::new(),
            },
        ],
        bounce: vec![
            bounce(9, &[(0, 0), (1 << 5, 1 << 40), (1 << 40, 0)], 0x5000),
            bounce(
                3,
                &[
                    (1 << 1, 0),
                    (1 << 40, 1 << 1),
                    (0, 0),
                    ((1 << 1) | (1 << 40), 0),
                ],
                0x3c00,
            ),
        ],
        depth: vec![
            depth(20, 6, mover20, 3000),
            depth(9, 70, vec![Unmarked, Stored, Unmarked], 2000),
            depth(3, 4, vec![Stored, Base, Unmarked, Stored], 0),
        ],
    }
}

fn posed_record(mover_id: u32) -> Result<MoverRecord, ()> {
    let knots = match mover_id {
        3 => vec![0.0, 0.25, 0.6, 1.0],
        9 => vec![0.0, 0.5, 1.0],
        20 => (0..17).map(|k| k as f32 / 16.0).collect(),
        _ => vec![0.0, 1.0],
    };
    Ok(MoverRecord {
        mover_id,
        ramp_start: if mover_id == 3 { 0.2 } else { 0.0 },
        ramp_end: 1.0,
        knots,
    })
}

fn posed_sample() -> MoverProbeChannelSection {
    MoverProbeChannelSection::assemble(
        posed_parts(),
        |cell| if cell == 0 { ALL_VALID } else { CELL1_VALID },
        posed_record,
    )
    .unwrap()
    .expect("the posed sample holds entries")
}

/// Byte offsets of each part of `section`'s bytes.
struct Offsets {
    movers: usize,
    bounce: usize,
    depth: usize,
}

fn offsets(section: &MoverProbeChannelSection) -> Offsets {
    let parts = section.byte_breakdown();
    let movers = parts.header + parts.cell_index;
    let bounce = movers + parts.mover_records + parts.direct_block();
    Offsets {
        movers,
        bounce,
        depth: bounce + parts.bounce_index + parts.bounce_payload,
    }
}

fn decode(bytes: &[u8]) -> Result<MoverProbeChannelSection, MoverProbeChannelError> {
    MoverProbeChannelSection::from_bytes(bytes)
}

// Movers of different pose counts round-trip with every per-pose state in
// place: direct q per pose, bounce stored, base and unmarked probes per pose
// with tiles pose-major, and depth marks over one and two mark words.
#[test]
fn a_section_of_movers_with_different_pose_counts_round_trips_every_pose() {
    use PoseMark::{Base, Stored, Unmarked};
    let section = posed_sample();
    let bytes = section.to_bytes();
    assert_eq!(section.encoded_len(), bytes.len());
    let restored = decode(&bytes).unwrap();
    assert_eq!(restored, section);
    assert_eq!(
        restored
            .mover_records
            .iter()
            .map(|record| (record.mover_id, record.pose_count()))
            .collect::<Vec<_>>(),
        [(3, 4), (9, 3), (12, 2), (20, 17)]
    );
    assert_eq!(
        restored.mover_record(3).unwrap().knots,
        [0.0, 0.25, 0.6, 1.0]
    );

    // Direct: each mover's q per pose, probe-major (byte slot·N + k).
    let records: Vec<_> = restored.direct_record_views().collect();
    let movers: Vec<_> = records[0].movers().collect();
    assert_eq!((movers[0].mover_id, movers[0].pose_count), (3, 4));
    assert_eq!(movers[0].probe_q(0), [10, 20, 255, 255]);
    assert_eq!((movers[1].mover_id, movers[1].pose_count), (9, 3));
    assert_eq!(movers[1].q(1, 1), 40, "a mid-pose-only q keeps its probe");
    assert_eq!(records[0].record.probe_mask, 0b1001);
    assert_eq!(restored.direct_counts().visibility_bytes, 2 * 4 + 2 * 3 + 2);

    // Bounce: per-pose marks, tiles pose-major then by stored rank.
    let bounce: Vec<_> = restored.bounce_entry_views().collect();
    assert_eq!(bounce[0].entry.mover_id, 3);
    let entry = bounce[0].entry;
    assert_eq!(entry.affected_probe_mask(), (1 << 1) | (1 << 40));
    assert_eq!(
        (0..4).map(|pose| entry.mark(pose, 1)).collect::<Vec<_>>(),
        [Stored, Base, Unmarked, Stored]
    );
    assert_eq!(
        (0..4).map(|pose| entry.mark(pose, 40)).collect::<Vec<_>>(),
        [Unmarked, Stored, Unmarked, Stored]
    );
    let start = |view: &BounceEntryView<'_>, pose, local| {
        view.pose_tile(pose, local).map(|tile| tile.get(0).unwrap())
    };
    let tile = |first: u16, at: usize| Some(first + (at * MOVER_IRRADIANCE_TILE_F16_COUNT) as u16);
    assert_eq!(start(&bounce[0], 0, 1), tile(0x3c00, 0));
    assert_eq!(start(&bounce[0], 1, 40), tile(0x3c00, 1));
    assert_eq!(start(&bounce[0], 1, 1), None, "a base mark stores no tile");
    assert_eq!(start(&bounce[0], 3, 1), tile(0x3c00, 2));
    assert_eq!(start(&bounce[0], 3, 40), tile(0x3c00, 3));
    assert_eq!(bounce[1].entry.mover_id, 9);
    assert_eq!(bounce[1].entry.mark(1, 40), Base);
    assert_eq!(start(&bounce[1], 1, 5), tile(0x5000, 0));
    assert_eq!(start(&bounce[1], 2, 40), tile(0x5000, 1));

    // Depth: marks and tiles in pose order.
    let depth: Vec<_> = restored.depth_record_views().collect();
    assert_eq!(
        depth
            .iter()
            .map(|view| view.record.order_key())
            .collect::<Vec<_>>(),
        [(3, 4), (9, 70), (20, 6)]
    );
    assert_eq!(depth[0].record.marks, [Stored, Base, Unmarked, Stored]);
    assert_eq!(depth[0].pose_tile(0).unwrap()[0], [0, 0x4000]);
    assert_eq!(depth[0].pose_tile(1), None);
    assert_eq!(
        depth[0].pose_tile(3).unwrap()[0],
        [PROBE_DEPTH_TILE_TEXELS as u16, 0x4000]
    );
    assert_eq!(depth[1].pose_tile(1).unwrap()[0], [2000, 0x4000]);
    assert_eq!(depth[2].record.marks[8], Base);
    assert_eq!(depth[2].pose_tile(16).unwrap()[0], [3000, 0x4000]);
    assert_eq!(restored.live_tile_count(), 3);
}

// The per-pose parts sit where the layout epochs 5 and 6 share pins them:
// mover records after the cell index, bounce entries behind byte offsets
// with a (stored, base) pair per pose, and depth marks two bits a pose, low
// first, padded to whole words.
#[test]
fn per_pose_storage_sits_at_its_pinned_offsets() {
    let section = posed_sample();
    let bytes = section.to_bytes();
    let at = offsets(&section);
    // Mover 3: id, ramp, four poses, knots.
    let m = at.movers;
    assert_eq!(&bytes[m..m + 4], &4u32.to_le_bytes());
    assert_eq!(&bytes[m + 4..m + 8], &3u32.to_le_bytes());
    assert_eq!(&bytes[m + 8..m + 12], &0.2f32.to_le_bytes());
    assert_eq!(&bytes[m + 16..m + 20], &4u32.to_le_bytes());
    assert_eq!(&bytes[m + 24..m + 28], &0.25f32.to_le_bytes());

    // Bounce: three per-cell offsets, three entry byte offsets, then mover
    // 3's entry of 8 + 4 × 16 bytes and mover 9's of 8 + 3 × 16.
    let b = at.bounce;
    assert_eq!(
        &bytes[b..b + 12],
        &[0u32, 0, 2].map(u32::to_le_bytes).concat()[..]
    );
    assert_eq!(
        &bytes[b + 12..b + 24],
        &[0u32, 72, 128].map(u32::to_le_bytes).concat()[..]
    );
    let entry = b + 24;
    assert_eq!(&bytes[entry..entry + 4], &3u32.to_le_bytes());
    assert_eq!(&bytes[entry + 4..entry + 8], &0u32.to_le_bytes());
    // Pose 1: stored probe 40, base probe 1.
    assert_eq!(&bytes[entry + 24..entry + 32], &(1u64 << 40).to_le_bytes());
    assert_eq!(&bytes[entry + 32..entry + 40], &(1u64 << 1).to_le_bytes());

    // Depth: count, then (3, 4) with one mark word: stored, base, -, stored.
    let d = at.depth;
    assert_eq!(&bytes[d..d + 4], &3u32.to_le_bytes());
    assert_eq!(
        &bytes[d + 4..d + 12],
        &[3u32, 4].map(u32::to_le_bytes).concat()[..]
    );
    assert_eq!(&bytes[d + 12..d + 16], &0b01_00_10_01u32.to_le_bytes());
    // (9, 70), one word; then (20, 6) with two words: base at pose 8, stored
    // at pose 16, the first mark of the second word.
    let mover20 = d + 4 + 12 + 12;
    assert_eq!(&bytes[mover20..mover20 + 4], &20u32.to_le_bytes());
    assert_eq!(
        &bytes[mover20 + 8..mover20 + 12],
        &(0b10u32 << 16).to_le_bytes()
    );
    assert_eq!(&bytes[mover20 + 12..mover20 + 16], &1u32.to_le_bytes());
}

/// `bytes` with the little-endian `u32` at `at` replaced.
fn with_u32(bytes: &[u8], at: usize, value: u32) -> Vec<u8> {
    let mut out = bytes.to_vec();
    out[at..at + 4].copy_from_slice(&value.to_le_bytes());
    out
}

/// `bytes` with the `f32` at `at` replaced.
fn with_f32(bytes: &[u8], at: usize, value: f32) -> Vec<u8> {
    with_u32(bytes, at, value.to_bits())
}

/// Mover 3's knot `knot` in the posed sample's bytes.
fn mover3_knot(section: &MoverProbeChannelSection, knot: usize) -> usize {
    offsets(section).movers + 4 + 16 + 4 * knot
}

#[test]
fn a_mover_record_with_fewer_than_two_poses_is_refused_by_name() {
    let section = posed_sample();
    let bytes = section.to_bytes();
    let count = offsets(&section).movers + 4 + 12;
    for poses in [0, 1] {
        assert_eq!(
            decode(&with_u32(&bytes, count, poses)),
            Err(MoverProbeChannelError::PoseCount {
                mover_id: 3,
                count: poses
            })
        );
    }
}

#[test]
fn knots_out_of_order_are_refused_by_name() {
    let section = posed_sample();
    let bytes = section.to_bytes();
    for (value, previous) in [(0.25, 0.25), (0.1, 0.25)] {
        assert_eq!(
            decode(&with_f32(&bytes, mover3_knot(&section, 2), value)),
            Err(MoverProbeChannelError::KnotOrder {
                mover_id: 3,
                knot: 2,
                value,
                previous
            })
        );
    }
}

#[test]
fn a_knot_outside_zero_to_one_is_refused_by_name() {
    let section = posed_sample();
    let bytes = section.to_bytes();
    for (knot, value) in [(1, -0.25), (3, 1.5)] {
        assert_eq!(
            decode(&with_f32(&bytes, mover3_knot(&section, knot), value)),
            Err(MoverProbeChannelError::KnotOutOfRange {
                mover_id: 3,
                knot,
                value
            })
        );
    }
}

#[test]
fn a_non_finite_knot_is_refused_by_name() {
    let section = posed_sample();
    let bytes = section.to_bytes();
    for value in [f32::NAN, f32::INFINITY] {
        let refused = decode(&with_f32(&bytes, mover3_knot(&section, 1), value));
        assert!(
            matches!(
                refused,
                Err(MoverProbeChannelError::KnotNotFinite {
                    mover_id: 3,
                    knot: 1,
                    ..
                })
            ),
            "{refused:?}"
        );
    }
}

#[test]
fn knots_not_running_exactly_from_zero_to_one_are_refused_by_name() {
    let section = posed_sample();
    let bytes = section.to_bytes();
    assert_eq!(
        decode(&with_f32(&bytes, mover3_knot(&section, 0), 0.1)),
        Err(MoverProbeChannelError::KnotEnds {
            mover_id: 3,
            first: 0.1,
            last: 1.0
        })
    );
    assert_eq!(
        decode(&with_f32(&bytes, mover3_knot(&section, 3), 0.9)),
        Err(MoverProbeChannelError::KnotEnds {
            mover_id: 3,
            first: 0.0,
            last: 0.9
        })
    );
}

// A block can name a pose only through its mover's record: a mover with no
// record has no poses, so each block naming one is refused for that block,
// and a record no block names is refused too.
#[test]
fn a_block_naming_a_mover_without_a_record_is_refused_by_name() {
    let section = posed_sample();
    let bytes = section.to_bytes();
    let movers = offsets(&section).movers;
    // Mover records 3, 9, 12, 20: renumber one past its neighbours' order.
    let record = |index: usize| {
        movers
            + 4
            + section.mover_records[..index]
                .iter()
                .map(|record| 16 + 4 * record.pose_count())
                .sum::<usize>()
    };
    for (index, renumbered, block, missing) in [
        (0, 2, "direct", 3),
        (2, 13, "direct", 12),
        (3, 21, "depth", 20),
    ] {
        assert_eq!(
            decode(&with_u32(&bytes, record(index), renumbered)),
            Err(MoverProbeChannelError::MoverRecordMissing {
                block,
                mover_id: missing
            })
        );
    }
    let mut bounce_only = posed_parts();
    bounce_only.direct.clear();
    bounce_only.depth.clear();
    let bounce_only =
        MoverProbeChannelSection::assemble(bounce_only, |_| CELL1_VALID, posed_record)
            .unwrap()
            .unwrap();
    let bytes = bounce_only.to_bytes();
    assert_eq!(
        decode(&with_u32(&bytes, offsets(&bounce_only).movers + 4, 2)),
        Err(MoverProbeChannelError::MoverRecordMissing {
            block: "bounce",
            mover_id: 3
        })
    );
    // An extra record for mover 30, which no block names.
    let at = offsets(&bounce_only).movers;
    let end = at + bounce_only.byte_breakdown().mover_records;
    let mut extra = bytes[..end].to_vec();
    let tail = &bytes[end..];
    extra[at..at + 4].copy_from_slice(&3u32.to_le_bytes());
    for word in [30u32, 0, 1f32.to_bits(), 2, 0, 1f32.to_bits()] {
        extra.extend_from_slice(&word.to_le_bytes());
    }
    extra.extend_from_slice(tail);
    assert_eq!(
        decode(&extra),
        Err(MoverProbeChannelError::MoverRecordUnused { mover_id: 30 })
    );
}

// A bounce entry spans exactly its mover's poses: a whole extra pose pair
// names a pose past the mover's count, and any other length is refused as
// a length mismatch.
#[test]
fn a_bounce_entry_naming_a_pose_past_its_movers_count_is_refused_by_name() {
    let section = posed_sample();
    let bytes = section.to_bytes();
    let b = offsets(&section).bounce;
    let (entry_offsets, entries) = (b + 12, b + 24);
    // Mover 3's entry claims 8 + 5 × 16 bytes: it swallows mover 9's head.
    let longer = with_u32(&bytes, entry_offsets + 4, 72 + 16);
    assert_eq!(
        decode(&longer),
        Err(MoverProbeChannelError::PoseOutOfRange {
            block: "bounce",
            what: "entry",
            index: 0,
            mover_id: 3,
            pose_count: 4
        })
    );
    let ragged = with_u32(&bytes, entry_offsets + 4, 72 - 4);
    assert_eq!(
        decode(&ragged),
        Err(MoverProbeChannelError::BounceEntryLength {
            entry: 0,
            mover_id: 3,
            pose_count: 4,
            bytes: 68,
            expected: 72
        })
    );
    assert!(entries > entry_offsets);
}

#[test]
fn a_depth_record_marking_a_pose_past_its_movers_count_is_refused_by_name() {
    let section = posed_sample();
    let bytes = section.to_bytes();
    // Record (3, 4): four poses, so mark bits 8 and up are padding.
    let marks = offsets(&section).depth + 12;
    assert_eq!(
        decode(&with_u32(&bytes, marks, 0b01_00_10_01 | 0b01 << 8)),
        Err(MoverProbeChannelError::PoseOutOfRange {
            block: "depth",
            what: "record",
            index: 0,
            mover_id: 3,
            pose_count: 4
        })
    );
}

#[test]
fn a_probe_marked_both_stored_and_base_at_one_pose_is_refused_by_name() {
    let section = posed_sample();
    let bytes = section.to_bytes();
    // Mover 3's bounce entry, pose 1: base gains probe 40, which it stores.
    let base = offsets(&section).bounce + 24 + 8 + 16 + 8;
    let mut both = bytes.clone();
    both[base..base + 8].copy_from_slice(&((1u64 << 1) | (1 << 40)).to_le_bytes());
    assert_eq!(
        decode(&both),
        Err(MoverProbeChannelError::StoredAndBase {
            block: "bounce",
            what: "entry",
            index: 0,
            pose: 1,
            probe: 40
        })
    );
    // Depth record (3, 4), pose 1: both bits.
    let marks = offsets(&section).depth + 12;
    assert_eq!(
        decode(&with_u32(&bytes, marks, 0b01_00_11_01)),
        Err(MoverProbeChannelError::StoredAndBase {
            block: "depth",
            what: "record",
            index: 0,
            pose: 1,
            probe: 4
        })
    );
}

#[test]
fn a_base_mark_at_a_terminus_is_refused_by_name() {
    let section = posed_sample();
    let bytes = section.to_bytes();
    // Mover 3's bounce entry, pose 0: base marks probe 40.
    let base = offsets(&section).bounce + 24 + 8 + 8;
    let mut first = bytes.clone();
    first[base..base + 8].copy_from_slice(&(1u64 << 40).to_le_bytes());
    assert_eq!(
        decode(&first),
        Err(MoverProbeChannelError::BaseAtTerminus {
            block: "bounce",
            what: "entry",
            index: 0,
            pose: 0
        })
    );
    // Depth record (3, 4): the last pose reads base.
    let marks = offsets(&section).depth + 12;
    assert_eq!(
        decode(&with_u32(&bytes, marks, 0b10_00_10_01)),
        Err(MoverProbeChannelError::BaseAtTerminus {
            block: "depth",
            what: "record",
            index: 0,
            pose: 3
        })
    );
}

// A base mark is storage sparsity over the probes the entry affects: one on
// a probe no pose stores would mark a probe the entry never changes.
#[test]
fn a_base_mark_outside_the_affected_probes_is_refused_by_name() {
    let section = posed_sample();
    let bytes = section.to_bytes();
    let base = offsets(&section).bounce + 24 + 8 + 16 + 8;
    let mut stray = bytes;
    stray[base..base + 8].copy_from_slice(&((1u64 << 1) | (1 << 5)).to_le_bytes());
    assert_eq!(
        decode(&stray),
        Err(MoverProbeChannelError::BaseOutsideAffected {
            cell: 1,
            entry: 0,
            pose: 1,
            mask: 1 << 5
        })
    );
}

// Epoch 4's checks carry over per pose: a depth record storing no pose, a
// bounce entry whose poses store nothing, and a direct mover at 255 at every
// pose are each refused by name.
#[test]
fn a_record_or_entry_storing_no_pose_is_refused_by_name() {
    let section = posed_sample();
    let bytes = section.to_bytes();
    let marks = offsets(&section).depth + 12;
    assert_eq!(
        decode(&with_u32(&bytes, marks, 0b00_00_10_00)),
        Err(MoverProbeChannelError::DepthRecordStoresNothing { record: 0 })
    );
    let mut empty = bytes.clone();
    let entry = offsets(&section).bounce + 24;
    for pose in 0..4 {
        let at = entry + 8 + 16 * pose;
        empty[at..at + 16].fill(0);
    }
    assert!(matches!(
        decode(&empty),
        Err(MoverProbeChannelError::ProbeMaskEmpty {
            block: "bounce",
            ..
        })
    ));
    let mut ungated = posed_sample();
    // Mover 9's q under the cell-0 record: 255 at every pose of both probes.
    let first = ungated.cell_slices()[0].visibility.start + 2 * 4;
    ungated.visibility[first..first + 6].fill(u8::MAX);
    assert_eq!(
        decode(&ungated.to_bytes()),
        Err(MoverProbeChannelError::MoverGatesNothing {
            cell: 0,
            record: 0,
            entry: 1,
            mover_id: 9
        })
    );
}

#[test]
fn mover_records_out_of_order_are_refused_by_name() {
    let section = posed_sample();
    let bytes = section.to_bytes();
    // Mover 9's record (after mover 3's 32 bytes) becomes mover 3 again.
    let second = offsets(&section).movers + 4 + 32;
    assert_eq!(
        decode(&with_u32(&bytes, second, 3)),
        Err(MoverProbeChannelError::MoverRecordOrder { record: 1 })
    );
}

// No across block (epoch 7): the q payload follows the direct block's mover
// ids directly, whether or not a mover holds a mid pose, and the direct
// block's bytes are its index, records and payload alone.
#[test]
fn the_q_payload_follows_the_mover_ids_with_or_without_mid_poses() {
    for section in [sample(), posed_sample()] {
        let bytes = section.to_bytes();
        let parts = section.byte_breakdown();
        let q = offsets(&section).movers
            + parts.mover_records
            + parts.direct_index
            + parts.direct_records;
        assert_eq!(&bytes[q..q + parts.visibility], &section.visibility[..]);
        assert_eq!(
            parts.direct_block(),
            parts.direct_index + parts.direct_records + parts.visibility + parts.own_tiles
        );
        assert_eq!(decode(&bytes).unwrap(), section);
    }
}

// −0.0 equals 0.0 as a value but is another encoding of the first knot, so
// it is refused: one section has one byte encoding.
#[test]
fn a_negative_zero_first_knot_is_refused_by_name() {
    let section = posed_sample();
    let bytes = section.to_bytes();
    let refused = decode(&with_f32(&bytes, mover3_knot(&section, 0), -0.0));
    assert!(
        matches!(
            refused,
            Err(MoverProbeChannelError::KnotEnds { mover_id: 3, first, last })
                if first.to_bits() == (-0.0f32).to_bits() && last == 1.0
        ),
        "{refused:?}"
    );
}

// A pose count at the field's top is refused by the length check on its
// knots, before anything is allocated for them.
#[test]
fn a_pose_count_past_the_sections_bytes_is_refused_without_allocating() {
    let section = posed_sample();
    let bytes = section.to_bytes();
    let count = offsets(&section).movers + 4 + 12;
    let refused = decode(&with_u32(&bytes, count, u32::MAX));
    assert!(
        matches!(
            refused,
            Err(MoverProbeChannelError::Truncated {
                what: "mover knots",
                ..
            })
        ),
        "{refused:?}"
    );
}

/// One listed cell of 64 valid probes and one mover of `pose_count` evenly
/// spaced poses, both blocks empty and resident. A test fills one block.
fn one_mover_section(pose_count: usize) -> MoverProbeChannelSection {
    let last = (pose_count - 1) as f32;
    MoverProbeChannelSection {
        affinity_factor: AFFINITY_FACTOR,
        affinity_dims: [1, 1, 1],
        direct_placement: MoverPayloadPlacement::Resident,
        bounce_placement: MoverPayloadPlacement::Resident,
        cells: vec![MoverChannelCell {
            cell: 0,
            valid_probe_mask: ALL_VALID,
        }],
        mover_records: vec![MoverRecord {
            mover_id: 1,
            ramp_start: 0.0,
            ramp_end: 1.0,
            knots: (0..pose_count).map(|pose| pose as f32 / last).collect(),
        }],
        direct_offsets: vec![0, 0],
        direct_records: Vec::new(),
        mover_offsets: vec![0],
        direct_movers: Vec::new(),
        visibility: Vec::new(),
        own_tiles: Vec::new(),
        bounce_offsets: vec![0, 0],
        bounce_entries: Vec::new(),
        bounce_tiles: Vec::new(),
        depth_records: Vec::new(),
        depth_tiles: Vec::new(),
    }
}

// An id-50 row carries a cell's streamed payload length as a u32. Decode
// refuses a cell past it from the section's lengths alone, as no streamed
// payload is in id 57, so no load path sizes such a row. Direct: 1,024
// bake-only lights, each with its own tile at all 64 probes, gated by one
// 65,536-pose mover, put 2³² bytes of q in the cell.
#[test]
fn a_streamed_direct_row_past_u32_is_refused_by_name_from_lengths_alone() {
    use MoverLightNamespace::BakeOnlyStaticLight;
    const LIGHTS: u32 = 1024;
    let mut section = one_mover_section(65_536);
    section.direct_placement = MoverPayloadPlacement::Streamed;
    section.direct_offsets = vec![0, LIGHTS];
    section.direct_records = (0..LIGHTS)
        .map(|light| record(BakeOnlyStaticLight, light, true, ALL_VALID))
        .collect();
    section.mover_offsets = (0..=LIGHTS).collect();
    section.direct_movers = vec![1; LIGHTS as usize];
    // Stands in for the payload so the encoder sees it held; a streamed
    // payload is never written to id 57.
    section.visibility = vec![255];
    let expected = 64 * 65_536 * u64::from(LIGHTS)
        + 2 * 64 * MOVER_IRRADIANCE_TILE_F16_COUNT as u64 * u64::from(LIGHTS);

    let bytes = section.to_bytes();
    assert!(bytes.len() < 1 << 20, "the section holds lengths, not q");
    assert_eq!(
        decode(&bytes),
        Err(MoverProbeChannelError::StreamedRowOverflow {
            block: MoverChannelBlock::Direct,
            cell: 0,
            bytes: expected,
        })
    );
}

// Bounce: one entry storing all 64 probes at every pose of a mover with
// enough poses that its tiles pass 2³² bytes.
#[test]
fn a_streamed_bounce_row_past_u32_is_refused_by_name_from_lengths_alone() {
    let pose_bytes = 64 * MOVER_IRRADIANCE_TILE_F16_COUNT * 2;
    let pose_count = u32::MAX as usize / pose_bytes + 1;
    let mut section = one_mover_section(pose_count);
    section.bounce_placement = MoverPayloadPlacement::Streamed;
    section.bounce_offsets = vec![0, 1];
    section.bounce_entries = vec![MoverBounceEntry {
        mover_id: 1,
        poses: vec![
            BouncePoseMasks {
                stored: ALL_VALID,
                base: 0
            };
            pose_count
        ],
    }];
    // Stands in for the payload so the encoder sees it held; a streamed
    // payload is never written to id 57.
    section.bounce_tiles = vec![0];

    assert_eq!(
        decode(&section.to_bytes()),
        Err(MoverProbeChannelError::StreamedRowOverflow {
            block: MoverChannelBlock::Bounce,
            cell: 0,
            bytes: (pose_count * pose_bytes) as u64,
        })
    );
}

// The ramp's knot mirror of the game's hand-over: a subnormal span
// overflows the quotient past the window's end, which reads 1, not 0; the
// window's start still reads 0, and −0.0 still reads +0.0.
#[test]
fn a_subnormal_ramp_span_reads_one_past_its_end() {
    let end = f32::from_bits(1);
    assert_eq!(ramped_knot(1.0, 0.0, end), 1.0);
    assert_eq!(ramped_knot(0.5, 0.0, end), 1.0);
    assert_eq!(ramped_knot(end, 0.0, end), 1.0);
    assert_eq!(ramped_knot(0.0, 0.0, end).to_bits(), 0.0f32.to_bits());
    assert_eq!(ramped_knot(-0.0, 0.0, 1.0).to_bits(), 0.0f32.to_bits());
    assert_eq!(ramped_knot(f32::NAN, 0.0, end), 0.0, "never NaN");
}
