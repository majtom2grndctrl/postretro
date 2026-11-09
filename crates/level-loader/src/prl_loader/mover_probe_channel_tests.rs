use super::*;
use postretro_level_format::mover_probe_channel::MoverPayloadPlacement::{Resident, Streamed};
use postretro_level_format::mover_probe_channel::{
    BounceEntryParts, DepthRecordParts, DirectRecordParts, MOVER_IRRADIANCE_TILE_F16_COUNT,
    MoverBounceEntry, MoverChannelCell, MoverDepthRecord, MoverDirectRecord, MoverLightRampRecord,
    MoverProbeChannelParts,
};
use postretro_level_format::probe_depth_tiles::PROBE_DEPTH_TILE_TEXELS;
use postretro_level_format::{SectionBlob, SectionId};

use crate::prl_loader::load_prl_with_streaming_mode_for_test;
use crate::prl_loader::tests::sample_kinematic_section;
use crate::sh_stream::ShStreamingMode;
use crate::sh_stream_tests::{
    write_stream_fixture_with_base, write_stream_fixture_with_mover_channel,
    write_whole_load_fixture_with_base,
};
use crate::{FalloffModel, LightType, LoadedKinematicMover};

// --- Unit fixture: two affinity cells along x, decoded against plain inputs.

const GRID: [u32; 3] = [8, 4, 4];
const AFFINITY: [u32; 3] = [2, 1, 1];
const LATTICE: u32 = 8 * 4 * 4;
/// Header (16 B), then the listed cell count and two 12-byte cell records.
const CELLS_AT: usize = 16 + 4;
/// Then the three direct offsets, then the first 16-byte term record.
const DIRECT_OFFSETS_AT: usize = CELLS_AT + 2 * 12;
const FIRST_RECORD_AT: usize = DIRECT_OFFSETS_AT + 3 * 4;

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

/// A term record's parts: own tiles sized from the mask when flagged.
fn direct(cell: u32, record: MoverDirectRecord, movers: Vec<(u32, Vec<u8>)>) -> DirectRecordParts {
    let tiles = if record.carries_own_tile {
        record.masked_probe_count() * MOVER_IRRADIANCE_TILE_F16_COUNT
    } else {
        0
    };
    DirectRecordParts {
        cell,
        record,
        movers,
        own_tiles: vec![0x3c00; tiles],
    }
}

fn bounce(cell: u32, mover_id: u32, affected_probe_mask: u64) -> BounceEntryParts {
    BounceEntryParts {
        cell,
        entry: MoverBounceEntry {
            mover_id,
            affected_probe_mask,
        },
        tiles: vec![
            0x3800;
            affected_probe_mask.count_ones() as usize * 2 * MOVER_IRRADIANCE_TILE_F16_COUNT
        ],
    }
}

fn depth(mover_id: u32, probe_index: u32) -> DepthRecordParts {
    DepthRecordParts {
        record: MoverDepthRecord {
            mover_id,
            probe_index,
        },
        tiles: vec![[0x3c00, 0x4000]; 2 * PROBE_DEPTH_TILE_TEXELS],
    }
}

/// Ramps (0, 1) except mover 9's (0.6, 0.9).
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

