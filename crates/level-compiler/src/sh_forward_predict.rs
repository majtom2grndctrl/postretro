//! Output-preserving base-density forward-predictor efficacy harness (spike,
//! measurement only). Task 2: the full input-cost spectrum — P1 contribution
//! geometry, P2 + occlusion, the distance / surface-distance negative control,
//! and the P3 direct-field ceiling — ALL scored against the one Task 1 oracle.
//! See: context/plans/in-progress/lighting-scale--base-density-forward-predictor-spike/index.md
//!
//! Question the harness exists to answer: can a cheap forward signal — evaluable
//! from light geometry and probe positions BEFORE the base-indirect SH bake —
//! reproduce the post-bake composed-receiver coarsenability oracle accurately
//! enough (near-zero unsafe false-positives) to justify a deferred base-probe-
//! density spec? This module computes the oracle (reusing `sh_analyze`'s brick
//! primitives + `sh_coarsen::CoarsenParams`, so its numbers are the production
//! gate the foundational spec would ship) once, then scores each predictor family
//! against it over a threshold sweep — the false-positive-rate vs recovered-
//! savings tradeoff curve, per family.
//!
//! Every family reduces to the same shape: a per-probe scalar field over the
//! brick, then the SAME spatial-variation proxies (L2 = relative max deviation
//! from the brick mean; L1 = relative trilinear-corner residual) and the SAME
//! scoring. Families differ ONLY in the per-probe scalar — delivered light (P1),
//! occluded delivered light (P2), nearest-light distance / nearest-surface
//! distance (the invalidated distance control), or the baked direct-at-probe
//! magnitude (P3 ceiling).
//!
//! Byte-preserving: it reads the finalized owned sections, the pre-bake light
//! set, and the bake BVH, mutates nothing that reaches the packer, and writes
//! only its own JSON. Measure-and-report only — no accuracy/cost threshold is
//! contracted in code.

use std::collections::BTreeMap;
use std::path::Path;

use bvh::bvh::Bvh;
use glam::Vec3;
use postretro_level_format::sh_reconstruct::{Level, corner_locals, local_xyz, trilinear_weight};
use serde::Serialize;

use crate::affinity_grid::AFFINITY_FACTOR;
use crate::bvh_build::BvhPrimitive;
use crate::geometry::GeometryResult;
use crate::map_data::{LightType, MapLight};
use crate::sh_analyze::{AnalyzeInputs, BrickRecord, decode_base_direct_tile, run_analysis};
use crate::sh_bake::{RaytracingCtx, incident_radiance_at_point, segment_clear};
use crate::sh_coarsen::CoarsenParams;

const AF: usize = AFFINITY_FACTOR as usize; // 4
const PROBES_PER_CELL: usize = AF * AF * AF; // 64

/// Distance a directional-light shadow ray is cast toward the light to test for
/// a finite occluder. Directional lights have no finite origin, so P2 traces a
/// long segment along the incident direction; anything short-circuiting it is a
/// shadow boundary.
const DIRECTIONAL_SHADOW_DISTANCE: f32 = 1.0e4;

/// Predictor decision thresholds swept for the tradeoff curve. The predictor's
/// per-brick proxies are RELATIVE (per-probe scalar deviation normalized by the
/// brick's mean scalar), so these are dimensionless relative-deviation cutoffs
/// bracketing near-uniform (0.01) up past the oracle's own relative gate band
/// (`rel_max_max` 0.25). Mirrors `sh_analyze::DEFAULT_THRESHOLDS`' swept-cutoff
/// style; a debug-tool knob, never a user-facing setting.
pub const DEFAULT_PREDICTOR_THRESHOLDS: [f32; 11] = [
    0.01, 0.02, 0.03, 0.05, 0.075, 0.10, 0.15, 0.20, 0.25, 0.35, 0.50,
];

// ---------------------------------------------------------------------------
// Inputs
// ---------------------------------------------------------------------------

/// Everything the harness needs, routed from the pipeline at the post-section-
/// finalization seam. `analyze` is the same `AnalyzeInputs` the `--sh-analyze`
/// pass builds — reused verbatim so the oracle is numerically the production
/// classification of the base-indirect field. The bake BVH + geometry back the
/// P2 occlusion and surface-distance families.
pub struct ForwardPredictInputs<'a> {
    pub analyze: AnalyzeInputs<'a>,
    /// The static (baked) light set feeding the base-indirect SH bake — the
    /// field the predictor forecasts. In base-indirect bake order.
    pub lights: &'a [&'a MapLight],
    /// Count of animated baked lights (id 27 source). Recorded for the fixture-
    /// honesty precondition (delta-indirect + aimed spots), not used by scoring.
    pub animated_light_count: usize,
    pub predictor_thresholds: &'a [f32],
    /// Bake BVH + primitives + geometry — the same triples the base bake traces.
    /// P2 shadow-tests each light against these; surface distance scans the
    /// geometry triangles.
    pub bvh: &'a Bvh<f32, 3>,
    pub primitives: &'a [BvhPrimitive],
    pub geometry: &'a GeometryResult,
}

// ---------------------------------------------------------------------------
// Report structures (JSON-serialized, mirrors `--sh-analyze-out`)
// ---------------------------------------------------------------------------

#[derive(Serialize, Clone, Default)]
pub struct LevelHistogram {
    pub l0: u64,
    pub l1: u64,
    pub l2: u64,
}

impl LevelHistogram {
    fn bump(&mut self, level: Level) {
        match level {
            Level::L0 => self.l0 += 1,
            Level::L1 => self.l1 += 1,
            Level::L2 => self.l2 += 1,
        }
    }
}

/// One brick's predictor signals and the shared oracle classification.
/// `linear_cell` keys back to the `sh_analyze` `BrickRecord`.
#[derive(Serialize, Clone, Default)]
pub struct BrickPrediction {
    pub cell: [u32; 3],
    pub linear_cell: u32,
    pub valid_probes: u32,
    /// Mean of the family's per-probe scalar over the brick's valid probes — the
    /// relative-deviation anchor (delivered magnitude for P1/P2, distance for the
    /// distance control, baked direct magnitude for P3).
    pub mean_scalar: f32,
    /// Primary continuous coarsenability score: relative brick-mean deviation of
    /// the per-probe scalar (the L2 proxy). Lower = more uniform = more
    /// coarsenable. `f32::INFINITY` maps to this being unevaluable.
    pub score: f32,
    /// L1 (trilinear-corner) relative residual proxy; `None` when the brick's 8
    /// corners are not all evaluable (mirroring the oracle).
    pub l1_proxy: Option<f32>,
    /// L2 (brick-mean) relative deviation proxy; `None` when unevaluable.
    pub l2_proxy: Option<f32>,
    /// Oracle level: coarsest base-indirect level the production relative gate
    /// admits (0/1/2). Shared across all families.
    pub oracle_level: u8,
}

