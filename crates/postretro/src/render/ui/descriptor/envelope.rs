// Top-level placement envelope (`AnchoredTree`) wrapping the root widget, plus the
// `CaptureMode` it declares: anchor/offset placement, input-capture behavior,
// initial focus, and the text-entry target slot.
// See: context/lib/ui.md §1

use serde::{Deserialize, Serialize};

use super::super::layout::Anchor;
use super::Widget;

/// Whether a tree captures input (freezing gameplay + lower trees and releasing
/// the cursor) or passes it through to gameplay. Declared on the `AnchoredTree`
/// envelope so a JSON-authored tree states its own behavior. `Passthrough` is the
/// default — a HUD never captures, and an omitted `captureMode` keeps the pre-F
/// wire form byte-identical (`skip_serializing_if` below). The app-side modal
/// stack reads the TOP tree's mode to drive the input-dispatch seam and focus.
///
/// This is the descriptor/wire twin of `input::UiCaptureMode`; the modal stack
/// converts one to the other via `into()`. Kept separate so the descriptor module
/// carries no input-subsystem dependency.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum CaptureMode {
    /// Tree consumes input; gameplay + lower trees freeze, cursor releases.
    Capture,
    /// Tree ignores input; events flow through to gameplay (HUD behavior).
    #[default]
    Passthrough,
}

impl CaptureMode {
    /// True for the default `Passthrough`. Used by `skip_serializing_if` so a
    /// passthrough tree omits the `captureMode` key (pre-F wire compatibility).
    fn is_passthrough(&self) -> bool {
        matches!(self, CaptureMode::Passthrough)
    }
}

/// Top-level placement envelope wrapping the root widget. `anchor`/`offset`
/// live ONLY here, not on widget variants: a widget tree is placed once, as a
/// whole, against the logical-reference canvas (see `layout::Anchor`). `offset`
/// is logical-reference px, `[x, y]` (+x right, +y down), matching
/// `UiElement::offset`.
///
/// `capture_mode` declares whether the tree captures input or passes it through;
/// it defaults to `Passthrough` and skip-serializes when passthrough, so a
/// pre-F descriptor (no `captureMode` key) round-trips byte-identically. The
/// modal stack reads the TOP tree's mode to drive the input seam.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AnchoredTree {
    pub anchor: Anchor,
    pub offset: [f32; 2],
    pub root: Widget,
    /// Input capture behavior; defaults to `Passthrough`. Skip-serialized when
    /// passthrough so a HUD/pre-F tree omits the key entirely (wire-identical).
    #[serde(default, skip_serializing_if = "CaptureMode::is_passthrough")]
    pub capture_mode: CaptureMode,
    /// Authored id of the node focus starts on when this tree becomes the top of
    /// the modal stack (M13 Goal F, Task 3). Absent selects the first focusable
    /// node in tree order. Skip-serialized when absent so a pre-F tree omits it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub initial_focus: Option<String>,
    /// Writable String slot this tree's text entry edits (M13 Text-Entry, Task 3).
    /// `Some(slot)` is the "text entry is open" condition for the TOP tree: while
    /// it is set, the input stage routes hardware key text events into edit
    /// reactions against this slot (append / backspace), and Enter/Escape
    /// commit/cancel the entry. Absent on every non-text-entry tree, so a tree
    /// without text entry omits the key entirely (skip-serialized) and round-trips
    /// byte-identically.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub text_entry_target: Option<String>,
}

impl AnchoredTree {
    /// Build a passthrough-mode tree (the HUD/splash default). Most programmatic
    /// trees never capture, so this keeps their construction terse and lets the
    /// `capture_mode` field be added without touching every call site.
    #[cfg_attr(not(test), allow(dead_code))]
    pub fn passthrough(anchor: Anchor, offset: [f32; 2], root: Widget) -> Self {
        Self {
            anchor,
            offset,
            root,
            capture_mode: CaptureMode::Passthrough,
            initial_focus: None,
            text_entry_target: None,
        }
    }
}
