//! Delta-SH probe coarsening classifier (slices G3-α + G3-δ).
//!
//! **α — the gate (`classify_levels`).** Pure numeric logic: given
//! already-computed per-brick reconstruction-error and magnitude statistics for
//! the affinity grid, decide each 4×4×4 affinity brick's probe-density
//! [`Level`] (L0 dense / L1 corners / L2 brick-mean), force protected bricks
//! back to L0, then run a fixpoint seam-smoothing sweep so no two face-adjacent
//! bricks differ by more than one level. This part is std-only apart from the
//! shared [`Level`] type and is unit-testable with hand-built inputs.
//!
//! **δ — the provider (`classify_section_levels`).** Turns pre-BC6H SH sections
//! into the α gate's [`BrickClass`] inputs and runs the gate per section. This
//! part depends on `postretro_level_format` section types and reuses
//! `sh_analyze`'s `pub(crate)` decode/accumulate helpers (`build_brick_tiles`,
//! `level_errors`, `tile_magnitude`, …), so — unlike α — it is not std-only.

use postretro_level_format::animated_direct_sh_delta_volumes::AnimatedDirectShDeltaVolumesSection;
use postretro_level_format::delta_sh_volumes::DeltaShVolumesSection;
use postretro_level_format::direct_sh_delta_volumes::DirectShDeltaVolumesSection;
use postretro_level_format::direct_sh_volume::DirectShVolumeSection;
use postretro_level_format::sh_reconstruct::{Level, Tile, local_xyz, zero_tile};
use postretro_level_format::sh_volume::OctahedralShVolumeSection;

use crate::affinity_grid::AFFINITY_FACTOR;
use crate::sh_analyze::{
    AnalyzeInputs, DeltaView, LevelKind, accumulate_delta_for_cell, brick_world_aabb,
    build_brick_tiles, level_errors, level_errors_with_l1_zero_fallback, tile_magnitude,
};

