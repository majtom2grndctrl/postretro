// The accessibility panel's global input (`nav.options`: gamepad Select/Back,
// keyboard F1) and the rule that frames drawing no UI drop UI input.
// See: context/lib/input.md §5 · context/lib/ui.md §4.1

use postretro_ui::demo::ACCESSIBILITY_PANEL_NAME;
use winit::keyboard::KeyCode;

use crate::startup::BootState;
use crate::*;

/// Keyboard default of the panel's global input. It reads ahead of text entry,
/// so it must be a key text entry never consumes.
pub(crate) const PANEL_TOGGLE_KEY: KeyCode = KeyCode::F1;

/// Whether a key event is the global input's press edge. OS key repeat never
/// toggles, so a held F1 toggles once in either direction.
pub(crate) fn is_panel_toggle_press(code: KeyCode, pressed: bool, repeat: bool) -> bool {
    code == PANEL_TOGGLE_KEY && pressed && !repeat
}

/// What the global input does against the current stack.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum PanelToggle {
    Open,
    Close,
    /// The panel is open beneath another tree.
    Nothing,
}

pub(crate) fn panel_toggle_for(modal_stack: &postretro_ui::modal_stack::ModalStack) -> PanelToggle {
    if modal_stack.active_name() == Some(ACCESSIBILITY_PANEL_NAME) {
        PanelToggle::Close
    } else if modal_stack.contains_pushed(ACCESSIBILITY_PANEL_NAME) {
        PanelToggle::Nothing
    } else {
        PanelToggle::Open
    }
}

impl App {
    /// Only frames that draw UI take UI input. Splash and Loading frames draw
    /// none, so input pressed on them is dropped, never delivered later.
    pub(crate) fn boot_state_accepts_ui_input(&self) -> bool {
        matches!(
            self.boot_state,
            BootState::Frontend | BootState::FirstLaunchHold | BootState::Running
        )
    }

    /// The global input was pressed (F1 press edge, or a gamepad Select). It
    /// latches for this frame's game logic only while UI is on screen.
    pub(crate) fn request_panel_toggle(&mut self) {
        if self.boot_state_accepts_ui_input() {
            self.pending_panel_toggle = true;
        }
    }

    /// A frame that draws no UI: drop queued UI intents, latched toggles and
    /// buffered gamepad presses, so none reaches the first frame that does.
    pub(crate) fn drop_ui_input_on_non_ui_frame(&mut self) {
        self.pending_panel_toggle = false;
        self.pending_menu_toggle = false;
        if let Some(session) = self.session.as_mut() {
            session.ui_dispatch.discard_all();
            if let Some(gp) = session.gamepad_system.as_mut() {
                gp.discard_pending_events();
            }
        }
    }

    /// Game logic: apply a pending global toggle. Opens the panel over whatever
    /// shows, closes it when it is the active tree, and does nothing while it is
    /// open beneath another tree. Intents captured in the same input stage were
    /// aimed at the tree the toggle just revealed or covered, so they are
    /// discarded rather than delivered to it next frame. Returns whether the
    /// stack changed.
    pub(crate) fn apply_panel_toggle(&mut self) -> bool {
        if !std::mem::take(&mut self.pending_panel_toggle) {
            return false;
        }
        let Some(session) = self.session.as_mut() else {
            return false;
        };
        match panel_toggle_for(&session.modal_stack) {
            PanelToggle::Open => {
                session
                    .modal_stack
                    .push_named(ACCESSIBILITY_PANEL_NAME, None);
                // Noticed as open even if a close lands before the next
                // options update, so that close still writes the record.
                self.accessibility_panel_was_open = true;
            }
            PanelToggle::Close => session.modal_stack.pop(),
            PanelToggle::Nothing => return false,
        }
        session.ui_dispatch.discard_ready();
        true
    }
}
