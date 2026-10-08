// Window-mode request owner; all fullscreen, monitor and video-mode calls live here.
// See: context/lib/player_options.md §7

mod backend;
#[cfg(test)]
mod menu_tests;
mod picker;
mod policy;
#[cfg(test)]
mod policy_tests;
mod projection;

use crate::options::PlayerOptions;
use backend::{ReadbackCache, WinitBackend};
use policy::{Backend, Controller};
use postretro_ui::actions::DisplayModeAction;
use std::time::Instant;
use winit::{event_loop::ActiveEventLoop, window::Window};

/// Whether boot holds a saved exclusive mode until the renderer has a surface.
/// Observed on DX12 (root cause unconfirmed): wgpu's default HWND swapchain
/// creation fails with `DXGI_ERROR_INVALID_CALL` once the window holds an
/// exclusive display mode, while a swapchain created first resizes through
/// the change as on a live switch. wgpu's DirectComposition swapchain avoids
/// the failure but changes every frame's present path. Gated on Windows, not
/// DX12: the backend is unknown until the renderer picks an adapter, and
/// wgpu falls back to DX12 wherever Vulkan is missing.
const DEFER_BOOT_EXCLUSIVE: bool = cfg!(windows);

pub(crate) struct WindowModes {
    controller: Controller,
    cache: ReadbackCache,
    wayland: bool,
    boot_exclusive_deferred: bool,
    confirm_instance: Option<postretro_ui::modal_stack::ModalInstance>,
}

impl WindowModes {
    pub(crate) fn new(force_windowed: bool) -> Self {
        Self {
            controller: Controller::new(force_windowed),
            cache: ReadbackCache::default(),
            wayland: false,
            boot_exclusive_deferred: false,
            confirm_instance: None,
        }
    }

    /// Apply the saved mode to the new window, before the renderer exists.
    /// Where exclusive is deferred, the window shows borderless on its monitor
    /// until [`Self::finish_boot`] applies the saved mode.
    pub(crate) fn apply_boot(
        &mut self,
        window: &Window,
        event_loop: &ActiveEventLoop,
        options: &PlayerOptions,
    ) {
        self.wayland = backend::is_wayland(event_loop);
        if let Some(interim) = self.controller.boot_interim(options, DEFER_BOOT_EXCLUSIVE) {
            self.boot_exclusive_deferred = true;
            let mut backend = WinitBackend {
                window,
                wayland: self.wayland,
                cache: &mut self.cache,
            };
            backend.apply(&interim);
            return;
        }
        self.boot(window, options);
    }

    /// Apply an exclusive mode `apply_boot` deferred, once the renderer has
    /// created its surface. A no-op for every other boot.
    pub(crate) fn finish_boot(&mut self, window: &Window, options: &PlayerOptions) {
        if std::mem::take(&mut self.boot_exclusive_deferred) {
            self.boot(window, options);
        }
    }

    fn boot(&mut self, window: &Window, options: &PlayerOptions) {
        let mut backend = WinitBackend {
            window,
            wayland: self.wayland,
            cache: &mut self.cache,
        };
        self.controller.boot(&mut backend, options, Instant::now());
    }
}

fn request_mode(
    controller: &mut Controller,
    session: &mut crate::session::Session,
    backend: &mut impl policy::Backend,
    mode: crate::options::WindowMode,
    now: Instant,
) -> policy::Change {
    let change = controller.request_mode(backend, &mut session.player_options, mode, now);
    if change == policy::Change::None {
        let visible = if controller.pending.is_some() {
            crate::options::WindowMode::Exclusive
        } else {
            session.player_options.window_mode
        };
        session.options_bridge.reseed_window_mode(
            &mut session.scripting.script_ctx.slot_table.borrow_mut(),
            visible,
        );
    }
    change
}

fn dispatch_display_mode_action(
    controller: &mut Controller,
    confirm_instance: Option<postretro_ui::modal_stack::ModalInstance>,
    session: &mut crate::session::Session,
    backend: &mut impl policy::Backend,
    action: DisplayModeAction,
    now: Instant,
) -> policy::Change {
    // A removed confirm cannot be kept by a later queued activation.
    let invalid = controller.pending.is_some()
        && !confirm_instance
            .is_some_and(|instance| session.modal_stack.contains_instance(instance));
    if invalid {
        return controller.revert(backend, now);
    }
    match action {
        DisplayModeAction::Keep => controller.keep(&mut session.player_options),
        DisplayModeAction::Revert => controller.revert(backend, now),
        DisplayModeAction::Apply => {
            controller.apply_selected(backend, &mut session.player_options, now)
        }
        DisplayModeAction::Next | DisplayModeAction::Previous => controller.step(
            backend,
            &mut session.player_options,
            action == DisplayModeAction::Next,
            now,
        ),
    }
}

fn service(
    controller: &mut Controller,
    confirm_instance: Option<postretro_ui::modal_stack::ModalInstance>,
    session: &crate::session::Session,
    backend: &mut impl policy::Backend,
    now: Instant,
) -> policy::Change {
    controller.service(
        backend,
        confirm_instance.is_some_and(|instance| session.modal_stack.contains_instance(instance)),
        now,
    )
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
            Change::None | Change::SelectionChanged => {}
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
        let change = request_mode(
            &mut self.window_modes.controller,
            session,
            &mut backend,
            mode,
            Instant::now(),
        );
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

    pub(crate) fn apply_display_mode_action(&mut self, action: DisplayModeAction) {
        let (Some(ws), Some(session)) = (self.window_state.as_ref(), self.session.as_mut()) else {
            return;
        };
        let mut backend = WinitBackend {
            window: &ws.window,
            wayland: self.window_modes.wayland,
            cache: &mut self.window_modes.cache,
        };
        let change = dispatch_display_mode_action(
            &mut self.window_modes.controller,
            self.window_modes.confirm_instance,
            session,
            &mut backend,
            action,
            Instant::now(),
        );
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
        let change = service(
            &mut self.window_modes.controller,
            self.window_modes.confirm_instance,
            session,
            &mut backend,
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

#[cfg(test)]
mod boot_tests {
    #[test]
    fn boot_mode_applies_after_visible_creation_before_first_redraw() {
        let main = include_str!("../../main.rs");
        let start = main.find("fn resumed(").unwrap();
        let body = &main[start..main[start..].find("fn suspended(").unwrap() + start];
        let create = body.find("create_window(window_attributes())").unwrap();
        let apply = body.find("self.window_modes.apply_boot(").unwrap();
        let renderer = body.find("Renderer::new(&window)").unwrap();
        let finish = body.find("self.window_modes.finish_boot(").unwrap();
        let redraw = body.find("ws.window.request_redraw();").unwrap();
        // A deferred exclusive mode lands only once the renderer owns a surface.
        assert!(create < apply && apply < renderer && renderer < finish && finish < redraw);
        assert!(!main.contains(".with_fullscreen("));
        assert!(!include_str!("../../frame_loop/mod.rs").contains(".with_fullscreen("));
        let startup = include_str!("../../startup/session.rs");
        assert!(
            startup.find("BootOptions::load(").unwrap()
                < startup.find("let event_loop = EventLoop::new()").unwrap()
        );
    }
}
