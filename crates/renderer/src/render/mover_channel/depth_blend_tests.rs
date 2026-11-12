// The live-tile blend over one probe: a stated mover's texels come from the
// format's depth rule, and the per-frame cost of the mean-and-variance mix
// against the raw-moment lerp over a room-sized probe set.

use super::*;
use crate::render::mover_channel::depth_test_support::ramp_tile;

/// A tile as the plan holds it: decoded f32 moment pairs.
fn decoded(base: f32) -> Vec<[f32; 2]> {
    ramp_tile(base)
        .iter()
        .map(|&[mean, mean_sq]| [f16_to_f32(mean), f16_to_f32(mean_sq)])
        .collect()
}

fn texels(bytes: &[u8]) -> Vec<[u16; 2]> {
    bytes
        .as_chunks::<TEXEL_BYTES>()
        .0
        .iter()
        .map(|texel| {
            let word = u32::from_le_bytes(*texel);
            [word as u16, (word >> 16) as u16]
        })
        .collect()
}

// One stated mover writes the format's texel mix, packed to f16, for either
// mix; the same tile at both ends writes that tile.
#[test]
fn one_movers_live_tile_is_the_formats_texel_mix() {
    let (from, to, base) = (decoded(1.0), decoded(3.5), decoded(8.0));
    for mix in [DepthMix::Raw, DepthMix::MeanVariance] {
        for weight in [0.0, 0.3, 0.7, 1.0] {
            let blend = DepthBlend {
                from: &from,
                to: &to,
                weight,
                mix,
            };
            let mut out = Vec::new();
            blend_probe(&base, std::iter::once(blend), &mut out);
            let expected: Vec<[u16; 2]> = from
                .iter()
                .zip(&to)
                .map(|(&a, &b)| mix_depth_moments(a, b, weight, mix).map(f32_to_f16))
                .collect();
            assert_eq!(texels(&out), expected, "{mix:?} at {weight}");
        }
        let same = DepthBlend {
            from: &from,
            to: &from,
            weight: 0.4,
            mix,
        };
        let mut out = Vec::new();
        blend_probe(&base, std::iter::once(same), &mut out);
        let stored: Vec<[u16; 2]> = from.iter().map(|m| m.map(f32_to_f16)).collect();
        assert_eq!(texels(&out), stored, "{mix:?}: one tile at both ends");
    }
}

/// Probes the setpiece room's travelling mover rewrites per frame (task 13's
/// `--release` bake): a room-sized live-tile set.
const ROOM_PROBES: usize = 843;

/// The epoch-5 single-mover blend loop, kept as the timing baseline: each
/// moment lerped, `a + s·(b − a)`, then packed.
fn epoch_5_blend(from: &[[f32; 2]], to: &[[f32; 2]], s: f32, out: &mut Vec<u8>) {
    let at = out.len();
    out.resize(at + TILE_BYTES, 0);
    let (tile, _) = out[at..].as_chunks_mut::<TEXEL_BYTES>();
    for ((texel, a), b) in tile.iter_mut().zip(from).zip(to) {
        *texel = texel_bytes([a[0] + s * (b[0] - a[0]), a[1] + s * (b[1] - a[1])]);
    }
}

/// Mean time per frame to blend `ROOM_PROBES` live tiles, each from one or
/// two stated movers mid-interval, under `mix`, or (`None`, one mover) the
/// epoch-5 loop.
fn frame_ms(movers: usize, mix: Option<DepthMix>, frames: u32) -> f64 {
    let tiles: Vec<Vec<[f32; 2]>> = (0..ROOM_PROBES * 3)
        .map(|k| decoded(0.5 + (k % 97) as f32 * 0.37))
        .collect();
    let mut out = Vec::with_capacity(ROOM_PROBES * TILE_BYTES);
    let started = std::time::Instant::now();
    for frame in 0..frames {
        out.clear();
        let weight = (frame as f32 + 0.5) / frames as f32;
        for probe in 0..ROOM_PROBES {
            let [base, a, b] = [0, 1, 2].map(|k| tiles[probe * 3 + k].as_slice());
            let Some(mix) = mix else {
                epoch_5_blend(a, b, weight, &mut out);
                continue;
            };
            let blends = (0..movers).map(|mover| DepthBlend {
                from: if mover == 0 { a } else { b },
                to: if mover == 0 { b } else { a },
                weight,
                mix,
            });
            blend_probe(base, blends, &mut out);
        }
        std::hint::black_box(&out);
    }
    started.elapsed().as_secs_f64() * 1e3 / f64::from(frames)
}

/// Per-frame CPU cost of the live-tile blend over a room-sized probe set
/// (843 probes, 84,300 texels with borders): the epoch-5 loop, the raw-moment
/// lerp (the two-terminus rule) and mean and variance. Timing only: run
/// under `--release` and read the printout.
#[test]
#[ignore = "timing printout: cargo test --release -p postretro-renderer --lib \
            live_tile_blend_cost -- --ignored --nocapture"]
fn live_tile_blend_cost_raw_against_mean_and_variance() {
    const FRAMES: u32 = 400;
    let best = |runs: &[f64]| runs.iter().copied().fold(f64::INFINITY, f64::min);
    for movers in [1, 2] {
        let mut rules = vec![
            ("raw", Some(DepthMix::Raw)),
            ("mean and variance", Some(DepthMix::MeanVariance)),
        ];
        if movers == 1 {
            rules.insert(0, ("epoch-5 loop", None));
        }
        // Warm every path, then alternate to share any drift.
        for &(_, mix) in &rules {
            frame_ms(movers, mix, 20);
        }
        let mut runs = vec![Vec::new(); rules.len()];
        for _ in 0..5 {
            for (rule, &(_, mix)) in runs.iter_mut().zip(&rules) {
                rule.push(frame_ms(movers, mix, FRAMES));
            }
        }
        for ((name, _), runs) in rules.iter().zip(&runs) {
            eprintln!(
                "[MoverDepthBlendCost] {ROOM_PROBES} probes, {movers} mover(s), {name}: {:.4} ms \
                 per frame (best of 5 × {FRAMES} frames; {runs:.4?})",
                best(runs)
            );
        }
    }
}
