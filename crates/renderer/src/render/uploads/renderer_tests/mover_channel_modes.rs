//! C10 on compiled maps: streaming `off` and `sync-proof` compose identical
//! channel output. The same PRL is loaded whole (the legacy atlases) and
//! streamed with every cluster resident (the dense slot pools). With every
//! mover handed no state, then t′ = 0, 0.5 and 1, each probe the channel
//! touches is read back from the indirect atlas and both direct atlases
//! through that load's own indirection word, and the depth-tile texture is
//! read whole.
//!
//! The no-state frame also compares every stored tile of both loads. On a
//! BC6H build they differ before any mover state: the whole load decodes id
//! 34 and id 35 from their compact atlases, whose 4×4 blocks straddle the
//! 6-texel tiles, while id 50 re-encodes each tile isolated in an 8×8 cell.
//! The channel's output is therefore compared with the base each mode
//! composes taken out:
//! - Direct atlases: the gated term replaces the light's mover-absent term,
//!   so the output is (with state − no state), equal across modes up to the
//!   two f16 stores' rounding.
//! - Indirect, at a probe n movers carry bounce at: compose writes
//!   Σ lerp(S^b₀, S^b₁, t′) − (n − 1) × base, plus id 27's deltas, which the
//!   no-state frame also carries; so (with state) + (n − 1) × (no state) is
//!   Σ lerp + n × deltas, the same in both modes. At n = 1 that is the
//!   composed tile itself, bit for bit; at n = 0 the indirect does not move.
//!
//! Under `--uncompressed-irradiance` both loads hold the same bases, and
//! every comparison is bit for bit.
//! See: context/lib/rendering_pipeline.md §4 (Mover state lighting, Cluster
//! SH residency)
use super::real_level::{Gate, RealLevel};
use crate::render::composed_tiles::{
    ATLAS_NAMES, ComposedReadback, TILE_HALVES, f16_step, f16_to_f32, read_depth_tiles,
};
use postretro_level_loader::ShStreamingMode;
use std::collections::{BTreeMap, BTreeSet};

/// No state, then the three hand-overs C10 names.
const TS: [Option<f32>; 4] = [None, Some(0.0), Some(0.5), Some(1.0)];

type Tile = [u16; TILE_HALVES];
/// One probe's tile in each composed atlas, `None` where the load has none.
type ProbeTiles = [Option<Tile>; 3];

/// What one mode composed: per t, the channel probes' tiles and the depth
/// tile texture; and the no-state frame's every stored tile, keyed by the
/// first probe (grid order) naming its slot span, so both loads key alike.
struct ModeTiles {
    channel: Vec<BTreeMap<u32, ProbeTiles>>,
    depth: Vec<Vec<u16>>,
    stored: BTreeMap<u32, Vec<ProbeTiles>>,
}

fn probe_tiles(readback: &ComposedReadback, probe: u32) -> ProbeTiles {
    [0, 1, 2].map(|atlas| readback.l0_tile(atlas, probe))
}

fn stored_tiles(readback: &ComposedReadback) -> BTreeMap<u32, Vec<ProbeTiles>> {
    let mut seen = BTreeSet::new();
    let mut stored = BTreeMap::new();
    for probe in 0..readback.words.len() as u32 {
        let slots = readback.stored_slots(probe);
        if slots.is_empty() || !seen.insert(slots.start) {
            continue;
        }
        let tiles = slots
            .map(|slot| {
                [0, 1, 2].map(|atlas| {
                    readback.atlases[atlas]
                        .as_ref()
                        .map(|readback| readback.tile(slot))
                })
            })
            .collect();
        stored.insert(probe, tiles);
    }
    stored
}

fn compose_mode(path: &str, mode: ShStreamingMode, probes: &BTreeSet<u32>) -> Option<ModeTiles> {
    let mut level = RealLevel::load(path, mode)?;
    let none = level.states(None);
    if mode != ShStreamingMode::Off {
        level.install_all(&none, Gate::All);
    }
    let mut tiles = ModeTiles {
        channel: Vec::new(),
        depth: Vec::new(),
        stored: BTreeMap::new(),
    };
    for t in TS {
        let states = level.states(t);
        level.frame(&states, Gate::All);
        let readback = level.read();
        if t.is_none() {
            tiles.stored = stored_tiles(&readback);
        }
        tiles.channel.push(
            probes
                .iter()
                .map(|&probe| (probe, probe_tiles(&readback, probe)))
                .collect(),
        );
        tiles.depth.push(read_depth_tiles(&level.renderer));
    }
    Some(tiles)
}

