// On-screen keyboard shortcuts: `text_backspace`, `text_space`, and
// `text_commit` activate the keyboard tree's own keys without moving focus,
// so they act through whatever the tree authors.
// See: context/lib/input.md §7 · context/lib/ui.md §4

use crate::input::{Command, UiIntent, UiIntentPayload};
use crate::*;

/// The keyboard-tree key each shortcut activates. A reskinned keyboard keeps
/// these ids for the shortcuts to reach its keys.
pub(crate) fn shortcut_key_id(command: Command) -> Option<&'static str> {
    match command {
        Command::TextBackspace => Some("key_backspace"),
        Command::TextSpace => Some("key_space"),
        Command::TextCommit => Some("key_done"),
        _ => None,
    }
}

impl App {
    /// Resolve this frame's queued shortcuts and a held backspace's repeat.
    /// Runs after text-entry commit and cancel: with no text-entry tree on
    /// top, shortcuts do nothing (P26), and a commit earlier in the batch ends
    /// the rest of it.
    pub(crate) fn apply_text_shortcuts(&mut self, ui_intents: &[UiIntent], dt: f32) {
        for intent in ui_intents {
            let UiIntentPayload::TextShortcut(command) = intent.payload else {
                continue;
            };
            let Some(key_id) = shortcut_key_id(command) else {
                continue;
            };
            if self.activate_text_key(key_id) {
                let Some(session) = self.session.as_mut() else {
                    return;
                };
                if let Some(rects) = session.ui_focus_rects.as_ref() {
                    session.ui_focus.arm_shortcut_repeat(key_id, rects);
                }
            }
        }
        let repeat = self
            .session
            .as_mut()
            .and_then(|session| session.ui_focus.advance_shortcut_repeat(dt));
        if let Some(key_id) = repeat
            && !self.activate_text_key(&key_id)
            && let Some(session) = self.session.as_mut()
        {
            session.ui_focus.release_shortcut_repeat();
        }
    }

    /// Fire the open text-entry tree's key `key_id`, leaving focus where it
    /// is. `false` when no text-entry tree is on top or the focus export does
    /// not describe it, or it has no such key.
    fn activate_text_key(&mut self, key_id: &str) -> bool {
        let Some(session) = self.session.as_ref() else {
            return false;
        };
        if session.modal_stack.active_text_entry_target().is_none() {
            return false;
        }
        let Some(active) = session.modal_stack.active_name() else {
            return false;
        };
        let has_key = crate::session::focus_rects_for(session.ui_focus_rects.as_ref(), active)
            .is_some_and(|rects| rects.rects.iter().any(|r| r.id == key_id && !r.disabled));
        if has_key {
            self.fire_focused_button_activation(Some(key_id));
        }
        has_key
    }
}

#[cfg(test)]
#[path = "text_shortcuts_tests.rs"]
mod tests;
