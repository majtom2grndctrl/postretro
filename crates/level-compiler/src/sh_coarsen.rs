//! Delta-SH probe coarsening classifier core (slice G3-α).
//!
//! Pure numeric logic: given already-computed per-brick reconstruction-error
//! and magnitude statistics for the affinity grid, decide each 4×4×4 affinity
//! brick's probe-density [`Level`] (L0 dense / L1 corners / L2 brick-mean),
//! force protected bricks back to L0, then run a fixpoint seam-smoothing sweep
//! so no two face-adjacent bricks differ by more than one level.
//!
//! This module owns none of the tile-decoding that produces the input stats —
//! that is a separate later slice. Everything here is std-only apart from the
//! shared [`Level`] type and is unit-testable with hand-built inputs.

use postretro_level_format::sh_reconstruct::Level;

/// Tunable gate parameters — the precommitted operating point.
pub(crate) struct CoarsenParams {
    /// Maximum allowed relative p95 reconstruction error for a level to pass.
    pub rel_p95_max: f32,
    /// Maximum allowed relative max reconstruction error for a level to pass.
    pub rel_max_max: f32,
    /// Fraction of the map-wide p95 magnitude below which a brick is treated as
    /// near-black and bypasses the relative-error comparison.
    pub darkness_frac: f32,
}

impl Default for CoarsenParams {
    fn default() -> Self {
        CoarsenParams {
            rel_p95_max: 0.10,
            rel_max_max: 0.25,
            darkness_frac: 0.02,
        }
    }
}

/// One brick's precomputed classification inputs (all pure numbers).
///
/// `*_evaluable` is false when that level's reconstruction had zero scored
/// texel-samples (e.g. L1 with no valid corner) — an unevaluable level is never
/// chosen and scores +inf.
pub(crate) struct BrickClass {
    /// This brick's p95 reconstruction-target magnitude.
    pub mag_p95: f32,
    /// This brick's max reconstruction-target magnitude.
    pub mag_max: f32,
    /// L1 (corners) p95 reconstruction error.
    pub l1_p95: f32,
    /// L1 (corners) max reconstruction error.
    pub l1_max: f32,
    /// Whether the L1 reconstruction had any scored samples.
    pub l1_evaluable: bool,
    /// L2 (brick-mean) p95 reconstruction error.
    pub l2_p95: f32,
    /// L2 (brick-mean) max reconstruction error.
    pub l2_max: f32,
    /// Whether the L2 reconstruction had any scored samples.
    pub l2_evaluable: bool,
    /// Zero-valid bricks are excluded from coarsening and non-participating in
    /// seam-smoothing adjacency (they neither demote nor are demoted).
    pub has_any_valid: bool,
    /// World-space minimum corner of the brick's AABB.
    pub world_min: [f32; 3],
    /// World-space maximum corner of the brick's AABB.
    pub world_max: [f32; 3],
}

