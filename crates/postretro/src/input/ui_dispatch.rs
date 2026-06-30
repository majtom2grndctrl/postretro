// UI-dispatch seam: decides whether an input event is consumed by the UI layer
// or forwarded to the gameplay input system, ahead of the gameplay forward.
// See: context/lib/input.md

//! Input-stage tap point the UI layer owns, sitting between raw input collection
//! and gameplay forwarding — mirroring the `egui_consumed` gate in
//! `App::window_event`. The capture decision is sourced from the active UI
//! descriptor's capture mode. The modal stack drives `Capture` from the top
//! tree (via `App::reconcile_ui_focus`). The boot splash is renderer-owned and
//! never touches this seam: it leaves the default `Passthrough` in place, so the
//! seam is inert against gameplay while the splash is shown.
//!
//! `InputFocus::Menu` is the intended *structural* home for UI capture — the
//! gate a menu/modal system flips. The boot splash makes no live focus change
//! and enters no `Menu` state; the capture/passthrough decision is the mode flag
//! at this seam alone.
//!
//! ## Intent vocabulary
//!
//! Captured events queue a kinded [`UiIntent`]: a [`NavIntent`] for directional
//! navigation/confirm/cancel/menu, a [`UiIntent::PointerClick`] for a mouse
//! click at a device-pixel position, or a [`UiIntent::Text`] for typed text.
//! `Text` is defined here so there is one queue and one ordering contract, but
//! it is produced only by the text-entry plan — the nav-intent stage never emits
//! it. The action→intent mapping (which keys/buttons/stick edges become which
//! [`NavIntent`]) lives in [`super::ui_nav`]; this module owns the queue and the
//! per-capture sequence stamp.
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
//! interleaves input events with the redraw. All intent sources, gamepad
//! included, must enqueue before the frame's `take_ready`/`advance_frame` pair so
//! they share this contract.

use super::ui_nav::NavIntent;
use postretro_scripting_core::ui::descriptor::CaptureMode;

/// Whether the active UI layer captures input events or lets them pass through to
/// gameplay. The active gameplay UI descriptor sets the mode via `set_mode`. The
/// boot splash is renderer-owned and leaves the default `Passthrough` — it never
/// drives this seam. A capturing UI (menu or modal) drives `Capture` to queue
/// events for next-frame game logic.
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