/// One swept-threshold operating point: the predictor-vs-oracle confusion matrix
/// and its derived tradeoff metrics.
#[derive(Serialize, Clone, Default)]
pub struct SweepRow {
    pub threshold: f32,
    /// `confusion[pred][oracle]` counts; index 0/1/2 == L0/L1/L2.
    pub confusion: [[u64; 3]; 3],
    /// Bricks the predictor calls strictly COARSER than the oracle — the unsafe,
    /// under-baked direction. This is the metric the recommendation rests on.
    pub false_positive_bricks: u64,
    pub false_positive_rate: f32,
    /// Predictor and oracle agree exactly on the level.
    pub agreement_bricks: u64,
    pub agreement_rate: f32,
    /// Oracle-coarsenable bricks (oracle in {L1,L2}) — the recovered-savings
    /// denominator.
    pub oracle_coarsenable: u64,
    /// Of those, how many the predictor also frees at a SAFE level (predictor
    /// coarsens the brick and no coarser than the oracle).
    pub recovered_safe: u64,
    pub recovered_savings_fraction: f32,
}

/// One predictor family's full evaluation against the shared oracle — its own
/// per-brick scores, threshold sweep, best operating point, correlation, and
/// evaluation wall-time. The `families` map holds one per named family so the
/// cost/accuracy axis is per-family.
#[derive(Serialize, Clone, Default)]
pub struct FamilyReport {
    pub name: String,
    pub description: String,
    /// Pearson correlation of the continuous predictor score with the oracle
    /// coarseness level over evaluable bricks (contribution-aware signal
    /// strength). NaN-guarded to 0.0 when variance is zero.
    pub score_vs_oracle_correlation: f32,
    /// Wall-time of THIS family's per-brick evaluation (excludes the shared
    /// oracle decode) — the per-family cost side of "cheap enough to save bake
    /// time".
    pub predictor_eval_seconds: f64,
    /// Best near-zero-FP operating point: index into `sweep` maximizing recovered
    /// savings subject to zero false positives; falls back to the minimum-FP row
    /// when no zero-FP row exists. `None` with no non-empty bricks.
    pub best_operating_point: Option<usize>,
    pub sweep: Vec<SweepRow>,
    /// Cost/viability caveat (e.g. P3 is a ceiling because id 35 bakes after
    /// id 34). Empty for the cheap forward candidates.
    pub cost_note: String,
    pub bricks: Vec<BrickPrediction>,
}

#[derive(Serialize, Clone, Default)]
pub struct ForwardPredictReport {
    pub grid_dims: [u32; 3],
    pub cell_size: [f32; 3],
    pub brick_count: u64,
    pub nonempty_bricks: u64,
    // Fixture-honesty precondition surface.
    pub has_base_direct: bool,
    pub has_delta_indirect: bool,
    pub light_count: u64,
    pub animated_light_count: u64,
    // Oracle gate parameters (echoed for provenance).
    pub rel_p95_max: f32,
    pub rel_max_max: f32,
    pub darkness_frac: f32,
    pub map_p95: f32,
    pub darkness_floor: f32,
    pub oracle_histogram: LevelHistogram,
    /// P3 open-question measurement: is the direct-at-probe field (id 35) baked
    /// BEFORE the base-indirect field (id 34) it would predict? Measured from the
    /// compiler stage order, not assumed. `false` here means P3 is a ceiling
    /// reference only, never a viable cheap pre-bake predictor.
    pub p3_direct_baked_before_base_indirect: bool,
    pub p3_ordering_note: String,
    // --- P1 top-level surface, preserved from Task 1 (== families["P1"]). ---
    /// Pearson correlation of P1's continuous score with oracle level.
    pub score_vs_oracle_correlation: f32,
    pub best_operating_point: Option<usize>,
    pub sweep: Vec<SweepRow>,
    /// Wall-time of P1's evaluation (kept at top level for Task 1 continuity;
    /// per-family times live in `families`).
    pub predictor_eval_seconds: f64,
    pub bricks: Vec<BrickPrediction>,
    // --- Task 2: the full family spectrum, addressable by name. ---
    /// Every family keyed by name (`P1`, `P2`, `distance`, `surface_distance`,
    /// `P3`), each scored against the SAME oracle above.
    pub families: BTreeMap<String, FamilyReport>,
}

// ---------------------------------------------------------------------------
// Oracle — reuse sh_analyze BrickRecord primitives + sh_coarsen::CoarsenParams
// ---------------------------------------------------------------------------

/// Coarsest base-indirect level admitted by the production relative gate, per
/// brick. Numerator = base-indirect reconstruction error (`base_l1`/`base_l2`),
/// denominator = composed magnitude — mirroring `sh_coarsen::classify_levels`'
/// Phase A map-p95 / Phase B per-brick gate exactly (no seam-smoothing, no
/// protection: the oracle is the raw per-brick classification the predictor is
/// scored against).
fn oracle_level(record: &BrickRecord, floor: f32, params: &CoarsenParams) -> Level {
    let mag_p95 = record.composed_magnitude.p95;
    let mag_max = record.composed_magnitude.max;
    let l1_ev = record.base_l1.evaluable;
    let l2_ev = record.base_l2.evaluable;

    if mag_p95 < floor {
        // Darkness bypass: a sub-floor brick takes the coarsest EVALUABLE level.
        if l2_ev {
            Level::L2
        } else if l1_ev {
            Level::L1
        } else {
            Level::L0
        }
    } else {
        // Bright brick: coarsest level passing BOTH relative thresholds. An
        // unevaluable level fails (scores +inf).
        let l2_ok = l2_ev
            && record.base_l2.p95 / mag_p95.max(floor) <= params.rel_p95_max
            && record.base_l2.max / mag_max.max(floor) <= params.rel_max_max;
        let l1_ok = l1_ev
            && record.base_l1.p95 / mag_p95.max(floor) <= params.rel_p95_max
            && record.base_l1.max / mag_max.max(floor) <= params.rel_max_max;
        if l2_ok {
            Level::L2
        } else if l1_ok {
            Level::L1
        } else {
            Level::L0
        }
    }
}

/// Map-wide p95 magnitude, matching `sh_coarsen::classify_levels`' truncated
/// order statistic (`floor(0.95 * (n-1))`) over the non-empty bricks.
fn map_p95_magnitude(bricks: &[BrickRecord]) -> f32 {
    if bricks.is_empty() {
        return 0.0;
    }
    let mut mags: Vec<f32> = bricks.iter().map(|b| b.composed_magnitude.p95).collect();
    mags.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    let idx = ((mags.len() - 1) as f32 * 0.95).floor() as usize;
    mags[idx]
}

// ---------------------------------------------------------------------------
// Per-brick spatial-variation proxies (shared across all families)
// ---------------------------------------------------------------------------

/// The predictor's per-brick signals. Each proxy is `None` when unevaluable
/// (the level's reconstruction basis is absent — e.g. L1 with a non-present
/// corner), mirroring the oracle's per-level ineligibility.
struct BrickSignal {
    mean_scalar: f32,
    l1_proxy: Option<f32>,
    l2_proxy: Option<f32>,
}

