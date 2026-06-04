// UI-dispatch seam: decides whether an input event is consumed by the UI layer
// or forwarded to the gameplay input system, ahead of the gameplay forward.
// See: context/lib/input.md

//! Input-stage tap point the UI layer owns, sitting between raw input collection
//! and gameplay forwarding — mirroring the `egui_consumed` gate in
//! `App::window_event`. The capture decision is sourced from the active UI
//! descriptor's capture mode via `Renderer::splash_capture_mode`; the splash
//! descriptor installs `Passthrough` so the seam is inert against gameplay while
//! the splash is shown. A capturing UI (future menu or modal) drives `Capture`.
//!
//! `InputFocus::Menu` is the intended *structural* home for UI capture — the
//! gate a future menu/modal system flips. The current splash makes no live focus
//! change and enters no `Menu` state; the capture/passthrough decision is the
//! mode flag at this seam alone.
//!
//! ## N→N+1 ordering contract
//!
//! A UI-consumed event on frame N must not reach game logic before frame N+1.
//! Captured events are pushed onto a pending queue ([`UiDispatch::pending`]).
//! Each Game-logic phase reads the already-promoted captures with
//! [`UiDispatch::take_ready`] and *then* calls [`UiDispatch::advance_frame`] to
//! promote this frame's `pending` into `ready` for the next frame. Because the
//! read happens before the promotion, an event captured during frame N's Input
//! stage is only promoted by frame N's `advance_frame` and first becomes
//! readable at frame N+1's `take_ready` — there is deliberately no same-frame
//! path from capture to game-logic visibility, independent of how winit
//! interleaves input events with the redraw. The intent vocabulary is a reserved
//! seam concern; the current implementation carries an opaque marker so the
//! ordering is provable without pinning the vocabulary.

/// Whether the active UI layer captures input events or lets them pass through to
/// gameplay. The active UI descriptor sets the mode via `set_mode`; the splash
/// descriptor sources the value from `Renderer::splash_capture_mode`.
///
/// The splash is non-interactive, so it installs `Passthrough` — the seam is
/// inert against gameplay while the splash is shown. A capturing UI (future
/// menu or modal) drives `Capture` to queue events for next-frame game logic.
#[cfg_attr(not(test), allow(dead_code))]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum UiCaptureMode {
    /// UI consumes events; they are queued for next-frame game logic and NOT
    /// forwarded to the gameplay input system this frame.
    Capture,
    /// UI ignores events; they flow through to the gameplay input system as if
    /// the seam were absent. The inert default: with no UI layer active, gameplay
    /// forwarding behaves exactly as before this seam existed.
    #[default]
    Passthrough,
}

/// The outcome of dispatching one event through the seam.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UiDispatchOutcome {
    /// Event was consumed by the UI layer. It must NOT be forwarded to the
    /// gameplay input system this frame; it is queued for next-frame game logic.
    Captured,
    /// Event is not the UI layer's; forward it to the gameplay input system per
    /// the existing focus gate.
    Forward,
}

impl UiDispatchOutcome {
    /// True when the gameplay input system should still receive this event.
    pub fn forwards_to_gameplay(self) -> bool {
        matches!(self, UiDispatchOutcome::Forward)
    }
}

/// An opaque marker for a UI-captured event awaiting next-frame delivery. The
/// payload is a monotonically rising sequence number identifying which event was
/// captured, sufficient to prove the ordering contract without pinning the full
/// intent vocabulary.
///
/// No production consumer reads `seq` today — queued intents are drained and
/// dropped at the game-logic seam; the stamp is asserted on by the ordering
/// test. A future menu/modal path will define and consume the real vocabulary.
#[cfg_attr(not(test), allow(dead_code))]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct UiIntent {
    /// Per-dispatch sequence number, assigned in capture order.
    pub seq: u64,
}

/// Input-stage UI dispatch state: the capture/passthrough mode plus the
/// pending-intent queue that enforces the N→N+1 ordering contract.
///
/// Frame lifecycle (driven by the App, one cycle per rendered frame):
/// 1. During the Input stage, [`dispatch_event`](Self::dispatch_event) classifies
///    each event; captures land in `pending`.
/// 2. At the start of the Game-logic phase, [`take_ready`](Self::take_ready)
///    hands game logic the captures promoted by the *previous* frame, then
///    [`advance_frame`](Self::advance_frame) promotes this frame's `pending`
///    into `ready` for the next frame.
///
/// Because the read precedes the promotion, an event captured during frame N's
/// Input stage is never visible to frame N's game logic — it first surfaces at
/// frame N+1's `take_ready`.
#[derive(Debug, Default)]
pub struct UiDispatch {
    /// Capture/passthrough mode for the active UI layer. Set by the active UI
    /// descriptor via `set_mode`; sourced from `Renderer::splash_capture_mode`.
    mode: UiCaptureMode,

    /// Captures recorded during the current frame's Input stage. Promoted to
    /// `ready` only on the next frame's `advance_frame`.
    pending: Vec<UiIntent>,

