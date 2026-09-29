// `App` behavior split by concern: UI action routing, keyboard intake, and the
// options-menu bridge wiring. `main.rs` keeps the frame loop and composition.
// See: context/lib/ui.md §4 · context/lib/input.md §5

pub(crate) mod accessibility_panel;
#[cfg(test)]
mod accessibility_panel_tests;
#[cfg(test)]
mod accessibility_surface_fixture_tests;
pub(crate) mod keyboard_input;
pub(crate) mod options_menu;
pub(crate) mod ui_actions;
pub(crate) mod ui_input_frames;
#[cfg(test)]
mod ui_input_frames_tests;
