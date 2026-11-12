// MoverProbeChannel (id 57) pose knots after the author ramp, and the rule
// every consumer blends a mover's per-pose states by: the renderer's compose
// (its shaders mirror it) and the compiler's redundant-pose skipping, which
// must judge exactly what compose will read.
// See: context/lib/rendering_pipeline.md §4 (Mover state lighting) ·
// context/lib/build_pipeline.md §Compiler pipeline step 12 (Mover probe channel)
//
// Knots are mover position only: no light, probe or residency reaches them.
// The interval rule blends between absolute states; it never zeroes a
// weight, flags or excludes a probe.

use super::poses::MoverRecord;

/// One pose compose can read, at its t′.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Knot {
    pub t: f32,
    pub pose: u32,
}

/// `clamp((t − s) / (e − s), 0, 1)`: the game's ramp
/// (`runtime_movers::travel::ramped_travel_fraction`), mirrored operation
/// for operation so a knot lands on the very t′ the game hands over at that
/// path position. −0.0 reads as 0, as the hand-over stores it. A subnormal
/// span overflows the quotient past the window's end: +∞ reads 1, and −∞ or
/// NaN reads 0.
pub fn ramped_knot(t: f32, start: f32, end: f32) -> f32 {
    let span = end - start;
    let ramped = if span.is_finite() && span > 0.0 && start.is_finite() {
        (t - start) / span
    } else {
        t
    };
    if ramped.is_finite() {
        ramped.clamp(0.0, 1.0) + 0.0
    } else if ramped > 0.0 {
        1.0
    } else {
        0.0
    }
}

/// The knots compose reads for path positions `knots` (pose order, the
/// first 0 and the last 1) under the ramp `(ramp_start, ramp_end)`: pose 0
/// at t′ 0, every pose whose ramped knot falls strictly inside (0, 1), and
/// the last pose at t′ 1. A knot the ramp squeezes onto 0 or 1 is never
/// read, so the first or last pose's state holds across each ramp edge; t′
/// strictly ascends.
pub fn effective_knots(knots: &[f32], ramp_start: f32, ramp_end: f32) -> Vec<Knot> {
    let last = knots.len().saturating_sub(1);
    let mut out = vec![Knot { t: 0.0, pose: 0 }];
    for (pose, &knot) in knots.iter().enumerate().take(last).skip(1) {
        let t = ramped_knot(knot, ramp_start, ramp_end);
        // Strictly above the previous knot: a squeezed pose, or one rounding
        // onto its neighbour's t′, would open an empty interval.
        if t < 1.0 && t > out[out.len() - 1].t {
            out.push(Knot {
                t,
                pose: pose as u32,
            });
        }
    }
    out.push(Knot {
        t: 1.0,
        pose: last as u32,
    });
    out
}

impl MoverRecord {
    /// This mover's knots through its ramp ([`effective_knots`]).
    pub fn effective_knots(&self) -> Vec<Knot> {
        effective_knots(&self.knots, self.ramp_start, self.ramp_end)
    }
}

/// Where a mover's t′ falls among the poses one probe defines: blend pose
/// `lo`'s state toward pose `hi`'s by `weight`. `weight == 0` reads pose
/// `lo` exactly (`hi == lo` there).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PoseInterval {
    pub lo: u32,
    pub hi: u32,
    pub weight: f32,
}

impl PoseInterval {
    pub fn exact(pose: u32) -> Self {
        Self {
            lo: pose,
            hi: pose,
            weight: 0.0,
        }
    }
}

/// The interval rule: the defined poses around `t` (a stated t′, ≥ 0) among
/// `knots`, and the blend weight between them. The termini always define a
/// state (stored, or the base); `defined(pose)` says whether a mid pose
/// does, and an undefined one is blended across. At a defined knot the
/// state reads exactly; t′ ≤ 0 reads the first pose and t′ ≥ 1 the last.
/// The lookup is a function of t′ alone, so out and back to one t′ reads
/// alike. The renderer's `mc_interval` and `mb_entry_blend` mirror it.
pub fn pose_interval(knots: &[Knot], t: f32, defined: impl Fn(u32) -> bool) -> PoseInterval {
    let last = knots.len() - 1;
    if t >= 1.0 {
        return PoseInterval::exact(knots[last].pose);
    }
    if t <= 0.0 || t.is_nan() {
        return PoseInterval::exact(knots[0].pose);
    }
    let is_defined = |at: usize| at == 0 || at == last || defined(knots[at].pose);
    // The last knot at or below t′: knot 0 sits at 0 < t′, knot `last` at
    // 1 > t′.
    let at = knots.partition_point(|knot| knot.t <= t) - 1;
    let mut lo = at;
    while !is_defined(lo) {
        lo -= 1;
    }
    if knots[lo].t >= t {
        return PoseInterval::exact(knots[lo].pose);
    }
    let mut hi = at + 1;
    while !is_defined(hi) {
        hi += 1;
    }
    PoseInterval {
        lo: knots[lo].pose,
        hi: knots[hi].pose,
        weight: (t - knots[lo].t) / (knots[hi].t - knots[lo].t),
    }
}

/// One end of a depth blend: a pose's moment set (its stored tile, or the
/// probe's base where it reads the base) or the probe's base itself.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum DepthSource {
    Pose(u32),
    Base,
}

/// How a depth blend mixes its two moment sets, texel by texel.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum DepthMix {
    /// The raw moments `E[d]` and `E[d²]`, each linearly.
    Raw,
    /// The mean and the variance, each linearly, then `E[d²]` rebuilt
    /// from them.
    MeanVariance,
}

