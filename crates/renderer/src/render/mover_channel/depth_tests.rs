// CPU proofs for the mover depth live-tile plan: the blend, the write and
// reference triggers, the several-mover rule (differences from the base add,
// clamped to a valid moment set), run coalescing and the frame's
// allocations. GPU proofs live in `mover_depth_tiles_tests.rs`.
// See: context/lib/rendering_pipeline.md §4 (Mover state lighting)

use super::*;
use crate::render::MoverTravel;
use crate::render::mover_channel::depth_test_support::{
    PosedDepth, depth_section, posed_depth_section, ramp_tile, stored_depth_section, through_base,
    uniform_tile,
};
use postretro_level_format::probe_depth_tiles::ProbeDepthTilesSection;
use postretro_level_format::sh_volume::OctahedralShProbe;

use super::super::knots::MoverKnots;
use super::super::state::MoverTBlock;

struct Plan {
    depth: MoverDepthTiles,
    t: Vec<f32>,
    /// What the last frame wrote: `(tile, texels)` per tile, and the runs.
    tiles: Vec<(u32, Vec<[u16; 2]>)>,
    runs: Vec<(u32, u32)>,
    references: Vec<ReferenceChange>,
}

impl Plan {
    /// `section` bound after `id59` id-59 tiles, which tile `id59_probes`;
    /// every untiled probe's averaged moments are zero.
    fn new(section: &MoverProbeChannelSection, id59_probes: &[u32]) -> Self {
        let id59 = ProbeDepthTilesSection {
            probe_indices: id59_probes.to_vec(),
            texels: vec![[0, 0]; id59_probes.len() * PROBE_DEPTH_TILE_TEXELS],
        };
        let probes = section
            .depth_records
            .iter()
            .map(|record| record.probe_index as usize + 1)
            .max()
            .unwrap_or(0);
        Self::with_bases(section, &id59, &vec![averaged(0.0, 0.0); probes])
    }

    /// `section` bound after `id59`'s tiles, over the id-34 `probes` an
    /// untiled probe's averaged moments come from.
    fn with_bases(
        section: &MoverProbeChannelSection,
        id59: &ProbeDepthTilesSection,
        probes: &[OctahedralShProbe],
    ) -> Self {
        let ids = section.mover_ids();
        let mut depth = MoverDepthTiles::new(Some(section), &ids).expect("the depth block loads");
        let live = depth.live_tile_count();
        let references = ProbeDepthTileReferences::from_section(Some(id59), live);
        depth
            .bind(
                id59.tile_count() as u32,
                references.texture_layout().expect("tiles lay out"),
                &references,
                DepthBases {
                    id59: Some(id59),
                    probes,
                },
            )
            .expect("every probe has a base");
        Self {
            depth,
            t: vec![-1.0; ids.len()],
            tiles: Vec::new(),
            runs: Vec::new(),
            references: Vec::new(),
        }
    }

    /// One frame: set each `(slot, t′)`, then plan with `changed` slots.
    fn frame(&mut self, states: &[(usize, f32)], changed: &[u32]) {
        for &(slot, t) in states {
            self.t[slot] = t;
        }
        let mut tiles = Vec::new();
        let mut runs = Vec::new();
        let references = self
            .depth
            .plan(&self.t, changed, |first, count, bytes| {
                runs.push((first, count));
                tiles.extend(unpack_run(first, count, bytes));
            })
            .to_vec();
        self.tiles = tiles;
        self.runs = runs;
        self.references = references;
    }

    fn tile(&self, tile: u32) -> &[[u16; 2]] {
        &self
            .tiles
            .iter()
            .find(|(written, _)| *written == tile)
            .unwrap_or_else(|| panic!("tile {tile} was written"))
            .1
    }
}

/// A run's bytes back into its tiles' texels.
fn unpack_run(first: u32, count: u32, bytes: &[u8]) -> Vec<(u32, Vec<[u16; 2]>)> {
    let half = |at: usize| u16::from_le_bytes([bytes[at], bytes[at + 1]]);
    (0..count as usize)
        .map(|tile| {
            let texels = (0..PROBE_DEPTH_TILE_TEXELS)
                .map(|texel| {
                    let (row, column) = (texel / EDGE, texel % EDGE);
                    let at = ((row * count as usize + tile) * EDGE + column) * TEXEL_BYTES;
                    [half(at), half(at + 2)]
                })
                .collect();
            (first + tile as u32, texels)
        })
        .collect()
}

fn f16(bits: u16) -> f32 {
    f16_bits_to_f32(bits)
}

/// An id-34 probe whose averaged moments are `[mean, mean_sq]`.
fn averaged(mean: f32, mean_sq: f32) -> OctahedralShProbe {
    OctahedralShProbe {
        validity: 1,
        mean_distance: f32_to_f16_bits(mean),
        mean_sq_distance: f32_to_f16_bits(mean_sq),
        density_level: 0,
        node_scale: 0,
    }
}

/// An id 59 that tiles no probe.
fn no_id59() -> ProbeDepthTilesSection {
    ProbeDepthTilesSection {
        probe_indices: Vec::new(),
        texels: Vec::new(),
    }
}

/// Within one f16 ulp of `expected`, per channel.
fn assert_within_ulp(texels: &[[u16; 2]], expected: impl Fn(usize, usize) -> f32) {
    for (at, texel) in texels.iter().enumerate() {
        for (channel, &got) in texel.iter().enumerate() {
            let want = expected(at, channel);
            let rounded = f32_to_f16_bits(want);
            assert!(
                got.abs_diff(rounded) <= 1,
                "texel {at} channel {channel}: {} vs exact {want}",
                f16(got)
            );
        }
    }
}

