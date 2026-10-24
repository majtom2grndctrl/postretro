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

/// Half the step between neighbouring edge samples. What two neighbours draw
/// differs by at most a step, so a point between them draws within half a
/// step of the line through the nearer one, and its swings differ from that
/// one's by at most this.
const HALF_STEP: f32 = 0.5 / EDGE_SAMPLES as f32;

/// How far what draws may sit from what the clamp judged. The draw packs
/// opacity as unorm16, up to 7.6e-6 from the judged f32 opacity, and a swing
/// compares two drawn values; the rest covers f32 rounding of the drawn value.
const DRAWN_ROUNDING: f32 = 2.0e-5;

/// The swing an edge tracker counts and reverses at: the flash threshold less
/// half a sample step (a point between two samples swings within half a step
/// of the nearer one) and less [`DRAWN_ROUNDING`], so any flash that draws is
/// a swing of at least this at some edge point.
pub(super) const EDGE_THRESHOLD: f32 = FLASH_LUMINANCE_THRESHOLD - HALF_STEP - DRAWN_ROUNDING;

/// How far back an onward tracker may move: the rounding of what an edge point
/// draws along a step that is monotone there, never a visible change.
const ONWARD_ROUNDING: f32 = 1.0e-6;

/// The `last_sign` of an edge tracker whose history is unknown and that has
/// not moved since ([`EdgeTrackers::unknown_history`]). Its `dir` is 0, as for
/// any tracker that has not moved; its first move sets a sign of its own. A
/// sentinel in the sign rather than a field of its own: the trackers are the
/// screen channels' [`Tracker`], and a mask beside them would leave the look
/// clamp no room in its per-ghost size budget. [`EdgeTrackers::step`] handles
/// it before any rule reads the sign.
const UNKNOWN_HISTORY: i8 = i8::MIN;

/// The (albedo, background) of edge point `edge`.
pub(super) fn edge_point(edge: usize) -> (f32, f32) {
    let step = |k: usize| k as f32 / EDGE_SAMPLES as f32;
    if edge <= EDGE_SAMPLES {
        (1.0, step(edge))
    } else {
        (step(edge - EDGE_SAMPLES - 1), 1.0)
    }
}

/// The edge points beside `edge` along the square's outer edges, which run
/// from (1, 0) up to the corner (1, 1) and across to (0, 1).
fn neighbours(edge: usize) -> [Option<usize>; 2] {
    // Position along that run, and back: edges past the corner run backwards.
    let along = |edge: usize| {
        if edge <= EDGE_SAMPLES {
            edge
        } else {
            3 * EDGE_SAMPLES + 1 - edge
        }
    };
    let position = along(edge);
    [
        position.checked_sub(1).map(along),
        (position < 2 * EDGE_SAMPLES).then(|| along(position + 1)),
    ]
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

    /// How much more edge point `neighbour` draws than edge point `edge`: the
    /// slope of the line the points between them draw on. Neighbours along
    /// the background edge differ by `black_over_white` / 16, along the albedo
    /// edge by `white_over_black` / 16.
    fn spread(self, edge: usize, neighbour: usize) -> f32 {
        let (albedo, background) = edge_point(edge);
        let (next_albedo, next_background) = edge_point(neighbour);
        (next_albedo - albedo) * self.white_over_black
            + (next_background - background) * self.black_over_white
    }
}

