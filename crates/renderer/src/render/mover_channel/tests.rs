// Mover probe channel table: layout, term resolution, cell writes and row
// membership, GPU-free. Compose proofs are in `compose_tests.rs`.
// See: context/lib/rendering_pipeline.md §4 (Mover state lighting)

use postretro_level_format::mover_probe_channel::{
    MOVER_IRRADIANCE_TILE_F16_COUNT, MoverChannelCell, MoverDirectRecord, MoverLightNamespace,
    MoverLightRampRecord, MoverPayloadPlacement, MoverProbeChannelSection,
};

use super::layout::{
    MoverChannelInputs, MoverChannelLayout, NO_LIGHT, NO_STATE, TermFamily, WholeTermRows,
};

const TILE: usize = MOVER_IRRADIANCE_TILE_F16_COUNT;

/// One term record of a fixture section: its light, its own tile's fill
/// (`None` borrows from a delta family), its probes, and per mover `(id,
/// q₀, q₁)` at every masked probe.
pub(super) struct FixtureRecord {
    pub(super) cell: u32,
    pub(super) namespace: MoverLightNamespace,
    pub(super) light: u32,
    pub(super) own_tile: Option<u16>,
    pub(super) mask: u64,
    pub(super) movers: Vec<(u32, u8, u8)>,
}

/// A resident direct-only section over `dims` affinity cells, every probe
/// valid. Records must ascend by (cell, namespace, light).
pub(super) fn fixture_section(
    dims: [u32; 3],
    records: &[FixtureRecord],
) -> MoverProbeChannelSection {
    let mut cells: Vec<u32> = records.iter().map(|record| record.cell).collect();
    cells.dedup();
    let mut section = MoverProbeChannelSection {
        affinity_factor: 4,
        affinity_dims: dims,
        direct_placement: MoverPayloadPlacement::Resident,
        bounce_placement: MoverPayloadPlacement::Resident,
        cells: cells
            .iter()
            .map(|&cell| MoverChannelCell {
                cell,
                valid_probe_mask: u64::MAX,
            })
            .collect(),
        direct_offsets: vec![0],
        direct_records: Vec::new(),
        mover_offsets: vec![0],
        direct_movers: Vec::new(),
        visibility: Vec::new(),
        own_tiles: Vec::new(),
        bounce_offsets: vec![0; cells.len() + 1],
        bounce_entries: Vec::new(),
        bounce_tiles: Vec::new(),
        depth_records: Vec::new(),
        depth_tiles: Vec::new(),
        ramps: Vec::new(),
    };
    for &cell in &cells {
        for record in records.iter().filter(|record| record.cell == cell) {
            section.direct_records.push(MoverDirectRecord {
                namespace: record.namespace,
                carries_own_tile: record.own_tile.is_some(),
                light_index: record.light,
                probe_mask: record.mask,
            });
            let probes = record.mask.count_ones() as usize;
            for &(mover, q0, q1) in &record.movers {
                section.direct_movers.push(mover);
                for _ in 0..probes {
                    section.visibility.extend_from_slice(&[q0, q1]);
                }
            }
            section
                .mover_offsets
                .push(section.direct_movers.len() as u32);
            if let Some(fill) = record.own_tile {
                section
                    .own_tiles
                    .extend(std::iter::repeat_n(fill, probes * TILE));
            }
        }
        section
            .direct_offsets
            .push(section.direct_records.len() as u32);
    }
    let mut movers = section.mover_ids();
    movers.dedup();
    section.ramps = movers
        .into_iter()
        .map(|mover_id| MoverLightRampRecord {
            mover_id,
            start: 0.0,
            end: 1.0,
        })
        .collect();
    section
}

pub(super) fn record(
    cell: u32,
    namespace: MoverLightNamespace,
    light: u32,
    own_tile: Option<u16>,
    mask: u64,
    movers: Vec<(u32, u8, u8)>,
) -> FixtureRecord {
    FixtureRecord {
        cell,
        namespace,
        light,
        own_tile,
        mask,
        movers,
    }
}