const AF: usize = AFFINITY_FACTOR as usize; // 4
const PROBES_PER_CELL: usize = AF * AF * AF; // 64

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
    debug_assert_eq!(
        bricks.len(),
        affinity_dims.iter().map(|&d| d as usize).product::<usize>(),
        "bricks.len() must equal product(affinity_dims); seam-smoothing indexes by affinity coords",
    );
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
    // Whether the brick may be represented as L1 after the gate. Seam
    // smoothing can turn an initially selected L2 into L1, so reconstructable
    // is insufficient: L1 must independently satisfy the same error contract.
    let mut l1_eligible: Vec<bool> = Vec::with_capacity(bricks.len());
    // Parallel participation flags: zero-valid bricks do not participate in
    // coarsening or seam-smoothing adjacency.
    let mut participating: Vec<bool> = Vec::with_capacity(bricks.len());

    for b in bricks {
        if !b.has_any_valid {
            levels.push(Level::L0);
            l1_eligible.push(false);
            participating.push(false);
            continue;
        }
        participating.push(true);

        let (level, l1_ok) = if b.mag_p95 < floor {
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
                (Level::L2, b.l1_evaluable)
            } else if b.l1_evaluable {
                (Level::L1, true)
            } else {
                (Level::L0, false)
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
                (Level::L2, l1_ok)
            } else if l1_ok {
                (Level::L1, true)
            } else {
                (Level::L0, false)
            }
        };
        levels.push(level);
        l1_eligible.push(l1_ok);
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
    // A brick whose L1 failed its own gate must never be assigned L1 by
    // demotion. L1 is not assumed to be more accurate than L2 for sparse
    // corner lattices.
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
                        demoted |= smooth_pair(&mut levels, &participating, &l1_eligible, i, i + 1);
                    }
                    if y + 1 < dyu {
                        demoted |=
                            smooth_pair(&mut levels, &participating, &l1_eligible, i, i + dxu);
                    }
                    if z + 1 < dzu {
                        demoted |= smooth_pair(
                            &mut levels,
                            &participating,
                            &l1_eligible,
                            i,
                            i + dxu * dyu,
                        );
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
/// pair), demote the coarser endpoint one step. Returns whether a demotion
/// occurred.
fn smooth_pair(
    levels: &mut [Level],
    participating: &[bool],
    l1_eligible: &[bool],
    a: usize,
    b: usize,
) -> bool {
    if !participating[a] || !participating[b] {
        return false;
    }
    let (la, lb) = (levels[a].to_u8(), levels[b].to_u8());
    if la.abs_diff(lb) < 2 {
        return false;
    }
    // Demote the coarser (numerically higher) endpoint.
    if la > lb {
        levels[a] = demote_one(levels[a], l1_eligible[a]);
    } else {
        levels[b] = demote_one(levels[b], l1_eligible[b]);
    }
    true
}

/// Demote a level by one step: L2→L1, L1→L0, L0 stays. **Exception:** when the
/// brick's L1 failed its independent gate (including unevaluable sparse
/// lattices), skip L1 and demote L2 straight to L0. Demoting to the
/// finest level still satisfies the ≤1-level seam bound (L0 is never the coarser
/// endpoint of any pair) and keeps the brick reconstructable.
fn demote_one(level: Level, l1_eligible: bool) -> Level {
    match level {
        Level::L2 => {
            if l1_eligible {
                Level::L1
            } else {
                Level::L0
            }
        }
        Level::L1 => Level::L0,
        Level::L0 => Level::L0,
    }
}

// ---------------------------------------------------------------------------
// Composed-tile → classification provider (slice G3-δ)
// ---------------------------------------------------------------------------
//
// Turns raw pre-BC6H SH sections into the pure [`BrickClass`] inputs the α gate
// consumes, then runs the gate. Classification is PER SECTION: coarsening one
// delta section perturbs only that section's contribution to the composed
// receiver, so by linearity the induced composed-receiver error equals that
// section's own reconstruction error. The magnitude denominator and the
// map-wide darkness floor, however, are the COMPOSED brightness (base indirect +
// base direct + Σ all three delta sections), shared across sections. All
// tile-decode / accumulate / reconstruction math is reused verbatim from
// `sh_analyze` so the numbers match the measurement pass exactly.

/// Which of the three affinity-CSR delta sections (ids 27/41/45) a run targets.
#[derive(Clone, Copy)]
pub(crate) enum TargetDeltaSection {
    /// Indirect delta (id 27).
    Indirect,
    /// Direct delta (id 41).
    Direct,
    /// Animated-direct delta (id 45).
    AnimatedDirect,
}

/// Borrowed handles to the three delta sections. All present sections feed the
/// SHARED composed magnitude; [`TargetDeltaSection`] selects which one's own
/// reconstruction error becomes the per-section numerator.
#[derive(Clone, Copy, Default)]
pub(crate) struct DeltaSectionsRef<'a> {
    pub indirect: Option<&'a DeltaShVolumesSection>,
    pub direct: Option<&'a DirectShDeltaVolumesSection>,
    pub anim_direct: Option<&'a AnimatedDirectShDeltaVolumesSection>,
}

/// Probe-grid geometry: enough to place bricks in the world (AABB) and map
/// probes to affinity cells. `validity` is per base probe (len ==
/// product(`grid_dims`)), x-fastest, non-zero ⇒ valid.
#[derive(Clone, Copy)]
pub(crate) struct SectionGrid<'a> {
    pub grid_origin: [f32; 3],
    pub cell_size: [f32; 3],
    pub grid_dims: [u32; 3],
    pub validity: &'a [u8],
}

