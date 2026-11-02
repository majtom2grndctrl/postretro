// System-reaction commands: app-drained queue, reaction classes, player-routed presentation.
// See: context/lib/scripting.md §10.4, §12 (Player events)

use crate::EntityId;
use serde::{Deserialize, Serialize};
use std::cell::RefCell;
use std::collections::HashSet;
use std::rc::Rc;

use postretro_foundation::{Seat, ir::IrValue};

use super::emitter::Emitter;

/// A single deferred system-reaction effect. Variants carry their full args so
/// the drain seam is typed end to end.
#[derive(Debug, Clone, PartialEq)]
pub enum SystemReactionCommand {
    PlaySound {
        audience: Option<Audience>,
        sound: String,
        bus: Option<String>,
        /// Where the sound plays from, resolved from the firing source's
        /// emitter when the reaction authored `at: on.emitter`. `None` plays
        /// it unpositioned.
        at: Option<Emitter>,
    },
    Rumble {
        audience: Option<Audience>,
        strong: f32,
        weak: Option<f32>,
        duration_ms: f32,
    },
    FlashScreen {
        audience: Option<Audience>,
        color: [f32; 4],
        duration_ms: f32,
    },
    Vignette {
        audience: Option<Audience>,
        color: Option<[f32; 3]>,
        strength: f32,
        duration_ms: f32,
    },
    ScreenShake {
        audience: Option<Audience>,
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
        /// Stable identity of the firing source for once-per-source diagnostics.
        /// It is runtime queue context only, never persistent or on the wire.
        dispatch_source: String,
    },
    RestartLevel {
        /// Stable identity of the firing source for once-per-source diagnostics.
        /// It is runtime queue context only, never persistent or on the wire.
        dispatch_source: String,
    },
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
    /// `setGhostAudio` from `postretro/audio`: a validated set of Ghost bus
    /// field targets. The host applies it at its next fixed tick, in call
    /// order; a connected client drops it (the bus state is the host's).
    SetGhostAudio(postretro_foundation::GhostAudioTargets),
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

/// Audience tokens retain their existing descriptor spellings.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Audience {
    #[serde(rename = "@activators")]
    Activators,
    #[serde(rename = "@trigger")]
    Trigger,
    #[serde(rename = "@player")]
    Player,
    #[serde(rename = "everyone")]
    Everyone,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FireOrigin {
    EveryMachine(MachineSource),
    HostOnly(FireSubject),
}

/// Why a source fires on every machine, which decides where and on which
/// clock a `wait` tail it parks lands (`scripting.md` §12, Steps after a
/// `wait`). Presentation routes the same for all three.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum MachineSource {
    /// `levelLoad`: every machine fires it at its own install, for a level
    /// the host started, so its tails count on the host timeline.
    HostTimeline,
    /// A host decision each machine observes and fires for itself, on its own
    /// clock: a crossing over shared slots, a weapon event or observer cue.
    HostMirrored,
    /// Each machine fires for its own player: a crossing over own, owner-private
    /// or local slots, a movement event, a UI press.
    OwnState,
}

