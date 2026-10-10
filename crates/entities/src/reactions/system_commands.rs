// System-reaction command queue: deferred typed commands drained by the app.
// See: context/lib/scripting.md §10.4

use std::cell::RefCell;
use std::rc::Rc;

use postretro_foundation::{Seat, ir::IrValue};

use super::emitter::Emitter;

/// A single deferred system-reaction effect. Variants carry their full args so
/// the drain seam is typed end to end.
#[derive(Debug, Clone, PartialEq)]
pub enum SystemReactionCommand {
    PlaySound {
        sound: String,
        bus: Option<String>,
        /// Where the sound plays from, resolved from the firing source's
        /// emitter when the reaction authored `at: on.emitter`. `None` plays
        /// it unpositioned.
        at: Option<Emitter>,
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
    /// Set the live sentiment from one authored faction toward another. Names
    /// cross this queue boundary so the app drain can resolve them against the
    /// current immutable faction registry at the frame-end write point.
    SetSentiment {
        from: String,
        to: String,
        value: f32,
    },
    /// Add a delta to the live sentiment from one authored faction toward
    /// another. An absent overlay pair starts from its authored baseline.
    AdjustSentiment {
        from: String,
        to: String,
        delta: f32,
    },
    /// Add a delta to the authoritative value of one per-owner numeric slot
    /// for every seat resolved while the reaction fired. The app drain checks
    /// that each queued seat is still live before mutating the slot table.
    AddOwnerSlot {
        slot: String,
        seats: Vec<Seat>,
        delta: f32,
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
    /// Presentation a player event fired for one player, held for the app to
    /// present on that player's machine.
    routed: Rc<RefCell<Vec<(Seat, SystemReactionCommand)>>>,
    fire_context: Rc<RefCell<SystemCommandFireContext>>,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct SystemCommandFireContext {
    pub source: String,
    pub values: Vec<(String, IrValue)>,
    /// The emitter a named gameplay source published, if any. Sources without
    /// one (level load, crossings, triggers, deaths, follow-ups) leave it
    /// `None`, and a reaction reading `on.emitter` there is skipped.
    pub emitter: Option<Emitter>,
    /// The player whose machine presentation from this fire plays on. Only a
    /// player event sets it; every other source presents where it drains.
    pub presentation_seat: Option<Seat>,
}

impl SystemCommandQueue {
    pub fn new() -> Self {
        Self::default()
    }

    /// Enqueue a command. Presentation fired for one player is held apart
    /// for [`Self::take_routed`]; everything else drains locally.
    pub fn push(&self, command: SystemReactionCommand) {
        let seat = self.fire_context.borrow().presentation_seat;
        match seat {
            Some(seat) if command.class() == SystemReactionClass::Presentation => {
                self.routed.borrow_mut().push((seat, command));
            }
            _ => self.commands.borrow_mut().push(command),
        }
    }

    /// Presentation routed to a player, in fire order.
    pub fn take_routed(&self) -> Vec<(Seat, SystemReactionCommand)> {
        std::mem::take(&mut self.routed.borrow_mut())
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

    /// Drop every queued command, routed presentation included.
    pub fn discard_all(&self) {
        self.commands.borrow_mut().clear();
        self.routed.borrow_mut().clear();
    }

    pub fn is_empty(&self) -> bool {
        self.commands.borrow().is_empty() && self.routed.borrow().is_empty()
    }
}

impl std::fmt::Debug for SystemCommandQueue {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SystemCommandQueue")
            .field("len", &self.commands.borrow().len())
            .finish()
    }
}

/// How a system reaction relates to the machine that drains it. A player
/// event decides from this which reactions it may fire on the host for one
/// player (`scripting.md` §12, Player events).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SystemReactionClass {
    /// Presents to whoever plays on the draining machine. A player event
    /// forwards it to its player's machine.
    Presentation,
    /// Changes the draining machine's own UI or text state. Nothing forwards
    /// it, so on the host it would land on the host's screen.
    MachineLocal,
    /// A host decision whose effect reaches clients through replication or
    /// level control.
    HostConsequence,
    /// `setState`: a host consequence on a replicated slot, machine-local on
    /// any other.
    SlotWrite,
}

/// Every system reaction primitive, by its registered name.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SystemReactionKind {
    PlaySound,
    Rumble,
    FlashScreen,
    Vignette,
    ScreenShake,
    ShowDialog,
    OpenMenu,
    CloseDialog,
    LoadLevel,
    RestartLevel,
    ReturnToFrontend,
    SetState,
    SetSentiment,
    AdjustSentiment,
    CellWrite,
    AppendText,
    BackspaceText,
    ClearText,
}

impl SystemReactionKind {
    pub const ALL: &'static [SystemReactionKind] = &[
        Self::PlaySound,
        Self::Rumble,
        Self::FlashScreen,
        Self::Vignette,
        Self::ScreenShake,
        Self::ShowDialog,
        Self::OpenMenu,
        Self::CloseDialog,
        Self::LoadLevel,
        Self::RestartLevel,
        Self::ReturnToFrontend,
        Self::SetState,
        Self::SetSentiment,
        Self::AdjustSentiment,
        Self::CellWrite,
        Self::AppendText,
        Self::BackspaceText,
        Self::ClearText,
    ];