/// How far two tile sets differ, half by half.
#[derive(Default, Debug)]
struct Gap {
    halves: usize,
    differing: usize,
    worst_steps: f32,
    worst_abs: f32,
}

impl Gap {
    fn add(&mut self, a: f32, b: f32, scale: f32) {
        self.halves += 1;
        if a.to_bits() == b.to_bits() || a == b {
            return;
        }
        self.differing += 1;
        let gap = (a - b).abs();
        self.worst_abs = self.worst_abs.max(gap);
        self.worst_steps = self.worst_steps.max(gap / f16_step(scale));
    }

    fn tiles(&mut self, a: &Tile, b: &Tile) {
        for (&a, &b) in a.iter().zip(b) {
            let (a, b) = (f16_to_f32(a), f16_to_f32(b));
            self.add(a, b, a.abs().max(b.abs()));
        }
    }
}

/// Halves a compose clamped at zero in either mode. A clamp is 1-Lipschitz,
/// so the two modes' clamped halves may differ by no more than their bases,
/// weighted as the compose weighs them: |a − b| ≤ w × |a₀ − b₀|.
#[derive(Default, Debug)]
struct Clamped {
    halves: usize,
    /// Worst amount past w × |a₀ − b₀|, in f16 steps; zero or less holds.
    worst_excess_steps: f32,
}

impl Clamped {
    fn add(&mut self, [a, a0, b, b0]: [f32; 4], weight: f32) {
        self.halves += 1;
        let scale = a
            .abs()
            .max(b.abs())
            .max(weight * a0.abs())
            .max(weight * b0.abs());
        let excess = (a - b).abs() - weight * (a0 - b0).abs();
        self.worst_excess_steps = self.worst_excess_steps.max(excess / f16_step(scale));
    }
}

/// Indirect channel classes by bounce movers at the probe: 0, 1, 2 or more.
const CLASS_NAMES: [&str; 3] = ["no bounce", "one mover", "several movers"];

/// The per-map C10 numbers, printed and asserted by the caller.
struct MapGaps {
    /// Every stored tile with no state, off against sync-proof.
    baseline: [Gap; 3],
    /// Per handed t: the indirect channel output per bounce class.
    indirect: Vec<[Gap; 3]>,
    /// Per handed t: halves either mode clamped at zero (a several-mover
    /// bounce sum, or a gated direct term past its base), which the output
    /// comparison leaves to [`Clamped`].
    clamped: Vec<Clamped>,
    /// Per handed t: the direct atlases' channel output (with − without).
    direct: Vec<[Gap; 2]>,
    /// Per handed t and atlas: halves the channel moved in off, and in
    /// sync-proof.
    moved: Vec<[(usize, usize); 3]>,
    /// Per t (no state first): depth-tile halves that differ between modes,
    /// and halves the channel moved in off.
    depth: Vec<(usize, usize)>,
}

