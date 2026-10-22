// Per-ghost look clamp: each ghost's opacity, tint redness, drawn brightness and
// darkening on one flash window per ghost, judged at the worst background.
// See: context/lib/rendering_pipeline.md §7.8 (Photosensitivity limiter), §7.9

use std::hash::Hash;

use super::ghost_tint::{INVISIBLE_OPACITY, drawn_luminance, presented_tint};
use super::keyed::{KeyClamp, KeyedClamp};
use super::value::{DarkState, Decided, Judging, Redness, Strength, as_is, vanishing};
use super::window::{FlashWindow, Part};
use crate::flash_limiter::{LimiterFrameInput, LimiterFrameUniform, pack_limiter_frame};
use crate::ghost_instance::{GhostInstanceLook, SCREEN_DOOR_PIXELS, screen_door_coverage};
use crate::ghost_transparency::GhostTransparency;

/// How far a part's decision may sit from the look that presents it and still
/// be that look: the f32 rounding of an opacity split into brightness and
/// darkening.
const RESOLVE_EPSILON: f32 = 1.0e-6;

/// The opacity a ghost presents as under `transparency`: itself when blended,
/// the share of pixels the screen door keeps when dithered.
fn presents(transparency: GhostTransparency) -> fn(f32) -> f32 {
    match transparency {
        GhostTransparency::Blend => as_is,
        GhostTransparency::Dither => screen_door_coverage,
    }
}

/// `opacity` moved toward `from` onto a value `transparency` presents exactly:
/// itself when blended, a whole sixteenth when dithered.
fn presentable(opacity: f32, from: f32, transparency: GhostTransparency) -> f32 {
    match transparency {
        GhostTransparency::Blend => opacity,
        GhostTransparency::Dither => {
            let steps = opacity * SCREEN_DOOR_PIXELS as f32;
            let kept = if from < opacity {
                steps.floor()
            } else {
                steps.ceil()
            };
            kept / SCREEN_DOOR_PIXELS as f32
        }
    }
}

/// What a ghost asks to present on a frame, and how it draws.
#[derive(Clone, Copy, Debug)]
struct AskedLook {
    look: GhostInstanceLook,
    transparency: GhostTransparency,
}

/// One frame's decisions for a ghost's look, before it presents.
#[derive(Clone, Copy, Debug)]
struct Decisions {
    opacity: Decided,
    red: f32,
    /// The tint, red-limited.
    tint: [f32; 3],
    /// That tint's luminance as drawn.
    luminance: f32,
    brightness: Decided,
    darkening: Decided,
}

impl Decisions {
    /// Whether every luminance part decided what `opacity` and `brightness`
    /// present.
    fn presents(&self, opacity: f32, brightness: f32) -> bool {
        let near = |decided: f32, presented: f32| (decided - presented).abs() <= RESOLVE_EPSILON;
        near(self.opacity.level, opacity)
            && near(self.brightness.level, brightness)
            && near(self.darkening.level, opacity - brightness)
    }
}

/// One ghost's look through the limiter, judged at the worst background. Over
/// a background of luminance B a ghost of albedo a draws B(1 − o) + a·o·L at
/// presented opacity o and drawn tint luminance L. That is bilinear in a and
/// B, so the largest change sits at a corner, and the clamp judges all three
/// that change:
///
/// - brightness, o·L: a white ghost over black;
/// - opacity, o: a black ghost over white, which draws 1 − o, so a swing's
///   darker state is on either side;
/// - darkening, o·(1 − L): a white ghost over white, which draws one minus it.
///
/// Those three and the tint's redness (weighted by opacity) share one merging
/// flash window: at most three flashes a second across all of them, and one
/// change in what draws counts once though its parts cross the threshold on
/// different frames. Dithered, opacity is judged as the screen door's coverage.
/// Each part keeps its own rate cap.
///
/// Brightness and darkening sum to opacity, so the three can't move freely:
/// over budget the clamp presents the look closest to the asked one that keeps
/// every part no further than it decided. Opacity yields only when brightness
/// and darkening together can't follow it, and the tint then presents the
/// brightness closest to its asked luminance. Failing that, the look holds.
#[derive(Clone, Copy, Debug)]
struct LookValueClamp {
    window: FlashWindow,
    opacity: Strength,
    red: Redness,
    brightness: Strength,
    darkening: Strength,
}

