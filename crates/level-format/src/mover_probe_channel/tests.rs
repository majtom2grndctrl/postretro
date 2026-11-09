use super::*;
use crate::SectionId;
use crate::delta_sh_volumes::AFFINITY_FACTOR;

const ALL_VALID: u64 = u64::MAX;

/// Two cells along x. Cell 0 is all valid; cell 1 holds three valid probes.
/// Mover 3 gates a static light in cell 0 with its own tile, and an animated
/// light and a bake-only light (with its own tile) in cell 1; mover 9 gates
/// the same static light in cell 0. Mover 3 changes bounce at two probes of
/// cell 1; mover 9 carries depth at two probes.
fn sample() -> MoverProbeChannelSection {
    let cell1_mask = (1u64 << 1) | (1 << 5) | (1 << 40);
    let direct_entries = vec![
        MoverDirectEntry {
            mover_id: 3,
            namespace: MoverLightNamespace::StaticLight,
            carries_own_tile: true,
            light_index: 2,
        },
        MoverDirectEntry {
            mover_id: 9,
            namespace: MoverLightNamespace::StaticLight,
            carries_own_tile: false,
            light_index: 2,
        },
        MoverDirectEntry {
            mover_id: 3,
            namespace: MoverLightNamespace::AnimatedBaked,
            carries_own_tile: false,
            light_index: 0,
        },
        MoverDirectEntry {
            mover_id: 3,
            namespace: MoverLightNamespace::BakeOnlyStaticLight,
            carries_own_tile: true,
            light_index: 0,
        },
    ];
    let visibility_len = (64 + 64 + 3 + 3) * MOVER_VISIBILITY_BYTES_PER_PROBE;
    let affected = (1u64 << 1) | (1 << 40);
    MoverProbeChannelSection {
        affinity_factor: AFFINITY_FACTOR,
        affinity_dims: [2, 1, 1],
        bounce_placement: MoverBouncePlacement::Resident,
        valid_probe_masks: vec![ALL_VALID, cell1_mask],
        direct_offsets: vec![0, 2, 4],
        direct_entries,
        visibility: (0..visibility_len).map(|i| i as u8).collect(),
        own_tiles: (0..(64 + 3) * MOVER_IRRADIANCE_TILE_F16_COUNT)
            .map(|i| i as u16)
            .collect(),
        bounce_offsets: vec![0, 0, 1],
        bounce_entries: vec![MoverBounceEntry {
            mover_id: 3,
            affected_probe_mask: affected,
        }],
        bounce_tiles: (0..2 * 2 * MOVER_IRRADIANCE_TILE_F16_COUNT)
            .map(|i| 0x3c00 + i as u16)
            .collect(),
        depth_records: vec![
            MoverDepthRecord {
                mover_id: 9,
                probe_index: 4,
            },
            MoverDepthRecord {
                mover_id: 9,
                probe_index: 70,
            },
        ],
        depth_tiles: (0..2 * 2 * PROBE_DEPTH_TILE_TEXELS)
            .map(|i| [i as u16, 0x4000])
            .collect(),
        ramps: vec![
            MoverLightRampRecord {
                mover_id: 3,
                start: 0.0,
                end: 1.0,
            },
            MoverLightRampRecord {
                mover_id: 9,
                start: 0.6,
                end: 0.9,
            },
        ],
    }
}

#[test]
fn section_id_is_57() {
    assert_eq!(SectionId::MoverProbeChannel as u32, 57);
    assert_eq!(SectionId::from_u32(57), Some(SectionId::MoverProbeChannel));
}

