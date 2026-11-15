// Machine-local system reactions and the scope a `wait` landing dispatches in.
// See: context/lib/scripting.md §12 (Steps after a `wait`)

use std::cell::{Cell, RefCell};

use crate::slot_table::{ReplicationScope, SlotTable};

use super::system_commands::{SystemReactionClass, SystemReactionKind};

/// The machine a predicate answers for. Single player is a host.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MachineRole {
    Host,
    Client,
}

/// Never replicated and not consequential: what the same reaction does on one
/// machine stays on that machine. The client tail filter, the late-joiner
/// convergence and the player-event install rule all read this one predicate.
///
/// `returnToFrontend` and `leaveSession` are machine-local only on a client,
/// so a client tail parks and lands them; the app drain then decides what each
/// does by role. `endSession` is a host consequence everywhere. `args` names
/// the slot a `setState` writes; an unknown slot is not machine-local.
pub fn system_reaction_is_machine_local(
    kind: SystemReactionKind,
    args: &serde_json::Value,
    slot_table: &SlotTable,
    role: MachineRole,
) -> bool {
    match kind.class() {
        SystemReactionClass::Presentation | SystemReactionClass::MachineLocal => true,
        SystemReactionClass::SlotWrite => args
            .get("slot")
            .and_then(serde_json::Value::as_str)
            .and_then(|slot| slot_table.get(slot))
            .is_some_and(|record| record.schema.network == ReplicationScope::None),
        SystemReactionClass::HostConsequence => {
            matches!(
                kind,
                SystemReactionKind::ReturnToFrontend | SystemReactionKind::LeaveSession
            ) && role == MachineRole::Client
        }
    }
}

/// [`system_reaction_is_machine_local`] by primitive name; any primitive that
/// is not a system reaction is not machine-local.
pub fn primitive_is_machine_local(
    primitive: &str,
    args: &serde_json::Value,
    slot_table: &SlotTable,
    role: MachineRole,
) -> bool {
    SystemReactionKind::from_primitive_name(primitive)
        .is_some_and(|kind| system_reaction_is_machine_local(kind, args, slot_table, role))
}

/// How a client lands a tail: every machine-local step, or, for a segment due
/// before the host tick this client revealed at (a late joiner's), only the
/// steps that converge state. Transient presentation, `returnToFrontend` and
/// `leaveSession` are skipped there, since the moment they belonged to has
/// passed: a late joiner stays connected.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ClientLanding {
    Land,
    Converge,
}

impl ClientLanding {
    /// Whether a client landing in this mode applies `primitive`.
    pub fn keeps(self, primitive: &str, args: &serde_json::Value, slot_table: &SlotTable) -> bool {
        let Some(kind) = SystemReactionKind::from_primitive_name(primitive) else {
            return false;
        };
        if !system_reaction_is_machine_local(kind, args, slot_table, MachineRole::Client) {
            return false;
        }
        match self {
            Self::Land => true,
            Self::Converge => {
                kind.class() != SystemReactionClass::Presentation
                    && !matches!(
                        kind,
                        SystemReactionKind::ReturnToFrontend | SystemReactionKind::LeaveSession
                    )
            }
        }
    }
}

/// A step a client landing dropped: the reaction holding it and its primitive.
pub type DroppedStep = (String, String);

/// Set by the scheduler for the whole of one landing's drain: its tail and the
/// follow-ups that tail queued, which dispatch after the tail's fire context
/// is restored. Dispatch and presentation capture read it there.
#[derive(Debug, Default)]
pub struct LandingScope {
    /// A host-only or host-mirrored landing on the host: light, fog and
    /// emitter writes record as host-only overrides, because no client lands
    /// them (`coop--networked-by-default` AC10).
    host_capture: Cell<bool>,
    /// A client landing: only steps this mode keeps apply.
    client: Cell<Option<ClientLanding>>,
    /// The first step a client landing dropped, in authored order: the
    /// reaction that holds it (the landing's own, or a `fire` target's) and
    /// its primitive.
    first_dropped: RefCell<Option<DroppedStep>>,
    /// The landing kept a `leaveSession`: the client leaves when it drains,
    /// so what the landing drops after it is routine, never a mistake.
    left: Cell<bool>,
}

impl LandingScope {
    pub fn host_capture(&self) -> bool {
        self.host_capture.get()
    }
    pub fn client(&self) -> Option<ClientLanding> {
        self.client.get()
    }
    /// Enter a landing's scope; the previous scope is restored on drop.
    pub fn enter(
        &self,
        host_capture: bool,
        client: Option<ClientLanding>,
    ) -> LandingScopeGuard<'_> {
        let previous = (
            self.host_capture.replace(host_capture),
            self.client.replace(client),
            self.first_dropped.take(),
            self.left.replace(false),
        );
        LandingScopeGuard {
            scope: self,
            previous: Some(previous),
        }
    }
    /// Record a step a client landing kept.
    pub fn note_kept(&self, primitive: &str) {
        if primitive == SystemReactionKind::LeaveSession.primitive_name() {
            self.left.set(true);
        }
    }
    /// Record a step a client landing did not apply. Returns `false` for a
    /// step after a kept `leaveSession`, which is not reported: the steps
    /// after a leave drop on every path (`scripting.md` §10.4).
    pub fn note_dropped(&self, reaction: &str, primitive: &str) -> bool {
        if self.left.get() {
            return false;
        }
        let mut first = self.first_dropped.borrow_mut();
        if first.is_none() {
            *first = Some((reaction.to_string(), primitive.to_string()));
        }
        true
    }
    /// The first dropped step of the landing in scope.
    pub fn first_dropped(&self) -> Option<DroppedStep> {
        self.first_dropped.borrow().clone()
    }
}

pub struct LandingScopeGuard<'a> {
    scope: &'a LandingScope,
    previous: Option<(bool, Option<ClientLanding>, Option<DroppedStep>, bool)>,
}

impl Drop for LandingScopeGuard<'_> {
    fn drop(&mut self) {
        if let Some((host_capture, client, first_dropped, left)) = self.previous.take() {
            self.scope.host_capture.set(host_capture);
            self.scope.client.set(client);
            *self.scope.first_dropped.borrow_mut() = first_dropped;
            self.scope.left.set(left);
        }
    }
}
