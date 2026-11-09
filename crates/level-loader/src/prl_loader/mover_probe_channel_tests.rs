use super::*;
use postretro_level_format::mover_probe_channel::{
    MOVER_IRRADIANCE_TILE_F16_COUNT, MOVER_VISIBILITY_BYTES_PER_PROBE, MoverBounceEntry,
    MoverDepthRecord, MoverDirectEntry, MoverLightRampRecord,
};
use postretro_level_format::probe_depth_tiles::PROBE_DEPTH_TILE_TEXELS;
use postretro_level_format::{SectionBlob, SectionId};

use crate::prl_loader::load_prl_with_streaming_mode_for_test;
use crate::prl_loader::tests::sample_kinematic_section;
use crate::sh_stream::ShStreamingMode;
use crate::sh_stream_tests::{write_stream_fixture_with_base, write_whole_load_fixture_with_base};
use crate::{FalloffModel, LightType, LoadedKinematicMover};

// --- Unit fixture: two affinity cells along x, decoded against plain inputs.

const GRID: [u32; 3] = [8, 4, 4];
const AFFINITY: [u32; 3] = [2, 1, 1];
const LATTICE: u32 = 8 * 4 * 4;
/// Header (15 B) and two cell masks, then the three direct offsets.
const DIRECT_OFFSETS_AT: usize = 15 + 2 * 8;
const FIRST_DIRECT_ENTRY_AT: usize = DIRECT_OFFSETS_AT + 3 * 4;

fn light(is_dynamic: bool, shadow_type: ShadowType, animated_slot: Option<u32>) -> MapLight {
    MapLight {
        origin: [0.0; 3],
        light_type: LightType::Point,
        intensity: 1.0,
        color: [1.0; 3],
        falloff_model: FalloffModel::Linear,
        falloff_range: 8.0,
        cone_angle_inner: 0.0,
        cone_angle_outer: 0.0,
        cone_direction: [0.0; 3],
        is_dynamic,
        casts_entity_shadows: false,
        animated_slot,
        tags: Vec::new(),
        cell_index: 0,
        shadow_type,
    }
}

/// The rest of the level id 57 is checked against: every probe valid at L0;
/// lights 0 static lightmap, 1 dynamic, 2 SDF, 3 animated; a one-light
/// animated roster; movers 3 and 9; no id-59 tiles; no id-50 bounce source.
struct Level {
    probes: Vec<OctahedralShProbe>,
    lights: Vec<MapLight>,
    mover_ids: Vec<u32>,
    id59_tile_count: usize,
    id50_carries_bounce: bool,
    tile_dimension: u32,
    grid_dimensions: [u32; 3],
}

impl Level {
    fn new() -> Self {
        Self {
            probes: vec![
                OctahedralShProbe {
                    validity: 1,
                    mean_distance: 0x3c00,
                    mean_sq_distance: 0x4000,
                    density_level: 0,
                    node_scale: 0,
                };
                LATTICE as usize
            ],
            lights: vec![
                light(false, ShadowType::StaticLightMap, None),
                light(true, ShadowType::StaticLightMap, None),
                light(false, ShadowType::Sdf, None),
                light(false, ShadowType::StaticLightMap, Some(0)),
            ],
            mover_ids: vec![3, 9],
            id59_tile_count: 0,
            id50_carries_bounce: false,
            tile_dimension: 6,
            grid_dimensions: GRID,
        }
    }

    fn mask(&self, cell: usize) -> u64 {
        valid_probe_mask_for_grid_cell(GRID, &self.probes, AFFINITY, cell)
    }

    fn decode(&self, bytes: &[u8]) -> Result<MoverProbeChannelSection, Error> {
        decode_mover_probe_channel(
            bytes,
            &MoverProbeChannelInputs {
                grid_dimensions: self.grid_dimensions,
                tile_dimension: self.tile_dimension,
                tile_border: 1,
                probes: &self.probes,
                lights: &self.lights,
                animated_roster_len: 1,
                id59_tile_count: self.id59_tile_count,
                mover_ids: &self.mover_ids,
                id50_carries_bounce: self.id50_carries_bounce,
            },
        )
    }

