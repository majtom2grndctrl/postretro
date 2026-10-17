// Correlated start/release/cancel recovery beside movement playout.
// See: context/lib/networking.md §Combat authority · §`shot_id`: the security spine
use crate::prediction::client_tick_le;
use crate::sim::RemoteStartAim;
use postretro_foundation::{ActivationInput, ActivationRelease, ActivationToken};
use postretro_sim::weapon::pawn_gate::VaultSparedCharges;
use std::collections::VecDeque;

pub const MAX_RETAINED_ACTIVATION_EDGES: usize = 64;
/// Two seconds at 60 Hz: how long an unknown edge, or a due start the lane
/// cannot admit, is retained.
pub(crate) const RETENTION_TICKS: u32 = 120;
/// Resolved client ticks whose fire gate the lane remembers, for a start that
/// reaches it after its own tick resolved: as long as a due start is retained.
const FIRE_GATE_HISTORY: usize = RETENTION_TICKS as usize;
#[derive(Debug, Clone, Copy)]
struct RetainedEdge {
    token: ActivationToken,
    release_tick: Option<u32>,
    cancel: bool,
    first_tick: u32,
    cancel_first_tick: Option<u32>,
    /// Client tick of the first command that carried the cancel. A cancel
    /// waits until the live execution's clock reaches it.
    cancel_client_tick: Option<u32>,
    delivered: bool,
    cancel_delivered: bool,
}
impl RetainedEdge {
    /// A release still to deliver. A cancel stamped at or before the release
    /// tick wins over it, as within one command. A later cancel, such as a
    /// vault entry after the client fired its charge, waits behind it, so the
    /// charged shot is minted first.
    fn release_pending(&self) -> bool {
        let suppressed = self.cancel
            && self
                .cancel_client_tick
                .zip(self.release_tick)
                .is_none_or(|(cancel, release)| client_tick_le(cancel, release));
        !self.delivered && self.release_tick.is_some() && !suppressed
    }
}
/// A client-named start retained beside movement playout, so a catch-up trim or
/// stale-drop of its carrying command cannot erase it.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct RetainedStart {
    /// Intake requires the start tick to be the carrying command's tick, so the
    /// start is due once the resolved cursor reaches it and never fires ahead
    /// of its own aim.
    pub token: ActivationToken,
    /// Firing slot the carrying command named.
    pub firing_slot: u8,
    /// Aim the carrying command declared, already through intake sanitization.
    /// The start's shot fires along it, whichever later command delivers it.
    pub aim: RemoteStartAim,
    pub teleport_yaw_stamp: f32,
    /// Host tick at which playout first found it due. Expiry runs from here, so
    /// a start playout has not reached yet is never refused for waiting.
    due_since: Option<u32>,
    /// The shooter's pawn-state fire gate at the start of the start's own
    /// client tick, stamped by the first resolution that finds it due
    /// (`stamp_fire_gate`). A start delivered later is judged by it, not by the
    /// pawn's state at delivery. Set to `Some(true)` ahead of that when a vault
    /// cancel the start missed cannot be held (`observe_start`,
    /// `cancel_before`): the start is refused instead of firing through it.
    fire_blocked: Option<bool>,
}
/// Why the lane refused a retained start.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LaneRefusal {
    /// The client half of the cadence rule refused it at the lane front.
    ClientHalf,
    /// Its claimed host time lagged past the catch-up allowance.
    Lagged,
    /// It waited due in the lane for `RETENTION_TICKS` without admission.
    Expired,
    /// A terminal settled it while it was retained.
    Settled,
}
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) enum DueStart {
    Start(RetainedStart),
    /// `RETENTION_TICKS` passed since it first became due without admission;
    /// the caller publishes the refusal.
    Expired(ActivationToken),
}
#[derive(Debug, Default)]
pub(crate) struct ActivationEdges {
    edges: VecDeque<RetainedEdge>,
    starts: VecDeque<RetainedStart>,
    /// The start most recently taken from the lane, for its shot's aim.
    delivered: Option<RetainedStart>,
    /// The start most recently refused from the lane and why, for its own
    /// weapon's recovery in the refusal.
    refused: Option<(RetainedStart, LaneRefusal)>,
    admitted: VecDeque<ActivationToken>,
    overflow_cancel: Option<ActivationToken>,
    settled_start: [Option<u32>; 2],
    /// `(client tick, fire gate)` as each recently resolved tick started,
    /// oldest first, at most `FIRE_GATE_HISTORY` ticks.
    fire_gates: VecDeque<(u32, bool)>,
    /// The shooter's firing-slot actions whose charge a vault spares, as of
    /// the latest resolution (`set_vault_spared_charges`).
    vault_spared: VaultSparedCharges,
}
impl ActivationEdges {
    /// Retain a start in intake order, received on host tick `tick`.
    /// Duplicates, settled starts, and starts past the per-client bound are not
    /// retained, so playout never delivers them.
    ///
    /// A start whose own tick already resolved arrives late, such as a later
    /// copy of a command the playout yielded past. It missed `cancel_before`
    /// for any closed tick recorded since, so it takes a cancel at the first
    /// one now, before any of its edges can be delivered: a release the client
    /// stamped at or after that tick is then suppressed, as the client's gate
    /// cancelled the charge before it.
    #[cfg(test)]
    pub fn observe_start(
        &mut self,
        token: ActivationToken,
        firing_slot: u8,
        aim: RemoteStartAim,
        tick: u32,
    ) {
        self.observe_start_stamped(token, firing_slot, aim, 0.0, tick);
    }

