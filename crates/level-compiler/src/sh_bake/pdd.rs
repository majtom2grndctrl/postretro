// SPIKE HACK (per-direction-depth): depth-only tile trace, never merged.
// See: context/research/per-direction-depth-spike/README.md
//
// `PDD_OUT=<prefix>` writes `<prefix>.t{4,6,8}.bin`: per-probe octahedral tiles
// of (E[d], E[d^2]) as f16 pairs, in the volume's linear probe order. `PDD_RAYS`
// sets the ray count; radiance is untouched, so a 1024-ray run isolates depth noise.

use std::io::Write;

use postretro_level_format::lightmap::f32_to_f16_bits;
use postretro_level_format::octahedral;
use rayon::prelude::*;

use super::grid::{ProbeGridLayout, vec3_from};
use super::trace::{gather_hit, segment_clear, sphere_directions};
use super::{SAMPLING_LATTICE_OFFSET, ShBakeCtx};

pub(crate) const TILE_SIZES: [u32; 3] = [4, 6, 8];

/// Texel holding `dir`, using the sampler's octahedral convention.
pub(crate) fn texel_of(dir: glam::Vec3, t: u32) -> u32 {
    let [u, v] = octahedral::encode(dir.x, dir.y, dir.z);
    let cell = |c: u16| ((c as f32 / 65535.0 * t as f32) as u32).min(t - 1);
    cell(v) * t + cell(u)
}

/// One tile size's ray-to-texel plan. A texel no ray lands in borrows the
/// nearest ray, so the tile has no holes.
pub(crate) struct TilePlan {
    pub(crate) t: u32,
    pub(crate) members: Vec<Vec<u32>>,
    pub(crate) borrowed: u32,
}

impl TilePlan {
    pub(crate) fn new(t: u32, dirs: &[glam::Vec3]) -> Self {
        let mut members = vec![Vec::new(); (t * t) as usize];
        for (i, d) in dirs.iter().enumerate() {
            members[texel_of(*d, t) as usize].push(i as u32);
        }
        let mut borrowed = 0;
        for (texel, m) in members.iter_mut().enumerate() {
            if m.is_empty() {
                let (tx, ty) = (texel as u32 % t, texel as u32 / t);
                let q = |c: u32| ((c as f32 + 0.5) / t as f32 * 65535.0) as u16;
                let c = glam::Vec3::from(octahedral::decode([q(tx), q(ty)]));
                let nearest = (0..dirs.len())
                    .max_by(|a, b| c.dot(dirs[*a]).total_cmp(&c.dot(dirs[*b])))
                    .unwrap();
                m.push(nearest as u32);
                borrowed += 1;
            }
        }
        Self {
            t,
            members,
            borrowed,
        }
    }

    /// `(E[d], E[d^2])` per texel from one probe's ray distances.
    pub(crate) fn moments(&self, distances: &[f32]) -> Vec<[f32; 2]> {
        self.members
            .iter()
            .map(|m| {
                let n = m.len() as f32;
                let (mut s, mut s2) = (0.0f32, 0.0f32);
                for &i in m {
                    let d = distances[i as usize];
                    s += d;
                    s2 += d * d;
                }
                [s / n, s2 / n]
            })
            .collect()
    }
}

/// One probe's ray distances; a miss reads `far_sentinel`, as the gather does.
pub(crate) fn trace_distances(
    ctx: &ShBakeCtx<'_>,
    pos: glam::Vec3,
    dirs: &[glam::Vec3],
    far_sentinel: f32,
) -> Vec<f32> {
    let ray = ctx.ray_ctx();
    dirs.iter()
        .map(|d| gather_hit(&ray, pos, *d).map_or(far_sentinel, |h| h.distance))
        .collect()
}

pub(crate) fn directions() -> Vec<glam::Vec3> {
    let rays = std::env::var("PDD_RAYS").map_or(256, |v| v.parse().unwrap());
    sphere_directions(rays, SAMPLING_LATTICE_OFFSET)
}