/// Compute the L1/L2 spatial-variation proxies for one brick from a per-probe
/// scalar field. `scalar(probe_index, world_point) -> Option<f32>` is the family-
/// specific per-probe value; `None` marks a probe with no evaluable scalar
/// (mirroring an invalid probe). The proxies are RELATIVE to the brick's mean
/// scalar — a pre-bake predictor cannot know composed magnitude, so it normalizes
/// by the local scalar (delivered brightness, distance, or baked direct), the
/// closest forward analog.
fn brick_signal_scalar<F: Fn(usize, Vec3) -> Option<f32>>(
    scalar: F,
    grid_origin: Vec3,
    cell_size: Vec3,
    dims: [u32; 3],
    cx: usize,
    cy: usize,
    cz: usize,
    validity: &[u8],
) -> BrickSignal {
    let (nx, ny, nz) = (dims[0] as usize, dims[1] as usize, dims[2] as usize);

    let mut mag = [0.0f32; PROBES_PER_CELL];
    let mut present = [false; PROBES_PER_CELL];
    let mut sum = 0.0f64;
    let mut count = 0usize;
    for local in 0..PROBES_PER_CELL {
        let (lx, ly, lz) = local_xyz(local);
        let (px, py, pz) = (cx * AF + lx, cy * AF + ly, cz * AF + lz);
        if px >= nx || py >= ny || pz >= nz {
            continue;
        }
        let probe_index = px + py * nx + pz * nx * ny;
        if validity.get(probe_index).copied().unwrap_or(0) == 0 {
            continue;
        }
        let point = grid_origin
            + Vec3::new(
                px as f32 * cell_size.x,
                py as f32 * cell_size.y,
                pz as f32 * cell_size.z,
            );
        let Some(m) = scalar(probe_index, point) else {
            continue;
        };
        mag[local] = m;
        present[local] = true;
        sum += m as f64;
        count += 1;
    }

    if count == 0 {
        return BrickSignal {
            mean_scalar: 0.0,
            l1_proxy: None,
            l2_proxy: None,
        };
    }

    let mean = (sum / count as f64) as f32;
    // Floored so a dark/far brick's tiny absolute deviations don't blow up the
    // ratio.
    let anchor = mean.abs().max(1e-6);

    // L2 proxy: relative max deviation of the scalar from the brick mean
    // (brick-mean reconstruction residual). Always evaluable when count>0.
    let mut l2_dev = 0.0f32;
    for local in 0..PROBES_PER_CELL {
        if present[local] {
            l2_dev = l2_dev.max((mag[local] - mean).abs());
        }
    }
    let l2_proxy = Some(l2_dev / anchor);

    // L1 proxy: relative max residual of the scalar from a trilinear
    // reconstruction of the 8 corner values. Unevaluable unless every corner is
    // a present probe — the condition under which the oracle's L1 SH
    // reconstruction has a full corner basis.
    let corners = corner_locals();
    let l1_proxy = if corners.iter().all(|&c| present[c]) {
        let mut l1_res = 0.0f32;
        for local in 0..PROBES_PER_CELL {
            if !present[local] {
                continue;
            }
            let target = local_xyz(local);
            let mut recon = 0.0f32;
            let mut wsum = 0.0f32;
            for &corner in &corners {
                let w = trilinear_weight(target, local_xyz(corner));
                if w <= 0.0 {
                    continue;
                }
                recon += mag[corner] * w;
                wsum += w;
            }
            if wsum > 0.0 {
                recon /= wsum;
                l1_res = l1_res.max((mag[local] - recon).abs());
            }
        }
        Some(l1_res / anchor)
    } else {
        None
    };

    BrickSignal {
        mean_scalar: mean,
        l1_proxy,
        l2_proxy,
    }
}

/// P1 wrapper: per-probe scalar is the delivered-light magnitude (falloff + cone,
/// no occlusion). The entry point calls `brick_signal_scalar` with the delivered-
/// magnitude closure directly; this named form exists for the P1 unit tests.
#[cfg(test)]
fn brick_signal(
    lights: &[&MapLight],
    grid_origin: Vec3,
    cell_size: Vec3,
    dims: [u32; 3],
    cx: usize,
    cy: usize,
    cz: usize,
    validity: &[u8],
) -> BrickSignal {
    brick_signal_scalar(
        |_probe, point| Some(delivered_magnitude(lights, point)),
        grid_origin,
        cell_size,
        dims,
        cx,
        cy,
        cz,
        validity,
    )
}

// ---------------------------------------------------------------------------
// Per-probe scalar fields — one per family
// ---------------------------------------------------------------------------

/// Reduce an incident-radiance sum to the scalar the oracle's magnitude anchor
/// uses (max-per-channel, matching `sh_analyze::tile_magnitude`).
fn max_per_channel(v: Vec3) -> f32 {
    v.x.abs().max(v.y.abs()).max(v.z.abs())
}

/// P1: delivered-light magnitude at a world point — sum of every light's incident
/// radiance (color × intensity × falloff × cone), no ray tracing.
fn delivered_magnitude(lights: &[&MapLight], point: Vec3) -> f32 {
    let mut radiance = Vec3::ZERO;
    for light in lights {
        if let Some((r, _l)) = incident_radiance_at_point(light, point) {
            radiance += r;
        }
    }
    max_per_channel(radiance)
}

/// P2: delivered-light magnitude with a per-light shadow test against the bake
/// BVH — a light contributes only when the segment from the probe to the light
/// (or, for a directional light, a long ray toward it) is clear. This captures
/// the shadow boundaries P1 over-brightens.
fn delivered_magnitude_occluded(lights: &[&MapLight], point: Vec3, ctx: &RaytracingCtx<'_>) -> f32 {
    let mut radiance = Vec3::ZERO;
    for light in lights {
        let Some((r, l)) = incident_radiance_at_point(light, point) else {
            continue;
        };
        let visible = match light.light_type {
            LightType::Directional => {
                segment_clear(ctx, point, point + l * DIRECTIONAL_SHADOW_DISTANCE)
            }
            LightType::Point | LightType::Spot => {
                let origin = Vec3::new(
                    light.origin.x as f32,
                    light.origin.y as f32,
                    light.origin.z as f32,
                );
                segment_clear(ctx, point, origin)
            }
        };
        if visible {
            radiance += r;
        }
    }
    max_per_channel(radiance)
}

/// Distance control: distance from the point to the nearest finite-origin light
/// (point/spot). `None` when no such light exists — directional lights have no
/// finite origin and never define a distance field. The invalidated archived
/// classifier keyed on this; the spike reproduces its refutation.
fn nearest_light_distance(lights: &[&MapLight], point: Vec3) -> Option<f32> {
    let mut best = f32::INFINITY;
    for light in lights {
        match light.light_type {
            LightType::Point | LightType::Spot => {
                let origin = Vec3::new(
                    light.origin.x as f32,
                    light.origin.y as f32,
                    light.origin.z as f32,
                );
                best = best.min(point.distance(origin));
            }
            LightType::Directional => {}
        }
    }
    best.is_finite().then_some(best)
}

/// Cached triangle for the surface-distance scan: vertices + AABB, so the
/// per-point loop can cheap-reject with a point→AABB lower bound before the
/// barycentric kernel (mirrors `sdf_bake`'s `nearest_triangle_distance`).
struct Tri {
    a: Vec3,
    b: Vec3,
    c: Vec3,
    aabb_min: Vec3,
    aabb_max: Vec3,
}

