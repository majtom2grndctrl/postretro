// Per-ghost look clamp: what each ghost draws, judged over every background at
// the edge points on one flash window per ghost, and the look that presents.
// See: context/lib/rendering_pipeline.md §7.8 (Photosensitivity limiter), §7.9

use std::hash::Hash;

use super::ghost_red::GhostRedness;
use super::ghost_tint::{drawn_luminance, on_grid_at_luminance};
use super::ghost_trackers::{Drawn, EdgeTrackers, rate_cap_scale};
use super::ghost_window::{Budget, GhostWindow, SlotClass};
use super::keyed::{KeyClamp, KeyedClamp};
use super::value::{INTENSITY_RATE_PER_SECOND, unit_strength};
use crate::flash_limiter::{LimiterFrameInput, LimiterFrameUniform, pack_limiter_frame};
use crate::ghost_instance::{GhostInstanceLook, screen_door_coverage};
use crate::ghost_transparency::GhostTransparency;

/// Halvings of a refused frame's segment while searching for the furthest
/// look that needs no new slot.
const BISECTION_STEPS: usize = 12;

/// At or below this opacity a look draws no luminance to divide out of its
/// drawn state, so it keeps the luminance asked.
const NO_OPACITY: f32 = 1.0e-6;

/// A blended ghost's opacity draws as itself.
fn blended(opacity: f32) -> f32 {
    opacity
}

/// The opacity a ghost draws at under `transparency`: itself when blended,
/// the share of pixels the screen door keeps when dithered.
fn drawn_opacity(transparency: GhostTransparency) -> fn(f32) -> f32 {
    match transparency {
        GhostTransparency::Blend => blended,
        GhostTransparency::Dither => screen_door_coverage,
    }
}

/// What a ghost asks to present on a frame, and how it draws.
#[derive(Clone, Copy, Debug)]
struct AskedLook {
    look: GhostInstanceLook,
    transparency: GhostTransparency,
}

/// One frame's choices, as opacity and drawn tint luminance: from the exempt
/// ray's end (the asked opacity's fall at the held luminance) to the asked
/// look under the rate cap. A point between them draws on the straight line
/// between what the two draw.
#[derive(Clone, Copy, Debug)]
struct Segment {
    exempt: (f32, f32),
    capped: (f32, f32),
}

impl Segment {
    /// The look `tau` of the way from the exempt end to the capped one.
    fn at(&self, tau: f32) -> (f32, f32) {
        if tau == 0.0 {
            return self.exempt;
        }
        if tau == 1.0 {
            return self.capped;
        }
        let ((exempt_opacity, held), (capped_opacity, capped_luminance)) =
            (self.exempt, self.capped);
        let opacity = exempt_opacity + (capped_opacity - exempt_opacity) * tau;
        if held == capped_luminance {
            return (opacity, held);
        }
        let lit = exempt_opacity * held;
        let lit = lit + (capped_opacity * capped_luminance - lit) * tau;
        let luminance = if opacity > NO_OPACITY {
            (lit / opacity).clamp(0.0, 1.0)
        } else {
            capped_luminance
        };
        (opacity, luminance)
    }
}

/// What one frame of a ghost asks, as the clamp weighs it.
#[derive(Clone, Copy, Debug)]
struct FrameAsk {
    /// The asked tint, before the red rule.
    tint: [f32; 3],
    drawn_opacity: fn(f32) -> f32,
    segment: Segment,
    budget: Budget,
}