/// A live tile reads D₀ at t′ = 0, D₁ at t′ = 1 and the probe's base (its
/// id-59 tile, else its averaged moments) at t′ = 0.5, each bit for bit, and
/// the blend through the base between them within one f16 ulp, rising and
/// after a reversal; the first write points the probe at it, and later ones
/// leave the reference alone.
#[test]
fn a_live_tile_blends_through_its_probes_base_exact_at_termini_and_midpoint() {
    let (d0, d1) = (ramp_tile(0.25), ramp_tile(3.0));
    let tiled = ramp_tile(1.5);
    // Probe 9 is tiled by id 59; probe 12 is untiled.
    let section = depth_section(vec![
        (4, 9, d0.clone(), d1.clone()),
        (4, 12, d1.clone(), d0.clone()),
    ]);
    let id59 = ProbeDepthTilesSection {
        probe_indices: vec![2, 9],
        texels: [ramp_tile(7.0), tiled.clone()].concat(),
    };
    let mut probes = vec![averaged(0.0, 0.0); 13];
    probes[12] = averaged(2.0, 4.5);
    let averaged_12 = uniform_tile(2.0, 4.5);
    let mut plan = Plan::with_bases(&section, &id59, &probes);
    let cases = [
        (2, d0.clone(), tiled.clone(), d1.clone()),
        (3, d1.clone(), averaged_12.clone(), d0.clone()),
    ];
    plan.frame(&[(0, 0.0)], &[0]);
    assert_eq!(plan.tile(2), &d0[..], "t′ = 0 holds D₀ bit for bit");
    assert_eq!(plan.references, [(9, Some(2)), (12, Some(3))]);
    for t in [0.25, 0.5, 0.75, 1.0, 0.6, 0.5, 0.4, 0.0, 0.3] {
        plan.frame(&[(0, t)], &[0]);
        assert!(plan.references.is_empty(), "the reference already points");
        for (live, d0, base, d1) in &cases {
            match t {
                0.0 => assert_eq!(plan.tile(*live), &d0[..], "D₀ at 0"),
                0.5 => assert_eq!(plan.tile(*live), &base[..], "the base at 0.5"),
                1.0 => assert_eq!(plan.tile(*live), &d1[..], "D₁ at 1"),
                _ => assert_within_ulp(plan.tile(*live), |at, channel| {
                    through_base(
                        f16(d0[at][channel]),
                        f16(base[at][channel]),
                        f16(d1[at][channel]),
                        t,
                    )
                }),
            }
        }
    }
}

/// A terminus a record stores no tile for is the probe's base moment set:
/// its id-59 tile where it has one, else its averaged id-34 moments in every
/// texel. So the live tile blends between the stored terminus and the base
/// over that terminus's half of travel, and holds the base bit for bit over
/// the other half. With two movers, a mover at its unstored half adds
/// nothing.
#[test]
fn an_unstored_terminus_blends_toward_the_probes_base_moments() {
    let (d0, d1) = (ramp_tile(0.25), ramp_tile(3.0));
    let tiled = ramp_tile(1.5);
    // Probe 9 is tiled by id 59 and stores D₀ only; probe 12 is untiled and
    // stores D₁ only; probe 20 is untiled, mover 4 stores D₀ only and mover
    // 6 both.
    let section = stored_depth_section(vec![
        (4, 9, [Some(d0.clone()), None]),
        (4, 12, [None, Some(d1.clone())]),
        (4, 20, [Some(uniform_tile(4.0, 17.0)), None]),
        (
            6,
            20,
            [Some(uniform_tile(6.0, 37.0)), Some(uniform_tile(0.5, 1.0))],
        ),
    ]);
    let id59 = ProbeDepthTilesSection {
        probe_indices: vec![9],
        texels: tiled.clone(),
    };
    let mut probes = vec![averaged(0.0, 0.0); 21];
    probes[12] = averaged(2.0, 4.5);
    probes[20] = averaged(3.0, 9.5);
    let mut plan = Plan::with_bases(&section, &id59, &probes);
    let averaged_12 = uniform_tile(2.0, 4.5);
    let averaged_20 = uniform_tile(3.0, 9.5);
    // Live tiles follow id 59's one tile, in record order: 9, 12, 20.
    let (live_9, live_12, live_20) = (1, 2, 3);

    plan.frame(&[(0, 0.0)], &[0]);
    assert_eq!(plan.tile(live_9), &d0[..], "probe 9 at its stored D₀");
    assert_eq!(
        plan.tile(live_12),
        &averaged_12[..],
        "probe 12 reads its averaged moments"
    );
    plan.frame(&[(0, 1.0)], &[0]);
    assert_eq!(
        plan.tile(live_9),
        &tiled[..],
        "probe 9 reads its id-59 tile"
    );
    assert_eq!(plan.tile(live_12), &d1[..], "probe 12 at its stored D₁");
    for t in [0.5, 0.75, 0.6] {
        plan.frame(&[(0, t)], &[0]);
        assert_eq!(plan.tile(live_9), &tiled[..], "probe 9 holds its base");
    }
    for t in [0.25, 0.4] {
        plan.frame(&[(0, t)], &[0]);
        assert_eq!(
            plan.tile(live_12),
            &averaged_12[..],
            "probe 12 holds its base"
        );
    }
    plan.frame(&[(0, 0.25)], &[0]);
    assert_within_ulp(plan.tile(live_9), |at, channel| {
        0.5 * f16(d0[at][channel]) + 0.5 * f16(tiled[at][channel])
    });
    plan.frame(&[(0, 0.75)], &[0]);
    assert_within_ulp(plan.tile(live_12), |at, channel| {
        0.5 * f16(averaged_12[at][channel]) + 0.5 * f16(d1[at][channel])
    });

    // Probe 20 (base: averaged moments 3, 9.5): mover 4 parked at its
    // unstored D₁ adds nothing, so the tile is mover 6's alone, bit for bit,
    // at D₀ and then at D₁.
    plan.frame(&[(0, 1.0), (1, 0.0)], &[0, 1]);
    assert_eq!(
        plan.tile(live_20),
        &uniform_tile(6.0, 37.0)[..],
        "the parked mover adds nothing"
    );
    plan.frame(&[(1, 1.0)], &[1]);
    assert_eq!(plan.tile(live_20), &uniform_tile(0.5, 1.0)[..]);
    // Mover 4 at its stored D₀ (4, 17) adds (1, 7.5) to mover 6's
    // (−2.5, −8.5): base + both = (1.5, 8.5).
    plan.frame(&[(0, 0.0)], &[0]);
    assert_eq!(plan.tile(live_20), &uniform_tile(1.5, 8.5)[..]);
    // Mover 4 anywhere on its unstored half adds nothing; mover 6 at 0.5
    // is the base and adds nothing either.
    plan.frame(&[(0, 0.7), (1, 0.0)], &[0, 1]);
    assert_eq!(plan.tile(live_20), &uniform_tile(6.0, 37.0)[..]);
    plan.frame(&[(1, 0.5)], &[1]);
    assert_eq!(plan.tile(live_20), &averaged_20[..]);
    // Both mid-half add their through-base differences: mover 4 at 0.25
    // is (3.5, 13.25), mover 6 at 0.75 is (1.75, 5.25), so the base (3,
    // 9.5) plus both is (2.25, 9).
    plan.frame(&[(0, 0.25), (1, 0.75)], &[0, 1]);
    assert_eq!(plan.tile(live_20), &uniform_tile(2.25, 9.0)[..]);
    // With mover 6's state gone, mover 4 at its unstored D₁ is the base.
    plan.frame(&[(0, 1.0), (1, -1.0)], &[0, 1]);
    assert_eq!(plan.tile(live_20), &averaged_20[..]);

    // The CPU holds five stored tiles and one base tile per probe (9, 12, 20).
    assert_eq!(
        plan.depth.host_bytes(),
        (8 * PROBE_DEPTH_TILE_TEXELS * size_of::<[f32; 2]>()) as u64
    );
}

