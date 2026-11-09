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
            entry: MoverBounceEntry {
                mover_id: 3,
                stored_probe_masks: [(1 << 1) | (1 << 40), 1 << 1],
            },
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
                let record = MoverDepthRecord {
                    mover_id: 9,
                    probe_index,
                    stored_termini,
                };
                DepthRecordParts {
                    record,
                    tiles: (0..record.stored_tile_count() * PROBE_DEPTH_TILE_TEXELS)
                        .map(|i| [(first + i) as u16, 0x4000])
                        .collect(),
                }
            })
            .collect(),
    }
}

fn ramp(mover_id: u32) -> Result<MoverLightRampRecord, ()> {
    let (start, end) = if mover_id == 9 {
        (0.6, 0.9)
    } else {
        (0.0, 1.0)
    };
    Ok(MoverLightRampRecord {
        mover_id,
        start,
        end,
    })
}

fn assemble(parts: MoverProbeChannelParts) -> Option<MoverProbeChannelSection> {
    MoverProbeChannelSection::assemble(
        parts,
        |cell| if cell == 0 { ALL_VALID } else { CELL1_VALID },
        ramp,
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
    assert_eq!(section.ramps.len(), 2);

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
    assert_eq!(movers[0].q(0), [10, 255]);
    assert_eq!(movers[1].mover_id, 9);
    assert_eq!(movers[1].q(1), [255, 20]);

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
            visibility_pairs: 7,
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
    // id 34 at terminus 1. Tiles run probe after probe, S^b₀ first.
    let tile_start = |halves: Option<MoverHalves<'_>>| halves.map(|tiles| tiles.get(0).unwrap());
    let first = |tile: usize| Some(0x3c00 + (tile * MOVER_IRRADIANCE_TILE_F16_COUNT) as u16);
    assert_eq!(
        bounce[0].probe_tiles(1).map(|t| t.map(tile_start)),
        Some([first(0), first(1)])
    );
    assert_eq!(
        bounce[0].probe_tiles(40).map(|t| t.map(tile_start)),
        Some([first(2), None])
    );
    assert_eq!(bounce[0].probe_tiles(5), None, "an unaffected valid probe");
    assert_eq!(bounce[0].entry.affected_probe_mask(), (1 << 1) | (1 << 40));
    assert_eq!(
        restored.bounce_payload_bytes(),
        (3 * MOVER_IRRADIANCE_TILE_F16_COUNT * 2) as u64
    );

    let depth: Vec<_> = restored.depth_record_tiles().collect();
    assert_eq!(depth[0].0.probe_index, 4);
    let [d0, d1] = depth[0].1;
    assert_eq!(d0.unwrap()[0], [0, 0x4000]);
    assert_eq!(d1.unwrap()[0], [PROBE_DEPTH_TILE_TEXELS as u16, 0x4000]);
    let [d0, d1] = depth[1].1;
    assert_eq!(
        d0.unwrap()[0],
        [(2 * PROBE_DEPTH_TILE_TEXELS) as u16, 0x4000]
    );
    assert!(d1.is_none(), "probe 70 reads its base at terminus 1");
    assert_eq!(restored.depth_tiles.len(), 3 * PROBE_DEPTH_TILE_TEXELS);
    assert_eq!(restored.live_tile_count(), 2);
    assert_eq!(
        restored.ramp_of(9).map(|ramp| (ramp.start, ramp.end)),
        Some((0.6, 0.9))
    );
    assert_eq!(restored.ramp_of(4), None);
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
    // Two cell records, then three direct offsets, then the first record.
    let record = 16 + 4 + 2 * 12 + 3 * 4;
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
    // Header, cell index, three direct offsets, then record 2's flags byte.
    let flags = 16 + 4 + 2 * 12 + 3 * 4 + 2 * 16 + 1;
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
    let resident = sample();
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
    let resident = sample();
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
            assert_eq!(records(&loaded, payload), records(&resident, expected));
            let bounce = |section: &MoverProbeChannelSection, payload| {
                section
                    .bounce_entries_at(position, payload)
                    .map(|view| (*view.entry, view.tiles.map(|tiles| tiles.to_vec())))
                    .collect::<Vec<_>>()
            };
            assert_eq!(bounce(&loaded, payload), bounce(&resident, expected));
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

// The header's two placement bytes are read per block; an
// earlier epoch is a named re-bake, an unknown placement names its block,
// and a streamed block with no entry is refused for that block.
#[test]
fn placements_are_read_per_block_and_an_earlier_epoch_is_refused_by_name() {
    use MoverPayloadPlacement::{Resident, Streamed};
    let bytes = sample().to_bytes();
    let mut stale = bytes.clone();
    stale[0] = 3;
    assert_eq!(
        MoverProbeChannelSection::from_bytes(&stale),
        Err(MoverProbeChannelError::EpochMismatch {
            found: 3,
            expected: 4
        })
    );
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
    assert_eq!(depth_only.ramps.len(), 1);
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

/// The byte offset of the depth block's record count in `section`'s bytes.
fn depth_block_offset(section: &MoverProbeChannelSection) -> usize {
    let parts = section.byte_breakdown();
    parts.header
        + parts.cell_index
        + parts.direct_block()
        + parts.bounce_index
        + parts.bounce_payload
}

// Epoch 4 stores per terminus. A bounce entry's two masks and a
// depth record's stored-termini word sit where the layout pins them; a
// record storing neither terminus, a reserved termini bit, and a bounce
// entry storing nothing are each refused by name.
#[test]
fn per_terminus_storage_sits_at_its_pinned_offsets_and_is_refused_when_malformed() {
    let section = sample();
    let bytes = section.to_bytes();
    let parts = section.byte_breakdown();
    // The bounce block: three offsets, then mover 3's 24-byte entry.
    let entry = parts.header + parts.cell_index + parts.direct_block() + 3 * 4;
    assert_eq!(&bytes[entry..entry + 4], &3u32.to_le_bytes());
    assert_eq!(&bytes[entry + 4..entry + 8], &0u32.to_le_bytes());
    assert_eq!(
        &bytes[entry + 8..entry + 16],
        &((1u64 << 1) | (1 << 40)).to_le_bytes()
    );
    assert_eq!(&bytes[entry + 16..entry + 24], &(1u64 << 1).to_le_bytes());
    // The depth block: count, then (mover 9, probe 4, both), (9, 70, D₀).
    let depth = depth_block_offset(&section);
    assert_eq!(&bytes[depth..depth + 4], &2u32.to_le_bytes());
    assert_eq!(&bytes[depth + 4..depth + 8], &9u32.to_le_bytes());
    assert_eq!(&bytes[depth + 8..depth + 12], &4u32.to_le_bytes());
    assert_eq!(&bytes[depth + 12..depth + 16], &0b11u32.to_le_bytes());
    assert_eq!(&bytes[depth + 20..depth + 24], &70u32.to_le_bytes());
    assert_eq!(&bytes[depth + 24..depth + 28], &0b01u32.to_le_bytes());

    let mut nothing = bytes.clone();
    nothing[depth + 24..depth + 28].copy_from_slice(&0u32.to_le_bytes());
    assert_eq!(
        MoverProbeChannelSection::from_bytes(&nothing),
        Err(MoverProbeChannelError::DepthRecordStoresNothing { record: 1 })
    );
    let mut reserved = bytes.clone();
    reserved[depth + 24..depth + 28].copy_from_slice(&0b101u32.to_le_bytes());
    assert_eq!(
        MoverProbeChannelSection::from_bytes(&reserved),
        Err(MoverProbeChannelError::ReservedBits {
            what: "depth record stored-termini bits",
            index: 1,
            value: 0b101,
        })
    );
    let mut empty = bytes;
    empty[entry + 8..entry + 24].fill(0);
    assert!(matches!(
        MoverProbeChannelSection::from_bytes(&empty),
        Err(MoverProbeChannelError::ProbeMaskEmpty {
            block: "bounce",
            ..
        })
    ));
}

// A bounce entry's tiles are indexed probe after probe, S^b₀
// before S^b₁, counting only stored termini.
#[test]
fn a_bounce_entrys_probe_tiles_count_only_stored_termini() {
    let entry = MoverBounceEntry {
        mover_id: 1,
        stored_probe_masks: [0b0101, 0b0110],
    };
    assert_eq!(entry.affected_probe_mask(), 0b0111);
    assert_eq!(entry.stored_tile_count(), 4);
    assert_eq!(entry.probe_tiles(0), Some([Some(0), None]));
    assert_eq!(entry.probe_tiles(1), Some([None, Some(1)]));
    assert_eq!(entry.probe_tiles(2), Some([Some(2), Some(3)]));
    assert_eq!(entry.probe_tiles(3), None);
    assert_eq!(entry.probe_tiles(64), None);
    let high = MoverBounceEntry {
        mover_id: 1,
        stored_probe_masks: [1 << 63, 1 << 63],
    };
    assert_eq!(high.probe_tiles(63), Some([Some(0), Some(1)]));
}