fn compare(off: &ModeTiles, sync: &ModeTiles, bounce: &BTreeMap<u32, u32>) -> MapGaps {
    let mut baseline: [Gap; 3] = Default::default();
    assert_eq!(
        off.stored.keys().collect::<Vec<_>>(),
        sync.stored.keys().collect::<Vec<_>>(),
        "both loads store the same nodes"
    );
    for (probe, off_tiles) in &off.stored {
        let sync_tiles = &sync.stored[probe];
        assert_eq!(off_tiles.len(), sync_tiles.len());
        for (a, b) in off_tiles.iter().zip(sync_tiles) {
            for atlas in 0..3 {
                assert_eq!(
                    a[atlas].is_some(),
                    b[atlas].is_some(),
                    "{}: both loads compose it or neither",
                    ATLAS_NAMES[atlas]
                );
                if let (Some(a), Some(b)) = (&a[atlas], &b[atlas]) {
                    baseline[atlas].tiles(a, b);
                }
            }
        }
    }
    let mut gaps = MapGaps {
        baseline,
        indirect: Vec::new(),
        clamped: Vec::new(),
        direct: Vec::new(),
        moved: Vec::new(),
        depth: Vec::new(),
    };
    for at in 1..TS.len() {
        let mut indirect: [Gap; 3] = Default::default();
        let mut direct: [Gap; 2] = Default::default();
        let mut clamped = Clamped::default();
        let mut moved = [(0, 0); 3];
        for (probe, off_now) in &off.channel[at] {
            let (off_none, sync_now, sync_none) = (
                &off.channel[0][probe],
                &sync.channel[at][probe],
                &sync.channel[0][probe],
            );
            let movers = bounce.get(probe).copied().unwrap_or(0);
            for atlas in 0..3 {
                let (Some(a), Some(a0), Some(b), Some(b0)) = (
                    &off_now[atlas],
                    &off_none[atlas],
                    &sync_now[atlas],
                    &sync_none[atlas],
                ) else {
                    assert!(
                        off_now[atlas].is_none() && sync_now[atlas].is_none(),
                        "probe {probe} {}: both loads hold its L0 tile or neither",
                        ATLAS_NAMES[atlas]
                    );
                    continue;
                };
                for half in 0..TILE_HALVES {
                    let [a, a0, b, b0] = [a[half], a0[half], b[half], b0[half]].map(f16_to_f32);
                    moved[atlas].0 += usize::from(a.to_bits() != a0.to_bits());
                    moved[atlas].1 += usize::from(b.to_bits() != b0.to_bits());
                    let clamps = a == 0.0 || b == 0.0;
                    if atlas > 0 {
                        if clamps {
                            clamped.add([a, a0, b, b0], 1.0);
                            continue;
                        }
                        let scale = a.abs().max(a0.abs()).max(b.abs()).max(b0.abs());
                        direct[atlas - 1].add(a - a0, b - b0, scale);
                        continue;
                    }
                    let class = movers.min(2) as usize;
                    if class == 0 {
                        // No bounce entry: the indirect holds in each mode.
                        indirect[0].add(a, a0, a.abs().max(a0.abs()));
                        indirect[0].add(b, b0, b.abs().max(b0.abs()));
                        continue;
                    }
                    let base = movers as f32 - 1.0;
                    if class == 2 && clamps {
                        clamped.add([a, a0, b, b0], base);
                        continue;
                    }
                    let scale = a
                        .abs()
                        .max(b.abs())
                        .max((base * a0).abs())
                        .max((base * b0).abs());
                    indirect[class].add(a + base * a0, b + base * b0, scale);
                }
            }
        }
        gaps.indirect.push(indirect);
        gaps.clamped.push(clamped);
        gaps.direct.push(direct);
        gaps.moved.push(moved);
    }
    gaps.depth = (0..TS.len())
        .map(|at| {
            assert_eq!(
                off.depth[at].len(),
                sync.depth[at].len(),
                "one tile texture"
            );
            let differ = off.depth[at]
                .iter()
                .zip(&sync.depth[at])
                .filter(|(a, b)| a != b)
                .count();
            let live = off.depth[at]
                .iter()
                .zip(&off.depth[0])
                .filter(|(a, b)| a != b)
                .count();
            (differ, live)
        })
        .collect();
    gaps
}

/// The f16 stores' rounding, in steps at the largest magnitude involved:
/// each composed half lies within half a step of the f32 the compose
/// computed, and an output combines two halves per mode, one of them
/// weighted (n − 1) at a probe n movers carry bounce at. One step covers
/// the direct atlases and two movers, the most any fixture puts on a probe.
const ROUNDING_STEPS: f32 = 1.0;

/// C10 on each map named by `MOVER_C10_PRLS` (`:`-separated compiled PRLs,
/// each with id 50 and id 57). The channel's output at every probe it
/// touches equals across `off` and `sync-proof`, base taken out as the
/// module docs derive: indirect bit for bit where one mover carries bounce
/// and unmoved where none does; within [`ROUNDING_STEPS`] where two do, and
/// in the direct atlases. A half either mode clamped at zero differs from
/// its counterpart by no more than the weighted bases ([`Clamped`]). Where the two loads' bases agree (an
/// uncompressed build), every comparison is bit for bit. The depth-tile
/// texture is bit-identical between the modes at every t′.
#[test]
#[ignore = "requires MOVER_C10_PRLS: `:`-separated compiled PRLs (mover-closet, mover-setpiece, \
            mover-setpiece built with --mover-payload-resident-bar 0)"]