    fn refusal(&self, section: &MoverProbeChannelSection) -> Error {
        self.decode(&section.to_bytes())
            .expect_err("the section must be refused")
    }

    /// Mover 3 gates static light 0 in cell 0 with its own tile and animated
    /// light 0 in cell 1; mover 9 gates static light 0 in cell 0. Mover 3
    /// changes bounce at two probes of cell 1; mover 9 carries depth at
    /// probes 4 and 70. Mover 9 ramps over (0.6, 0.9).
    fn sample(&self) -> MoverProbeChannelSection {
        let entry = |mover_id, namespace, carries_own_tile, light_index| MoverDirectEntry {
            mover_id,
            namespace,
            carries_own_tile,
            light_index,
        };
        build(
            [self.mask(0), self.mask(1)],
            vec![
                (0, entry(3, MoverLightNamespace::StaticLight, true, 0)),
                (0, entry(9, MoverLightNamespace::StaticLight, false, 0)),
                (1, entry(3, MoverLightNamespace::AnimatedBaked, false, 0)),
            ],
            vec![(
                1,
                MoverBounceEntry {
                    mover_id: 3,
                    affected_probe_mask: 0b11,
                },
            )],
            vec![
                MoverDepthRecord {
                    mover_id: 9,
                    probe_index: 4,
                },
                MoverDepthRecord {
                    mover_id: 9,
                    probe_index: 70,
                },
            ],
        )
    }
}

/// A section with every payload sized from `masks`, ramps (0, 1) except
/// mover 9's (0.6, 0.9), and the bounce payload resident.
fn build(
    masks: [u64; 2],
    direct: Vec<(usize, MoverDirectEntry)>,
    bounce: Vec<(usize, MoverBounceEntry)>,
    depth_records: Vec<MoverDepthRecord>,
) -> MoverProbeChannelSection {
    let offsets = |cells: Vec<usize>| {
        (0..=masks.len())
            .map(|cell| cells.iter().filter(|&&c| c < cell).count() as u32)
            .collect::<Vec<_>>()
    };
    let probes = |cell: usize| masks[cell].count_ones() as usize;
    let visibility_len: usize = direct
        .iter()
        .map(|(cell, _)| probes(*cell) * MOVER_VISIBILITY_BYTES_PER_PROBE)
        .sum();
    let own_tile_len: usize = direct
        .iter()
        .filter(|(_, entry)| entry.carries_own_tile)
        .map(|(cell, _)| probes(*cell) * MOVER_IRRADIANCE_TILE_F16_COUNT)
        .sum();
    let bounce_len: usize = bounce
        .iter()
        .map(|(_, entry)| {
            entry.affected_probe_mask.count_ones() as usize * 2 * MOVER_IRRADIANCE_TILE_F16_COUNT
        })
        .sum();
    let mut section = MoverProbeChannelSection {
        affinity_factor: AFFINITY_FACTOR,
        affinity_dims: AFFINITY,
        bounce_placement: MoverBouncePlacement::Resident,
        valid_probe_masks: masks.to_vec(),
        direct_offsets: offsets(direct.iter().map(|(cell, _)| *cell).collect()),
        direct_entries: direct.iter().map(|(_, entry)| *entry).collect(),
        visibility: (0..visibility_len).map(|i| i as u8).collect(),
        own_tiles: vec![0x3c00; own_tile_len],
        bounce_offsets: offsets(bounce.iter().map(|(cell, _)| *cell).collect()),
        bounce_entries: bounce.iter().map(|(_, entry)| *entry).collect(),
        bounce_tiles: vec![0x3800; bounce_len],
        depth_tiles: vec![[0x3c00, 0x4000]; depth_records.len() * 2 * PROBE_DEPTH_TILE_TEXELS],
        depth_records,
        ramps: Vec::new(),
    };
    section.ramps = section
        .mover_ids()
        .into_iter()
        .map(|mover_id| {
            let (start, end) = if mover_id == 9 {
                (0.6, 0.9)
            } else {
                (0.0, 1.0)
            };
            MoverLightRampRecord {
                mover_id,
                start,
                end,
            }
        })
        .collect();
    section
}

