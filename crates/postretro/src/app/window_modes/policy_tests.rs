// Window-mode policy fixtures exercise the same adapter contract as winit.
// See: context/lib/player_options.md §7

use super::policy::*;
use crate::options::{DisplayMode, PlayerOptions, WindowMode};
use std::{cell::RefCell, time::Instant};

pub(super) fn mode(width: u32, rate: u32, monitor: &str) -> DisplayMode {
    DisplayMode {
        width,
        height: 720,
        refresh_millihertz: rate,
        bit_depth: 32,
        monitor: monitor.into(),
    }
}

#[derive(Default)]
pub(super) struct FakeBackend {
    pub(super) modes: RefCell<Vec<DisplayMode>>,
    pub(super) desktop: Option<DisplayMode>,
    pub(super) monitor_size: Option<[u32; 2]>,
    pub(super) actual: Target,
    pub(super) requests: Vec<Target>,
    pub(super) wayland: bool,
}
impl Backend for FakeBackend {
    fn enumerate(&self) -> Vec<DisplayMode> {
        if self.wayland {
            vec![]
        } else {
            self.modes.borrow().clone()
        }
    }
    fn desktop_mode(&self, choices: &[DisplayMode]) -> Option<DisplayMode> {
        self.desktop
            .as_ref()
            .filter(|mode| choices.contains(mode))
            .cloned()
    }
    fn picker_choices(&self, available: &[DisplayMode]) -> Vec<DisplayMode> {
        self.monitor_size.map_or_else(
            || available.to_vec(),
            |size| super::picker::choices(available, size),
        )
    }
    fn apply(&mut self, target: &Target) -> bool {
        let applied = target.mode != WindowMode::Exclusive
            || self
                .enumerate()
                .iter()
                .any(|mode| Some(mode) == target.display.as_ref());
        self.actual = if applied {
            target.clone()
        } else {
            Target {
                mode: WindowMode::Borderless,
                display: None,
            }
        };
        self.requests.push(self.actual.clone());
        applied
    }
    fn readback(&mut self) -> &Target {
        &self.actual
    }
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
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap();
    let allowed = root.join("postretro/src/app/window_modes");
    let mut dirs = vec![root.to_path_buf()];
    let forbidden = [
        ".set_fullscreen(",
        ".fullscreen()",
        ".current_monitor()",
        ".available_monitors()",
        ".primary_monitor()",
        ".video_modes()",
        ".with_fullscreen(",
        ".set_simple_fullscreen(",
    ];
    while let Some(dir) = dirs.pop() {
        for entry in std::fs::read_dir(dir).unwrap() {
            let path = entry.unwrap().path();
            if path.starts_with(&allowed) {
                continue;
            }
            if path.is_dir() {
                if path.file_name().unwrap() != "target" {
                    dirs.push(path);
                }
            } else if path.extension().is_some_and(|ext| ext == "rs") {
                let source = std::fs::read_to_string(&path).unwrap();
                for needle in forbidden {
                    assert!(
                        !source.contains(needle),
                        "{} contains {needle}",
                        path.display()
                    );
                }
            }
        }
    }
}

