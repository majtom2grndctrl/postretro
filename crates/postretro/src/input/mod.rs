// Input subsystem: action mapping, binding resolution, per-frame snapshots.
// See: context/lib/input.md

mod activation;
mod activator;
#[cfg(test)]
mod activator_tests;
mod author_layer;
#[cfg(test)]
mod author_layer_tests;
mod binding_capture;
mod binding_state;
mod binding_table;
#[cfg(test)]
mod binding_table_tests;
mod bindings;
mod commands;
pub use activation::ActivationInputCapture;
pub mod cursor;
mod defaults;
mod device_family;
pub mod diagnostics;
mod focus;
mod glyphs;
pub mod gamepad;
mod input_names;
mod latch;
mod look;
mod player_rows;
mod rebind;
mod relevance;
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
mod ui_nav_map;
pub use ui_nav_map::UiNavContext;
mod wieldable_selection;

pub use author_layer::author_layer_from_block;
pub use binding_capture::{BindingCapture, CaptureTarget};
pub use binding_state::{BindingSources, BindingState};
pub use binding_table::{AuthorLayer, EffectiveTable, GlyphDirs, PlayerLayer};
#[cfg(test)]
pub use binding_table::{AuthorBinding, CommandPresentation};
pub use commands::{Command, CommandContext};
pub use input_names::{input_label, input_name};
pub use rebind::{RebindProposal, propose_rebind, reset_command};
pub use relevance::Relevance;
pub use defaults::default_bindings;
pub use device_family::{DeviceFamily, DeviceFamilyTracker};
pub use glyphs::{GlyphView, glyph_key, resolve_glyph};
pub use diagnostics::{DiagnosticAction, DiagnosticInputs, default_diagnostic_chords};
pub use focus::InputFocus;
pub use input_names::DeviceClass;
pub use latch::GameplayInputLatch;
pub use look::DEFAULT_GAMEPAD_LOOK_SENSITIVITY;
pub use look::LookInputs;
pub use player_rows::player_layer_from_rows;
pub use relevance::RelevanceFacts;
pub(crate) use scroll::wheel_diagnostics_enabled;
pub use snapshot::ActionSnapshot;
pub use system::{DEFAULT_GAMEPAD_LOOK_DEAD_ZONE, DEFAULT_MOUSE_SENSITIVITY, InputSystem};
pub use types::{Action, ButtonState, PhysicalInput};
#[allow(unused_imports)]
pub use types::{Activator, ActivatorKind, DEFAULT_ACTIVATOR_THRESHOLD};
// Outside `input/`, only tests name the binding vocabulary today.
#[cfg(test)]
pub use types::{AxisSource, Binding};
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
pub use ui_nav::{NavIntent, StickNavTrackers, TextEntryKey, text_entry_key};
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
pub use ui_focus::{FocusTickResult, InputMode, UiFocusEngine, capture_slider_step, slider_value};