#[test]
fn a_well_formed_section_decodes_against_its_level() {
    let level = Level::new();
    let section = level.sample();
    assert_eq!(level.decode(&section.to_bytes()), Ok(section));
}

#[test]
fn a_wrong_epoch_is_refused_by_name() {
    let level = Level::new();
    let mut bytes = level.sample().to_bytes();
    bytes[0] += 1;
    assert_eq!(
        level.decode(&bytes),
        Err(Error::EpochMismatch {
            found: 2,
            expected: 1
        })
    );
}

#[test]
fn a_truncated_section_is_refused_by_name() {
    let level = Level::new();
    let mut bytes = level.sample().to_bytes();
    bytes.pop();
    assert!(
        matches!(
            level.decode(&bytes),
            Err(Error::Truncated {
                what: "ramp records",
                ..
            })
        ),
        "{:?}",
        level.decode(&bytes)
    );
}

#[test]
fn trailing_bytes_are_refused_by_name() {
    let level = Level::new();
    let mut bytes = level.sample().to_bytes();
    bytes.push(0);
    assert_eq!(level.decode(&bytes), Err(Error::TrailingBytes { count: 1 }));
}

#[test]
fn a_section_with_no_entry_is_refused_as_empty() {
    let level = Level::new();
    let empty = build([level.mask(0), level.mask(1)], vec![], vec![], vec![]);
    assert_eq!(level.refusal(&empty), Error::Empty);
}

#[test]
fn an_affinity_grid_unlike_id_35s_is_refused_by_name() {
    let mut level = Level::new();
    level.grid_dimensions = [12, 4, 4];
    assert_eq!(
        level.refusal(&level.sample()),
        Error::GridMismatch {
            found: AFFINITY,
            grid: [12, 4, 4],
            expected: [3, 1, 1],
        }
    );
}

#[test]
fn an_id_34_tile_geometry_unlike_the_sections_tiles_is_refused_by_name() {
    let mut level = Level::new();
    level.tile_dimension = 8;
    assert!(matches!(
        level.refusal(&level.sample()),
        Error::TileGeometry { dimension: 8, .. }
    ));
}

#[test]
fn a_valid_probe_mask_unlike_id_34s_validity_is_refused_by_name() {
    let mut level = Level::new();
    let section = level.sample();
    // Probe 70 is cell 1's local probe 34.
    level.probes[70].validity = 0;
    assert_eq!(
        level.refusal(&section),
        Error::ValidityMismatch {
            cell: 1,
            found: u64::MAX,
            expected: !(1u64 << 34),
        }
    );
}

#[test]
fn an_unknown_bounce_placement_is_refused_by_name() {
    let level = Level::new();
    let mut bytes = level.sample().to_bytes();
    bytes[14] = 2;
    assert_eq!(
        level.decode(&bytes),
        Err(Error::UnknownBouncePlacement { value: 2 })
    );
}

#[test]
fn a_streamed_bounce_placement_with_no_bounce_entry_is_refused_by_name() {
    let level = Level::new();
    let mut section = level.sample();
    section.bounce_offsets = vec![0; 3];
    section.bounce_entries.clear();
    section.bounce_tiles.clear();
    section.bounce_placement = MoverBouncePlacement::Streamed;
    assert_eq!(level.refusal(&section), Error::StreamedBounceWithoutEntries);
}

