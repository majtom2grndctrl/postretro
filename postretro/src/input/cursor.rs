// Cursor capture and release for gameplay mouse handling.
// See: context/lib/input.md $4

use winit::window::{CursorGrabMode, Window};

/// Attempt to capture the mouse cursor, trying Locked first then Confined.
pub fn capture_cursor(window: &Window) {
    if window.set_cursor_grab(CursorGrabMode::Locked).is_ok() {
        window.set_cursor_visible(false);
        return;
    }
    // Locked not supported (some Linux WMs); fall back to Confined.
    if let Err(err) = window.set_cursor_grab(CursorGrabMode::Confined) {
        log::warn!("[Input] Failed to grab cursor: {err}");
        return;
    }
    log::warn!("[Input] CursorGrabMode::Locked not supported, using Confined fallback");
    window.set_cursor_visible(false);
}

/// Release the cursor -- unlock and show it.
pub fn release_cursor(window: &Window) {
    let _ = window.set_cursor_grab(CursorGrabMode::None);
    window.set_cursor_visible(true);
}

/// Handle a window focus change. Captures on focus, releases on blur.
pub fn handle_focus_change(focused: bool, window: &Window) {
    if focused {
        capture_cursor(window)
    } else {
        release_cursor(window)
    }
}
