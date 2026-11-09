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
/// changes bounce at two probes of cell 1; mover 9 carries depth at two
/// probes. Parts arrive out of wire order.
fn parts() -> MoverProbeChannelParts {
    use MoverLightNamespace::{AnimatedBaked, BakeOnlyStaticLight, StaticLight};
    MoverProbeChannelParts {
        affinity_dims: [2, 1, 1],
        bounce_placement: MoverBouncePlacement::Resident,
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
                affected_probe_mask: (1 << 1) | (1 << 40),
            },
            tiles: (0..2 * 2 * MOVER_IRRADIANCE_TILE_F16_COUNT)
                .map(|i| 0x3c00 + i as u16)
                .collect(),
        }],
        depth: [70, 4]
            .into_iter()
            .enumerate()
            .map(|(at, probe_index)| DepthRecordParts {
                record: MoverDepthRecord {
                    mover_id: 9,
                    probe_index,
                },
                tiles: (0..2 * PROBE_DEPTH_TILE_TEXELS)
                    .map(|i| [((1 - at) * 2 * PROBE_DEPTH_TILE_TEXELS + i) as u16, 0x4000])
                    .collect(),
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
        shared.own_tiles.map(<[u16]>::len),
        Some(2 * MOVER_IRRADIANCE_TILE_F16_COUNT)
    );
    assert_eq!(
        shared.own_tile(1).unwrap()[0],
        MOVER_IRRADIANCE_TILE_F16_COUNT as u16
    );
    let movers: Vec<_> = shared.movers().collect();
    assert_eq!(movers[0].mover_id, 3);
    assert_eq!(movers[0].q(0), [10, 255]);
    assert_eq!(movers[1].mover_id, 9);
    assert_eq!(movers[1].q(1), [255, 20]);

    assert!(records[1].own_tiles.is_none());
    assert_eq!(records[1].movers().next().unwrap().visibility, [0, 128]);
    assert_eq!(
        records[2].own_tiles.unwrap()[0],
        1000,
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
    assert_eq!(bounce[0].tiles, Some(&section.bounce_tiles[..]));
    assert_eq!(
        restored.bounce_payload_bytes(),
        (section.bounce_tiles.len() * 2) as u64
    );

    let [d0, d1] = restored.depth_tile_pair(1);
    assert_eq!(d0[0], [(2 * PROBE_DEPTH_TILE_TEXELS) as u16, 0x4000]);
    assert_eq!(d1[0], [(3 * PROBE_DEPTH_TILE_TEXELS) as u16, 0x4000]);
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
    assert_eq!(bytes[14], MoverBouncePlacement::Resident as u8);
    assert_eq!(&bytes[15..19], &2u32.to_le_bytes(), "two listed cells");
    assert_eq!(&bytes[19..23], &0u32.to_le_bytes());
    assert_eq!(&bytes[23..31], &ALL_VALID.to_le_bytes());
    assert_eq!(&bytes[31..35], &1u32.to_le_bytes());
    // Two cell records, then three direct offsets, then the first record.
    let record = 15 + 4 + 2 * 12 + 3 * 4;
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
    let flags = 15 + 4 + 2 * 12 + 3 * 4 + 2 * 16 + 1;
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

#[test]
fn a_streamed_bounce_payload_keeps_its_index_and_leaves_its_tiles_out() {
    let resident = sample();
    let mut streamed = resident.clone();
    streamed.bounce_placement = MoverBouncePlacement::Streamed;
    streamed.bounce_tiles.clear();

    let bytes = streamed.to_bytes();
    assert_eq!(
        resident.to_bytes().len() - bytes.len(),
        resident.bounce_tiles.len() * 2,
        "only the payload leaves the section"
    );
    let restored = MoverProbeChannelSection::from_bytes(&bytes).unwrap();
    assert_eq!(restored, streamed);
    assert_eq!(restored.bounce_entry_views().next().unwrap().tiles, None);
    assert_eq!(
        restored.bounce_payload_bytes(),
        resident.bounce_payload_bytes(),
        "the payload's weight does not depend on where it lives"
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