#[test]
fn a_streamed_bounce_payload_resolves_only_against_an_id_50_source() {
    let mut level = Level::new();
    let mut section = level.sample();
    section.bounce_placement = MoverBouncePlacement::Streamed;
    section.bounce_tiles.clear();
    assert_eq!(level.refusal(&section), Error::StreamedBounceUnresolved);

    level.id50_carries_bounce = true;
    let decoded = level.decode(&section.to_bytes()).unwrap();
    assert_eq!(decoded.bounce_entry_views().next().unwrap().tiles, None);
}

#[test]
fn bad_csr_offsets_are_refused_by_name() {
    let level = Level::new();
    let bytes = level.sample().to_bytes();

    let mut nonzero_first = bytes.clone();
    nonzero_first[DIRECT_OFFSETS_AT..DIRECT_OFFSETS_AT + 4].copy_from_slice(&1u32.to_le_bytes());
    assert_eq!(
        level.decode(&nonzero_first),
        Err(Error::CsrOffsets {
            block: "direct",
            index: 0,
            offset: 1,
            previous: 0,
        })
    );

    // Offsets [0, 2, 3] become [0, 4, 3].
    let mut decreasing = bytes;
    decreasing[DIRECT_OFFSETS_AT + 4..DIRECT_OFFSETS_AT + 8].copy_from_slice(&4u32.to_le_bytes());
    assert_eq!(
        level.decode(&decreasing),
        Err(Error::CsrOffsets {
            block: "direct",
            index: 2,
            offset: 3,
            previous: 4,
        })
    );
}

#[test]
fn a_set_reserved_bit_is_refused_by_name() {
    let level = Level::new();
    let section = level.sample();
    let bytes = section.to_bytes();

    let mut flag = bytes.clone();
    flag[FIRST_DIRECT_ENTRY_AT + 5] |= 0b10;
    assert_eq!(
        level.decode(&flag),
        Err(Error::ReservedBits {
            what: "direct entry flags",
            index: 0,
            value: 0b11,
        })
    );

    let mut reserved_u16 = bytes.clone();
    reserved_u16[FIRST_DIRECT_ENTRY_AT + 7] = 1;
    assert_eq!(
        level.decode(&reserved_u16),
        Err(Error::ReservedBits {
            what: "direct entry reserved u16",
            index: 0,
            value: 0x100,
        })
    );

    let bounce_entry_at = FIRST_DIRECT_ENTRY_AT
        + section.direct_entries.len() * 12
        + section.visibility.len()
        + section.own_tiles.len() * 2
        + 3 * 4;
    let mut reserved_u32 = bytes;
    reserved_u32[bounce_entry_at + 4] = 1;
    assert_eq!(
        level.decode(&reserved_u32),
        Err(Error::ReservedBits {
            what: "bounce entry reserved u32",
            index: 0,
            value: 1,
        })
    );
}

#[test]
fn an_unknown_light_namespace_is_refused_by_name() {
    let level = Level::new();
    let mut bytes = level.sample().to_bytes();
    bytes[FIRST_DIRECT_ENTRY_AT + 4] = 2;
    assert_eq!(
        level.decode(&bytes),
        Err(Error::UnknownLightNamespace { entry: 0, value: 2 })
    );
}

#[test]
fn unordered_or_duplicate_entries_are_refused_by_name() {
    let level = Level::new();

    let mut unordered = level.sample();
    unordered.direct_entries.swap(0, 1);
    let mut duplicate = level.sample();
    duplicate.direct_entries[1].mover_id = 3;
    for section in [unordered, duplicate] {
        assert_eq!(
            level.refusal(&section),
            Error::EntryOrder {
                block: "direct",
                cell: 0,
                entry: 1,
            }
        );
    }

    let mut bounce = level.sample();
    bounce.bounce_offsets = vec![0, 0, 2];
    bounce.bounce_entries.push(bounce.bounce_entries[0]);
    bounce
        .bounce_tiles
        .extend_from_slice(&bounce.bounce_tiles.clone());
    assert_eq!(
        level.refusal(&bounce),
        Error::EntryOrder {
            block: "bounce",
            cell: 1,
            entry: 1,
        }
    );

    let mut depth = level.sample();
    depth.depth_records.swap(0, 1);
    assert_eq!(level.refusal(&depth), Error::DepthRecordOrder { record: 1 });
    let mut repeated = level.sample();
    repeated.depth_records[1].probe_index = 4;
    assert_eq!(
        level.refusal(&repeated),
        Error::DepthRecordOrder { record: 1 }
    );
}

