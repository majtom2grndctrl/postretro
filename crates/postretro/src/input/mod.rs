// Input subsystem: action mapping, binding resolution, per-frame snapshots.
// See: context/lib/input.md

mod activation;
mod bindings;
pub use activation::ActivationInputCapture;
pub mod cursor;
mod defaults;
pub mod diagnostics;
mod focus;
pub mod gamepad;
mod latch;
mod look;
mod scroll;
mod snapshot;
mod system;
#[cfg(test)]
mod system_tests;
mod text_entry;
mod types;
mod ui_dispatch;
mod ui_focus;
mod ui_nav;
mod wieldable_selection;

pub use defaults::default_bindings;
pub use diagnostics::{DiagnosticAction, DiagnosticInputs, default_diagnostic_chords};
pub use focus::InputFocus;
pub use latch::GameplayInputLatch;
pub use look::LookInputs;
pub(crate) use scroll::wheel_diagnostics_enabled;
pub use snapshot::ActionSnapshot;
pub use system::{DEFAULT_MOUSE_SENSITIVITY, InputSystem};
pub use types::{Action, ButtonState};
// Outside `input/`, only tests name the binding vocabulary today.
#[cfg(test)]
pub use types::{AxisSource, Binding, PhysicalInput};
pub use wieldable_selection::WieldableSelectionPolicy;
// `UiCaptureMode` is the capture/passthrough mode flag, driven by the active
// gameplay UI descriptor via `UiDispatch::set_mode`. The boot splash leaves the
// default `Passthrough`.
pub use ui_dispatch::{UiCaptureMode, UiDispatch};
// `UiDispatchOutcome` is `dispatch_event`'s per-event return type. `UiIntent`
// is the queued kinded capture; `UiIntentPayload`/`PointerPos` are its payload
// vocabulary. The modal stack (M13 Goal F) consumes the queued intents.
#[allow(unused_imports)]
pub use ui_dispatch::{PointerPos, UiDispatchOutcome, UiIntent, UiIntentPayload};
// Nav-intent vocabulary plus the action→intent mapping the input stage feeds
// into `UiDispatch`. `StickNavTracker` does stick-past-deadzone edge detection.
#[allow(unused_imports)]
pub use ui_nav::{
    NavIntent, StickNavTracker, TextEntryKey, nav_intent_for_gamepad_button, nav_intent_for_key,
    text_entry_key,
};
// Text-entry intent resolution (M13 Text-Entry, Task 3): drained intents →
// edit/commit/cancel decisions against the open text-entry surface.
#[allow(unused_imports)]
pub use text_entry::{
    TextEntryDisposition, TextEntryEdit, TextEntryResolution, escape_is_dev_quit_chord,
    resolve_text_entry,
};
// App-side focus engine (M13 Goal F, Task 3): consumes nav intents + cursor, moves
// focus through the renderer's exported focus rect list, runs the dt-clocked
// hold-to-repeat timer, and reports the focused id back for the focus ring.
#[allow(unused_imports)]
pub use ui_focus::{FocusTickResult, InputMode, UiFocusEngine, capture_slider_step};