impl LookValueClamp {
    /// Decide every part for asking `opacity` and `tint`, with `brightness`
    /// asked at the decided opacity in place of the tint's when given. Nothing
    /// presents.
    fn decide(
        &mut self,
        opacity: f32,
        brightness: Option<f32>,
        tint: [f32; 3],
        transparency: GhostTransparency,
        frame: &LimiterFrameUniform,
    ) -> Decisions {
        let presents = presents(transparency);
        let (opacity_from, ramp_from) = (self.opacity.presented(), self.opacity.ramp());
        let brightness_from = self.brightness.presented();
        let darkening_from = self.darkening.presented();
        // Which way each luminance part moves, so a transition never joins a
        // slot whose parts are moving back.
        let trying = presents(self.opacity.capped(opacity, 0.0, frame));
        let trying_brightness = brightness.unwrap_or(trying * drawn_luminance(tint));
        self.window.moving(Part::Level, trying - opacity_from);
        self.window.moving(Part::Brightness, trying_brightness - brightness_from);
        self.window.moving(Part::Darkening, trying - trying_brightness - darkening_from);

        let level = self.opacity.decide(
            &mut self.window,
            Part::Level,
            opacity,
            Judging {
                dark: DarkState::Either,
                presents,
                shift: 0.0,
            },
            frame,
        );
        let (red, tint) = self.red.decide(&mut self.window, level.level, tint);
        let luminance = drawn_luminance(tint);
        let asked_brightness = brightness.unwrap_or(level.level * luminance);
        let asked_darkening = level.level - asked_brightness;
        self.window.moving(Part::Level, level.level - opacity_from);
        self.window.moving(Part::Brightness, asked_brightness - brightness_from);
        self.window.moving(Part::Darkening, asked_darkening - darkening_from);
        // A screen-door step carries brightness and darkening with it at the
        // tint's last luminance, past what the opacity's own rate cap moved.
        let stepped = (level.level - opacity_from) - (level.ramp - ramp_from);
        let last_luminance = if opacity_from > INVISIBLE_OPACITY {
            (brightness_from / opacity_from).clamp(0.0, 1.0)
        } else {
            luminance
        };
        let brightness = self.brightness.decide(
            &mut self.window,
            Part::Brightness,
            asked_brightness,
            Judging {
                shift: last_luminance * stepped,
                ..Judging::OVER_DARK
            },
            frame,
        );
        let darkening = self.darkening.decide(
            &mut self.window,
            Part::Darkening,
            asked_darkening,
            Judging {
                dark: DarkState::Stronger,
                presents: as_is,
                shift: (1.0 - last_luminance) * stepped,
            },
            frame,
        );
        Decisions {
            opacity: level,
            red,
            tint,
            luminance,
            brightness,
            darkening,
        }
    }

    /// The opacity and brightness that present every part no further from
    /// where it last presented than it decided: the decided opacity, unless
    /// brightness and darkening together can't reach it (then the nearest
    /// opacity they can, on a value the transparency presents exactly), and
    /// within that the brightness closest to the tint's at that opacity.
    fn resolve(&self, decided: &Decisions, transparency: GhostTransparency) -> (f32, f32) {
        let span = |from: f32, to: f32| (from.min(to), from.max(to));
        let opacity_from = self.opacity.presented();
        let (opacity_low, opacity_high) = span(opacity_from, decided.opacity.level);
        let (brightness_low, brightness_high) =
            span(self.brightness.presented(), decided.brightness.level);
        let (darkening_low, darkening_high) =
            span(self.darkening.presented(), decided.darkening.level);
        let reachable = decided
            .opacity
            .level
            .max(brightness_low + darkening_low)
            .min(brightness_high + darkening_high);
        let opacity = if (reachable - decided.opacity.level).abs() <= RESOLVE_EPSILON {
            decided.opacity.level
        } else {
            presentable(reachable, opacity_from, transparency)
                .max(opacity_low)
                .min(opacity_high)
        };
        let brightness = (opacity * decided.luminance)
            .max(brightness_low.max(opacity - darkening_high))
            .min(brightness_high.min(opacity - darkening_low));
        (opacity, brightness)
    }

    /// Hold opacity, brightness and darkening where they last presented,
    /// limiting redness at that opacity.
    fn hold(&mut self, tint: [f32; 3]) -> Decisions {
        let held = |part: &Strength| Decided {
            level: part.presented(),
            ramp: part.ramp(),
        };
        let opacity = held(&self.opacity);
        let (red, tint) = self.red.decide(&mut self.window, opacity.level, tint);
        Decisions {
            opacity,
            red,
            tint,
            luminance: drawn_luminance(tint),
            brightness: held(&self.brightness),
            darkening: held(&self.darkening),
        }
    }

    /// Whether each part holds its onsets: opacity, redness, brightness,
    /// darkening.
    fn suppressing(&self) -> [bool; 4] {
        [
            self.opacity.suppressing(),
            self.red.suppressing(),
            self.brightness.suppressing(),
            self.darkening.suppressing(),
        ]
    }

