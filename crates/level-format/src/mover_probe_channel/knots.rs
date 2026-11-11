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
/// path position. −0.0 reads as 0, as the hand-over stores it.
pub fn ramped_knot(t: f32, start: f32, end: f32) -> f32 {
    let span = end - start;
    let ramped = if span.is_finite() && span > 0.0 && start.is_finite() {
        (t - start) / span
    } else {
        t
    };
    if ramped.is_finite() {
        ramped.clamp(0.0, 1.0) + 0.0
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

/// Which leg of a depth interval a blend weight falls on. Depth runs
/// through the base: D_lo → base → D_hi, reaching the base at the interval's
/// midpoint (owner, 2026-11-10), so a termini-to-termini interval keeps
/// 12c's through-base blend.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum DepthLeg {
    /// `weight == 0`: pose `lo`'s moments exactly.
    Exact,
    /// D_lo toward the base by this weight.
    ToBase(f32),
    /// The base toward D_hi by this weight.
    FromBase(f32),
}

/// The depth leg an interval weight reads ([`DepthLeg`]). Weights are exact:
/// doubling, and 2w − 1 over (0.5, 1].
pub fn depth_leg(weight: f32) -> DepthLeg {
    if weight <= 0.0 {
        DepthLeg::Exact
    } else if weight <= 0.5 {
        DepthLeg::ToBase(2.0 * weight)
    } else {
        DepthLeg::FromBase(2.0 * weight - 1.0)
    }
}