/// One ghost's look through the limiter, judged over every background it may
/// cover. Over background luminance B a ghost of albedo a draws a·p + B·q at
/// p = o·L and q = 1 − o, for presented opacity o (dithered, the screen
/// door's coverage) and drawn tint luminance L. The clamp tracks what draws
/// at 33 points on the outer edges of the (a, B) square, (1, k/16) and
/// (k/16, 1), with no dark test, and counts their transitions on one window
/// ([`EdgeTrackers::step`], [`GhostWindow`]). The tint's redness, weighted by
/// opacity, spends the same window under the screen channels' red rule.
///
/// Each frame:
///
/// - the asked look is rate-capped on the straight line in (p, q), so no
///   point draws faster than a screen channel ramps;
/// - the exempt ray, the asked opacity's fall at the held tint, always
///   presents: a ghost can always fade out or vanish, and its transitions share
///   one slot;
/// - under budget, the capped look presents if every transition it counts
///   joins a slot or opens one with room for one more; otherwise the furthest
///   look toward it that needs no new slot presents, its tint at that look's
///   luminance;
/// - over budget only the exempt ray moves, except that the change which
///   opened the latest slot may carry on the way it was going while every
///   transition joins a slot, so a strobe's last admitted change completes and
///   then rests;
/// - only the look that presents is judged.
///
/// The tint presents as asked, or at the judged luminance keeping its hue
/// angle, placed on the 8-bit grid the draw decodes; the luminance judged is
/// that grid tint's. Saturation passes through unlimited: it changes what
/// draws only where a coloured tint meets a coloured texture, which the clamp
/// cannot see.
///
/// # Six transitions over any background
///
/// From the frame its history starts, and across a drop at rest, no (a, B)
/// in [0, 1]² draws more than six transitions (a 0.1 swing whose darker side
/// is under 0.8) of what presents in any second:
///
/// 1. (a, B) is m·x for x on the outer edges, m = max(a, B), so it counts
///    what x would under the threshold T = 0.1/m ≥ 0.1 and dark limit 0.8/m.
///    x lies between neighbouring edge points y and z, nearer y: x = y +
///    λ(z − y), λ ≤ 1/2, where z − y moves by at most a sample step, so x's
///    swings are within half a step of y's.
/// 2. x counts on the first frame its swing reaches T while dark; the dark
///    test only admits values further on, so x passes every value of its
///    excursion there, moving its way, and so does y or z.
/// 3. No earlier frame of y's tracker excursion drew x as far: one before
///    x's excursion that did is followed by x going more than T back to its
///    excursion's start, and y, following within half a step, by a swing of
///    the edge threshold, which turns y's tracker. So on x's frame y passes
///    its peak; or stops short of it by at most half the spread's move, its
///    way, since y's reference frame (which drew y's peak); or sits inside a
///    smaller swing back that x never saw, within half a step of where that
///    swing started. The edge rules fire in each case, restarting the age of
///    the latest slot holding y or counting y anew, and leave y's last count
///    running x's way.
/// 4. So y counts between any two of x's transitions, and one tracker's
///    transitions sit in slots of their own: x's transitions inside one
///    second each hold a distinct slot, restarted on that transition's frame
///    and alive at the last one.
/// 5. The window never holds more than six edge slots.
/// 6. Across a drop. A ghost is dropped only at rest, nothing drawn and no
///    slot held, so x's last transition before it is more than a second old
///    and shares no second with one after. The clamp comes back with
///    unknown history ([`Self::appearing`]): each edge point counts its first
///    move after the drop as a transition its way, whatever its size. The
///    one transition the drop hides, x completing the swing it was in, falls
///    on a frame x reaches further its way than on any frame since it
///    rested. There y or z has moved and counted, and that count runs as in
///    step 3: carried on while it reaches further, counted again back near
///    where it rested. From then on steps 2 to 5 run as from a history
///    start. For the window the first moves change nothing: an appearance is
///    one step, so its first moves share one exempt slot; a point still since
///    the drop draws at the presented look what it drew with nothing drawn,
///    so the exempt ray, a lower opacity at that luminance, never moves it,
///    and from six edge slots a tracker still counts at most once on the
///    ray. The exempt ray still always fits, so a fade or vanish presents.
///
/// The thresholds leave room for the unorm16 opacity the draw packs and f32
/// rounding. Not covered across a drop: a sample whose first move is under
/// the edge threshold, once it has counted its swing back near where it
/// rested, counts nothing moving on that way short of the threshold, so a
/// hidden swing completing there restarts no slot on its frame; and a point
/// whose nearer sample has not moved since the drop (a white tint keeps the
/// corner (1, 1) still) leans on the farther one, whose slot the nearer
/// one's first count may later join. Targeted searches over both find no
/// seventh transition.
#[derive(Clone, Copy, Debug)]
struct LookValueClamp {
    window: GhostWindow,
    edges: EdgeTrackers,
    red: GhostRedness,
    /// The presented opacity. Dithered, the screen door draws its coverage.
    opacity: f32,
    /// The presented tint and the luminance it draws at.
    tint: [f32; 3],
    luminance: f32,
    /// The red-limited asked tint whose hue the presented tint keeps.
    hue: [f32; 3],
}