/// Id 41 over `cells` cells, every probe valid at L0, selection slot 0 at
/// each, its tiles all `fill`.
struct StaticFamily {
    masks: Vec<u64>,
    levels: Vec<u8>,
    offsets: Vec<u32>,
    lights: Vec<u32>,
    tiles: Vec<u16>,
}

impl StaticFamily {
    fn new(cells: usize, fill: u16) -> Self {
        Self {
            masks: vec![u64::MAX; cells],
            levels: vec![0; cells],
            offsets: (0..=cells as u32).collect(),
            lights: vec![0; cells],
            tiles: vec![fill; cells * 64 * TILE],
        }
    }

    fn family(&self) -> TermFamily<'_> {
        TermFamily {
            name: "id 41",
            valid_probe_masks: &self.masks,
            cell_levels: &self.levels,
            affinity_offsets: &self.offsets,
            affinity_lights: &self.lights,
        }
    }
}

fn inputs<'a>(
    section: &'a MoverProbeChannelSection,
    static_terms: Option<TermFamily<'a>>,
) -> MoverChannelInputs<'a> {
    MoverChannelInputs {
        section: Some(section),
        entity_shadow_lights: &[5],
        static_terms,
        animated_terms: None,
        animation_descriptor_indices: &[],
        indirect_descriptor_indices: &[],
    }
}

const F16_QUARTER: u16 = 0x3400;
const F16_EIGHTH: u16 = 0x3000;

#[test]
fn records_lay_out_static_lights_first_with_their_light_keys_and_mover_slots() {
    let family = StaticFamily::new(1, F16_QUARTER);
    // Wire order: AlphaLights 5 (selected), bake-only 0.
    let section = fixture_section(
        [1, 1, 1],
        &[
            record(
                0,
                MoverLightNamespace::StaticLight,
                5,
                None,
                0b11,
                vec![(7, 0, 255)],
            ),
            record(
                0,
                MoverLightNamespace::BakeOnlyStaticLight,
                0,
                Some(F16_EIGHTH),
                0b100,
                vec![(3, 10, 20), (7, 255, 0)],
            ),
        ],
    );
    let layout = MoverChannelLayout::build(&inputs(&section, Some(family.family())), false)
        .expect("the fixture lays out");
    let words = layout.initial_words();
    assert_eq!(words[0], 1, "one listed cell");
    assert_eq!(words[words[1] as usize], 0, "affinity cell 0");
    let ranges = words[2] as usize;
    assert_eq!(&words[ranges..ranges + 3], &[0, 2, 2], "two static records");
    let records = words[3] as usize;
    assert_eq!(
        words[records + 2],
        0,
        "AlphaLights 5 promotes from selection slot 0"
    );
    assert_eq!(
        words[records + 8 + 2],
        NO_LIGHT,
        "a bake-only light never promotes"
    );
    assert_eq!(layout.mover_ids(), &[3, 7]);
    let t = layout.t_byte_offset() as usize / 4;
    assert_eq!(
        &words[t..t + 2],
        &[NO_STATE.to_bits(); 2],
        "no state until handed over"
    );
    // Mover 7 (slot 1) gates both static records in cell 0; mover 3 (slot 0)
    // one. Pass B rows hold none: no animated light is gated.
    assert_eq!(layout.rows().static_by_mover.rows(1), &[0]);
    assert_eq!(layout.rows().static_by_mover.rows(0), &[0]);
    assert!(layout.rows().animated_by_mover.rows(1).is_empty());
    assert!(layout.rows().has_promotable(0));
    assert!(!layout.rows().has_animated(0));
}