/// A
/// probe keeps its id-59 tile or averaged moments until its mover's first
/// hand-over, and returns to it once no mover changing it has state.
#[test]
fn a_probe_keeps_its_id_59_reference_without_state_and_returns_to_it() {
    let section = depth_section(vec![
        (1, 5, uniform_tile(1.0, 2.0), uniform_tile(2.0, 5.0)),
        (1, 9, uniform_tile(1.0, 2.0), uniform_tile(2.0, 5.0)),
    ]);
    let mut plan = Plan::new(&section, &[5]);
    plan.frame(&[], &[]);
    assert!(plan.tiles.is_empty() && plan.references.is_empty());
    plan.frame(&[(0, 0.5)], &[0]);
    assert_eq!(plan.references, [(5, Some(1)), (9, Some(2))]);
    plan.frame(&[(0, -1.0)], &[0]);
    assert!(plan.tiles.is_empty(), "no state writes no tile");
    assert_eq!(
        plan.references,
        [(5, Some(0)), (9, None)],
        "id 59's tile and the averaged moments again"
    );
}

/// A frame whose reference moves fail abandons its plan: the next plan from
/// the same uncommitted change emits the same moves again, so a reference
/// never stays behind its live tile.
#[test]
fn an_abandoned_plan_emits_its_reference_moves_again() {
    let section = depth_section(vec![(1, 5, uniform_tile(1.0, 2.0), uniform_tile(2.0, 5.0))]);
    let mut plan = Plan::new(&section, &[]);
    plan.frame(&[(0, 0.5)], &[0]);
    assert_eq!(plan.references, [(5, Some(0))]);
    plan.depth.abandon_plan();
    plan.frame(&[], &[0]);
    assert_eq!(plan.references, [(5, Some(0))], "the move is emitted again");
    plan.frame(&[], &[0]);
    assert!(plan.references.is_empty(), "a landed move is not repeated");
    // Returning to the base, abandoned, is emitted again too.
    plan.frame(&[(0, -1.0)], &[0]);
    assert_eq!(plan.references, [(5, None)]);
    plan.depth.abandon_plan();
    plan.frame(&[], &[0]);
    assert_eq!(plan.references, [(5, None)]);
}

/// One frame of the live-tile step as `MoverChannelResources` drives it:
/// the t′ block's changes plan the frame, the first `land` reference moves
/// land in `texels` and the rest fail. A frame whose moves all land
/// commits; one that fails abandons both halves.
fn step(
    depth: &mut MoverDepthTiles,
    block: &mut MoverTBlock,
    texels: &mut std::collections::BTreeMap<u32, Option<u16>>,
    states: &[MoverTravel],
    land: usize,
) -> Vec<ReferenceChange> {
    block.set_states(states);
    let (t, changed) = block.depth_changes();
    let references = depth.plan(t, changed, |_, _, _| {}).to_vec();
    for &(probe, tile) in references.iter().take(land) {
        texels.insert(probe, tile);
    }
    if land >= references.len() {
        block.commit_depth_changes();
    } else {
        depth.abandon_plan();
        block.forget_depth_changes();
    }
    references
}