/// How far toward `to` a step from `from` may go under the straight-line rate
/// cap: the whole step scaled so that its largest change over black, over
/// white or for a white ghost over white, max(|Δp|, |Δq|, |Δp + Δq|), is at
/// most `cap`. Each of those three keeps the screen channels' speed, and the
/// step stays on the straight line, so it never draws a detour the asked
/// change does not. `None` when the whole step fits.
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
///
/// Each edge point also keeps a reference: the drawn state of one frame of
/// its current excursion on which it drew its peak. The line its segment to
/// a neighbour drew then bounds how far a point between them has reached, so
/// a later frame drawing that segment below it somewhere may bring such a
/// point to a new extreme. Along the background edge only `black_over_white`
/// sets that line's slope, along the albedo edge only `white_over_black`, so
/// each point keeps the one it needs and the corner keeps both.
#[derive(Clone, Copy, Debug)]
pub(super) struct EdgeTrackers {
    trackers: [Tracker; EDGE_POINTS],
    /// The reference `black_over_white` of edge points 0..=16 (albedo 1).
    reference_covered: [f32; EDGE_SAMPLES + 1],
    /// The reference `white_over_black` of edge points 16..=32 (background 1,
    /// the corner first).
    reference_lit: [f32; EDGE_SAMPLES + 1],
}

impl EdgeTrackers {
    /// Trackers starting fresh at what `drawn` draws: a swing counts once it
    /// reaches [`EDGE_THRESHOLD`] from here. For trackers whose history starts
    /// with the limiter, so nothing before was presented under it.
    pub(super) fn fresh(drawn: Drawn) -> Self {
        let mut edges = Self {
            trackers: [Tracker::default(); EDGE_POINTS],
            reference_covered: [0.0; EDGE_SAMPLES + 1],
            reference_lit: [0.0; EDGE_SAMPLES + 1],
        };
        for edge in 0..EDGE_POINTS {
            let (albedo, background) = edge_point(edge);
            edges.trackers[edge].restart(drawn.at(albedo, background));
            edges.set_reference(edge, drawn);
        }
        edges
    }

    /// Trackers at what `drawn` draws whose history is unknown: the screen
    /// showed earlier states the clamp cannot see, so each point may be part
    /// way into a swing that any move completes. Each tracker counts its
    /// first move, of any size and either way, as a transition that way, and
    /// from then on runs as [`Self::step`] describes. A tracker that has not
    /// moved counts nothing.
    ///
    /// The rule for any ghost whose earlier presentation the clamp can't see:
    /// one dropped at rest and appearing again today; a reload or a join in
    /// the middle of a strobe later.
    pub(super) fn unknown_history(drawn: Drawn) -> Self {
        let mut edges = Self::fresh(drawn);
        for tracker in &mut edges.trackers {
            tracker.last_sign = UNKNOWN_HISTORY;
        }
        edges
    }

    /// Edge point `edge`'s reference, the share its spreads do not depend on
    /// left at zero.
    fn reference(&self, edge: usize) -> Drawn {
        Drawn {
            white_over_black: if edge >= EDGE_SAMPLES {
                self.reference_lit[edge - EDGE_SAMPLES]
            } else {
                0.0
            },
            black_over_white: if edge <= EDGE_SAMPLES {
                self.reference_covered[edge]
            } else {
                0.0
            },
        }
    }

    fn set_reference(&mut self, edge: usize, drawn: Drawn) {
        if edge <= EDGE_SAMPLES {
            self.reference_covered[edge] = drawn.black_over_white;
        }
        if edge >= EDGE_SAMPLES {
            self.reference_lit[edge - EDGE_SAMPLES] = drawn.white_over_black;
        }
    }