#[test]
fn a_bounce_entry_naming_a_non_valid_probe_is_refused_by_name() {
    let mut level = Level::new();
    // Cell 1's local probe 34 (dense probe 70) is invalid.
    level.probes[70].validity = 0;
    let mut outside = level.sample();
    outside.bounce_entries[0].affected_probe_mask = (1 << 34) | 1;
    let mut empty = level.sample();
    empty.bounce_entries[0].affected_probe_mask = 0;
    empty.bounce_tiles.clear();
    for (section, mask) in [(outside, (1u64 << 34) | 1), (empty, 0)] {
        assert_eq!(
            level.refusal(&section),
            Error::BounceProbeNotValid {
                cell: 1,
                entry: 0,
                mask,
                valid: !(1u64 << 34),
            }
        );
    }
}

#[test]
fn a_ramp_record_naming_the_wrong_mover_is_refused_by_name() {
    let level = Level::new();
    let mut section = level.sample();
    section.ramps[1].mover_id = 10;
    assert_eq!(
        level.refusal(&section),
        Error::RampMover {
            record: 1,
            found: 10,
            expected: 9,
        }
    );
}

#[test]
fn a_ramp_outside_zero_to_one_or_not_ascending_is_refused_by_name() {
    let level = Level::new();
    for (start, end) in [
        (0.5, 0.5),
        (0.9, 0.6),
        (-0.1, 0.5),
        (0.2, 1.1),
        (f32::NAN, 0.5),
        (0.0, f32::INFINITY),
    ] {
        let mut section = level.sample();
        section.ramps[1] = MoverLightRampRecord {
            mover_id: 9,
            start,
            end,
        };
        match level.refusal(&section) {
            Error::RampRange { mover_id: 9, .. } => {}
            other => panic!("({start}, {end}): {other:?}"),
        }
    }
}

#[test]
fn a_mover_absent_from_id_43_is_refused_by_name() {
    let mut level = Level::new();
    level.mover_ids = vec![3];
    assert_eq!(
        level.refusal(&level.sample()),
        Error::UnresolvedMover { mover_id: 9 }
    );
}

#[test]
fn a_light_outside_its_namespace_is_refused_by_name() {
    let level = Level::new();
    // Light 99 is past id 18; 1 is dynamic, 2 SDF, 3 animated.
    for light_index in [99, 1, 2, 3] {
        let mut section = level.sample();
        section.direct_entries[0].light_index = light_index;
        assert!(
            matches!(
                level.refusal(&section),
                Error::LightOutOfNamespace {
                    entry: 0,
                    namespace: MoverLightNamespace::StaticLight,
                    light_index: index,
                    ..
                } if index == light_index
            ),
            "static light {light_index}"
        );
    }
    let mut section = level.sample();
    section.direct_entries[2].light_index = 1;
    assert!(matches!(
        level.refusal(&section),
        Error::LightOutOfNamespace {
            entry: 2,
            namespace: MoverLightNamespace::AnimatedBaked,
            light_index: 1,
            ..
        }
    ));
}

#[test]
fn a_depth_record_naming_a_probe_outside_the_lattice_is_refused_by_name() {
    let level = Level::new();
    let mut section = level.sample();
    section.depth_records[1].probe_index = LATTICE;
    assert_eq!(
        level.refusal(&section),
        Error::DepthProbeOutOfRange {
            record: 1,
            probe: LATTICE,
            lattice: u64::from(LATTICE),
        }
    );
}

