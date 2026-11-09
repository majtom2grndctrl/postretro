// CPU proofs for the mover depth live-tile plan: the blend, the write and
// reference triggers, the two-mover rule, run coalescing and the frame's
// allocations. GPU proofs live in `mover_depth_tiles_tests.rs`.
// See: context/lib/rendering_pipeline.md §4 (Mover state lighting)

use super::*;
use crate::render::mover_channel::depth_test_support::{depth_section, ramp_tile, uniform_tile};
use postretro_level_format::probe_depth_tiles::ProbeDepthTilesSection;

struct Plan {
    depth: MoverDepthTiles,
    t: Vec<f32>,
    /// What the last frame wrote: `(tile, texels)` per tile, and the runs.
    tiles: Vec<(u32, Vec<[u16; 2]>)>,
    runs: Vec<(u32, u32)>,
    references: Vec<ReferenceChange>,
}

impl Plan {
    /// `section` bound after `id59` id-59 tiles, which tile `id59_probes`.
    fn new(section: &MoverProbeChannelSection, id59_probes: &[u32]) -> Self {
        let ids = section.mover_ids();
        let mut depth = MoverDepthTiles::new(Some(section), &ids);
        let id59 = ProbeDepthTilesSection {
            probe_indices: id59_probes.to_vec(),
            texels: vec![[0, 0]; id59_probes.len() * PROBE_DEPTH_TILE_TEXELS],
        };
        let live = depth.live_tile_count();
        let references = ProbeDepthTileReferences::from_section(Some(&id59), live);
        depth.bind(
            id59_probes.len() as u32,
            references.texture_layout().expect("tiles lay out"),
            &references,
        );
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

/// Depth row D1, CPU half: a live tile reads D₀ at t′ = 0, D₁ at t′ = 1 and
/// their linear mix at 0.5 within one f16 ulp, rising and after a reversal;
/// the first write points the probe at it, and later ones leave the
/// reference alone.
#[test]
fn a_live_tile_blends_its_movers_closed_pose_tiles_exact_at_both_termini() {
    let (d0, d1) = (ramp_tile(0.25), ramp_tile(3.0));
    let section = depth_section(vec![(4, 9, d0.clone(), d1.clone())]);
    let mut plan = Plan::new(&section, &[2, 9]);
    let live = 2;
    let exact = |t: f32| {
        let (d0, d1) = (d0.clone(), d1.clone());
        move |at: usize, channel: usize| (1.0 - t) * f16(d0[at][channel]) + t * f16(d1[at][channel])
    };
    plan.frame(&[(0, 0.0)], &[0]);
    assert_eq!(plan.tile(live), &d0[..], "t′ = 0 holds D₀ bit for bit");
    assert_eq!(plan.references, [(9, Some(live as u16))]);
    for t in [0.5, 1.0, 0.5, 0.0, 0.3] {
        plan.frame(&[(0, t)], &[0]);
        assert!(plan.references.is_empty(), "the reference already points");
        match t {
            0.0 => assert_eq!(plan.tile(live), &d0[..]),
            1.0 => assert_eq!(plan.tile(live), &d1[..]),
            _ => assert_within_ulp(plan.tile(live), exact(t)),
        }
    }
}

/// Depth rows D1 (bit-for-bit clause) and D2's no-state clause, CPU half: a
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

/// Depth row D3: a frame with no change writes nothing; a travelling mover
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
    // Probe 4: mover 1 at 0.5 reads E[d] 2.0, mover 2 held at 1 reads 0.5;
    // the nearer pair is the held mover's.
    assert_eq!(plan.tile(1), &uniform_tile(0.5, 0.25)[..]);
}

/// Depth row D6 (P26): two movers changing one probe write its live tile
/// once a frame, with bytes that depend only on their t′, whichever state
/// is handed over first; each texel holds the nearer mover's blended pair.
#[test]
fn two_movers_at_one_probe_write_it_once_with_the_nearer_blend_per_texel() {
    // Mover 1 closes near on the low texels, mover 2 on the high ones.
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
    let tile = |states: &[(usize, f32)]| {
        let mut plan = Plan::new(&section, &[]);
        plan.frame(states, &[0, 1]);
        assert_eq!(plan.runs, [(0, 1)], "one write of the shared tile");
        plan.tile(0).to_vec()
    };
    let both_closed = tile(&[(0, 0.0), (1, 0.0)]);
    assert_eq!(both_closed, tile(&[(1, 0.0), (0, 0.0)]), "hand-over order");
    for (at, texel) in both_closed.iter().enumerate() {
        assert_eq!(f16(texel[0]), 0.5, "texel {at}: the nearer door");
    }
    let one_open = tile(&[(0, 1.0), (1, 0.0)]);
    assert_eq!(one_open, near_high.to_vec());
    assert_eq!(tile(&[(0, 1.0), (1, 1.0)]), open);
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

/// Depth row D12, CPU half: after the first frame, planning two travelling
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
    let mut depth = MoverDepthTiles::new(Some(section), &ids);
    let live = depth.live_tile_count();
    let references = ProbeDepthTileReferences::from_section(id59, live);
    depth.bind(
        id59.map_or(0, |tiles| tiles.tile_count() as u32),
        references.texture_layout().expect("tiles lay out"),
        &references,
    );
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