/// Classify every brick to a level. `protect_aabbs` are world AABBs as
/// `[minx,miny,minz,maxx,maxy,maxz]`. Returns per-cell [`Level`]-as-u8,
/// x-fastest, length == `bricks.len()` == product(`affinity_dims`).
pub(crate) fn classify_levels(
    bricks: &[BrickClass],
    affinity_dims: [u32; 3],
    protect_aabbs: &[[f32; 6]],
    params: &CoarsenParams,
) -> Vec<u8> {
    // --- Phase A — map-wide magnitude. ---
    let mut valid_mags: Vec<f32> = bricks
        .iter()
        .filter(|b| b.has_any_valid)
        .map(|b| b.mag_p95)
        .collect();
    let map_p95 = if valid_mags.is_empty() {
        0.0
    } else {
        valid_mags.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
        let n = valid_mags.len();
        // Truncated p95 index, matching the operating-point selector
        // `relerr_opmap.py` (`int(0.95 * (len - 1))`) exactly — `round()` would
        // pick a different order statistic for most small/medium n and shift
        // the darkness floor.
        let idx = ((n - 1) as f32 * 0.95).floor() as usize;
        valid_mags[idx]
    };
    let floor = f32::max(params.darkness_frac * map_p95, 1e-6);

    // --- Phase B — per-brick gate. ---
    let mut levels: Vec<Level> = Vec::with_capacity(bricks.len());
    // Parallel participation flags: zero-valid bricks do not participate in
    // coarsening or seam-smoothing adjacency.
    let mut participating: Vec<bool> = Vec::with_capacity(bricks.len());

    for b in bricks {
        if !b.has_any_valid {
            levels.push(Level::L0);
            participating.push(false);
            continue;
        }
        participating.push(true);

        let level = if b.mag_p95 < floor {
            // Darkness bypass (spec AC5 / pin P8): a sub-floor brick takes the
            // coarsest evaluable level, skipping the relative comparison
            // entirely. This is stronger than the operating-point selector
            // `relerr_opmap.py`, which clamps the denominator (`err/max(mag,
            // floor)`) but still gates. The two agree on physical data: a
            // brick below the floor has near-zero-magnitude probes, so its
            // reconstruction error is bounded near zero and thus imperceptible
            // — dense storage there buys nothing. The bypass is the deliberate
            // spec choice; the clamp only diverges for non-physical dark
            // bricks with large absolute error.
            if b.l2_evaluable {
                Level::L2
            } else if b.l1_evaluable {
                Level::L1
            } else {
                Level::L0
            }
        } else {
            // Bright brick: choose the coarsest level passing BOTH thresholds.
            // An unevaluable level is treated as failing (score +inf).
            let l2_ok = b.l2_evaluable
                && b.l2_p95 / f32::max(b.mag_p95, floor) <= params.rel_p95_max
                && b.l2_max / f32::max(b.mag_max, floor) <= params.rel_max_max;
            let l1_ok = b.l1_evaluable
                && b.l1_p95 / f32::max(b.mag_p95, floor) <= params.rel_p95_max
                && b.l1_max / f32::max(b.mag_max, floor) <= params.rel_max_max;
            if l2_ok {
                Level::L2
            } else if l1_ok {
                Level::L1
            } else {
                Level::L0
            }
        };
        levels.push(level);
    }

    // --- Phase C — protection (after the gate, before smoothing). ---
    // Protected bricks are forced to the finest level and can force coarser
    // neighbors to demote, but can never demote further.
    for (i, b) in bricks.iter().enumerate() {
        if protect_aabbs
            .iter()
            .any(|a| aabb_overlap(&b.world_min, &b.world_max, a))
        {
            levels[i] = Level::L0;
        }
    }

    // --- Phase D — fixpoint seam-smoothing. ---
    // Face-adjacency over the x-fastest affinity grid. Repeat sweeps until a
    // full sweep performs zero demotions. Monotone (levels only decrease) so it
    // terminates.
    let [dx, dy, dz] = affinity_dims;
    let (dxu, dyu, dzu) = (dx as usize, dy as usize, dz as usize);
    loop {
        let mut demoted = false;
        for z in 0..dzu {
            for y in 0..dyu {
                for x in 0..dxu {
                    let i = x + y * dxu + z * dxu * dyu;
                    if !participating[i] {
                        continue;
                    }
                    // Only inspect the positive-direction neighbors so each
                    // face-adjacent pair is visited once per sweep.
                    if x + 1 < dxu {
                        demoted |= smooth_pair(&mut levels, &participating, i, i + 1);
                    }
                    if y + 1 < dyu {
                        demoted |= smooth_pair(&mut levels, &participating, i, i + dxu);
                    }
                    if z + 1 < dzu {
                        demoted |= smooth_pair(&mut levels, &participating, i, i + dxu * dyu);
                    }
                }
            }
        }
        if !demoted {
            break;
        }
    }

    levels.iter().map(|l| l.to_u8()).collect()
}

/// Standard 3-axis AABB overlap: overlap iff for every axis
/// `min[a] <= aabb_max[a] && max[a] >= aabb_min[a]`.
fn aabb_overlap(world_min: &[f32; 3], world_max: &[f32; 3], aabb: &[f32; 6]) -> bool {
    for a in 0..3 {
        if !(world_min[a] <= aabb[3 + a] && world_max[a] >= aabb[a]) {
            return false;
        }
    }
    true
}

/// If two participating face-adjacent bricks differ by ≥ 2 levels (an L2/L0
/// pair), demote the coarser endpoint one level (L2→L1). Returns whether a
/// demotion occurred.
fn smooth_pair(levels: &mut [Level], participating: &[bool], a: usize, b: usize) -> bool {
    if !participating[a] || !participating[b] {
        return false;
    }
    let (la, lb) = (levels[a].to_u8(), levels[b].to_u8());
    if la.abs_diff(lb) < 2 {
        return false;
    }
    // Demote the coarser (numerically higher) endpoint by one level.
    if la > lb {
        levels[a] = demote_one(levels[a]);
    } else {
        levels[b] = demote_one(levels[b]);
    }
    true
}