/// The rest of the level id 57 is checked against: every probe valid at L0;
/// lights 0 static lightmap, 1 dynamic, 2 SDF, 3 animated; a one-light
/// animated roster; movers 3 and 9; no id-59 tiles; no id-50 source for id 57.
struct Level {
    probes: Vec<OctahedralShProbe>,
    lights: Vec<MapLight>,
    mover_ids: Vec<u32>,
    id59_tile_count: usize,
    id50_carries_payload: bool,
    borrowed_families: BorrowedFamilies,
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
            id50_carries_payload: false,
            borrowed_families: BorrowedFamilies {
                static_terms: true,
                animated_terms: true,
            },
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
                id50_carries_payload: self.id50_carries_payload,
                borrowed_families: self.borrowed_families,
            },
        )
    }

    fn refusal(&self, section: &MoverProbeChannelSection) -> Error {
        self.decode(&section.to_bytes())
            .expect_err("the section must be refused")
    }

    /// Movers 3 and 9 gate static light 0 at local probes 0 and 2 of cell 0,
    /// which carries its own tile once; mover 3 gates animated light 0 at
    /// local probe 0 and bake-only light 0 (with its own tile) at local
    /// probes 1 and 2 of cell 1. Mover 3 changes bounce at two probes of
    /// cell 1; mover 9 carries depth at probes 4 and 70. Mover 9 ramps over
    /// (0.6, 0.9).
    fn parts(&self) -> MoverProbeChannelParts {
        use MoverLightNamespace::{AnimatedBaked, BakeOnlyStaticLight, StaticLight};
        MoverProbeChannelParts {
            affinity_dims: AFFINITY,
            direct: vec![
                direct(
                    0,
                    record(StaticLight, 0, true, 0b101),
                    vec![(3, vec![10, 255, 255, 255]), (9, vec![255, 255, 30, 255])],
                ),
                direct(
                    1,
                    record(AnimatedBaked, 0, false, 1),
                    vec![(3, vec![0, 128])],
                ),
                direct(
                    1,
                    record(BakeOnlyStaticLight, 0, true, 0b110),
                    vec![(3, vec![1, 2, 3, 4])],
                ),
            ],
            bounce: vec![bounce(1, 3, 0b11)],
            depth: vec![depth(9, 4), depth(9, 70)],
        }
    }

    fn assemble(&self, parts: MoverProbeChannelParts) -> MoverProbeChannelSection {
        MoverProbeChannelSection::assemble(parts, |cell| self.mask(cell as usize), ramp)
            .unwrap()
            .expect("the parts hold entries")
    }

    fn sample(&self) -> MoverProbeChannelSection {
        self.assemble(self.parts())
    }
}

#[test]
fn a_well_formed_section_decodes_against_its_level() {
    let level = Level::new();
    let section = level.sample();
    assert_eq!(level.decode(&section.to_bytes()), Ok(section));
}

/// A record without its own tile borrows its term: a PRL without that family
/// is refused naming it. A record carrying its tile needs no family.
#[test]
fn a_record_borrowing_from_an_absent_family_is_refused_naming_the_family() {
    let mut level = Level::new();
    let section = level.sample();
    // The static record carries its own tile.
    level.borrowed_families.static_terms = false;
    assert_eq!(level.decode(&section.to_bytes()), Ok(section.clone()));
    // The animated record (record 1) borrows from id 45.
    level.borrowed_families = BorrowedFamilies {
        static_terms: true,
        animated_terms: false,
    };
    let refusal = level.refusal(&section);
    assert_eq!(
        refusal,
        Error::BorrowedFamilyAbsent {
            record: 1,
            family: "AnimatedDirectShDeltaVolumes (id 45)",
        }
    );
    assert!(
        refusal
            .to_string()
            .contains("AnimatedDirectShDeltaVolumes (id 45)")
    );
}