impl LookValueClamp {
    /// A clamp presenting `asked` with an empty window, its edge trackers
    /// started by `edges` at what `asked` draws.
    fn starting(asked: AskedLook, edges: fn(Drawn) -> EdgeTrackers) -> Self {
        let AskedLook { look, transparency } = asked;
        let opacity = unit_strength(look.opacity);
        let drawn = drawn_opacity(transparency)(opacity);
        let luminance = drawn_luminance(look.tint);
        Self {
            window: GhostWindow::default(),
            edges: edges(Drawn::of(drawn, luminance)),
            red: GhostRedness::fresh(drawn, look.tint),
            opacity,
            tint: look.tint,
            luminance,
            hue: look.tint,
        }
    }

    /// A clamp presenting `asked` whose earlier presentation it can't see:
    /// the screen showed states it never judged, so any point may be part way
    /// into a swing. Its edge trackers start with unknown history
    /// ([`EdgeTrackers::unknown_history`]), each counting its first move as a
    /// transition. The rule for any ghost the clamp lost sight of: one dropped
    /// at rest and appearing again today; a re-tenanted key, a reload or a
    /// join in the middle of a strobe later.
    ///
    /// Redness needs no such start while a ghost is dropped only at rest: its
    /// presented redness is then zero, the floor, so no swing it forgot can
    /// complete. A later caller presenting redness has to weigh that.
    fn with_unknown_history(asked: AskedLook) -> Self {
        Self::starting(asked, EdgeTrackers::unknown_history)
    }

    /// The asked `opacity` and drawn `luminance` under the straight-line rate
    /// cap from the presented look.
    fn capped(&self, opacity: f32, luminance: f32, cap: f32) -> (f32, f32) {
        let from = Drawn::of(self.opacity, self.luminance);
        let Some(scale) = rate_cap_scale(from, Drawn::of(opacity, luminance), cap) else {
            return (opacity, luminance);
        };
        if scale == 0.0 {
            return (self.opacity, self.luminance);
        }
        let capped_opacity = self.opacity + (opacity - self.opacity) * scale;
        let capped_luminance = if luminance == self.luminance {
            luminance
        } else if capped_opacity > NO_OPACITY {
            let lit = from.white_over_black + (opacity * luminance - from.white_over_black) * scale;
            (lit / capped_opacity).clamp(0.0, 1.0)
        } else {
            luminance
        };
        (capped_opacity, capped_luminance)
    }

    /// The tint that draws luminance `aimed` at the hue of `hue`, and the
    /// luminance it draws at: `hue` itself when it already draws at `aimed`,
    /// the presented tint while neither changed (so a held tint never drifts
    /// on the grid), else `hue` re-aimed and placed on the 8-bit grid.
    fn realize(&self, hue: [f32; 3], aimed: f32) -> ([f32; 3], f32) {
        if drawn_luminance(hue) == aimed {
            return (hue, aimed);
        }
        if hue == self.hue && aimed == self.luminance {
            return (self.tint, aimed);
        }
        on_grid_at_luminance(hue, aimed)
    }

