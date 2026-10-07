// `App` behavior split by concern (e.g. UI actions, keyboard intake, options and
// controls panels, glyph art, window modes). `main.rs` keeps the frame loop.
// See: context/lib/ui.md §4 · context/lib/input.md §5

pub(crate) mod accessibility_panel;
#[cfg(test)]
mod accessibility_panel_tests;
#[cfg(test)]
mod accessibility_surface_fixture_tests;
pub(crate) mod bindings;
pub(crate) mod controls_panel;
pub(crate) mod glyph_art;
#[cfg(test)]
mod input_surface_fixture_tests;
pub(crate) mod keyboard_input;
pub(crate) mod options_menu;
pub(crate) mod render_extents;
pub(crate) mod text_shortcuts;
pub(crate) mod ui_actions;
pub(crate) mod ui_input_frames;
#[cfg(test)]
mod ui_input_frames_tests;

pub(crate) mod window_modes;