fn c10_off_and_sync_proof_compose_the_same_channel_tiles_at_every_probe() {
    let paths = std::env::var("MOVER_C10_PRLS").expect("set MOVER_C10_PRLS");
    let mut misses = Vec::new();
    for path in paths.split(':').filter(|path| !path.is_empty()) {
        let Some(probe_level) = RealLevel::load(path, ShStreamingMode::Off) else {
            return;
        };
        let (direct, bounce, depth) = (
            probe_level.direct_probes(),
            probe_level.bounce_probes(),
            probe_level.depth_probes(),
        );
        drop(probe_level);
        let probes: BTreeSet<u32> = direct
            .iter()
            .chain(bounce.keys())
            .chain(&depth)
            .copied()
            .collect();
        let off = compose_mode(path, ShStreamingMode::Off, &probes).expect("adapter present");
        let sync = compose_mode(path, ShStreamingMode::SyncProof, &probes).expect("adapter");
        let gaps = compare(&off, &sync, &bounce);
        let name = std::path::Path::new(path)
            .file_stem()
            .and_then(|stem| stem.to_str())
            .unwrap_or(path);
        let several = bounce.values().filter(|&&movers| movers > 1).count();
        assert!(
            bounce.values().all(|&movers| movers <= 2),
            "{name}: ROUNDING_STEPS covers at most two movers at a probe"
        );
        eprintln!(
            "[C10] {name}: {} channel probes (direct {}, bounce {} of which {several} under several movers, depth {})",
            probes.len(),
            direct.len(),
            bounce.len(),
            depth.len()
        );
        for (atlas, gap) in gaps.baseline.iter().enumerate() {
            eprintln!(
                "[C10] {name} {}: no-state baseline, every stored tile: {gap:?}",
                ATLAS_NAMES[atlas]
            );
        }
        let exact = gaps.baseline.iter().all(|gap| gap.differing == 0);
        let check = |misses: &mut Vec<String>, what: String, gap: &Gap, steps: f32| {
            let bar = if exact { 0.0 } else { steps };
            eprintln!("[C10] {name} {what}: {gap:?} (bar {bar} steps)");
            if gap.differing > 0 && (exact || gap.worst_steps > steps) {
                misses.push(format!("{name} {what}: {gap:?} past {bar} steps"));
            }
        };
        for (at, t) in TS.iter().enumerate().skip(1) {
            let t = t.unwrap();
            for (class, gap) in gaps.indirect[at - 1].iter().enumerate() {
                let steps = if class == 2 { ROUNDING_STEPS } else { 0.0 };
                check(
                    &mut misses,
                    format!("t′ {t} indirect output, {}", CLASS_NAMES[class]),
                    gap,
                    steps,
                );
            }
            for (atlas, gap) in gaps.direct[at - 1].iter().enumerate() {
                check(
                    &mut misses,
                    format!("t′ {t} {} output", ATLAS_NAMES[atlas + 1]),
                    gap,
                    ROUNDING_STEPS,
                );
            }
            let clamped = &gaps.clamped[at - 1];
            eprintln!(
                "[C10] {name} t′ {t}: moved halves (off, sync-proof) {:?}; clamped at zero in a mode: {clamped:?}",
                gaps.moved[at - 1]
            );
            if clamped.worst_excess_steps > ROUNDING_STEPS {
                misses.push(format!(
                    "{name} t′ {t}: clamped halves differ past their bases: {clamped:?}"
                ));
            }
        }
        for (at, (differ, live)) in gaps.depth.iter().enumerate() {
            eprintln!(
                "[C10] {name} t′ {:?}: depth tiles differ at {differ} halves between modes; the channel moved {live}",
                TS[at]
            );
            if *differ > 0 {
                misses.push(format!(
                    "{name} t′ {:?}: {differ} depth-tile halves differ between the modes",
                    TS[at]
                ));
            }
        }
        let moved: usize = gaps
            .moved
            .iter()
            .flatten()
            .map(|(off, _)| off)
            .sum::<usize>()
            + gaps.depth.iter().map(|(_, live)| live).sum::<usize>();
        if moved == 0 {
            misses.push(format!("{name}: the channel moves nothing"));
        }
    }
    assert!(misses.is_empty(), "C10 misses:\n{}", misses.join("\n"));
}
