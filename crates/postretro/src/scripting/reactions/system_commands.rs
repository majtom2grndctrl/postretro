// System-reaction command path: the second execution arm of the shared
// named-reaction vocabulary. Entity reactions mutate `EntityRegistry`; system
// reactions (no `tag`) push typed commands onto a per-frame queue the app
// drains after the post-tick event drains, so audio/input/UI subsystems
// consume their commands without threading engine services into scripting.
// See: context/lib/scripting.md §10.4

use std::collections::HashMap;

use super::ReactionError;
pub(crate) use postretro_entities::reactions::system_commands::{
    SystemCommandQueue, SystemReactionCommand,
};

/// A system-reaction handler: parses `args` and enqueues zero or more typed
/// commands. Unlike [`super::registry::ReactionPrimitiveFn`], it touches no
/// `EntityRegistry` — system reactions target no entities.
pub(crate) type SystemReactionFn =
    Box<dyn Fn(&serde_json::Value, &SystemCommandQueue) -> Result<(), ReactionError>>;

/// Name → handler table for the system-reaction arm. Registered at startup
/// alongside the entity-targeted `ReactionPrimitiveRegistry`; both share the
/// one named-event vocabulary, so a `Primitive` reaction with no `tag`
/// resolves here while one with a `tag` resolves against the entity registry.
#[derive(Default)]
pub(crate) struct SystemReactionRegistry {
    handlers: HashMap<String, SystemReactionFn>,
}

impl SystemReactionRegistry {
    pub(crate) fn new() -> Self {
        Self::default()
    }

    pub(crate) fn register<F>(&mut self, name: impl Into<String>, handler: F)
    where
        F: Fn(&serde_json::Value, &SystemCommandQueue) -> Result<(), ReactionError> + 'static,
    {
        let name = name.into();
        if self.handlers.contains_key(&name) {
            debug_assert!(false, "duplicate system reaction registration: {name}");
            log::warn!(
                "[Scripting] SystemReactionRegistry: overwriting existing handler for '{name}'"
            );
        }
        self.handlers.insert(name, Box::new(handler));
    }

    pub(crate) fn contains(&self, name: &str) -> bool {
        self.handlers.contains_key(name)
    }

    /// Resolve `name` and run its handler, enqueueing onto `queue`. Returns
    /// `Ok(false)` when no handler is registered — callers log this defensively,
    /// mirroring [`super::registry::ReactionPrimitiveRegistry::dispatch`].
    pub(crate) fn dispatch(
        &self,
        name: &str,
        args: &serde_json::Value,
        queue: &SystemCommandQueue,
    ) -> Result<bool, ReactionError> {
        let Some(handler) = self.handlers.get(name) else {
            return Ok(false);
        };
        handler(args, queue).map(|_| true)
    }
}

impl std::fmt::Debug for SystemReactionRegistry {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SystemReactionRegistry")
            .field("handlers", &self.handlers.keys().collect::<Vec<_>>())
            .finish()
    }
}

/// Maximum `screenShake` amplitude in logical-reference px. Matches
/// `SHAKE_REFERENCE_WIDTH` in `render/screen_effects.rs` (1280 px): at this
/// amplitude the peak UV offset is exactly 1.0 — one full reference frame-width
/// — which is the natural ceiling of the 1280×720 reference coordinate system.
/// Amplitudes beyond this produce UV offsets > 1.0 that cause whole-frame
/// ClampToEdge edge-smear with no meaningful additional shake effect.
const MAX_SHAKE_AMPLITUDE_PX: f32 = 1280.0;

