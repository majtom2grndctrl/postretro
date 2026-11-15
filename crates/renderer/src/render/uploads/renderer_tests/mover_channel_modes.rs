//! Compiled maps: streaming `off` and `sync-proof` compose identical
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
//! - Indirect, at a probe n movers carry bounce at: compose writes, per
//!   mover, the lerp between the two stored or base-reading poses around t′
//!   (an unmarked mid pose blended across), summed, − (n − 1) × base, plus
//!   id 27's deltas, which the no-state frame also carries. A terminus a
//!   mover stores no state for, or a mid pose marked base, reads the base,
//!   so each lerp puts weight w on the base (its weight on each such end):
//!   the compose writes Σ stored parts + (Σ w − (n − 1)) × base + deltas.
//!   With c = n − 1 − Σ w, (with state) + c × (no state) is Σ stored
//!   parts plus (1 + c) × deltas, the same in both modes. Where c is 0 (one
//!   mover reading no base) that is the composed tile itself, bit for bit;
//!   at n = 0 the indirect does not move; elsewhere the two f16 stores'
//!   rounding bounds it ([`mode_gap_bound`]).
//!
//! The case runs twice. Under `--uncompressed-irradiance` both loads hold
//! the same bases, and every comparison is bit for bit
//! ([`c10_uncompressed_builds_compose_bit_for_bit_in_both_modes`]). On a
//! BC6H build every half stays within the derived bound
//! ([`c10_off_and_sync_proof_compose_the_same_channel_tiles_at_every_probe`]).
//! See: context/lib/rendering_pipeline.md §4 (Mover state lighting, Cluster
//! SH residency)
use super::real_level::{BounceAtProbe, Gate, RealLevel};
use crate::render::composed_tiles::{
    ATLAS_NAMES, ComposedReadback, TILE_HALVES, f16_step, f16_to_f32, read_depth_tiles,
};
use postretro_level_loader::ShStreamingMode;
use std::collections::{BTreeMap, BTreeSet};

/// No state, then three hand-overs (t′ = 0, 0.5, 1).
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

/// The most one f16 store moves a value, in ulps of the stored half. WGSL
/// converts an f32 that f16 cannot hold to one of its two neighbours, so a
/// store errs by under one ulp of the half it writes. Nearest rounding
/// (half an ulp) is not what this GPU does: at enemy-movers probe 1255,
/// t′ 0.5, a half stored as 2⁻⁵ in one mode sits 1.81 ulps of its
/// counterpart's binade from where nearest rounding allows 1.59, which
/// rounding toward zero explains.
const F16_STORE_ERROR_ULPS: f32 = 1.0;

/// The compose's own f32 arithmetic, per unit of the magnitudes involved:
/// a handful of f32 roundings (2⁻²⁴ each) in the lerp, the base term and
/// the test's c × a₀, and the GPU's interval weight differing from the
/// CPU's c by a few f32 ulps, which moves the base term even where c is
/// near 0. 2⁻¹⁸ covers 64 such roundings. On |a| and |b| it is under 1/32
/// of their store term (an f16 ulp exceeds 2⁻¹¹ of its value).
const F32_COMPOSE_SLACK: f32 = 1.0 / 262_144.0;

/// How far two stores' errors can differ: both modes store through one
/// shader into one format on one adapter, so one rounding rule applies to
/// both. Under any such rule (toward zero, up, down or nearest) two values
/// of one sign err the same way, and their errors differ by less than the
/// larger ulp; values of opposite sign (or a zero) may err apart, by under
/// the sum.
fn store_pair_error(a: f32, b: f32) -> f32 {
    let (ulp_a, ulp_b) = (f16_step(a), f16_step(b));
    let ulps = if a * b > 0.0 {
        ulp_a.max(ulp_b)
    } else {
        ulp_a + ulp_b
    };
    F16_STORE_ERROR_ULPS * ulps
}