/// The depth blend one interval reads at its weight: `from` toward `to` by
/// `weight`, mixed by `mix`. `weight == 0` reads `from` exactly.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct DepthSegment {
    pub from: DepthSource,
    pub to: DepthSource,
    pub weight: f32,
    pub mix: DepthMix,
}

impl DepthSegment {
    /// One texel of this blend ([`mix_depth_moments`]).
    #[inline]
    pub fn texel(&self, from: [f32; 2], to: [f32; 2]) -> [f32; 2] {
        mix_depth_moments(from, to, self.weight, self.mix)
    }
}

/// The depth rule: what interval `span` of a mover with `pose_count` poses
/// blends. An interval whose ends are both termini (a two-pose mover, or a
/// probe whose every mid pose is blended across) runs its raw moments
/// through the base: D_lo → base → D_hi, reaching the base at the
/// interval's midpoint, weights exact (doubling, and 2w − 1 over
/// (0.5, 1]). Every other interval blends its two poses' mean and variance
/// directly: a hit distance on a translating surface moves linearly with
/// the mover, so the mean lerp follows it, and mixing raw moments would add
/// `s(1 − s)·Δμ²` of variance the scene does not hold. The renderer's live
/// tiles and the compiler's redundant-pose skipping both read depth through
/// this and [`mix_depth_moments`], so skipping judges exactly what compose
/// reads.
pub fn depth_segment(span: PoseInterval, pose_count: usize) -> DepthSegment {
    let pose = DepthSource::Pose;
    let weight = span.weight;
    if weight <= 0.0 {
        return DepthSegment {
            from: pose(span.lo),
            to: pose(span.lo),
            weight: 0.0,
            mix: DepthMix::Raw,
        };
    }
    let termini = span.lo == 0 && span.hi as usize + 1 == pose_count;
    if !termini {
        return DepthSegment {
            from: pose(span.lo),
            to: pose(span.hi),
            weight,
            mix: DepthMix::MeanVariance,
        };
    }
    let (from, to, weight) = if weight <= 0.5 {
        (pose(span.lo), DepthSource::Base, 2.0 * weight)
    } else {
        (DepthSource::Base, pose(span.hi), 2.0 * weight - 1.0)
    };
    DepthSegment {
        from,
        to,
        weight,
        mix: DepthMix::Raw,
    }
}

/// The largest finite binary16, the most a depth moment may hold: the bake
/// saturates a far texel's `E[d²]` to it.
const F16_MAX: f32 = 65504.0;

/// One texel's `[E[d], E[d²]]` blended from `from` toward `to` by `s`, by
/// `mix`. Exact at `s` 0 and 1, and exactly `from` wherever `to == from`.
///
/// [`DepthMix::Raw`] lerps each moment, `a + s·(b − a)`.
/// [`DepthMix::MeanVariance`] lerps the mean, rounded to binary16 as the
/// live tile stores it, and the variance `σ² = max(0, E[d²] − E[d]²)` of
/// each end, then rebuilds `E[d²] = σ² + μ²` from the rounded mean, capped
/// at the largest finite binary16. So the result's variance is never
/// negative against the mean it stores, and a saturated far texel (`E[d²]`
/// stored as 65504, below `E[d]²`) blends to 65504 again. A few flops per
/// texel: the renderer runs it on every live texel a travelling mover
/// changes, every frame.
#[inline]
pub fn mix_depth_moments(from: [f32; 2], to: [f32; 2], s: f32, mix: DepthMix) -> [f32; 2] {
    if s <= 0.0 {
        return from;
    }
    if s >= 1.0 {
        return to;
    }
    match mix {
        DepthMix::Raw => [lerp(from[0], to[0], s), lerp(from[1], to[1], s)],
        DepthMix::MeanVariance => {
            if from == to {
                return from;
            }
            // Plain compares rather than `f32::max`/`min`, which spend extra
            // work on NaN the finite moments never hold: this runs per texel.
            let variance = |[mean, mean_sq]: [f32; 2]| {
                let variance = mean_sq - mean * mean;
                if variance > 0.0 { variance } else { 0.0 }
            };
            let mean = round_to_f16(lerp(from[0], to[0], s));
            // A lerp of two non-negative variances is non-negative.
            let mean_sq = lerp(variance(from), variance(to), s) + mean * mean;
            [mean, if mean_sq < F16_MAX { mean_sq } else { F16_MAX }]
        }
    }
}

/// `a + s·(b − a)`, for `s` strictly inside (0, 1).
#[inline]
fn lerp(a: f32, b: f32, s: f32) -> f32 {
    a + s * (b - a)
}

/// `value` rounded to the nearest binary16 (ties to even), as an f32:
/// exactly what a live tile's f16 store keeps. The normal binary16 range by
/// integer rounding of the f32 bits, everything else through the shared
/// converters.
#[inline]
fn round_to_f16(value: f32) -> f32 {
    let bits = value.to_bits();
    let magnitude = bits & 0x7fff_ffff;
    // [2^-14, 65520): normal binary16 results, no overflow to infinity.
    if (0x3880_0000..0x477f_f000).contains(&magnitude) {
        let rounded = bits.wrapping_add(0x0fff + ((bits >> 13) & 1)) & !0x1fff;
        return f32::from_bits(rounded);
    }
    round_to_f16_outside_normal(value)
}

/// [`round_to_f16`] past the normal binary16 range: zero, subnormals,
/// overflow and non-finite values. Out of line, so the per-texel path stays
/// small.
#[cold]
#[inline(never)]
fn round_to_f16_outside_normal(value: f32) -> f32 {
    crate::probe_depth_tiles::f16_bits_to_f32(crate::lightmap::f32_to_f16_bits(value))
}

#[cfg(test)]
#[path = "depth_mix_tests.rs"]
mod depth_mix_tests;