/// Collect world triangles from the geometry once, so the surface-distance
/// family scans a flat list rather than re-indexing per point.
fn collect_tris(geometry: &GeometryResult) -> Vec<Tri> {
    let g = &geometry.geometry;
    let mut tris = Vec::with_capacity(g.indices.len() / 3);
    let mut i = 0;
    while i + 3 <= g.indices.len() {
        let i0 = g.indices[i] as usize;
        let i1 = g.indices[i + 1] as usize;
        let i2 = g.indices[i + 2] as usize;
        i += 3;
        let a = Vec3::from(g.vertices[i0].position);
        let b = Vec3::from(g.vertices[i1].position);
        let c = Vec3::from(g.vertices[i2].position);
        tris.push(Tri {
            a,
            b,
            c,
            aabb_min: a.min(b).min(c),
            aabb_max: a.max(b).max(c),
        });
    }
    tris
}

/// Lower bound on the squared point→AABB distance, for the surface-distance
/// cheap-reject. Trivial iteration, not delicate math — the delicate barycentric
/// kernel is the hoisted `sdf_bake::point_triangle_distance_sq`.
fn point_aabb_distance_sq(p: Vec3, mn: Vec3, mx: Vec3) -> f32 {
    let dx = (mn.x - p.x).max(0.0).max(p.x - mx.x);
    let dy = (mn.y - p.y).max(0.0).max(p.y - mx.y);
    let dz = (mn.z - p.z).max(0.0).max(p.z - mx.z);
    dx * dx + dy * dy + dz * dz
}

/// Surface-distance control: distance from the point to the nearest geometry
/// triangle, reusing `sdf_bake`'s tested point-to-triangle kernel. `None` when
/// the map has no triangles.
fn nearest_surface_distance(tris: &[Tri], point: Vec3) -> Option<f32> {
    if tris.is_empty() {
        return None;
    }
    let mut best_sq = f32::INFINITY;
    for tri in tris {
        if point_aabb_distance_sq(point, tri.aabb_min, tri.aabb_max) >= best_sq {
            continue;
        }
        let d_sq = crate::sdf_bake::point_triangle_distance_sq(point, tri.a, tri.b, tri.c);
        if d_sq < best_sq {
            best_sq = d_sq;
        }
    }
    best_sq.is_finite().then(|| best_sq.sqrt())
}

/// P3 ceiling: the baked direct-at-probe magnitude for one dense probe — decode
/// the id-35 direct SH tile and reduce to a mean-per-texel max-per-channel
/// scalar (a probe brightness). `None` when the probe has no direct tile.
fn direct_probe_magnitude(
    section: &postretro_level_format::direct_sh_volume::DirectShVolumeSection,
    probe_index: usize,
    interior: usize,
    border: usize,
) -> Option<f32> {
    let tile = decode_base_direct_tile(section, probe_index, interior, border)?;
    let texels = interior * interior;
    if texels == 0 {
        return None;
    }
    let mut sum = 0.0f64;
    for v in &tile[..texels] {
        sum += max_per_channel(*v) as f64;
    }
    Some((sum / texels as f64) as f32)
}

// ---------------------------------------------------------------------------
// Level thresholding + scoring (shared across all families)
// ---------------------------------------------------------------------------

/// Threshold the predictor proxies to a level, mirroring
/// `sh_analyze::choose_level`: the coarsest level whose proxy is within the
/// threshold. An unevaluable proxy (`None`) never selects that level.
fn predictor_level(l1_proxy: Option<f32>, l2_proxy: Option<f32>, t: f32) -> Level {
    if l2_proxy.is_some_and(|v| v <= t) {
        Level::L2
    } else if l1_proxy.is_some_and(|v| v <= t) {
        Level::L1
    } else {
        Level::L0
    }
}

/// Coarseness rank: higher is coarser (fewer stored probes). Used for the
/// false-positive (predictor coarser than oracle) comparison.
fn coarseness(level: Level) -> u8 {
    level.to_u8()
}

/// The threshold sweep + best operating point + correlation for one family's
/// per-brick predictions against the shared oracle. Identical scoring for every
/// family — only the per-brick proxies differ.
fn score_family(
    predictions: &[BrickPrediction],
    thresholds: &[f32],
) -> (Vec<SweepRow>, Option<usize>, f32) {
    let nonempty = predictions.len() as u64;
    let mut sweep = Vec::with_capacity(thresholds.len());
    for &t in thresholds {
        let mut confusion = [[0u64; 3]; 3];
        let mut fp = 0u64;
        let mut agree = 0u64;
        let mut oracle_coarsenable = 0u64;
        let mut recovered_safe = 0u64;
        for p in predictions {
            let oracle = Level::from_u8(p.oracle_level).unwrap_or(Level::L0);
            let pred = predictor_level(p.l1_proxy, p.l2_proxy, t);
            confusion[coarseness(pred) as usize][coarseness(oracle) as usize] += 1;
            if pred == oracle {
                agree += 1;
            }
            if coarseness(pred) > coarseness(oracle) {
                fp += 1;
            }
            if oracle != Level::L0 {
                oracle_coarsenable += 1;
                if pred != Level::L0 && coarseness(pred) <= coarseness(oracle) {
                    recovered_safe += 1;
                }
            }
        }
        sweep.push(SweepRow {
            threshold: t,
            confusion,
            false_positive_bricks: fp,
            false_positive_rate: ratio(fp, nonempty),
            agreement_bricks: agree,
            agreement_rate: ratio(agree, nonempty),
            oracle_coarsenable,
            recovered_safe,
            recovered_savings_fraction: ratio(recovered_safe, oracle_coarsenable),
        });
    }
    let best = best_operating_point(&sweep);
    let corr = score_oracle_correlation(predictions);
    (sweep, best, corr)
}

/// Index of the sweep row that maximizes recovered savings among zero-FP rows;
/// falls back to the minimum-FP row (ties broken by higher recovered savings)
/// when no zero-FP row exists.
fn best_operating_point(sweep: &[SweepRow]) -> Option<usize> {
    if sweep.is_empty() {
        return None;
    }
    let zero_fp = sweep
        .iter()
        .enumerate()
        .filter(|(_, r)| r.false_positive_bricks == 0)
        .max_by(|(_, a), (_, b)| {
            a.recovered_savings_fraction
                .partial_cmp(&b.recovered_savings_fraction)
                .unwrap_or(std::cmp::Ordering::Equal)
        })
        .map(|(i, _)| i);
    zero_fp.or_else(|| {
        sweep
            .iter()
            .enumerate()
            .min_by(|(_, a), (_, b)| {
                a.false_positive_bricks.cmp(&b.false_positive_bricks).then(
                    b.recovered_savings_fraction
                        .partial_cmp(&a.recovered_savings_fraction)
                        .unwrap_or(std::cmp::Ordering::Equal),
                )
            })
            .map(|(i, _)| i)
    })
}

/// Pearson correlation of the continuous predictor score with oracle coarseness
/// over bricks whose score is finite (evaluable). Zero when either side has no
/// variance — the honest reading of "no linear signal".
fn score_oracle_correlation(predictions: &[BrickPrediction]) -> f32 {
    let pairs: Vec<(f32, f32)> = predictions
        .iter()
        .filter(|p| p.score.is_finite())
        .map(|p| (p.score, p.oracle_level as f32))
        .collect();
    let n = pairs.len();
    if n < 2 {
        return 0.0;
    }
    let n_f = n as f32;
    let mean_x = pairs.iter().map(|(x, _)| *x).sum::<f32>() / n_f;
    let mean_y = pairs.iter().map(|(_, y)| *y).sum::<f32>() / n_f;
    let mut sxy = 0.0f32;
    let mut sxx = 0.0f32;
    let mut syy = 0.0f32;
    for (x, y) in &pairs {
        let dx = x - mean_x;
        let dy = y - mean_y;
        sxy += dx * dy;
        sxx += dx * dx;
        syy += dy * dy;
    }
    let denom = (sxx * syy).sqrt();
    if denom <= 0.0 { 0.0 } else { sxy / denom }
}

