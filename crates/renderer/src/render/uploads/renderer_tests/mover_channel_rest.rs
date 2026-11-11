//! Compiled maps: at rest, mid poses change no probe's channel contribution.
//! Each pair names a map built `--release` with two poses per mover by the
//! parity baseline compiler (`mover_probe_channel_prl_parity`: today's
//! terminus states as epoch 5, since the loader refuses the pre-change
//! epoch 4) and by this one, with mid poses. Each build loads
//! under `off` and under `sync-proof` with every cluster resident, hands
//! every mover no state, then t′ = 0 and t′ = 1, and reads back every probe
//! either build's channel touches.
//!
//! The channel's contribution is compared build against build with the base
//! taken out as C10 takes it out ([`super::mover_channel_modes`]): the
//! indirect output (with state) + c × (no state), c = n − 1 − Σ w over the
//! build's own bounce movers at the probe (a mover whose termini both read
//! the base adds 1 to n and 1 to Σ w, so a mid-pose-only entry leaves c as
//! it was), and each direct atlas's (with state − no state). Where the two
//! builds' no-state tiles agree the outputs must agree bit for bit; a probe
//! whose base moved (a cell mid poses newly pin to a finer storage level)
//! is judged within one f16 step of the base-free output, which no base
//! enters. On the whole load each probe's sampled moments must agree bit
//! for bit too, and a probe whose live tile comes only from a mid pose must
//! read its base: the moments it samples with no state.
//! See: context/lib/rendering_pipeline.md §4 (Mover state lighting) ·
//! context/lib/build_pipeline.md §Compiler pipeline step 12 (Mover probe
//! channel, Poses)
use super::mover_channel_modes::base_weight;
use super::real_level::{BounceAtProbe, Gate, RealLevel};
use crate::render::composed_tiles::{ATLAS_NAMES, TILE_HALVES, f16_step, f16_to_f32};
use postretro_level_loader::ShStreamingMode;
use std::collections::{BTreeMap, BTreeSet};

/// No state, then each terminus.
const TS: [Option<f32>; 3] = [None, Some(0.0), Some(1.0)];

type Tile = [u16; TILE_HALVES];
type ProbeTiles = [Option<Tile>; 3];
type Moments = Vec<[u16; 2]>;

/// What one build id 57 says about the probes, read from its whole load.
struct Channel {
    probes: BTreeSet<u32>,
    bounce: BTreeMap<u32, Vec<BounceAtProbe>>,
    mid_only_depth: BTreeSet<u32>,
}

fn channel(path: &str) -> Option<Channel> {
    let level = RealLevel::load(path, ShStreamingMode::Off)?;
    let bounce = level.bounce_probes();
    let probes = level
        .direct_probes()
        .into_iter()
        .chain(bounce.keys().copied())
        .chain(level.depth_probes())
        .collect();
    Some(Channel {
        probes,
        bounce,
        mid_only_depth: level.mid_pose_only_depth_probes(),
    })
}

/// One build under one mode: per t, each probe's tile in every atlas, and
/// on a whole load the moments each probe samples.
struct Composed {
    tiles: Vec<BTreeMap<u32, ProbeTiles>>,
    depth: Vec<BTreeMap<u32, Moments>>,
}

fn compose(path: &str, mode: ShStreamingMode, probes: &BTreeSet<u32>) -> Composed {
    let mut level = RealLevel::load(path, mode).expect("adapter present");
    if mode != ShStreamingMode::Off {
        let none = level.states(None);
        level.install_all(&none, Gate::All);
    }
    let mut composed = Composed {
        tiles: Vec::new(),
        depth: Vec::new(),
    };
    for t in TS {
        let states = level.states(t);
        level.frame(&states, Gate::All);
        let readback = level.read();
        composed.tiles.push(
            probes
                .iter()
                .map(|&probe| (probe, [0, 1, 2].map(|atlas| readback.l0_tile(atlas, probe))))
                .collect(),
        );
        if mode == ShStreamingMode::Off {
            composed.depth.push(level.sampled_depth(probes));
        }
    }
    composed
}