#[test]
fn live_confirm_isolated_from_every_save_path_and_keep_wins_expiry_tick() {
    use crate::{
        input::{InputSystem, default_bindings},
        options::OptionsBridge,
    };
    use postretro_entities::ScriptCtx;
    use postretro_scripting_core::store_bridge::write_state_slot_json;
    use std::time::Duration;
    for stepping in [false, true] {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("settings.toml");
        let old = mode(1280, 60000, "current");
        let new = mode(1920, 60000, "current");
        let mut options = PlayerOptions::default();
        options.set_display_mode(old.clone());
        if stepping {
            options.window_mode = WindowMode::Exclusive;
        }
        options.save(&path).unwrap();
        let prior = PlayerOptions::load(&path);
        let ctx = ScriptCtx::new();
        let mut input = InputSystem::new(default_bindings());
        let mut bridge = OptionsBridge::new();
        bridge.seed_on_open(&mut ctx.slot_table.borrow_mut(), &options);
        let mut backend = FakeBackend::default();
        *backend.modes.borrow_mut() = vec![old.clone(), new.clone()];
        let now = Instant::now();
        let mut controller = Controller::new(false);
        controller.boot(&mut backend, &options, now);
        assert!(!controller.can_apply());
        assert_eq!(
            controller.apply_selected(&mut backend, &mut options, now),
            Change::None
        );
        bridge.schedule_save(Some(&path));
        let change = if stepping {
            assert_eq!(
                controller.step(&mut backend, &mut options, true, now),
                Change::SelectionChanged
            );
            assert_eq!(
                options, prior,
                "browsing keeps the accepted tuple out of every save"
            );
            controller.apply_selected(&mut backend, &mut options, now)
        } else {
            write_state_slot_json(&ctx, "options.windowMode", &serde_json::json!("exclusive"))
                .unwrap();
            let effects = bridge.update(
                0.0,
                &mut ctx.slot_table.borrow_mut(),
                &mut options,
                &mut input,
                Some(&path),
            );
            controller.request_mode(
                &mut backend,
                &mut options,
                effects.window_mode.unwrap(),
                now,
            )
        };
        assert_eq!(change, Change::OpenConfirm);
        assert_eq!(options, prior, "unconfirmed tuple stays out of the store");
        bridge.flush_on_options_close(&options, Some(&path));
        assert_eq!(PlayerOptions::load(&path), prior);
        bridge.schedule_save(Some(&path));
        bridge.update(
            0.3,
            &mut ctx.slot_table.borrow_mut(),
            &mut options,
            &mut input,
            Some(&path),
        );
        assert_eq!(PlayerOptions::load(&path), prior);
        bridge.schedule_save(Some(&path));
        bridge.flush_on_clean_exit(&options, Some(&path));
        assert_eq!(PlayerOptions::load(&path), prior);
        let requests = backend.requests.len();
        assert_eq!(
            controller.step(&mut backend, &mut options, false, now),
            Change::None
        );
        assert_eq!(
            controller.request_mode(&mut backend, &mut options, WindowMode::Borderless, now),
            Change::None
        );
        assert_eq!(backend.requests.len(), requests);
        assert_eq!(controller.keep(&mut options), Change::Accepted);
        assert_eq!(
            controller.service(&mut backend, true, now + Duration::from_secs(15)),
            Change::None
        );
        options.save(&path).unwrap();
        let kept = PlayerOptions::load(&path);
        assert_eq!(kept.window_mode, WindowMode::Exclusive);
        assert_eq!(kept.display_mode, Some(if stepping { new } else { old }));
        assert_eq!(controller.keep(&mut options), Change::None);
        assert_eq!(controller.revert(&mut backend, now), Change::None);
        assert_eq!(backend.requests.len(), requests);
    }
}

#[test]
fn removed_confirm_instance_and_loading_deadline_revert_without_persistence() {
    use postretro_ui::{descriptor::AnchoredTree, modal_stack::ModalStack};
    use std::time::Duration;
    let tree: AnchoredTree = serde_json::from_str(include_str!(
        "../../../../../core/ui/displayModeConfirm.json"
    ))
    .unwrap();
    assert_eq!(tree.initial_focus.as_deref(), Some("displayModeRevert"));
    for removed in [true, false] {
        let mut options = PlayerOptions::default();
        options.set_display_mode(mode(1280, 60000, "current"));
        let prior = options.clone();
        let mut backend = FakeBackend::default();
        backend
            .modes
            .borrow_mut()
            .push(options.display_mode.clone().unwrap());
        let mut controller = Controller::new(false);
        let now = Instant::now();
        assert_eq!(
            controller.request_mode(&mut backend, &mut options, WindowMode::Exclusive, now),
            Change::OpenConfirm
        );
        let mut stack = ModalStack::new();
        stack.push("displayModeConfirm", tree.clone());
        let original = stack.active_instance().unwrap();
        if removed {
            stack.clear_pushed();
            stack.push("displayModeConfirm", tree.clone());
            assert!(
                !stack.contains_instance(original),
                "same name is a different confirm"
            );
        }
        assert_eq!(
            controller.service(
                &mut backend,
                stack.contains_instance(original),
                now + Duration::from_secs(if removed { 1 } else { 15 })
            ),
            Change::Reverted
        );
        assert_eq!(controller.effective.mode, WindowMode::Windowed);
        assert_eq!(options, prior);
        assert_eq!(controller.keep(&mut options), Change::None);
    }
}

