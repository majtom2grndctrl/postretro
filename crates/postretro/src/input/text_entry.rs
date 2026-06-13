// Text-entry intent resolution (M13 Text-Entry, Task 3): turns the drained UI
// intents into edit/commit/cancel decisions against the open text-entry surface.
// Pure CPU — no App state, no GPU — so the printable→append, backspace→delete,
// and Enter→commit / Escape→cancel contract is unit-testable. The App applies the
// returned decisions: edits ride Task 1's text-edit command path, commit fires the
// opener's `on_commit` then pops, cancel pops only.
// See: context/lib/input.md §7

use super::ui_dispatch::{UiIntent, UiIntentPayload};
use super::ui_nav::NavIntent;

/// One resolved text-entry action, produced in capture order from the drained
/// intents while a text-entry tree is open. The App maps each to a side effect.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TextEntryEdit {
    /// Append the captured text to the bound slot (`Text` intent).
    Append(String),
    /// Delete the last grapheme from the bound slot (`Backspace` intent).
    Backspace,
}

/// The terminal disposition of a text-entry resolution pass: whether the entry
/// committed (Enter / `done`), cancelled (Escape), or stayed open. Commit and
/// cancel are terminal — once one fires, no further intents are resolved (the tree
/// is about to pop), so the App pops exactly once per pass.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TextEntryDisposition {
    /// No commit or cancel this pass; the entry stays open.
    Open,
    /// `nav.confirm` (Enter / the `done` key) fired: the App fires the opener's
    /// `on_commit`, then pops the tree.
    Commit,
    /// `nav.cancel` (Escape) fired: the App pops the tree without firing `on_commit`.
    Cancel,
}

/// The result of resolving one frame's drained intents against the open
/// text-entry surface: the ordered edits to apply to the bound slot, and the
/// terminal disposition (open / commit / cancel).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TextEntryResolution {
    /// Edits in capture order, applied before a commit/cancel acts on the slot.
    pub edits: Vec<TextEntryEdit>,
    /// Whether the entry committed, cancelled, or stayed open this pass.
    pub disposition: TextEntryDisposition,
}

impl TextEntryResolution {
    /// True when a commit or cancel was consumed — the App filters those
    /// confirm/cancel intents out of the focus engine and skips the pause-menu path.
    pub fn consumed_commit_or_cancel(&self) -> bool {
        !matches!(self.disposition, TextEntryDisposition::Open)
    }
}

/// Resolve the drained `ui_intents` against an open text-entry surface. Walks the
/// intents in capture order: each `Text` becomes an `Append` edit, each
/// `Backspace` a `Backspace` edit, a `nav.confirm` commits and a `nav.cancel`
/// cancels — and commit/cancel are terminal (resolution stops, since the tree is
/// about to pop). Directional / next-prev nav and pointer clicks are left for the
/// focus engine (the on-screen keyboard still navigates between keys).
pub fn resolve_text_entry(ui_intents: &[UiIntent]) -> TextEntryResolution {
    let mut edits = Vec::new();
    let mut disposition = TextEntryDisposition::Open;
    for intent in ui_intents {
        match &intent.payload {
            UiIntentPayload::Text(text) => edits.push(TextEntryEdit::Append(text.clone())),
            UiIntentPayload::Backspace => edits.push(TextEntryEdit::Backspace),
            UiIntentPayload::Nav(NavIntent::Confirm) => {
                disposition = TextEntryDisposition::Commit;
                break;
            }
            UiIntentPayload::Nav(NavIntent::Cancel) => {
                disposition = TextEntryDisposition::Cancel;
                break;
            }
            _ => {}
        }
    }
    TextEntryResolution { edits, disposition }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn intent(seq: u64, payload: UiIntentPayload) -> UiIntent {
        UiIntent { seq, payload }
    }

    #[test]
    fn printable_text_resolves_to_append_in_capture_order() {
        // Two typed characters become two Append edits in capture order; the entry
        // stays open (no terminal disposition).
        let intents = [
            intent(0, UiIntentPayload::Text("a".to_string())),
            intent(1, UiIntentPayload::Text("B".to_string())),
        ];
        let r = resolve_text_entry(&intents);
        assert_eq!(
            r.edits,
            vec![
                TextEntryEdit::Append("a".to_string()),
                TextEntryEdit::Append("B".to_string()),
            ],
        );
        assert_eq!(r.disposition, TextEntryDisposition::Open);
        assert!(!r.consumed_commit_or_cancel());
    }

    #[test]
    fn backspace_resolves_to_a_backspace_edit() {
        let intents = [intent(0, UiIntentPayload::Backspace)];
        let r = resolve_text_entry(&intents);
        assert_eq!(r.edits, vec![TextEntryEdit::Backspace]);
        assert_eq!(r.disposition, TextEntryDisposition::Open);
    }

    #[test]
    fn confirm_commits_and_is_terminal() {
        // Edits before the confirm apply; the confirm commits and stops resolution,
        // so a trailing intent after it is ignored (the tree is about to pop).
        let intents = [
            intent(0, UiIntentPayload::Text("x".to_string())),
            intent(1, UiIntentPayload::Nav(NavIntent::Confirm)),
            intent(2, UiIntentPayload::Text("y".to_string())),
        ];
        let r = resolve_text_entry(&intents);
        assert_eq!(r.edits, vec![TextEntryEdit::Append("x".to_string())]);
        assert_eq!(r.disposition, TextEntryDisposition::Commit);
        assert!(r.consumed_commit_or_cancel());
    }

    #[test]
    fn cancel_cancels_and_is_terminal() {
        let intents = [
            intent(0, UiIntentPayload::Text("z".to_string())),
            intent(1, UiIntentPayload::Nav(NavIntent::Cancel)),
        ];
        let r = resolve_text_entry(&intents);
        assert_eq!(r.edits, vec![TextEntryEdit::Append("z".to_string())]);
        assert_eq!(r.disposition, TextEntryDisposition::Cancel);
        assert!(r.consumed_commit_or_cancel());
    }

    #[test]
    fn directional_nav_and_clicks_are_left_for_the_focus_engine() {
        // A directional nav and a pointer click produce no edits and no terminal
        // disposition — the focus engine resolves them (key navigation).
        let intents = [
            intent(0, UiIntentPayload::Nav(NavIntent::Down)),
            intent(
                1,
                UiIntentPayload::PointerClick {
                    pos: super::super::ui_dispatch::PointerPos { x: 1.0, y: 2.0 },
                },
            ),
        ];
        let r = resolve_text_entry(&intents);
        assert!(r.edits.is_empty());
        assert_eq!(r.disposition, TextEntryDisposition::Open);
        assert!(!r.consumed_commit_or_cancel());
    }
}
