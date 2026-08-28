//! Output-preserving base-density forward-predictor efficacy harness (spike,
//! measurement only). Task 1 thin slice: the P1 contribution-geometry predictor
//! on one fixture.
//! See: context/plans/in-progress/lighting-scale--base-density-forward-predictor-spike/index.md
//!
//! Question the harness exists to answer: can a cheap forward signal — evaluable
//! from light geometry and probe positions BEFORE the base-indirect SH bake —
//! reproduce the post-bake composed-receiver coarsenability oracle accurately
//! enough (near-zero unsafe false-positives) to justify a deferred base-probe-
//! density spec? This module computes the oracle (reusing `sh_analyze`'s brick
//! primitives + `sh_coarsen::CoarsenParams`, so its numbers are the production
//! gate the foundational spec would ship), the P1 predictor (per-brick delivered-
//! light magnitude and its spatial variation across the brick), and scores the
//! predictor against the oracle over a threshold sweep — the false-positive-rate
//! vs recovered-savings tradeoff curve.
//!
//! Byte-preserving: it reads the finalized owned sections and the pre-bake light
//! set, mutates nothing that reaches the packer, and writes only its own JSON.
//! Measure-and-report only — no accuracy/cost threshold is contracted in code.

use std::path::Path;

use glam::Vec3;
use postretro_level_format::sh_reconstruct::{Level, corner_locals, local_xyz, trilinear_weight};
use serde::Serialize;

use crate::affinity_grid::AFFINITY_FACTOR;
use crate::map_data::MapLight;
use crate::sh_analyze::{AnalyzeInputs, BrickRecord, run_analysis};
use crate::sh_bake::incident_radiance_at_point;
use crate::sh_coarsen::CoarsenParams;

const AF: usize = AFFINITY_FACTOR as usize; // 4
const PROBES_PER_CELL: usize = AF * AF * AF; // 64

/// Predictor decision thresholds swept for the tradeoff curve. The predictor's
/// per-brick proxies are RELATIVE (delivered-light deviation normalized by the
/// brick's mean delivered magnitude), so these are dimensionless relative-error
/// cutoffs bracketing near-uniform (0.01) up past the oracle's own relative
/// gate band (`rel_max_max` 0.25). Mirrors `sh_analyze::DEFAULT_THRESHOLDS`'
/// swept-cutoff style; a debug-tool knob, never a user-facing setting.
pub const DEFAULT_PREDICTOR_THRESHOLDS: [f32; 11] = [
    0.01, 0.02, 0.03, 0.05, 0.075, 0.10, 0.15, 0.20, 0.25, 0.35, 0.50,
];

// ---------------------------------------------------------------------------
// Inputs
// ---------------------------------------------------------------------------

/// Everything the harness needs, routed from the pipeline at the post-section-
/// finalization seam. `analyze` is the same `AnalyzeInputs` the `--sh-analyze`
/// pass builds — reused verbatim so the oracle is numerically the production
/// classification of the base-indirect field.
pub struct ForwardPredictInputs<'a> {
    pub analyze: AnalyzeInputs<'a>,
    /// The static (baked) light set feeding the base-indirect SH bake — the
    /// field the predictor forecasts. In base-indirect bake order.
    pub lights: &'a [&'a MapLight],
    /// Count of animated baked lights (id 27 source). Recorded for the fixture-
    /// honesty precondition (delta-indirect + aimed spots), not used by scoring.
    pub animated_light_count: usize,
    pub predictor_thresholds: &'a [f32],
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

