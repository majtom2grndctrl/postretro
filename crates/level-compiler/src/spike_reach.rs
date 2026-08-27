//! Cold-bake reaching-light measurement spike.
//!
//! Build-to-learn instrumentation for
//! `context/plans/drafts/lighting-scale--cold-bake-reaching-light-spike`.
//!
//! Question: in the cold base-indirect SH bake and the cold lightmap bake, what
//! fraction of `static_lights` actually reaches a given receiver, and is culling
//! to that set worth a full implementation spec?
//!
//! Everything here is gated behind env vars and defaults OFF, so ordinary builds
//! — and every shipped `.prl` — are byte-for-byte unaffected. This is not a
//! shipping feature (see the spike's Non-goals); it is a measurement harness the
//! findings note is written from.
//!
//! Two independent switches:
//!   * `POSTRETRO_SPIKE_REACH_STATS=1` — record, per receiver, the reaching-light
//!     distribution for both cold bakes and log it after each stage. Measures:
//!       - `mechanism`: the count the shipped affinity cull would keep for the
//!         receiver's cell (falloff-sphere AABB ∩ portal-reachability flood).
//!       - `in_range`: the count within falloff range of the receiver's exact
//!         point (what a per-receiver range early-out keeps — a byte-identical,
//!         portal-free cull).
//!   * `POSTRETRO_SPIKE_REACH_CULL=1` — apply the byte-identical per-receiver
//!     range early-out in the cold SH bake (skip the shadow ray for a light
//!     whose falloff is provably zero at the hit point). Lets the wall-clock of
//!     that cull be measured against an unculled baseline. The cold lightmap
//!     bake already performs this early-out, so the flag is a no-op there.
//!
//! Receiver attribution honors the spike's correctness constraints: the SH bake
//! keys on the shadow-ray *hit point* (the bounce origin), the lightmap on the
//! texel world position — never the probe's cell. Directional lights are never
//! range-culled and are counted as reaching every cell.

use std::sync::OnceLock;
use std::sync::atomic::{AtomicU64, Ordering};

use glam::Vec3;

use crate::affinity_grid::WorldReachIndex;
use crate::map_data::{LightType, MapLight};

fn env_flag(name: &str) -> bool {
    std::env::var_os(name).is_some_and(|v| v == "1" || v == "true")
}

/// Recording of the per-receiver reaching-light distribution enabled.
pub fn stats_enabled() -> bool {
    static F: OnceLock<bool> = OnceLock::new();
    *F.get_or_init(|| env_flag("POSTRETRO_SPIKE_REACH_STATS"))
}

/// Byte-identical per-receiver range early-out (SH bake) enabled.
pub fn cull_enabled() -> bool {
    static F: OnceLock<bool> = OnceLock::new();
    *F.get_or_init(|| env_flag("POSTRETRO_SPIKE_REACH_CULL"))
}

/// Any spike behavior active — cheap short-circuit for the hot loops.
#[inline]
pub fn active() -> bool {
    stats_enabled() || cull_enabled()
}

/// Does `light` have a non-zero falloff at `point`? A light for which this is
/// false contributes exactly zero regardless of the shadow ray, so skipping its
/// shadow ray is byte-identical. Mirrors `sh_bake::falloff` / `lightmap_bake::
/// falloff` exactly: all falloff models return 0 iff `distance > range`, with
/// `range = falloff_range.max(1e-4)`. Directional lights have no range and are
/// always considered reaching.
#[inline]
pub fn reaches_range(light: &MapLight, point: Vec3) -> bool {
    match light.light_type {
        LightType::Directional => true,
        LightType::Point | LightType::Spot => {
            let to_light = Vec3::new(
                light.origin.x as f32 - point.x,
                light.origin.y as f32 - point.y,
                light.origin.z as f32 - point.z,
            );
            let range = light.falloff_range.max(1.0e-4);
            to_light.length() <= range
        }
    }
}

/// Per-stage accumulator. Two histograms keyed on reaching-light *count*
/// (0..=total), so exact min/median/p95/max/mean fall out of the counts with no
/// per-sample storage. `AtomicU64` with `Relaxed` ordering — counts are exact
/// under contention; ordering does not matter for a pure tally.
struct Stage {
    total: usize,
    /// Affinity-cell mechanism reaching-count histogram (`None` if no index —
    /// stats disabled but cull enabled, or empty geometry).
    index: Option<WorldReachIndex>,
    mechanism: Vec<AtomicU64>,
    in_range: Vec<AtomicU64>,
}

impl Stage {
    fn new(total: usize, index: Option<WorldReachIndex>) -> Self {
        Stage {
            total,
            index,
            mechanism: (0..=total).map(|_| AtomicU64::new(0)).collect(),
            in_range: (0..=total).map(|_| AtomicU64::new(0)).collect(),
        }
    }