/// P22 and C15's data half: a record carrying its own tile and one borrowing
/// id 41's per-probe term write the same T when the tiles agree.
#[test]
fn an_own_tile_and_a_borrowed_id_41_tile_write_the_same_term() {
    let family = StaticFamily::new(1, F16_QUARTER);
    let borrowed = fixture_section(
        [1, 1, 1],
        &[record(
            0,
            MoverLightNamespace::StaticLight,
            5,
            None,
            0b1,
            vec![(7, 0, 255)],
        )],
    );
    let own = fixture_section(
        [1, 1, 1],
        &[record(
            0,
            MoverLightNamespace::StaticLight,
            5,
            Some(F16_QUARTER),
            0b1,
            vec![(7, 0, 255)],
        )],
    );
    let rows = WholeTermRows::new(family.family(), &family.tiles);
    let write = |section: &MoverProbeChannelSection| {
        let layout =
            MoverChannelLayout::build(&inputs(section, Some(family.family())), false).unwrap();
        let slices = section.cell_slices();
        layout
            .held_writes(section, &slices, |cell, _| rows.row(cell))
            .unwrap()
    };
    let (borrowed, own) = (write(&borrowed), write(&own));
    assert_eq!(borrowed.len(), 1);
    // The cell's payload: its tiles, then its q bytes padded to a word.
    let pair = u32::from(F16_QUARTER) | (u32::from(F16_QUARTER) << 16);
    let mut expected = vec![pair; TILE / 2];
    expected.push(u32::from_le_bytes([0, 255, 0, 0]));
    assert_eq!(borrowed[0].words, expected);
    assert_eq!(borrowed[0].words, own[0].words);
}

/// Task 5's open item: an unflagged record whose family is absent at load
/// composes ungated; one whose family holds no L0 entry is refused.
#[test]
fn a_missing_family_leaves_the_record_ungated_and_a_coarse_cell_is_refused() {
    let section = fixture_section(
        [1, 1, 1],
        &[record(
            0,
            MoverLightNamespace::StaticLight,
            5,
            None,
            0b1,
            vec![(7, 0, 255)],
        )],
    );
    let layout = MoverChannelLayout::build(&inputs(&section, None), false).unwrap();
    assert_eq!(layout.record_count(), 0);
    assert_eq!(layout.ungated_records(), 1);

    let mut coarse = StaticFamily::new(1, F16_QUARTER);
    coarse.levels[0] = 1;
    let error =
        MoverChannelLayout::build(&inputs(&section, Some(coarse.family())), false).unwrap_err();
    assert!(error.to_string().contains("not L0"), "{error}");

    let mut other_light = StaticFamily::new(1, F16_QUARTER);
    other_light.lights[0] = 3;
    let error = MoverChannelLayout::build(&inputs(&section, Some(other_light.family())), false)
        .unwrap_err();
    assert!(error.to_string().contains("holds no entry"), "{error}");
}

/// A streamed payload's masks are unchecked at load under async: the cell
/// write checks them, and a mask that is not tight is refused.
#[test]
fn a_cell_whose_mask_is_not_tight_is_refused_at_write() {
    let mut section = fixture_section(
        [1, 1, 1],
        &[record(
            0,
            MoverLightNamespace::BakeOnlyStaticLight,
            0,
            Some(F16_QUARTER),
            0b11,
            vec![(7, 0, 255)],
        )],
    );
    // The second masked probe now stores 255 at both termini.
    section.visibility[2] = 255;
    section.visibility[3] = 255;
    let layout = MoverChannelLayout::build(&inputs(&section, None), false).unwrap();
    let error = layout
        .held_writes(&section, &section.cell_slices(), |_, _| None)
        .unwrap_err();
    assert!(error.to_string().contains("not tight"), "{error}");
}

/// C13's data half: records resolve by affinity row, never by stored slot,
/// so a re-tenanted slot composes its new row's own entries.
#[test]
fn records_resolve_by_affinity_row() {
    let section = fixture_section(
        [2, 1, 1],
        &[
            record(
                0,
                MoverLightNamespace::BakeOnlyStaticLight,
                0,
                Some(F16_QUARTER),
                0b1,
                vec![(7, 0, 255)],
            ),
            record(
                1,
                MoverLightNamespace::BakeOnlyStaticLight,
                1,
                Some(F16_EIGHTH),
                0b10,
                vec![(9, 0, 255)],
            ),
        ],
    );
    let layout = MoverChannelLayout::build(&inputs(&section, None), false).unwrap();
    assert_eq!(layout.plan_of_cell(0), Some(0));
    assert_eq!(layout.plan_of_cell(1), Some(1));
    assert_eq!(
        layout.rows().static_by_mover.rows(0),
        &[0],
        "mover 7: row 0 only"
    );
    assert_eq!(
        layout.rows().static_by_mover.rows(1),
        &[1],
        "mover 9: row 1 only"
    );
}