/// A reference move fails after another landed, and the mover's state is
/// gone by the next frame, which is the t′ the slot was last written from.
/// The next frame still replans and moves every probe of the abandoned plan
/// back to its base, the landed one included, so no probe keeps a live tile
/// while no mover has state.
#[test]
fn a_partial_landing_then_no_state_returns_every_probe_to_its_base() {
    let section = depth_section(vec![
        (1, 5, uniform_tile(1.0, 2.0), uniform_tile(2.0, 5.0)),
        (1, 9, uniform_tile(1.0, 2.0), uniform_tile(2.0, 5.0)),
    ]);
    let mut plan = Plan::new(&section, &[]);
    let ids = section.mover_ids();
    let knots = MoverKnots::new(Some(&section), &ids).expect("every mover has a record");
    let mut block = MoverTBlock::new(&ids, &knots);
    // Each probe's reference in the moment texels; absent is its base.
    let mut texels = std::collections::BTreeMap::new();
    let state = [MoverTravel {
        mover_id: 1,
        t: 0.5,
    }];
    let moves = step(&mut plan.depth, &mut block, &mut texels, &state, 1);
    assert_eq!(moves, [(5, Some(0)), (9, Some(1))]);
    assert_eq!(texels.get(&5), Some(&Some(0)), "probe 5's move landed");
    assert_eq!(texels.get(&9), None, "probe 9's failed");

    let moves = step(&mut plan.depth, &mut block, &mut texels, &[], usize::MAX);
    assert_eq!(
        moves,
        [(5, None), (9, None)],
        "both moves back, landed or not"
    );
    assert!(
        texels.values().all(Option::is_none),
        "every probe at its base"
    );
    let moves = step(&mut plan.depth, &mut block, &mut texels, &[], usize::MAX);
    assert!(moves.is_empty(), "nothing left to move");

    // The same failure, then the same t′ again: both moves are emitted.
    let moves = step(&mut plan.depth, &mut block, &mut texels, &state, 1);
    assert_eq!(moves.len(), 2);
    let moves = step(&mut plan.depth, &mut block, &mut texels, &state, usize::MAX);
    assert_eq!(moves, [(5, Some(0)), (9, Some(1))]);
    assert_eq!(
        texels.values().copied().collect::<Vec<_>>(),
        [Some(0), Some(1)]
    );
}

/// A frame with no change writes nothing; a travelling mover
/// writes only live tiles its records cover; a tile only a held mover
/// covers stays untouched; a shared tile is rewritten at the held t′.
#[test]
fn only_a_travelling_movers_tiles_are_written_and_shared_ones_read_the_held_t() {
    let section = depth_section(vec![
        (1, 3, uniform_tile(1.0, 1.0), uniform_tile(3.0, 9.0)),
        (1, 4, uniform_tile(1.0, 1.0), uniform_tile(3.0, 9.0)),
        (2, 4, uniform_tile(2.0, 4.0), uniform_tile(0.5, 0.25)),
        (2, 7, uniform_tile(2.0, 4.0), uniform_tile(0.5, 0.25)),
    ]);
    let mut plan = Plan::new(&section, &[]);
    plan.frame(&[(0, 0.0), (1, 1.0)], &[0, 1]);
    assert_eq!(plan.tiles.len(), 3, "the first hand-over writes every tile");
    plan.frame(&[], &[]);
    assert!(plan.tiles.is_empty() && plan.runs.is_empty());

    // Mover 2 (slot 1) holds at 1; mover 1 (slot 0) travels.
    plan.frame(&[(0, 0.5)], &[0]);
    let written: Vec<u32> = plan.tiles.iter().map(|(tile, _)| *tile).collect();
    assert_eq!(
        written,
        [0, 1],
        "probe 7's tile, mover 2's alone, is untouched"
    );
    // Probe 4 is rewritten from both movers, mover 2 at its held t′ = 1:
    // the tile a fresh plan writes from both states at once.
    let mut fresh = Plan::new(&section, &[]);
    fresh.frame(&[(0, 0.5), (1, 1.0)], &[0, 1]);
    assert_eq!(plan.tile(1), fresh.tile(1));
}

/// Two movers changing one probe write its live tile
/// once a frame, with bytes that depend only on their t′, whichever state
/// is handed over first; each texel holds the base plus both movers'
/// blended differences from it, so an open mover adds nothing.
#[test]
fn two_movers_at_one_probe_write_it_once_adding_their_differences() {
    // Probe 6's base is the open tile (averaged moments 4, 16). Mover 1
    // closes near on the low texels, mover 2 on the high ones.
    let near_low: Vec<[u16; 2]> = (0..PROBE_DEPTH_TILE_TEXELS)
        .map(|i| {
            let mean = if i < 50 { 0.5 } else { 4.0 };
            [f32_to_f16_bits(mean), f32_to_f16_bits(mean * mean)]
        })
        .collect();
    let near_high: Vec<[u16; 2]> = near_low.iter().rev().copied().collect();
    let open = uniform_tile(4.0, 16.0);
    let section = depth_section(vec![
        (1, 6, near_low.clone(), open.clone()),
        (2, 6, near_high.clone(), open.clone()),
    ]);
    let mut probes = vec![averaged(0.0, 0.0); 7];
    probes[6] = averaged(4.0, 16.0);
    let tile = |states: &[(usize, f32)]| {
        let mut plan = Plan::with_bases(&section, &no_id59(), &probes);
        plan.frame(states, &[0, 1]);
        assert_eq!(plan.runs, [(0, 1)], "one write of the shared tile");
        plan.tile(0).to_vec()
    };
    let both_closed = tile(&[(0, 0.0), (1, 0.0)]);
    assert_eq!(both_closed, tile(&[(1, 0.0), (0, 0.0)]), "hand-over order");
    assert_eq!(
        both_closed,
        uniform_tile(0.5, 0.25),
        "each texel takes its one closing mover's difference"
    );
    let one_open = tile(&[(0, 1.0), (1, 0.0)]);
    assert_eq!(one_open, near_high.to_vec(), "the open mover adds nothing");
    assert_eq!(tile(&[(0, 1.0), (1, 1.0)]), open);
}

