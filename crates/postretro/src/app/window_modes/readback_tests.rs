// Readback traces cover asynchronous/failed requests and persistence feedback.
// See: context/lib/player_options.md §7
use super::*;
use crate::options::OptionsBridge;
use crate::input::{InputSystem, default_bindings};
use postretro_entities::ScriptCtx;
use postretro_scripting_core::store_bridge::write_state_slot_json;
use std::time::Duration;

#[test]
fn os_readback_persists_once_and_reseed_emits_no_request() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("settings.toml");
    let mut options = PlayerOptions::default();
    options.save(&path).unwrap();
    let ctx = ScriptCtx::new();
    let mut bridge = OptionsBridge::new();
    let mut input = InputSystem::new(default_bindings());
    bridge.seed_on_open(&mut ctx.slot_table.borrow_mut(), &options);
    let mut controller = Controller::new(false);
    let mut backend = FakeBackend::default();
    let now = Instant::now();
    controller.boot(&mut backend, &options, now);
    assert_eq!(
        controller.observe(&mut backend, Some(&mut options), now),
        Change::None
    );
    backend.actual.mode = WindowMode::Borderless;
    assert_eq!(
        controller.observe(&mut backend, Some(&mut options), now),
        Change::Accepted
    );
    bridge.reseed_window_mode(&mut ctx.slot_table.borrow_mut(), options.window_mode);
    bridge.schedule_save(Some(&path));
    let effects = bridge.update(
        0.3,
        &mut ctx.slot_table.borrow_mut(),
        &mut options,
        &mut input,
        Some(&path),
    );
    assert!(effects.window_mode.is_none());
    assert!(backend.requests.is_empty());
    assert_eq!(
        PlayerOptions::load(&path).window_mode,
        WindowMode::Borderless
    );
    let generation = ctx
        .slot_table
        .borrow()
        .get("options.windowMode")
        .unwrap()
        .write_generation();
    let accepted = options.clone();
    for _ in 0..30 {
        assert_eq!(
            controller.observe(&mut backend, Some(&mut options), now),
            Change::None
        );
        super::super::projection::project(&controller, &mut ctx.slot_table.borrow_mut(), now);
    }
    assert_eq!(options, accepted);
    assert_eq!(
        ctx.slot_table
            .borrow()
            .get("options.windowMode")
            .unwrap()
            .write_generation(),
        generation
    );
}

#[test]
fn transition_stale_reading_and_failed_entry_adopt_unwritten_baseline() {
    for failed in [false, true] {
        let mut options = PlayerOptions::default();
        let mut controller = Controller::new(false);
        let mut backend = FakeBackend::default();
        let now = Instant::now();
        assert_eq!(
            controller.request_mode(&mut backend, &mut options, WindowMode::Borderless, now),
            Change::Accepted
        );
        let requested = options.clone();
        backend.actual = Target::default();
        assert_eq!(
            controller.observe(
                &mut backend,
                Some(&mut options),
                now + Duration::from_secs(1)
            ),
            Change::None
        );
        if !failed {
            backend.actual.mode = WindowMode::Borderless;
        }
        assert_eq!(
            controller.observe(&mut backend, Some(&mut options), now + SETTLE_TIME),
            Change::None
        );
        assert_eq!(options, requested);
        // Failed entry stays unwritten after settling; subsequent OS change is accepted.
        assert_eq!(
            controller.observe(&mut backend, Some(&mut options), now + SETTLE_TIME),
            Change::None
        );
        backend.actual.mode = if failed {
            WindowMode::Borderless
        } else {
            WindowMode::Windowed
        };
        assert_eq!(
            controller.observe(&mut backend, Some(&mut options), now + SETTLE_TIME),
            Change::Accepted
        );
        assert_eq!(options.window_mode, backend.actual.mode);
    }
}

#[test]
fn pending_and_fallback_readback_never_persist_even_after_settling() {
    for fallback in [false, true] {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("settings.toml");
        let mut options = PlayerOptions::default();
        options.set_display_mode(mode(1280, 60000, "current"));
        let mut controller = Controller::new(false);
        let mut backend = FakeBackend::default();
        let now = Instant::now();
        if fallback {
            options.window_mode = WindowMode::Exclusive;
            controller.boot(&mut backend, &options, now);
        } else {
            backend
                .modes
                .borrow_mut()
                .push(options.display_mode.clone().unwrap());
            assert_eq!(
                controller.request_mode(&mut backend, &mut options, WindowMode::Exclusive, now),
                Change::OpenConfirm
            );
        }
        let prior = options.clone();
        for seconds in [1, 3, 4, 8, 14] {
            backend.actual.mode = WindowMode::Windowed;
            assert_eq!(
                controller.observe(
                    &mut backend,
                    Some(&mut options),
                    now + Duration::from_secs(seconds)
                ),
                Change::None
            );
            options.save(&path).unwrap();
            assert_eq!(PlayerOptions::load(&path), prior);
        }
        if fallback {
            let loaded = PlayerOptions::load(&path);
            let mut relaunched = Controller::new(false);
            relaunched.boot(&mut backend, &loaded, now);
            assert!(relaunched.fallback);
            assert_eq!(loaded.window_mode, WindowMode::Exclusive);
        }
    }
}

