// Accessibility half of the options bridge: projection, working copies, OS
// following, and engine-write reseeds.
// See: context/lib/player_options.md §5

use std::path::Path;

use log::Level;
use postretro_entities::ScriptCtx;
use postretro_entities::slot_table::SlotValue;
use postretro_scripting_core::store_bridge::write_state_slot_json;
use postretro_test_log_capture::LogCapture;
use serde_json::json;

use super::super::resolved::OsPreferences;
use super::super::{AccessibilityOptions, PlayerOptions};
use super::OptionsBridge;
use crate::input::InputSystem;

const REDUCE_MOTION: &str = "accessibility.reduceMotion";
const FOLLOWS_SYSTEM: &str = "accessibility.reduceMotionFollowsSystem";
const REDUCE_MOTION_COPY: &str = "options.reduceMotion";

struct Rig {
    ctx: ScriptCtx,
    options: PlayerOptions,
    input: InputSystem,
    bridge: OptionsBridge,
}

impl Rig {
    /// A session just built: the bridge seeded every accessibility slot once.
    fn new(options: PlayerOptions) -> Self {
        let ctx = ScriptCtx::new();
        let mut bridge = OptionsBridge::new();
        bridge.seed_accessibility(&mut ctx.slot_table.borrow_mut(), &options);
        Self {
            ctx,
            options,
            input: InputSystem::new(crate::input::default_bindings()),
            bridge,
        }
    }

    fn frame(&mut self) -> super::OptionsApplyEffects {
        self.frame_saving(None)
    }

    fn frame_saving(&mut self, path: Option<&Path>) -> super::OptionsApplyEffects {
        self.bridge.update(
            0.0,
            &mut self.ctx.slot_table.borrow_mut(),
            &mut self.options,
            &mut self.input,
            path,
        )
    }

    /// A mod menu control writing a working copy through `setState`.
    fn menu_write(&self, slot: &str, value: serde_json::Value) {
        write_state_slot_json(&self.ctx, slot, &value).expect("working copy accepts the write");
    }

    fn slot(&self, name: &str) -> Option<SlotValue> {
        self.ctx
            .slot_table
            .borrow()
            .get(name)
            .unwrap()
            .value
            .clone()
    }

    fn generation(&self, name: &str) -> u64 {
        self.ctx
            .slot_table
            .borrow()
            .get(name)
            .unwrap()
            .write_generation()
    }

    fn os_reduce_motion(&mut self, value: Option<bool>) {
        self.bridge.set_os_preferences(OsPreferences {
            reduce_motion: value,
        });
    }
}

#[test]
fn session_build_seeds_resolved_slots_and_working_copies() {
    let rig = Rig::new(PlayerOptions {
        accessibility: AccessibilityOptions {
            screen_shake_scale: 0.3,
            flash_limiter: false,
            ..AccessibilityOptions::default()
        },
        ..PlayerOptions::default()
    });
    assert_eq!(
        rig.slot("accessibility.screenShakeScale"),
        Some(SlotValue::Number(0.3))
    );
    assert_eq!(
        rig.slot("options.screenShakeScale"),
        Some(SlotValue::Number(0.3))
    );
    assert_eq!(
        rig.slot("accessibility.flashLimiter"),
        Some(SlotValue::Boolean(false))
    );
    assert_eq!(rig.slot(FOLLOWS_SYSTEM), Some(SlotValue::Boolean(true)));
}

#[test]
fn an_os_change_moves_an_unset_field_and_reseeds_without_a_menu_write() {
    let mut rig = Rig::new(PlayerOptions::default());
    rig.os_reduce_motion(Some(true));
    let effects = rig.frame();

    assert_eq!(rig.slot(REDUCE_MOTION), Some(SlotValue::Boolean(true)));
    assert_eq!(rig.slot(FOLLOWS_SYSTEM), Some(SlotValue::Boolean(true)));
    assert_eq!(rig.slot(REDUCE_MOTION_COPY), Some(SlotValue::Boolean(true)));
    assert_eq!(
        rig.options.accessibility.reduce_motion, None,
        "an OS value leaves the field unset"
    );
    assert!(effects.accessibility.is_some_and(|r| r.reduce_motion));

    // The reseed does not read back as a menu write, and a later OS change
    // still moves the field.
    rig.frame();
    assert_eq!(rig.options.accessibility.reduce_motion, None);
    rig.os_reduce_motion(Some(false));
    rig.frame();
    assert_eq!(rig.slot(REDUCE_MOTION), Some(SlotValue::Boolean(false)));
    assert_eq!(
        rig.slot(REDUCE_MOTION_COPY),
        Some(SlotValue::Boolean(false))
    );
}