/// How far the two builds' outputs differ over one class of halves.
#[derive(Default, Debug)]
struct Gap {
    halves: usize,
    differing: usize,
    worst_steps: f32,
}

impl Gap {
    fn add(&mut self, a: f32, b: f32, scale: f32) {
        self.halves += 1;
        if a.to_bits() == b.to_bits() || a == b {
            return;
        }
        self.differing += 1;
        self.worst_steps = self.worst_steps.max((a - b).abs() / f16_step(scale));
    }
}

/// Per atlas, the halves whose two no-state tiles agree, and those on a
/// moved base.
#[derive(Default, Debug)]
struct AtlasGaps {
    same_base: Gap,
    moved_base: Gap,
}

/// The channel's output at one half: the indirect's base-free sum, or a
/// direct atlas's change from no state.
fn output(atlas: usize, with: f32, none: f32, c: f32) -> f32 {
    if atlas == 0 {
        with + c * none
    } else {
        with - none
    }
}

#[test]
#[ignore = "requires MOVER_REST_PAIRS: `:`-separated `old.prl=new.prl` pairs, each a map built \
            `--release` by the parity baseline compiler and by this one (mover-closet, \
            mover-setpiece, enemy-movers)"]
fn rest_channel_output_equals_the_pre_change_build() {
    let pairs = std::env::var("MOVER_REST_PAIRS").expect("set MOVER_REST_PAIRS");
    let mut misses = Vec::new();
    for pair in pairs.split(':').filter(|pair| !pair.is_empty()) {
        let (old_path, new_path) = pair.split_once('=').expect("an `old=new` pair");
        let name = std::path::Path::new(new_path)
            .file_stem()
            .and_then(|stem| stem.to_str())
            .unwrap_or(new_path);
        let Some(old) = channel(old_path) else {
            return;
        };
        let new = channel(new_path).expect("adapter present");
        let probes: BTreeSet<u32> = old.probes.union(&new.probes).copied().collect();
        let most = new.bounce.values().map(Vec::len).max().unwrap_or(0);
        let mid_only_bounce = new
            .bounce
            .values()
            .flatten()
            .filter(|mover| mover.reads_base(0) && mover.reads_base(mover.marks.len() as u32 - 1))
            .count();
        eprintln!(
            "[AC1] {name}: {} probes (pre-change {}, now {}); bounce at {} probes, at most {most} \
             movers at one; {mid_only_bounce} (probe, mover) bounce pair(s) and {} depth probe(s) \
             stored by mid poses alone",
            probes.len(),
            old.probes.len(),
            new.probes.len(),
            new.bounce.len(),
            new.mid_only_depth.len(),
        );
        // The comparison means something only where the channel moves some
        // half at some terminus (the closet's open door moves none).
        let mut moved_any = false;
        for mode in [ShStreamingMode::Off, ShStreamingMode::SyncProof] {
            let before = compose(old_path, mode, &probes);
            let after = compose(new_path, mode, &probes);
            for (at, t) in TS.iter().enumerate().skip(1) {
                let t = t.expect("a handed t");
                let mut gaps: [AtlasGaps; 3] = Default::default();
                let mut moved = [(0usize, 0usize); 3];
                let mut moved_base_probes = 0;
                let mut l0_in_one_build = BTreeSet::new();
                for &probe in &probes {
                    let c = |channel: &Channel| {
                        base_weight(channel.bounce.get(&probe).map_or(&[][..], Vec::as_slice), t)
                    };
                    let (c_old, c_new) = (c(&old), c(&new));
                    let mut base_moved = false;
                    for atlas in 0..3 {
                        let tiles = [
                            &before.tiles[at][&probe][atlas],
                            &before.tiles[0][&probe][atlas],
                            &after.tiles[at][&probe][atlas],
                            &after.tiles[0][&probe][atlas],
                        ];
                        let [Some(a), Some(a0), Some(b), Some(b0)] = tiles else {
                            // A probe one build stores at L0 and the other
                            // coarsens: its base storage moved, and no
                            // per-probe output exists on one side.
                            if tiles.iter().any(|tile| tile.is_some()) {
                                l0_in_one_build.insert(probe);
                            }
                            continue;
                        };
                        let same_base = a0 == b0;
                        base_moved |= !same_base;
                        for half in 0..TILE_HALVES {
                            let [a, a0, b, b0] =
                                [a[half], a0[half], b[half], b0[half]].map(f16_to_f32);
                            moved[atlas].0 += usize::from(a.to_bits() != a0.to_bits());
                            moved[atlas].1 += usize::from(b.to_bits() != b0.to_bits());
                            let (x, y) = (output(atlas, a, a0, c_old), output(atlas, b, b0, c_new));
                            let scale = a.abs().max(b.abs()).max(a0.abs()).max(b0.abs());
                            let gap = &mut gaps[atlas];
                            if same_base {
                                gap.same_base.add(x, y, scale);
                            } else {
                                gap.moved_base.add(x, y, scale);
                            }
                        }
                    }
                    moved_base_probes += usize::from(base_moved);
                }
                eprintln!(
                    "[AC1] {name} {mode:?} t′ {t}: moved halves (pre-change, now) {moved:?}; \
                     {moved_base_probes} probe(s) on a moved base; {} stored at L0 by one build \
                     alone",
                    l0_in_one_build.len()
                );
                if !l0_in_one_build.is_empty() {
                    misses.push(format!(
                        "{name} {mode:?} t′ {t}: {} probe(s) stored at L0 by one build alone, \
                         e.g. {:?}",
                        l0_in_one_build.len(),
                        l0_in_one_build.iter().take(8).collect::<Vec<_>>()
                    ));
                }
                for (atlas, gap) in gaps.iter().enumerate() {
                    eprintln!(
                        "[AC1] {name} {mode:?} t′ {t} {}: {gap:?}",
                        ATLAS_NAMES[atlas]
                    );
                    if gap.same_base.differing > 0 {
                        misses.push(format!(
                            "{name} {mode:?} t′ {t} {}: {:?} on an unchanged base",
                            ATLAS_NAMES[atlas], gap.same_base
                        ));
                    }
                    if gap.moved_base.worst_steps > 1.0 {
                        misses.push(format!(
                            "{name} {mode:?} t′ {t} {}: {:?} on a moved base, past one f16 step",
                            ATLAS_NAMES[atlas], gap.moved_base
                        ));
                    }
                }
                moved_any |= moved.iter().any(|&(old, new)| old > 0 || new > 0);
                if mode != ShStreamingMode::Off {
                    continue;
                }
                let differing: Vec<u32> = probes
                    .iter()
                    .copied()
                    .filter(|probe| before.depth[at][probe] != after.depth[at][probe])
                    .collect();
                let unbased: Vec<u32> = new
                    .mid_only_depth
                    .iter()
                    .copied()
                    .filter(|probe| after.depth[at][probe] != after.depth[0][probe])
                    .collect();
                eprintln!(
                    "[AC1] {name} t′ {t}: sampled moments differ at {} of {} probes; {} of {} \
                     mid-pose-only depth probes read other than their base",
                    differing.len(),
                    probes.len(),
                    unbased.len(),
                    new.mid_only_depth.len(),
                );
                if !differing.is_empty() {
                    misses.push(format!(
                        "{name} t′ {t}: sampled moments differ at probes {:?}",
                        &differing[..differing.len().min(8)]
                    ));
                }
                if !unbased.is_empty() {
                    misses.push(format!(
                        "{name} t′ {t}: mid-pose-only depth probes {:?} read other than their base",
                        &unbased[..unbased.len().min(8)]
                    ));
                }
            }
        }
        if !moved_any {
            misses.push(format!(
                "{name}: the channel moves nothing at either terminus"
            ));
        }
    }
    assert!(misses.is_empty(), "AC 1 misses:\n{}", misses.join("\n"));
}
