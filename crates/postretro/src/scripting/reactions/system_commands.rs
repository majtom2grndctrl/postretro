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
/// the drain seam is typed end to end; the actual subsystem consumers
/// (audio/gilrs/UI stack) land in later tasks. Until a consumer is wired, the
/// drain logs the command (see [`SystemCommandQueue::drain_to_log`]).
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

    /// Drain to a log sink. Stand-in for the not-yet-wired subsystem consumers
    /// (audio/gilrs/UI stack land in Task 4 / Goal F); keeps the typed queue
    /// and drain hook exercised end to end until those land.
    pub(crate) fn drain_to_log(&self) {
        for command in self.take() {
            log::debug!("[Scripting] system reaction command (no consumer yet): {command:?}");
        }
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

/// Register the UI-stack system reactions (`pushTree` / `popTree`). The
/// audio/input/UI helper primitives (`playSound`, `rumble`, `flashScreen`)
/// land in Task 4 with their consumers; this seam already enqueues their
/// typed commands so those tasks wire the sink, not the queue.
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
    registry.register("pushTree", |args, queue| {
        let parsed: PushTreeArgs =
            serde_json::from_value(args.clone()).map_err(|e| ReactionError::InvalidArgument {
                reason: format!("pushTree: failed to deserialize args: {e}"),
            })?;
        queue.push(SystemReactionCommand::PushTree {
            tree: parsed.tree,
            on_commit: parsed.on_commit,
        });
        Ok(())
    });
    registry.register("popTree", |_args, queue| {
        queue.push(SystemReactionCommand::PopTree);
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
struct PushTreeArgs {
    tree: String,
    #[serde(default)]
    on_commit: Option<String>,
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
        assert!(r.contains("pushTree"));
        assert!(r.contains("popTree"));
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
    fn push_tree_dispatch_enqueues_command_with_on_commit() {
        let mut r = SystemReactionRegistry::new();
        register_system_reaction_primitives(&mut r);
        let queue = SystemCommandQueue::new();

        let args = serde_json::json!({ "tree": "pauseMenu", "onCommit": "resumeGame" });
        assert!(r.dispatch("pushTree", &args, &queue).unwrap());
        assert_eq!(
            queue.take(),
            vec![SystemReactionCommand::PushTree {
                tree: "pauseMenu".to_string(),
                on_commit: Some("resumeGame".to_string()),
            }]
        );
    }

    #[test]
    fn pop_tree_dispatch_enqueues_command() {
        let mut r = SystemReactionRegistry::new();
        register_system_reaction_primitives(&mut r);
        let queue = SystemCommandQueue::new();

        assert!(
            r.dispatch("popTree", &serde_json::Value::Null, &queue)
                .unwrap()
        );
        assert_eq!(queue.take(), vec![SystemReactionCommand::PopTree]);
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