#[test]
fn a_depth_record_naming_an_invalid_probe_is_refused_by_name() {
    let mut level = Level::new();
    level.probes[70].validity = 0;
    // Masks follow id 34, so the refusal is the depth record's own.
    let section = level.sample();
    assert_eq!(
        level.refusal(&section),
        Error::DepthProbeNotValid {
            record: 1,
            probe: 70,
        }
    );
}

#[test]
fn live_tiles_past_the_tile_reference_range_are_refused_by_name_and_exactly_at_it_load() {
    let mut level = Level::new();
    let section = level.sample();
    let max = MAX_PROBE_DEPTH_TILES as usize;
    level.id59_tile_count = max - 2;
    level.decode(&section.to_bytes()).unwrap();
    level.id59_tile_count = max - 1;
    assert_eq!(
        level.refusal(&section),
        Error::TileRange {
            id59: max - 1,
            live: 2,
            max: MAX_PROBE_DEPTH_TILES,
        }
    );
}

#[test]
fn a_cell_with_entries_over_coarse_base_storage_is_refused_by_name() {
    let mut level = Level::new();
    // Cell 0 (x 0..4) is stored at L1; it holds direct entries.
    for (index, probe) in level.probes.iter_mut().enumerate() {
        if index % 8 < 4 {
            probe.density_level = 1;
        }
    }
    assert!(matches!(
        level.refusal(&level.sample()),
        Error::StorageCeiling { .. }
    ));
}

#[test]
fn ramps_reach_only_the_movers_the_section_names() {
    let level = Level::new();
    let section = level.sample();
    let mut geometry = KinematicGeometry {
        movers: [3, 9, 12]
            .into_iter()
            .map(|mover_id| LoadedKinematicMover {
                mover_id,
                ..LoadedKinematicMover::from(sample_kinematic_section().movers[0].clone())
            })
            .collect(),
        waypoints: Vec::new(),
    };
    apply_light_ramps(&section, &mut geometry);
    let ramps: Vec<_> = geometry
        .movers
        .iter()
        .map(|mover| mover.light_ramp)
        .collect();
    assert_eq!(
        ramps,
        [
            MoverLightRamp::IDENTITY,
            MoverLightRamp {
                start: 0.6,
                end: 0.9
            },
            MoverLightRamp::IDENTITY,
        ]
    );
}

// --- Through the load: the stream fixture (16×16×4 probes, every probe
// valid, static light 0, one animated descriptor) plus id 43's mover 7.

const FIXTURE_CELLS: usize = 16;

fn blob(section_id: SectionId, data: Vec<u8>) -> SectionBlob {
    SectionBlob {
        section_id: section_id as u32,
        version: 1,
        data,
    }
}

/// Mover 7 gates static light 0 in cell 0, changes bounce at one probe of
/// cell 5, and carries depth at probe 17; it ramps over (0.25, 0.75).
fn fixture_section() -> MoverProbeChannelSection {
    let mut direct_offsets = vec![0u32];
    direct_offsets.extend(std::iter::repeat_n(1, FIXTURE_CELLS));
    let bounce_offsets: Vec<u32> = (0..=FIXTURE_CELLS)
        .map(|cell| u32::from(cell > 5))
        .collect();
    MoverProbeChannelSection {
        affinity_factor: AFFINITY_FACTOR,
        affinity_dims: [4, 4, 1],
        bounce_placement: MoverBouncePlacement::Resident,
        valid_probe_masks: vec![u64::MAX; FIXTURE_CELLS],
        direct_offsets,
        direct_entries: vec![MoverDirectEntry {
            mover_id: 7,
            namespace: MoverLightNamespace::StaticLight,
            carries_own_tile: false,
            light_index: 0,
        }],
        visibility: vec![128; 64 * MOVER_VISIBILITY_BYTES_PER_PROBE],
        own_tiles: Vec::new(),
        bounce_offsets,
        bounce_entries: vec![MoverBounceEntry {
            mover_id: 7,
            affected_probe_mask: 1,
        }],
        bounce_tiles: vec![0x3800; 2 * MOVER_IRRADIANCE_TILE_F16_COUNT],
        depth_records: vec![MoverDepthRecord {
            mover_id: 7,
            probe_index: 17,
        }],
        depth_tiles: vec![[0x3c00, 0x4000]; 2 * PROBE_DEPTH_TILE_TEXELS],
        ramps: vec![MoverLightRampRecord {
            mover_id: 7,
            start: 0.25,
            end: 0.75,
        }],
    }
}

