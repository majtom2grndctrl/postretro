// Frames that draw no UI drop UI input, so nothing pressed on a splash,
// Loading or Settling frame reaches the first frame that does.
// See: context/lib/input.md §5

use crate::*;

impl App {
    /// Only frames that draw UI take UI input. Splash, Loading and Settling
    /// frames draw none, so input pressed on them is dropped, never delivered
    /// later. The exact inverse of `BootState::drops_ui_input`.
    pub(crate) fn boot_state_accepts_ui_input(&self) -> bool {
        !self.boot_state.drops_ui_input()
    }

    /// A frame that draws no UI: drop queued UI intents, the latched menu toggle
    /// and buffered gamepad presses, so none reaches the first frame that does.
    /// A buffered cancel therefore cannot close the first-launch panel on its
    /// first frame.
    pub(crate) fn drop_ui_input_on_non_ui_frame(&mut self) {
        self.pending_menu_toggle = false;
        if let Some(session) = self.session.as_mut() {
            session.ui_dispatch.discard_all();
            if let Some(gp) = session.gamepad_system.as_mut() {
                gp.discard_pending_events(&mut session.input_system);
            }
        }
    }
}