#[test]
fn a_wrong_epoch_is_refused_by_name() {
    let level = Level::new();
    let mut bytes = level.sample().to_bytes();
    bytes[0] += 1;
    assert_eq!(
        level.decode(&bytes),
        Err(Error::EpochMismatch {
            found: 4,
            expected: 3
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
    let mut empty = level.sample();
    empty.depth_records.clear();
    empty.depth_tiles.clear();
    empty.cells.clear();
    empty.direct_offsets = vec![0];
    empty.direct_records.clear();
    empty.mover_offsets = vec![0];
    empty.direct_movers.clear();
    empty.visibility.clear();
    empty.own_tiles.clear();
    empty.bounce_offsets = vec![0];
    empty.bounce_entries.clear();
    empty.bounce_tiles.clear();
    empty.ramps.clear();
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
fn an_unknown_placement_is_refused_by_name_for_its_block() {
    let level = Level::new();
    for (at, block) in [
        (14, MoverChannelBlock::Direct),
        (15, MoverChannelBlock::Bounce),
    ] {
        let mut bytes = level.sample().to_bytes();
        bytes[at] = 2;
        assert_eq!(
            level.decode(&bytes),
            Err(Error::UnknownPlacement { block, value: 2 })
        );
    }
}

#[test]
fn a_streamed_placement_with_no_entry_in_its_block_is_refused_by_name() {
    let level = Level::new();
    let mut parts = level.parts();
    parts.bounce.clear();
    let mut section = level.assemble(parts);
    section.bounce_placement = Streamed;
    assert_eq!(
        level.refusal(&section),
        Error::StreamedBlockWithoutEntries {
            block: MoverChannelBlock::Bounce
        }
    );
    let mut parts = level.parts();
    parts.direct.clear();
    let mut section = level.assemble(parts);
    section.direct_placement = Streamed;
    assert_eq!(
        level.refusal(&section),
        Error::StreamedBlockWithoutEntries {
            block: MoverChannelBlock::Direct
        }
    );
}

// A streamed payload resolves only when id 50 lists id 57 as a source; the
// refusal names the block.
#[test]
fn a_streamed_payload_resolves_only_against_an_id_50_source() {
    for (direct, bounce, block) in [
        (Resident, Streamed, MoverChannelBlock::Bounce),
        (Streamed, Resident, MoverChannelBlock::Direct),
        (Streamed, Streamed, MoverChannelBlock::Direct),
    ] {
        let mut level = Level::new();
        let mut section = level.sample();
        section.direct_placement = direct;
        section.bounce_placement = bounce;
        assert_eq!(
            level.refusal(&section),
            Error::StreamedPayloadUnresolved { block }
        );
        level.id50_carries_payload = true;
        let decoded = level.decode(&section.to_bytes()).unwrap();
        assert_eq!(
            decoded.payload_held(MoverChannelBlock::Bounce),
            bounce == Resident
        );
        assert_eq!(
            decoded.payload_held(MoverChannelBlock::Direct),
            direct == Resident
        );
    }
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

    // Offsets [0, 1, 3] become [0, 5, 3].
    let mut decreasing = bytes.clone();
    decreasing[DIRECT_OFFSETS_AT + 4..DIRECT_OFFSETS_AT + 8].copy_from_slice(&5u32.to_le_bytes());
    assert_eq!(
        level.decode(&decreasing),
        Err(Error::CsrOffsets {
            block: "direct",
            index: 2,
            offset: 3,
            previous: 5,
        })
    );

    // Mover offsets [0, 2, 3, 4] become [0, 2, 1, 4].
    let movers_at = FIRST_RECORD_AT + 3 * 16;
    let mut movers = bytes;
    movers[movers_at + 8..movers_at + 12].copy_from_slice(&1u32.to_le_bytes());
    assert_eq!(
        level.decode(&movers),
        Err(Error::CsrOffsets {
            block: "direct movers",
            index: 2,
            offset: 1,
            previous: 2,
        })
    );
}

#[test]
fn a_set_reserved_bit_is_refused_by_name() {
    let level = Level::new();
    let section = level.sample();
    let bytes = section.to_bytes();

    let mut flag = bytes.clone();
    flag[FIRST_RECORD_AT + 1] |= 0b10;
    assert_eq!(
        level.decode(&flag),
        Err(Error::ReservedBits {
            what: "direct record flags",
            index: 0,
            value: 0b11,
        })
    );

    let mut reserved_u16 = bytes.clone();
    reserved_u16[FIRST_RECORD_AT + 3] = 1;
    assert_eq!(
        level.decode(&reserved_u16),
        Err(Error::ReservedBits {
            what: "direct record reserved u16",
            index: 0,
            value: 0x100,
        })
    );

    let parts = section.byte_breakdown();
    let bounce_entry_at = parts.header + parts.cell_index + parts.direct_block() + 3 * 4;
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
    bytes[FIRST_RECORD_AT] = 3;
    assert_eq!(
        level.decode(&bytes),
        Err(Error::UnknownLightNamespace {
            record: 0,
            value: 3
        })
    );
}

#[test]
fn an_unsorted_or_duplicate_cell_list_is_refused_by_name() {
    let level = Level::new();
    let mut unsorted = level.sample();
    unsorted.cells.swap(0, 1);
    assert_eq!(
        level.refusal(&unsorted),
        Error::CellOrder {
            listed: 1,
            cell: 0,
            previous: 1,
        }
    );
    let mut duplicate = level.sample();
    duplicate.cells[1].cell = 0;
    assert_eq!(
        level.refusal(&duplicate),
        Error::CellOrder {
            listed: 1,
            cell: 0,
            previous: 0,
        }
    );
}

#[test]
fn a_listed_cell_outside_the_affinity_grid_is_refused_by_name() {
    let level = Level::new();
    let mut section = level.sample();
    section.cells[1].cell = 2;
    assert_eq!(
        level.refusal(&section),
        Error::CellOutOfRange {
            listed: 1,
            cell: 2,
            cells: 2,
        }
    );
}

#[test]
fn a_listed_cell_with_no_entry_is_refused_by_name() {
    let level = Level::new();
    let mut parts = level.parts();
    parts.direct.truncate(1);
    parts.bounce.clear();
    let mut section = level.assemble(parts);
    section.cells.push(MoverChannelCell {
        cell: 1,
        valid_probe_mask: level.mask(1),
    });
    section.direct_offsets.push(1);
    section.bounce_offsets.push(0);
    assert_eq!(
        level.refusal(&section),
        Error::CellWithoutEntries { cell: 1 }
    );
}

#[test]
fn unordered_or_duplicate_entries_are_refused_by_name() {
    let level = Level::new();

    // Cell 1 holds (animated 0) then (bake-only 0).
    let mut unordered = level.sample();
    unordered.direct_records.swap(1, 2);
    let mut duplicate = level.sample();
    duplicate.direct_records[2].namespace = MoverLightNamespace::AnimatedBaked;
    for section in [unordered, duplicate] {
        assert_eq!(
            level.refusal(&section),
            Error::EntryOrder {
                block: "direct",
                cell: 1,
                entry: 2,
            }
        );
    }

    let mut parts = level.parts();
    parts.bounce.push(bounce(1, 3, 0b11));
    assert_eq!(
        level.refusal(&level.assemble(parts)),
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

// A record's movers ascend by id with no repeats.
#[test]
fn unordered_or_duplicate_record_movers_are_refused_by_name() {
    let level = Level::new();
    let mut unordered = level.sample();
    unordered.direct_movers.swap(0, 1);
    let mut duplicate = level.sample();
    duplicate.direct_movers[1] = 3;
    for section in [unordered, duplicate] {
        assert_eq!(
            level.refusal(&section),
            Error::MoverOrder {
                cell: 0,
                record: 0,
                entry: 1,
            }
        );
    }
}

#[test]
fn a_record_with_no_mover_is_refused_by_name() {
    let level = Level::new();
    let mut section = level.sample();
    // Record 1's one mover entry (index 2) and its two visibility bytes,
    // which follow record 0's 2 movers × 2 probes × 2 bytes.
    section.direct_movers.remove(2);
    section.mover_offsets = vec![0, 2, 2, 3];
    section.visibility.drain(8..10);
    assert_eq!(
        level.refusal(&section),
        Error::RecordWithoutMovers { cell: 1, record: 1 }
    );
}

#[test]
fn an_empty_probe_mask_is_refused_by_name() {
    let level = Level::new();
    let mut parts = level.parts();
    parts.direct[0] = direct(
        0,
        record(MoverLightNamespace::StaticLight, 0, true, 0),
        vec![(3, Vec::new())],
    );
    assert_eq!(
        level.refusal(&level.assemble(parts)),
        Error::ProbeMaskEmpty {
            block: "direct",
            cell: 0,
            entry: 0,
        }
    );

    let mut parts = level.parts();
    parts.bounce[0] = bounce(1, 3, 0);
    assert_eq!(
        level.refusal(&level.assemble(parts)),
        Error::ProbeMaskEmpty {
            block: "bounce",
            cell: 1,
            entry: 0,
        }
    );
}

#[test]
fn a_probe_mask_naming_a_non_valid_probe_is_refused_by_name() {
    let mut level = Level::new();
    // Cell 1's local probe 34 (dense probe 70) is invalid.
    level.probes[70].validity = 0;
    let mut parts = level.parts();
    parts.direct[2].record.probe_mask = (1 << 1) | (1 << 34);
    assert_eq!(
        level.refusal(&level.assemble(parts)),
        Error::ProbeMaskNotValid {
            block: "direct",
            cell: 1,
            entry: 2,
            mask: (1 << 1) | (1 << 34),
            valid: !(1u64 << 34),
        }
    );

    let mut parts = level.parts();
    parts.bounce[0] = bounce(1, 3, (1 << 34) | 1);
    assert_eq!(
        level.refusal(&level.assemble(parts)),
        Error::ProbeMaskNotValid {
            block: "bounce",
            cell: 1,
            entry: 0,
            mask: (1 << 34) | 1,
            valid: !(1u64 << 34),
        }
    );
}

// The mask is exactly the union of the movers' q < 255 probes: a masked
// probe where every mover stores 255 at both termini is refused.
#[test]
fn a_masked_probe_no_mover_gates_is_refused_by_name() {
    let level = Level::new();
    let mut parts = level.parts();
    parts.direct[0] = direct(
        0,
        record(MoverLightNamespace::StaticLight, 0, true, 0b111),
        vec![
            (3, vec![10, 255, 255, 255, 255, 255]),
            (9, vec![255, 255, 255, 255, 30, 255]),
        ],
    );
    assert_eq!(
        level.refusal(&level.assemble(parts)),
        Error::ProbeMaskNotTight {
            cell: 0,
            record: 0,
            probe: 1,
        }
    );
}

#[test]
fn a_mover_entry_that_gates_nothing_is_refused_by_name() {
    let level = Level::new();
    let mut section = level.sample();
    // Mover 9's two pairs in record 0 follow mover 3's.
    section.visibility[4..8].fill(255);
    assert_eq!(
        level.refusal(&section),
        Error::MoverGatesNothing {
            cell: 0,
            record: 0,
            entry: 1,
            mover_id: 9,
        }
    );
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
        section.direct_records[0].light_index = light_index;
        assert!(
            matches!(
                level.refusal(&section),
                Error::LightOutOfNamespace {
                    record: 0,
                    namespace: MoverLightNamespace::StaticLight,
                    light_index: index,
                    ..
                } if index == light_index
            ),
            "static light {light_index}"
        );
    }
    let mut section = level.sample();
    section.direct_records[1].light_index = 1;
    assert!(matches!(
        level.refusal(&section),
        Error::LightOutOfNamespace {
            record: 1,
            namespace: MoverLightNamespace::AnimatedBaked,
            light_index: 1,
            ..
        }
    ));
    // The section names one bake-only light, so its roster index must be 0.
    let mut section = level.sample();
    section.direct_records[2].light_index = 1;
    assert!(matches!(
        level.refusal(&section),
        Error::LightOutOfNamespace {
            record: 2,
            namespace: MoverLightNamespace::BakeOnlyStaticLight,
            light_index: 1,
            ..
        }
    ));
}

// A bake-only roster is dense: two lights load as indices 0 and 1, and a
// gap (0 and 2) refuses the level.
#[test]
fn a_bake_only_roster_with_a_gap_is_refused_by_name() {
    let level = Level::new();
    let second = |light_index| {
        direct(
            0,
            record(
                MoverLightNamespace::BakeOnlyStaticLight,
                light_index,
                true,
                1,
            ),
            vec![(3, vec![7, 255])],
        )
    };
    let mut parts = level.parts();
    parts.direct.push(second(1));
    level
        .decode(&level.assemble(parts).to_bytes())
        .expect("a dense roster loads");

    let mut parts = level.parts();
    parts.direct.push(second(2));
    // Cell 0 holds (static 0) then (bake-only 2).
    assert!(matches!(
        level.refusal(&level.assemble(parts)),
        Error::LightOutOfNamespace {
            record: 1,
            namespace: MoverLightNamespace::BakeOnlyStaticLight,
            light_index: 2,
            ..
        }
    ));
}

// Namespace 2 has no id-41 or id-45 entry to borrow a tile from, so a
// record without its own refuses the level.
#[test]
fn a_bake_only_record_without_its_own_tile_is_refused_by_name() {
    let level = Level::new();
    let mut section = level.sample();
    section.direct_records[2].carries_own_tile = false;
    section
        .own_tiles
        .truncate(section.own_tiles.len() - 2 * MOVER_IRRADIANCE_TILE_F16_COUNT);
    assert_eq!(
        level.refusal(&section),
        Error::OwnTileRequired {
            record: 2,
            namespace: MoverLightNamespace::BakeOnlyStaticLight,
        }
    );
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
    // Cell 0 (x 0..4) is stored at L1; it holds direct records.
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

fn blob(section_id: SectionId, data: Vec<u8>) -> SectionBlob {
    SectionBlob {
        section_id: section_id as u32,
        version: 1,
        data,
    }
}

/// Mover 7 gates static light 0 at one probe of cell 0, changes bounce at
/// one probe of cell 5, and carries depth at probe 17; it ramps over (0.25,
/// 0.75).
fn fixture_section() -> MoverProbeChannelSection {
    MoverProbeChannelSection::assemble(
        MoverProbeChannelParts {
            affinity_dims: [4, 4, 1],
            direct: vec![direct(
                0,
                record(MoverLightNamespace::StaticLight, 0, false, 1),
                vec![(7, vec![128, 128])],
            )],
            bounce: vec![bounce(5, 7, 1)],
            depth: vec![depth(7, 17)],
        },
        |_| u64::MAX,
        |mover_id| {
            Ok::<_, ()>(MoverLightRampRecord {
                mover_id,
                start: 0.25,
                end: 0.75,
            })
        },
    )
    .unwrap()
    .expect("the fixture holds entries")
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
    unresolved_mover.direct_movers[0] = 8;
    unresolved_mover.bounce_entries[0].mover_id = 8;
    unresolved_mover.depth_records[0].mover_id = 8;
    for result in load_every_way(Some(unresolved_mover.to_bytes())) {
        match result {
            Err(PrlLoadError::MoverProbeChannel(Error::UnresolvedMover { mover_id: 8 })) => {}
            other => panic!("expected the unresolved mover refusal, got {other:?}"),
        }
    }

    // No id 50 in this build lists id 57 as a source, so a streamed payload
    // cannot resolve on any path, streamed or whole.
    let mut streamed = fixture_section();
    streamed.bounce_placement = Streamed;
    for result in load_every_way(Some(streamed.to_bytes())) {
        match result {
            Err(PrlLoadError::MoverProbeChannel(Error::StreamedPayloadUnresolved {
                block: MoverChannelBlock::Bounce,
            })) => {}
            other => panic!("expected the streamed-bounce refusal, got {other:?}"),
        }
    }
}

/// `section` with each block's payload placed as given, payload held (as
/// the compiler holds it).
fn placed(
    section: &MoverProbeChannelSection,
    direct: MoverPayloadPlacement,
    bounce: MoverPayloadPlacement,
) -> MoverProbeChannelSection {
    let mut placed = section.clone();
    placed.direct_placement = direct;
    placed.bounce_placement = bounce;
    placed
}

// Task 7b: a payload placed with id 50's cells loads whole under `off`
// (monolithic and staged parse alike, through the load's cross-check) into
// exactly the payload a resident placement holds, placements kept. Under
// streaming it stays with id 50: the section holds no streamed payload, and
// each listed cell read through the chunk's rows gives the resident cell's
// records, q, own tiles and bounce tiles.
#[test]
fn a_streamed_payload_loads_whole_under_off_equal_to_the_resident_load() {
    let resident = fixture_section();
    let resident_slices = resident.cell_slices();
    for (direct, bounce) in [
        (Streamed, Resident),
        (Resident, Streamed),
        (Streamed, Streamed),
    ] {
        let placed = placed(&resident, direct, bounce);
        let fixture = write_stream_fixture_with_mover_channel(
            &placed,
            vec![
                blob(
                    SectionId::KinematicGeometry,
                    sample_kinematic_section().to_bytes(),
                ),
                blob(SectionId::MoverProbeChannel, placed.to_bytes()),
            ],
        );
        let path = fixture.path.to_str().unwrap();

        let off = load_prl_with_streaming_mode_for_test(path, ShStreamingMode::Off).unwrap();
        let loaded = off.mover_probe_channel().expect("id 57 loads");
        assert_eq!(
            loaded, &placed,
            "{direct:?}/{bounce:?}: whole, placements kept"
        );
        assert_eq!(
            &self::placed(loaded, Resident, Resident),
            &resident,
            "{direct:?}/{bounce:?}: the same payload a resident placement holds"
        );

        let streamed = load_prl_with_streaming_mode_for_test(path, ShStreamingMode::Async).unwrap();
        let section = streamed.mover_probe_channel().expect("id 57 loads");
        for block in MoverChannelBlock::BOTH {
            assert_eq!(
                section.payload_held(block),
                section.placement(block) == Resident,
                "{block}: a streamed payload stays with id 50 while SH streams"
            );
        }
        let crate::ShStorage::Streaming(manifest) = streamed.sh_storage() else {
            panic!("the fixture streams");
        };
        let chunk = manifest.read_and_decode_cluster(0).unwrap();
        let rows =
            parse_streamed_rows(chunk.mover_channel_rows().expect("cluster 0 carries id 57"))
                .unwrap();
        let slices = section.cell_slices();
        assert_eq!(slices, resident_slices);
        for (position, cell) in section.listed_cells().enumerate() {
            let row = rows.iter().copied().find(|row| row.cell as usize == cell);
            let payload = section
                .cell_payload(position, &slices[position], row)
                .unwrap();
            let expected = resident
                .cell_payload(position, &slices[position], None)
                .unwrap();
            assert_eq!(payload.visibility, expected.visibility);
            assert_eq!(payload.own_tiles, expected.own_tiles);
            assert_eq!(payload.bounce_tiles, expected.bounce_tiles);
        }
    }
}

/// Bake row B5, loader side, on a real PRL: id 59's tiles plus id 57's
/// live tiles exactly at the tile reference's 1 << 15 range load, and the
/// loaded count the renderer sizes its texture for is that range; one live
/// tile more is refused by name. The 32×32×17 lattice holds enough valid
/// probes for 16,384 id-59 tiles and 16,384 (or 16,385) live ones.
#[test]
fn a_prl_whose_id_59_and_live_tiles_fill_the_range_exactly_loads_and_one_more_is_refused() {
    use crate::sh_stream_tests::{base_section_with_grid, write_minimal_whole_load_fixture};
    use postretro_level_format::mover_probe_channel::depth_tile_texture_count;
    use postretro_level_format::probe_depth_tiles::{
        MAX_PROBE_DEPTH_TILES, ProbeDepthTilesSection,
    };
    let grid = [32, 32, 17];
    let base = base_section_with_grid(grid);
    let id59_tiles = 1 << 14;
    let id59 = ProbeDepthTilesSection {
        probe_indices: (0..id59_tiles).collect(),
        texels: vec![[0x3c00, 0x3c00]; id59_tiles as usize * PROBE_DEPTH_TILE_TEXELS],
    };
    let channel = |live: u32| {
        MoverProbeChannelSection::assemble(
            MoverProbeChannelParts {
                affinity_dims: expected_affinity_dims(grid, AFFINITY_FACTOR),
                direct: Vec::new(),
                bounce: Vec::new(),
                depth: (0..live)
                    .map(|probe_index| DepthRecordParts {
                        record: MoverDepthRecord {
                            mover_id: 7,
                            probe_index,
                        },
                        tiles: vec![[0x3c00, 0x4000]; 2 * PROBE_DEPTH_TILE_TEXELS],
                    })
                    .collect(),
            },
            |_| u64::MAX,
            |mover_id| {
                Ok::<_, ()>(MoverLightRampRecord {
                    mover_id,
                    start: 0.0,
                    end: 1.0,
                })
            },
        )
        .unwrap()
        .expect("depth records make a section")
    };
    let load = |live: u32| {
        let fixture = write_minimal_whole_load_fixture(
            &base,
            vec![
                blob(
                    SectionId::KinematicGeometry,
                    sample_kinematic_section().to_bytes(),
                ),
                blob(SectionId::ProbeDepthTiles, id59.to_bytes()),
                blob(SectionId::MoverProbeChannel, channel(live).to_bytes()),
            ],
        );
        load_prl_with_streaming_mode_for_test(
            fixture.path.to_str().unwrap(),
            ShStreamingMode::Async,
        )
    };

    let exact = MAX_PROBE_DEPTH_TILES - id59_tiles;
    let world = load(exact).expect("exactly the range loads");
    let id59_loaded = world.probe_depth_tiles().expect("id 59 loads").tile_count();
    assert_eq!(id59_loaded, id59_tiles as usize);
    assert_eq!(
        depth_tile_texture_count(id59_loaded, world.mover_probe_channel()),
        MAX_PROBE_DEPTH_TILES as usize
    );

    match load(exact + 1) {
        Err(PrlLoadError::MoverProbeChannel(Error::TileRange { id59, live, max })) => {
            assert_eq!(
                (id59, live, max),
                (
                    id59_tiles as usize,
                    exact as usize + 1,
                    MAX_PROBE_DEPTH_TILES
                )
            );
        }
        other => panic!("expected the tile-range refusal, got {other:?}"),
    }
}
