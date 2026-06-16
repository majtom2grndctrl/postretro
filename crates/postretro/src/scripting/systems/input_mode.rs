// App-side input-mode tracker for the engine-owned `input.mode` slot.
// Observes the input phase's mode signals — mouse motion (→ `pointer`) and
// stick/D-pad/nav-key input (→ `focus`) — debounces them so jitter doesn't flap
// the mode, and writes the resolved `input.mode` enum slot. The store write is
// app composition, NOT a subsystem output: the input subsystem's contract output
// stays the action snapshot.
// See: context/lib/input.md §5, §7 · context/lib/scripting.md §5

use crate::input::InputMode;
use crate::scripting::ctx::ScriptCtx;
use crate::scripting::primitives::store::write_store_slot;
use crate::scripting::slot_table::SlotValue;

/// Dotted name of the engine-owned input-mode slot.
const MODE_SLOT: &str = "input.mode";

/// Debounce window (seconds): a fresh signal for the OPPOSITE mode must persist
/// (accumulate this much frame time without the current mode's signal resetting
/// it) before the mode flips. Short enough to feel instant on a real device
/// switch, long enough that a stray mouse micro-jitter mid-nav (or a stick
/// twitch mid-mouse) doesn't flap the cursor/ring. Tuned by feel, not measured.
const DEBOUNCE_SECS: f32 = 0.08;

/// The mode-change signal observed during the input phase. Mouse motion votes
/// for `Pointer`; any nav input (stick edge, D-pad, nav key) votes for `Focus`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ModeSignal {
    /// The mouse moved this frame — vote for `Pointer`.
    MouseMotion,
    /// Stick/D-pad/nav-key input this frame — vote for `Focus`.
    NavInput,
}

impl ModeSignal {
    /// The mode this signal votes for.
    fn mode(self) -> InputMode {
        match self {
            ModeSignal::MouseMotion => InputMode::Pointer,
            ModeSignal::NavInput => InputMode::Focus,
        }
    }
}

/// Tracks the debounced pointer-vs-focus interaction mode and publishes it to
/// the `input.mode` slot. Holds a clone of the engine's `ScriptCtx` (cheap `Rc`
/// bump). The App feeds it the frame's mode signal (if any) plus the frame
/// delta each input phase; it returns the resolved [`InputMode`] (also driving
/// `App::ui_input_mode`) and writes the slot only when the published mode
/// changes (no per-frame store churn).
///
/// Mode is observation-only: the App applies it to cursor visibility / the
/// focus ring ONLY while a capturing tree is on the stack (the mode is inert
/// otherwise). This tracker does not know about the stack — it just resolves the
/// mode; the App gates the effect.
pub(crate) struct InputModeTracker {
    ctx: ScriptCtx,
    /// The currently published mode. Seeded to the slot's default (`Focus`).
    current: InputMode,
    /// A pending opposite-mode candidate accumulating debounce time, or `None`
    /// when the latest signal agreed with `current` (or no signal yet).
    pending: Option<(InputMode, f32)>,
    /// True once the slot has been written at least once, so the first resolved
    /// frame publishes the seed even if it equals the default.
    wrote_once: bool,
}

impl InputModeTracker {
    /// Build a tracker holding a clone of the engine's `ScriptCtx`, seeded to the
    /// `Focus` default (matching the `input.mode` slot's declared default).
    pub(crate) fn new(ctx: ScriptCtx) -> Self {
        Self {
            ctx,
            current: InputMode::default(),
            pending: None,
            wrote_once: false,
        }
    }

    /// Reset to the `Focus` default and drop any pending candidate, re-seeding
    /// the slot on the next `update`. Called on level load so a mid-transition
    /// mode never bleeds across levels.
    pub(crate) fn reset(&mut self) {
        self.current = InputMode::default();
        self.pending = None;
        self.wrote_once = false;
    }

    /// The currently published mode. The App reads `update`'s return value
    /// directly, so this accessor is only used by tests.
    #[cfg(test)]
    pub(crate) fn mode(&self) -> InputMode {
        self.current
    }

    /// Advance the tracker by `dt` (seconds) given the frame's mode signal (or
    /// `None` when neither mouse motion nor nav input occurred). Returns the
    /// resolved mode and publishes `input.mode` when it changes.
    ///
    /// Debounce: a signal that agrees with the current mode clears any pending
    /// flip (a brief opposite twitch is overridden by sustained same-mode
    /// input). A signal for the opposite mode accumulates `dt` against
    /// [`DEBOUNCE_SECS`]; only once that window elapses does the mode flip. With
    /// no signal, a pending candidate keeps accumulating (so a single decisive
    /// device-switch press still flips after the window even if the user then
    /// holds still).
    pub(crate) fn update(&mut self, signal: Option<ModeSignal>, dt: f32) -> InputMode {
        let before = self.current;
        match signal.map(ModeSignal::mode) {
            Some(voted) if voted == self.current => {
                // Sustained agreement with the current mode cancels any pending
                // flip — the opposite twitch was transient jitter.
                self.pending = None;
            }
            Some(voted) => {
                // A vote for the opposite mode: start or extend its debounce.
                let elapsed = match self.pending {
                    Some((pending_mode, acc)) if pending_mode == voted => acc + dt,
                    _ => dt,
                };
                if elapsed >= DEBOUNCE_SECS {
                    self.current = voted;
                    self.pending = None;
                } else {
                    self.pending = Some((voted, elapsed));
                }
            }
            None => {
                // No signal this frame: let a decisive pending flip keep aging so
                // a one-shot device switch still resolves.
                if let Some((pending_mode, acc)) = self.pending {
                    let elapsed = acc + dt;
                    if elapsed >= DEBOUNCE_SECS {
                        self.current = pending_mode;
                        self.pending = None;
                    } else {
                        self.pending = Some((pending_mode, elapsed));
                    }
                }
            }
        }

        // Publish only on a transition (or the very first resolved frame), so the
        // store is touched on mode changes, not every frame in the hot path.
        if !self.wrote_once || self.current != before {
            self.write(self.current);
            self.wrote_once = true;
        }
        self.current
    }

