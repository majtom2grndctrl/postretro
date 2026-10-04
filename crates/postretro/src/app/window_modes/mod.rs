// Window-mode request owner; all fullscreen, monitor and video-mode calls live here.
// See: context/lib/player_options.md §7

mod backend;
mod policy;
#[cfg(test)]
mod policy_tests;

use std::time::Instant;
use crate::options::PlayerOptions;
use winit::{event_loop::ActiveEventLoop, window::Window};
use backend::{ReadbackCache, WinitBackend};
use policy::Controller;

pub(crate) struct WindowModes {
    controller: Controller,
    cache: ReadbackCache,
    wayland: bool,
}

impl WindowModes {
    pub(crate) fn new(force_windowed: bool) -> Self {
        Self { controller: Controller::new(force_windowed), cache: ReadbackCache::default(), wayland: false }
    }

    pub(crate) fn apply_boot(&mut self, window: &Window, event_loop: &ActiveEventLoop, options: &PlayerOptions) {
        self.wayland = backend::is_wayland(event_loop);
        let mut backend = WinitBackend { window, wayland: self.wayland, cache: &mut self.cache };
        self.controller.boot(&mut backend, options, Instant::now());
    }
}

#[cfg(test)]
mod boot_tests {
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
