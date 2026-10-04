// Window-mode policy fixtures exercise the same adapter contract as winit.
// See: context/lib/player_options.md §7

use super::policy::*;
use crate::options::{DisplayMode, PlayerOptions, WindowMode};
use std::{cell::RefCell, time::Instant};

fn mode(width: u32, rate: u32, monitor: &str) -> DisplayMode {
    DisplayMode { width, height: 720, refresh_millihertz: rate, bit_depth: 32, monitor: monitor.into() }
}

#[derive(Default)]
struct FakeBackend {
    modes: RefCell<Vec<DisplayMode>>,
    actual: Target,
    requests: Vec<Target>,
    wayland: bool,
}
impl Backend for FakeBackend {
    fn enumerate(&self) -> Vec<DisplayMode> { if self.wayland { vec![] } else { self.modes.borrow().clone() } }
    fn apply(&mut self, target: &Target) -> bool {
        let applied = target.mode != WindowMode::Exclusive || self.enumerate().iter().any(|mode| Some(mode) == target.display.as_ref());
        self.actual = if applied { target.clone() } else { Target { mode: WindowMode::Borderless, display: None } };
        self.requests.push(self.actual.clone());
        applied
    }
    fn readback(&mut self) -> &Target { &self.actual }
}

#[test]
fn boot_refinds_modes_on_current_monitor_and_preserves_fallback_preference() {
    let selected = mode(1280, 60000, "current");
    let mut options = PlayerOptions::default();
    options.window_mode = WindowMode::Exclusive;
    options.display_mode = Some(selected.clone());
    let mut backend = FakeBackend::default();
    backend.modes.borrow_mut().push(selected.clone());
    let mut controller = Controller::new(false);
    controller.boot(&mut backend, &options, Instant::now());
    assert_eq!(controller.effective.mode, WindowMode::Exclusive);
    backend.modes.borrow_mut()[0].monitor = "other".into();
    controller.boot(&mut backend, &options, Instant::now());
    assert_eq!(controller.effective.mode, WindowMode::Borderless);
    assert!(controller.fallback);
    assert_eq!(options.window_mode, WindowMode::Exclusive);
    assert_eq!(options.display_mode, Some(selected));
    assert_eq!(controller.picked, None);
}

#[test]
fn enumeration_deduplicates_zero_rate_and_wayland_suppresses_choices() {
    let selected = mode(1280, 0, "current");
    let mut backend = FakeBackend::default();
    *backend.modes.borrow_mut() = vec![selected.clone(), selected.clone()];
    let mut options = PlayerOptions::default();
    options.window_mode = WindowMode::Exclusive;
    options.display_mode = Some(selected);
    let mut controller = Controller::new(false);
    controller.boot(&mut backend, &options, Instant::now());
    assert_eq!(controller.choices.len(), 1);
    backend.wayland = true;
    controller.boot(&mut backend, &options, Instant::now());
    assert!(controller.choices.is_empty());
    assert_eq!(controller.picked, None);
    assert_eq!(controller.effective.mode, WindowMode::Borderless);
}

#[test]
fn boot_escape_makes_no_request_and_keeps_saved_choice() {
    let mut options = PlayerOptions::default();
    options.window_mode = WindowMode::Borderless;
    let mut backend = FakeBackend::default();
    let mut controller = Controller::new(true);
    controller.boot(&mut backend, &options, Instant::now());
    assert!(backend.requests.is_empty());
    assert_eq!(options.window_mode, WindowMode::Borderless);
}

#[test]
fn window_calls_live_only_in_the_chokepoint() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap();
    let allowed = root.join("postretro/src/app/window_modes");
    let mut dirs = vec![root.to_path_buf()];
    let forbidden = [".set_fullscreen(", ".fullscreen()", ".current_monitor()", ".available_monitors()", ".primary_monitor()", ".video_modes()", ".with_fullscreen(", ".set_simple_fullscreen("];
    while let Some(dir) = dirs.pop() {
        for entry in std::fs::read_dir(dir).unwrap() {
            let path = entry.unwrap().path();
            if path.starts_with(&allowed) { continue; }
            if path.is_dir() { if path.file_name().unwrap() != "target" { dirs.push(path); } }
            else if path.extension().is_some_and(|ext| ext == "rs") {
                let source = std::fs::read_to_string(&path).unwrap();
                for needle in forbidden { assert!(!source.contains(needle), "{} contains {needle}", path.display()); }
            }
        }
    }
}