#[test]
fn no_section_lays_out_the_header_and_pass_b_descriptor_map_only() {
    let layout = MoverChannelLayout::empty(&[4, 9]);
    let words = layout.initial_words();
    assert_eq!(words[0], 0);
    assert_eq!(words[7], 2, "two descriptors");
    assert_eq!(&words[words[6] as usize..], &[4, 9]);
    assert!(layout.mover_ids().is_empty());
}

mod planner {
    //! C7, C8, C11, C12 and C14 on the streamed planner, for the direct
    //! passes and the indirect pass.
    use super::super::layout::MoverChannelRows;
    use super::*;
    use crate::render::animated_direct_sh_compose::AnimatedDirectShDebugOverride;
    use crate::render::direct_sh_compose::DirectShDebugOverride;
    use crate::render::sh_streaming::compose_plan_test_support::PlannerHarness;
    use postretro_render_cpu::frame_uniforms::LightTermMask;

    /// Row 0: a static record of mover slot 0 (A) and slot 1 (B). Row 1:
    /// slot 0 only, static. Row 2: slot 1, an animated-baked light.
    fn rows() -> MoverChannelRows {
        let section = fixture_section(
            [3, 1, 1],
            &[
                record(
                    0,
                    MoverLightNamespace::BakeOnlyStaticLight,
                    0,
                    Some(F16_QUARTER),
                    0b1,
                    vec![(10, 0, 255), (20, 0, 255)],
                ),
                record(
                    1,
                    MoverLightNamespace::BakeOnlyStaticLight,
                    1,
                    Some(F16_QUARTER),
                    0b1,
                    vec![(10, 0, 255)],
                ),
                record(
                    2,
                    MoverLightNamespace::AnimatedBaked,
                    0,
                    Some(F16_QUARTER),
                    0b1,
                    vec![(20, 0, 255)],
                ),
            ],
        );
        let animated = TermFamily {
            name: "id 45",
            valid_probe_masks: &[u64::MAX; 3],
            cell_levels: &[0; 3],
            affinity_offsets: &[0, 0, 0, 0],
            affinity_lights: &[],
        };
        let layout = MoverChannelLayout::build(
            &MoverChannelInputs {
                section: Some(&section),
                entity_shadow_lights: &[],
                static_terms: None,
                animated_terms: Some(animated),
                animation_descriptor_indices: &[0],
                indirect_descriptor_indices: &[],
            },
            false,
        )
        .unwrap();
        let mut rows = layout.rows().clone();
        // Mover B (slot 1) also changes bounce at row 2; A changes none.
        rows.bounce_by_mover = super::super::layout::MoverRows::from_sets(vec![vec![], vec![2]]);
        rows
    }

    fn harness() -> PlannerHarness {
        let mut harness = PlannerHarness::new(4);
        harness.observe_resident(&[0, 1, 2]);
        // Settle every row once.
        harness.frame(true, &[], &[], &MoverChannelRows::default());
        harness.commit_all();
        harness
    }

    /// C7 and C12: a frame with no t′ change composes no channel row; a
    /// travelling mover (B, slot 1) composes only its rows; a row it shares
    /// with a held mover is among them, and a row only the held mover
    /// carries stays current.
    #[test]
    fn only_the_travelling_movers_rows_recompose() {
        let rows = rows();
        let mut harness = harness();
        let plan = harness.frame(true, &[0, 1, 2], &[], &rows);
        assert!(plan.static_rows.is_empty() && plan.animated_rows.is_empty());
        let plan = harness.frame(true, &[0, 1, 2], &[1], &rows);
        assert_eq!(plan.static_rows, vec![0], "B's static row, shared with A");
        assert_eq!(
            plan.animated_rows,
            vec![0, 2],
            "Pass B: B's animated row, and Pass A's planned row after it"
        );
    }