/// A fire with no stated source is a host decision observed on every machine.
impl Default for FireOrigin {
    fn default() -> Self {
        Self::EveryMachine(MachineSource::HostMirrored)
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub enum FireSubject {
    #[default]
    None,
    Trigger {
        player: Option<Seat>,
        trigger: EntityId,
        occupants: Vec<Seat>,
    },
    PlayerEvent {
        player: Option<Seat>,
    },
    Teleport {
        player: Option<Seat>,
        teleporter: EntityId,
    },
    /// A `wait` landing whose subject left during the wait: what the same step
    /// routed to them before the wait presents on no machine. A sound the
    /// source plays everywhere by default (a trigger's, a teleport's) still
    /// does.
    Departed {
        sounds_everywhere: bool,
    },
}

impl FireOrigin {
    /// Named follow-ups preserve where they run but carry no scoped subject.
    pub fn follow_up(&self) -> Self {
        match self {
            Self::EveryMachine(source) => Self::EveryMachine(*source),
            Self::HostOnly(_) => Self::HostOnly(FireSubject::None),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Recipients {
    Local,
    Seats(Vec<Seat>),
    Everyone,
    Skip,
}

/// One ordered queue includes both local work and routed presentation.
#[derive(Clone, Default)]
pub struct SystemCommandQueue {
    commands: Rc<RefCell<Vec<(Recipients, SystemReactionCommand)>>>,
    fire_context: Rc<RefCell<SystemCommandFireContext>>,
    warned_audiences: Rc<RefCell<HashSet<(String, &'static str)>>>,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct SystemCommandFireContext {
    pub source: String,
    pub values: Vec<(String, IrValue)>,
    pub emitter: Option<Emitter>,
    pub origin: FireOrigin,
}

impl SystemCommandQueue {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn push(&self, command: SystemReactionCommand) {
        let context = self.fire_context.borrow();
        let recipients = if command.class() == SystemReactionClass::Presentation {
            resolve_recipients(
                &context.origin,
                command.audience(),
                matches!(command, SystemReactionCommand::PlaySound { .. }),
            )
        } else {
            Recipients::Local
        };
        if recipients == Recipients::Skip {
            let key = (
                context.source.clone(),
                command.presentation_primitive().unwrap(),
            );
            if self.warned_audiences.borrow_mut().insert(key) {
                log::warn!(
                    "[Scripting] source '{}' does not publish audience for {}; skipping",
                    context.source,
                    command.presentation_primitive().unwrap()
                );
            }
            return;
        }
        self.commands.borrow_mut().push((recipients, command));
    }

    /// Drain the whole queue for ordered delivery at the app boundary.
    pub fn take_ordered(&self) -> Vec<(Recipients, SystemReactionCommand)> {
        std::mem::take(&mut self.commands.borrow_mut())
    }

    /// Routed presentation, retaining relative fire order.
    pub fn take_routed(&self) -> Vec<(Recipients, SystemReactionCommand)> {
        let (local, routed): (Vec<_>, Vec<_>) = self
            .take_ordered()
            .into_iter()
            .partition(|(recipients, _)| *recipients == Recipients::Local);
        *self.commands.borrow_mut() = local;
        routed
    }

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
        let (local, routed): (Vec<_>, Vec<_>) = self
            .take_ordered()
            .into_iter()
            .partition(|(recipients, _)| *recipients == Recipients::Local);
        *self.commands.borrow_mut() = routed;
        local.into_iter().map(|(_, command)| command).collect()
    }
    pub fn discard_all(&self) {
        self.commands.borrow_mut().clear();
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

fn resolve_recipients(origin: &FireOrigin, audience: Option<Audience>, sound: bool) -> Recipients {
    use Audience::*;
    use FireOrigin::*;
    match (origin, audience) {
        (EveryMachine(_), Option::None | Some(Everyone)) => Recipients::Local,
        (EveryMachine(_), Some(Activators | Audience::Trigger | Player)) => Recipients::Skip,
        (HostOnly(_), Some(Everyone)) => Recipients::Everyone,
        (
            HostOnly(FireSubject::Trigger { player, .. } | FireSubject::Teleport { player, .. }),
            Some(Activators),
        ) => Recipients::Seats(player.iter().copied().collect()),
        (HostOnly(FireSubject::Trigger { occupants, .. }), Some(Audience::Trigger)) => {
            Recipients::Seats(occupants.clone())
        }
        (
            HostOnly(
                FireSubject::Trigger { .. } | FireSubject::Teleport { .. } | FireSubject::None,
            ),
            Option::None,
        ) if sound => Recipients::Everyone,
        (HostOnly(FireSubject::Teleport { player, .. }), Option::None) => {
            player.map_or(Recipients::Local, |seat| Recipients::Seats(vec![seat]))
        }
        (HostOnly(FireSubject::Trigger { player, .. }), Option::None) => {
            Recipients::Seats(player.iter().copied().collect())
        }
        (HostOnly(FireSubject::PlayerEvent { player }), Option::None | Some(Player)) => {
            player.map_or(Recipients::Local, |seat| Recipients::Seats(vec![seat]))
        }
        (HostOnly(FireSubject::Departed { sounds_everywhere }), Option::None) => {
            if sound && *sounds_everywhere {
                Recipients::Everyone
            } else {
                Recipients::Seats(Vec::new())
            }
        }
        (HostOnly(FireSubject::None), Option::None) => Recipients::Local,
        (HostOnly(_), Some(Activators | Audience::Trigger | Player)) => Recipients::Skip,
    }
}

/// How a system reaction relates to the machine that drains it. A player
/// event decides from this which reactions it may fire on the host for one
/// player (`scripting.md` §12, Player events). `system_reaction_is_machine_local`
/// (`reactions/machine_local.rs`) also reads it to decide what a client lands
/// after a `wait`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SystemReactionClass {
    /// A one-shot effect whose audience is resolved from its fire origin and
    /// optional audience.
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
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
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
    SetGhostAudio,
    CellWrite,
    AppendText,
    BackspaceText,
    ClearText,
}

impl SystemReactionKind {
    /// Every kind. No compiler exhaustiveness check covers this list; the sim
    /// test `registers_exactly_the_classified_system_reaction_kinds` keeps it
    /// in step with the registered system primitives.
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
        Self::SetGhostAudio,
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
            Self::SetGhostAudio => postretro_foundation::SET_GHOST_AUDIO,
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
            | Self::AdjustSentiment
            | Self::SetGhostAudio => SystemReactionClass::HostConsequence,
            Self::SetState => SystemReactionClass::SlotWrite,
        }
    }
}

impl SystemReactionCommand {
    pub fn presentation_primitive(&self) -> Option<&'static str> {
        match self {
            Self::PlaySound { .. } => Some("playSound"),
            Self::Rumble { .. } => Some("rumble"),
            Self::FlashScreen { .. } => Some("flashScreen"),
            Self::Vignette { .. } => Some("vignette"),
            Self::ScreenShake { .. } => Some("screenShake"),
            _ => None,
        }
    }
    pub fn audience(&self) -> Option<Audience> {
        match self {
            Self::PlaySound { audience, .. }
            | Self::Rumble { audience, .. }
            | Self::FlashScreen { audience, .. }
            | Self::Vignette { audience, .. }
            | Self::ScreenShake { audience, .. } => *audience,
            _ => None,
        }
    }
    pub fn clear_audience(&mut self) {
        match self {
            Self::PlaySound { audience, .. }
            | Self::Rumble { audience, .. }
            | Self::FlashScreen { audience, .. }
            | Self::Vignette { audience, .. }
            | Self::ScreenShake { audience, .. } => *audience = None,
            _ => {}
        }
    }

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
            | Self::RestartLevel { .. }
            | Self::ReturnToFrontend
            | Self::SetSentiment { .. }
            | Self::AdjustSentiment { .. }
            | Self::SetGhostAudio(_)
            | Self::AddOwnerSlot { .. } => SystemReactionClass::HostConsequence,
            Self::SetState { .. } => SystemReactionClass::SlotWrite,
        }
    }
}

#[cfg(test)]
mod kind_tests {
    use super::*;