fn ratio(n: u64, d: u64) -> f32 {
    if d == 0 { 0.0 } else { n as f32 / d as f32 }
}

// ---------------------------------------------------------------------------
// Entry point
// ---------------------------------------------------------------------------

/// Evaluate one family: run its per-probe scalar over every non-empty brick,
/// attach the shared oracle level, and time the loop. Returns the per-brick
/// predictions and the evaluation wall-time.
fn evaluate_family<F: Fn(usize, Vec3) -> Option<f32>>(
    scalar: F,
    bricks: &[BrickRecord],
    oracle_levels: &[Level],
    grid_origin: Vec3,
    cell_size: Vec3,
    dims: [u32; 3],
    validity: &[u8],
) -> (Vec<BrickPrediction>, f64) {
    let start = std::time::Instant::now();
    let mut predictions = Vec::with_capacity(bricks.len());
    for (record, &oracle) in bricks.iter().zip(oracle_levels.iter()) {
        let [cx, cy, cz] = record.cell;
        let signal = brick_signal_scalar(
            &scalar,
            grid_origin,
            cell_size,
            dims,
            cx as usize,
            cy as usize,
            cz as usize,
            validity,
        );
        let score = signal.l2_proxy.unwrap_or(f32::INFINITY);
        predictions.push(BrickPrediction {
            cell: record.cell,
            linear_cell: record.linear_cell,
            valid_probes: record.valid_probes,
            mean_scalar: signal.mean_scalar,
            score,
            l1_proxy: signal.l1_proxy,
            l2_proxy: signal.l2_proxy,
            oracle_level: coarseness(oracle),
        });
    }
    (predictions, start.elapsed().as_secs_f64())
}

/// Assemble a `FamilyReport` from a family's predictions.
fn build_family(
    name: &str,
    description: &str,
    cost_note: &str,
    predictions: Vec<BrickPrediction>,
    eval_seconds: f64,
    thresholds: &[f32],
) -> FamilyReport {
    let (sweep, best, corr) = score_family(&predictions, thresholds);
    FamilyReport {
        name: name.to_string(),
        description: description.to_string(),
        score_vs_oracle_correlation: corr,
        predictor_eval_seconds: eval_seconds,
        best_operating_point: best,
        sweep,
        cost_note: cost_note.to_string(),
        bricks: predictions,
    }
}

pub fn run_forward_predict(inputs: &ForwardPredictInputs<'_>) -> ForwardPredictReport {
    let analysis = run_analysis(&inputs.analyze);
    let params = CoarsenParams::default();

    // Measured, not assumed: id 35 (base direct / DirectShBake) bakes AFTER id 34
    // (base indirect / ShBake) in the compiler stage order — see
    // pipeline.rs::STAGE_ORDER (ShBake → DeltaShBake → DirectShBake). The direct
    // field is therefore NOT available before the base-indirect bake P3 would
    // predict; P3 is an accuracy ceiling only, never a cheap forward predictor.
    let p3_ordering_note =
        "id 35 (base_direct/DirectShBake) bakes AFTER id 34 (base_indirect/ShBake) in \
         pipeline STAGE_ORDER; the direct-at-probe field is not available pre-base-indirect-bake, \
         so P3 is an accuracy ceiling only, not a viable cheap predictor"
            .to_string();

    let mut report = ForwardPredictReport {
        grid_dims: analysis.grid_dims,
        cell_size: analysis.cell_size,
        brick_count: analysis.brick_count,
        nonempty_bricks: analysis.nonempty_bricks,
        has_base_direct: analysis.has_base_direct,
        has_delta_indirect: analysis.has_delta_indirect,
        light_count: inputs.lights.len() as u64,
        animated_light_count: inputs.animated_light_count as u64,
        rel_p95_max: params.rel_p95_max,
        rel_max_max: params.rel_max_max,
        darkness_frac: params.darkness_frac,
        p3_direct_baked_before_base_indirect: false,
        p3_ordering_note,
        ..Default::default()
    };

    let map_p95 = map_p95_magnitude(&analysis.bricks);
    let floor = (params.darkness_frac * map_p95).max(1e-6);
    report.map_p95 = map_p95;
    report.darkness_floor = floor;

    // S3: no non-empty bricks (no lights / empty / all-invalid) → a well-formed
    // report with each family carrying a zeroed sweep, never a panic.
    if analysis.bricks.is_empty() {
        let empty_sweep: Vec<SweepRow> = inputs
            .predictor_thresholds
            .iter()
            .map(|&t| SweepRow {
                threshold: t,
                ..Default::default()
            })
            .collect();
        report.sweep = empty_sweep.clone();
        for name in ["P1", "P2", "distance", "surface_distance", "P3"] {
            report.families.insert(
                name.to_string(),
                FamilyReport {
                    name: name.to_string(),
                    sweep: empty_sweep.clone(),
                    ..Default::default()
                },
            );
        }
        return report;
    }

    let grid_origin = Vec3::from(inputs.analyze.grid_origin);
    let cell_size = Vec3::from(inputs.analyze.cell_size);
    let dims = inputs.analyze.grid_dims;
    let validity = inputs.analyze.validity;
    let thresholds = inputs.predictor_thresholds;

    // Shared oracle: computed ONCE, reused by every family (S2 — one production
    // classification of the base-indirect field for all families to score
    // against).
    let oracle_levels: Vec<Level> = analysis
        .bricks
        .iter()
        .map(|r| oracle_level(r, floor, &params))
        .collect();
    for &level in &oracle_levels {
        report.oracle_histogram.bump(level);
    }

    let ray_ctx = RaytracingCtx {
        bvh: inputs.bvh,
        primitives: inputs.primitives,
        geometry: inputs.geometry,
    };
    let lights = inputs.lights;

    // --- P1: contribution geometry, no occlusion (the Task 1 candidate). ---
    let (p1_pred, p1_secs) = evaluate_family(
        |_probe, point| Some(delivered_magnitude(lights, point)),
        &analysis.bricks,
        &oracle_levels,
        grid_origin,
        cell_size,
        dims,
        validity,
    );
    let p1 = build_family(
        "P1",
        "contribution geometry (per-light falloff + cone) at the brick probes, no ray tracing",
        "",
        p1_pred,
        p1_secs,
        thresholds,
    );

    // --- P2: P1 + per-light segment-clear occlusion against the bake BVH. ---
    let (p2_pred, p2_secs) = evaluate_family(
        |_probe, point| Some(delivered_magnitude_occluded(lights, point, &ray_ctx)),
        &analysis.bricks,
        &oracle_levels,
        grid_origin,
        cell_size,
        dims,
        validity,
    );
    let p2 = build_family(
        "P2",
        "P1 plus a per-light corner shadow test (segment-clear against the bake BVH)",
        "",
        p2_pred,
        p2_secs,
        thresholds,
    );

    // --- distance control: nearest-light distance (invalidated classifier). ---
    let (dist_pred, dist_secs) = evaluate_family(
        |_probe, point| nearest_light_distance(lights, point),
        &analysis.bricks,
        &oracle_levels,
        grid_origin,
        cell_size,
        dims,
        validity,
    );
    let distance = build_family(
        "distance",
        "nearest point/spot light distance (negative control — the invalidated archived classifier)",
        "negative control: reproduced to confirm the distance signal's refutation, never a candidate",
        dist_pred,
        dist_secs,
        thresholds,
    );

    // --- surface-distance control: nearest geometry triangle distance. ---
    let tris = collect_tris(inputs.geometry);
    let (surf_pred, surf_secs) = evaluate_family(
        |_probe, point| nearest_surface_distance(&tris, point),
        &analysis.bricks,
        &oracle_levels,
        grid_origin,
        cell_size,
        dims,
        validity,
    );
    let surface_distance = build_family(
        "surface_distance",
        "nearest geometry-surface distance (negative control; sdf_bake point-to-triangle kernel)",
        "negative control: shares distance's cone-blindness, never a candidate",
        surf_pred,
        surf_secs,
        thresholds,
    );

    // --- P3 ceiling: baked direct-at-probe field gradient (id 35). ---
    let base = inputs.analyze.base_indirect;
    let interior = (base.tile_dimension as usize).saturating_sub(2 * base.tile_border as usize);
    let border = base.tile_border as usize;
    let base_direct = inputs.analyze.base_direct;
    let (p3_pred, p3_secs) = evaluate_family(
        |probe, _point| {
            base_direct.and_then(|section| direct_probe_magnitude(section, probe, interior, border))
        },
        &analysis.bricks,
        &oracle_levels,
        grid_origin,
        cell_size,
        dims,
        validity,
    );
    let p3 = build_family(
        "P3",
        "baked direct-at-probe field (id 35) per-probe gradient across the brick — richest signal",
        "ACCURACY CEILING ONLY: id 35 bakes AFTER id 34 (measured), so it is not a pre-bake input; \
         reported to bound achievable accuracy, never as a cheap predictor",
        p3_pred,
        p3_secs,
        thresholds,
    );

    // Top-level P1 surface preserved for Task 1 continuity.
    report.score_vs_oracle_correlation = p1.score_vs_oracle_correlation;
    report.best_operating_point = p1.best_operating_point;
    report.sweep = p1.sweep.clone();
    report.predictor_eval_seconds = p1.predictor_eval_seconds;
    report.bricks = p1.bricks.clone();

    report.families.insert("P1".to_string(), p1);
    report.families.insert("P2".to_string(), p2);
    report.families.insert("distance".to_string(), distance);
    report
        .families
        .insert("surface_distance".to_string(), surface_distance);
    report.families.insert("P3".to_string(), p3);

    report
}