/// How far the two modes' channel output may differ at one half, derived.
///
/// In each mode m the compose computes an f32 x_m and stores a = fl(x_m),
/// and the no-state frame stores a₀ = fl(y_m). Their combination
/// x_m + c·y_m is the same real number in both modes: the stored parts and
/// id 27's deltas are mode-independent (the "one mover" class proves it,
/// bit for bit on BC6H), and the base each mode decodes cancels exactly.
/// Only the bases differ between modes (BC6H decodes them differently), so
/// x and y land at different places between f16 neighbours and the
/// roundings do not cancel. With e = fl(x) − x and O_m = a + c·a₀ read
/// back:
///
/// |O_off − O_sync| = |(e_a − e_b) + c·(e_a₀ − e_b₀)|
///                  ≤ [`store_pair_error`](a, b) + |c|·[`store_pair_error`](a₀, b₀),
///
/// plus [`F32_COMPOSE_SLACK`] on every magnitude. `weight` is |c|: the
/// indirect's n − 1 − Σ w, or 1 for a direct atlas's (with − without).
/// In f16 steps at the largest of |a|, |b| and |c|·a₀ this is under 3 (|c|·
/// ulp a₀ is under twice ulp(|c|·a₀)) and at most 2 where |c| is a power of
/// two; computed per half from the read values it is tighter than either.
/// Where c is 0 and no base enters, x is the same f32 in both modes and the
/// bound is 0: those classes are judged bit for bit.
fn mode_gap_bound([a, a0, b, b0]: [f32; 4], weight: f32) -> f32 {
    store_pair_error(a, b)
        + weight * store_pair_error(a0, b0)
        + F32_COMPOSE_SLACK * (a.abs() + b.abs() + (weight + 1.0) * (a0.abs() + b0.abs()))
}

/// How far two tile sets differ, half by half.
#[derive(Default, Debug)]
struct Gap {
    halves: usize,
    /// Halves NaN or infinite in either mode: a miss under any bar, which
    /// the `f32::max` folds below would otherwise drop.
    non_finite: usize,
    differing: usize,
    /// Worst difference in f16 steps at the half's scale, for the report.
    worst_steps: f32,
    worst_abs: f32,
    /// Worst difference as a fraction of its half's [`mode_gap_bound`];
    /// past 1 is a miss.
    worst_of_bound: f32,
}

impl Gap {
    /// One half: `a` and `b` the two modes' outputs, `scale` the magnitude
    /// the report's steps are taken at, `bound` the half's allowed gap.
    fn add(&mut self, a: f32, b: f32, scale: f32, bound: f32) {
        self.halves += 1;
        if !(a.is_finite() && b.is_finite()) {
            self.non_finite += 1;
            return;
        }
        if a.to_bits() == b.to_bits() || a == b {
            return;
        }
        self.differing += 1;
        let gap = (a - b).abs();
        self.worst_abs = self.worst_abs.max(gap);
        self.worst_steps = self.worst_steps.max(gap / f16_step(scale));
        self.worst_of_bound = self.worst_of_bound.max(gap / bound);
    }

    /// Stored tiles, compared unjudged: the no-state baseline.
    fn tiles(&mut self, a: &Tile, b: &Tile) {
        for (&a, &b) in a.iter().zip(b) {
            let (a, b) = (f16_to_f32(a), f16_to_f32(b));
            self.add(a, b, a.abs().max(b.abs()), f32::INFINITY);
        }
    }

    /// Whether every half holds `bar`, and none is NaN or infinite.
    fn holds(&self, bar: Bar) -> bool {
        self.non_finite == 0
            && match bar {
                Bar::BitForBit => self.differing == 0,
                Bar::Derived => self.worst_of_bound <= 1.0,
            }
    }
}

