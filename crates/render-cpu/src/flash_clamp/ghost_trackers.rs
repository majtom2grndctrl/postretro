// Ghost edge trackers: what a ghost draws over every background, tracked at
// the edge points that bound it, and the straight-line rate cap.
// See: context/lib/rendering_pipeline.md §7.8 (Photosensitivity limiter), §7.9

use super::ghost_window::{GhostWindow, SlotClass};
use super::value::{FLASH_LUMINANCE_THRESHOLD, Tracker};

/// Samples along each outer edge of the (albedo, background) square.
const EDGE_SAMPLES: usize = 16;

/// Edge points tracked: (1, k/16) and (k/16, 1) for k = 0..=16, the corner
/// (1, 1) once.
pub(super) const EDGE_POINTS: usize = 2 * EDGE_SAMPLES + 1;

/// The swing an edge tracker counts and reverses at: the flash threshold less
/// half a sample step. A point between two samples draws within half a step
/// of the nearer one, so any flash it sees is a swing of at least this there.
pub(super) const EDGE_THRESHOLD: f32 = FLASH_LUMINANCE_THRESHOLD - 0.5 / EDGE_SAMPLES as f32;

/// How far back an onward tracker may move: the rounding of what an edge point
/// draws along a step that is monotone there, never a visible change.
const ONWARD_ROUNDING: f32 = 1.0e-6;

/// The (albedo, background) of edge point `edge`.
pub(super) fn edge_point(edge: usize) -> (f32, f32) {
    let step = |k: usize| k as f32 / EDGE_SAMPLES as f32;
    if edge <= EDGE_SAMPLES {
        (1.0, step(edge))
    } else {
        (step(edge - EDGE_SAMPLES - 1), 1.0)
    }
}

/// What a ghost draws, as the two values every background's draw mixes. Over
/// background luminance B a ghost of albedo a at presented opacity o and drawn
/// tint luminance L draws B(1 − o) + a·o·L: `a · white_over_black + B ·
/// black_over_white`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct Drawn {
    /// A white ghost over black: o·L.
    pub(super) white_over_black: f32,
    /// A black ghost over white: 1 − o.
    pub(super) black_over_white: f32,
}

impl Drawn {
    /// What a ghost draws at presented `opacity` with drawn tint `luminance`.
    pub(super) fn of(opacity: f32, luminance: f32) -> Self {
        Self {
            white_over_black: opacity * luminance,
            black_over_white: 1.0 - opacity,
        }
    }

    /// What it draws with `albedo` over `background`.
    fn at(self, albedo: f32, background: f32) -> f32 {
        albedo * self.white_over_black + background * self.black_over_white
    }
}

/// How far toward `to` a step from `from` may go under the straight-line rate
/// cap: the whole step scaled so that its largest change over black, over
/// white or for a white ghost over white, max(|Δp|, |Δq|, |Δp + Δq|), is at
/// most `cap`. Each corner keeps the screen channels' speed, and the step stays
/// on the straight line, so it never draws a detour the asked change does not.
/// `None` when the whole step fits.
pub(super) fn rate_cap_scale(from: Drawn, to: Drawn, cap: f32) -> Option<f32> {
    let lit = to.white_over_black - from.white_over_black;
    let covered = to.black_over_white - from.black_over_white;
    let largest = lit.abs().max(covered.abs()).max((lit + covered).abs());
    if largest > cap {
        Some(cap / largest)
    } else {
        None
    }
}

/// What a ghost draws, tracked at every edge point: an interior (albedo,
/// background) is a scale of an edge point, so its swings are a scale of that
/// point's, and no dark state is tested, so a near-white swing counts too.
#[derive(Clone, Copy, Debug)]
pub(super) struct EdgeTrackers {
    trackers: [Tracker; EDGE_POINTS],
}

impl EdgeTrackers {
    /// Trackers starting fresh at what `drawn` draws.
    pub(super) fn fresh(drawn: Drawn) -> Self {
        let mut trackers = [Tracker::default(); EDGE_POINTS];
        for (edge, tracker) in trackers.iter_mut().enumerate() {
            let (albedo, background) = edge_point(edge);
            tracker.restart(drawn.at(albedo, background));
        }
        Self { trackers }
    }

    /// Move every edge point to what `drawn` draws, counting each transition
    /// (an alternating swing of at least [`EDGE_THRESHOLD`] from the last
    /// extremum) on `window` as `class`, opening a slot only where `opens`
    /// allows. A counted swing that reaches further carries its count on (see
    /// [`GhostWindow::extend`]): a background between edge samples, or an
    /// albedo inside the square, may reach the flash threshold only then. A
    /// tracker in `onward` must not move against the way it last counted,
    /// beyond rounding.
    /// Returns `false` when a transition finds no slot or an `onward` tracker
    /// turns back; the trackers and window are then partly moved, for the
    /// caller to discard.
    pub(super) fn step(
        &mut self,
        drawn: Drawn,
        class: SlotClass,
        opens: bool,
        onward: u64,
        window: &mut GhostWindow,
    ) -> bool {
        for (edge, tracker) in self.trackers.iter_mut().enumerate() {
            let (albedo, background) = edge_point(edge);
            let next = drawn.at(albedo, background);
            if onward & (1u64 << edge) != 0
                && (next - tracker.out) * f32::from(tracker.last_sign) < -ONWARD_ROUNDING
            {
                return false;
            }
            let further = tracker.dir != 0.0 && (next - tracker.peak) * tracker.dir > 0.0;
            let (excursion, _) = tracker.excursion(next, EDGE_THRESHOLD);
            tracker.present(next, EDGE_THRESHOLD);
            if excursion.abs() < EDGE_THRESHOLD {
                continue;
            }
            let sign: i8 = if excursion > 0.0 { 1 } else { -1 };
            let counted = if sign != tracker.last_sign {
                tracker.last_sign = sign;
                window.count_edge(edge, class, opens)
            } else if further {
                window.extend(edge, class, opens)
            } else {
                true
            };
            if !counted {
                return false;
            }
        }
        true
    }
}

#[cfg(test)]
#[path = "ghost_trackers_tests.rs"]
mod tests;