    /// This clamp moved to the look `tau` along this frame's segment, and
    /// whether every transition it counts found a slot. Only that look is
    /// judged, since only it draws: at `tau` 0 the exempt ray's end, which
    /// always fits, else the look `tau` along, budgeted. A look that does not
    /// fit is partly moved, for the caller to discard.
    fn advance(&self, ask: &FrameAsk, tau: f32) -> (Self, bool) {
        let mut next = *self;
        if tau == 0.0 {
            // The exempt ray: the opacity's fall at the held luminance,
            // exactly, so it is monotone at every edge point. The held tint
            // presents unchanged; its redness falls with the opacity, which
            // the red rule counts as it would any fall.
            let (exempt_opacity, held) = ask.segment.exempt;
            let drawn = (ask.drawn_opacity)(exempt_opacity);
            let fits = next.edges.step(
                Drawn::of(drawn, held),
                SlotClass::Exempt,
                true,
                0,
                &mut next.window,
            );
            next.red.limit(&mut next.window, drawn, self.tint);
            next.opacity = exempt_opacity;
            return (next, fits);
        }
        let (opacity, aimed) = ask.segment.at(tau);
        let drawn = (ask.drawn_opacity)(opacity);
        let hue = next.red.limit(&mut next.window, drawn, ask.tint);
        let (tint, luminance) = self.realize(hue, aimed);
        let onward = if ask.budget == Budget::Continue {
            next.window.continuing()
        } else {
            0
        };
        if !next.edges.step(
            Drawn::of(drawn, luminance),
            SlotClass::Budgeted,
            ask.budget == Budget::Open,
            onward,
            &mut next.window,
        ) {
            return (next, false);
        }
        next.opacity = opacity;
        next.tint = tint;
        next.luminance = luminance;
        next.hue = hue;
        (next, true)
    }

    /// The furthest look along this frame's segment that fits: the capped
    /// look if it does, else a bisection between it and the exempt ray.
    fn furthest(&self, ask: &FrameAsk) -> Self {
        if ask.budget == Budget::ExemptOnly {
            return self.advance(ask, 0.0).0;
        }
        let (whole, fits) = self.advance(ask, 1.0);
        if fits {
            return whole;
        }
        let mut best = self.advance(ask, 0.0).0;
        let (mut low, mut high) = (0.0f32, 1.0f32);
        for _ in 0..BISECTION_STEPS {
            let mid = 0.5 * (low + high);
            let (next, fits) = self.advance(ask, mid);
            if fits {
                low = mid;
                best = next;
            } else {
                high = mid;
            }
        }
        best
    }
}

impl KeyClamp for LookValueClamp {
    type Value = AskedLook;

    /// A ghost already presenting as its history starts (the limiter turning
    /// on, or the clamp's first frame) starts against its look, counting
    /// nothing, as a screen channel does, so enabling never dips it. Nothing
    /// was presented under the limiter before, so there is no earlier swing
    /// to complete. A ghost the clamp dropped differs: the screen did show
    /// its earlier states, and the clamp forgot them ([`Self::appearing`]).
    fn fresh(asked: AskedLook) -> Self {
        Self::starting(asked, EdgeTrackers::fresh)
    }

