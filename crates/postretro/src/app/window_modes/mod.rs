// Window-mode request owner; all fullscreen, monitor and video-mode calls live here.
// See: context/lib/player_options.md §7

use crate::options::{PlayerOptions, WindowMode};
use winit::window::{Fullscreen, Window};

pub(crate) struct WindowModes {
    force_windowed: bool,
}

impl WindowModes {
    pub(crate) fn new(force_windowed: bool) -> Self {
        Self { force_windowed }
    }

    pub(crate) fn apply_boot(&mut self, window: &Window, options: &PlayerOptions) {
        let mode = if self.force_windowed {
            WindowMode::Windowed
        } else {
            options.window_mode
        };
        let fullscreen = match mode {
            WindowMode::Windowed => None,
            WindowMode::Borderless => Some(Fullscreen::Borderless(None)),
            WindowMode::Exclusive => {
                log::warn!(
                    "[Window] exclusive display mode is unavailable; using borderless for this session"
                );
                Some(Fullscreen::Borderless(None))
            }
        };
        if fullscreen.is_some() {
            window.set_fullscreen(fullscreen);
        }
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn boot_mode_applies_after_visible_creation_before_first_redraw() {
        let main = include_str!("../../main.rs");
        let start = main.find("fn resumed(").unwrap();
        let body = &main[start..main[start..].find("fn suspended(").unwrap() + start];
        let create = body.find("create_window(window_attributes())").unwrap();
        let apply = body.find("self.window_modes.apply_boot(").unwrap();
        let redraw = body.find("ws.window.request_redraw();").unwrap();
        assert!(create < apply && apply < redraw);
        assert!(!main.contains(".with_fullscreen("));
        let startup = include_str!("../../startup/session.rs");
        assert!(
            startup.find("BootOptions::load(").unwrap()
                < startup.find("let event_loop = EventLoop::new()").unwrap()
        );
    }
}