#[test]
fn display_steps_browse_until_apply_and_borderless_refuses_selection() {
    let mut options = PlayerOptions::default();
    let mut backend = FakeBackend::default();
    let old = mode(1280, 60_000, "current");
    let new = mode(1920, 144_000, "current");
    *backend.modes.borrow_mut() = vec![old.clone(), new.clone()];
    let mut controller = Controller::new(false);
    let now = Instant::now();
    for window_mode in [WindowMode::Windowed, WindowMode::Exclusive] {
        options.window_mode = window_mode;
        options.set_display_mode(old.clone());
        controller.boot(&mut backend, &options, now);
        let requests = backend.requests.len();
        let accepted = options.clone();
        for expected in [&new, &old, &new] {
            assert_eq!(
                controller.step(&mut backend, &mut options, true, now),
                Change::SelectionChanged
            );
            assert_eq!(controller.picked.as_ref(), Some(expected));
            assert_eq!(options, accepted);
            assert_eq!(controller.can_apply(), expected != &old);
        }
        assert_eq!(backend.requests.len(), requests);
        assert!(controller.pending.is_none());
        assert_eq!(
            controller.apply_selected(&mut backend, &mut options, now),
            if window_mode == WindowMode::Exclusive {
                Change::OpenConfirm
            } else {
                Change::Accepted
            }
        );
        if window_mode == WindowMode::Exclusive {
            assert_eq!(options, accepted);
            assert_eq!(controller.revert(&mut backend, now), Change::Reverted);
            assert_eq!(controller.picked, Some(old.clone()));
            assert!(!controller.can_apply());
            assert_eq!(options, accepted);
        } else {
            assert_eq!(options.display_mode, Some(new.clone()));
            assert!(!controller.can_apply());
            assert_eq!(backend.requests.len(), requests);
        }
    }
    controller.request_mode(&mut backend, &mut options, WindowMode::Borderless, now);
    let prior = options.clone();
    let requests = backend.requests.len();
    assert_eq!(
        controller.step(&mut backend, &mut options, true, now),
        Change::None
    );
    assert_eq!(
        controller.apply_selected(&mut backend, &mut options, now),
        Change::None
    );
    assert_eq!(backend.requests.len(), requests);
    assert_eq!(options, prior);

    controller.request_mode(&mut backend, &mut options, WindowMode::Windowed, now);
    backend.modes.borrow_mut().clear();
    let prior = options.clone();
    let requests = backend.requests.len();
    assert_eq!(
        controller.step(&mut backend, &mut options, true, now),
        Change::None
    );
    assert_eq!(backend.requests.len(), requests);
    assert_eq!(options, prior);
}

#[test]
fn unset_display_mode_uses_launch_default_without_saving_until_keep() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("settings.toml");
    let desktop = mode(1920, 144_000, "current");
    let mut backend = FakeBackend::default();
    backend.desktop = Some(desktop.clone());
    backend.modes.borrow_mut().push(desktop.clone());
    let mut options = PlayerOptions::default();
    options.save(&path).unwrap();
    let prior = options.clone();
    let now = Instant::now();
    let mut controller = Controller::new(false);
    controller.boot(&mut backend, &options, now);
    assert_eq!(controller.picked, Some(desktop.clone()));
    assert!(backend.requests.is_empty());
    assert_eq!(options, prior);
    assert_eq!(PlayerOptions::load(&path), prior);
    assert_eq!(
        controller.request_mode(&mut backend, &mut options, WindowMode::Exclusive, now),
        Change::OpenConfirm
    );
    assert_eq!(
        backend.requests.last().unwrap().display,
        Some(desktop.clone())
    );
    assert_eq!(options, prior);
    assert_eq!(controller.revert(&mut backend, now), Change::Reverted);
    assert_eq!(controller.picked, Some(desktop.clone()));
    assert_eq!(options, prior);
    assert_eq!(
        controller.request_mode(&mut backend, &mut options, WindowMode::Exclusive, now),
        Change::OpenConfirm
    );
    assert_eq!(controller.keep(&mut options), Change::Accepted);
    options.save(&path).unwrap();
    assert_eq!(
        PlayerOptions::load(&path).display_mode,
        Some(desktop.clone())
    );

    let mut unset_exclusive = PlayerOptions::default();
    unset_exclusive.window_mode = WindowMode::Exclusive;
    let mut launch = Controller::new(false);
    launch.boot(&mut backend, &unset_exclusive, now);
    assert_eq!(launch.effective.mode, WindowMode::Exclusive);
    assert_eq!(backend.requests.last().unwrap().display, Some(desktop));
    assert!(
        launch.pending.is_none(),
        "saved exclusive boot does not show a live confirm"
    );
    assert!(
        unset_exclusive.display_mode.is_none(),
        "boot never writes the resolved default to the store"
    );

    // A missing saved tuple still earns the fallback rather than being replaced
    // by the desktop default. The boot escape never changes that preference.
    options.set_display_mode(mode(1280, 60_000, "disconnected"));
    let saved = options.clone();
    controller.boot(&mut backend, &options, now);
    assert!(controller.fallback);
    assert_eq!(controller.picked, None);
    assert_eq!(options, saved);
    let mut escape = Controller::new(true);
    let requests = backend.requests.len();
    escape.boot(&mut backend, &options, now);
    assert_eq!(backend.requests.len(), requests);
    assert_eq!(options, saved);
}