/// One brick's predictor signals and both classifications. `linear_cell` keys
/// back to the `sh_analyze` `BrickRecord`.
#[derive(Serialize, Clone, Default)]
pub struct BrickPrediction {
    pub cell: [u32; 3],
    pub linear_cell: u32,
    pub valid_probes: u32,
    /// Mean delivered-light magnitude (max-per-channel radiance) over the
    /// brick's valid probes — the predictor's brightness anchor.
    pub mean_delivered: f32,
    /// Primary continuous coarsenability score: relative brick-mean deviation of
    /// delivered magnitude (the L2 proxy). Lower = more uniform = more
    /// coarsenable. `f32::INFINITY` maps to this being unevaluable.
    pub score: f32,
    /// L1 (trilinear-corner) relative residual proxy; `None` when the brick's 8
    /// corners are not all valid (L1 unevaluable, mirroring the oracle).
    pub l1_proxy: Option<f32>,
    /// L2 (brick-mean) relative deviation proxy; `None` when unevaluable.
    pub l2_proxy: Option<f32>,
    /// Oracle level: coarsest base-indirect level the production relative gate
    /// admits (0/1/2).
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
    /// Pearson correlation of the continuous predictor score with the oracle
    /// coarseness level over evaluable bricks (contribution-aware signal
    /// strength). NaN-guarded to 0.0 when variance is zero.
    pub score_vs_oracle_correlation: f32,
    /// Best near-zero-FP operating point: index into `sweep` maximizing recovered
    /// savings subject to zero false positives; falls back to the minimum-FP row
    /// when no zero-FP row exists. `None` when there are no non-empty bricks.
    pub best_operating_point: Option<usize>,
    pub sweep: Vec<SweepRow>,
    /// Wall-time of the predictor evaluation (excludes the oracle's `run_analysis`
    /// decode) — the cost side of "cheap enough to save bake time".
    pub predictor_eval_seconds: f64,
    pub bricks: Vec<BrickPrediction>,
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
// P1 predictor — contribution geometry, no ray tracing
// ---------------------------------------------------------------------------

/// The predictor's per-brick signals. Each proxy is `None` when unevaluable
/// (the level's reconstruction basis is absent — e.g. L1 with a non-valid
/// corner), mirroring the oracle's per-level ineligibility.
struct BrickSignal {
    mean_delivered: f32,
    l1_proxy: Option<f32>,
    l2_proxy: Option<f32>,
}

/// Delivered-light magnitude at a world point: sum of every light's incident
/// radiance (color × intensity × falloff × cone), reduced to a scalar by the
/// same max-per-channel reduction `sh_analyze::tile_magnitude` applies, so the
/// predictor's brightness units line up with the oracle's magnitude anchor.
fn delivered_magnitude(lights: &[&MapLight], point: Vec3) -> f32 {
    let mut radiance = Vec3::ZERO;
    for light in lights {
        if let Some((r, _l)) = incident_radiance_at_point(light, point) {
            radiance += r;
        }
    }
    radiance.x.abs().max(radiance.y.abs()).max(radiance.z.abs())
}

/// Compute the P1 signals for one brick from the delivered-light magnitude at
/// each valid probe position and its spatial variation across the brick.
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
    let (nx, ny, nz) = (dims[0] as usize, dims[1] as usize, dims[2] as usize);

    // Per-local delivered magnitude for valid, in-bounds probes.
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
        let m = delivered_magnitude(lights, point);
        mag[local] = m;
        present[local] = true;
        sum += m as f64;
        count += 1;
    }

    if count == 0 {
        return BrickSignal {
            mean_delivered: 0.0,
            l1_proxy: None,
            l2_proxy: None,
        };
    }

    let mean = (sum / count as f64) as f32;
    // Relative anchor: the brick's own mean delivered magnitude. A pre-bake
    // predictor cannot know composed magnitude, so it normalizes by the local
    // delivered brightness — the closest forward analog. Floored so a dark
    // brick's tiny absolute deviations don't blow up the ratio.
    let anchor = mean.max(1e-6);

    // L2 proxy: relative max deviation of delivered magnitude from the brick
    // mean (brick-mean reconstruction residual). Always evaluable when count>0.
    let mut l2_dev = 0.0f32;
    for local in 0..PROBES_PER_CELL {
        if present[local] {
            l2_dev = l2_dev.max((mag[local] - mean).abs());
        }
    }
    let l2_proxy = Some(l2_dev / anchor);

    // L1 proxy: relative max residual of delivered magnitude from a trilinear
    // reconstruction of the 8 corner magnitudes. Unevaluable unless every corner
    // is a present (valid, in-bounds) probe — the same condition under which the
    // oracle's L1 SH reconstruction has a full corner basis.
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
        mean_delivered: mean,
        l1_proxy,
        l2_proxy,
    }
}

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