pub(crate) fn run(ctx: &ShBakeCtx<'_>, layout: &ProbeGridLayout) {
    let Ok(prefix) = std::env::var("PDD_OUT") else {
        return;
    };
    let dirs = directions();
    let plans: Vec<TilePlan> = TILE_SIZES
        .iter()
        .map(|t| TilePlan::new(*t, &dirs))
        .collect();
    let total = layout.total_probes();
    let per_probe: Vec<Vec<Vec<[f32; 2]>>> = (0..total)
        .into_par_iter()
        .map(|i| {
            if layout.validity[i] == 0 {
                return Vec::new();
            }
            let pos = vec3_from(layout.probe_positions[i]);
            let d = trace_distances(ctx, pos, &dirs, layout.far_sentinel);
            plans.iter().map(|p| p.moments(&d)).collect()
        })
        .collect();
    for (pi, plan) in plans.iter().enumerate() {
        let path = format!("{prefix}.t{}.bin", plan.t);
        let mut f = std::io::BufWriter::new(std::fs::File::create(&path).unwrap());
        let [gx, gy, gz] = layout.dims;
        for w in [0x3144_4450u32, plan.t, gx, gy, gz, dirs.len() as u32] {
            f.write_all(&w.to_le_bytes()).unwrap();
        }
        let texels = (plan.t * plan.t) as usize;
        for probe in &per_probe {
            for x in 0..texels {
                let [m, m2] = probe.get(pi).map_or([0.0, 0.0], |tile| tile[x]);
                f.write_all(&f32_to_f16_bits(m).to_le_bytes()).unwrap();
                f.write_all(&f32_to_f16_bits(m2).to_le_bytes()).unwrap();
            }
        }
        let min = plan.members.iter().map(Vec::len).min().unwrap();
        eprintln!(
            "[pdd] {path}: {total} probes, {} rays, {} borrowed texels, min {min} rays per texel",
            dirs.len(),
            plan.borrowed
        );
    }
}

// Q3: which probes does a tile change? Compare the sampler's visibility, isotropic
// against each tile size, toward the probe's 26 lattice neighbours at their distance.
// Mirrors `sh_corner_depth_visibility` in f32, with shipping floor and bias.

const MIN_VARIANCE: f32 = 1.0e-4;
const MIN_VISIBILITY: f32 = 0.03;
const BIAS_FRACTION: f32 = 0.05;
const THRESHOLDS: [f32; 4] = [0.05, 0.1, 0.2, 0.5];

fn visibility(m: [f32; 2], distance: f32, bias: f32) -> f32 {
    let variance = (m[1] - m[0] * m[0]).max(MIN_VARIANCE);
    let delta = (distance - m[0] - bias).max(0.0);
    let v = if delta > 0.0 {
        variance / (variance + delta * delta)
    } else {
        1.0
    };
    v.clamp(MIN_VISIBILITY, 1.0)
}

/// The renderer's border fold, so a rim tap reads its neighbour on the sphere.
fn fold(x: i32, y: i32, t: i32) -> usize {
    let (cx, cy) = (x.clamp(0, t - 1), y.clamp(0, t - 1));
    let (sx, sy) = match (x != cx, y != cy) {
        (false, false) => (cx, cy),
        (true, false) => (cx, t - 1 - cy),
        (false, true) => (t - 1 - cx, cy),
        (true, true) => (t - 1 - cx, t - 1 - cy),
    };
    (sy * t + sx) as usize
}

fn bilinear(tile: &[[f32; 2]], t: u32, dir: glam::Vec3) -> [f32; 2] {
    let [u, v] = octahedral::encode(dir.x, dir.y, dir.z);
    let fx = u as f32 / 65535.0 * t as f32 - 0.5;
    let fy = v as f32 / 65535.0 * t as f32 - 0.5;
    let (x0, y0) = (fx.floor(), fy.floor());
    let (ax, ay) = (fx - x0, fy - y0);
    let (x, y, ti) = (x0 as i32, y0 as i32, t as i32);
    let mut out = [0.0f32; 2];
    for (dx, dy, w) in [
        (0, 0, (1.0 - ax) * (1.0 - ay)),
        (1, 0, ax * (1.0 - ay)),
        (0, 1, (1.0 - ax) * ay),
        (1, 1, ax * ay),
    ] {
        let m = tile[fold(x + dx, y + dy, ti)];
        out[0] += w * m[0];
        out[1] += w * m[1];
    }
    out
}