/// Where two movers change the same texel their differences add, which can
/// leave no valid moment set; the clamp restores `E[d] ≥ 0` and
/// `E[d²] ≥ E[d]²`, and a sum already valid passes unchanged.
#[test]
fn overlapping_differences_clamp_to_a_valid_moment_set() {
    let section = depth_section(vec![
        (1, 6, uniform_tile(2.0, 5.0), uniform_tile(4.0, 17.0)),
        (2, 6, uniform_tile(3.0, 10.0), uniform_tile(1.0, 1.5)),
    ]);
    let mut probes = vec![averaged(0.0, 0.0); 7];
    probes[6] = averaged(4.0, 17.0);
    let mut plan = Plan::with_bases(&section, &no_id59(), &probes);
    // Both at D₀: (4 − 2 − 1, 17 − 12 − 7) = (1, −2); E[d²] lifts to E[d]².
    plan.frame(&[(0, 0.0), (1, 0.0)], &[0, 1]);
    assert_eq!(plan.tile(0), &uniform_tile(1.0, 1.0)[..]);
    // Mover 2 at D₁: (4 − 2 − 3, 17 − 12 − 15.5) = (−1, −10.5); both clamp
    // to zero.
    plan.frame(&[(1, 1.0)], &[1]);
    assert_eq!(plan.tile(0), &uniform_tile(0.0, 0.0)[..]);
    // Mover 1 at its D₁, the base, adds nothing: mover 2's D₁ bit for bit.
    plan.frame(&[(0, 1.0)], &[0]);
    assert_eq!(plan.tile(0), &uniform_tile(1.0, 1.5)[..]);
}

/// A far texel the bake saturated (`E[d²]` stored as the largest finite
/// f16, 65504, below `E[d]²`) passes the several-mover sum exactly: two
/// movers at t′ = 0.5 return the base bit for bit, never an infinite `E[d²]`
/// the sampler would turn into NaN. Overlapping far differences stay finite.
#[test]
fn a_saturated_far_texel_stays_finite_under_several_movers() {
    let far = uniform_tile(300.0, 65504.0);
    assert_eq!(
        far[0],
        [f32_to_f16_bits(300.0), 0x7bff],
        "the bake's saturated texel"
    );
    let section = depth_section(vec![
        (1, 6, uniform_tile(1.0, 2.0), uniform_tile(2.0, 5.0)),
        (2, 6, uniform_tile(3.0, 10.0), uniform_tile(0.5, 1.0)),
    ]);
    // Probe 6 is tiled by id 59 with the saturated texel: live tile 1.
    let id59 = ProbeDepthTilesSection {
        probe_indices: vec![6],
        texels: far.clone(),
    };
    let mut plan = Plan::with_bases(&section, &id59, &[]);
    plan.frame(&[(0, 0.5), (1, 0.5)], &[0, 1]);
    assert_eq!(plan.tile(1), &far[..], "the saturated base bit for bit");

    // Both movers close on far texels over a nearer base: each adds (200,
    // 55504), so the sum passes the largest finite f16 in both moments.
    let near = uniform_tile(100.0, 10000.0);
    let section = depth_section(vec![
        (1, 6, far.clone(), near.clone()),
        (2, 6, far.clone(), near.clone()),
    ]);
    let mut probes = vec![averaged(0.0, 0.0); 7];
    probes[6] = averaged(100.0, 10000.0);
    let mut plan = Plan::with_bases(&section, &no_id59(), &probes);
    for t in [0.0, 0.1, 0.25, 0.4] {
        plan.frame(&[(0, t), (1, t)], &[0, 1]);
        for texel in plan.tile(0) {
            for &half in texel {
                assert_ne!(half & 0x7c00, 0x7c00, "t′ = {t}: a non-finite moment");
            }
        }
    }
    plan.frame(&[(0, 0.0), (1, 0.0)], &[0, 1]);
    assert_eq!(plan.tile(0), &uniform_tile(500.0, 65504.0)[..]);
}

/// An untiled probe outside the level's id-34 probes has no base to blend
/// through: binding refuses it, never blending toward zero moments. A bound
/// level holds exactly its stored and base tiles, the host ledger's row.
#[test]
fn a_probe_with_no_base_refuses_the_bind() {
    let section = depth_section(vec![(1, 9, uniform_tile(1.0, 2.0), uniform_tile(2.0, 5.0))]);
    let id59 = no_id59();
    let mut depth = MoverDepthTiles::new(Some(&section), &section.mover_ids()).unwrap();
    let references = ProbeDepthTileReferences::from_section(Some(&id59), depth.live_tile_count());
    let probes = vec![averaged(1.0, 1.0); 9];
    let error = depth
        .bind(
            0,
            references.texture_layout().expect("tiles lay out"),
            &references,
            DepthBases {
                id59: Some(&id59),
                probes: &probes,
            },
        )
        .unwrap_err();
    assert!(error.to_string().contains("probe 9"), "{error}");

    let plan = Plan::with_bases(&section, &id59, &[averaged(1.0, 1.0); 10]);
    assert_eq!(
        (plan.depth.capacities()[4] * size_of::<f32>()) as u64,
        plan.depth.host_bytes(),
        "the CPU-held tiles are reserved exactly"
    );
}

/// Runs of live tiles coalesce while consecutive on one tile row and break
/// at a row's end or a gap; every dirty tile still lands exactly once.
#[test]
fn consecutive_live_tiles_on_one_row_share_one_copy() {
    // 3 id-59 tiles + 6 live tiles: 4 tiles per row, so live tiles 3..9
    // sit on rows 0 (3), 1 (4..8) and 2 (8).
    let section = depth_section(
        (0..6u32)
            .map(|probe| {
                let mover = if probe == 4 { 2 } else { 1 };
                (mover, probe, uniform_tile(1.0, 1.0), uniform_tile(2.0, 4.0))
            })
            .collect(),
    );
    let mut plan = Plan::new(&section, &[10, 11, 12]);
    plan.frame(&[(0, 0.5), (1, 0.5)], &[0, 1]);
    assert_eq!(plan.runs, [(3, 1), (4, 4), (8, 1)]);
    // Mover 1's probes 0–3 and 5 hold tiles 3–7; mover 2's probe 4 follows.
    plan.frame(&[(0, 0.25)], &[0]);
    assert_eq!(plan.runs, [(3, 1), (4, 4)], "tile 8 (mover 2) is skipped");
    plan.frame(&[(1, 0.25)], &[1]);
    assert_eq!(plan.runs, [(8, 1)]);
}