/// Every id-34 path: the streamed base, the `off` escape over the same file,
/// and a level with no ids 49/50 that loads its SH whole. Each load is also
/// cross-checked against the staged parse.
fn load_every_way(id57: Option<Vec<u8>>) -> Vec<Result<crate::LevelWorld, PrlLoadError>> {
    let extra = || {
        let mut sections = vec![blob(
            SectionId::KinematicGeometry,
            sample_kinematic_section().to_bytes(),
        )];
        sections.extend(
            id57.clone()
                .map(|data| blob(SectionId::MoverProbeChannel, data)),
        );
        sections
    };
    let streamed = write_stream_fixture_with_base(|_| {}, extra());
    let whole = write_whole_load_fixture_with_base(|_| {}, extra());
    [
        (&streamed, ShStreamingMode::Async),
        (&streamed, ShStreamingMode::Off),
        (&whole, ShStreamingMode::Async),
    ]
    .into_iter()
    .map(|(fixture, mode)| {
        load_prl_with_streaming_mode_for_test(fixture.path.to_str().unwrap(), mode)
    })
    .collect()
}

#[test]
fn the_loaded_section_and_its_ramp_reach_the_level_on_every_id_34_path() {
    let section = fixture_section();
    for world in load_every_way(Some(section.to_bytes())) {
        let world = world.unwrap();
        assert_eq!(world.mover_probe_channel(), Some(&section));
        assert_eq!(
            world.kinematic_geometry.movers[0].light_ramp,
            MoverLightRamp {
                start: 0.25,
                end: 0.75
            }
        );
    }
}

#[test]
fn a_level_without_the_section_loads_identity_ramps_and_no_channel() {
    for world in load_every_way(None) {
        let world = world.unwrap();
        assert_eq!(world.mover_probe_channel(), None);
        assert_eq!(
            world.kinematic_geometry.movers[0].light_ramp,
            MoverLightRamp::IDENTITY
        );
    }
}

#[test]
fn an_unresolved_section_fails_the_load_by_name_on_every_id_34_path() {
    let mut unresolved_mover = fixture_section();
    for ramp in &mut unresolved_mover.ramps {
        ramp.mover_id = 8;
    }
    unresolved_mover.direct_entries[0].mover_id = 8;
    unresolved_mover.bounce_entries[0].mover_id = 8;
    unresolved_mover.depth_records[0].mover_id = 8;
    for result in load_every_way(Some(unresolved_mover.to_bytes())) {
        match result {
            Err(PrlLoadError::MoverProbeChannel(Error::UnresolvedMover { mover_id: 8 })) => {}
            other => panic!("expected the unresolved mover refusal, got {other:?}"),
        }
    }

    // No id 50 in this build carries a bounce source, so a streamed payload
    // cannot resolve on any path.
    let mut streamed = fixture_section();
    streamed.bounce_placement = MoverBouncePlacement::Streamed;
    streamed.bounce_tiles.clear();
    for result in load_every_way(Some(streamed.to_bytes())) {
        match result {
            Err(PrlLoadError::MoverProbeChannel(Error::StreamedBounceUnresolved)) => {}
            other => panic!("expected the streamed-bounce refusal, got {other:?}"),
        }
    }
}
