// Cursor capture and release for gameplay mouse handling.
// See: context/lib/input.md $4

use winit::window::{CursorGrabMode, Window};

/// Whether the cursor is currently captured for gameplay.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum CursorState {
    Captured,
    Released,
}

impl Default for CursorState {
    fn default() -> Self {
        CursorState::Released
    }
}

/// Attempt to capture the mouse cursor, trying Locked first then Confined.
/// Returns the resulting cursor state.
pub fn capture_cursor(window: &Window) -> CursorState {
    if window.set_cursor_grab(CursorGrabMode::Locked).is_ok() {
        window.set_cursor_visible(false);
        return CursorState::Captured;
    }
    // Locked not supported (some Linux WMs); fall back to Confined.
    if let Err(err) = window.set_cursor_grab(CursorGrabMode::Confined) {
        log::warn!("[Input] Failed to grab cursor: {err}");
        return CursorState::Released;
    }
    log::warn!("[Input] CursorGrabMode::Locked not supported, using Confined fallback");
    window.set_cursor_visible(false);
    CursorState::Captured
}

/// Release the cursor -- unlock and show it.
pub fn release_cursor(window: &Window) -> CursorState {
    let _ = window.set_cursor_grab(CursorGrabMode::None);
    window.set_cursor_visible(true);
    CursorState::Released
}

/// Handle a window focus change. Captures on focus, releases on blur.
pub fn handle_focus_change(focused: bool, window: &Window) -> CursorState {
    if focused {
        capture_cursor(window)
    } else {
        release_cursor(window)
    }
}