#[test]
fn a_menu_write_of_the_resolved_value_marks_the_field_player_set() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("settings.toml");
    let mut rig = Rig::new(PlayerOptions::default());
    rig.os_reduce_motion(Some(true));
    rig.frame();

    // The working copy already reads true; the menu writes true anyway.
    rig.menu_write(REDUCE_MOTION_COPY, json!(true));
    rig.frame_saving(Some(&path));
    assert_eq!(rig.options.accessibility.reduce_motion, Some(true));
    assert_eq!(rig.slot(FOLLOWS_SYSTEM), Some(SlotValue::Boolean(false)));

    // It persists as a key, and a later OS change leaves it alone.
    rig.bridge
        .flush_on_options_close(&rig.options, Some(path.as_path()));
    let saved = std::fs::read_to_string(&path).unwrap();
    assert!(saved.contains("reduce_motion = true"), "{saved}");
    rig.os_reduce_motion(Some(false));
    rig.frame();
    assert_eq!(rig.slot(REDUCE_MOTION), Some(SlotValue::Boolean(true)));
}

#[test]
fn an_os_reply_and_a_menu_write_in_one_frame_leave_the_menu_value_player_set() {
    // UO1: the OS channel is polled at the frame top, the menu write drains
    // before the bridge runs. The player's write wins.
    let mut rig = Rig::new(PlayerOptions::default());
    rig.os_reduce_motion(Some(true));
    rig.menu_write(REDUCE_MOTION_COPY, json!(false));
    rig.frame();
    assert_eq!(rig.options.accessibility.reduce_motion, Some(false));
    assert_eq!(rig.slot(REDUCE_MOTION), Some(SlotValue::Boolean(false)));
    assert_eq!(
        rig.slot(REDUCE_MOTION_COPY),
        Some(SlotValue::Boolean(false))
    );
}

#[test]
fn a_menu_write_updates_the_store_and_the_resolved_slot_in_the_same_frame() {
    let mut rig = Rig::new(PlayerOptions::default());
    rig.menu_write("options.sfxVolume", json!(0.5));
    let effects = rig.frame();
    assert!((rig.options.accessibility.sfx_volume - 0.5).abs() < 1e-6);
    assert_eq!(
        rig.slot("accessibility.sfxVolume"),
        Some(SlotValue::Number(0.5)),
        "the same frame's UI snapshot carries the resolved slot"
    );
    assert!(effects.accessibility.is_some_and(|r| r.sfx_volume == 0.5));
}

#[test]
fn an_engine_write_reseeds_the_working_copy_without_counting_as_a_menu_write() {
    let mut rig = Rig::new(PlayerOptions::default());
    let copy_generation = rig.generation("options.monoAudio");

    // A panel action writes the store directly.
    rig.options.accessibility.mono_audio = true;
    rig.frame();
    assert_eq!(
        rig.slot("options.monoAudio"),
        Some(SlotValue::Boolean(true))
    );
    assert_eq!(
        rig.slot("accessibility.monoAudio"),
        Some(SlotValue::Boolean(true))
    );
    assert_ne!(rig.generation("options.monoAudio"), copy_generation);

    // The next frame sees no menu write, so nothing changes or saves.
    let effects = rig.frame();
    assert!(effects.accessibility.is_none());
    assert!(rig.options.accessibility.mono_audio);
}

#[test]
fn a_settled_frame_writes_no_slot() {
    let mut rig = Rig::new(PlayerOptions::default());
    rig.frame();
    let before = rig.generation(REDUCE_MOTION);
    let copy_before = rig.generation(REDUCE_MOTION_COPY);
    for _ in 0..3 {
        rig.frame();
    }
    assert_eq!(rig.generation(REDUCE_MOTION), before);
    assert_eq!(rig.generation(REDUCE_MOTION_COPY), copy_before);
}

#[test]
fn a_script_write_to_a_resolved_slot_warns_and_changes_nothing() {
    let capture = LogCapture::start();
    let mut rig = Rig::new(PlayerOptions::default());
    let result = write_state_slot_json(&rig.ctx, "accessibility.flashLimiter", &json!(false));
    assert!(
        result.is_ok(),
        "a readonly write warns rather than erroring"
    );
    capture.assert_logged(Level::Warn, "rejected write to readonly slot");
    rig.frame();
    assert_eq!(
        rig.slot("accessibility.flashLimiter"),
        Some(SlotValue::Boolean(true))
    );
    assert!(rig.options.accessibility.flash_limiter);
}