    /// Appearing presents the asked look and counts it: a ghost that was not
    /// drawn changes what draws over every background as it appears, so its
    /// first second keeps only the budget the appearance leaves.
    ///
    /// A key appearing after history started may be one dropped at rest,
    /// which the container cannot tell from a new one, so it starts from
    /// nothing drawn with unknown history ([`Self::with_unknown_history`]).
    /// The appearance is every moving edge point's first move, however
    /// faint, so each counts it, all in one exempt slot, which an empty
    /// window always admits. A point the appearance leaves still counts its
    /// first move later, in whatever slot that step counts in.
    fn appearing(asked: AskedLook) -> Self {
        let AskedLook { look, transparency } = asked;
        let unseen = AskedLook {
            look: GhostInstanceLook {
                opacity: 0.0,
                ..look
            },
            transparency,
        };
        let mut clamp = Self::with_unknown_history(unseen);
        let opacity = unit_strength(look.opacity);
        let drawn = drawn_opacity(transparency)(opacity);
        // An empty window has room for a red onset, so the tint is unchanged.
        clamp.red.limit(&mut clamp.window, drawn, look.tint);
        let counted = clamp.edges.step(
            Drawn::of(drawn, clamp.luminance),
            SlotClass::Exempt,
            true,
            0,
            &mut clamp.window,
        );
        debug_assert!(counted, "an empty window admits an exempt slot");
        clamp.opacity = opacity;
        clamp
    }

    fn limit(&mut self, asked: AskedLook, frame: &LimiterFrameUniform) -> AskedLook {
        self.window.age(frame.dt_window);
        let AskedLook { look, transparency } = asked;
        let cap = INTENSITY_RATE_PER_SECOND * frame.dt_rate;
        let capped = self.capped(unit_strength(look.opacity), drawn_luminance(look.tint), cap);
        let ask = FrameAsk {
            tint: look.tint,
            drawn_opacity: drawn_opacity(transparency),
            segment: Segment {
                exempt: (self.opacity.min(capped.0), self.luminance),
                capped,
            },
            budget: self.window.budget(),
        };
        *self = self.furthest(&ask);
        AskedLook {
            look: GhostInstanceLook {
                opacity: self.opacity,
                saturation: look.saturation,
                tint: self.tint,
            },
            transparency,
        }
    }

    /// A vanished ghost's opacity falls to nothing at once at its held tint:
    /// the exempt ray, so the fall always presents, and it counts like any
    /// fall. Nothing draws in either transparency.
    fn absent(&mut self, frame: &LimiterFrameUniform) {
        let gone = AskedLook {
            look: GhostInstanceLook {
                opacity: 0.0,
                saturation: 0.0,
                tint: self.tint,
            },
            transparency: GhostTransparency::Blend,
        };
        // A rate allowance of a full blend: the clamp cannot hold back what
        // already vanished.
        let vanishing = LimiterFrameUniform {
            dt_rate: 1.0 / INTENSITY_RATE_PER_SECOND,
            ..*frame
        };
        self.limit(gone, &vanishing);
    }

    fn is_at_rest(&self) -> bool {
        self.window.is_empty() && self.opacity == 0.0 && self.red.presented() == 0.0
    }
}

/// Every ghost's look through the photosensitivity limiter, the one place a
/// ghost draw collector limits a look. Keys are a collector's ghost identity
/// (its link and generation-checked entity), so an entity seen through two
/// links is two ghosts with their own history.
///
/// Each frame: [`Self::begin_frame`], then [`Self::present`] once per
/// presented ghost (culled or not: the limiter judges the source, not its
/// framing), then [`Self::end_frame`], then [`Self::finish_presentation`] once
/// the renderer knows whether the frame presented. A frame begun with no
/// limiter input (a capture) passes every look through and leaves history
/// untouched. [`Self::set_transparency`] says how the ghosts draw, so a
/// dithered ghost is judged on the coverage its screen door presents.
///
/// What each ghost draws over any background, and its tint's redness, share
/// one flash window, so one ghost presents at most 3 flashes in any 1 s over
/// any background, and over budget it rests at a held look. A ghost not
/// presented on a frame vanished: its opacity falls to nothing, which always
/// presents and counts once. A ghost appearing presents as asked and counts
/// its appearance the same way, however faint, since it may be one whose
/// history was dropped at rest; one already presenting when the limiter
/// turns on starts against its look and counts nothing.
#[derive(Clone, Debug)]
pub struct GhostLookClamp<K> {
    /// Every ghost's look clamp.
    looks: KeyedClamp<K, LookValueClamp>,
    /// `looks` as the unsettled frame began, restored if it never presents.
    /// Overwritten in place each frame. Both tables start allocated at the
    /// same size and only swap, so the snapshot allocates only when `looks`
    /// outgrows it.
    frame_start: KeyedClamp<K, LookValueClamp>,
    /// The limiter input of the frame begun and not yet settled.
    unsettled: Option<LimiterFrameUniform>,
    /// Presented time of frames that never presented, which the next limited
    /// frame takes on.
    unconsumed: Option<LimiterFrameInput>,
    /// This frame runs the limiter: it was begun with a limiter input.
    limiting: bool,
    /// How these ghosts draw translucent.
    transparency: GhostTransparency,
}