    pub fn observe_start_stamped(
        &mut self,
        token: ActivationToken,
        firing_slot: u8,
        aim: RemoteStartAim,
        teleport_yaw_stamp: f32,
        tick: u32,
    ) {
        if self.is_settled(token)
            || self.admitted.contains(&token)
            || self.starts.iter().any(|start| start.token == token)
            || self.starts.len() >= MAX_RETAINED_ACTIVATION_EDGES
        {
            return;
        }
        // A closed tick recorded after the start's own: it arrives late. A
        // charge the vault spares misses only one after its known release.
        let missed = if self.vault_spared.spares(firing_slot, token.lane) {
            self.known_release(token)
                .and_then(|release| self.first_closed_after(release))
        } else {
            self.first_closed_after(token.start_tick)
        };
        // Unheld, the cancel could not stop the start: refuse it instead.
        let fire_blocked = missed
            .is_some_and(|closed| !self.retain(token, None, Some(closed), tick))
            .then_some(true);
        self.starts.push_back(RetainedStart {
            teleport_yaw_stamp,
            token,
            firing_slot,
            aim,
            due_since: None,
            fire_blocked,
        });
    }
    /// The oldest retained start once its command tick is resolved; the caller
    /// holds it while an execution is live. Starts leave strictly in intake
    /// order, so the ledger's monotonic settled-start watermark never passes a
    /// start still retained. A due start not admitted within `RETENTION_TICKS`
    /// expires.
    pub fn due_start(&mut self, resolved_tick: u32, tick: u32) -> Option<DueStart> {
        let front = self.starts.front_mut()?;
        if !client_tick_le(front.token.start_tick, resolved_tick) {
            return None;
        }
        let due_since = *front.due_since.get_or_insert(tick);
        if tick.wrapping_sub(due_since) >= RETENTION_TICKS {
            let token = front.token;
            self.refuse_front(token, LaneRefusal::Expired);
            return Some(DueStart::Expired(token));
        }
        Some(DueStart::Start(*front))
    }
    /// Remove the delivered front start and open its edge correlation. False
    /// when a terminal settled it meanwhile; the caller refuses it.
    pub fn take_start(&mut self, token: ActivationToken) -> bool {
        let taken = self.pop_front_start(token);
        let admitted = self.admit(token);
        if let Some(start) = taken {
            if admitted {
                self.delivered = Some(start);
            } else {
                self.refused = Some((start, LaneRefusal::Settled));
            }
        }
        admitted
    }
    /// Refuse the front start without admitting it; the caller publishes the
    /// refusal and settles it.
    pub fn refuse_front(&mut self, token: ActivationToken, reason: LaneRefusal) {
        if let Some(start) = self.pop_front_start(token) {
            self.refused = Some((start, reason));
        }
    }
    fn pop_front_start(&mut self, token: ActivationToken) -> Option<RetainedStart> {
        if self
            .starts
            .front()
            .is_some_and(|start| start.token == token)
        {
            self.starts.pop_front()
        } else {
            None
        }
    }
    /// Whether a retained start from `firing_slot` is due: its command tick
    /// has resolved, so the client stamped it before the resolving command.
    pub fn has_due_start(&self, firing_slot: u8, resolved_tick: u32) -> bool {
        self.starts.iter().any(|start| {
            start.firing_slot == firing_slot
                && client_tick_le(start.token.start_tick, resolved_tick)
        })
    }
    /// Command tick of the oldest start still retained. Presses stamped after
    /// it wait, so none reaches the weapon ahead of that start.
    pub fn oldest_start_tick(&self) -> Option<u32> {
        self.starts.front().map(|start| start.token.start_tick)
    }
    /// Client release tick of `token`'s release, received but not yet
    /// delivered. A cancel stamped at or before it suppresses it, so none is
    /// reported.
    pub fn undelivered_release(&self, token: ActivationToken) -> Option<u32> {
        self.edges
            .iter()
            .find(|edge| edge.token == token && edge.release_pending())
            .and_then(|edge| edge.release_tick)
    }
    /// Firing slot `token`'s start named, once the lane has refused it.
    pub fn refused_slot(&self, token: ActivationToken) -> Option<u8> {
        self.refused
            .filter(|(start, _)| start.token == token)
            .map(|(start, _)| start.firing_slot)
    }
    /// Why the lane refused `token`'s start, once it has.
    pub fn refusal(&self, token: ActivationToken) -> Option<LaneRefusal> {
        self.refused
            .filter(|(start, _)| start.token == token)
            .map(|(_, reason)| reason)
    }
    /// Record `blocked`, the shooter's fire gate at the start of client tick
    /// `resolved_tick`, and stamp every retained or just-delivered start
    /// stamped at or before it that has no verdict yet. Called once per
    /// resolution, before its movement runs. Returns the gate recorded for
    /// `resolved_tick`, which the caller judges that tick by.
    ///
    /// A start takes the gate recorded for its own tick. One that reached the
    /// lane after its tick resolved, such as a later copy of a command the
    /// playout yielded past, finds it here; it took its missed vault cancel
    /// at intake (`observe_start`). A start whose tick never resolved, because
    /// a catch-up trim reseated the cursor past it, or resolved before the
    /// history's reach, takes the gate of the resolution that stamps it.
    pub fn stamp_fire_gate(&mut self, resolved_tick: u32, blocked: bool) -> bool {
        // The cursor never moves back, so a repeat is the same tick resolving
        // again after a frontier hold. The first resolution is the closest the
        // host has to the client's start of that tick; each re-resolution adds
        // a host-only held step on top, which can enter or leave a vault the
        // client never did. Keep the first.
        let recorded = match self.fire_gates.back() {
            Some(&(last, gate)) if last == resolved_tick => gate,
            _ => {
                if self.fire_gates.len() == FIRE_GATE_HISTORY {
                    self.fire_gates.pop_front();
                }
                self.fire_gates.push_back((resolved_tick, blocked));
                blocked
            }
        };
        let gates = &self.fire_gates;
        for start in self.starts.iter_mut().chain(self.delivered.iter_mut()) {
            let own_tick = start.token.start_tick;
            if start.fire_blocked.is_some() || !client_tick_le(own_tick, resolved_tick) {
                continue;
            }
            let own = gates
                .iter()
                .rev()
                .find(|&&(gate_tick, _)| gate_tick == own_tick)
                .map_or(recorded, |&(_, gate)| gate);
            start.fire_blocked = Some(own);
        }
        recorded
    }
    /// Retain a cancel stamped `client_tick`, received on host tick `tick`, for
    /// every execution the client began before that tick: `live`, the start
    /// just delivered, and each retained start stamped earlier. Like a cancel
    /// the client sent, each waits until its execution's clock reaches
    /// `client_tick`, so the shots the client fired before it are minted
    /// first, and is inert for an execution that ends sooner.
    ///
    /// Bound: with `MAX_RETAINED_ACTIVATION_EDGES` edges held, which only a
    /// flooding client reaches, an admitted execution takes the overflow
    /// cancel at once and a retained start is refused at delivery. Either
    /// loses shots fired before `client_tick`; neither fires through the vault.
    ///
    /// An execution the closed tick spares (`vault_spares`) takes none: its
    /// client kept charging it through the vault.
    pub fn cancel_before(&mut self, client_tick: u32, live: Option<ActivationToken>, tick: u32) {
        let before = |token: ActivationToken| !client_tick_le(client_tick, token.start_tick);
        let delivered = self
            .delivered
            .map(|start| start.token)
            .filter(|&token| before(token));
        for token in live.into_iter().chain(delivered) {
            let slot = self.firing_slot(token);
            if !self.vault_spares(token, slot, client_tick) {
                self.retain(token, None, Some(client_tick), tick);
            }
        }
        // `retain` never reads the retained starts.
        let mut starts = std::mem::take(&mut self.starts);
        for start in starts.iter_mut().filter(|start| before(start.token)) {
            if self.vault_spares(start.token, Some(start.firing_slot), client_tick) {
                continue;
            }
            if !self.retain(start.token, None, Some(client_tick), tick) {
                start.fire_blocked = Some(true);
            }
        }
        self.starts = starts;
    }
    /// Record which of the shooter's firing-slot actions charge through a
    /// vault, read from its inventory as this resolution starts.
    pub fn set_vault_spared_charges(&mut self, spared: VaultSparedCharges) {
        self.vault_spared = spared;
    }
    /// Whether the gate closed at client tick `closed` spares `live`, as the
    /// client spared it: the caller must not cancel it at once.
    pub fn vault_spares_live(&self, live: ActivationToken, closed: u32) -> bool {
        self.vault_spares(live, self.firing_slot(live), closed)
    }
    /// Whether a fire gate closed at client tick `closed` spares `token`'s
    /// execution, as the client's gate did: its action, in `firing_slot`,
    /// charges through a vault, and no release the client stamped before
    /// `closed` is known, so it was still charging as that tick began. The
    /// client holds a spared charge's release until the gate opens, so a
    /// release stamped at or after `closed` never fired it before the vault.
    /// An unknown slot spares nothing, as before.
    fn vault_spares(&self, token: ActivationToken, firing_slot: Option<u8>, closed: u32) -> bool {
        firing_slot.is_some_and(|slot| self.vault_spared.spares(slot, token.lane))
            && self
                .known_release(token)
                .is_none_or(|release| client_tick_le(closed, release))
    }
    /// The firing slot `token`'s start named: retained, or the start most
    /// recently delivered, which is the live execution's.
    fn firing_slot(&self, token: ActivationToken) -> Option<u8> {
        self.starts
            .iter()
            .chain(self.delivered.iter())
            .find(|start| start.token == token)
            .map(|start| start.firing_slot)
    }
    /// Client release tick of `token`'s release, received, delivered or not.
    fn known_release(&self, token: ActivationToken) -> Option<u32> {
        self.edges
            .iter()
            .find(|edge| edge.token == token)
            .and_then(|edge| edge.release_tick)
    }
    /// The first closed gate recorded after client tick `tick`.
    fn first_closed_after(&self, tick: u32) -> Option<u32> {
        self.fire_gates
            .iter()
            .find(|&&(gate_tick, gate)| gate && !client_tick_le(gate_tick, tick))
            .map(|&(gate_tick, _)| gate_tick)
    }
    /// The live execution whose pending release must wait, this resolution,
    /// for its own client tick: a charge a vault spared, released ahead of
    /// `resolved`. `None` while nothing has resolved.
    ///
    /// Its client held the release over the vault and stamped it on the first
    /// open tick after it (`pawn_gate::hold_release_over_gate`), so it reaches
    /// intake about a buffer's depth before that tick resolves. Delivered then,
    /// it would fire the charged shot on the host mid-climb, from below the lip
    /// and along an earlier command's aim, while the client fired from the top.
    /// The execution counts as spared when its action, in its firing slot,
    /// charges through a vault and a closed gate recorded after its start is
    /// at or before the release. That gate passed over it (`vault_spares`).
    /// Every other release keeps its timing.
    ///
    /// Held, it goes on the resolution of its own tick. Within a resolution
    /// delivery runs before `stamp_fire_gate` records that tick's gate, and
    /// that is safe here. The release's tick is one the client found open.
    /// Should the host's gate for it be closed, it still spares the release
    /// (`vault_spares` treats a release at the closed tick as charged
    /// through it), so the stamp after delivery could never have cancelled
    /// it. Delivered then, the shot fires in that tick's simulation, on the
    /// client's tick and along its command's aim. Waiting for a later
    /// resolution would fire a tick late, along the next command's aim.
    ///
    /// A release that reaches intake before the vault's first closed tick
    /// resolves on the host, possible only when the vault is shorter than the
    /// playout lead, has no closed gate yet and goes at once, as before.
    fn release_held_to_its_tick(&self, resolved: Option<u32>) -> Option<ActivationToken> {
        let start = self.delivered?;
        let token = start.token;
        let release = self.undelivered_release(token)?;
        let spared = self.vault_spared.spares(start.firing_slot, token.lane)
            && self
                .first_closed_after(token.start_tick)
                .is_some_and(|closed| client_tick_le(closed, release));
        let ahead = resolved.is_none_or(|resolved| !client_tick_le(release, resolved));
        (spared && ahead).then_some(token)
    }
    /// The fire gate stamped on `token`'s start, once the lane delivered it.
    pub fn delivered_fire_blocked(&self, token: ActivationToken) -> Option<bool> {
        self.delivered
            .filter(|start| start.token == token)
            .and_then(|start| start.fire_blocked)
    }
    /// Aim captured with `token`'s start, once the lane has delivered it.
    pub fn delivered_aim(&self, token: ActivationToken) -> Option<RemoteStartAim> {
        self.delivered
            .filter(|start| start.token == token)
            .map(|start| start.aim)
    }