    /// Move every edge point to what `drawn` draws, counting each transition
    /// on `window` as `class`, opening a slot only where `opens` allows. A
    /// tracker in `onward` must not move against the way it last counted,
    /// beyond rounding.
    ///
    /// At each edge point, in order:
    ///
    /// - a point whose history is unknown ([`Self::unknown_history`]) counts
    ///   its first move, of any size, as a transition its way, and runs the
    ///   rules below from the next frame on;
    /// - a swing of at least [`EDGE_THRESHOLD`] from the last extremum, the
    ///   other way from the one last counted, counts a transition;
    /// - a counted swing reaching past its peak carries its count on
    ///   ([`GhostWindow::extend`]);
    /// - a counted swing smaller than the edge threshold (only an unknown
    ///   first move counts one), once this point has counted its swing back,
    ///   reaching past its peak again carries that earlier transition on
    ///   while the window holds both ([`GhostWindow::restart_earlier`]), and
    ///   otherwise counts it anew: a point nearby may only now complete the
    ///   swing the clamp could not see. On an exempt step it carries its last
    ///   count on in place instead ([`GhostWindow::restart_latest`]), opening
    ///   nothing: the first move left what nothing drawn draws and the exempt
    ///   ray moves every point back toward there, so the ray passes the peak
    ///   by f32 rounding at most, never a flash; a real pass is a budgeted
    ///   step, which still counts;
    /// - a counted swing that stops short of its peak, on a frame that moves
    ///   this point or a neighbour its way and draws the segment between
    ///   them below the reference line somewhere (by twice how far short it
    ///   is, for the half nearer this point), carries its count on too: a
    ///   point there may reach a new extreme, and cross the flash threshold,
    ///   on this frame;
    /// - a counted swing back within half a step of the extremum it started
    ///   from, on a frame that moves this point or a neighbour toward it,
    ///   counts a transition that way: a point nearby that never saw this
    ///   swing, smaller than the flash threshold, may reach a new extreme of
    ///   its own older one there.
    ///
    /// None of these fires on a frame where nothing moves. A look that keeps
    /// moving near a counted peak keeps its slot while it moves, which only
    /// ever limits more. Returns `false` when a transition finds no slot or
    /// an `onward` tracker turns back; the trackers and window are then partly
    /// moved, for the caller to discard.
    pub(super) fn step(
        &mut self,
        drawn: Drawn,
        class: SlotClass,
        opens: bool,
        onward: u64,
        window: &mut GhostWindow,
    ) -> bool {
        let next: [f32; EDGE_POINTS] = std::array::from_fn(|edge| {
            let (albedo, background) = edge_point(edge);
            drawn.at(albedo, background)
        });
        let moved: [f32; EDGE_POINTS] =
            std::array::from_fn(|edge| next[edge] - self.trackers[edge].out);
        for edge in 0..EDGE_POINTS {
            let tracker = self.trackers[edge];
            let value = next[edge];
            if tracker.last_sign == UNKNOWN_HISTORY {
                // Not in any slot of its own yet, so no onward rule binds it.
                if moved[edge] == 0.0 {
                    continue;
                }
                let mut stepped = tracker;
                stepped.present(value, EDGE_THRESHOLD);
                stepped.last_sign = if moved[edge] > 0.0 { 1 } else { -1 };
                self.trackers[edge] = stepped;
                self.set_reference(edge, drawn);
                if !window.count_edge(edge, class, opens) {
                    return false;
                }
                continue;
            }
            if onward & (1u64 << edge) != 0
                && moved[edge] * f32::from(tracker.last_sign) < -ONWARD_ROUNDING
            {
                return false;
            }
            let further = tracker.dir != 0.0 && (value - tracker.peak) * tracker.dir > 0.0;
            let (excursion, extremum) = tracker.excursion(value, EDGE_THRESHOLD);
            let mut stepped = tracker;
            stepped.present(value, EDGE_THRESHOLD);
            if stepped.peak != tracker.peak || stepped.dir != tracker.dir {
                self.set_reference(edge, drawn);
            }
            let sign: i8 = if excursion > 0.0 { 1 } else { -1 };
            let swung = excursion.abs() >= EDGE_THRESHOLD;
            // The swing this point last counted is the one it is running.
            let counted_run = tracker.dir != 0.0
                && stepped.dir == tracker.dir
                && f32::from(tracker.last_sign) == tracker.dir;
            // This point counted a swing back inside a swing smaller than the
            // edge threshold, which only an unknown first move counts.
            let turned_back_small = tracker.dir != 0.0
                && stepped.dir == tracker.dir
                && f32::from(tracker.last_sign) == -tracker.dir
                && (tracker.peak - extremum).abs() < EDGE_THRESHOLD;
            let counted = if swung && sign != tracker.last_sign {
                stepped.last_sign = sign;
                window.count_edge(edge, class, opens)
            } else if swung && further {
                window.extend(edge, class, opens)
            } else if turned_back_small && further {
                if window.restart_earlier(edge) {
                    true
                } else if class == SlotClass::Exempt {
                    // A pass on the exempt ray is f32 rounding only while
                    // unknown history starts from nothing drawn; a caller
                    // starting it from a drawn look has to weigh this.
                    window.restart_latest(edge);
                    true
                } else {
                    stepped.last_sign = if tracker.dir > 0.0 { 1 } else { -1 };
                    window.count_edge(edge, class, opens)
                }
            } else if !counted_run {
                true
            } else if self.nears_new_extreme(edge, drawn, &tracker, value, &moved) {
                window.extend(edge, class, opens)
            } else if self.back_near_extremum(edge, &tracker, value, extremum, &moved) {
                stepped.last_sign = if tracker.dir > 0.0 { -1 } else { 1 };
                window.count_edge(edge, class, opens)
            } else {
                true
            };
            self.trackers[edge] = stepped;
            if !counted {
                return false;
            }
        }
        true
    }