/// Halves a compose clamped at zero in either mode. A clamp is 1-Lipschitz,
/// so before rounding the two modes' clamped values differ by no more than
/// |c| × the bases' difference, and each base lies within ½ ulp of its
/// no-state half: |a − b| ≤ |c|·|a₀ − b₀| + [`mode_gap_bound`].
#[derive(Default, Debug)]
struct Clamped {
    halves: usize,
    /// Halves with any of the four values NaN or infinite: a miss.
    non_finite: usize,
    differing: usize,
    /// Worst amount past |c|·|a₀ − b₀|, as a fraction of the half's
    /// [`mode_gap_bound`]; past 1 is a miss.
    worst_of_bound: f32,
}

impl Clamped {
    fn add(&mut self, [a, a0, b, b0]: [f32; 4], weight: f32) {
        self.halves += 1;
        if ![a, a0, b, b0, weight].iter().all(|value| value.is_finite()) {
            self.non_finite += 1;
            return;
        }
        if a.to_bits() == b.to_bits() {
            return;
        }
        self.differing += 1;
        let excess = (a - b).abs() - weight * (a0 - b0).abs();
        let bound = mode_gap_bound([a, a0, b, b0], weight);
        self.worst_of_bound = self.worst_of_bound.max(excess / bound);
    }

    /// Whether every half holds `bar`, and none is NaN or infinite.
    fn holds(&self, bar: Bar) -> bool {
        self.non_finite == 0
            && match bar {
                Bar::BitForBit => self.differing == 0,
                Bar::Derived => self.worst_of_bound <= 1.0,
            }
    }
}

/// Indirect channel classes: no bounce mover at the probe; one mover whose
/// lerp reads no base at this t′; one mover reading the base at an unstored
/// terminus; two or more movers.
const CLASS_NAMES: [&str; 4] = [
    "no bounce",
    "one mover",
    "one mover reading id 34",
    "several movers",
];

/// The weight the no-state frame takes in the mode-independent indirect
/// output at a probe carried by `movers`, all at `t`: c = n − 1 − Σ w, w
/// being each mover's lerp weight on the base
/// ([`BounceAtProbe::base_weight`]).
pub(super) fn base_weight(movers: &[BounceAtProbe], t: f32) -> f32 {
    let reads_base: f32 = movers.iter().map(|mover| mover.base_weight(t)).sum();
    movers.len() as f32 - 1.0 - reads_base
}

