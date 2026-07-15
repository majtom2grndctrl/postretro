// System-reaction command queue: deferred typed commands drained by the app.
// See: context/lib/scripting.md §10.4

use std::cell::RefCell;
use std::rc::Rc;

use postretro_foundation::ir::IrValue;

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
        /// Stable identity of the firing source for once-per-source diagnostics.
        /// It is runtime queue context only, never persistent or on the wire.
        dispatch_source: String,
        /// Ephemeral values published by the firing source. They are not part
        /// of command selection or any persistent/wire format.
        dispatch_values: Vec<(String, IrValue)>,
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
    fire_context: Rc<RefCell<SystemCommandFireContext>>,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct SystemCommandFireContext {
    pub source: String,
    pub values: Vec<(String, IrValue)>,
}

impl SystemCommandQueue {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn push(&self, command: SystemReactionCommand) {
        self.commands.borrow_mut().push(command);
    }

    /// Replace the active named-fire context, returning the prior context so a
    /// caller can restore it after dispatch. Registry handler signatures stay
    /// source-agnostic; only commands that need context snapshot it.
    pub fn replace_fire_context(
        &self,
        context: SystemCommandFireContext,
    ) -> SystemCommandFireContext {
        std::mem::replace(&mut *self.fire_context.borrow_mut(), context)
    }

    pub fn fire_context(&self) -> SystemCommandFireContext {
        self.fire_context.borrow().clone()
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