    /// C14: entries route to the passes they feed. A static-only mover (A)
    /// stales Pass A, which then schedules Pass B at those rows; it never
    /// touches an animated-only row.
    #[test]
    fn a_static_only_mover_composes_pass_a_then_pass_b_at_its_rows() {
        let rows = rows();
        let mut harness = harness();
        let plan = harness.frame(true, &[0, 1, 2], &[0], &rows);
        assert_eq!(plan.static_rows, vec![0, 1]);
        assert_eq!(plan.animated_rows, vec![0, 1]);
    }

    /// C7's lag half and C11: a row outside the view gate lags and composes
    /// the frame it re-enters; a change on a frame that records no compose
    /// composes on the next recording frame though t′ no longer moves.
    #[test]
    fn a_change_outside_the_gate_or_unrecorded_composes_later() {
        let rows = rows();
        let mut harness = harness();
        let plan = harness.frame(true, &[1], &[0], &rows);
        assert_eq!(plan.static_rows, vec![1], "row 0 is outside the gate");
        harness.commit_all();
        let plan = harness.frame(true, &[0, 1], &[], &rows);
        assert_eq!(plan.static_rows, vec![0], "row 0 re-enters and composes");
        harness.commit_all();

        let _ = harness.frame(false, &[0, 1, 2], &[0], &rows);
        let plan = harness.frame(true, &[0, 1, 2], &[], &rows);
        assert_eq!(
            plan.static_rows,
            vec![0, 1],
            "the unrecorded change is kept"
        );
    }

    /// C7, C14 (indirect): a travelling mover recomposes in the indirect
    /// pass only the rows its bounce entries name; a mover with no bounce
    /// entry stales no indirect row, and a frame with no t′ change none.
    #[test]
    fn only_a_travelling_movers_bounce_rows_recompose_indirect() {
        let rows = rows();
        let mut harness = harness();
        let plan = harness.frame(true, &[0, 1, 2], &[], &rows);
        assert!(
            plan.indirect_rows.is_empty(),
            "no t′ change, no indirect row"
        );
        let plan = harness.frame(true, &[0, 1, 2], &[1], &rows);
        assert_eq!(plan.indirect_rows, vec![2], "B's bounce row only");
        assert_eq!(plan.static_rows, vec![0], "B's direct rows as before");
        harness.commit_all();
        let plan = harness.frame(true, &[0, 1, 2], &[0], &rows);
        assert!(
            plan.indirect_rows.is_empty(),
            "A carries no bounce entry: the indirect pass composes nothing"
        );
    }

    /// C11 (indirect): a bounce change outside the gate lags to the frame
    /// the row re-enters; one on a frame recording no compose is kept.
    #[test]
    fn a_bounce_change_outside_the_gate_or_unrecorded_composes_later() {
        let rows = rows();
        let mut harness = harness();
        let plan = harness.frame(true, &[0, 1], &[1], &rows);
        assert!(plan.indirect_rows.is_empty(), "row 2 is outside the gate");
        harness.commit_all();
        let plan = harness.frame(true, &[0, 1, 2], &[], &rows);
        assert_eq!(plan.indirect_rows, vec![2], "row 2 re-enters and composes");
        harness.commit_all();

        let _ = harness.frame(false, &[0, 1, 2], &[1], &rows);
        let plan = harness.frame(true, &[0, 1, 2], &[], &rows);
        assert_eq!(plan.indirect_rows, vec![2], "the unrecorded change is kept");
    }

    /// C8: a promotion weight or a curve changing raises no mover trigger:
    /// rows the channel alone carries stay current.
    #[test]
    fn weight_and_curve_changes_raise_no_mover_trigger() {
        let rows = rows();
        let mut harness = harness();
        let plan = harness.frame_with(
            true,
            &[0, 1, 2],
            &[],
            &rows,
            &[0.5],
            true,
            LightTermMask::ALL,
            DirectShDebugOverride::default(),
            AnimatedDirectShDebugOverride::default(),
        );
        assert!(
            plan.static_rows.is_empty(),
            "no id-41 row and no mover moved: Pass A composes nothing"
        );
    }
}