/// The per-map numbers, printed and asserted by the caller.
struct MapGaps {
    /// Every stored tile with no state, off against sync-proof.
    baseline: [Gap; 3],
    /// Per handed t: the indirect channel output per bounce class.
    indirect: Vec<[Gap; 4]>,
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

fn compare(
    off: &ModeTiles,
    sync: &ModeTiles,
    bounce: &BTreeMap<u32, Vec<BounceAtProbe>>,
) -> MapGaps {
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
    for (at, t) in TS.iter().enumerate().skip(1) {
        let t = t.expect("a handed t");
        let mut indirect: [Gap; 4] = Default::default();
        let mut direct: [Gap; 2] = Default::default();
        let mut clamped = Clamped::default();
        let mut moved = [(0, 0); 3];
        for (probe, off_now) in &off.channel[at] {
            let (off_none, sync_now, sync_none) = (
                &off.channel[0][probe],
                &sync.channel[at][probe],
                &sync.channel[0][probe],
            );
            let stored = bounce.get(probe).map_or(&[][..], Vec::as_slice);
            let movers = stored.len();
            let base = base_weight(stored, t);
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
                        let bound = mode_gap_bound([a, a0, b, b0], 1.0);
                        direct[atlas - 1].add(a - a0, b - b0, scale, bound);
                        continue;
                    }
                    let class = match movers {
                        0 => 0,
                        1 if base == 0.0 => 1,
                        1 => 2,
                        _ => 3,
                    };
                    if class == 0 {
                        // No bounce entry: the indirect holds in each mode.
                        indirect[0].add(a, a0, a.abs().max(a0.abs()), 0.0);
                        indirect[0].add(b, b0, b.abs().max(b0.abs()), 0.0);
                        continue;
                    }
                    if class > 1 && clamps {
                        clamped.add([a, a0, b, b0], base.abs());
                        continue;
                    }
                    let scale = a
                        .abs()
                        .max(b.abs())
                        .max((base * a0).abs())
                        .max((base * b0).abs());
                    // One mover reading no base: c is 0 and the same f32
                    // reaches both stores, so the bound is 0.
                    let bound = if class == 1 {
                        0.0
                    } else {
                        mode_gap_bound([a, a0, b, b0], base.abs())
                    };
                    indirect[class].add(a + base * a0, b + base * b0, scale, bound);
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

/// How a C10 run judges its maps.
#[derive(Clone, Copy, PartialEq)]
enum Bar {
    /// Both loads hold the same bases: every half bit for bit.
    BitForBit,
    /// BC6H bases: every half within its [`mode_gap_bound`].
    Derived,
}

/// The C10 comparison over every map `var` names (`:`-separated compiled
/// PRLs, each with id 50 and id 57), judged by `bar`; the misses. `None`
/// when no adapter is present.
fn c10_misses(var: &str, bar: Bar) -> Option<Vec<String>> {
    let paths = std::env::var(var).unwrap_or_else(|_| panic!("set {var}"));
    let mut misses = Vec::new();
    for path in paths.split(':').filter(|path| !path.is_empty()) {
        let probe_level = RealLevel::load(path, ShStreamingMode::Off)?;
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
        let several = bounce.values().filter(|movers| movers.len() > 1).count();
        let most = bounce.values().map(Vec::len).max().unwrap_or(0);
        let reading_base = bounce
            .values()
            .flatten()
            .filter(|mover| (0..mover.marks.len() as u32).any(|pose| mover.reads_base(pose)))
            .count();
        eprintln!(
            "[C10] {name}: {} channel probes (direct {}, bounce {} of which {several} under several movers, at most {most} at one probe, {reading_base} (probe, mover) pair(s) reading the base at some pose, depth {})",
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
        let same_bases = gaps
            .baseline
            .iter()
            .all(|gap| gap.differing == 0 && gap.non_finite == 0);
        if bar == Bar::BitForBit && !same_bases {
            misses.push(format!(
                "{name}: the loads' bases differ with no state; build it \
                 `--uncompressed-irradiance`"
            ));
        }
        let check = |misses: &mut Vec<String>, what: String, gap: &Gap| {
            eprintln!("[C10] {name} {what}: {gap:?}");
            if gap.non_finite > 0 {
                misses.push(format!(
                    "{name} {what}: {} non-finite half(s) in a mode",
                    gap.non_finite
                ));
            } else if !gap.holds(bar) {
                misses.push(format!("{name} {what}: {gap:?} past its bar"));
            }
        };
        for (at, t) in TS.iter().enumerate().skip(1) {
            let t = t.unwrap();
            for (class, gap) in gaps.indirect[at - 1].iter().enumerate() {
                check(
                    &mut misses,
                    format!("t′ {t} indirect output, {}", CLASS_NAMES[class]),
                    gap,
                );
            }
            for (atlas, gap) in gaps.direct[at - 1].iter().enumerate() {
                check(
                    &mut misses,
                    format!("t′ {t} {} output", ATLAS_NAMES[atlas + 1]),
                    gap,
                );
            }
            let clamped = &gaps.clamped[at - 1];
            eprintln!(
                "[C10] {name} t′ {t}: moved halves (off, sync-proof) {:?}; clamped at zero in a mode: {clamped:?}",
                gaps.moved[at - 1]
            );
            if clamped.non_finite > 0 {
                misses.push(format!(
                    "{name} t′ {t}: {} clamped half(s) non-finite in a mode",
                    clamped.non_finite
                ));
            } else if !clamped.holds(bar) {
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
    Some(misses)
}

/// Each map named by `MOVER_C10_PRLS`, built with BC6H bases. The channel's
/// output at every probe it touches agrees across `off` and `sync-proof`,
/// base taken out as the module docs derive: indirect bit for bit where one
/// mover reading no base carries bounce and unmoved where none does; within
/// each half's [`mode_gap_bound`] where one reads the base or several carry
/// it, at any mover count and pose count, and in the direct atlases. A half
/// either mode clamped at zero differs from its counterpart by no more than
/// the weighted bases plus that bound ([`Clamped`]). The depth-tile texture
/// is bit-identical between the modes at every t′.
#[test]
#[ignore = "requires MOVER_C10_PRLS: `:`-separated compiled PRLs (mover-closet, mover-setpiece, \
            mover-setpiece built with --mover-payload-resident-bar 0, enemy-movers)"]
fn c10_off_and_sync_proof_compose_the_same_channel_tiles_at_every_probe() {
    let Some(misses) = c10_misses("MOVER_C10_PRLS", Bar::Derived) else {
        return;
    };
    assert!(misses.is_empty(), "C10 misses:\n{}", misses.join("\n"));
}

/// Each map named by `MOVER_C10_UNCOMPRESSED_PRLS`, built
/// `--uncompressed-irradiance`: both loads hold the same bases, so every
/// class, both direct atlases, every clamped half and the depth tiles agree
/// bit for bit at every t′.
#[test]
#[ignore = "requires MOVER_C10_UNCOMPRESSED_PRLS: `:`-separated compiled PRLs built \
            --uncompressed-irradiance (mover-closet, mover-setpiece, enemy-movers)"]
fn c10_uncompressed_builds_compose_bit_for_bit_in_both_modes() {
    let Some(misses) = c10_misses("MOVER_C10_UNCOMPRESSED_PRLS", Bar::BitForBit) else {
        return;
    };
    assert!(misses.is_empty(), "C10 misses:\n{}", misses.join("\n"));
}

// A NaN or infinite half in either mode is a miss by name under either bar,
// though the folds over worst differences would drop it.
#[test]
fn a_non_finite_half_in_either_mode_misses_every_bar() {
    for (a, b) in [(f32::NAN, 1.0), (1.0, f32::INFINITY), (f32::NAN, f32::NAN)] {
        let mut gap = Gap::default();
        gap.add(1.0, 1.0, 1.0, 1.0);
        gap.add(a, b, 1.0, 1.0);
        assert_eq!(gap.non_finite, 1, "{a} against {b}");
        assert!(!gap.holds(Bar::Derived) && !gap.holds(Bar::BitForBit));
        let mut clamped = Clamped::default();
        clamped.add([a, 0.5, b, 0.5], 1.0);
        assert_eq!(clamped.non_finite, 1);
        assert!(!clamped.holds(Bar::Derived) && !clamped.holds(Bar::BitForBit));
    }
    let mut finite = Gap::default();
    finite.add(1.0, 1.0, 1.0, 1.0);
    assert!(finite.holds(Bar::BitForBit) && finite.holds(Bar::Derived));
}

// One f16 step is 2⁻²⁴ through the subnormals and at the least normal half,
// and 2⁻¹⁰ of the binade above it: the bound never takes a subnormal half's
// step at half its size.
#[test]
fn an_f16_step_is_the_spacing_of_halves_down_through_the_subnormals() {
    let least_normal = 2f32.powi(-14);
    for value in [
        0.0,
        2f32.powi(-24),
        3.0e-6,
        least_normal * 0.999,
        least_normal,
    ] {
        assert_eq!(f16_step(value), 2f32.powi(-24), "at {value}");
        assert_eq!(f16_step(-value), 2f32.powi(-24), "at −{value}");
    }
    assert_eq!(f16_step(1.0), 2f32.powi(-10));
    assert_eq!(f16_step(1.5), 2f32.powi(-10));
    assert_eq!(f16_step(2.0), 2f32.powi(-9));
}
