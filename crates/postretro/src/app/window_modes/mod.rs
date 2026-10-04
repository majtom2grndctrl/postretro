// Window-mode request owner; all fullscreen, monitor and video-mode calls live here.
// See: context/lib/player_options.md §7

mod backend;
mod policy;
#[cfg(test)]
mod policy_tests;
mod projection;

use crate::options::PlayerOptions;
use backend::{ReadbackCache, WinitBackend};
use policy::Controller;
use std::time::Instant;
use winit::{event_loop::ActiveEventLoop, window::Window};

pub(crate) struct WindowModes {
    controller: Controller,
    cache: ReadbackCache,
    wayland: bool,
    confirm_instance: Option<postretro_ui::modal_stack::ModalInstance>,
}

impl WindowModes {
    pub(crate) fn new(force_windowed: bool) -> Self {
        Self {
            controller: Controller::new(force_windowed),
            cache: ReadbackCache::default(),
            wayland: false,
            confirm_instance: None,
        }
    }

    pub(crate) fn apply_boot(
        &mut self,
        window: &Window,
        event_loop: &ActiveEventLoop,
        options: &PlayerOptions,
    ) {
        self.wayland = backend::is_wayland(event_loop);
        let mut backend = WinitBackend {
            window,
            wayland: self.wayland,
            cache: &mut self.cache,
        };
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

impl crate::App {
    pub(crate) fn display_mode_confirm_is_top(&self) -> bool {
        self.session.as_ref().is_some_and(|session| {
            session.modal_stack.active_name() == Some(postretro_ui::demo::DISPLAY_MODE_CONFIRM_NAME)
        })
    }

    fn finish_window_change(&mut self, change: policy::Change) {
        use policy::Change;
        use postretro_ui::demo::DISPLAY_MODE_CONFIRM_NAME;
        let Some(session) = self.session.as_mut() else {
            return;
        };
        match change {
            Change::None => {}
            Change::OpenConfirm => {
                session
                    .modal_stack
                    .push_named(DISPLAY_MODE_CONFIRM_NAME, None);
                self.window_modes.confirm_instance =
                    session.modal_stack.active_instance().filter(|_| {
                        session.modal_stack.active_name() == Some(DISPLAY_MODE_CONFIRM_NAME)
                    });
            }
            Change::Accepted | Change::Reverted => {
                if let Some(instance) = self.window_modes.confirm_instance.take() {
                    session.modal_stack.remove_instance(instance);
                }
                if change == Change::Accepted {
                    session
                        .options_bridge
                        .schedule_save(session.settings_path.as_deref());
                }
                session.options_bridge.reseed_window_mode(
                    &mut session.scripting.script_ctx.slot_table.borrow_mut(),
                    session.player_options.window_mode,
                );
            }
        }
        projection::project(
            &self.window_modes.controller,
            &mut session.scripting.script_ctx.slot_table.borrow_mut(),
            Instant::now(),
        );
    }

    pub(crate) fn request_window_mode(&mut self, mode: crate::options::WindowMode) {
        let (Some(ws), Some(session)) = (self.window_state.as_ref(), self.session.as_mut()) else {
            return;
        };
        let mut backend = WinitBackend {
            window: &ws.window,
            wayland: self.window_modes.wayland,
            cache: &mut self.window_modes.cache,
        };
        let change = self.window_modes.controller.request_mode(
            &mut backend,
            &mut session.player_options,
            mode,
            Instant::now(),
        );
        if change == policy::Change::None {
            let visible = if self.window_modes.controller.pending.is_some() {
                crate::options::WindowMode::Exclusive
            } else {
                session.player_options.window_mode
            };
            session.options_bridge.reseed_window_mode(
                &mut session.scripting.script_ctx.slot_table.borrow_mut(),
                visible,
            );
        }
        self.finish_window_change(change);
    }

    pub(crate) fn refresh_window_modes(&mut self) {
        let (Some(ws), Some(session)) = (self.window_state.as_ref(), self.session.as_mut()) else {
            return;
        };
        if self.window_modes.controller.pending.is_some() {
            return;
        }
        let backend = WinitBackend {
            window: &ws.window,
            wayland: self.window_modes.wayland,
            cache: &mut self.window_modes.cache,
        };
        self.window_modes
            .controller
            .refresh(&backend, &session.player_options);
        projection::project(
            &self.window_modes.controller,
            &mut session.scripting.script_ctx.slot_table.borrow_mut(),
            Instant::now(),
        );
    }

    pub(crate) fn apply_display_mode_action(&mut self, op: &str) {
        let (Some(ws), Some(session)) = (self.window_state.as_ref(), self.session.as_mut()) else {
            return;
        };
        let mut backend = WinitBackend {
            window: &ws.window,
            wayland: self.window_modes.wayland,
            cache: &mut self.window_modes.cache,
        };
        let now = Instant::now();
        // A removed confirm cannot be kept by a later queued activation.
        let invalid = self.window_modes.controller.pending.is_some()
            && !self
                .window_modes
                .confirm_instance
                .is_some_and(|instance| session.modal_stack.contains_instance(instance));
        let change = if invalid {
            self.window_modes.controller.revert(&mut backend, now)
        } else {
            match op {
                "keep" => self
                    .window_modes
                    .controller
                    .keep(&mut session.player_options),
                "revert" => self.window_modes.controller.revert(&mut backend, now),
                "next" | "previous" => self.window_modes.controller.step(
                    &mut backend,
                    &mut session.player_options,
                    op == "next",
                    now,
                ),
                _ => policy::Change::None,
            }
        };
        self.finish_window_change(change);
    }

    pub(crate) fn service_window_modes(&mut self) {
        let (Some(ws), Some(session)) = (self.window_state.as_ref(), self.session.as_mut()) else {
            return;
        };
        let mut backend = WinitBackend {
            window: &ws.window,
            wayland: self.window_modes.wayland,
            cache: &mut self.window_modes.cache,
        };
        let change = self.window_modes.controller.service(
            &mut backend,
            self.window_modes
                .confirm_instance
                .is_some_and(|instance| session.modal_stack.contains_instance(instance)),
            Instant::now(),
        );
        self.finish_window_change(change);
    }
}

impl crate::App {
    pub(crate) fn poll_window_mode_readback(&mut self) {
        let Some(ws) = self.window_state.as_ref() else {
            return;
        };
        let mut backend = WinitBackend {
            window: &ws.window,
            wayland: self.window_modes.wayland,
            cache: &mut self.window_modes.cache,
        };
        let change = self.window_modes.controller.observe(
            &mut backend,
            self.session
                .as_mut()
                .map(|session| &mut session.player_options),
            Instant::now(),
        );
        self.finish_window_change(change);
    }
}