    // Drift guard: the foundation name list the script compiler reads is
    // exactly `SystemReactionKind::ALL`, in order.
    #[test]
    fn foundation_system_reaction_names_match_every_kind() {
        let names: Vec<_> = SystemReactionKind::ALL
            .iter()
            .map(|kind| kind.primitive_name())
            .collect();
        assert_eq!(names, postretro_foundation::SYSTEM_REACTION_PRIMITIVES);
    }
}

#[cfg(test)]
mod audience_tests {
    use super::*;
    fn command(sound: bool, audience: Option<Audience>) -> SystemReactionCommand {
        if sound {
            SystemReactionCommand::PlaySound {
                audience,
                sound: "beep".into(),
                bus: None,
                at: None,
            }
        } else {
            SystemReactionCommand::FlashScreen {
                audience,
                color: [1.0; 4],
                duration_ms: 100.0,
            }
        }
    }

    #[test]
    fn presentation_audience_resolution_matches_every_origin_subject_row() {
        use Audience::*;
        let trigger = |player| {
            FireOrigin::HostOnly(FireSubject::Trigger {
                player,
                trigger: EntityId::from_raw(7),
                occupants: vec![Seat(0), Seat(2)],
            })
        };
        let event = |player| FireOrigin::HostOnly(FireSubject::PlayerEvent { player });
        let none = FireOrigin::HostOnly(FireSubject::None);
        let departed =
            |sounds_everywhere| FireOrigin::HostOnly(FireSubject::Departed { sounds_everywhere });
        let every = FireOrigin::EveryMachine(MachineSource::HostMirrored);
        let rows = [
            (every.clone(), None, false, Recipients::Local),
            (every.clone(), Some(Everyone), true, Recipients::Local),
            (every.clone(), Some(Activators), true, Recipients::Skip),
            (every.clone(), Some(Trigger), false, Recipients::Skip),
            (every.clone(), Some(Player), false, Recipients::Skip),
            (trigger(Some(Seat(2))), None, true, Recipients::Everyone),
            (
                trigger(Some(Seat(2))),
                None,
                false,
                Recipients::Seats(vec![Seat(2)]),
            ),
            (trigger(None), None, false, Recipients::Seats(vec![])),
            (
                trigger(Some(Seat(2))),
                Some(Activators),
                true,
                Recipients::Seats(vec![Seat(2)]),
            ),
            (
                trigger(None),
                Some(Activators),
                true,
                Recipients::Seats(vec![]),
            ),
            (
                trigger(None),
                Some(Trigger),
                false,
                Recipients::Seats(vec![Seat(0), Seat(2)]),
            ),
            (trigger(None), Some(Player), false, Recipients::Skip),
            (
                event(Some(Seat(3))),
                None,
                true,
                Recipients::Seats(vec![Seat(3)]),
            ),
            (
                event(Some(Seat(3))),
                Some(Player),
                false,
                Recipients::Seats(vec![Seat(3)]),
            ),
            (event(None), None, true, Recipients::Local),
            (event(None), Some(Player), false, Recipients::Local),
            (event(None), Some(Activators), false, Recipients::Skip),
            (event(None), Some(Trigger), false, Recipients::Skip),
            (none.clone(), None, true, Recipients::Everyone),
            (none.clone(), None, false, Recipients::Local),
            (none.clone(), Some(Activators), true, Recipients::Skip),
            (none.clone(), Some(Trigger), true, Recipients::Skip),
            (none.clone(), Some(Player), true, Recipients::Skip),
            (none.clone(), Some(Everyone), false, Recipients::Everyone),
            (trigger(None), Some(Everyone), false, Recipients::Everyone),
            (event(None), Some(Everyone), true, Recipients::Everyone),
            (departed(true), None, true, Recipients::Everyone),
            (departed(true), None, false, Recipients::Seats(vec![])),
            (departed(false), None, true, Recipients::Seats(vec![])),
            (departed(false), Some(Everyone), false, Recipients::Everyone),
            (departed(false), Some(Player), true, Recipients::Skip),
        ];
        for (origin, audience, sound, expected) in rows {
            let queue = SystemCommandQueue::new();
            queue.replace_fire_context(SystemCommandFireContext {
                origin: origin.clone(),
                source: "row".into(),
                ..Default::default()
            });
            let command = command(sound, audience);
            queue.push(command.clone());
            let entries = queue.take_ordered();
            if expected == Recipients::Skip {
                assert!(entries.is_empty(), "{origin:?} {audience:?}");
            } else {
                assert_eq!(entries, [(expected, command)], "{origin:?} {audience:?}");
            }
        }
    }

    #[test]
    fn presentation_origin_does_not_route_other_classes_and_followups_clear_subject() {
        let queue = SystemCommandQueue::new();
        let origin = FireOrigin::HostOnly(FireSubject::PlayerEvent {
            player: Some(Seat(2)),
        });
        queue.replace_fire_context(SystemCommandFireContext {
            origin: origin.clone(),
            ..Default::default()
        });
        queue.push(SystemReactionCommand::PopTree);
        assert_eq!(queue.take(), [SystemReactionCommand::PopTree]);
        assert_eq!(origin.follow_up(), FireOrigin::HostOnly(FireSubject::None));
        for source in [
            MachineSource::HostTimeline,
            MachineSource::HostMirrored,
            MachineSource::OwnState,
        ] {
            assert_eq!(
                FireOrigin::EveryMachine(source).follow_up(),
                FireOrigin::EveryMachine(source)
            );
        }
    }
}
