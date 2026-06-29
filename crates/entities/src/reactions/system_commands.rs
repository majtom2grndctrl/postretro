// System-reaction command queue: deferred typed commands drained by the app.
// See: context/lib/scripting.md §10.4

use std::cell::RefCell;
use std::rc::Rc;

/// A single deferred system-reaction effect. Variants carry their full args so
/// the drain seam is typed end to end.
#[derive(Debug, Clone, PartialEq)]
pub enum SystemReactionCommand {
    PlaySound {
        sound: String,
        bus: Option<String>,
    },
    Rumble {
        strong: f32,
        weak: Option<f32>,
        duration_ms: f32,
    },
    FlashScreen {
        color: [f32; 4],
        duration_ms: f32,
    },
    Vignette {
        color: Option<[f32; 3]>,
        strength: f32,
        duration_ms: f32,
    },
    ScreenShake {
        amplitude: f32,
        duration_ms: f32,
        frequency: Option<f32>,
    },
    PushTree {
        tree: String,
        on_commit: Option<String>,
    },
    LoadLevel {
        map: String,
    },
    RestartLevel,
    ReturnToFrontend,
    PopTree,
    SetState {
        slot: String,
        value: serde_json::Value,
    },
    CellWrite {
        scope: String,
        cell: String,
        value: serde_json::Value,
    },
    AppendText {
        slot: String,
        text: String,
    },
    BackspaceText {
        slot: String,
    },
    ClearText {
        slot: String,
    },
}

/// Shared handle to the per-frame system-command queue.
#[derive(Clone, Default)]
pub struct SystemCommandQueue {
    commands: Rc<RefCell<Vec<SystemReactionCommand>>>,
}

impl SystemCommandQueue {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn push(&self, command: SystemReactionCommand) {
        self.commands.borrow_mut().push(command);
    }

    pub fn take(&self) -> Vec<SystemReactionCommand> {
        std::mem::take(&mut self.commands.borrow_mut())
    }

    pub fn is_empty(&self) -> bool {
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