/// Register the system-reaction primitives onto `registry`:
/// - Audio: `playSound`
/// - Input/rumble: `rumble`
/// - Display/flash: `flashScreen`
/// - Screen-space effects: `vignette`, `screenShake`
/// - UI stack: `showDialog`, `openMenu`, `closeDialog` (push/pop `PushTree`/`PopTree`)
/// - Game flow: `loadLevel`, `restartLevel`, `returnToFrontend`
/// - Slot write: `setState`
/// - Presentation-cell write: `cellWrite`
/// - Text-edit: `appendText`, `backspaceText`, `clearText`
pub(crate) fn register_system_reaction_primitives(registry: &mut SystemReactionRegistry) {
    registry.register("playSound", |args, queue| {
        let parsed: PlaySoundArgs =
            serde_json::from_value(args.clone()).map_err(|e| ReactionError::InvalidArgument {
                reason: format!("playSound: failed to deserialize args: {e}"),
            })?;
        queue.push(SystemReactionCommand::PlaySound {
            sound: parsed.sound,
            bus: parsed.bus,
        });
        Ok(())
    });
    registry.register("rumble", |args, queue| {
        let parsed: RumbleArgs =
            serde_json::from_value(args.clone()).map_err(|e| ReactionError::InvalidArgument {
                reason: format!("rumble: failed to deserialize args: {e}"),
            })?;
        queue.push(SystemReactionCommand::Rumble {
            strong: parsed.strong,
            weak: parsed.weak,
            duration_ms: parsed.duration_ms,
        });
        Ok(())
    });
    registry.register("flashScreen", |args, queue| {
        let parsed: FlashScreenArgs =
            serde_json::from_value(args.clone()).map_err(|e| ReactionError::InvalidArgument {
                reason: format!("flashScreen: failed to deserialize args: {e}"),
            })?;
        queue.push(SystemReactionCommand::FlashScreen {
            color: parsed.color,
            duration_ms: parsed.duration_ms,
        });
        Ok(())
    });
    // `vignette` darkens (or tints) the screen edges, rising then decaying. An
    // omitted `color` defaults to black (pure strength-only edge-darken); the
    // default is applied at the drain when the command maps onto
    // `VignetteDecay::start`, so the absent-color case is one behavior.
    registry.register("vignette", |args, queue| {
        let parsed: VignetteArgs =
            serde_json::from_value(args.clone()).map_err(|e| ReactionError::InvalidArgument {
                reason: format!("vignette: failed to deserialize args: {e}"),
            })?;
        // The JSON bridge (serde_json::Number::from_f64) maps non-finite f32
        // values to JSON null, so NaN/Infinity arrive as null and are rejected
        // at deserialize before reaching this guard. The is_finite() arm is
        // defense-in-depth for any future non-JSON caller.
        if !parsed.duration_ms.is_finite() || parsed.duration_ms <= 0.0 {
            return Err(ReactionError::InvalidArgument {
                reason: format!(
                    "vignette: durationMs must be finite and > 0, got {}",
                    parsed.duration_ms
                ),
            });
        }
        queue.push(SystemReactionCommand::Vignette {
            color: parsed.color,
            strength: parsed.strength,
            duration_ms: parsed.duration_ms,
        });
        Ok(())
    });
    // `screenShake` starts a decaying oscillation. The omitted-frequency 18 Hz
    // default is applied by the DRIVER (`ShakeDecay::start`), not here — the
    // deserializer passes `None` through unchanged so the absent-frequency case
    // is one behavior regardless of how the reaction surface parses its args.
    registry.register("screenShake", |args, queue| {
        let parsed: ScreenShakeArgs =
            serde_json::from_value(args.clone()).map_err(|e| ReactionError::InvalidArgument {
                reason: format!("screenShake: failed to deserialize args: {e}"),
            })?;
        // The JSON bridge (serde_json::Number::from_f64) maps non-finite f32
        // values to JSON null, so NaN/Infinity arrive as null and are rejected
        // at deserialize before reaching this guard. The is_finite() arm is
        // defense-in-depth for any future non-JSON caller.
        if !parsed.duration_ms.is_finite() || parsed.duration_ms <= 0.0 {
            return Err(ReactionError::InvalidArgument {
                reason: format!(
                    "screenShake: durationMs must be finite and > 0, got {}",
                    parsed.duration_ms
                ),
            });
        }
        // amplitude == 0.0 is a valid no-op shake (zero displacement); reject
        // only non-finite and negative. The upper bound caps the peak UV offset
        // at 1.0 (one full reference frame-width) to prevent whole-frame
        // ClampToEdge edge-smear. The JSON bridge maps non-finite to null
        // (rejected at deserialize); the is_finite() arm is defense-in-depth.
        if !parsed.amplitude.is_finite() || parsed.amplitude < 0.0 {
            return Err(ReactionError::InvalidArgument {
                reason: format!(
                    "screenShake: amplitude must be finite and >= 0, got {}",
                    parsed.amplitude
                ),
            });
        }
        if parsed.amplitude > MAX_SHAKE_AMPLITUDE_PX {
            return Err(ReactionError::InvalidArgument {
                reason: format!(
                    "screenShake: amplitude must be <= {MAX_SHAKE_AMPLITUDE_PX} px, got {}",
                    parsed.amplitude
                ),
            });
        }
        if let Some(freq) = parsed.frequency {
            // The JSON bridge maps non-finite to null (rejected at deserialize);
            // the is_finite() arm is defense-in-depth for non-JSON callers.
            if !freq.is_finite() || freq <= 0.0 {
                return Err(ReactionError::InvalidArgument {
                    reason: format!("screenShake: frequency must be finite and > 0, got {freq}"),
                });
            }
        }
        queue.push(SystemReactionCommand::ScreenShake {
            amplitude: parsed.amplitude,
            duration_ms: parsed.duration_ms,
            frequency: parsed.frequency,
        });
        Ok(())
    });
    // `showDialog` / `openMenu` are v1 aliases: both push a `PushTree` for a
    // named registered tree. `showDialog` carries the optional `onCommit`
    // reaction fired when the pushed tree commits; `openMenu` never does (a menu
    // has no commit payload), so its handler ignores any `onCommit` key. The
    // capture mode etc. travel on the tree's registered envelope (F's concern),
    // not the command. `closeDialog` pops the top tree.
    registry.register("showDialog", |args, queue| {
        let parsed: ShowDialogArgs =
            serde_json::from_value(args.clone()).map_err(|e| ReactionError::InvalidArgument {
                reason: format!("showDialog: failed to deserialize args: {e}"),
            })?;
        queue.push(SystemReactionCommand::PushTree {
            tree: parsed.tree,
            on_commit: parsed.on_commit,
        });
        Ok(())
    });
    registry.register("openMenu", |args, queue| {
        let parsed: OpenMenuArgs =
            serde_json::from_value(args.clone()).map_err(|e| ReactionError::InvalidArgument {
                reason: format!("openMenu: failed to deserialize args: {e}"),
            })?;
        queue.push(SystemReactionCommand::PushTree {
            tree: parsed.tree,
            // A menu carries no commit payload; the alias never sets `on_commit`.
            on_commit: None,
        });
        Ok(())
    });
    registry.register("closeDialog", |_args, queue| {
        queue.push(SystemReactionCommand::PopTree);
        Ok(())
    });
    registry.register("loadLevel", |args, queue| {
        let parsed: LoadLevelArgs =
            serde_json::from_value(args.clone()).map_err(|e| ReactionError::InvalidArgument {
                reason: format!("loadLevel: failed to deserialize args: {e}"),
            })?;
        queue.push(SystemReactionCommand::LoadLevel { map: parsed.map });
        Ok(())
    });
    registry.register("restartLevel", |_args, queue| {
        queue.push(SystemReactionCommand::RestartLevel);
        Ok(())
    });
    registry.register("returnToFrontend", |_args, queue| {
        queue.push(SystemReactionCommand::ReturnToFrontend);
        Ok(())
    });
    // `setState` writes a value to a writable store slot at the game-logic stage
    // (M13 Goal F, Task 4). It carries no `tag` (system-targeted); the drain
    // applies it through the readonly-gated JSON write. The slider widget emits
    // this on a captured nav step; scripts may fire it as a named reaction.
    registry.register("setState", |args, queue| {
        let parsed: SetStateArgs =
            serde_json::from_value(args.clone()).map_err(|e| ReactionError::InvalidArgument {
                reason: format!("setState: failed to deserialize args: {e}"),
            })?;
        queue.push(SystemReactionCommand::SetState {
            slot: parsed.slot,
            value: parsed.value,
        });
        Ok(())
    });
    // `cellWrite` writes a presentation cell at the game-logic stage.
    // It carries no `tag` (system-targeted); the drain routes it into the
    // app-side `PresentationCellStore`, NOT the slot table. Distinct from
    // `setState` (which writes the authoritative store); the `ui.createLocalState`
    // handle's `.set(v)` emits this, never `setState`.
    registry.register("cellWrite", |args, queue| {
        let parsed: CellWriteArgs =
            serde_json::from_value(args.clone()).map_err(|e| ReactionError::InvalidArgument {
                reason: format!("cellWrite: failed to deserialize args: {e}"),
            })?;
        queue.push(SystemReactionCommand::CellWrite {
            scope: parsed.scope,
            cell: parsed.cell,
            value: parsed.value,
        });
        Ok(())
    });
    // Text-edit reactions (M13 Text Entry, Task 1): `appendText` / `backspaceText`
    // / `clearText` mutate a writable String slot at the game-logic stage through
    // the same readonly-gated path as `setState`. No `tag` — system-targeted. The
    // hardware-keyboard and on-screen-keyboard paths both fire these against
    // `ui.textEntry`.
    registry.register("appendText", |args, queue| {
        let parsed: AppendTextArgs =
            serde_json::from_value(args.clone()).map_err(|e| ReactionError::InvalidArgument {
                reason: format!("appendText: failed to deserialize args: {e}"),
            })?;
        queue.push(SystemReactionCommand::AppendText {
            slot: parsed.slot,
            text: parsed.text,
        });
        Ok(())
    });
    registry.register("backspaceText", |args, queue| {
        let parsed: SlotOnlyArgs =
            serde_json::from_value(args.clone()).map_err(|e| ReactionError::InvalidArgument {
                reason: format!("backspaceText: failed to deserialize args: {e}"),
            })?;
        queue.push(SystemReactionCommand::BackspaceText { slot: parsed.slot });
        Ok(())
    });
    registry.register("clearText", |args, queue| {
        let parsed: SlotOnlyArgs =
            serde_json::from_value(args.clone()).map_err(|e| ReactionError::InvalidArgument {
                reason: format!("clearText: failed to deserialize args: {e}"),
            })?;
        queue.push(SystemReactionCommand::ClearText { slot: parsed.slot });
        Ok(())
    });
}