    /// Hold the onsets of each part `suppressing` names.
    fn keep_suppressing(&mut self, suppressing: [bool; 4]) {
        let [opacity, red, brightness, darkening] = suppressing;
        self.opacity.keep_suppressing(opacity);
        self.red.keep_suppressing(red);
        self.brightness.keep_suppressing(brightness);
        self.darkening.keep_suppressing(darkening);
    }

    fn present(&mut self, decided: &Decisions) {
        self.opacity.present(decided.opacity);
        self.red.present(decided.red);
        self.brightness.present(decided.brightness);
        self.darkening.present(decided.darkening);
    }
}

impl KeyClamp for LookValueClamp {
    type Value = AskedLook;

    fn fresh(asked: AskedLook) -> Self {
        let AskedLook { look, transparency } = asked;
        let mut opacity = Strength::default();
        opacity.restart_presenting(look.opacity, presents(transparency));
        let level = opacity.presented();
        let mut red = Redness::default();
        red.restart(level, look.tint);
        let lit = level * drawn_luminance(look.tint);
        let mut brightness = Strength::default();
        brightness.restart(lit);
        let mut darkening = Strength::default();
        darkening.restart(level - lit);
        Self {
            window: FlashWindow::merging(),
            opacity,
            red,
            brightness,
            darkening,
        }
    }

    /// Every part is decided on the window first: opacity, then redness at the
    /// decided opacity (over budget a red tint desaturates at unchanged
    /// luminance), then brightness and darkening at that opacity. When those
    /// decisions can't all present at once, the look that can is decided
    /// again as itself, so the window counts the transitions that present
    /// and no others, and the onsets the first decisions refused stay
    /// refused; failing that, the look holds. Saturation passes through
    /// unlimited: it changes what draws only where a coloured tint meets a
    /// coloured texture, which the clamp cannot see.
    fn limit(&mut self, asked: AskedLook, frame: &LimiterFrameUniform) -> AskedLook {
        self.window.age(frame.dt_window);
        let AskedLook { look, transparency } = asked;
        let start = *self;
        let mut decided = self.decide(look.opacity, None, look.tint, transparency, frame);
        let (opacity, brightness) = start.resolve(&decided, transparency);
        if !decided.presents(opacity, brightness) {
            // An onset refused against the asked look stays refused: the look
            // that presents holds it.
            let refused = self.suppressing();
            *self = start;
            let asked_opacity = if opacity == decided.opacity.level {
                look.opacity
            } else {
                opacity
            };
            decided = self.decide(asked_opacity, Some(brightness), look.tint, transparency, frame);
            if !decided.presents(opacity, brightness) {
                *self = start;
                decided = self.hold(look.tint);
            }
            self.keep_suppressing(refused);
        }
        self.present(&decided);
        let opacity = decided.opacity.level;
        AskedLook {
            look: GhostInstanceLook {
                opacity,
                saturation: look.saturation,
                tint: presented_tint(
                    decided.tint,
                    opacity,
                    decided.luminance,
                    decided.brightness.level,
                ),
            },
            transparency,
        }
    }

    /// A vanished ghost's opacity falls to nothing and that fall counts. Its
    /// brightness and darkening fall with it on the same frame, the same
    /// change, so they count once. Nothing presents in either transparency.
    fn absent(&mut self, frame: &LimiterFrameUniform) {
        let gone = GhostInstanceLook {
            opacity: 0.0,
            saturation: 0.0,
            tint: [0.0; 3],
        };
        self.limit(
            AskedLook {
                look: gone,
                transparency: GhostTransparency::Blend,
            },
            &vanishing(frame),
        );
    }

    fn is_at_rest(&self) -> bool {
        self.window.is_empty()
            && self.opacity.presented() == 0.0
            && self.red.presented() == 0.0
            && self.brightness.presented() == 0.0
            && self.darkening.presented() == 0.0
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
/// Each ghost's opacity, tint redness, drawn brightness and darkening share
/// one flash window, so one ghost presents at most 3 flashes in any 1 s over
/// any background. A ghost not presented on a frame vanished: everything it
/// drew falls to nothing together, and that fall counts once.
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
    /// (`opacity` already the link's opacity times membership).
    ///
    /// Opacity, the tint's redness, the drawn brightness and the darkening
    /// are limited on one window. Over budget a red tint desaturates at
    /// unchanged luminance and the tint presents the held brightness at the
    /// presented opacity, keeping its hue. Saturation passes through
    /// unlimited: it changes what draws only where a coloured tint meets a
    /// coloured texture, which the clamp cannot see.
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

    /// End the frame: a ghost not presented this frame vanished. Everything
    /// it drew falls to nothing, counted once, and its history is kept until
    /// its clamp is at rest.
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
