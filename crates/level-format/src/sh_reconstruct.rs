//! Intra-brick trilinear reconstruction math for SH probe tiles.
//!
//! Relocated verbatim from the level-compiler's measurement-only `sh_analyze`
//! (behavior-preserving) so the compiler classifier, the CPU compose reference
//! (`postretro-render-cpu`), and — via the WGSL port — the GPU compose passes
//! share **one** definition of how a dropped-valid probe is reconstructed from a
//! coarser kept lattice. See
//! `context/plans/drafts/lighting-scale--delta-sh-probe-coarsening/` (task G1).
//!
//! ## Three candidate stored levels per 4×4×4 brick
//! - **L0** — all 64 base probes (dense; ground truth).
//! - **L1** — the 8 corner probes (in-brick local ∈ {0,3}³), trilinear
//!   reconstruction with per-axis weights `local/3`.
//! - **L2** — a single brick-mean tile over the brick's VALID probes only.
//!
//! Reconstruction is strictly **intra-brick**: an L1 target reads only the
//! brick's own 8 corners, never a neighbor cell.

use glam::Vec3;

use crate::delta_sh_volumes::{AFFINITY_FACTOR, PROBES_PER_CELL};

/// Affinity factor as `usize` — a 4×4×4 brick edge (= one affinity cell).
const AF: usize = AFFINITY_FACTOR as usize; // 4

/// An octahedral interior tile: `interior*interior` RGB texels.
pub type Tile = Vec<Vec3>;

/// A zero-filled tile of `texels` RGB texels.
pub fn zero_tile(texels: usize) -> Tile {
    vec![Vec3::ZERO; texels]
}

/// The 8 corner local indices of a 4×4×4 brick: local ∈ {0,3}³, x-fastest.
pub fn corner_locals() -> [usize; 8] {
    let mut out = [0usize; 8];
    let mut k = 0;
    for &cz in &[0usize, AF - 1] {
        for &cy in &[0usize, AF - 1] {
            for &cx in &[0usize, AF - 1] {
                out[k] = cx + cy * AF + cz * AF * AF;
                k += 1;
            }
        }
    }
    out
}

/// Decompose a local probe index (0..63) into `(x, y, z)` within the brick.
pub fn local_xyz(local: usize) -> (usize, usize, usize) {
    (local % AF, (local / AF) % AF, local / (AF * AF))
}

/// Trilinear weight of corner `(cx,cy,cz)∈{0,3}³` for a target at local
/// `(tx,ty,tz)`, per-axis weight = position along the 0..3 span.
pub fn trilinear_weight(target: (usize, usize, usize), corner: (usize, usize, usize)) -> f32 {
    let axis = |t: usize, c: usize| -> f32 {
        let f = t as f32 / (AF - 1) as f32; // 0..1 along the brick span
        if c == AF - 1 {
            f
        } else {
            1.0 - f
        }
    };
    axis(target.0, corner.0) * axis(target.1, corner.1) * axis(target.2, corner.2)
}

/// L1 reconstruction of the tile at `target_local` from the brick's valid corner
/// tiles. Corners that are absent/invalid are dropped and the surviving weights
/// renormalized. Returns `None` when no valid corner exists.
pub fn reconstruct_l1_tile(
    tiles: &[Option<Tile>; PROBES_PER_CELL],
    target_local: usize,
    texels: usize,
) -> Option<Tile> {
    let target = local_xyz(target_local);
    let mut acc = zero_tile(texels);
    let mut wsum = 0.0f32;
    for corner_local in corner_locals() {
        if let Some(tile) = &tiles[corner_local] {
            let w = trilinear_weight(target, local_xyz(corner_local));
            if w <= 0.0 {
                continue;
            }
            for (a, t) in acc.iter_mut().zip(tile.iter()) {
                *a += *t * w;
            }
            wsum += w;
        }
    }
    if wsum <= 0.0 {
        return None;
    }
    for a in acc.iter_mut() {
        *a /= wsum;
    }
    Some(acc)
}