/// Live tiles group by mover: a mover's own probes take consecutive tiles,
/// a probe shared with a lower mover sits with that mover's, so one
/// travelling mover's tiles reach the texture in a run per tile row.
#[test]
fn a_movers_live_tiles_are_consecutive() {
    let section = depth_section(vec![
        (1, 2, uniform_tile(1.0, 1.0), uniform_tile(2.0, 4.0)),
        (1, 8, uniform_tile(1.0, 1.0), uniform_tile(2.0, 4.0)),
        (2, 1, uniform_tile(1.0, 1.0), uniform_tile(2.0, 4.0)),
        (2, 5, uniform_tile(1.0, 1.0), uniform_tile(2.0, 4.0)),
        (2, 8, uniform_tile(1.0, 1.0), uniform_tile(2.0, 4.0)),
        (2, 9, uniform_tile(1.0, 1.0), uniform_tile(2.0, 4.0)),
    ]);
    let mut plan = Plan::new(&section, &[]);
    assert_eq!(plan.depth.probes(), [2, 8, 1, 5, 9]);
    plan.frame(&[(1, 0.5)], &[1]);
    // Probe 8 (tile 1), then mover 2's own 1, 5, 9 (tiles 2–4); four tiles
    // per row break the run at tile 4.
    assert_eq!(plan.runs, [(1, 3), (4, 1)]);
}

/// After the first frame, planning two travelling
/// movers' tiles keeps every buffer at its capacity.
#[test]
fn a_frame_keeps_every_depth_buffer_at_its_capacity() {
    let section = depth_section(
        (0..40u32)
            .map(|probe| (probe % 2, probe, ramp_tile(0.5), ramp_tile(2.0)))
            .collect(),
    );
    let mut plan = Plan::new(&section, &[]);
    let mut sink = 0usize;
    plan.depth
        .plan(&[0.1, 0.9], &[0, 1], |_, _, bytes| sink += bytes.len());
    let before = plan.depth.capacities();
    for frame in 0..16 {
        let t = frame as f32 / 16.0;
        let probe = postretro_sim::alloc_probe::AllocSnapshot::arm();
        plan.depth
            .plan(&[t, 1.0 - t], &[0, 1], |_, _, bytes| sink += bytes.len());
        assert_eq!(probe.allocs_since(), 0, "frame {frame} allocates nothing");
    }
    assert_eq!(plan.depth.capacities(), before);
    assert!(sink > 0);
}

/// A closed-pose tile holding an infinite or NaN moment refuses the install,
/// as a non-finite bounce tile does; finite tiles load.
#[test]
fn a_non_finite_closed_pose_tile_is_refused() {
    let mut d1 = uniform_tile(2.0, 4.0);
    let finite = depth_section(vec![(1, 3, uniform_tile(1.0, 1.0), d1.clone())]);
    assert!(MoverDepthTiles::new(Some(&finite), &finite.mover_ids()).is_ok());
    for bad in [0x7c00, 0xfc00, 0x7e00] {
        d1[17][1] = bad;
        let section = depth_section(vec![(1, 3, uniform_tile(1.0, 1.0), d1.clone())]);
        let error = MoverDepthTiles::new(Some(&section), &section.mover_ids()).unwrap_err();
        assert!(error.to_string().contains("non-finite"), "{error}");
    }
}

/// The fast f16 paths agree with the shared converters bit for bit.
#[test]
fn fast_f16_conversions_match_the_shared_converters() {
    for bits in 0..=u16::MAX {
        let exponent = (bits >> 10) & 0x1f;
        if exponent == 0x1f && bits & 0x3ff != 0 {
            assert!(f16_to_f32(bits).is_nan());
            continue;
        }
        assert_eq!(f16_to_f32(bits).to_bits(), f16_bits_to_f32(bits).to_bits());
    }
    let mut value = 1.0e-9f32;
    while value < 1.0e6 {
        for candidate in [value, -value, f32::from_bits(value.to_bits() + 0x0fff)] {
            assert_eq!(
                f32_to_f16(candidate),
                f32_to_f16_bits(candidate),
                "{candidate}"
            );
        }
        value *= 1.0007;
    }
    for special in [0.0, -0.0, 65504.0, 65519.0, 65520.0, f32::INFINITY] {
        assert_eq!(f32_to_f16(special), f32_to_f16_bits(special), "{special}");
    }
}

/// `a + s·(b − a)` in f32, the blend's own arithmetic.
fn mix(a: f32, b: f32, s: f32) -> f32 {
    a + s * (b - a)
}

/// One `(mean, mean_sq)` moment set, as a uniform tile.
type Moments = (f32, f32);

fn tile_of(moments: Moments) -> Vec<[u16; 2]> {
    uniform_tile(moments.0, moments.1)
}

/// What one mover's blend between `lo` and `hi` at interval weight `w`
/// holds where a mid pose bounds the interval: the mean lerped and rounded
/// to f16, the variance (`E[d²] − E[d]²`, at least 0 at each end) lerped,
/// and `E[d²]` rebuilt from the rounded mean, capped at the largest f16.
fn mean_variance(lo: Moments, hi: Moments, w: f32) -> Moments {
    if w <= 0.0 {
        return lo;
    }
    let variance = |m: Moments| (m.1 - m.0 * m.0).max(0.0);
    let mean = f16(f32_to_f16_bits(mix(lo.0, hi.0, w)));
    let spread = mix(variance(lo), variance(hi), w).max(0.0);
    (mean, (spread + mean * mean).min(65504.0))
}