    /// Captures promoted from a prior frame, awaiting the current frame's
    /// `take_ready`. Separated from `pending` so the same-frame captures never
    /// leak into this frame's game logic.
    ready: Vec<UiIntent>,

    /// Monotonic sequence stamp for captured events.
    next_seq: u64,
}

impl UiDispatch {
    pub fn new() -> Self {
        Self::default()
    }

    /// Set the active capture/passthrough mode. Called from `App::paint_splash`
    /// with the value from `Renderer::splash_capture_mode` — the splash is
    /// non-interactive, so it stays `Passthrough` and the seam is inert against
    /// gameplay; the seam tests drive the `Capture` path with synthetic events.
    pub fn set_mode(&mut self, mode: UiCaptureMode) {
        self.mode = mode;
    }

    /// The active capture/passthrough mode. Reserved seam API: retained alongside
    /// `set_mode` for the future descriptor/Goal-F consumer.
    #[allow(dead_code)]
    pub fn mode(&self) -> UiCaptureMode {
        self.mode
    }

    /// Classify one Input-stage event. In `Capture` mode the event is consumed
    /// (queued for next-frame game logic, NOT forwarded this frame); in
    /// `Passthrough` mode it is forwarded to the gameplay input system.
    ///
    /// This is the pure dispatch decision — no window or GPU state — so it is
    /// drivable by synthetic events in tests.
    pub fn dispatch_event(&mut self) -> UiDispatchOutcome {
        match self.mode {
            UiCaptureMode::Capture => {
                let seq = self.next_seq;
                self.next_seq = self.next_seq.wrapping_add(1);
                self.pending.push(UiIntent { seq });
                UiDispatchOutcome::Captured
            }
            UiCaptureMode::Passthrough => UiDispatchOutcome::Forward,
        }
    }

    /// Promote captures recorded since the last call into the ready set. Called
    /// once per Game-logic phase, AFTER `take_ready`, so a capture from the
    /// current frame's Input stage is promoted now but only read next frame.
    /// This is the N→N+1 boundary: pending (this frame) becomes ready (read by
    /// next frame's `take_ready`).
    pub fn advance_frame(&mut self) {
        self.ready.append(&mut self.pending);
    }

    /// Drain the intents that are visible to game logic this frame. Empty unless
    /// a prior frame captured events and `advance_frame` has since promoted them.
    /// Called BEFORE `advance_frame` so this frame's own captures are not yet
    /// visible.
    pub fn take_ready(&mut self) -> Vec<UiIntent> {
        std::mem::take(&mut self.ready)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A capture-marked event reports `Captured` and must not forward to the
    /// gameplay input system; a passthrough event reports `Forward` and does.
    #[test]
    fn dispatch_captures_event_in_capture_mode_and_forwards_in_passthrough() {
        let mut dispatch = UiDispatch::new();

        dispatch.set_mode(UiCaptureMode::Capture);
        let captured = dispatch.dispatch_event();
        assert_eq!(captured, UiDispatchOutcome::Captured);
        assert!(
            !captured.forwards_to_gameplay(),
            "a captured event must not reach the gameplay input system",
        );

        dispatch.set_mode(UiCaptureMode::Passthrough);
        let forwarded = dispatch.dispatch_event();
        assert_eq!(forwarded, UiDispatchOutcome::Forward);
        assert!(
            forwarded.forwards_to_gameplay(),
            "a passthrough event must reach the gameplay input system",
        );
    }

    /// An event captured during frame N's Input stage is absent from frame N's
    /// game logic and present on frame N+1 — never same-frame. Simulated across
    /// two frames in the exact order the App's frame loop runs the seam: the
    /// Input stage captures, then the Game-logic phase reads (`take_ready`)
    /// before promoting (`advance_frame`).
    #[test]
    fn captured_event_reaches_game_logic_no_earlier_than_next_frame() {
        let mut dispatch = UiDispatch::new();
        dispatch.set_mode(UiCaptureMode::Capture);

        // --- Frame N: Input stage captures one event. ---
        let outcome = dispatch.dispatch_event();
        assert_eq!(outcome, UiDispatchOutcome::Captured);

        // --- Frame N: Game-logic phase. Read happens before promotion, so the
        // event captured this same frame is NOT yet visible. ---
        let frame_n_visible = dispatch.take_ready();
        assert!(
            frame_n_visible.is_empty(),
            "frame N's capture must not become visible within frame N",
        );
        dispatch.advance_frame();

        // --- Frame N+1: no new captures. Game-logic phase reads the frame-N
        // capture, which the previous frame's `advance_frame` promoted. ---
        let frame_n_plus_1_visible = dispatch.take_ready();
        assert_eq!(
            frame_n_plus_1_visible.len(),
            1,
            "the frame-N capture is delivered on frame N+1",
        );
        dispatch.advance_frame();

        // --- Frame N+2: the intent was drained, not redelivered. ---
        let frame_n_plus_2_visible = dispatch.take_ready();
        assert!(
            frame_n_plus_2_visible.is_empty(),
            "a delivered capture is not re-delivered on later frames",
        );
    }
}