#[test]
fn every_block_round_trips_with_its_payload_reachable_by_entry() {
    let section = sample();
    let bytes = section.to_bytes();
    assert_eq!(section.encoded_len(), bytes.len());
    let restored = MoverProbeChannelSection::from_bytes(&bytes).unwrap();
    assert_eq!(restored, section);

    let direct: Vec<_> = restored.direct_entry_views().collect();
    assert_eq!(direct.len(), 4);
    assert_eq!(
        direct.iter().map(|view| view.cell).collect::<Vec<_>>(),
        [0, 0, 1, 1]
    );
    assert_eq!(direct[0].visibility.len(), 128);
    assert_eq!(direct[0].visibility[0], 0, "entry 0 starts the payload");
    assert_eq!(
        direct[1].visibility[0], 128,
        "entry 1 follows entry 0's 64 probes"
    );
    assert_eq!(
        direct[2].visibility.len(),
        6,
        "cell 1 has three valid probes"
    );
    assert_eq!(
        direct[0].own_tiles.map(<[u16]>::len),
        Some(64 * MOVER_IRRADIANCE_TILE_F16_COUNT)
    );
    assert!(direct[1].own_tiles.is_none() && direct[2].own_tiles.is_none());
    let bake_only_tiles = direct[3]
        .own_tiles
        .expect("a bake-only entry carries its tile");
    assert_eq!(bake_only_tiles.len(), 3 * MOVER_IRRADIANCE_TILE_F16_COUNT);
    assert_eq!(
        bake_only_tiles[0],
        (64 * MOVER_IRRADIANCE_TILE_F16_COUNT) as u16,
        "its tiles follow entry 0's"
    );
    assert_eq!(restored.bake_only_light_count(), 1);

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
    assert_eq!(restored.mover_ids(), [3, 9]);
    assert_eq!(restored.live_tile_count(), 2);
    assert_eq!(
        restored.ramp_of(9).map(|ramp| (ramp.start, ramp.end)),
        Some((0.6, 0.9))
    );
    assert_eq!(restored.ramp_of(4), None);
}

#[test]
fn header_and_first_direct_entry_sit_at_their_pinned_offsets() {
    let bytes = sample().to_bytes();
    assert_eq!(bytes[0], MOVER_PROBE_CHANNEL_EPOCH);
    assert_eq!(bytes[1], AFFINITY_FACTOR);
    assert_eq!(&bytes[2..14], &[2, 0, 0, 0, 1, 0, 0, 0, 1, 0, 0, 0]);
    assert_eq!(bytes[14], MoverBouncePlacement::Resident as u8);
    // Two masks, then three direct offsets, then the first 12-byte entry.
    let entry = 15 + 2 * 8 + 3 * 4;
    assert_eq!(
        &bytes[entry..entry + 12],
        &[3, 0, 0, 0, 0, MOVER_ENTRY_FLAG_OWN_TILE, 0, 0, 2, 0, 0, 0]
    );
}

// Namespace 2 names lights no other section holds, so its entries always
// carry their own tile: the decoder refuses one with the flag clear.
#[test]
fn a_bake_only_entry_without_its_own_tile_is_refused_by_name() {
    let mut bytes = sample().to_bytes();
    // Header, two masks, three offsets, then entry 3's flags byte.
    let flags = 15 + 2 * 8 + 3 * 4 + 3 * 12 + 5;
    assert_eq!(
        bytes[flags - 1],
        MoverLightNamespace::BakeOnlyStaticLight as u8
    );
    bytes[flags] = 0;
    assert_eq!(
        MoverProbeChannelSection::from_bytes(&bytes),
        Err(MoverProbeChannelError::OwnTileRequired {
            entry: 3,
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
fn each_block_alone_round_trips() {
    let full = sample();
    let without_direct = MoverProbeChannelSection {
        direct_offsets: vec![0; 3],
        direct_entries: Vec::new(),
        visibility: Vec::new(),
        own_tiles: Vec::new(),
        ..full.clone()
    };
    let bounce_only = MoverProbeChannelSection {
        depth_records: Vec::new(),
        depth_tiles: Vec::new(),
        ramps: vec![full.ramps[0]],
        ..without_direct.clone()
    };
    let depth_only = MoverProbeChannelSection {
        bounce_offsets: vec![0; 3],
        bounce_entries: Vec::new(),
        bounce_tiles: Vec::new(),
        ramps: vec![full.ramps[1]],
        ..without_direct
    };
    let direct_only = MoverProbeChannelSection {
        bounce_offsets: vec![0; 3],
        bounce_entries: Vec::new(),
        bounce_tiles: Vec::new(),
        depth_records: Vec::new(),
        depth_tiles: Vec::new(),
        ..full
    };
    for section in [bounce_only, depth_only, direct_only] {
        let restored = MoverProbeChannelSection::from_bytes(&section.to_bytes()).unwrap();
        assert_eq!(restored, section);
    }
}