/// AC 15 (depth half): at each stored pose's t′ the live tile holds that
/// pose's tile bit for bit; between two stored poses, a mid pose bounding
/// each interval, it blends their mean and variance (owner, 2026-11-11), so
/// poses sharing one spread compose that spread with no added variance and
/// never pass through the base; out and back to one t′ writes the same
/// bytes.
#[test]
fn piecewise_depth_reads_each_stored_pose_exactly_and_blends_mean_and_variance_between() {
    let knots = vec![0.0, 0.25, 0.5, 1.0];
    let poses: [Moments; 4] = [(1.0, 2.0), (2.0, 5.0), (4.0, 17.0), (3.0, 10.0)];
    let base: Moments = (8.0, 65.0);
    let section = posed_depth_section(
        &[(1, (0.0, 1.0), knots.clone())],
        vec![(
            1,
            3,
            poses
                .iter()
                .map(|&pose| PosedDepth::Tile(tile_of(pose)))
                .collect(),
        )],
    );
    let mut probes = vec![averaged(0.0, 0.0); 4];
    probes[3] = averaged(base.0, base.1);
    let mut plan = Plan::with_bases(&section, &no_id59(), &probes);
    for (pose, &t) in knots.iter().enumerate() {
        plan.frame(&[(0, t)], &[0]);
        assert_eq!(
            plan.tile(0),
            &tile_of(poses[pose])[..],
            "pose {pose} at its knot"
        );
    }
    // (t′, lo, hi, w): the intervals [0, 0.25] and [0.5, 1] end at a
    // terminus and a mid pose, so they blend mean and variance too.
    for (t, lo, hi, w) in [
        (0.0625, 0, 1, 0.25),
        (0.125, 0, 1, 0.5),
        (0.375, 1, 2, 0.5),
        (0.625, 2, 3, 0.25),
        (0.875, 2, 3, 0.75),
    ] {
        plan.frame(&[(0, t)], &[0]);
        let expected = mean_variance(poses[lo], poses[hi], w);
        assert_eq!(
            plan.tile(0),
            &tile_of(expected)[..],
            "t′ {t} blends poses {lo}–{hi}"
        );
        let mean = mix(poses[lo].0, poses[hi].0, w);
        let texel = plan.tile(0)[0].map(f16);
        assert!(
            (texel[1] - texel[0] * texel[0] - 1.0).abs() < 0.02 && (texel[0] - mean).abs() < 0.01,
            "t′ {t}: every pose's spread is 1, and the blend keeps it: {texel:?}"
        );
    }
    plan.frame(&[(0, 0.375)], &[0]);
    let out = plan.tile(0).to_vec();
    plan.frame(&[(0, 0.9)], &[0]);
    plan.frame(&[(0, 0.375)], &[0]);
    assert_eq!(plan.tile(0), &out[..], "out and back reads alike");
}

/// A probe whose mid poses are all unmarked (outside the near-field domain,
/// or every one skipped) composes the plain through-base blend at every t′,
/// as a two-pose mover's probe does.
#[test]
fn termini_to_termini_depth_runs_through_the_base() {
    let (d0, d1, base): (Moments, Moments, Moments) = ((1.0, 2.0), (3.0, 10.0), (8.0, 65.0));
    let section = posed_depth_section(
        &[
            (1, (0.0, 1.0), vec![0.0, 0.25, 0.5, 1.0]),
            (2, (0.0, 1.0), vec![0.0, 1.0]),
        ],
        vec![
            (
                1,
                3,
                vec![
                    PosedDepth::Tile(tile_of(d0)),
                    PosedDepth::Across,
                    PosedDepth::Across,
                    PosedDepth::Tile(tile_of(d1)),
                ],
            ),
            (
                2,
                5,
                vec![PosedDepth::Tile(tile_of(d0)), PosedDepth::Tile(tile_of(d1))],
            ),
        ],
    );
    let mut probes = vec![averaged(0.0, 0.0); 6];
    probes[3] = averaged(base.0, base.1);
    probes[5] = averaged(base.0, base.1);
    let mut plan = Plan::with_bases(&section, &no_id59(), &probes);
    for t in [0.0, 0.1, 0.25, 0.4, 0.5, 0.6, 0.75, 0.9, 1.0] {
        plan.frame(&[(0, t), (1, t)], &[0, 1]);
        let expected = (
            through_base(d0.0, base.0, d1.0, t),
            through_base(d0.1, base.1, d1.1, t),
        );
        for live in [0, 1] {
            assert_eq!(
                plan.tile(live),
                &tile_of(expected)[..],
                "live tile {live} at t′ {t} runs through the base"
            );
        }
    }
}

/// AC 16 (depth half): two movers at one probe, with different pose counts
/// and an unmarked mid pose, each blend mean and variance between the poses
/// around their t′, then add their blended differences from the base at t′
/// between knots for both, and clamp to a valid moment set.
#[test]
fn two_posed_movers_at_one_probe_add_their_blended_differences() {
    let base: Moments = (8.0, 65.0);
    let (a0, a2, a3): (Moments, Moments, Moments) = ((6.0, 37.0), (2.0, 5.0), (7.0, 50.0));
    let (b0, b1): (Moments, Moments) = ((9.0, 82.0), (10.0, 101.0));
    let section = posed_depth_section(
        &[
            (1, (0.0, 1.0), vec![0.0, 0.25, 0.5, 1.0]),
            (2, (0.0, 1.0), vec![0.0, 0.5, 1.0]),
        ],
        vec![
            (
                1,
                3,
                vec![
                    PosedDepth::Tile(tile_of(a0)),
                    PosedDepth::Across,
                    PosedDepth::Tile(tile_of(a2)),
                    PosedDepth::Tile(tile_of(a3)),
                ],
            ),
            (
                2,
                3,
                vec![
                    PosedDepth::Tile(tile_of(b0)),
                    PosedDepth::Tile(tile_of(b1)),
                    PosedDepth::Base,
                ],
            ),
        ],
    );
    let mut probes = vec![averaged(0.0, 0.0); 4];
    probes[3] = averaged(base.0, base.1);
    let mut plan = Plan::with_bases(&section, &no_id59(), &probes);
    // Mover 1 at 0.375 blends across its unmarked pose 1, poses 0 → 2 at
    // w 0.75; mover 2 at 0.25 blends poses 0 → 1 at w 0.5.
    plan.frame(&[(0, 0.375), (1, 0.25)], &[0, 1]);
    let first = mean_variance(a0, a2, 0.75);
    let second = mean_variance(b0, b1, 0.5);
    let sum = |moment: fn(Moments) -> f32| {
        f64::from(moment(base))
            + (f64::from(moment(first)) - f64::from(moment(base)))
            + (f64::from(moment(second)) - f64::from(moment(base)))
    };
    let mean = sum(|m| m.0).clamp(0.0, 65504.0);
    let mean_sq = sum(|m| m.1).max(mean * mean).min(65504.0);
    assert_eq!(
        plan.tile(0),
        &tile_of((mean as f32, mean_sq as f32))[..],
        "base plus both movers' differences"
    );
}