/// L2 brick-mean tile over valid probes. `None` when the brick has no valid
/// probe.
pub fn reconstruct_l2_tile(tiles: &[Option<Tile>; PROBES_PER_CELL], texels: usize) -> Option<Tile> {
    let mut acc = zero_tile(texels);
    let mut n = 0u32;
    for tile in tiles.iter().flatten() {
        for (a, t) in acc.iter_mut().zip(tile.iter()) {
            *a += *t;
        }
        n += 1;
    }
    if n == 0 {
        return None;
    }
    for a in acc.iter_mut() {
        *a /= n as f32;
    }
    Some(acc)
}

/// Per-brick, per-section probe-density level.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Level {
    L0,
    L1,
    L2,
}

/// Stored tile count for a brick at a given level, intersected with validity.
pub fn stored_tiles(level: Level, valid_mask: &[bool; PROBES_PER_CELL]) -> usize {
    match level {
        Level::L0 => valid_mask.iter().filter(|&&v| v).count(),
        Level::L1 => corner_locals().iter().filter(|&&l| valid_mask[l]).count(),
        Level::L2 => {
            if valid_mask.iter().any(|&v| v) {
                1
            } else {
                0
            }
        }
    }
}

/// Stored tile count for a delta entry at a candidate level. Unlike the base
/// model, this reads the delta section's self-describing compact probe set.
pub fn stored_delta_tiles(level: Level, valid_probe_mask: u64) -> usize {
    match level {
        Level::L0 => valid_probe_mask.count_ones() as usize,
        Level::L1 => corner_locals()
            .iter()
            .filter(|&&local| valid_probe_mask & (1u64 << local) != 0)
            .count(),
        Level::L2 => usize::from(valid_probe_mask != 0),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn const_tiles(value: f32, texels: usize) -> [Option<Tile>; PROBES_PER_CELL] {
        std::array::from_fn(|_| Some(vec![Vec3::splat(value); texels]))
    }

    #[test]
    fn corner_locals_are_the_eight_brick_corners() {
        // {0,3}^3, x-fastest: 0, 3, 12, 15, 48, 51, 60, 63.
        assert_eq!(corner_locals(), [0, 3, 12, 15, 48, 51, 60, 63]);
    }

    #[test]
    fn local_xyz_round_trips_x_fastest() {
        for lz in 0..AF {
            for ly in 0..AF {
                for lx in 0..AF {
                    let local = lx + ly * AF + lz * AF * AF;
                    assert_eq!(local_xyz(local), (lx, ly, lz));
                }
            }
        }
    }

    #[test]
    fn trilinear_weights_partition_unity_and_hit_endpoints() {
        // Corner (0,0,0) at target (0,0,0) → weight 1.
        assert!((trilinear_weight((0, 0, 0), (0, 0, 0)) - 1.0).abs() < 1e-6);
        // Corner (3,3,3) at target (3,3,3) → weight 1.
        assert!((trilinear_weight((3, 3, 3), (3, 3, 3)) - 1.0).abs() < 1e-6);
        // Corner (3,0,0) at target (0,0,0) → weight 0.
        assert!(trilinear_weight((0, 0, 0), (3, 0, 0)).abs() < 1e-6);
        // Weights across the 8 corners partition unity at any interior target.
        for target_local in 0..PROBES_PER_CELL {
            let target = local_xyz(target_local);
            let mut sum = 0.0;
            for c in corner_locals() {
                sum += trilinear_weight(target, local_xyz(c));
            }
            assert!(
                (sum - 1.0).abs() < 1e-5,
                "trilinear weights must partition unity at local {target_local}"
            );
        }
    }

    #[test]
    fn l1_trilinear_reproduces_linear_ramp_exactly() {
        // A brick whose per-probe value is linear in local x: trilinear from the
        // 8 corners must reproduce it exactly at every interior probe.
        let texels = 4;
        let mut tiles: [Option<Tile>; PROBES_PER_CELL] = std::array::from_fn(|_| None);
        for local in 0..PROBES_PER_CELL {
            let (lx, _ly, _lz) = local_xyz(local);
            let val = 10.0 + lx as f32 * 2.0;
            tiles[local] = Some(vec![Vec3::splat(val); texels]);
        }
        for target in 0..PROBES_PER_CELL {
            let recon = reconstruct_l1_tile(&tiles, target, texels).unwrap();
            let (lx, _, _) = local_xyz(target);
            let expect = 10.0 + lx as f32 * 2.0;
            assert!(
                (recon[0].x - expect).abs() < 1e-4,
                "L1 trilinear must reproduce a linear ramp: got {} expect {expect}",
                recon[0].x
            );
        }
    }

    #[test]
    fn l1_renormalizes_over_surviving_corners() {
        // Only corner (0,0,0) valid. A target with a positive weight to it
        // (here local (1,1,1), raw weight (2/3)^3) renormalizes to reproduce the
        // survivor's value exactly. A target whose only surviving corner has
        // weight 0 (the opposite corner (3,3,3)) has no basis → `None`.
        let texels = 2;
        let mut tiles: [Option<Tile>; PROBES_PER_CELL] = std::array::from_fn(|_| None);
        tiles[0] = Some(vec![Vec3::splat(7.0); texels]);

        let interior = 1 + 1 * AF + 1 * AF * AF; // local (1,1,1)
        let recon = reconstruct_l1_tile(&tiles, interior, texels).unwrap();
        for v in &recon {
            assert!((v.x - 7.0).abs() < 1e-6);
        }

        let opposite = (AF - 1) + (AF - 1) * AF + (AF - 1) * AF * AF; // local (3,3,3)
        assert!(reconstruct_l1_tile(&tiles, opposite, texels).is_none());
    }

    #[test]
    fn l1_is_none_without_a_valid_corner() {
        let texels = 2;
        let mut tiles: [Option<Tile>; PROBES_PER_CELL] = std::array::from_fn(|_| None);
        // A valid non-corner probe (local 1) must not rescue L1.
        tiles[1] = Some(vec![Vec3::splat(1.0); texels]);
        assert!(reconstruct_l1_tile(&tiles, 0, texels).is_none());
    }

    #[test]
    fn l2_mean_of_constant_brick_is_exact() {
        let texels = 16;
        let tiles = const_tiles(3.0, texels);
        let recon = reconstruct_l2_tile(&tiles, texels).unwrap();
        for v in &recon {
            assert!((v.x - 3.0).abs() < 1e-6);
        }
    }

    #[test]
    fn l2_is_none_for_empty_brick() {
        let texels = 4;
        let tiles: [Option<Tile>; PROBES_PER_CELL] = std::array::from_fn(|_| None);
        assert!(reconstruct_l2_tile(&tiles, texels).is_none());
    }

    #[test]
    fn stored_tile_counts_match_level_lattice() {
        let mut valid = [true; PROBES_PER_CELL];
        assert_eq!(stored_tiles(Level::L0, &valid), 64);
        assert_eq!(stored_tiles(Level::L1, &valid), 8);
        assert_eq!(stored_tiles(Level::L2, &valid), 1);

        // Drop a corner (local 0) → L1 loses one, L2 still 1, L0 loses one.
        valid[0] = false;
        assert_eq!(stored_tiles(Level::L0, &valid), 63);
        assert_eq!(stored_tiles(Level::L1, &valid), 7);
        assert_eq!(stored_tiles(Level::L2, &valid), 1);

        let empty = [false; PROBES_PER_CELL];
        assert_eq!(stored_tiles(Level::L2, &empty), 0);
    }

    #[test]
    fn stored_delta_tile_counts_track_popcount() {
        let full = u64::MAX;
        assert_eq!(stored_delta_tiles(Level::L0, full), 64);
        assert_eq!(stored_delta_tiles(Level::L1, full), 8);
        assert_eq!(stored_delta_tiles(Level::L2, full), 1);

        // Only non-corner bits set → L1 keeps nothing, L2 keeps its one tile.
        let non_corner = 1u64 << 1;
        assert_eq!(stored_delta_tiles(Level::L0, non_corner), 1);
        assert_eq!(stored_delta_tiles(Level::L1, non_corner), 0);
        assert_eq!(stored_delta_tiles(Level::L2, non_corner), 1);

        assert_eq!(stored_delta_tiles(Level::L2, 0), 0);
    }
}