    /// Write `input.mode` via the engine write path (bypasses readonly, validates
    /// the enum). An error here would be an engine bug (the slot is engine-owned
    /// and always declared), so it is logged rather than skipped.
    fn write(&self, mode: InputMode) {
        if let Err(err) = write_store_slot(
            &self.ctx,
            MODE_SLOT,
            SlotValue::Enum(mode.wire_name().to_string()),
        ) {
            log::warn!("[InputMode] failed to write `{MODE_SLOT}`: {err}");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scripting::primitives::store::read_store_slot;

    fn read_mode(ctx: &ScriptCtx) -> String {
        match read_store_slot(ctx, MODE_SLOT).unwrap() {
            SlotValue::Enum(value) => value,
            other => panic!("input.mode should be an Enum, got {other:?}"),
        }
    }

    #[test]
    fn mouse_motion_switches_to_pointer_after_debounce() {
        // A mouse-motion signal votes for `Pointer`; the mode flips only once the
        // debounce window has elapsed, not on the first jittery frame.
        let ctx = ScriptCtx::new();
        let mut tracker = InputModeTracker::new(ctx.clone());

        // One short frame of mouse motion: not enough to flip yet.
        assert_eq!(
            tracker.update(Some(ModeSignal::MouseMotion), 0.02),
            InputMode::Focus,
            "a single sub-window mouse frame must not flip the mode",
        );
        // Sustained mouse motion past the window flips to Pointer.
        tracker.update(Some(ModeSignal::MouseMotion), 0.05);
        assert_eq!(
            tracker.update(Some(ModeSignal::MouseMotion), 0.05),
            InputMode::Pointer,
            "sustained mouse motion past the debounce window flips to pointer",
        );
    }

    #[test]
    fn nav_input_switches_to_focus_after_debounce() {
        let ctx = ScriptCtx::new();
        let mut tracker = InputModeTracker::new(ctx.clone());
        // Drive to Pointer first.
        for _ in 0..3 {
            tracker.update(Some(ModeSignal::MouseMotion), 0.05);
        }
        assert_eq!(tracker.mode(), InputMode::Pointer);

        // A single nav frame under the window does not flip.
        assert_eq!(
            tracker.update(Some(ModeSignal::NavInput), 0.02),
            InputMode::Pointer,
        );
        // Sustained nav past the window flips back to Focus.
        tracker.update(Some(ModeSignal::NavInput), 0.05);
        assert_eq!(
            tracker.update(Some(ModeSignal::NavInput), 0.05),
            InputMode::Focus,
        );
    }

    #[test]
    fn brief_opposite_jitter_does_not_flap_the_mode() {
        // A stray single-frame mouse twitch while navigating with the stick must
        // NOT flap the mode: sustained nav input cancels the pending pointer flip.
        let ctx = ScriptCtx::new();
        let mut tracker = InputModeTracker::new(ctx.clone());
        assert_eq!(tracker.mode(), InputMode::Focus);

        // A one-frame mouse twitch starts a pending Pointer flip...
        assert_eq!(
            tracker.update(Some(ModeSignal::MouseMotion), 0.02),
            InputMode::Focus,
        );
        // ...but the next nav frame cancels it, so the mode never flipped.
        assert_eq!(
            tracker.update(Some(ModeSignal::NavInput), 0.02),
            InputMode::Focus,
            "sustained nav cancels a brief pointer twitch — no flap",
        );
        // Even continuing mouse motion now must re-accumulate from scratch.
        assert_eq!(
            tracker.update(Some(ModeSignal::MouseMotion), 0.02),
            InputMode::Focus,
        );
    }

    #[test]
    fn published_slot_follows_the_resolved_mode() {
        // The App publishes the slot on each transition; verify the enum string
        // written matches the resolved mode.
        let ctx = ScriptCtx::new();
        let mut tracker = InputModeTracker::new(ctx.clone());

        // First update seeds the slot with the default (focus).
        tracker.update(None, 0.016);
        assert_eq!(read_mode(&ctx), "focus");

        // Drive to Pointer; `update` itself publishes the transition.
        for _ in 0..3 {
            tracker.update(Some(ModeSignal::MouseMotion), 0.05);
        }
        assert_eq!(tracker.mode(), InputMode::Pointer);
        assert_eq!(read_mode(&ctx), "pointer");
    }

    #[test]
    fn reset_restores_focus_default() {
        let ctx = ScriptCtx::new();
        let mut tracker = InputModeTracker::new(ctx.clone());
        for _ in 0..3 {
            tracker.update(Some(ModeSignal::MouseMotion), 0.05);
        }
        assert_eq!(tracker.mode(), InputMode::Pointer);
        tracker.reset();
        assert_eq!(tracker.mode(), InputMode::Focus);
    }
}