#[test]
fn same_saved_slot_request_under_escape_and_fallback_reapplies() {
    for escape in [false, true] {
        for matching in [false, true] {
            let mut options = PlayerOptions::default();
            options.window_mode = WindowMode::Exclusive;
            options.set_display_mode(mode(1280, 60000, "current"));
            let ctx = ScriptCtx::new();
            let mut bridge = OptionsBridge::new();
            let mut input = InputSystem::new(default_bindings());
            bridge.seed_on_open(&mut ctx.slot_table.borrow_mut(), &options);
            let mut controller = Controller::new(escape);
            let mut backend = FakeBackend::default();
            let now = Instant::now();
            controller.boot(&mut backend, &options, now);
            if matching {
                backend
                    .modes
                    .borrow_mut()
                    .push(options.display_mode.clone().unwrap());
            }
            write_state_slot_json(&ctx, "options.windowMode", &serde_json::json!("exclusive"))
                .unwrap();
            let effects = bridge.update(
                0.0,
                &mut ctx.slot_table.borrow_mut(),
                &mut options,
                &mut input,
                None,
            );
            let requests = backend.requests.len();
            assert_eq!(effects.window_mode, Some(WindowMode::Exclusive));
            let change = controller.request_mode(
                &mut backend,
                &mut options,
                effects.window_mode.unwrap(),
                now,
            );
            assert_eq!(backend.requests.len(), requests + 1);
            assert_eq!(
                change,
                if matching {
                    Change::OpenConfirm
                } else {
                    Change::Accepted
                }
            );
            assert_eq!(controller.fallback, !matching);
            assert!(!controller.escape);
        }
    }
}

#[test]
fn empty_choices_project_zero_and_idle_projection_writes_nothing() {
    let mut options = PlayerOptions::default();
    options.window_mode = WindowMode::Exclusive;
    options.set_display_mode(mode(1280, 60000, "gone"));
    let mut controller = Controller::new(false);
    let mut backend = FakeBackend::default();
    let now = Instant::now();
    controller.boot(&mut backend, &options, now);
    let ctx = ScriptCtx::new();
    super::super::projection::project(&controller, &mut ctx.slot_table.borrow_mut(), now);
    let names = [
        "window.displayModeWidth",
        "window.displayModeHeight",
        "window.displayModeRefreshHz",
        "window.displayModeBitDepth",
        "window.displayModeRevertSeconds",
        "window.displayModeMonitor",
    ];
    let before: Vec<_> = names
        .iter()
        .map(|name| {
            ctx.slot_table
                .borrow()
                .get(name)
                .unwrap()
                .write_generation()
        })
        .collect();
    super::super::projection::project(&controller, &mut ctx.slot_table.borrow_mut(), now);
    for (name, generation) in names.into_iter().zip(before) {
        let table = ctx.slot_table.borrow();
        let slot = table.get(name).unwrap();
        assert_eq!(slot.write_generation(), generation);
        assert!(
            matches!(
                slot.value.as_ref(),
                Some(postretro_entities::SlotValue::Number(0.0))
            ) || matches!(slot.value.as_ref(), Some(postretro_entities::SlotValue::String(text)) if text.is_empty())
        );
    }
}

#[test]
fn every_redraw_reads_once_before_boot_and_loading_services_deadline() {
    let source = include_str!("../../main.rs");
    let body = &source[source.find("WindowEvent::RedrawRequested =>").unwrap()..];
    assert_eq!(body.matches("self.poll_window_mode_readback();").count(), 1);
    assert!(
        body.find("self.poll_window_mode_readback();").unwrap()
            < body.find("self.drive_boot_state_for_redraw(").unwrap()
    );
    let early_return = &body[body.find("if !self.drive_boot_state_for_redraw(").unwrap()..];
    assert!(
        early_return.find("self.service_window_modes();").unwrap()
            < early_return.find("return;").unwrap()
    );
}