/// Keeps a stable fraction of probes, independent of thread order.
fn sampled(i: usize, fraction: f64) -> bool {
    let mut h = (i as u64).wrapping_mul(0x9E37_79B9_7F4A_7C15);
    h ^= h >> 29;
    h = h.wrapping_mul(0xBF58_476D_1CE4_E5B9);
    h ^= h >> 32;
    ((h >> 11) as f64 / (1u64 << 53) as f64) < fraction
}

/// Per tile size: largest gain and loss of visibility over valid neighbours,
/// then the largest absolute change over all in-grid neighbours.
#[derive(Clone, Copy, Default)]
struct Change {
    darker: f32,
    brighter: f32,
    any: f32,
    // Valid-neighbour directions at the 0.1 threshold, against a traced segment.
    blocked_flagged: u32,
    blocked_missed: u32,
    clear_flagged: u32,
    clear_passed: u32,
}

pub(crate) fn q3(ctx: &ShBakeCtx<'_>, config: &super::ShConfig) {
    let Ok(out) = std::env::var("PDD_Q3") else {
        return;
    };
    let fraction: f64 = std::env::var("PDD_Q3_FRACTION").map_or(1.0, |v| v.parse().unwrap());
    let start = std::time::Instant::now();
    let layout = super::grid::probe_grid_layout(ctx, config);
    run(ctx, &layout);
    let dirs = directions();
    let plans: Vec<TilePlan> = TILE_SIZES
        .iter()
        .map(|t| TilePlan::new(*t, &dirs))
        .collect();
    let [gx, gy, gz] = layout.dims.map(|d| d as i32);
    let cell = glam::Vec3::from(layout.cell_size);
    let bias = cell.min_element().max(0.0) * BIAS_FRACTION;
    let total = layout.total_probes();
    let valid = layout.validity.iter().filter(|v| **v != 0).count();
    eprintln!(
        "[pdd q3] grid {:?}, cell {:?}, {total} probes, {valid} valid, {} rays, fraction {fraction}",
        layout.dims,
        layout.cell_size,
        dirs.len()
    );

    let rows: Vec<(usize, [Change; 3])> = (0..total)
        .into_par_iter()
        .filter(|i| layout.validity[*i] != 0 && sampled(*i, fraction))
        .map(|i| {
            let pos = vec3_from(layout.probe_positions[i]);
            let d = trace_distances(ctx, pos, &dirs, layout.far_sentinel);
            let n = d.len() as f32;
            let iso = [
                d.iter().sum::<f32>() / n,
                d.iter().map(|x| x * x).sum::<f32>() / n,
            ];
            let tiles: Vec<Vec<[f32; 2]>> = plans.iter().map(|p| p.moments(&d)).collect();
            let (px, py, pz) = (
                i as i32 % gx,
                (i as i32 / gx) % gy,
                i as i32 / (gx * gy),
            );
            let mut change = [Change::default(); 3];
            let ray = ctx.ray_ctx();
            for oz in -1..=1i32 {
                for oy in -1..=1i32 {
                    for ox in -1..=1i32 {
                        let (nx, ny, nz) = (px + ox, py + oy, pz + oz);
                        if (ox, oy, oz) == (0, 0, 0)
                            || nx < 0
                            || ny < 0
                            || nz < 0
                            || nx >= gx
                            || ny >= gy
                            || nz >= gz
                        {
                            continue;
                        }
                        let neighbour_valid =
                            layout.validity[(nx + gx * (ny + gy * nz)) as usize] != 0;
                        let offset = glam::Vec3::new(ox as f32, oy as f32, oz as f32) * cell;
                        let distance = offset.length();
                        let dir = offset / distance;
                        let before = visibility(iso, distance, bias);
                        let index = (nx + gx * (ny + gy * nz)) as usize;
                        let clear = !neighbour_valid
                            || segment_clear(&ray, pos, vec3_from(layout.probe_positions[index]));
                        for (ti, plan) in plans.iter().enumerate() {
                            let after =
                                visibility(bilinear(&tiles[ti], plan.t, dir), distance, bias);
                            let c = &mut change[ti];
                            c.any = c.any.max((after - before).abs());
                            if neighbour_valid {
                                c.darker = c.darker.max(before - after);
                                c.brighter = c.brighter.max(after - before);
                                let flagged = before - after > 0.1;
                                match (clear, flagged) {
                                    (false, true) => c.blocked_flagged += 1,
                                    (false, false) => c.blocked_missed += 1,
                                    (true, true) => c.clear_flagged += 1,
                                    (true, false) => c.clear_passed += 1,
                                }
                            }
                        }
                    }
                }
            }
            (i, change)
        })
        .collect();

    let n = rows.len();
    eprintln!(
        "[pdd q3] traced {n} probes in {:.1} s",
        start.elapsed().as_secs_f32()
    );
    for (ti, plan) in plans.iter().enumerate() {
        for threshold in THRESHOLDS {
            let count = |f: &dyn Fn(&Change) -> bool| rows.iter().filter(|r| f(&r.1[ti])).count();
            let valid_any = count(&|c| c.darker > threshold || c.brighter > threshold);
            let darker = count(&|c| c.darker > threshold);
            let brighter = count(&|c| c.brighter > threshold);
            let any = count(&|c| c.any > threshold);
            eprintln!(
                "[pdd q3] T{} >{threshold}: valid-neighbour {valid_any} ({:.2}%), darker {darker} ({:.2}%), brighter {brighter} ({:.2}%), all-neighbour {any} ({:.2}%)",
                plan.t,
                100.0 * valid_any as f64 / n as f64,
                100.0 * darker as f64 / n as f64,
                100.0 * brighter as f64 / n as f64,
                100.0 * any as f64 / n as f64,
            );
        }
    }
    for (ti, plan) in plans.iter().enumerate() {
        let sum = |f: &dyn Fn(&Change) -> u32| rows.iter().map(|r| f(&r.1[ti]) as u64).sum::<u64>();
        let (bf, bm) = (sum(&|c| c.blocked_flagged), sum(&|c| c.blocked_missed));
        let (cf, cp) = (sum(&|c| c.clear_flagged), sum(&|c| c.clear_passed));
        let probes_blocked = rows
            .iter()
            .filter(|r| r.1[ti].blocked_flagged + r.1[ti].blocked_missed > 0)
            .count();
        let probes_true = rows.iter().filter(|r| r.1[ti].blocked_flagged > 0).count();
        let probes_false = rows.iter().filter(|r| r.1[ti].clear_flagged > 0).count();
        eprintln!(
            "[pdd q3] T{} truth: directions blocked {} (flagged {bf}, missed {bm}), clear {} (flagged {cf}, passed {cp}); probes with a blocked valid neighbour {probes_blocked}, with a correct flag {probes_true}, with a false flag {probes_false}",
            plan.t,
            bf + bm,
            cf + cp,
        );
    }
    // Byte per linear probe, bit per tile size, at the 0.1 threshold over valid
    // neighbours; bit 7 marks a traced probe. Joined with SH slots by the engine.
    let mut bitmap = vec![0u8; total];
    for (i, change) in &rows {
        bitmap[*i] = 0x80;
        for (ti, c) in change.iter().enumerate() {
            if c.darker > 0.1 || c.brighter > 0.1 {
                bitmap[*i] |= 1 << ti;
            }
        }
    }
    std::fs::write(&out, &bitmap).unwrap();
    std::process::exit(0);
}
