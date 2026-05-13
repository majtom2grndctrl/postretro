// Coarse input-focus state: who owns pointer-lock and key-event consumption.
// See: context/lib/input.md

/// Coarse owner of the keyboard/mouse focus. Drives pointer-lock acquire/release
/// at the App layer; future menu and dev-UI consumers gate their input on the
/// same state so only one consumer ever interprets a given event.
///
/// `Menu` has no consumer yet — it is wired through `App::set_input_focus`
/// identically to `DevTools` so the wiring is complete the moment the menu
/// system arrives.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InputFocus {
    /// Player has the cursor locked; gameplay consumes input.
    Gameplay,
    /// Debug overlay (egui) consumes input; cursor released.
    /// Toggled by `DiagnosticAction::ToggleDebugPanel`; `App::set_input_focus`
    /// and `App::reapply_focus` are the other half of the wiring.
    #[cfg_attr(not(feature = "dev-tools"), allow(dead_code))]
    DevTools,
    /// Menu UI consumes input; cursor released.
    /// No consumer yet — wired through `App::set_input_focus` and
    /// `reapply_focus` so the menu system can drop in without re-plumbing.
    #[allow(dead_code)]
    Menu,
}

impl InputFocus {
    /// True when the cursor should be captured for this focus mode.
    #[allow(dead_code)]
    pub fn captures_cursor(self) -> bool {
        matches!(self, InputFocus::Gameplay)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn gameplay_captures_cursor() {
        assert!(InputFocus::Gameplay.captures_cursor());
    }

    #[test]
    fn devtools_does_not_capture_cursor() {
        assert!(!InputFocus::DevTools.captures_cursor());
    }

    #[test]
    fn menu_does_not_capture_cursor() {
        assert!(!InputFocus::Menu.captures_cursor());
    }

    /// Pin the variant set so adding a new focus mode forces a review of every
    /// `captures_cursor` consumer and the `App::set_input_focus` match.
    #[test]
    fn input_focus_variants_are_exhaustive() {
        for focus in [InputFocus::Gameplay, InputFocus::DevTools, InputFocus::Menu] {
            // Exhaustive match: compiler enforces that all variants are listed.
            match focus {
                InputFocus::Gameplay => assert!(focus.captures_cursor()),
                InputFocus::DevTools | InputFocus::Menu => assert!(!focus.captures_cursor()),
            }
        }
    }
}