// --- args shapes ------------------------------------------------------------
// Script-facing camelCase keys; absent optionals fall through serde defaults.

#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct PlaySoundArgs {
    sound: String,
    #[serde(default)]
    bus: Option<String>,
}

#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct RumbleArgs {
    strong: f32,
    #[serde(default)]
    weak: Option<f32>,
    duration_ms: f32,
}

#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct FlashScreenArgs {
    color: [f32; 4],
    duration_ms: f32,
}

#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct VignetteArgs {
    // Absent ⇒ the drain defaults to black (strength-only edge-darken).
    #[serde(default)]
    color: Option<[f32; 3]>,
    strength: f32,
    duration_ms: f32,
}

#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct ScreenShakeArgs {
    amplitude: f32,
    duration_ms: f32,
    // Absent ⇒ `None` rides to the driver, which applies its 18 Hz default. The
    // default is NOT applied here.
    #[serde(default)]
    frequency: Option<f32>,
}

#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct ShowDialogArgs {
    tree: String,
    #[serde(default)]
    on_commit: Option<String>,
}

#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct OpenMenuArgs {
    tree: String,
}

#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct LoadLevelArgs {
    map: String,
}

#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct SetStateArgs {
    slot: String,
    value: serde_json::Value,
}

