// System-reaction command path: the second execution arm of the shared
// named-reaction vocabulary. Entity reactions mutate `EntityRegistry`; system
// reactions (no `tag`) push typed commands onto a per-frame queue the app
// drains after the post-tick event drains, so audio/input/UI subsystems
// consume their commands without threading engine services into scripting.
// See: context/lib/scripting.md §10.4

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

use super::ReactionError;

/// A single deferred system-reaction effect. Variants carry their full args so
/// the drain seam is typed end to end. The app's `dispatch_system_commands`
/// routes each to its subsystem consumer: audio `play` (kira), gilrs rumble,
/// `screen.flash` decay, modal-stack push/pop, slot writes, and text-edit
/// mutations.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum SystemReactionCommand {
    /// Play a one-shot sound on an optional named audio bus.
    PlaySound { sound: String, bus: Option<String> },
    /// Gamepad rumble: `strong`/`weak` motor magnitudes in `[0, 1]` for
    /// `duration_ms`. `weak` absent ⇒ consumer mirrors `strong`.
    Rumble {
        strong: f32,
        weak: Option<f32>,
        duration_ms: f32,
    },
    /// Full-screen color flash fading over `duration_ms`.
    FlashScreen { color: [f32; 4], duration_ms: f32 },
    /// Push a registered UI tree onto the stack, optionally firing the named
    /// reaction when the pushed tree commits.
    PushTree {
        tree: String,
        on_commit: Option<String>,
    },
    /// Pop the top UI tree off the stack.
    PopTree,
    /// Write a value to a writable store slot at the game-logic stage (M13 Goal F,
    /// Task 4). The drain applies it through the readonly-gated JSON write
    /// (`primitives::store::write_state_slot_json`): a readonly slot warns and
    /// no-ops, an engine-owned writable slot is a valid target. `value` carries the
    /// raw JSON value coerced to the slot's declared type by the write path.
    SetState {
        slot: String,
        value: serde_json::Value,
    },
    /// Append `text` to the current string value of a writable String slot at the
    /// game-logic stage (M13 Text Entry, Task 1). The drain applies it through the
    /// readonly-gated text-edit path (`primitives::store::apply_text_edit`):
    /// readonly warns and no-ops; an engine-owned writable slot (`ui.textEntry`)
    /// is a valid target.
    AppendText { slot: String, text: String },
    /// Remove the last grapheme cluster (char-pop floor) from a writable String
    /// slot at the game-logic stage (M13 Text Entry, Task 1). Empty is a no-op
    /// with no warning. Same readonly-gated path as `AppendText`.
    BackspaceText { slot: String },
    /// Empty a writable String slot at the game-logic stage (M13 Text Entry,
    /// Task 1). Same readonly-gated path as `AppendText`.
    ClearText { slot: String },
}

/// Shared handle to the per-frame system-command queue. Cloned into the
/// scripting context and into system-reaction handlers; the app drains it once
/// per frame. `Rc<RefCell<_>>` matches the single-threaded scripting model
/// (`ctx.rs`): the queue is never touched from a background thread.
#[derive(Clone, Default)]
pub(crate) struct SystemCommandQueue {
    commands: Rc<RefCell<Vec<SystemReactionCommand>>>,
}

impl SystemCommandQueue {
    pub(crate) fn new() -> Self {
        Self::default()
    }

    /// Enqueue a command for the next app drain.
    pub(crate) fn push(&self, command: SystemReactionCommand) {
        self.commands.borrow_mut().push(command);
    }

    /// Take every queued command, leaving the queue empty. The app calls this
    /// once per frame after the post-tick event drains and routes each command
    /// to its subsystem consumer.
    pub(crate) fn take(&self) -> Vec<SystemReactionCommand> {
        std::mem::take(&mut self.commands.borrow_mut())
    }

    pub(crate) fn is_empty(&self) -> bool {
        self.commands.borrow().is_empty()
    }
}

impl std::fmt::Debug for SystemCommandQueue {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SystemCommandQueue")
            .field("len", &self.commands.borrow().len())
            .finish()
    }
}

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

/// Register all ten system-reaction primitives onto `registry`:
/// - Audio: `playSound`
/// - Input/rumble: `rumble`
/// - Display/flash: `flashScreen`
/// - UI stack: `showDialog`, `openMenu`, `closeDialog` (push/pop `PushTree`/`PopTree`)
/// - Slot write: `setState`
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
struct SetStateArgs {
    slot: String,
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
        assert!(r.contains("showDialog"));
        assert!(r.contains("openMenu"));
        assert!(r.contains("closeDialog"));
        assert!(r.contains("setState"));
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
}