    pub fn delivered_teleport_yaw_stamp(&self, token: ActivationToken) -> Option<f32> {
        self.delivered
            .filter(|start| start.token == token)
            .map(|start| start.teleport_yaw_stamp)
    }
    /// Retain the release and cancel edges a command at `client_tick` carried,
    /// received on host tick `tick`.
    ///
    /// A release of a charge the vault spares can arrive after a closed tick
    /// already passed over that charge, when its command came late. Released
    /// before that tick, the charge fired before the vault and the client's
    /// gate stopped the rest of it there, so it takes that cancel now.
    pub fn observe(&mut self, input: ActivationInput, client_tick: u32, tick: u32) {
        self.prune(tick);
        if let Some(release) = input.release {
            self.retain(release.token, Some(release.release_tick), None, tick);
            let token = release.token;
            if self
                .firing_slot(token)
                .is_some_and(|slot| self.vault_spared.spares(slot, token.lane))
                && let Some(closed) = self
                    .known_release(token)
                    .and_then(|release| self.first_closed_after(release))
            {
                self.retain(token, None, Some(closed), tick);
            }
        }
        if let Some(token) = input.cancel {
            self.retain(token, None, Some(client_tick), tick);
        }
    }
    /// `cancel`: the client tick of the command carrying a cancel edge.
    /// False when the edge was dropped: its token is settled, or the table is
    /// full and the token not admitted. An admitted token over the bound takes
    /// the overflow cancel instead.
    fn retain(
        &mut self,
        token: ActivationToken,
        release_tick: Option<u32>,
        cancel_client_tick: Option<u32>,
        tick: u32,
    ) -> bool {
        let cancel = cancel_client_tick.is_some();
        if self.is_settled(token) && !self.admitted.contains(&token) {
            return false;
        }
        if let Some(edge) = self.edges.iter_mut().find(|e| e.token == token) {
            // Cancellation may strengthen an undelivered release; duplicates never
            // refresh age or redeliver a terminal edge.
            if cancel && !edge.cancel_delivered {
                if edge.cancel_first_tick.is_none() {
                    edge.cancel_first_tick = Some(tick);
                    edge.cancel_client_tick = cancel_client_tick;
                } else if let (Some(held), Some(stamped)) =
                    (edge.cancel_client_tick, cancel_client_tick)
                    && !client_tick_le(held, stamped)
                {
                    // The client's own cancel and a vault entry can both name
                    // an execution, in either arrival order. The earlier stamp
                    // stops it, so no shot after either is minted.
                    edge.cancel_client_tick = Some(stamped);
                }
            }
            edge.cancel |= cancel;
            if !edge.delivered && edge.release_tick.is_none() {
                edge.release_tick = release_tick;
            }
            return true;
        }
        if self.edges.len() >= MAX_RETAINED_ACTIVATION_EDGES {
            if self.admitted.contains(&token) {
                self.overflow_cancel = Some(token);
                return true;
            }
            return false;
        }
        self.edges.push_back(RetainedEdge {
            token,
            release_tick,
            cancel,
            first_tick: tick,
            cancel_first_tick: cancel.then_some(tick),
            cancel_client_tick,
            delivered: false,
            cancel_delivered: false,
        });
        true
    }
    pub fn admit(&mut self, token: ActivationToken) -> bool {
        if self.admitted.contains(&token) {
            return true;
        }
        if self.is_settled(token) {
            return false;
        }
        if self.admitted.len() == MAX_RETAINED_ACTIVATION_EDGES {
            self.admitted.pop_front();
        }
        self.admitted.push_back(token);
        true
    }
    /// Deliver at most one cancel and one release. `horizon`: client tick the
    /// live execution's clock has reached; a cancel stamped after it waits, so
    /// every shot the client fired before cancelling is minted first. A cancel
    /// never overtakes a release of its own execution stamped before it, since
    /// that release may fire a charge; `spent_release` names the execution
    /// whose release can no longer fire anything (it does not charge or,
    /// defensively, its charge already fired), whose cancel goes regardless of
    /// a pending release. That cancel is still gated by `horizon` (`reached`)
    /// and by admission.
    ///
    /// `resolved`: the client tick the resolved cursor has reached, `None`
    /// before any. A release otherwise goes at the first resolution after
    /// admission, ahead of its own tick. The one exception is a charge a vault
    /// spared, whose release waits for the resolution of its own client tick
    /// (`release_held_to_its_tick`).
    pub fn deliver(
        &mut self,
        command: &mut ActivationInput,
        tick: u32,
        live: Option<ActivationToken>,
        horizon: Option<u32>,
        spent_release: Option<ActivationToken>,
        resolved: Option<u32>,
    ) {
        self.prune(tick);
        command.release = None;
        command.cancel = None;
        let held = self.release_held_to_its_tick(resolved);
        let reached = |edge: &RetainedEdge| {
            edge.cancel_client_tick
                .zip(horizon)
                .is_none_or(|(stamped, horizon)| client_tick_le(stamped, horizon))
        };
        // Queue admission does not authorize a competing start. Its edges must not
        // displace the execution actually owned by the host ledger.
        let cancel_edge = self
            .edges
            .iter_mut()
            .filter(|edge| {
                edge.cancel
                    && !edge.cancel_delivered
                    && (!edge.release_pending() || spent_release == Some(edge.token))
                    && reached(edge)
                    && self.admitted.contains(&edge.token)
            })
            .min_by_key(|edge| live != Some(edge.token));
        let prefer_retained = cancel_edge
            .as_ref()
            .is_some_and(|edge| live == Some(edge.token))
            && self.overflow_cancel != live;
        if self.overflow_cancel.is_some() && !prefer_retained {
            command.cancel = self.overflow_cancel.take();
        } else if let Some(edge) = cancel_edge {
            command.cancel = Some(edge.token);
            edge.cancel_delivered = true;
            edge.delivered = true;
        }
        if let Some(edge) = self
            .edges
            .iter_mut()
            .filter(|edge| {
                edge.release_pending()
                    && command.cancel != Some(edge.token)
                    && held != Some(edge.token)
                    && self.admitted.contains(&edge.token)
            })
            .min_by_key(|edge| live != Some(edge.token))
        {
            command.release = edge.release_tick.map(|release_tick| ActivationRelease {
                token: edge.token,
                release_tick,
            });
            edge.delivered = true;
        }
    }
    pub fn terminal(&mut self, token: ActivationToken) {
        self.admitted.retain(|admitted| *admitted != token);
        self.edges.retain(|edge| edge.token != token);
        self.starts.retain(|start| start.token != token);
        if self.overflow_cancel == Some(token) {
            self.overflow_cancel = None;
        }
        settle(&mut self.settled_start, token);
    }
    fn is_settled(&self, token: ActivationToken) -> bool {
        self.settled_start[token.lane as usize]
            .is_some_and(|watermark| (token.start_tick.wrapping_sub(watermark) as i32) <= 0)
    }
    /// Age out unknown edges. The edges of a retained start, or of the start
    /// just delivered, never age: they are delivered once it is admitted, or
    /// dropped when it is refused. Settling an edge raises its lane's watermark
    /// over every start at or below its token, so an expired edge stays,
    /// undeliverable, until no retained start sits at or below it; only a
    /// start's own refusal or terminal settles a start.
    fn prune(&mut self, tick: u32) {
        let starts = &self.starts;
        let delivered = self.delivered.map(|start| start.token);
        let own = |token: ActivationToken| {
            delivered == Some(token) || starts.iter().any(|start| start.token == token)
        };
        let shields = |token: ActivationToken| {
            starts.iter().any(|start| {
                start.token.lane == token.lane
                    && client_tick_le(start.token.start_tick, token.start_tick)
            })
        };
        for edge in &mut self.edges {
            if own(edge.token) {
                continue;
            }
            if tick.wrapping_sub(edge.first_tick) >= RETENTION_TICKS {
                edge.delivered = true;
            }
            if edge
                .cancel_first_tick
                .is_some_and(|first| tick.wrapping_sub(first) >= RETENTION_TICKS)
            {
                edge.cancel_delivered = true;
            }
        }
        self.edges.retain(|edge| {
            // A live activation keeps its delivered-edge history until terminal,
            // so duplicate releases cannot revive while later cancellation remains valid.
            let retain = self.admitted.contains(&edge.token)
                || shields(edge.token)
                || tick.wrapping_sub(edge.first_tick) < RETENTION_TICKS
                || edge
                    .cancel_first_tick
                    .is_some_and(|first| tick.wrapping_sub(first) < RETENTION_TICKS);
            if !retain {
                settle(&mut self.settled_start, edge.token);
            }
            retain
        });
    }
}
fn settle(watermarks: &mut [Option<u32>; 2], token: ActivationToken) {
    let watermark = &mut watermarks[token.lane as usize];
    if watermark.is_none_or(|old| (token.start_tick.wrapping_sub(old) as i32) > 0) {
        *watermark = Some(token.start_tick);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use postretro_foundation::ActivationLane;
    fn token(tick: u32) -> ActivationToken {
        ActivationToken {
            start_tick: tick,
            lane: ActivationLane::Secondary,
        }
    }
    fn aim(yaw: f32) -> RemoteStartAim {
        RemoteStartAim { pitch: 0.1, yaw }
    }

    // A start keeps the fire gate of its own tick: a vault that begins after
    // the tick resolved, while the start waits retained, never refuses it, and
    // a start stamped mid-vault stays refused whenever it is delivered.
    #[test]
    fn a_retained_start_keeps_the_fire_gate_stamped_when_its_tick_resolved() {
        let mut edges = ActivationEdges::default();
        edges.observe_start(token(10), 0, aim(0.0), 0);
        edges.observe_start(token(12), 0, aim(0.0), 0);
        edges.stamp_fire_gate(9, true);
        assert!(
            edges
                .starts
                .iter()
                .all(|start| start.fire_blocked.is_none()),
            "a start is stamped only once its own tick resolves"
        );
        edges.stamp_fire_gate(10, false);
        edges.stamp_fire_gate(11, true);
        edges.stamp_fire_gate(12, true);
        assert!(edges.take_start(token(10)));
        assert_eq!(edges.delivered_fire_blocked(token(10)), Some(false));
        assert!(edges.take_start(token(12)));
        assert_eq!(edges.delivered_fire_blocked(token(12)), Some(true));

        // Delivered on its own resolution, before the stamp: the stamp still
        // reaches it.
        let mut edges = ActivationEdges::default();
        edges.observe_start(token(20), 0, aim(0.0), 0);
        assert!(edges.take_start(token(20)));
        assert_eq!(edges.delivered_fire_blocked(token(20)), None);
        edges.stamp_fire_gate(20, true);
        assert_eq!(edges.delivered_fire_blocked(token(20)), Some(true));
    }
    // Regression: a start that reached the lane after its own tick resolved was
    // stamped with the gate of the next resolution, so a vault that began in
    // between refused a shot the client had fired before it; and it carried no
    // cancel for that vault, so it fired through it.
    #[test]
    fn a_start_arriving_after_its_tick_resolved_takes_that_ticks_fire_gate() {
        let mut edges = ActivationEdges::default();
        assert!(!edges.stamp_fire_gate(10, false));
        // A frontier hold resolves tick 10 again after a host-only movement
        // step that entered a vault; the first resolution stands.
        assert!(
            !edges.stamp_fire_gate(10, true),
            "the first resolution of a tick is the faithful one"
        );
        edges.stamp_fire_gate(11, true);
        edges.observe_start(token(10), 0, aim(0.0), 0);
        edges.stamp_fire_gate(12, true);
        assert!(edges.take_start(token(10)));
        assert_eq!(
            edges.delivered_fire_blocked(token(10)),
            Some(false),
            "its own tick's gate, not the stamping resolution's"
        );
        let mut delivered = ActivationInput::default();
        edges.deliver(&mut delivered, 4, Some(token(10)), Some(10), None, None);
        assert_eq!(delivered.cancel, None);
        edges.deliver(&mut delivered, 5, Some(token(10)), Some(11), None, None);
        assert_eq!(
            delivered.cancel,
            Some(token(10)),
            "cancelled where the vault it missed began"
        );

        // A tick older than the history falls back to the stamping resolution.
        let mut edges = ActivationEdges::default();
        for tick in 0..=FIRE_GATE_HISTORY as u32 {
            edges.stamp_fire_gate(tick, false);
        }
        edges.observe_start(token(0), 0, aim(0.0), 0);
        edges.stamp_fire_gate(FIRE_GATE_HISTORY as u32 + 1, true);
        assert!(edges.take_start(token(0)));
        assert_eq!(edges.delivered_fire_blocked(token(0)), Some(true));
    }
    // Regression: a vault entry after a charged release marked the release's
    // edge cancelled, so the release never went out and the charged shot the
    // client fired before the vault was never minted.
    #[test]
    fn a_vault_cancel_waits_behind_a_charge_released_before_it() {
        let release = ActivationInput {
            release: Some(ActivationRelease {
                token: token(6),
                release_tick: 8,
            }),
            ..ActivationInput::default()
        };
        let mut edges = ActivationEdges::default();
        edges.observe_start(token(6), 0, aim(0.0), 0);
        edges.observe(release, 8, 0);
        edges.cancel_before(10, None, 1);
        assert_eq!(
            edges.undelivered_release(token(6)),
            Some(8),
            "the release still holds the charge's clock"
        );
        assert!(edges.take_start(token(6)));
        let mut delivered = ActivationInput::default();
        edges.deliver(&mut delivered, 2, None, Some(8), None, None);
        assert_eq!(delivered.release, release.release);
        assert_eq!(delivered.cancel, None);
        edges.deliver(&mut delivered, 3, Some(token(6)), Some(9), None, None);
        assert_eq!(delivered, ActivationInput::default());
        edges.deliver(&mut delivered, 4, Some(token(6)), Some(10), None, None);
        assert_eq!(delivered.cancel, Some(token(6)));

        // Nothing holding the cancel still never lets it overtake the release.
        let mut edges = ActivationEdges::default();
        edges.admit(token(6));
        edges.observe(release, 8, 0);
        edges.cancel_before(10, Some(token(6)), 1);
        edges.deliver(&mut delivered, 2, Some(token(6)), None, None, None);
        assert_eq!(delivered.release, release.release);
        assert_eq!(delivered.cancel, None);
        edges.deliver(&mut delivered, 3, Some(token(6)), None, None, None);
        assert_eq!(delivered.cancel, Some(token(6)));
    }
    // With the edge table full, a retained start that cannot carry its vault
    // cancel is refused at delivery rather than fired through the vault.
    #[test]
    fn a_retained_start_whose_vault_cancel_cannot_be_held_is_refused() {
        let mut edges = ActivationEdges::default();
        for start in 100..100 + MAX_RETAINED_ACTIVATION_EDGES as u32 {
            edges.observe(
                ActivationInput {
                    cancel: Some(token(start)),
                    ..ActivationInput::default()
                },
                0,
                0,
            );
        }
        edges.observe_start(token(6), 0, aim(0.0), 0);
        edges.stamp_fire_gate(6, false);
        edges.cancel_before(10, None, 0);
        assert!(edges.take_start(token(6)));
        assert_eq!(edges.delivered_fire_blocked(token(6)), Some(true));
    }
    // Intake counterpart of the test above: a start that arrives after a closed
    // tick recorded since its own, with the edge table full, cannot hold the
    // cancel it missed, so it is retained already refused.
    #[test]
    fn a_late_start_whose_vault_cancel_cannot_be_held_at_intake_is_refused() {
        let mut edges = ActivationEdges::default();
        for start in 100..100 + MAX_RETAINED_ACTIVATION_EDGES as u32 {
            edges.observe(
                ActivationInput {
                    cancel: Some(token(start)),
                    ..ActivationInput::default()
                },
                0,
                0,
            );
        }
        edges.stamp_fire_gate(10, true);
        edges.observe_start(token(6), 0, aim(0.0), 0);
        assert_eq!(
            edges.starts.front().map(|start| start.fire_blocked),
            Some(Some(true)),
            "retained, but refused at intake"
        );
        assert!(edges.take_start(token(6)));
        assert_eq!(edges.delivered_fire_blocked(token(6)), Some(true));

        // With room for the edge, the same late start holds its cancel instead.
        let mut edges = ActivationEdges::default();
        edges.stamp_fire_gate(10, true);
        edges.observe_start(token(6), 0, aim(0.0), 0);
        assert_eq!(
            edges.starts.front().map(|start| start.fire_blocked),
            Some(None),
            "not refused: the cancel is held"
        );
    }
    // A late start is observed first, taking its cancel at the closed tick;
    // a release stamped at or after that tick then arrives. The release is
    // suppressed as the client's gate cancelled the charge before it, whichever
    // order the two reach the lane in.
    #[test]
    fn a_release_arriving_after_a_late_starts_intake_cancel_is_suppressed() {
        for release_tick in [10, 12] {
            let mut edges = ActivationEdges::default();
            edges.stamp_fire_gate(10, true);
            edges.observe_start(token(6), 0, aim(0.0), 0);
            edges.observe(
                ActivationInput {
                    release: Some(ActivationRelease {
                        token: token(6),
                        release_tick,
                    }),
                    ..ActivationInput::default()
                },
                release_tick,
                1,
            );
            assert_eq!(
                edges.undelivered_release(token(6)),
                None,
                "the release at {release_tick} is suppressed on arrival"
            );
            assert!(edges.take_start(token(6)));
            let mut delivered = ActivationInput::default();
            edges.deliver(&mut delivered, 2, Some(token(6)), Some(10), None, None);
            assert_eq!(delivered.cancel, Some(token(6)));
            assert_eq!(delivered.release, None, "never delivered");
            edges.deliver(&mut delivered, 3, Some(token(6)), Some(20), None, None);
            assert_eq!(delivered, ActivationInput::default());
        }
    }
    // A vault entry at client tick 10 stops every execution begun before it
    // where its clock reaches 10, as the client's own cancel would: a retained
    // start carries the cancel through delivery; a start stamped at 10 carries
    // none. An earlier client cancel arriving later still stops it sooner.
    #[test]
    fn a_cancel_before_a_tick_waits_for_each_executions_clock() {
        let mut edges = ActivationEdges::default();
        edges.observe_start(token(6), 0, aim(0.0), 0);
        edges.observe_start(token(10), 0, aim(0.0), 0);
        edges.cancel_before(10, None, 0);
        assert!(edges.take_start(token(6)));
        let mut delivered = ActivationInput::default();
        edges.deliver(&mut delivered, 1, Some(token(6)), Some(9), None, None);
        assert_eq!(delivered.cancel, None, "held until the clock reaches 10");
        edges.deliver(&mut delivered, 2, Some(token(6)), Some(10), None, None);
        assert_eq!(delivered.cancel, Some(token(6)));
        edges.terminal(token(6));
        assert!(edges.take_start(token(10)));
        edges.deliver(&mut delivered, 3, Some(token(10)), Some(30), None, None);
        assert_eq!(
            delivered.cancel, None,
            "a start on the closed tick is not cancelled"
        );

        let mut edges = ActivationEdges::default();
        edges.admit(token(4));
        edges.cancel_before(10, Some(token(4)), 0);
        edges.observe(
            ActivationInput {
                cancel: Some(token(4)),
                ..ActivationInput::default()
            },
            8,
            1,
        );
        edges.deliver(&mut delivered, 2, Some(token(4)), Some(8), None, None);
        assert_eq!(delivered.cancel, Some(token(4)), "the earlier stamp wins");
    }
    #[test]
    fn activation_start_lane_retains_a_duplicate_once_with_its_first_carrying_aim() {
        let mut edges = ActivationEdges::default();
        edges.observe_start(token(4), 0, aim(0.0), 0);
        edges.observe_start(token(4), 0, aim(0.7), 0);
        assert_eq!(edges.starts.len(), 1, "a duplicate start is retained once");
        assert_eq!(edges.due_start(3, 0), None, "not due before its tick");
        let Some(DueStart::Start(start)) = edges.due_start(4, 0) else {
            panic!("a resolved start is due");
        };
        assert_eq!(edges.delivered_aim(start.token), None, "not yet delivered");
        assert!(edges.take_start(start.token));
        assert_eq!(
            edges.delivered_aim(start.token),
            Some(aim(0.0)),
            "the delivered start keeps its first carrying command's aim"
        );
        assert_eq!(edges.delivered_aim(token(5)), None);
    }
    #[test]
    fn activation_start_lane_never_retains_a_settled_start_again() {
        let mut edges = ActivationEdges::default();
        edges.observe_start(token(4), 0, aim(0.0), 0);
        assert!(edges.take_start(token(4)));
        edges.terminal(token(4));
        edges.observe_start(token(4), 0, aim(0.0), 0);
        edges.observe_start(token(3), 0, aim(0.0), 0);
        assert!(edges.starts.is_empty());
    }
    #[test]
    fn activation_start_lane_overflow_drops_the_newest_start() {
        let mut edges = ActivationEdges::default();
        let first = 5;
        let overflow = first + MAX_RETAINED_ACTIVATION_EDGES as u32;
        for tick in first..=overflow {
            edges.observe_start(token(tick), 0, aim(0.0), 0);
        }
        assert_eq!(edges.starts.len(), MAX_RETAINED_ACTIVATION_EDGES);
        assert!(
            edges
                .starts
                .iter()
                .all(|start| start.token != token(overflow)),
            "overflow drops the newest start"
        );
    }
    // Regression: a start behind the lane front never aged, but its release edge
    // aged out and settled it, so the lane refused it on admission.
    #[test]
    fn activation_edge_expiry_never_settles_a_still_retained_start() {
        let mut edges = ActivationEdges::default();
        let (front, behind, unknown) = (token(4), token(6), token(8));
        edges.observe_start(front, 0, aim(0.0), 0);
        edges.observe_start(behind, 0, aim(0.0), 0);
        for edge in [behind, unknown] {
            edges.observe(
                ActivationInput {
                    release: Some(ActivationRelease {
                        token: edge,
                        release_tick: edge.start_tick + 1,
                    }),
                    ..ActivationInput::default()
                },
                0,
                0,
            );
        }
        let late = RETENTION_TICKS * 2;
        let mut delivered = ActivationInput::default();
        edges.deliver(&mut delivered, late, None, None, None, None);
        assert!(edges.take_start(front));
        edges.terminal(front);
        let Some(DueStart::Start(start)) = edges.due_start(6, late) else {
            panic!("the start behind the front is still due");
        };
        assert!(
            edges.take_start(start.token),
            "never settled while retained"
        );
        edges.deliver(&mut delivered, late + 1, None, None, None, None);
        assert_eq!(
            delivered.release.map(|release| release.token),
            Some(behind),
            "its own release still follows it"
        );
        // Nothing retained shields the expired unknown edge now; it settles.
        edges.deliver(&mut delivered, late + 2, None, None, None, None);
        edges.observe_start(unknown, 0, aim(0.0), 0);
        assert!(edges.starts.is_empty());
    }
    #[test]
    fn activation_edge_release_then_cancel_delivers_both_once() {
        let token = token(4);
        let mut edges = ActivationEdges::default();
        edges.admit(token);
        let release = ActivationInput {
            release: Some(ActivationRelease {
                token,
                release_tick: 9,
            }),
            ..ActivationInput::default()
        };
        edges.observe(release, 0, 10);
        let mut delivered = ActivationInput::default();
        edges.deliver(&mut delivered, 10, None, None, None, None);
        assert_eq!(delivered.release, release.release);
        let cancel = ActivationInput {
            cancel: Some(token),
            ..ActivationInput::default()
        };
        edges.observe(cancel, 0, 11);
        edges.deliver(&mut delivered, 11, None, None, None, None);
        assert_eq!(delivered.cancel, Some(token));
        edges.observe(cancel, 0, 12);
        edges.deliver(&mut delivered, 12, None, None, None, None);
        assert_eq!(delivered, ActivationInput::default());
    }
    #[test]
    fn activation_edge_same_token_cancel_suppresses_release_without_redelivery() {
        let token = token(4);
        let input = ActivationInput {
            release: Some(ActivationRelease {
                token,
                release_tick: 9,
            }),
            cancel: Some(token),
            ..ActivationInput::default()
        };
        let mut edges = ActivationEdges::default();
        edges.admit(token);
        edges.observe(input, 0, 0);
        let mut delivered = ActivationInput::default();
        edges.deliver(&mut delivered, 1, None, None, None, None);
        assert_eq!(delivered.cancel, Some(token));
        assert!(delivered.release.is_none());
        edges.observe(input, 0, 2);
        edges.deliver(&mut delivered, 2, None, None, None, None);
        assert_eq!(delivered, ActivationInput::default());
    }
    #[test]
    fn activation_edge_duplicate_does_not_refresh_unknown_expiry() {
        let token = token(4);
        let input = ActivationInput {
            cancel: Some(token),
            ..ActivationInput::default()
        };
        let mut edges = ActivationEdges::default();
        edges.observe(input, 0, 0);
        edges.observe(input, 0, 119);
        edges.admit(token);
        let mut delivered = ActivationInput::default();
        edges.deliver(&mut delivered, 120, None, None, None, None);
        assert!(delivered.cancel.is_none());
    }
    #[test]
    fn activation_edge_overflow_cancels_affected_live_execution() {
        let mut edges = ActivationEdges::default();
        for start in 0..64 {
            edges.observe(
                ActivationInput {
                    cancel: Some(token(start)),
                    ..ActivationInput::default()
                },
                0,
                0,
            );
        }
        let active = token(65);
        edges.admit(token(0));
        edges.admit(active);
        edges.observe(
            ActivationInput {
                release: Some(ActivationRelease {
                    token: active,
                    release_tick: 80,
                }),
                ..ActivationInput::default()
            },
            0,
            1,
        );
        let mut delivered = ActivationInput::default();
        edges.deliver(&mut delivered, 1, None, None, None, None);
        assert_eq!(delivered.cancel, Some(active));
        assert_eq!(edges.edges.len(), 64);
        edges.deliver(&mut delivered, 2, None, None, None, None);
        assert_eq!(delivered.cancel, Some(token(0)));
    }
    #[test]
    fn activation_edge_late_cancel_uses_its_own_first_receipt_expiry() {
        let token = token(4);
        let mut edges = ActivationEdges::default();
        edges.admit(token);
        edges.observe(
            ActivationInput {
                release: Some(ActivationRelease {
                    token,
                    release_tick: 9,
                }),
                ..ActivationInput::default()
            },
            0,
            0,
        );
        let mut delivered = ActivationInput::default();
        edges.deliver(&mut delivered, 1, None, None, None, None);
        edges.observe(
            ActivationInput {
                cancel: Some(token),
                ..ActivationInput::default()
            },
            0,
            119,
        );
        edges.deliver(&mut delivered, 120, None, None, None, None);
        assert_eq!(delivered.cancel, Some(token));
        edges.observe(
            ActivationInput {
                cancel: Some(token),
                ..ActivationInput::default()
            },
            0,
            121,
        );
        edges.deliver(&mut delivered, 121, None, None, None, None);
        assert!(delivered.cancel.is_none());
    }
    #[test]
    fn activation_edge_expired_unknown_cannot_revive_after_duplicate_and_admission() {
        let token = token(4);
        let release = ActivationInput {
            release: Some(ActivationRelease {
                token,
                release_tick: 9,
            }),
            ..ActivationInput::default()
        };
        let mut edges = ActivationEdges::default();
        edges.observe(release, 0, 0);
        let mut delivered = ActivationInput::default();
        edges.deliver(&mut delivered, 120, None, None, None, None);
        edges.observe(release, 0, 121);
        edges.admit(token);
        edges.deliver(&mut delivered, 122, None, None, None, None);
        assert_eq!(delivered, ActivationInput::default());
        assert!(edges.edges.is_empty());
        assert!(edges.admitted.is_empty());
    }
    #[test]
    fn activation_edge_unknown_watermark_preserves_live_release_history_and_later_cancel() {
        let live = token(4);
        let mut edges = ActivationEdges::default();
        edges.admit(live);
        let release = ActivationInput {
            release: Some(ActivationRelease {
                token: live,
                release_tick: 9,
            }),
            ..ActivationInput::default()
        };
        edges.observe(release, 0, 0);
        let mut delivered = ActivationInput::default();
        edges.deliver(&mut delivered, 1, None, None, None, None);
        assert_eq!(delivered.release, release.release);
        edges.observe(
            ActivationInput {
                cancel: Some(token(5)),
                ..ActivationInput::default()
            },
            0,
            0,
        );
        edges.deliver(&mut delivered, 120, None, None, None, None);
        edges.observe(release, 0, 121);
        edges.deliver(&mut delivered, 121, None, None, None, None);
        assert_eq!(delivered, ActivationInput::default());
        edges.observe(
            ActivationInput {
                cancel: Some(live),
                ..ActivationInput::default()
            },
            0,
            122,
        );
        edges.deliver(&mut delivered, 122, None, None, None, None);
        assert_eq!(delivered.cancel, Some(live));
    }

    fn release_at(start: u32, release_tick: u32) -> ActivationInput {
        ActivationInput {
            release: Some(ActivationRelease {
                token: token(start),
                release_tick,
            }),
            ..ActivationInput::default()
        }
    }
    fn spared_slot_zero() -> VaultSparedCharges {
        VaultSparedCharges::default().with(0, ActivationLane::Secondary)
    }

    // A closed gate passes over a charge whose action a vault does not
    // interrupt while it is still charging, live or retained, as the client
    // kept charging it; its release after the vault is delivered. Released
    // before the closed tick, or in a slot the vault does interrupt, it is
    // cancelled there as before, the cancel behind the release.
    #[test]
    fn a_closed_gate_spares_an_unreleased_charge_its_vault_does_not_interrupt() {
        let mut edges = ActivationEdges::default();
        edges.set_vault_spared_charges(spared_slot_zero());
        edges.observe_start(token(6), 0, aim(0.0), 0);
        assert!(edges.take_start(token(6)));
        edges.stamp_fire_gate(8, true);
        edges.cancel_before(8, Some(token(6)), 1);
        assert!(edges.vault_spares_live(token(6), 8));
        edges.observe(release_at(6, 12), 12, 2);
        let mut delivered = ActivationInput::default();
        edges.deliver(&mut delivered, 3, Some(token(6)), Some(12), None, Some(12));
        assert_eq!(
            delivered,
            release_at(6, 12),
            "the release goes, uncancelled"
        );
        edges.deliver(&mut delivered, 4, Some(token(6)), Some(13), None, None);
        assert_eq!(delivered.cancel, None);

        // A retained start the closed tick passes over keeps no cancel either.
        let mut edges = ActivationEdges::default();
        edges.set_vault_spared_charges(spared_slot_zero());
        edges.observe_start(token(6), 0, aim(0.0), 0);
        edges.stamp_fire_gate(8, true);
        edges.cancel_before(8, None, 1);
        assert!(edges.take_start(token(6)));
        edges.deliver(&mut delivered, 2, Some(token(6)), Some(9), None, None);
        assert_eq!(delivered.cancel, None);

        // Released before the vault, or in a slot the vault interrupts.
        for (spared, released) in [
            (spared_slot_zero(), Some(7)),
            (
                VaultSparedCharges::default().with(1, ActivationLane::Secondary),
                None,
            ),
            (
                VaultSparedCharges::default().with(0, ActivationLane::Primary),
                None,
            ),
        ] {
            let mut edges = ActivationEdges::default();
            edges.set_vault_spared_charges(spared);
            edges.observe_start(token(6), 0, aim(0.0), 0);
            assert!(edges.take_start(token(6)));
            if let Some(released) = released {
                edges.observe(release_at(6, released), released, 0);
            }
            edges.stamp_fire_gate(8, true);
            edges.cancel_before(8, Some(token(6)), 1);
            assert!(!edges.vault_spares_live(token(6), 8), "{spared:?}");
            if released.is_some() {
                edges.deliver(&mut delivered, 2, Some(token(6)), Some(7), None, None);
                assert_eq!(delivered.release, release_at(6, 7).release);
                assert_eq!(delivered.cancel, None, "the release goes first");
            }
            edges.deliver(&mut delivered, 3, Some(token(6)), Some(8), None, None);
            assert_eq!(delivered.cancel, Some(token(6)), "{spared:?}");
        }
    }

    // A start or release arriving after a closed tick it missed: a charge the
    // vault spares takes the cancel only when released before that tick,
    // whichever of its start and its release reaches the lane first.
    #[test]
    fn a_late_spared_charge_takes_its_vault_cancel_only_when_released_before_it() {
        // Charged through the vault (10 onward) and released at 14.
        let mut edges = ActivationEdges::default();
        edges.set_vault_spared_charges(spared_slot_zero());
        edges.stamp_fire_gate(10, true);
        edges.stamp_fire_gate(11, true);
        edges.observe_start(token(6), 0, aim(0.0), 0);
        edges.observe(release_at(6, 14), 14, 0);
        assert!(edges.take_start(token(6)));
        let mut delivered = ActivationInput::default();
        edges.deliver(&mut delivered, 1, Some(token(6)), Some(14), None, Some(14));
        assert_eq!(delivered, release_at(6, 14));

        // Released at 8, before the vault: cancelled at 10, behind the release.
        for release_first in [true, false] {
            let mut edges = ActivationEdges::default();
            edges.set_vault_spared_charges(spared_slot_zero());
            edges.stamp_fire_gate(10, true);
            if release_first {
                edges.observe(release_at(6, 8), 8, 0);
                edges.observe_start(token(6), 0, aim(0.0), 0);
            } else {
                edges.observe_start(token(6), 0, aim(0.0), 0);
                edges.observe(release_at(6, 8), 8, 0);
            }
            assert!(edges.take_start(token(6)));
            edges.deliver(&mut delivered, 1, Some(token(6)), Some(8), None, None);
            assert_eq!(
                delivered,
                release_at(6, 8),
                "release first: {release_first}"
            );
            edges.deliver(&mut delivered, 2, Some(token(6)), Some(10), None, None);
            assert_eq!(
                delivered.cancel,
                Some(token(6)),
                "release first: {release_first}"
            );
        }
    }

    // A charge the vault spared releases on its client's first open tick
    // after the vault, so the release reaches the lane a buffer's depth ahead
    // of that tick. It waits until that tick's own resolution and goes there,
    // uncancelled, before that tick's gate is stamped. A release with no vault
    // passed over its charge goes at once, ahead of its tick, as before,
    // whether or not its slot's charge survives a vault.
    #[test]
    fn a_spared_charges_early_release_waits_for_its_own_tick_and_no_other_does() {
        const VAULT: std::ops::Range<u32> = 10..20;
        let mut edges = ActivationEdges::default();
        edges.set_vault_spared_charges(spared_slot_zero());
        edges.observe_start(token(6), 0, aim(0.0), 0);
        assert!(edges.take_start(token(6)));
        // Each resolution delivers, then stamps its tick and closes its gate.
        let resolve =
            |edges: &mut ActivationEdges, resolved: u32, delivered: &mut ActivationInput| {
                edges.deliver(
                    delivered,
                    resolved,
                    Some(token(6)),
                    Some(20),
                    None,
                    Some(resolved),
                );
                let closed = VAULT.contains(&resolved);
                edges.stamp_fire_gate(resolved, closed);
                if closed {
                    edges.cancel_before(resolved, Some(token(6)), resolved);
                }
            };
        let mut delivered = ActivationInput::default();
        for resolved in 6..18 {
            resolve(&mut edges, resolved, &mut delivered);
        }
        edges.observe(release_at(6, 20), 20, 18);
        for resolved in 18..20 {
            resolve(&mut edges, resolved, &mut delivered);
            assert_eq!(
                delivered,
                ActivationInput::default(),
                "held while the cursor is at {resolved}"
            );
        }
        resolve(&mut edges, 20, &mut delivered);
        assert_eq!(
            delivered,
            release_at(6, 20),
            "delivered on its own tick's resolution, uncancelled"
        );
        resolve(&mut edges, 21, &mut delivered);
        assert_eq!(delivered, ActivationInput::default(), "delivered once");

        // No vault: the same early release goes at once, in a slot whose
        // charge survives a vault or one whose charge does not.
        for spared in [spared_slot_zero(), VaultSparedCharges::default()] {
            let mut edges = ActivationEdges::default();
            edges.set_vault_spared_charges(spared);
            edges.observe_start(token(6), 0, aim(0.0), 0);
            assert!(edges.take_start(token(6)));
            for resolved in 6..18 {
                edges.stamp_fire_gate(resolved, false);
            }
            edges.observe(release_at(6, 20), 20, 18);
            edges.deliver(&mut delivered, 18, Some(token(6)), Some(20), None, Some(18));
            assert_eq!(delivered, release_at(6, 20), "{spared:?}: at once");
        }
    }
}