#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct CellWriteArgs {
    scope: String,
    cell: String,
    value: serde_json::Value,
}

#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct AppendTextArgs {
    slot: String,
    text: String,
}

#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct SlotOnlyArgs {
    slot: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn registers_all_system_reaction_primitives_under_expected_names() {
        let mut r = SystemReactionRegistry::new();
        register_system_reaction_primitives(&mut r);
        assert!(r.contains("playSound"));
        assert!(r.contains("rumble"));
        assert!(r.contains("flashScreen"));
        assert!(r.contains("vignette"));
        assert!(r.contains("screenShake"));
        assert!(r.contains("showDialog"));
        assert!(r.contains("openMenu"));
        assert!(r.contains("closeDialog"));
        assert!(r.contains("loadLevel"));
        assert!(r.contains("restartLevel"));
        assert!(r.contains("returnToFrontend"));
        assert!(r.contains("setState"));
        assert!(r.contains("cellWrite"));
        assert!(r.contains("appendText"));
        assert!(r.contains("backspaceText"));
        assert!(r.contains("clearText"));
        // Defensive: system reactions are a distinct arm; entity primitives
        // are NOT registered here.
        assert!(!r.contains("setEmitterRate"));
    }

    #[test]
    fn play_sound_dispatch_enqueues_command_with_bus() {
        let mut r = SystemReactionRegistry::new();
        register_system_reaction_primitives(&mut r);
        let queue = SystemCommandQueue::new();

        let args = serde_json::json!({ "sound": "alarm", "bus": "sfx" });
        assert!(r.dispatch("playSound", &args, &queue).unwrap());

        assert_eq!(
            queue.take(),
            vec![SystemReactionCommand::PlaySound {
                sound: "alarm".to_string(),
                bus: Some("sfx".to_string()),
            }]
        );
    }

    #[test]
    fn play_sound_dispatch_defaults_absent_bus_to_none() {
        let mut r = SystemReactionRegistry::new();
        register_system_reaction_primitives(&mut r);
        let queue = SystemCommandQueue::new();

        let args = serde_json::json!({ "sound": "alarm" });
        assert!(r.dispatch("playSound", &args, &queue).unwrap());
        assert_eq!(
            queue.take(),
            vec![SystemReactionCommand::PlaySound {
                sound: "alarm".to_string(),
                bus: None,
            }]
        );
    }

    #[test]
    fn rumble_dispatch_enqueues_command() {
        let mut r = SystemReactionRegistry::new();
        register_system_reaction_primitives(&mut r);
        let queue = SystemCommandQueue::new();

        let args = serde_json::json!({ "strong": 0.8, "durationMs": 200.0 });
        assert!(r.dispatch("rumble", &args, &queue).unwrap());
        assert_eq!(
            queue.take(),
            vec![SystemReactionCommand::Rumble {
                strong: 0.8,
                weak: None,
                duration_ms: 200.0,
            }]
        );
    }

    #[test]
    fn vignette_dispatch_enqueues_command_with_color() {
        let mut r = SystemReactionRegistry::new();
        register_system_reaction_primitives(&mut r);
        let queue = SystemCommandQueue::new();

        let args =
            serde_json::json!({ "color": [0.1, 0.0, 0.2], "strength": 0.8, "durationMs": 300.0 });
        assert!(r.dispatch("vignette", &args, &queue).unwrap());
        assert_eq!(
            queue.take(),
            vec![SystemReactionCommand::Vignette {
                color: Some([0.1, 0.0, 0.2]),
                strength: 0.8,
                duration_ms: 300.0,
            }]
        );
    }

    #[test]
    fn vignette_dispatch_defaults_absent_color_to_none() {
        // The command carries `None`; the drain applies the black default when it
        // maps onto `VignetteDecay::start`.
        let mut r = SystemReactionRegistry::new();
        register_system_reaction_primitives(&mut r);
        let queue = SystemCommandQueue::new();

        let args = serde_json::json!({ "strength": 0.5, "durationMs": 200.0 });
        assert!(r.dispatch("vignette", &args, &queue).unwrap());
        assert_eq!(
            queue.take(),
            vec![SystemReactionCommand::Vignette {
                color: None,
                strength: 0.5,
                duration_ms: 200.0,
            }]
        );
    }

    #[test]
    fn screen_shake_dispatch_enqueues_command_with_frequency() {
        let mut r = SystemReactionRegistry::new();
        register_system_reaction_primitives(&mut r);
        let queue = SystemCommandQueue::new();

        let args = serde_json::json!({ "amplitude": 12.0, "durationMs": 250.0, "frequency": 24.0 });
        assert!(r.dispatch("screenShake", &args, &queue).unwrap());
        assert_eq!(
            queue.take(),
            vec![SystemReactionCommand::ScreenShake {
                amplitude: 12.0,
                duration_ms: 250.0,
                frequency: Some(24.0),
            }]
        );
    }

    #[test]
    fn screen_shake_dispatch_passes_absent_frequency_through_as_none() {
        // The omitted-frequency default is the DRIVER's job (18 Hz); the
        // deserializer carries `None` through unchanged.
        let mut r = SystemReactionRegistry::new();
        register_system_reaction_primitives(&mut r);
        let queue = SystemCommandQueue::new();

        let args = serde_json::json!({ "amplitude": 8.0, "durationMs": 150.0 });
        assert!(r.dispatch("screenShake", &args, &queue).unwrap());
        assert_eq!(
            queue.take(),
            vec![SystemReactionCommand::ScreenShake {
                amplitude: 8.0,
                duration_ms: 150.0,
                frequency: None,
            }]
        );
    }

    #[test]
    fn show_dialog_dispatch_enqueues_push_tree_with_on_commit() {
        let mut r = SystemReactionRegistry::new();
        register_system_reaction_primitives(&mut r);
        let queue = SystemCommandQueue::new();

        let args = serde_json::json!({ "tree": "pauseMenu", "onCommit": "resumeGame" });
        assert!(r.dispatch("showDialog", &args, &queue).unwrap());
        assert_eq!(
            queue.take(),
            vec![SystemReactionCommand::PushTree {
                tree: "pauseMenu".to_string(),
                on_commit: Some("resumeGame".to_string()),
            }]
        );
    }

    #[test]
    fn show_dialog_defaults_absent_on_commit_to_none() {
        let mut r = SystemReactionRegistry::new();
        register_system_reaction_primitives(&mut r);
        let queue = SystemCommandQueue::new();

        let args = serde_json::json!({ "tree": "hint" });
        assert!(r.dispatch("showDialog", &args, &queue).unwrap());
        assert_eq!(
            queue.take(),
            vec![SystemReactionCommand::PushTree {
                tree: "hint".to_string(),
                on_commit: None,
            }]
        );
    }

    #[test]
    fn open_menu_dispatch_enqueues_push_tree_without_on_commit() {
        // `openMenu` is a v1 alias of `showDialog` that never carries onCommit —
        // a menu has no commit payload, so the alias drops any onCommit key.
        let mut r = SystemReactionRegistry::new();
        register_system_reaction_primitives(&mut r);
        let queue = SystemCommandQueue::new();

        let args = serde_json::json!({ "tree": "mainMenu" });
        assert!(r.dispatch("openMenu", &args, &queue).unwrap());
        assert_eq!(
            queue.take(),
            vec![SystemReactionCommand::PushTree {
                tree: "mainMenu".to_string(),
                on_commit: None,
            }]
        );
    }

    #[test]
    fn close_dialog_dispatch_enqueues_pop_tree() {
        let mut r = SystemReactionRegistry::new();
        register_system_reaction_primitives(&mut r);
        let queue = SystemCommandQueue::new();

        assert!(
            r.dispatch("closeDialog", &serde_json::json!({}), &queue)
                .unwrap()
        );
        assert_eq!(queue.take(), vec![SystemReactionCommand::PopTree]);
    }

    #[test]
    fn load_level_dispatch_reads_map_key() {
        let mut r = SystemReactionRegistry::new();
        register_system_reaction_primitives(&mut r);
        let queue = SystemCommandQueue::new();

        let args = serde_json::json!({ "map": "e1m1" });
        assert!(r.dispatch("loadLevel", &args, &queue).unwrap());
        assert_eq!(
            queue.take(),
            vec![SystemReactionCommand::LoadLevel {
                map: "e1m1".to_string(),
            }]
        );
    }

    #[test]
    fn restart_level_dispatch_enqueues_argumentless_command() {
        let mut r = SystemReactionRegistry::new();
        register_system_reaction_primitives(&mut r);
        let queue = SystemCommandQueue::new();

        assert!(
            r.dispatch("restartLevel", &serde_json::json!({}), &queue)
                .unwrap()
        );
        assert_eq!(queue.take(), vec![SystemReactionCommand::RestartLevel]);
    }

    #[test]
    fn return_to_frontend_dispatch_enqueues_argumentless_command() {
        let mut r = SystemReactionRegistry::new();
        register_system_reaction_primitives(&mut r);
        let queue = SystemCommandQueue::new();

        assert!(
            r.dispatch("returnToFrontend", &serde_json::json!({}), &queue)
                .unwrap()
        );
        assert_eq!(queue.take(), vec![SystemReactionCommand::ReturnToFrontend]);
    }

    #[test]
    fn set_state_dispatch_enqueues_command_with_slot_and_value() {
        let mut r = SystemReactionRegistry::new();
        register_system_reaction_primitives(&mut r);
        let queue = SystemCommandQueue::new();

        let args = serde_json::json!({ "slot": "audio.master", "value": 0.5 });
        assert!(r.dispatch("setState", &args, &queue).unwrap());
        assert_eq!(
            queue.take(),
            vec![SystemReactionCommand::SetState {
                slot: "audio.master".to_string(),
                value: serde_json::json!(0.5),
            }]
        );
    }

    #[test]
    fn set_state_carries_arbitrary_json_value_shapes() {
        // The command is type-agnostic at the queue layer; the drain's write path
        // coerces to the slot's declared type. A string, bool, and array all ride.
        let mut r = SystemReactionRegistry::new();
        register_system_reaction_primitives(&mut r);
        let queue = SystemCommandQueue::new();
        r.dispatch(
            "setState",
            &serde_json::json!({ "slot": "ui.label", "value": "hi" }),
            &queue,
        )
        .unwrap();
        assert_eq!(
            queue.take(),
            vec![SystemReactionCommand::SetState {
                slot: "ui.label".to_string(),
                value: serde_json::json!("hi"),
            }]
        );
    }

    #[test]
    fn cell_write_dispatch_enqueues_command_with_scope_cell_and_value() {
        // The `ui.createLocalState().set(v)` path: distinct from `setState`, it
        // carries a scope id + cell name (NOT a slot) and rides into the
        // presentation-cell store, never the slot table.
        let mut r = SystemReactionRegistry::new();
        register_system_reaction_primitives(&mut r);
        let queue = SystemCommandQueue::new();

        let args = serde_json::json!({ "scope": "counter", "cell": "count", "value": 5 });
        assert!(r.dispatch("cellWrite", &args, &queue).unwrap());
        assert_eq!(
            queue.take(),
            vec![SystemReactionCommand::CellWrite {
                scope: "counter".to_string(),
                cell: "count".to_string(),
                value: serde_json::json!(5),
            }]
        );
    }

    #[test]
    fn append_text_dispatch_enqueues_command_with_slot_and_text() {
        let mut r = SystemReactionRegistry::new();
        register_system_reaction_primitives(&mut r);
        let queue = SystemCommandQueue::new();

        let args = serde_json::json!({ "slot": "ui.textEntry", "text": "ab" });
        assert!(r.dispatch("appendText", &args, &queue).unwrap());
        assert_eq!(
            queue.take(),
            vec![SystemReactionCommand::AppendText {
                slot: "ui.textEntry".to_string(),
                text: "ab".to_string(),
            }]
        );
    }

    #[test]
    fn backspace_text_dispatch_enqueues_command_with_slot() {
        let mut r = SystemReactionRegistry::new();
        register_system_reaction_primitives(&mut r);
        let queue = SystemCommandQueue::new();

        let args = serde_json::json!({ "slot": "ui.textEntry" });
        assert!(r.dispatch("backspaceText", &args, &queue).unwrap());
        assert_eq!(
            queue.take(),
            vec![SystemReactionCommand::BackspaceText {
                slot: "ui.textEntry".to_string(),
            }]
        );
    }

    #[test]
    fn clear_text_dispatch_enqueues_command_with_slot() {
        let mut r = SystemReactionRegistry::new();
        register_system_reaction_primitives(&mut r);
        let queue = SystemCommandQueue::new();

        let args = serde_json::json!({ "slot": "ui.textEntry" });
        assert!(r.dispatch("clearText", &args, &queue).unwrap());
        assert_eq!(
            queue.take(),
            vec![SystemReactionCommand::ClearText {
                slot: "ui.textEntry".to_string(),
            }]
        );
    }

    #[test]
    fn dispatch_unknown_name_returns_false() {
        let r = SystemReactionRegistry::new();
        let queue = SystemCommandQueue::new();
        assert!(
            !r.dispatch("noSuchReaction", &serde_json::Value::Null, &queue)
                .unwrap()
        );
        assert!(queue.is_empty());
    }

    #[test]
    fn invalid_args_surface_as_invalid_argument() {
        let mut r = SystemReactionRegistry::new();
        register_system_reaction_primitives(&mut r);
        let queue = SystemCommandQueue::new();

        // `playSound` requires `sound`; an empty object fails deserialization.
        let err = r
            .dispatch("playSound", &serde_json::json!({}), &queue)
            .unwrap_err();
        assert!(matches!(err, ReactionError::InvalidArgument { .. }));
        assert!(queue.is_empty());
    }

    #[test]
    fn take_leaves_queue_empty() {
        let queue = SystemCommandQueue::new();
        queue.push(SystemReactionCommand::PopTree);
        assert!(!queue.is_empty());
        let _ = queue.take();
        assert!(queue.is_empty());
    }

    // --- vignette durationMs validation -------------------------------------
    //
    // The nan/inf tests below assert that the bad arg IS rejected — that is the
    // guarantee under test. The rejection layer differs by arg source: the JSON
    // bridge (serde_json::Number::from_f64) maps non-finite f32 to null, so
    // NaN/Infinity arrive as null and are refused at deserialize ("invalid type:
    // null, expected f32") before the is_finite() guard runs. The is_finite()
    // arm is defense-in-depth for any future non-JSON caller. The tests remain
    // valuable as rejection guarantees regardless of which layer fires.

    #[test]
    fn vignette_nan_duration_ms_rejected_with_invalid_argument() {
        let mut r = SystemReactionRegistry::new();
        register_system_reaction_primitives(&mut r);
        let queue = SystemCommandQueue::new();

        let args = serde_json::json!({ "strength": 0.8, "durationMs": f32::NAN });
        let err = r.dispatch("vignette", &args, &queue).unwrap_err();
        assert!(matches!(err, ReactionError::InvalidArgument { .. }));
        assert!(queue.is_empty());
    }

    #[test]
    fn vignette_inf_duration_ms_rejected_with_invalid_argument() {
        let mut r = SystemReactionRegistry::new();
        register_system_reaction_primitives(&mut r);
        let queue = SystemCommandQueue::new();

        let args = serde_json::json!({ "strength": 0.5, "durationMs": f32::INFINITY });
        let err = r.dispatch("vignette", &args, &queue).unwrap_err();
        assert!(matches!(err, ReactionError::InvalidArgument { .. }));
        assert!(queue.is_empty());
    }

    #[test]
    fn vignette_zero_duration_ms_rejected_with_invalid_argument() {
        let mut r = SystemReactionRegistry::new();
        register_system_reaction_primitives(&mut r);
        let queue = SystemCommandQueue::new();

        let args = serde_json::json!({ "strength": 0.5, "durationMs": 0.0 });
        let err = r.dispatch("vignette", &args, &queue).unwrap_err();
        assert!(matches!(err, ReactionError::InvalidArgument { .. }));
        assert!(queue.is_empty());
    }

    #[test]
    fn vignette_negative_duration_ms_rejected_with_invalid_argument() {
        let mut r = SystemReactionRegistry::new();
        register_system_reaction_primitives(&mut r);
        let queue = SystemCommandQueue::new();

        let args = serde_json::json!({ "strength": 0.5, "durationMs": -100.0 });
        let err = r.dispatch("vignette", &args, &queue).unwrap_err();
        assert!(matches!(err, ReactionError::InvalidArgument { .. }));
        assert!(queue.is_empty());
    }

    #[test]
    fn vignette_valid_args_enqueues_command() {
        let mut r = SystemReactionRegistry::new();
        register_system_reaction_primitives(&mut r);
        let queue = SystemCommandQueue::new();

        let args = serde_json::json!({ "strength": 0.7, "durationMs": 400.0 });
        assert!(r.dispatch("vignette", &args, &queue).unwrap());
        assert_eq!(
            queue.take(),
            vec![SystemReactionCommand::Vignette {
                color: None,
                strength: 0.7,
                duration_ms: 400.0,
            }]
        );
    }

    // --- screenShake durationMs, frequency, and amplitude validation ---------
    //
    // The nan/inf tests below assert that the bad arg IS rejected — that is the
    // guarantee under test. The rejection layer differs by arg source: the JSON
    // bridge (serde_json::Number::from_f64) maps non-finite f32 to null, so
    // NaN/Infinity arrive as null and are refused at deserialize ("invalid type:
    // null, expected f32") before the is_finite() guard runs. The is_finite()
    // arm is defense-in-depth for any future non-JSON caller. The tests remain
    // valuable as rejection guarantees regardless of which layer fires.

    #[test]
    fn screen_shake_nan_duration_ms_rejected_with_invalid_argument() {
        let mut r = SystemReactionRegistry::new();
        register_system_reaction_primitives(&mut r);
        let queue = SystemCommandQueue::new();

        let args = serde_json::json!({ "amplitude": 10.0, "durationMs": f32::NAN });
        let err = r.dispatch("screenShake", &args, &queue).unwrap_err();
        assert!(matches!(err, ReactionError::InvalidArgument { .. }));
        assert!(queue.is_empty());
    }

    #[test]
    fn screen_shake_inf_duration_ms_rejected_with_invalid_argument() {
        let mut r = SystemReactionRegistry::new();
        register_system_reaction_primitives(&mut r);
        let queue = SystemCommandQueue::new();

        let args = serde_json::json!({ "amplitude": 10.0, "durationMs": f32::INFINITY });
        let err = r.dispatch("screenShake", &args, &queue).unwrap_err();
        assert!(matches!(err, ReactionError::InvalidArgument { .. }));
        assert!(queue.is_empty());
    }

    #[test]
    fn screen_shake_zero_duration_ms_rejected_with_invalid_argument() {
        let mut r = SystemReactionRegistry::new();
        register_system_reaction_primitives(&mut r);
        let queue = SystemCommandQueue::new();

        let args = serde_json::json!({ "amplitude": 10.0, "durationMs": 0.0 });
        let err = r.dispatch("screenShake", &args, &queue).unwrap_err();
        assert!(matches!(err, ReactionError::InvalidArgument { .. }));
        assert!(queue.is_empty());
    }

    #[test]
    fn screen_shake_negative_duration_ms_rejected_with_invalid_argument() {
        let mut r = SystemReactionRegistry::new();
        register_system_reaction_primitives(&mut r);
        let queue = SystemCommandQueue::new();

        let args = serde_json::json!({ "amplitude": 10.0, "durationMs": -50.0 });
        let err = r.dispatch("screenShake", &args, &queue).unwrap_err();
        assert!(matches!(err, ReactionError::InvalidArgument { .. }));
        assert!(queue.is_empty());
    }

    #[test]
    fn screen_shake_negative_frequency_rejected_with_invalid_argument() {
        // JSON cannot represent f32::INFINITY (serde_json::json! maps it to null,
        // which deserializes to None — the absent-frequency path — rather than a
        // non-finite value). A negative frequency is the JSON-reachable equivalent:
        // it hits the `freq <= 0.0` guard and is a realistic authoring error.
        let mut r = SystemReactionRegistry::new();
        register_system_reaction_primitives(&mut r);
        let queue = SystemCommandQueue::new();

        let args = serde_json::json!({ "amplitude": 10.0, "durationMs": 200.0, "frequency": -1.0 });
        let err = r.dispatch("screenShake", &args, &queue).unwrap_err();
        assert!(matches!(err, ReactionError::InvalidArgument { .. }));
        assert!(queue.is_empty());
    }

    #[test]
    fn screen_shake_zero_frequency_rejected_with_invalid_argument() {
        let mut r = SystemReactionRegistry::new();
        register_system_reaction_primitives(&mut r);
        let queue = SystemCommandQueue::new();

        let args = serde_json::json!({ "amplitude": 10.0, "durationMs": 200.0, "frequency": 0.0 });
        let err = r.dispatch("screenShake", &args, &queue).unwrap_err();
        assert!(matches!(err, ReactionError::InvalidArgument { .. }));
        assert!(queue.is_empty());
    }

    #[test]
    fn screen_shake_valid_args_enqueues_command() {
        let mut r = SystemReactionRegistry::new();
        register_system_reaction_primitives(&mut r);
        let queue = SystemCommandQueue::new();

        let args = serde_json::json!({ "amplitude": 6.0, "durationMs": 300.0, "frequency": 20.0 });
        assert!(r.dispatch("screenShake", &args, &queue).unwrap());
        assert_eq!(
            queue.take(),
            vec![SystemReactionCommand::ScreenShake {
                amplitude: 6.0,
                duration_ms: 300.0,
                frequency: Some(20.0),
            }]
        );
    }

    #[test]
    fn screen_shake_negative_amplitude_rejected_with_invalid_argument() {
        let mut r = SystemReactionRegistry::new();
        register_system_reaction_primitives(&mut r);
        let queue = SystemCommandQueue::new();

        let args = serde_json::json!({ "amplitude": -1.0, "durationMs": 200.0 });
        let err = r.dispatch("screenShake", &args, &queue).unwrap_err();
        assert!(matches!(err, ReactionError::InvalidArgument { .. }));
        assert!(queue.is_empty());
    }

    #[test]
    fn screen_shake_amplitude_above_max_rejected_with_invalid_argument() {
        // Amplitude above MAX_SHAKE_AMPLITUDE_PX (1280.0 px) produces a UV
        // offset > 1.0 that causes whole-frame ClampToEdge edge-smear.
        let mut r = SystemReactionRegistry::new();
        register_system_reaction_primitives(&mut r);
        let queue = SystemCommandQueue::new();

        let args = serde_json::json!({ "amplitude": 1281.0, "durationMs": 200.0 });
        let err = r.dispatch("screenShake", &args, &queue).unwrap_err();
        assert!(matches!(err, ReactionError::InvalidArgument { .. }));
        assert!(queue.is_empty());
    }

    #[test]
    fn screen_shake_zero_amplitude_accepted_enqueues_command() {
        // amplitude == 0.0 is a valid no-op shake (zero displacement).
        let mut r = SystemReactionRegistry::new();
        register_system_reaction_primitives(&mut r);
        let queue = SystemCommandQueue::new();

        let args = serde_json::json!({ "amplitude": 0.0, "durationMs": 200.0 });
        assert!(r.dispatch("screenShake", &args, &queue).unwrap());
        assert_eq!(
            queue.take(),
            vec![SystemReactionCommand::ScreenShake {
                amplitude: 0.0,
                duration_ms: 200.0,
                frequency: None,
            }]
        );
    }
}
