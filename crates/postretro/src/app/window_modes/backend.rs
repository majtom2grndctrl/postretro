// winit adapter for window-mode policy; handles never leave this chokepoint.
// See: context/lib/player_options.md §7

use super::policy::{Backend, Target};
use crate::options::{DisplayMode, WindowMode};
use winit::{
    event_loop::ActiveEventLoop,
    monitor::VideoModeHandle,
    window::{Fullscreen, Window},
};

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
    {
        let _ = event_loop;
        false
    }
}

fn normalize_refresh(reported: u32, monitor_refresh: Option<u32>) -> u32 {
    if reported == 0 {
        monitor_refresh.unwrap_or(0)
    } else {
        reported
    }
}

fn describe(mode: &VideoModeHandle) -> DisplayMode {
    let monitor = mode.monitor();
    let reported = mode.refresh_rate_millihertz();
    let monitor_refresh = if reported == 0 {
        monitor.refresh_rate_millihertz()
    } else {
        None
    };
    DisplayMode {
        width: mode.size().width,
        height: mode.size().height,
        refresh_millihertz: normalize_refresh(reported, monitor_refresh),
        bit_depth: mode.bit_depth(),
        monitor: monitor.name().unwrap_or_default(),
    }
}

fn desktop_choice(
    choices: &[DisplayMode],
    size: [u32; 2],
    refresh: Option<u32>,
    monitor: &str,
) -> Option<DisplayMode> {
    choices
        .iter()
        .filter(|mode| mode.width == size[0] && mode.height == size[1] && mode.monitor == monitor)
        .min_by_key(|mode| {
            (
                refresh.map_or(0, |rate| mode.refresh_millihertz.abs_diff(rate)),
                std::cmp::Reverse(mode.bit_depth),
                mode.refresh_millihertz,
            )
        })
        .cloned()
}

impl WinitBackend<'_> {
    fn modes(&self) -> Vec<(DisplayMode, VideoModeHandle)> {
        if self.wayland {
            return Vec::new();
        }
        let Some(monitor) = self.window.current_monitor() else {
            return Vec::new();
        };
        let mut modes: Vec<_> = monitor
            .video_modes()
            .map(|mode| (describe(&mode), mode))
            .collect();
        modes.sort_by(|a, b| a.0.cmp(&b.0));
        modes.dedup_by(|a, b| a.0 == b.0);
        modes
    }
}

impl Backend for WinitBackend<'_> {
    fn enumerate(&self) -> Vec<DisplayMode> {
        self.modes().into_iter().map(|(mode, _)| mode).collect()
    }

    fn picker_choices(&self, available: &[DisplayMode]) -> Vec<DisplayMode> {
        let Some(monitor) = self.window.current_monitor() else {
            return Vec::new();
        };
        let size = monitor.size();
        super::picker::choices(available, [size.width, size.height])
    }

    fn desktop_mode(&self, choices: &[DisplayMode]) -> Option<DisplayMode> {
        let monitor = self.window.current_monitor()?;
        let size = monitor.size();
        desktop_choice(
            choices,
            [size.width, size.height],
            monitor.refresh_rate_millihertz(),
            &monitor.name().unwrap_or_default(),
        )
    }

    fn apply(&mut self, target: &Target) -> bool {
        let fullscreen = match target.mode {
            WindowMode::Windowed => None,
            WindowMode::Borderless => Some(Fullscreen::Borderless(self.window.current_monitor())),
            WindowMode::Exclusive => {
                let found = self
                    .modes()
                    .into_iter()
                    .find(|(mode, _)| Some(mode) == target.display.as_ref());
                match found {
                    Some((_, handle)) => Some(Fullscreen::Exclusive(handle)),
                    None => {
                        self.window.set_fullscreen(Some(Fullscreen::Borderless(
                            self.window.current_monitor(),
                        )));
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
                Some(Fullscreen::Borderless(_)) => Target {
                    mode: WindowMode::Borderless,
                    display: None,
                },
                Some(Fullscreen::Exclusive(mode)) => Target {
                    mode: WindowMode::Exclusive,
                    display: Some(describe(mode)),
                },
            };
            self.cache.fullscreen = fullscreen;
            self.cache.initialized = true;
        }
        &self.cache.target
    }
}

#[cfg(test)]
mod tests {
    use super::{desktop_choice, normalize_refresh};
    use crate::options::DisplayMode;

    #[test]
    fn describe_normalizes_refresh_before_mode_deduplication() {
        assert_eq!(normalize_refresh(0, Some(60_000)), 60_000);
        assert_eq!(normalize_refresh(60_000, Some(60_000)), 60_000);
        assert_eq!(normalize_refresh(0, None), 0);
        assert_eq!(normalize_refresh(0, Some(0)), 0);
        assert_eq!(normalize_refresh(59_940, Some(60_000)), 59_940);

        let mode = |refresh_millihertz, bit_depth| DisplayMode {
            width: 1920,
            height: 1080,
            refresh_millihertz,
            bit_depth,
            monitor: "test monitor".into(),
        };
        let mut modes = vec![
            mode(normalize_refresh(0, Some(60_000)), 32),
            mode(normalize_refresh(60_000, Some(60_000)), 32),
            mode(normalize_refresh(0, Some(60_000)), 24),
        ];
        modes.sort();
        modes.dedup();

        assert_eq!(modes.len(), 2);
        assert!(modes.contains(&mode(60_000, 24)));
        assert!(modes.contains(&mode(60_000, 32)));

        assert_eq!(
            desktop_choice(&modes, [1920, 1080], Some(60_000), "test monitor"),
            Some(mode(60_000, 32)),
        );
        modes.push(mode(144_000, 32));
        assert_eq!(
            desktop_choice(&modes, [1920, 1080], Some(144_000), "test monitor"),
            Some(mode(144_000, 32)),
        );
        assert_eq!(
            desktop_choice(&modes, [1280, 720], Some(60_000), "test monitor"),
            None
        );
        assert_eq!(
            desktop_choice(&modes, [1920, 1080], Some(60_000), "other monitor"),
            None
        );
        assert!(desktop_choice(&[], [1920, 1080], None, "test monitor").is_none());
    }
}