impl<K> Default for GhostLookClamp<K> {
    fn default() -> Self {
        Self {
            looks: KeyedClamp::default(),
            frame_start: KeyedClamp::default(),
            unsettled: None,
            unconsumed: None,
            limiting: false,
            transparency: GhostTransparency::default(),
        }
    }
}

impl<K: Copy + Eq + Hash> GhostLookClamp<K> {
    /// How the ghosts this clamp limits draw translucent (`render.ghosts`;
    /// billboard ghosts always blend). Blend until set.
    pub fn set_transparency(&mut self, transparency: GhostTransparency) {
        self.transparency = transparency;
    }

    /// Start a frame. `frame` is this frame's limiter input (the ghost ramp's
    /// presented-frame time and the fail-safe enable flag), or `None` for a
    /// capture, which never involves the limiter. Time from earlier frames
    /// that never presented merges into it. A frame begun before the last one
    /// settled settles that one as presented.
    pub fn begin_frame(&mut self, frame: Option<&LimiterFrameUniform>) {
        self.limiting = frame.is_some();
        self.unsettled = None;
        let Some(frame) = frame else {
            return;
        };
        let frame = match self.unconsumed.take() {
            Some(unconsumed) => pack_limiter_frame(
                unconsumed.merge(LimiterFrameInput {
                    elapsed_seconds: frame.dt_window,
                }),
                frame.enabled,
                frame.init,
            ),
            None => *frame,
        };
        self.frame_start.clone_from(&self.looks);
        self.looks.begin_frame(&frame);
        self.unsettled = Some(frame);
    }

    /// The look ghost `key` presents this frame, from its unclamped look
    /// (`opacity` already the link's opacity times membership): judged over
    /// every background as [`GhostLookClamp`] describes, its tint at the
    /// presented luminance keeping its hue, and over budget a red tint
    /// desaturated at unchanged luminance. Saturation passes through.
    pub fn present(&mut self, key: K, look: GhostInstanceLook) -> GhostInstanceLook {
        if !self.limiting {
            return look;
        }
        let asked = AskedLook {
            look,
            transparency: self.transparency,
        };
        self.looks.limit(key, asked).look
    }

    /// End the frame: a ghost not presented this frame vanished. Its opacity
    /// falls to nothing, counted once, and its history is kept until its clamp
    /// is at rest.
    pub fn end_frame(&mut self) {
        if !self.limiting {
            return;
        }
        self.looks.end_frame();
    }

    /// Settle the frame last begun once the renderer knows whether it
    /// presented. A frame that presented nothing (its surface acquire failed)
    /// never consumes its presented time: every ghost's clamp returns to where
    /// the frame began, and its time merges into the next limited frame's, as
    /// the resolve's limiter input does.
    pub fn finish_presentation(&mut self, presented: bool) {
        let Some(frame) = self.unsettled.take() else {
            return;
        };
        if presented {
            return;
        }
        std::mem::swap(&mut self.looks, &mut self.frame_start);
        // The frame's time already holds any earlier unconsumed time.
        self.unconsumed = Some(LimiterFrameInput {
            elapsed_seconds: frame.dt_window,
        });
    }
}

#[cfg(test)]
#[path = "ghost_look_tests.rs"]
mod tests;