/// Demote a level by one step (L2→L1, L1→L0). L0 is already finest.
fn demote_one(level: Level) -> Level {
    match level {
        Level::L2 => Level::L1,
        Level::L1 => Level::L0,
        Level::L0 => Level::L0,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const L0: u8 = 0;
    const L1: u8 = 1;
    const L2: u8 = 2;

    /// Build a brick with no protection footprint (unit AABB placed by index so
    /// distinct indices do not overlap by default) and both levels evaluable.
    fn brick(mag_p95: f32, mag_max: f32, l1: (f32, f32), l2: (f32, f32)) -> BrickClass {
        BrickClass {
            mag_p95,
            mag_max,
            l1_p95: l1.0,
            l1_max: l1.1,
            l1_evaluable: true,
            l2_p95: l2.0,
            l2_max: l2.1,
            l2_evaluable: true,
            has_any_valid: true,
            world_min: [0.0, 0.0, 0.0],
            world_max: [0.0, 0.0, 0.0],
        }
    }

    /// Place a brick's AABB at grid index `i` along x so bricks never overlap
    /// each other, one unit apart.
    fn at_x(mut b: BrickClass, i: usize) -> BrickClass {
        let x = i as f32 * 10.0;
        b.world_min = [x, 0.0, 0.0];
        b.world_max = [x + 1.0, 1.0, 1.0];
        b
    }

    // ---- Gate, bright ----

    #[test]
    fn gate_bright_tiny_errors_pick_l2() {
        // Bright magnitude, tiny errors relative to it → coarsest (L2).
        let bricks = vec![brick(10.0, 12.0, (0.1, 0.2), (0.1, 0.2))];
        let out = classify_levels(&bricks, [1, 1, 1], &[], &CoarsenParams::default());
        assert_eq!(out, vec![L2]);
    }

    #[test]
    fn gate_bright_medium_errors_pick_l1() {
        // L2 fails p95 (0.2 rel > 0.10) but L1 passes both → L1.
        let bricks = vec![brick(
            10.0,
            10.0,
            (0.5, 1.0), // rel_p95 0.05, rel_max 0.10 → pass
            (2.0, 2.0), // rel_p95 0.20 > 0.10 → fail
        )];
        let out = classify_levels(&bricks, [1, 1, 1], &[], &CoarsenParams::default());
        assert_eq!(out, vec![L1]);
    }

    #[test]
    fn gate_bright_large_errors_pick_l0() {
        // Both levels fail p95 → L0.
        let bricks = vec![brick(10.0, 10.0, (2.0, 2.0), (3.0, 3.0))];
        let out = classify_levels(&bricks, [1, 1, 1], &[], &CoarsenParams::default());
        assert_eq!(out, vec![L0]);
    }

    #[test]
    fn gate_bright_and_of_both_thresholds() {
        // L2 passes p95 (0.05) but FAILS rel_max (0.30 > 0.25) → must not pick
        // L2. L1 also fails rel_max here → L0. Verifies the AND, not the OR.
        let bricks = vec![brick(
            10.0,
            10.0,
            (0.5, 3.0), // rel_p95 0.05 pass, rel_max 0.30 fail
            (0.5, 3.0), // rel_p95 0.05 pass, rel_max 0.30 fail
        )];
        let out = classify_levels(&bricks, [1, 1, 1], &[], &CoarsenParams::default());
        assert_eq!(out, vec![L0], "passing p95 but failing rel_max must stay finer");
    }

    // ---- P8 dark map ----

    #[test]
    fn p8_dark_map_still_coarsens() {
        // Entire map near-black (mag ≈ 1e-4). map_p95 ≈ 1e-4, floor = max(0.02 *
        // 1e-4, 1e-6) = 2e-6, so bricks are NOT sub-floor and take the bright
        // path — with tiny errors they coarsen rather than being forced dense.
        let bricks: Vec<BrickClass> = (0..4)
            .map(|_| brick(1e-4, 1e-4, (1e-7, 1e-7), (1e-7, 1e-7)))
            .collect();
        let out = classify_levels(&bricks, [4, 1, 1], &[], &CoarsenParams::default());
        assert!(out.iter().any(|&l| l != L0), "dark map must not be forced all-L0");
    }

    #[test]
    fn p8_single_subfloor_brick_bypasses_to_coarsest() {
        // Bright map sets map_p95 ≈ 100 → floor = 2.0. One brick with mag well
        // below floor bypasses the relative comparison to its coarsest evaluable
        // level (L2), regardless of its error magnitudes.
        let mut bricks: Vec<BrickClass> = (0..3)
            .map(|_| brick(100.0, 100.0, (1.0, 1.0), (1.0, 1.0)))
            .collect();
        // Sub-floor brick with huge nominal errors — bypass ignores them.
        bricks.push(brick(0.5, 0.5, (999.0, 999.0), (999.0, 999.0)));
        let out = classify_levels(&bricks, [4, 1, 1], &[], &CoarsenParams::default());
        assert_eq!(out[3], L2, "sub-floor brick must bypass to coarsest evaluable level");
    }

    // ---- P9 no valid corners ----

    #[test]
    fn p9_no_valid_corners_never_l1() {
        // l1 unevaluable, l2 evaluable and passing → L2, never L1.
        let mut b = brick(10.0, 10.0, (0.1, 0.1), (0.1, 0.1));
        b.l1_evaluable = false;
        let out = classify_levels(&[b], [1, 1, 1], &[], &CoarsenParams::default());
        assert_eq!(out, vec![L2]);
    }

    #[test]
    fn p9_no_valid_corners_l2_fails_gives_l0() {
        // l1 unevaluable and l2 fails → L0 (never L1).
        let mut b = brick(10.0, 10.0, (0.1, 0.1), (5.0, 5.0));
        b.l1_evaluable = false;
        let out = classify_levels(&[b], [1, 1, 1], &[], &CoarsenParams::default());
        assert_eq!(out, vec![L0]);
    }

    // ---- P11 all-non-corner valids (same shape as P9) ----

    #[test]
    fn p11_all_non_corner_valids_l2_only() {
        // No valid corners anywhere → L1 unevaluable across the map; passing
        // bricks coarsen only to L2, never L1.
        let bricks: Vec<BrickClass> = (0..3)
            .map(|_| {
                let mut b = brick(10.0, 10.0, (0.1, 0.1), (0.1, 0.1));
                b.l1_evaluable = false;
                b
            })
            .collect();
        let out = classify_levels(&bricks, [3, 1, 1], &[], &CoarsenParams::default());
        assert!(out.iter().all(|&l| l == L2), "must be L2-only coarsening, never L1");
    }

    // ---- P10 zero valid probes ----

    #[test]
    fn p10_zero_valid_is_l0_and_non_participating() {
        // Brick 0: zero-valid (L0, non-participating). Brick 1: would-be L2.
        // The zero-valid brick must NOT demote its L2 neighbor.
        let mut zero = brick(10.0, 10.0, (0.1, 0.1), (0.1, 0.1));
        zero.has_any_valid = false;
        let l2_neighbor = brick(10.0, 10.0, (0.1, 0.1), (0.1, 0.1));
        let bricks = vec![at_x(zero, 0), at_x(l2_neighbor, 1)];
        let out = classify_levels(&bricks, [2, 1, 1], &[], &CoarsenParams::default());
        assert_eq!(out[0], L0, "zero-valid brick is L0");
        assert_eq!(out[1], L2, "L2 neighbor not demoted by a non-participating brick");
    }

    // ---- P5 fixpoint chain ----

    #[test]
    fn p5_fixpoint_chain_resolves_invariant() {
        // An x-row that gates to L2,L2,L2,L0 must fully resolve so no adjacent
        // pair differs by ≥2. Bricks 0..2 gate L2 (tiny errors); brick 3 gates
        // L0 (large errors).
        let bricks = vec![
            at_x(brick(10.0, 10.0, (0.1, 0.1), (0.1, 0.1)), 0),
            at_x(brick(10.0, 10.0, (0.1, 0.1), (0.1, 0.1)), 1),
            at_x(brick(10.0, 10.0, (0.1, 0.1), (0.1, 0.1)), 2),
            at_x(brick(10.0, 10.0, (9.0, 9.0), (9.0, 9.0)), 3),
        ];
        let out = classify_levels(&bricks, [4, 1, 1], &[], &CoarsenParams::default());
        assert_eq!(out[3], L0);
        for w in out.windows(2) {
            assert!(
                (w[0] as i32 - w[1] as i32).abs() <= 1,
                "adjacent levels must differ by at most 1, got {out:?}"
            );
        }
    }

    // ---- P7 demote-coarser ----

    #[test]
    fn p7_demotes_the_coarser_plus_x_neighbor() {
        // Brick 0 gates L0 (large errors), brick 1 gates L2 (tiny errors). The
        // coarser endpoint is the +x neighbor; it — not brick 0 — is demoted.
        let bricks = vec![
            at_x(brick(10.0, 10.0, (9.0, 9.0), (9.0, 9.0)), 0), // L0
            at_x(brick(10.0, 10.0, (0.1, 0.1), (0.1, 0.1)), 1), // L2
        ];
        let out = classify_levels(&bricks, [2, 1, 1], &[], &CoarsenParams::default());
        assert_eq!(out[0], L0, "finer endpoint unchanged");
        assert_eq!(out[1], L1, "coarser +x neighbor demoted L2→L1");
    }

    // ---- P6/P14 protection ----

    #[test]
    fn p6_p14_protection_forces_l0_and_feeds_smoothing() {
        // Brick 0 would gate L2 but is covered by a protection AABB → forced L0.
        // Brick 1 gates L2; protection runs before smoothing, so the seam pass
        // sees L0 next to L2 and demotes brick 1 to within one level.
        let bricks = vec![
            at_x(brick(10.0, 10.0, (0.1, 0.1), (0.1, 0.1)), 0), // would be L2
            at_x(brick(10.0, 10.0, (0.1, 0.1), (0.1, 0.1)), 1), // L2
        ];
        // Protect covers brick 0's AABB (x in [0,1]).
        let protect = vec![[-1.0, -1.0, -1.0, 2.0, 2.0, 2.0]];
        let out = classify_levels(&bricks, [2, 1, 1], &protect, &CoarsenParams::default());
        assert_eq!(out[0], L0, "protected brick forced to L0");
        assert!(
            (out[0] as i32 - out[1] as i32).abs() <= 1,
            "protection feeds smoothing: neighbor demoted to ≤1 level diff, got {out:?}"
        );
        assert_eq!(out[1], L1, "would-be-L2 neighbor demoted to L1");
    }

    // ---- Adjacency / index sanity ----

    #[test]
    fn adjacency_x_fastest_2x2x2() {
        // 2×2×2 grid, x-fastest indexing. Corner cell 0 = L0 (large errors);
        // all others gate L2. Face neighbors of cell 0 are indices 1 (+x),
        // 2 (+y), 4 (+z) — each must demote to L1; the diagonal/farther cells
        // (3,5,6,7) are not face-adjacent to cell 0 and stay L2 (they sit next
        // to L1/L2 cells, never an L0).
        let mut bricks: Vec<BrickClass> = Vec::new();
        for i in 0..8usize {
            let b = if i == 0 {
                brick(10.0, 10.0, (9.0, 9.0), (9.0, 9.0)) // L0
            } else {
                brick(10.0, 10.0, (0.1, 0.1), (0.1, 0.1)) // L2
            };
            // Spread AABBs so none overlap and none is protected.
            let (x, y, z) = (i & 1, (i >> 1) & 1, (i >> 2) & 1);
            let mut b = b;
            b.world_min = [x as f32 * 10.0, y as f32 * 10.0, z as f32 * 10.0];
            b.world_max = [
                x as f32 * 10.0 + 1.0,
                y as f32 * 10.0 + 1.0,
                z as f32 * 10.0 + 1.0,
            ];
            bricks.push(b);
        }
        let out = classify_levels(&bricks, [2, 2, 2], &[], &CoarsenParams::default());
        assert_eq!(out[0], L0);
        // Face neighbors of cell 0 demoted to L1.
        assert_eq!(out[1], L1, "+x neighbor");
        assert_eq!(out[2], L1, "+y neighbor");
        assert_eq!(out[4], L1, "+z neighbor");
        // Non-face-adjacent cells stay coarse (all adjacent diffs ≤ 1).
        for &c in &[3usize, 5, 6, 7] {
            assert_eq!(out[c], L2, "cell {c} not face-adjacent to the L0 corner");
        }
    }

    #[test]
    fn adjacency_3x1x1_row() {
        // A simple 3×1×1 row exercising x-fastest neighbor derivation with a
        // mid-row L0 forcing both sides.
        let bricks = vec![
            at_x(brick(10.0, 10.0, (0.1, 0.1), (0.1, 0.1)), 0), // L2
            at_x(brick(10.0, 10.0, (9.0, 9.0), (9.0, 9.0)), 1), // L0 (center)
            at_x(brick(10.0, 10.0, (0.1, 0.1), (0.1, 0.1)), 2), // L2
        ];
        let out = classify_levels(&bricks, [3, 1, 1], &[], &CoarsenParams::default());
        assert_eq!(out, vec![L1, L0, L1]);
    }
}