#[path = "readback_tests.rs"]
mod readback;

#[test]
fn filtered_browse_keeps_raw_boot_and_revert_modes_and_exact_saved_refresh() {
    let mut legacy = mode(1280, 65_000, "current");
    legacy.height = 1024;
    let small = mode(1280, 59_000, "current");
    let mut large = mode(1920, 119_000, "current");
    large.height = 1080;
    let mut backend = FakeBackend::default();
    backend.monitor_size = Some([1920, 1080]);
    *backend.modes.borrow_mut() = vec![legacy.clone(), small.clone(), large.clone()];
    let mut options = PlayerOptions::default();
    options.window_mode = WindowMode::Exclusive;
    options.set_display_mode(legacy.clone());
    let mut controller = Controller::new(false);
    let now = Instant::now();
    controller.boot(&mut backend, &options, now);
    assert_eq!(
        backend.requests.last().unwrap().display,
        Some(legacy.clone())
    );
    assert!(!controller.fallback);
    assert!(!controller.can_apply());
    assert_eq!(
        controller.step(&mut backend, &mut options, true, now),
        Change::SelectionChanged
    );
    assert_eq!(controller.picked, Some(small.clone()));
    assert!(controller.can_apply());
    assert_eq!(
        controller.apply_selected(&mut backend, &mut options, now),
        Change::OpenConfirm
    );
    assert!(!controller.can_apply());
    assert_eq!(controller.revert(&mut backend, now), Change::Reverted);
    assert_eq!(backend.requests.last().unwrap().display, Some(legacy));
    assert!(!controller.can_apply());
    for expected in [&large, &small, &large] {
        assert_eq!(
            controller.step(&mut backend, &mut options, false, now),
            Change::SelectionChanged
        );
        assert_eq!(controller.picked.as_ref(), Some(expected));
    }
    assert_eq!(
        controller.apply_selected(&mut backend, &mut options, now),
        Change::OpenConfirm
    );
    assert_eq!(controller.keep(&mut options), Change::Accepted);
    assert!(!controller.can_apply());
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("settings.toml");
    options.save(&path).unwrap();
    assert_eq!(PlayerOptions::load(&path).display_mode, Some(large.clone()));

    // A changed monitor aspect invalidates the draft but not the accepted tuple.
    controller.step(&mut backend, &mut options, true, now);
    assert!(controller.can_apply());
    backend.monitor_size = Some([1280, 1024]);
    let requests = backend.requests.len();
    assert_eq!(
        controller.apply_selected(&mut backend, &mut options, now),
        Change::SelectionChanged
    );
    assert_eq!(backend.requests.len(), requests);
    assert!(!controller.can_apply());
    assert_eq!(controller.picked, Some(large));
}

#[test]
fn boot_interim_shows_saved_exclusive_as_borderless_only_when_deferred() {
    let mut options = PlayerOptions::default();
    options.window_mode = WindowMode::Exclusive;
    let controller = Controller::new(false);
    assert_eq!(
        controller.boot_interim(&options, true),
        Some(Target {
            mode: WindowMode::Borderless,
            display: None,
        })
    );
    assert_eq!(controller.boot_interim(&options, false), None);
}

#[test]
fn boot_interim_leaves_non_exclusive_modes_to_boot() {
    let controller = Controller::new(false);
    for mode in [WindowMode::Windowed, WindowMode::Borderless] {
        let mut options = PlayerOptions::default();
        options.window_mode = mode;
        assert_eq!(controller.boot_interim(&options, true), None);
    }
}

#[test]
fn boot_interim_respects_windowed_escape() {
    let mut options = PlayerOptions::default();
    options.window_mode = WindowMode::Exclusive;
    let controller = Controller::new(true);
    assert_eq!(controller.boot_interim(&options, true), None);
}