/// Resolve a descriptor-side `CaptureMode` (wire/envelope form) into the input
/// subsystem's `UiCaptureMode` (the seam form the modal stack drives). The two
/// enums are kept separate so the descriptor module carries no input dependency.
impl From<CaptureMode> for UiCaptureMode {
    fn from(mode: CaptureMode) -> Self {
        match mode {
            CaptureMode::Capture => UiCaptureMode::Capture,
            CaptureMode::Passthrough => UiCaptureMode::Passthrough,
        }
    }
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

/// A position in device pixels, top-left origin, as winit reports cursor and
/// click coordinates. Carried by [`UiIntent::PointerClick`] for hit-testing.
#[cfg_attr(not(test), allow(dead_code))]
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PointerPos {
    pub x: f64,
    pub y: f64,
}

/// What a UI-captured event means to the UI layer. Carried inside [`UiIntent`]
/// once stamped with a sequence number.
///
/// `Text` is part of the closed payload so the queue and ordering contract are
/// defined in one place, but it is produced only by the text-entry plan; the
/// nav-intent stage emits `Nav` and `PointerClick` only.
#[cfg_attr(not(test), allow(dead_code))]
#[derive(Debug, Clone, PartialEq)]
pub enum UiIntentPayload {
    /// Directional / activation navigation, gamepad-first.
    Nav(NavIntent),
    /// A pointer click at a device-pixel position (hit-tested by the focus
    /// engine, Task 3).
    PointerClick { pos: PointerPos },
    /// Typed text. Produced only while a text-entry tree is the top of the modal
    /// stack: the input stage maps a printable, non-control `KeyEvent.text` to
    /// this. The focus-resolution stage turns it into an `AppendText` edit against
    /// the tree's `text_entry_target` slot.
    Text(String),
    /// A logical Backspace delete inside text entry (M13 Text-Entry, Task 3).
    /// Produced from the logical Backspace KEY — never from `KeyEvent.text` (some
    /// platforms deliver Backspace as `\u{8}` text, which must NOT route through
    /// the `Text` channel). The focus-resolution stage turns it into a
    /// `BackspaceText` edit against the tree's `text_entry_target` slot.
    Backspace,
}

/// A UI-captured event awaiting next-frame delivery: the kinded payload plus a
/// monotonically rising sequence number assigned in capture order. The `seq`
/// stamp identifies which event was captured and underpins the ordering
/// contract's test assertions; game logic consumes the `payload`.
#[cfg_attr(not(test), allow(dead_code))]
#[derive(Debug, Clone, PartialEq)]
pub struct UiIntent {
    /// Per-dispatch sequence number, assigned in capture order.
    pub seq: u64,
    /// What the captured event means to the UI layer.
    pub payload: UiIntentPayload,
}

/// Input-stage UI dispatch state: the capture/passthrough mode plus the
/// pending-intent queue that enforces the N→N+1 ordering contract.
///
/// Frame lifecycle (driven by the App, one cycle per rendered frame):
/// 1. During the Input stage, [`dispatch_event`](Self::dispatch_event) classifies
///    each event; captures carrying a payload land in `pending`.
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
    /// Capture/passthrough mode for the active UI layer. Set by the active
    /// gameplay UI descriptor via `set_mode`; the boot splash leaves the default
    /// `Passthrough` and never drives it.
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

    /// Set the active capture/passthrough mode. Called from
    /// `App::reconcile_ui_focus` (modal-stack top → `Capture`). The boot splash
    /// does not call this — it leaves the default `Passthrough`, so the seam is
    /// inert against gameplay while the splash is shown.
    pub fn set_mode(&mut self, mode: UiCaptureMode) {
        self.mode = mode;
    }

    /// The active capture/passthrough mode. Reserved seam API: retained alongside
    /// `set_mode` for the modal-stack consumer.
    #[allow(dead_code)]
    pub fn mode(&self) -> UiCaptureMode {
        self.mode
    }

    /// Classify one Input-stage event. In `Capture` mode the event is consumed
    /// (NOT forwarded to gameplay this frame) and, when `intent` is `Some`, the
    /// kinded payload is stamped and queued for next-frame game logic. In
    /// `Passthrough` mode it is forwarded and nothing is queued.
    ///
    /// `intent` is `None` for events that the UI must swallow but that carry no
    /// queueable meaning — a raw mouse-motion delta, or a key the nav vocabulary
    /// ignores. Capturing them still suppresses the gameplay forward; they just
    /// add nothing to the queue.
    ///
    /// This is the pure dispatch decision — no window or GPU state — so it is
    /// drivable by synthetic events in tests.
    pub fn dispatch_event(&mut self, intent: Option<UiIntentPayload>) -> UiDispatchOutcome {
        match self.mode {
            UiCaptureMode::Capture => {
                if let Some(payload) = intent {
                    let seq = self.next_seq;
                    self.next_seq = self.next_seq.wrapping_add(1);
                    self.pending.push(UiIntent { seq, payload });
                }
                UiDispatchOutcome::Captured
            }
            UiCaptureMode::Passthrough => UiDispatchOutcome::Forward,
        }
    }