    pub const fn primitive_name(self) -> &'static str {
        match self {
            Self::PlaySound => "playSound",
            Self::Rumble => "rumble",
            Self::FlashScreen => "flashScreen",
            Self::Vignette => "vignette",
            Self::ScreenShake => "screenShake",
            Self::ShowDialog => "showDialog",
            Self::OpenMenu => "openMenu",
            Self::CloseDialog => "closeDialog",
            Self::LoadLevel => "loadLevel",
            Self::RestartLevel => "restartLevel",
            Self::ReturnToFrontend => "returnToFrontend",
            Self::SetState => "setState",
            Self::SetSentiment => "setSentiment",
            Self::AdjustSentiment => "adjustSentiment",
            Self::CellWrite => "cellWrite",
            Self::AppendText => "appendText",
            Self::BackspaceText => "backspaceText",
            Self::ClearText => "clearText",
        }
    }

    pub fn from_primitive_name(name: &str) -> Option<Self> {
        Self::ALL
            .iter()
            .copied()
            .find(|kind| kind.primitive_name() == name)
    }

    pub const fn class(self) -> SystemReactionClass {
        match self {
            Self::PlaySound
            | Self::Rumble
            | Self::FlashScreen
            | Self::Vignette
            | Self::ScreenShake => SystemReactionClass::Presentation,
            Self::ShowDialog
            | Self::OpenMenu
            | Self::CloseDialog
            | Self::CellWrite
            | Self::AppendText
            | Self::BackspaceText
            | Self::ClearText => SystemReactionClass::MachineLocal,
            Self::LoadLevel
            | Self::RestartLevel
            | Self::ReturnToFrontend
            | Self::SetSentiment
            | Self::AdjustSentiment => SystemReactionClass::HostConsequence,
            Self::SetState => SystemReactionClass::SlotWrite,
        }
    }
}

impl SystemReactionCommand {
    /// The class of the reaction that enqueued this command. Exhaustive, so a
    /// new command fails to build until it is classified.
    pub const fn class(&self) -> SystemReactionClass {
        match self {
            Self::PlaySound { .. }
            | Self::Rumble { .. }
            | Self::FlashScreen { .. }
            | Self::Vignette { .. }
            | Self::ScreenShake { .. } => SystemReactionClass::Presentation,
            Self::PushTree { .. }
            | Self::PopTree
            | Self::CellWrite { .. }
            | Self::AppendText { .. }
            | Self::BackspaceText { .. }
            | Self::ClearText { .. } => SystemReactionClass::MachineLocal,
            Self::LoadLevel { .. }
            | Self::RestartLevel
            | Self::ReturnToFrontend
            | Self::SetSentiment { .. }
            | Self::AdjustSentiment { .. }
            | Self::AddOwnerSlot { .. } => SystemReactionClass::HostConsequence,
            Self::SetState { .. } => SystemReactionClass::SlotWrite,
        }
    }
}