    /// Whether edge point `edge`, running a counted swing and stopped short
    /// of its peak at `value`, may have brought a point between it and a
    /// neighbour, nearer to it, to a new extreme this frame.
    ///
    /// Such a point x = y + λ(z − y), λ ≤ 1/2, has drawn no further its way
    /// on any frame of this point's excursion, the reference frame included,
    /// so reaching further now needs λ times the spread's move since that
    /// frame (its way) to cover how far short of the peak this point is.
    /// Back at its peak with a segment line no higher anywhere, this frame is
    /// the tighter reference; a line exactly the reference's draws nothing
    /// new.
    fn nears_new_extreme(
        &mut self,
        edge: usize,
        drawn: Drawn,
        tracker: &Tracker,
        value: f32,
        moved: &[f32; EDGE_POINTS],
    ) -> bool {
        let way = tracker.dir;
        let short = (tracker.peak - value) * way;
        let reference = self.reference(edge);
        let sides = neighbours(edge).map(|neighbour| {
            neighbour.map(|neighbour| {
                let now = drawn.spread(edge, neighbour);
                (neighbour, (now - reference.spread(edge, neighbour)) * way)
            })
        });
        if short == 0.0 && sides.iter().flatten().all(|&(_, toward)| toward >= 0.0) {
            self.set_reference(edge, drawn);
            if sides.iter().flatten().all(|&(_, toward)| toward == 0.0) {
                return false;
            }
        }
        sides.iter().flatten().any(|&(neighbour, toward)| {
            toward >= 2.0 * short - DRAWN_ROUNDING
                && (moved[edge] * way > 0.0 || moved[neighbour] * way > 0.0)
        })
    }

    /// Whether edge point `edge`, running a counted swing, is back at `value`
    /// within half a step of `extremum`, where that swing started, on a frame
    /// moving it or a neighbour that way. A point between it and a neighbour
    /// that never saw this swing may then reach a new extreme of the one
    /// before, which this point counted already and must count again.
    fn back_near_extremum(
        &self,
        edge: usize,
        tracker: &Tracker,
        value: f32,
        extremum: f32,
        moved: &[f32; EDGE_POINTS],
    ) -> bool {
        let way = -tracker.dir;
        (extremum - value) * way <= HALF_STEP + DRAWN_ROUNDING
            && (moved[edge] * way > 0.0
                || neighbours(edge)
                    .iter()
                    .flatten()
                    .any(|&neighbour| moved[neighbour] * way > 0.0))
    }
}

#[cfg(test)]
#[path = "ghost_trackers_tests.rs"]
mod tests;