    /// Enqueue an already-resolved intent that did not arrive through the
    /// `dispatch_event` capture gate — gamepad nav, polled in the input stage
    /// rather than delivered as a winit event. The poll runs only while a
    /// capturing tree owns input, so the caller has already made the capture
    /// decision; this shares the same `pending` queue and sequence stamp so
    /// gamepad intents ride the identical N→N+1 contract.
    #[cfg_attr(not(test), allow(dead_code))]
    pub fn enqueue_intent(&mut self, payload: UiIntentPayload) {
        let seq = self.next_seq;
        self.next_seq = self.next_seq.wrapping_add(1);
        self.pending.push(UiIntent { seq, payload });
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
        let captured = dispatch.dispatch_event(Some(UiIntentPayload::Nav(NavIntent::Confirm)));
        assert_eq!(captured, UiDispatchOutcome::Captured);
        assert!(
            !captured.forwards_to_gameplay(),
            "a captured event must not reach the gameplay input system",
        );

        dispatch.set_mode(UiCaptureMode::Passthrough);
        let forwarded = dispatch.dispatch_event(Some(UiIntentPayload::Nav(NavIntent::Confirm)));
        assert_eq!(forwarded, UiDispatchOutcome::Forward);
        assert!(
            forwarded.forwards_to_gameplay(),
            "a passthrough event must reach the gameplay input system",
        );
    }

    /// Capturing an event with no queueable payload (e.g. a raw mouse delta or a
    /// non-nav key) still suppresses the gameplay forward but queues nothing.
    #[test]
    fn capture_without_payload_suppresses_forward_but_queues_nothing() {
        let mut dispatch = UiDispatch::new();
        dispatch.set_mode(UiCaptureMode::Capture);

        let outcome = dispatch.dispatch_event(None);
        assert_eq!(outcome, UiDispatchOutcome::Captured);

        dispatch.advance_frame();
        assert!(
            dispatch.take_ready().is_empty(),
            "a payload-less capture must not enqueue an intent",
        );
    }

    /// An event captured during frame N's Input stage is absent from frame N's
    /// game logic and present on frame N+1 — never same-frame. Simulated across
    /// two frames in the exact order the App's frame loop runs the seam: the
    /// Input stage captures, then the Game-logic phase reads (`take_ready`)
    /// before promoting (`advance_frame`). Uses a real `Nav` intent so the
    /// payload — not just the ordering — is exercised end to end.
    #[test]
    fn captured_nav_intent_reaches_game_logic_no_earlier_than_next_frame() {
        let mut dispatch = UiDispatch::new();
        dispatch.set_mode(UiCaptureMode::Capture);

        // --- Frame N: Input stage captures one nav intent. ---
        let outcome = dispatch.dispatch_event(Some(UiIntentPayload::Nav(NavIntent::Down)));
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
        // capture, which the previous frame's `advance_frame` promoted, and the
        // kinded payload arrives intact. ---
        let frame_n_plus_1_visible = dispatch.take_ready();
        assert_eq!(
            frame_n_plus_1_visible.len(),
            1,
            "the frame-N capture is delivered on frame N+1",
        );
        assert_eq!(
            frame_n_plus_1_visible[0].payload,
            UiIntentPayload::Nav(NavIntent::Down),
            "the kinded nav payload survives the N→N+1 hop",
        );
        dispatch.advance_frame();

        // --- Frame N+2: the intent was drained, not redelivered. ---
        let frame_n_plus_2_visible = dispatch.take_ready();
        assert!(
            frame_n_plus_2_visible.is_empty(),
            "a delivered capture is not re-delivered on later frames",
        );
    }

    /// Gamepad intents enqueued via `enqueue_intent` ride the same N→N+1 queue
    /// as keyboard captures: an intent enqueued before the `take_ready` /
    /// `advance_frame` pair first surfaces on the next frame.
    #[test]
    fn enqueued_gamepad_intent_shares_the_n_plus_1_contract() {
        let mut dispatch = UiDispatch::new();
        dispatch.set_mode(UiCaptureMode::Capture);

        // --- Frame N: gamepad poll (pre-promotion) enqueues a nav intent. ---
        dispatch.enqueue_intent(UiIntentPayload::Nav(NavIntent::Right));

        let frame_n_visible = dispatch.take_ready();
        assert!(
            frame_n_visible.is_empty(),
            "an enqueued gamepad intent must not be visible the same frame",
        );
        dispatch.advance_frame();

        // --- Frame N+1: the gamepad intent is delivered. ---
        let frame_n_plus_1_visible = dispatch.take_ready();
        assert_eq!(frame_n_plus_1_visible.len(), 1);
        assert_eq!(
            frame_n_plus_1_visible[0].payload,
            UiIntentPayload::Nav(NavIntent::Right),
        );
    }