/// AC 17 (depth half) / MP1, MP10: with ramp (0.6, 0.9) the game hands
/// t′ = ramp(t). Every probe reads the first pose for t < 0.6 and the last
/// for t > 0.9; a mid knot reads its pose at ramp(t_k); poses squeezed onto
/// t′ 0 or 1 are never read, so t just past 0.6 and just under 0.9 blend
/// from the first pose and into the last. No state returns the probe to its
/// base reference.
#[test]
fn a_ramped_movers_depth_reads_its_end_poses_past_the_ramp_edges() {
    // The game's ramp, mirrored.
    let ramp = |t: f32| ((t - 0.6) / (0.9 - 0.6)).clamp(0.0, 1.0);
    let knots = vec![0.0, 0.2, 0.4, 0.6, 0.8, 0.95, 1.0];
    let (first, mid, last, squeezed): (Moments, Moments, Moments, Moments) =
        ((1.0, 2.0), (2.0, 5.0), (3.0, 10.0), (40.0, 1700.0));
    let base: Moments = (8.0, 65.0);
    let pose = |moments: Moments| PosedDepth::Tile(tile_of(moments));
    let section = posed_depth_section(
        &[(1, (0.6, 0.9), knots)],
        vec![(
            1,
            3,
            vec![
                pose(first),
                pose(squeezed),
                pose(squeezed),
                pose(squeezed),
                pose(mid),
                pose(squeezed),
                pose(last),
            ],
        )],
    );
    let mut probes = vec![averaged(0.0, 0.0); 4];
    probes[3] = averaged(base.0, base.1);
    let mut plan = Plan::with_bases(&section, &no_id59(), &probes);
    for (t, expected) in [
        (0.3, first),
        (0.6, first),
        (0.8, mid),
        (0.9, last),
        (0.97, last),
    ] {
        plan.frame(&[(0, ramp(t))], &[0]);
        assert_eq!(plan.tile(0), &tile_of(expected)[..], "t {t}");
    }
    let mid_t = ramp(0.8);
    for t in [0.61, 0.7] {
        let w = ramp(t) / mid_t;
        plan.frame(&[(0, ramp(t))], &[0]);
        let expected = mean_variance(first, mid, w);
        assert_eq!(plan.tile(0), &tile_of(expected)[..], "t {t}");
    }
    for t in [0.85, 0.89] {
        let w = (ramp(t) - mid_t) / (1.0 - mid_t);
        plan.frame(&[(0, ramp(t))], &[0]);
        let expected = mean_variance(mid, last, w);
        assert_eq!(plan.tile(0), &tile_of(expected)[..], "t {t}");
    }
    plan.frame(&[(0, -1.0)], &[0]);
    assert_eq!(plan.references, [(3, None)], "no state reads the base");
}

/// Per-frame CPU cost of one travelling mover's live tiles on a real map:
/// the blend and run assembly the frame does before staging. Ignored: set
/// `MOVER_DEPTH_TIMING_PRL` to a compiled map with an id-57 depth block.
/// Prints each mover's tiles, runs and bytes per frame and its mean time.
#[test]
#[ignore = "requires MOVER_DEPTH_TIMING_PRL: a compiled map with an id-57 depth block"]
fn a_travelling_movers_live_tile_plan_cost_on_a_real_map() {
    let path = std::env::var("MOVER_DEPTH_TIMING_PRL").expect("MOVER_DEPTH_TIMING_PRL");
    let world = postretro_level_loader::load_prl(&path).expect("the map loads");
    let section = world.mover_probe_channel().expect("an id-57 section");
    let id59 = world.probe_depth_tiles();
    let ids = section.mover_ids();
    let mut depth = MoverDepthTiles::new(Some(section), &ids).expect("the depth block loads");
    let live = depth.live_tile_count();
    let references = ProbeDepthTileReferences::from_section(id59, live);
    depth
        .bind(
            id59.map_or(0, |tiles| tiles.tile_count() as u32),
            references.texture_layout().expect("tiles lay out"),
            &references,
            DepthBases {
                id59,
                probes: world
                    .sh_volume()
                    .map_or(&[], |volume| volume.probes.as_slice()),
            },
        )
        .expect("every probe has a base");
    let mut t = vec![0.0f32; ids.len()];
    let all: Vec<u32> = (0..ids.len() as u32).collect();
    depth.plan(&t, &all, |_, _, _| {});
    eprintln!(
        "[MoverDepthTiming] {path}: {live} live tile(s), {} mover(s), {} B CPU-held",
        ids.len(),
        depth.host_bytes()
    );
    const FRAMES: u32 = 240;
    for slot in 0..ids.len() {
        let (mut runs, mut bytes) = (0usize, 0usize);
        let started = std::time::Instant::now();
        for frame in 0..FRAMES {
            t[slot] = (frame as f32 + 0.5) / FRAMES as f32;
            depth.plan(&t, &[slot as u32], |_, _, run| {
                runs += 1;
                bytes += run.len();
            });
        }
        let per_frame = started.elapsed().as_secs_f64() * 1e3 / f64::from(FRAMES);
        eprintln!(
            "[MoverDepthTiming] mover {}: {} tile(s), {} run(s), {} B per frame; {per_frame:.4} ms per frame (CPU blend and run assembly)",
            ids[slot],
            depth.tiles_written(),
            runs / FRAMES as usize,
            bytes / FRAMES as usize,
        );
    }
}
