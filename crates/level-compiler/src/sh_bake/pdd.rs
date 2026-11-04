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
use super::trace::{gather_hit, sphere_directions};
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