// ---------------------------------------------------------------------------
// Emit
// ---------------------------------------------------------------------------

pub fn write_json(report: &ForwardPredictReport, path: &Path) -> anyhow::Result<()> {
    let json = serde_json::to_vec_pretty(report)?;
    std::fs::write(path, json)?;
    Ok(())
}

pub fn log_summary(report: &ForwardPredictReport) {
    log::info!(
        "[sh-forward-predict] grid {}x{}x{}, {} bricks ({} non-empty); lights {} (animated {}); \
         base_direct={} delta_indirect={}",
        report.grid_dims[0],
        report.grid_dims[1],
        report.grid_dims[2],
        report.brick_count,
        report.nonempty_bricks,
        report.light_count,
        report.animated_light_count,
        report.has_base_direct,
        report.has_delta_indirect,
    );
    log::info!(
        "[sh-forward-predict] oracle levels: L0 {} L1 {} L2 {} (map-p95 {:.6}, floor {:.6}, gate p95<={:.2} max<={:.2})",
        report.oracle_histogram.l0,
        report.oracle_histogram.l1,
        report.oracle_histogram.l2,
        report.map_p95,
        report.darkness_floor,
        report.rel_p95_max,
        report.rel_max_max,
    );
    log::info!(
        "[sh-forward-predict] P3 direct-field ordering (measured): baked_before_base_indirect={} — {}",
        report.p3_direct_baked_before_base_indirect,
        report.p3_ordering_note,
    );
    for (name, fam) in &report.families {
        let best = fam.best_operating_point.map(|i| &fam.sweep[i]);
        log::info!(
            "[sh-forward-predict] family {name}: eval {:.4}s | score↔oracle r={:.3} | best t={:.3} FP {} ({:.4}) recovered {:.4} agree {:.4}{}",
            fam.predictor_eval_seconds,
            fam.score_vs_oracle_correlation,
            best.map(|r| r.threshold).unwrap_or(f32::NAN),
            best.map(|r| r.false_positive_bricks).unwrap_or(0),
            best.map(|r| r.false_positive_rate).unwrap_or(0.0),
            best.map(|r| r.recovered_savings_fraction).unwrap_or(0.0),
            best.map(|r| r.agreement_rate).unwrap_or(0.0),
            if fam.cost_note.is_empty() {
                String::new()
            } else {
                format!(" | note: {}", fam.cost_note)
            },
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bvh_build::build_bvh;
    use crate::geometry::GeometryResult;
    use crate::map_data::{FalloffModel, LightType, MapLight, ShadowType};
    use glam::DVec3;
    use postretro_level_format::geometry::{FaceMeta, GeometrySection, Vertex};
    use postretro_level_format::sh_volume::OctahedralShVolumeSection;
    use postretro_level_format::texture_names::TextureNamesSection;

    use crate::geometry::FaceIndexRange;

    fn point_light(origin: DVec3, intensity: f32, range: f32) -> MapLight {
        MapLight {
            origin,
            carrier: String::new(),
            light_type: LightType::Point,
            intensity,
            color: [1.0, 1.0, 1.0],
            falloff_model: FalloffModel::Linear,
            falloff_range: range,
            light_size: 0.0,
            angular_diameter: 0.0,
            cone_angle_inner: None,
            cone_angle_outer: None,
            cone_direction: None,
            animation: None,
            bake_only: false,
            is_dynamic: false,
            casts_entity_shadows: false,
            is_animated: false,
            tags: vec![],
            shadow_type: ShadowType::StaticLightMap,
        }
    }

    fn directional_light(dir: [f32; 3], intensity: f32) -> MapLight {
        let mut l = point_light(DVec3::ZERO, intensity, 20.0);
        l.light_type = LightType::Directional;
        l.cone_direction = Some(dir);
        l
    }

    fn brick_record(mag_p95: f32, mag_max: f32) -> BrickRecord {
        let mut r = BrickRecord {
            valid_probes: 64,
            ..Default::default()
        };
        r.composed_magnitude.p95 = mag_p95;
        r.composed_magnitude.max = mag_max;
        r
    }

    fn tri_vertex(p: [f32; 3]) -> Vertex {
        Vertex::new(
            p,
            [0.0, 0.0],
            [0.0, 1.0, 0.0],
            [1.0, 0.0, 0.0],
            true,
            [0.0, 0.0],
            0,
        )
    }

    /// One-triangle geometry with a buildable BVH, for the occlusion primitive
    /// test. The triangle spans a plane the caller positions between a light and
    /// a probe.
    fn triangle_geometry(tris: &[[[f32; 3]; 3]]) -> GeometryResult {
        let mut vertices = Vec::new();
        let mut indices = Vec::new();
        let mut faces = Vec::new();
        let mut face_index_ranges = Vec::new();
        for (i, tri) in tris.iter().enumerate() {
            let base = (i * 3) as u32;
            for &p in tri {
                vertices.push(tri_vertex(p));
            }
            indices.extend_from_slice(&[base, base + 1, base + 2]);
            faces.push(FaceMeta {
                leaf_index: 0,
                texture_index: 0,
            });
            face_index_ranges.push(FaceIndexRange {
                index_offset: base,
                index_count: 3,
            });
        }
        GeometryResult {
            geometry: GeometrySection {
                vertices,
                indices,
                faces,
            },
            texture_names: TextureNamesSection { names: Vec::new() },
            face_index_ranges,
        }
    }

    fn empty_geometry() -> GeometryResult {
        GeometryResult {
            geometry: GeometrySection {
                vertices: Vec::new(),
                indices: Vec::new(),
                faces: Vec::new(),
            },
            texture_names: TextureNamesSection { names: Vec::new() },
            face_index_ranges: Vec::new(),
        }
    }

    /// Empty base-indirect section (`grid_dimensions == [0,0,0]`) so
    /// `run_analysis` early-returns an empty report — the S3 no-bricks path.
    fn empty_base_section() -> OctahedralShVolumeSection {
        OctahedralShVolumeSection {
            grid_origin: [0.0, 0.0, 0.0],
            cell_size: [1.0, 1.0, 1.0],
            grid_dimensions: [0, 0, 0],
            probe_stride: 27,
            tile_dimension: 6,
            tile_border: 1,
            atlas_dimensions: [0, 0],
            layer_count: 0,
            tiles_per_layer: 0,
            atlas_tiles_per_row: 0,
            probes: Vec::new(),
            compact_atlas_dimensions: [0, 0],
            compact_atlas_tiles_per_row: 0,
            compact_atlas_tiles_per_layer: 0,
            compact_atlas_layer_count: 0,
            irradiance_format: 0,
            compact_atlas: Vec::new(),
            animation_descriptors: Vec::new(),
            slot_for_map_light: Vec::new(),
        }
    }

    #[test]
    fn predictor_level_picks_coarsest_proxy_within_threshold() {
        assert_eq!(predictor_level(Some(0.02), Some(0.05), 0.10), Level::L2);
        assert_eq!(predictor_level(Some(0.02), Some(0.20), 0.10), Level::L1);
        assert_eq!(predictor_level(Some(0.30), Some(0.40), 0.10), Level::L0);
        assert_eq!(predictor_level(None, None, 1.0), Level::L0);
        assert_eq!(predictor_level(None, Some(0.01), 0.10), Level::L2);
    }

    #[test]
    fn oracle_admits_coarsest_level_passing_the_relative_gate() {
        let params = CoarsenParams::default();
        let floor = 0.1;

        let mut r = brick_record(1.0, 1.0);
        r.base_l2 = evaluable(0.01, 0.02);
        r.base_l1 = evaluable(0.05, 0.10);
        assert_eq!(oracle_level(&r, floor, &params), Level::L2);

        let mut r = brick_record(1.0, 1.0);
        r.base_l2 = evaluable(0.50, 0.10);
        r.base_l1 = evaluable(0.05, 0.10);
        assert_eq!(oracle_level(&r, floor, &params), Level::L1);

        let mut r = brick_record(1.0, 1.0);
        r.base_l2 = evaluable(0.50, 0.60);
        r.base_l1 = evaluable(0.40, 0.55);
        assert_eq!(oracle_level(&r, floor, &params), Level::L0);
    }

    #[test]
    fn oracle_darkness_bypass_takes_coarsest_evaluable_level() {
        let params = CoarsenParams::default();
        let floor = 0.1;
        let mut r = brick_record(0.001, 0.001);
        r.base_l2 = evaluable(9.0, 9.0);
        r.base_l1 = evaluable(9.0, 9.0);
        assert_eq!(oracle_level(&r, floor, &params), Level::L2);
    }

    #[test]
    fn oracle_treats_unevaluable_level_as_ineligible() {
        let params = CoarsenParams::default();
        let floor = 0.1;
        let mut r = brick_record(1.0, 1.0);
        r.base_l2 = LevelErrStatsProxy::unevaluable();
        r.base_l1 = evaluable(0.05, 0.10);
        assert_eq!(oracle_level(&r, floor, &params), Level::L1);
    }

    #[test]
    fn predictor_scores_uniform_light_far_below_concentrated_light() {
        let dims = [AF as u32, AF as u32, AF as u32];
        let validity = vec![1u8; PROBES_PER_CELL];
        let origin = Vec3::ZERO;
        let cs = Vec3::ONE;

        let dir = directional_light([0.0, -1.0, 0.0], 1.0);
        let uniform = brick_signal(&[&dir], origin, cs, dims, 0, 0, 0, &validity);
        assert!(uniform.l2_proxy.unwrap() < 1e-5, "directional light is uniform");
        assert!(uniform.l1_proxy.unwrap() < 1e-5);
        assert!(uniform.mean_scalar > 0.0);

        let pt = point_light(DVec3::new(0.0, 0.0, 0.0), 1.0, 6.0);
        let concentrated = brick_signal(&[&pt], origin, cs, dims, 0, 0, 0, &validity);
        assert!(
            concentrated.l2_proxy.unwrap() > uniform.l2_proxy.unwrap() + 0.1,
            "concentrated light varies far more than uniform: {} vs {}",
            concentrated.l2_proxy.unwrap(),
            uniform.l2_proxy.unwrap()
        );
    }

    #[test]
    fn brick_signal_with_no_lights_is_well_formed_not_a_panic() {
        let dims = [AF as u32, AF as u32, AF as u32];
        let validity = vec![1u8; PROBES_PER_CELL];
        let signal = brick_signal(&[], Vec3::ZERO, Vec3::ONE, dims, 0, 0, 0, &validity);
        assert_eq!(signal.mean_scalar, 0.0);
        assert_eq!(signal.l2_proxy, Some(0.0));
        assert_eq!(signal.l1_proxy, Some(0.0));
    }

    #[test]
    fn brick_signal_all_invalid_probes_yields_unevaluable_proxies() {
        let dims = [AF as u32, AF as u32, AF as u32];
        let validity = vec![0u8; PROBES_PER_CELL];
        let dir = directional_light([0.0, -1.0, 0.0], 1.0);
        let signal = brick_signal(&[&dir], Vec3::ZERO, Vec3::ONE, dims, 0, 0, 0, &validity);
        assert_eq!(signal.mean_scalar, 0.0);
        assert_eq!(signal.l2_proxy, None);
        assert_eq!(signal.l1_proxy, None);
    }

    #[test]
    fn occlusion_zeros_a_light_blocked_by_geometry() {
        // A big occluder quad at y = 2, spanning the whole brick footprint in x/z,
        // with a light above it and the probe below. The light-to-probe segment
        // crosses the quad → occluded delivered magnitude is zero; the clear scene
        // keeps the light's contribution.
        let quad = [
            [[-100.0, 2.0, -100.0], [100.0, 2.0, -100.0], [100.0, 2.0, 100.0]],
            [[-100.0, 2.0, -100.0], [100.0, 2.0, 100.0], [-100.0, 2.0, 100.0]],
        ];
        let geo = triangle_geometry(&quad);
        let (bvh, prims, _) = build_bvh(&geo).expect("quad geometry builds a BVH");
        let ctx = RaytracingCtx {
            bvh: &bvh,
            primitives: &prims,
            geometry: &geo,
        };
        let light = point_light(DVec3::new(0.0, 5.0, 0.0), 1.0, 50.0);
        let probe = Vec3::new(0.0, 0.0, 0.0); // below the occluder

        let clear = delivered_magnitude(&[&light], probe);
        assert!(clear > 0.0, "light reaches the probe with no occlusion test");
        let occluded = delivered_magnitude_occluded(&[&light], probe, &ctx);
        assert_eq!(occluded, 0.0, "the occluder between light and probe zeros P2");

        // A probe on the light's side of the occluder stays lit under P2.
        let lit_probe = Vec3::new(0.0, 3.5, 0.0);
        let lit = delivered_magnitude_occluded(&[&light], lit_probe, &ctx);
        assert!(lit > 0.0, "an unobstructed probe keeps its contribution");
    }

    #[test]
    fn nearest_light_distance_none_without_finite_origin_lights() {
        // S3 degenerate: only directional lights (no finite origin) → no distance
        // field, so the scalar is None (unevaluable), not a panic.
        let dir = directional_light([0.0, -1.0, 0.0], 1.0);
        assert_eq!(nearest_light_distance(&[&dir], Vec3::ZERO), None);
        assert_eq!(nearest_light_distance(&[], Vec3::ZERO), None);
        // A point light yields its distance.
        let pt = point_light(DVec3::new(3.0, 0.0, 4.0), 1.0, 50.0);
        assert_eq!(nearest_light_distance(&[&pt], Vec3::ZERO), Some(5.0));
    }

    #[test]
    fn nearest_surface_distance_none_without_geometry() {
        // S3 degenerate: empty triangle list → None (unevaluable), not a panic.
        assert_eq!(nearest_surface_distance(&[], Vec3::ZERO), None);
        // A single triangle in the y=2 plane: nearest distance from origin is 2.
        let quad = [[[-1.0, 2.0, -1.0], [1.0, 2.0, -1.0], [0.0, 2.0, 1.0]]];
        let geo = triangle_geometry(&quad);
        let tris = collect_tris(&geo);
        let d = nearest_surface_distance(&tris, Vec3::ZERO).expect("triangle present");
        assert!((d - 2.0).abs() < 1e-4, "expected 2.0, got {d}");
    }

    #[test]
    fn distance_family_brick_signal_is_well_formed() {
        // S3: the distance scalar over a full brick produces evaluable proxies.
        let dims = [AF as u32, AF as u32, AF as u32];
        let validity = vec![1u8; PROBES_PER_CELL];
        let pt = point_light(DVec3::new(0.0, 0.0, 0.0), 1.0, 50.0);
        let lights: [&MapLight; 1] = [&pt];
        let signal = brick_signal_scalar(
            |_probe, point| nearest_light_distance(&lights, point),
            Vec3::ZERO,
            Vec3::ONE,
            dims,
            0,
            0,
            0,
            &validity,
        );
        assert!(signal.mean_scalar > 0.0);
        assert!(signal.l2_proxy.is_some());
        assert!(signal.l1_proxy.is_some());
    }

    #[test]
    fn p3_scalar_none_without_direct_section() {
        // S3: no base-direct section → P3 scalar is None everywhere, well-formed.
        let dims = [AF as u32, AF as u32, AF as u32];
        let validity = vec![1u8; PROBES_PER_CELL];
        let base_direct: Option<&postretro_level_format::direct_sh_volume::DirectShVolumeSection> =
            None;
        let signal = brick_signal_scalar(
            |probe, _point| {
                base_direct.and_then(|s| direct_probe_magnitude(s, probe, 4, 1))
            },
            Vec3::ZERO,
            Vec3::ONE,
            dims,
            0,
            0,
            0,
            &validity,
        );
        assert_eq!(signal.mean_scalar, 0.0);
        assert_eq!(signal.l2_proxy, None);
        assert_eq!(signal.l1_proxy, None);
    }

    #[test]
    fn run_forward_predict_on_empty_grid_yields_well_formed_families() {
        // S3: no baked lights / empty grid → run_analysis returns no bricks; the
        // harness must produce a well-formed (empty) sweep for EVERY family, never
        // a panic.
        let base = empty_base_section();
        let validity: Vec<u8> = Vec::new();
        let protect = Vec::new();
        let thresholds = DEFAULT_PREDICTOR_THRESHOLDS;
        let geo = empty_geometry();
        let (bvh, prims, _) = build_bvh(&geo).expect("empty geometry builds an empty BVH");
        let analyze = AnalyzeInputs {
            grid_origin: base.grid_origin,
            cell_size: base.cell_size,
            grid_dims: base.grid_dimensions,
            validity: &validity,
            base_indirect: &base,
            base_direct: None,
            delta_indirect: None,
            delta_direct: None,
            delta_anim_direct: None,
            protect_aabbs: &protect,
            thresholds: &crate::sh_analyze::DEFAULT_THRESHOLDS,
        };
        let inputs = ForwardPredictInputs {
            analyze,
            lights: &[],
            animated_light_count: 0,
            predictor_thresholds: &thresholds,
            bvh: &bvh,
            primitives: &prims,
            geometry: &geo,
        };
        let report = run_forward_predict(&inputs);
        assert_eq!(report.nonempty_bricks, 0);
        assert!(report.bricks.is_empty());
        assert_eq!(report.sweep.len(), thresholds.len());
        // Every family present with a well-formed empty sweep.
        for name in ["P1", "P2", "distance", "surface_distance", "P3"] {
            let fam = report.families.get(name).expect("family present");
            assert_eq!(fam.sweep.len(), thresholds.len());
            for row in &fam.sweep {
                assert_eq!(row.false_positive_bricks, 0);
                assert_eq!(row.oracle_coarsenable, 0);
                assert_eq!(row.confusion, [[0u64; 3]; 3]);
            }
        }
        assert!(!report.p3_direct_baked_before_base_indirect);
        assert!(serde_json::to_vec(&report).is_ok());
    }

    #[test]
    fn correlation_is_zero_without_variance() {
        assert_eq!(score_oracle_correlation(&[]), 0.0);
        let one = vec![BrickPrediction {
            score: 0.1,
            oracle_level: 2,
            ..Default::default()
        }];
        assert_eq!(score_oracle_correlation(&one), 0.0);
    }

    // --- test helpers ---
    use crate::sh_analyze::LevelErrStats;
    fn evaluable(p95: f32, max: f32) -> LevelErrStats {
        LevelErrStats {
            max,
            mean: max,
            p95,
            weighted_mean: max,
            weighted_p95: p95,
            texel_samples: 16,
            evaluable: true,
        }
    }
    struct LevelErrStatsProxy;
    impl LevelErrStatsProxy {
        fn unevaluable() -> LevelErrStats {
            LevelErrStats {
                evaluable: false,
                texel_samples: 0,
                ..Default::default()
            }
        }
    }
}