// ---------------------------------------------------------------------------
// Entry point
// ---------------------------------------------------------------------------

/// Coarseness rank: higher is coarser (fewer stored probes). Used for the
/// false-positive (predictor coarser than oracle) comparison.
fn coarseness(level: Level) -> u8 {
    level.to_u8()
}

pub fn run_forward_predict(inputs: &ForwardPredictInputs<'_>) -> ForwardPredictReport {
    let analysis = run_analysis(&inputs.analyze);
    let params = CoarsenParams::default();

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
        ..Default::default()
    };

    let map_p95 = map_p95_magnitude(&analysis.bricks);
    let floor = (params.darkness_frac * map_p95).max(1e-6);
    report.map_p95 = map_p95;
    report.darkness_floor = floor;

    // S3: no non-empty bricks (no lights / empty / all-invalid) → a well-formed,
    // empty report with a zeroed sweep, never a panic.
    if analysis.bricks.is_empty() {
        report.sweep = inputs
            .predictor_thresholds
            .iter()
            .map(|&t| SweepRow {
                threshold: t,
                ..Default::default()
            })
            .collect();
        return report;
    }

    let grid_origin = Vec3::from(inputs.analyze.grid_origin);
    let cell_size = Vec3::from(inputs.analyze.cell_size);
    let dims = inputs.analyze.grid_dims;
    let validity = inputs.analyze.validity;

    // --- Predictor evaluation (timed for the cost axis) ---
    let predictor_start = std::time::Instant::now();
    let mut predictions: Vec<BrickPrediction> = Vec::with_capacity(analysis.bricks.len());
    for record in &analysis.bricks {
        let [cx, cy, cz] = record.cell;
        let signal = brick_signal(
            inputs.lights,
            grid_origin,
            cell_size,
            dims,
            cx as usize,
            cy as usize,
            cz as usize,
            validity,
        );
        let oracle = oracle_level(record, floor, &params);
        report.oracle_histogram.bump(oracle);
        // Continuous score = the L2 (brick-mean deviation) proxy; +inf when
        // unevaluable so it never falsely reads as maximally uniform.
        let score = signal.l2_proxy.unwrap_or(f32::INFINITY);
        predictions.push(BrickPrediction {
            cell: record.cell,
            linear_cell: record.linear_cell,
            valid_probes: record.valid_probes,
            mean_delivered: signal.mean_delivered,
            score,
            l1_proxy: signal.l1_proxy,
            l2_proxy: signal.l2_proxy,
            oracle_level: coarseness(oracle),
        });
    }
    report.predictor_eval_seconds = predictor_start.elapsed().as_secs_f64();

    report.score_vs_oracle_correlation = score_oracle_correlation(&predictions);

    // --- Threshold sweep: confusion matrix + tradeoff metrics ---
    let nonempty = predictions.len() as u64;
    for &t in inputs.predictor_thresholds {
        let mut confusion = [[0u64; 3]; 3];
        let mut fp = 0u64;
        let mut agree = 0u64;
        let mut oracle_coarsenable = 0u64;
        let mut recovered_safe = 0u64;
        for p in &predictions {
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
                // Safe recovered saving: predictor also coarsens the brick and no
                // coarser than the oracle admits.
                if pred != Level::L0 && coarseness(pred) <= coarseness(oracle) {
                    recovered_safe += 1;
                }
            }
        }
        report.sweep.push(SweepRow {
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

    report.best_operating_point = best_operating_point(&report.sweep);
    report.bricks = predictions;
    report
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
                a.false_positive_bricks
                    .cmp(&b.false_positive_bricks)
                    .then(
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
    if denom <= 0.0 {
        0.0
    } else {
        sxy / denom
    }
}

fn ratio(n: u64, d: u64) -> f32 {
    if d == 0 { 0.0 } else { n as f32 / d as f32 }
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
        "[sh-forward-predict] oracle levels: L0 {} L1 {} L2 {} (map-p95 {:.6}, floor {:.6}, gate p95<={:.2} max<={:.2}); \
         score↔oracle r={:.3}; predictor eval {:.4}s",
        report.oracle_histogram.l0,
        report.oracle_histogram.l1,
        report.oracle_histogram.l2,
        report.map_p95,
        report.darkness_floor,
        report.rel_p95_max,
        report.rel_max_max,
        report.score_vs_oracle_correlation,
        report.predictor_eval_seconds,
    );
    log::info!(
        "[sh-forward-predict] === FP-rate vs recovered-savings tradeoff (predictor coarser than oracle = unsafe) ==="
    );
    for row in &report.sweep {
        log::info!(
            "[sh-forward-predict] t={:.3}: FP {} ({:.4}) | recovered {}/{} ({:.4}) | agree {:.4} | \
             confusion[pred={{L0,L1,L2}}][oracle] {:?}",
            row.threshold,
            row.false_positive_bricks,
            row.false_positive_rate,
            row.recovered_safe,
            row.oracle_coarsenable,
            row.recovered_savings_fraction,
            row.agreement_rate,
            row.confusion,
        );
    }
    if let Some(best) = report.best_operating_point {
        let row = &report.sweep[best];
        log::info!(
            "[sh-forward-predict] best near-zero-FP operating point: t={:.3}, FP {} ({:.4}), recovered {:.4}, agree {:.4}",
            row.threshold,
            row.false_positive_bricks,
            row.false_positive_rate,
            row.recovered_savings_fraction,
            row.agreement_rate,
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::map_data::{FalloffModel, LightType, MapLight, ShadowType};
    use glam::DVec3;
    use postretro_level_format::sh_volume::OctahedralShVolumeSection;

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
        // L2 proxy within threshold → coarsest level chosen.
        assert_eq!(predictor_level(Some(0.02), Some(0.05), 0.10), Level::L2);
        // L2 over, L1 under → L1.
        assert_eq!(predictor_level(Some(0.02), Some(0.20), 0.10), Level::L1);
        // Both over → dense L0.
        assert_eq!(predictor_level(Some(0.30), Some(0.40), 0.10), Level::L0);
        // Unevaluable proxies never select their level.
        assert_eq!(predictor_level(None, None, 1.0), Level::L0);
        assert_eq!(predictor_level(None, Some(0.01), 0.10), Level::L2);
    }

    #[test]
    fn oracle_admits_coarsest_level_passing_the_relative_gate() {
        let params = CoarsenParams::default();
        let floor = 0.1;

        // Bright brick, both levels low error → L2 (coarsest).
        let mut r = brick_record(1.0, 1.0);
        r.base_l2 = evaluable(0.01, 0.02);
        r.base_l1 = evaluable(0.05, 0.10);
        assert_eq!(oracle_level(&r, floor, &params), Level::L2);

        // L2 fails p95 gate, L1 passes → L1.
        let mut r = brick_record(1.0, 1.0);
        r.base_l2 = evaluable(0.50, 0.10);
        r.base_l1 = evaluable(0.05, 0.10);
        assert_eq!(oracle_level(&r, floor, &params), Level::L1);

        // Both fail → dense L0.
        let mut r = brick_record(1.0, 1.0);
        r.base_l2 = evaluable(0.50, 0.60);
        r.base_l1 = evaluable(0.40, 0.55);
        assert_eq!(oracle_level(&r, floor, &params), Level::L0);
    }

    #[test]
    fn oracle_darkness_bypass_takes_coarsest_evaluable_level() {
        let params = CoarsenParams::default();
        let floor = 0.1;
        // Sub-floor magnitude, huge base error, but L2 evaluable → still L2.
        let mut r = brick_record(0.001, 0.001);
        r.base_l2 = evaluable(9.0, 9.0);
        r.base_l1 = evaluable(9.0, 9.0);
        assert_eq!(oracle_level(&r, floor, &params), Level::L2);
    }

    #[test]
    fn oracle_treats_unevaluable_level_as_ineligible() {
        let params = CoarsenParams::default();
        let floor = 0.1;
        // L2 unevaluable (no samples), L1 evaluable and passing → L1, never L2.
        let mut r = brick_record(1.0, 1.0);
        r.base_l2 = LevelErrStatsProxy::unevaluable();
        r.base_l1 = evaluable(0.05, 0.10);
        assert_eq!(oracle_level(&r, floor, &params), Level::L1);
    }

    #[test]
    fn predictor_scores_uniform_light_far_below_concentrated_light() {
        // Single 4x4x4 brick, all probes valid, unit cells.
        let dims = [AF as u32, AF as u32, AF as u32];
        let validity = vec![1u8; PROBES_PER_CELL];
        let origin = Vec3::ZERO;
        let cs = Vec3::ONE;

        // Directional light delivers position-independent radiance → no spatial
        // variation across the brick → near-zero coarsenability proxies.
        let dir = directional_light([0.0, -1.0, 0.0], 1.0);
        let uniform = brick_signal(&[&dir], origin, cs, dims, 0, 0, 0, &validity);
        assert!(uniform.l2_proxy.unwrap() < 1e-5, "directional light is uniform");
        assert!(uniform.l1_proxy.unwrap() < 1e-5);
        assert!(uniform.mean_delivered > 0.0);

        // A point light at one corner with a tight range delivers a steep spatial
        // gradient across the brick → a much larger coarsenability proxy. This is
        // the contribution-awareness the predictor rests on.
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
        // S3 degenerate: no lights → zero delivered magnitude, evaluable zero
        // proxies, no panic or NaN.
        let dims = [AF as u32, AF as u32, AF as u32];
        let validity = vec![1u8; PROBES_PER_CELL];
        let signal = brick_signal(&[], Vec3::ZERO, Vec3::ONE, dims, 0, 0, 0, &validity);
        assert_eq!(signal.mean_delivered, 0.0);
        assert_eq!(signal.l2_proxy, Some(0.0));
        assert_eq!(signal.l1_proxy, Some(0.0));
    }

    #[test]
    fn brick_signal_all_invalid_probes_yields_unevaluable_proxies() {
        // S3 degenerate: an all-invalid brick has no reconstruction basis.
        let dims = [AF as u32, AF as u32, AF as u32];
        let validity = vec![0u8; PROBES_PER_CELL];
        let dir = directional_light([0.0, -1.0, 0.0], 1.0);
        let signal = brick_signal(&[&dir], Vec3::ZERO, Vec3::ONE, dims, 0, 0, 0, &validity);
        assert_eq!(signal.mean_delivered, 0.0);
        assert_eq!(signal.l2_proxy, None);
        assert_eq!(signal.l1_proxy, None);
    }

    #[test]
    fn run_forward_predict_on_empty_grid_yields_well_formed_empty_matrix() {
        // S3: no baked lights / empty grid → run_analysis returns no bricks; the
        // harness must produce a well-formed (empty) sweep, never a panic.
        let base = empty_base_section();
        let validity: Vec<u8> = Vec::new();
        let protect = Vec::new();
        let thresholds = DEFAULT_PREDICTOR_THRESHOLDS;
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
        };
        let report = run_forward_predict(&inputs);
        assert_eq!(report.nonempty_bricks, 0);
        assert!(report.bricks.is_empty());
        assert_eq!(report.sweep.len(), thresholds.len());
        for row in &report.sweep {
            assert_eq!(row.false_positive_bricks, 0);
            assert_eq!(row.oracle_coarsenable, 0);
            assert_eq!(row.confusion, [[0u64; 3]; 3]);
        }
        // Serializes cleanly (no non-finite floats reach JSON).
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