    /// A printable key typed while text entry is open resolves to a `Text` intent
    /// and is CAPTURED — never forwarded to the gameplay input system — and it
    /// reaches game logic no earlier than the next frame (the N→N+1 contract). This
    /// is the seam-level guarantee that captured keystrokes can't leak to game
    /// logic while a text-entry modal is open. The key→intent resolution mirrors the
    /// App's input stage: `text_entry_key` maps the logical key + text, then the
    /// `Text`/`Backspace` payload is dispatched through this seam.
    #[test]
    fn text_entry_keystrokes_are_captured_not_forwarded_and_obey_n_plus_1() {
        use super::super::ui_nav::{TextEntryKey, text_entry_key};
        use winit::keyboard::{Key, NamedKey};

        let mut dispatch = UiDispatch::new();
        dispatch.set_mode(UiCaptureMode::Capture);

        // --- Frame N Input stage: a printable 'a' and a Backspace key, resolved
        // exactly as the App's keyboard handler does, are dispatched through the
        // seam. Both are CAPTURED (not forwarded to gameplay) and queue a payload. ---
        let append = match text_entry_key(&Key::Character("a".into()), Some("a")) {
            Some(TextEntryKey::Append(s)) => UiIntentPayload::Text(s),
            other => panic!("expected Append, got {other:?}"),
        };
        let outcome = dispatch.dispatch_event(Some(append));
        assert_eq!(outcome, UiDispatchOutcome::Captured);
        assert!(
            !outcome.forwards_to_gameplay(),
            "a typed character must NOT reach the gameplay input system",
        );

        let backspace = match text_entry_key(&Key::Named(NamedKey::Backspace), Some("\u{8}")) {
            Some(TextEntryKey::Backspace) => UiIntentPayload::Backspace,
            other => panic!("expected Backspace, got {other:?}"),
        };
        let outcome = dispatch.dispatch_event(Some(backspace));
        assert!(
            !outcome.forwards_to_gameplay(),
            "a Backspace keystroke must NOT reach the gameplay input system",
        );

        // --- Frame N Game-logic phase: the read precedes the promotion, so this
        // frame's captures are NOT visible to frame N's game logic. ---
        assert!(
            dispatch.take_ready().is_empty(),
            "frame N's text keystrokes must not be visible within frame N",
        );
        dispatch.advance_frame();

        // --- Frame N+1: the captured keystrokes surface intact, in capture order. ---
        let ready = dispatch.take_ready();
        assert_eq!(ready.len(), 2, "both captured keystrokes arrive on N+1");
        assert_eq!(ready[0].payload, UiIntentPayload::Text("a".to_string()));
        assert_eq!(ready[1].payload, UiIntentPayload::Backspace);
    }

    /// Sequence numbers are assigned in capture order across both enqueue paths
    /// so the relative order of keyboard and gamepad intents in a frame is
    /// recoverable.
    #[test]
    fn sequence_numbers_rise_across_dispatch_and_enqueue() {
        let mut dispatch = UiDispatch::new();
        dispatch.set_mode(UiCaptureMode::Capture);

        dispatch.dispatch_event(Some(UiIntentPayload::Nav(NavIntent::Up)));
        dispatch.enqueue_intent(UiIntentPayload::Nav(NavIntent::Down));
        dispatch.advance_frame();

        let ready = dispatch.take_ready();
        assert_eq!(ready.len(), 2);
        assert!(
            ready[0].seq < ready[1].seq,
            "capture order must be preserved by rising seq stamps",
        );
    }
}