    #[inline]
    fn record(&self, point: Vec3, in_range: u32) {
        let ir = (in_range as usize).min(self.total);
        self.in_range[ir].fetch_add(1, Ordering::Relaxed);
        if let Some(idx) = &self.index {
            let m = (idx.reaching_count_at(point.as_dvec3()) as usize).min(self.total);
            self.mechanism[m].fetch_add(1, Ordering::Relaxed);
        }
    }
}

static SH: OnceLock<Stage> = OnceLock::new();
static LM: OnceLock<Stage> = OnceLock::new();

/// Register the SH cold-bake stage before it runs. `index` is built over the
/// same static-light set the bake iterates; `total` is that set's size.
pub fn install_sh(total: usize, index: Option<WorldReachIndex>) {
    let _ = SH.set(Stage::new(total, index));
}

/// Register the lightmap cold-bake stage before it runs.
pub fn install_lm(total: usize, index: Option<WorldReachIndex>) {
    let _ = LM.set(Stage::new(total, index));
}

/// Record one SH-bake receiver (a shadow-ray hit point).
#[inline]
pub fn record_sh(point: Vec3, in_range: u32) {
    if let Some(s) = SH.get() {
        s.record(point, in_range);
    }
}

/// Record one lightmap-bake receiver (a texel world position).
#[inline]
pub fn record_lm(point: Vec3, in_range: u32) {
    if let Some(s) = LM.get() {
        s.record(point, in_range);
    }
}

/// Log the recorded distributions. Called after each cold bake completes.
pub fn log_sh_summary() {
    if let Some(s) = SH.get() {
        log_stage("cold SH bake (indirect, hit-point receivers)", s);
    }
}

pub fn log_lm_summary() {
    if let Some(s) = LM.get() {
        log_stage("cold lightmap bake (texel receivers)", s);
    }
}

// --- summary math ----------------------------------------------------------

struct Dist {
    samples: u64,
    min: u32,
    max: u32,
    median: u32,
    p95: u32,
    mean: f64,
}

/// Reduce a count-keyed histogram to a distribution. Bucket `i` holds the number
/// of receivers whose reaching count was exactly `i`.
fn distribution(buckets: &[AtomicU64]) -> Option<Dist> {
    let counts: Vec<u64> = buckets.iter().map(|b| b.load(Ordering::Relaxed)).collect();
    let samples: u64 = counts.iter().sum();
    if samples == 0 {
        return None;
    }
    let mut min = None;
    let mut max = 0u32;
    let mut weighted: u128 = 0;
    for (i, &c) in counts.iter().enumerate() {
        if c > 0 {
            if min.is_none() {
                min = Some(i as u32);
            }
            max = i as u32;
            weighted += c as u128 * i as u128;
        }
    }
    let percentile = |frac: f64| -> u32 {
        // Smallest count value whose cumulative share reaches `frac`.
        let target = (samples as f64 * frac).ceil() as u64;
        let target = target.max(1).min(samples);
        let mut cum = 0u64;
        for (i, &c) in counts.iter().enumerate() {
            cum += c;
            if cum >= target {
                return i as u32;
            }
        }
        max
    };
    Some(Dist {
        samples,
        min: min.unwrap_or(0),
        max,
        median: percentile(0.50),
        p95: percentile(0.95),
        mean: weighted as f64 / samples as f64,
    })
}

fn frac(count: u32, total: usize) -> f64 {
    if total == 0 {
        0.0
    } else {
        count as f64 / total as f64
    }
}

fn log_stage(label: &str, s: &Stage) {
    log::info!(
        "SPIKE reach-light [{label}] — total static lights per receiver = {}",
        s.total
    );
    if let Some(d) = distribution(&s.in_range) {
        log::info!(
            "SPIKE   in-range (per-receiver falloff, byte-identical cull set): \
             receivers={} count[min={} median={} p95={} max={} mean={:.2}] \
             fraction[median={:.4} p95={:.4} mean={:.4}]",
            d.samples,
            d.min,
            d.median,
            d.p95,
            d.max,
            d.mean,
            frac(d.median, s.total),
            frac(d.p95, s.total),
            d.mean / s.total.max(1) as f64,
        );
    } else {
        log::info!("SPIKE   in-range: no receivers recorded");
    }
    if s.index.is_some() {
        if let Some(d) = distribution(&s.mechanism) {
            log::info!(
                "SPIKE   mechanism (affinity-cell cull set: falloff-AABB ∩ portal reach): \
                 receivers={} count[min={} median={} p95={} max={} mean={:.2}] \
                 fraction[median={:.4} p95={:.4} mean={:.4}]",
                d.samples,
                d.min,
                d.median,
                d.p95,
                d.max,
                d.mean,
                frac(d.median, s.total),
                frac(d.p95, s.total),
                d.mean / s.total.max(1) as f64,
            );
        }
    } else {
        log::info!("SPIKE   mechanism: not measured (no reach index installed)");
    }
}
