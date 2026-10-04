// winit adapter for window-mode policy; handles never leave this chokepoint.
// See: context/lib/player_options.md §7

use super::policy::{Backend, Target};
use crate::options::{DisplayMode, WindowMode};
use winit::{event_loop::ActiveEventLoop, monitor::VideoModeHandle, window::{Fullscreen, Window}};

#[derive(Default)]
pub(super) struct ReadbackCache {
    initialized: bool,
    fullscreen: Option<Fullscreen>,
    target: Target,
}

pub(super) struct WinitBackend<'a> {
    pub(super) window: &'a Window,
    pub(super) wayland: bool,
    pub(super) cache: &'a mut ReadbackCache,
}

pub(super) fn is_wayland(event_loop: &ActiveEventLoop) -> bool {
    #[cfg(target_os = "linux")]
    {
        use winit::platform::wayland::ActiveEventLoopExtWayland;
        event_loop.is_wayland()
    }
    #[cfg(not(target_os = "linux"))]
    { let _ = event_loop; false }
}

fn describe(mode: &VideoModeHandle) -> DisplayMode {
    let monitor = mode.monitor();
    let reported = mode.refresh_rate_millihertz();
    DisplayMode {
        width: mode.size().width,
        height: mode.size().height,
        refresh_millihertz: if reported == 0 { monitor.refresh_rate_millihertz().unwrap_or(0) } else { reported },
        bit_depth: mode.bit_depth(),
        monitor: monitor.name().unwrap_or_default(),
    }
}

impl WinitBackend<'_> {
    fn modes(&self) -> Vec<(DisplayMode, VideoModeHandle)> {
        if self.wayland { return Vec::new(); }
        let Some(monitor) = self.window.current_monitor() else { return Vec::new(); };
        let mut modes: Vec<_> = monitor.video_modes().map(|mode| (describe(&mode), mode)).collect();
        modes.sort_by(|a, b| a.0.cmp(&b.0));
        modes.dedup_by(|a, b| a.0 == b.0);
        modes
    }
}

impl Backend for WinitBackend<'_> {
    fn enumerate(&self) -> Vec<DisplayMode> { self.modes().into_iter().map(|(mode, _)| mode).collect() }

    fn apply(&mut self, target: &Target) -> bool {
        let fullscreen = match target.mode {
            WindowMode::Windowed => None,
            WindowMode::Borderless => Some(Fullscreen::Borderless(self.window.current_monitor())),
            WindowMode::Exclusive => {
                let found = self.modes().into_iter().find(|(mode, _)| Some(mode) == target.display.as_ref());
                match found {
                    Some((_, handle)) => Some(Fullscreen::Exclusive(handle)),
                    None => {
                        log::warn!("[Window] exclusive display mode is unavailable on the current monitor; using borderless for this session");
                        self.window.set_fullscreen(Some(Fullscreen::Borderless(self.window.current_monitor())));
                        return false;
                    }
                }
            }
        };
        self.window.set_fullscreen(fullscreen);
        true
    }

    fn readback(&mut self) -> &Target {
        let fullscreen = self.window.fullscreen();
        if !self.cache.initialized || fullscreen != self.cache.fullscreen {
            self.cache.target = match &fullscreen {
                None => Target::default(),
                Some(Fullscreen::Borderless(_)) => Target { mode: WindowMode::Borderless, display: None },
                Some(Fullscreen::Exclusive(mode)) => Target { mode: WindowMode::Exclusive, display: Some(describe(mode)) },
            };
            self.cache.fullscreen = fullscreen;
            self.cache.initialized = true;
        }
        &self.cache.target
    }
}