/// Classify one delta section's per-cell coarsening levels from the composed
/// receiver. `magnitude` is the COMPOSED brightness (shared across sections);
/// `error` is THIS section's own reconstruction error (per-section, by
/// linearity = the composed error it induces). Returns per-cell Level-as-u8,
/// x-fastest, length == affinity_cell_count.
pub(crate) fn classify_section_levels(
    base_indirect: &OctahedralShVolumeSection, // pre-BC6H RGBA16F id34
    base_direct: Option<&DirectShVolumeSection>, // pre-BC6H id35 (may be absent)
    all_deltas: DeltaSectionsRef<'_>,          // the three sections (for composed magnitude)
    target: TargetDeltaSection,                // which section's levels to produce
    grid: SectionGrid<'_>,                     // affinity dims + origin + spacing + validity
    protect_aabbs: &[[f32; 6]],
    params: &CoarsenParams,
) -> Vec<u8> {
    let base = base_indirect;
    let tile_dim = base.tile_dimension as usize;
    let border = base.tile_border as usize;
    let interior = tile_dim.saturating_sub(2 * border);
    let texels = interior * interior;

    let dims = grid.grid_dims;
    let (nx, ny, nz) = (dims[0] as usize, dims[1] as usize, dims[2] as usize);
    let total_probes = nx * ny * nz;

    // Affinity grid = ceil(dims / AF), x-fastest — the same derivation
    // `run_analysis` uses, and the layout `classify_levels` expects.
    let ax = nx.div_ceil(AF);
    let ay = ny.div_ceil(AF);
    let az = nz.div_ceil(AF);
    let brick_count = ax * ay * az;
    let affinity_dims = [ax as u32, ay as u32, az as u32];

    // Degenerate geometry: nothing to classify (mirrors run_analysis' guard).
    if total_probes == 0 || interior == 0 || brick_count == 0 {
        return vec![Level::L0.to_u8(); brick_count];
    }

    // Per-probe compact-atlas rank (x-fastest), identical to run_analysis so the
    // reused base-indirect decoder resolves the right compact slot.
    let mut valid_rank = vec![-1i64; total_probes];
    let mut rank = 0i64;
    for (i, r) in valid_rank.iter_mut().enumerate() {
        if grid.validity.get(i).copied().unwrap_or(0) != 0 {
            *r = rank;
            rank += 1;
        }
    }

    // Unweighted metric (cosine weighting dropped — operating-point.md): pass an
    // all-ones weight vector so `level_errors` / the magnitude take the
    // unweighted path, matching how `run_analysis` reports the unweighted stats.
    let weights = vec![1.0f32; texels];

    // CSR views over every PRESENT delta section, dropping any whose affinity
    // grid disagrees with this probe grid (same guard as run_analysis'
    // `check_delta`) — a misaligned section cannot be indexed by this grid's
    // linear cell id, so it is excluded from the composed sum here too.
    fn guard<'a>(view: Option<DeltaView<'a>>, expected: [u32; 3]) -> Option<DeltaView<'a>> {
        view.filter(|v| v.affinity_dims == expected)
    }
    let dv_ind = guard(
        all_deltas.indirect.map(DeltaView::from_indirect),
        affinity_dims,
    );
    let dv_dir = guard(all_deltas.direct.map(DeltaView::from_direct), affinity_dims);
    let dv_anim = guard(
        all_deltas.anim_direct.map(DeltaView::from_anim_direct),
        affinity_dims,
    );

    // The target section's own view. Absent (or affinity-mismatched) ⇒ nothing
    // to classify: every brick stays L0.
    let target_view = match target {
        TargetDeltaSection::Indirect => dv_ind.as_ref(),
        TargetDeltaSection::Direct => dv_dir.as_ref(),
        TargetDeltaSection::AnimatedDirect => dv_anim.as_ref(),
    };
    let Some(target_view) = target_view else {
        return vec![Level::L0.to_u8(); brick_count];
    };

    // Inputs for the reused brick assembly / world-AABB helpers. `build_brick_tiles`
    // reads only `base_direct`; `brick_world_aabb` reads only origin/cell_size;
    // neither touches `protect_aabbs`/`thresholds`, so empty slices suffice. The
    // provider applies its own `protect_aabbs` downstream via `classify_levels`.
    let inputs = AnalyzeInputs {
        grid_origin: grid.grid_origin,
        cell_size: grid.cell_size,
        grid_dims: dims,
        validity: grid.validity,
        base_indirect: base,
        base_direct,
        delta_indirect: all_deltas.indirect,
        delta_direct: all_deltas.direct,
        delta_anim_direct: all_deltas.anim_direct,
        protect_aabbs: &[],
        thresholds: &[],
    };

    let mut bricks: Vec<BrickClass> = Vec::with_capacity(brick_count);
    for cz in 0..az {
        for cy in 0..ay {
            for cx in 0..ax {
                let cell_lin = cx + cy * ax + cz * ax * ay;

                // COMPOSED receiver tiles (shared across sections) = base
                // indirect + base direct + Σ all three delta sections. This is
                // exactly `build_brick_tiles`' `composed` output, so the
                // magnitude denominator + darkness floor are identical no matter
                // which section is the target.
                let bt = build_brick_tiles(
                    &inputs,
                    base,
                    tile_dim,
                    interior,
                    border,
                    &valid_rank,
                    dims,
                    cell_lin,
                    cx,
                    cy,
                    cz,
                    ax,
                    ay,
                    &dv_ind,
                    &dv_dir,
                    &dv_anim,
                );
                let mag = tile_magnitude(&bt.composed, texels);

                // THE TARGET SECTION'S OWN delta tiles for this brick: seed an
                // all-zero accumulator, fold in ONLY the target section, then
                // expose Some(tile) exactly at the target's in-bounds valid
                // probes (its `valid_probe_mask`). Reconstruction error over
                // these dense tiles is, by linearity, the composed error that
                // coarsening THIS section would induce.
                let mut acc: [Tile; PROBES_PER_CELL] = std::array::from_fn(|_| zero_tile(texels));
                accumulate_delta_for_cell(target_view, cell_lin, interior, border, &mut acc);

                // Scored valid set = BASE validity (the sole validity authority,
                // and exactly the mask valid-probe compaction will emit for this
                // section). The dense, pre-compaction section carries an all-ones
                // `valid_probe_mask`, so scoring from the section mask would wrongly
                // include in-solid probes. Base validity is section-agnostic, so all
                // three sections share the same valid set — hence α computes one
                // consistent map-wide darkness floor across sections.
                const NONE_TILE: Option<Tile> = None;
                let mut s_tiles: [Option<Tile>; PROBES_PER_CELL] = [NONE_TILE; PROBES_PER_CELL];
                let mut any_valid = false;
                for local in 0..PROBES_PER_CELL {
                    let (lx, ly, lz) = local_xyz(local);
                    let (px, py, pz) = (cx * AF + lx, cy * AF + ly, cz * AF + lz);
                    if px >= nx || py >= ny || pz >= nz {
                        continue;
                    }
                    let probe_index = px + py * nx + pz * nx * ny;
                    if grid.validity.get(probe_index).copied().unwrap_or(0) == 0 {
                        continue;
                    }
                    any_valid = true;
                    s_tiles[local] = Some(std::mem::take(&mut acc[local]));
                }

                let l1 = level_errors_with_l1_zero_fallback(&s_tiles, texels, interior, &weights);
                let l2 = level_errors(&s_tiles, LevelKind::L2, texels, interior, &weights);

                let (wmin, wmax) = brick_world_aabb(&inputs, dims, cx, cy, cz);

                bricks.push(BrickClass {
                    // Shared composed brightness.
                    mag_p95: mag.p95,
                    mag_max: mag.max,
                    // Per-section (target-only) reconstruction error.
                    l1_p95: l1.p95,
                    l1_max: l1.max,
                    l1_evaluable: l1.texel_samples > 0,
                    l2_p95: l2.p95,
                    l2_max: l2.max,
                    l2_evaluable: l2.texel_samples > 0,
                    // ≥1 base-valid probe in this brick (section-agnostic).
                    has_any_valid: any_valid,
                    // Shared world AABB.
                    world_min: [wmin.x, wmin.y, wmin.z],
                    world_max: [wmax.x, wmax.y, wmax.z],
                });
            }
        }
    }

    classify_levels(&bricks, affinity_dims, protect_aabbs, params)
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
        assert_eq!(
            out,
            vec![L0],
            "passing p95 but failing rel_max must stay finer"
        );
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
        assert!(
            out.iter().any(|&l| l != L0),
            "dark map must not be forced all-L0"
        );
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
        assert_eq!(
            out[3], L2,
            "sub-floor brick must bypass to coarsest evaluable level"
        );
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
        assert!(
            out.iter().all(|&l| l == L2),
            "must be L2-only coarsening, never L1"
        );
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
        assert_eq!(
            out[1], L2,
            "L2 neighbor not demoted by a non-participating brick"
        );
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

    #[test]
    fn smoothing_demotes_l2_to_l0_when_l1_is_unevaluable() {
        // P9/P11 as a whole-classifier property, not just a gate property: a
        // brick that gates L2 but whose L1 is unevaluable (no valid corner ⇒
        // kept_mask(L1)==0) must, when the seam pass demotes it beside an L0
        // neighbor, go straight to L0 — NOT L1, which would store zero tiles and
        // leave every dropped-valid probe unreconstructable.
        let mut coarse = brick(10.0, 10.0, (0.1, 0.1), (0.1, 0.1)); // gates L2
        coarse.l1_evaluable = false; // no valid corner
        let bricks = vec![
            at_x(brick(10.0, 10.0, (9.0, 9.0), (9.0, 9.0)), 0), // L0
            at_x(coarse, 1),                                    // L2, L1 unevaluable
        ];
        let out = classify_levels(&bricks, [2, 1, 1], &[], &CoarsenParams::default());
        assert_eq!(out[0], L0, "finer endpoint unchanged");
        assert_eq!(
            out[1], L0,
            "L2 brick with unevaluable L1 demotes straight to L0, skipping L1"
        );
    }

    #[test]
    fn smoothing_skips_l1_when_l2_passes_but_l1_fails() {
        // Regression: L1 is not necessarily more accurate than L2 for a sparse
        // corner lattice. Brick 1 passes L2 but fails L1; the adjacent L0 brick
        // forces smoothing to refine it. The final post-smoothing level must be
        // a gate-valid L0, not an unchecked L1.
        let bricks = vec![
            at_x(brick(10.0, 10.0, (9.0, 9.0), (9.0, 9.0)), 0), // L0
            at_x(brick(10.0, 10.0, (5.0, 5.0), (0.1, 0.1)), 1), // L2; L1 fails
        ];
        let out = classify_levels(&bricks, [2, 1, 1], &[], &CoarsenParams::default());
        assert_eq!(
            out,
            vec![L0, L0],
            "smoothing must skip an L1 representation that failed its own gate"
        );
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

    // ---- Provider wiring (classify_section_levels) ----
    //
    // These prove the PROVIDER wiring (composed magnitude vs per-section error),
    // not α's gate (which is covered above). Fixtures use a single 4×4×4 brick
    // (grid [4,4,4] → one affinity cell, all 64 probes valid) with 1×1 tiles
    // (tile_dimension 1, border 0 → 1 interior texel).

    use postretro_level_format::lightmap::{IRRADIANCE_FORMAT_RGBA16F, f32_to_f16_bits};

    /// Base indirect (id 34) whose every probe decodes to a uniform RGB
    /// `value` — a bright, dense composed floor to test the coarsener against.
    fn bright_base_indirect(dims: [u32; 3], value: f32) -> OctahedralShVolumeSection {
        let total = dims[0] as usize * dims[1] as usize * dims[2] as usize;
        let mut base = OctahedralShVolumeSection::placeholder();
        base.grid_origin = [0.0; 3];
        base.cell_size = [1.0; 3];
        base.grid_dimensions = dims;
        base.tile_dimension = 1;
        base.tile_border = 0;
        base.irradiance_format = IRRADIANCE_FORMAT_RGBA16F;
        base.compact_atlas_dimensions = [total as u32, 1];
        base.compact_atlas_tiles_per_row = total as u32;
        base.compact_atlas_tiles_per_layer = total as u32;
        base.compact_atlas_layer_count = 1;
        let [lo, hi] = f32_to_f16_bits(value).to_le_bytes();
        let mut atlas = vec![0u8; total * 8];
        for r in 0..total {
            let b = r * 8;
            // R, G, B channels; A left at 0.
            atlas[b] = lo;
            atlas[b + 1] = hi;
            atlas[b + 2] = lo;
            atlas[b + 3] = hi;
            atlas[b + 4] = lo;
            atlas[b + 5] = hi;
        }
        base.compact_atlas = atlas;
        base
    }

    /// Direct-delta section (id 41) for a single all-valid affinity cell whose
    /// per-local-probe RGB delta is `f(local)`.
    fn direct_delta_from(f: impl Fn(usize) -> f32) -> DirectShDeltaVolumesSection {
        let mut sub = vec![0u16; PROBES_PER_CELL * 4];
        for local in 0..PROBES_PER_CELL {
            let h = f32_to_f16_bits(f(local));
            sub[local * 4] = h; // R
            sub[local * 4 + 1] = h; // G
            sub[local * 4 + 2] = h; // B
            // A left at 0.
        }
        DirectShDeltaVolumesSection {
            affinity_factor: 4,
            affinity_dims: [1, 1, 1],
            tile_dimension: 1,
            tile_border: 0,
            valid_probe_masks: vec![u64::MAX],
            cell_levels: vec![0u8; 1],
            affinity_offsets: vec![0, 1],
            affinity_lights: vec![0],
            delta_subblocks: sub,
        }
    }

    fn all_valid(dims: [u32; 3]) -> Vec<u8> {
        vec![1u8; dims[0] as usize * dims[1] as usize * dims[2] as usize]
    }

    fn only_valid(dims: [u32; 3], locals: &[usize]) -> Vec<u8> {
        let mut validity = vec![0u8; dims[0] as usize * dims[1] as usize * dims[2] as usize];
        for &local in locals {
            validity[local] = 1;
        }
        validity
    }

    fn grid_ref<'a>(dims: [u32; 3], validity: &'a [u8]) -> SectionGrid<'a> {
        SectionGrid {
            grid_origin: [0.0; 3],
            cell_size: [1.0; 3],
            grid_dims: dims,
            validity,
        }
    }

    /// High-frequency parity pattern: not reproducible by an L2 mean or an L1
    /// trilinear-from-corners blend, so it forces high per-section error.
    fn parity(local: usize, amp: f32) -> f32 {
        let (lx, ly, lz) = local_xyz(local);
        if (lx + ly + lz) % 2 == 0 { amp } else { -amp }
    }

    #[test]
    fn provider_uniform_target_over_bright_composed_coarsens() {
        // Bright composed (base 10) + a spatially-uniform target delta (0.5) →
        // the target's own reconstruction error is ~0, so it coarsens to L2.
        let dims = [4, 4, 4];
        let validity = all_valid(dims);
        let base = bright_base_indirect(dims, 10.0);
        let delta = direct_delta_from(|_| 0.5);
        let deltas = DeltaSectionsRef {
            direct: Some(&delta),
            ..Default::default()
        };
        let out = classify_section_levels(
            &base,
            None,
            deltas,
            TargetDeltaSection::Direct,
            grid_ref(dims, &validity),
            &[],
            &CoarsenParams::default(),
        );
        assert_eq!(
            out,
            vec![L2],
            "uniform target vs bright composed must coarsen to L2"
        );
    }

    #[test]
    fn provider_high_variance_target_stays_l0() {
        // Same bright composed, but a high-variance target delta (±8): the
        // target's own L1/L2 reconstruction error is large relative to the
        // (bright) composed magnitude, so it must stay dense (L0).
        let dims = [4, 4, 4];
        let validity = all_valid(dims);
        let base = bright_base_indirect(dims, 10.0);
        let delta = direct_delta_from(|l| parity(l, 8.0));
        let deltas = DeltaSectionsRef {
            direct: Some(&delta),
            ..Default::default()
        };
        let out = classify_section_levels(
            &base,
            None,
            deltas,
            TargetDeltaSection::Direct,
            grid_ref(dims, &validity),
            &[],
            &CoarsenParams::default(),
        );
        assert_eq!(
            out,
            vec![L0],
            "high-variance target must stay L0 despite bright composed"
        );
    }

    #[test]
    fn provider_sparse_l1_scores_shader_zero_fallback() {
        // Regression: Stress-Warren cell 5213 had a valid non-corner target
        // whose surviving L1 corner had zero trilinear weight. The old scorer
        // omitted that target, while the emitted shader path reconstructed it
        // as zero. The zero fallback's real error must reject L1 here.
        let dims = [4, 4, 4];
        let validity = only_valid(dims, &[0, 7]);
        let base = bright_base_indirect(dims, 10.0);
        let delta = direct_delta_from(|local| match local {
            0 => 0.5,
            7 => 50.0,
            _ => 0.0,
        });
        let deltas = DeltaSectionsRef {
            direct: Some(&delta),
            ..Default::default()
        };
        let out = classify_section_levels(
            &base,
            None,
            deltas,
            TargetDeltaSection::Direct,
            grid_ref(dims, &validity),
            &[],
            &CoarsenParams::default(),
        );
        assert_eq!(
            out,
            vec![L0],
            "a sparse L1 target represented as zero must contribute to the gate"
        );
    }

    #[test]
    fn provider_magnitude_denominator_is_composed_not_target() {
        // The denominator (local brightness) must be the COMPOSED receiver, not
        // the target section alone. Bright base (100) + a small high-variance
        // target delta (±1, magnitude ~1): the L2 relative error is
        // 1/101 ≈ 0.01 against the composed receiver → coarsens (L2). Were the
        // magnitude taken from the target section alone (~1), the relative error
        // would be 1/1 = 1.0 and the gate would force L0. L2 proves the
        // denominator is base + all deltas, so a near-zero-vs-base target brick
        // is NOT mistaken for a dark brick.
        let dims = [4, 4, 4];
        let validity = all_valid(dims);
        let base = bright_base_indirect(dims, 100.0);
        let delta = direct_delta_from(|l| parity(l, 1.0));
        let deltas = DeltaSectionsRef {
            direct: Some(&delta),
            ..Default::default()
        };
        let out = classify_section_levels(
            &base,
            None,
            deltas,
            TargetDeltaSection::Direct,
            grid_ref(dims, &validity),
            &[],
            &CoarsenParams::default(),
        );
        assert_eq!(
            out,
            vec![L2],
            "coarsening must key off the bright composed magnitude, not the dim target section"
        );
    }

    #[test]
    fn provider_absent_target_is_all_l0() {
        // No target section present → nothing to classify → every cell L0.
        // Grid [8,4,4] → 2 affinity cells along x, so the all-L0 vector length
        // is exercised too.
        let dims = [8, 4, 4];
        let validity = all_valid(dims);
        let base = bright_base_indirect(dims, 10.0);
        let out = classify_section_levels(
            &base,
            None,
            DeltaSectionsRef::default(),
            TargetDeltaSection::Direct,
            grid_ref(dims, &validity),
            &[],
            &CoarsenParams::default(),
        );
        assert_eq!(out, vec![L0, L0], "absent target section ⇒ all cells L0");
    }

    #[test]
    fn provider_base_invalid_brick_is_l0_despite_dense_mask() {
        // A brick with NO base-valid probes must stay L0 (non-participating),
        // even though the dense pre-compaction section carries an all-ones
        // `valid_probe_mask` and a spatially-uniform delta that would otherwise
        // gate to L2. This pins the fix: scoring uses base validity, not the
        // section's dense mask.
        let dims = [4, 4, 4];
        let validity = vec![0u8; 4 * 4 * 4]; // every base probe in-solid
        let base = bright_base_indirect(dims, 10.0);
        let delta = direct_delta_from(|_| 0.5); // uniform ⇒ would be L2 if scored
        let deltas = DeltaSectionsRef {
            direct: Some(&delta),
            ..Default::default()
        };
        let out = classify_section_levels(
            &base,
            None,
            deltas,
            TargetDeltaSection::Direct,
            grid_ref(dims, &validity),
            &[],
            &CoarsenParams::default(),
        );
        assert_eq!(
            out,
            vec![L0],
            "a base-invalid brick must be L0 regardless of the dense section mask"
        );
    }
}
