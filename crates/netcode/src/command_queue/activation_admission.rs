// Retained activation starts and their client-domain cadence beside host playout.
// See: context/lib/networking.md §Combat authority · §Host input command queue

use super::activation_cadence::{self, CadenceVerdict};
use super::press_edges::PressGate;
use super::{ClientCommandState, HostCommandQueues, MovementOwners};
use crate::activation_edges::DueStart;
use crate::prediction::client_tick_le;
use crate::sim::{RemoteStartAim, SimCommand};
use postretro_entities::components::inventory::Inventory;
use postretro_entities::{EntityId, EntityRegistry};
use postretro_foundation::{ActivationInput, ActivationToken};
use postretro_net::wire::InputCommand;

impl ClientCommandState {
    /// Intake retains edges and starts before stale-drop or catch-up trim. A
    /// start keeps the aim of the sanitized command that carried it.
    pub(super) fn observe_activation(&mut self, command: &InputCommand) {
        let input = crate::wire_convert::activation_input_from_wire(command.activation);
        self.activation_edges.observe(input, self.host_tick);
        if let Some(token) = input.initiation {
            self.activation_edges.observe_start(
                token,
                command.client_tick,
                command.movement.firing_slot,
                RemoteStartAim {
                    pitch: command.movement.aim_pitch,
                    yaw: command.movement.facing_yaw,
                },
            );
        }
    }

    /// Deliver one advancing resolution's retained lanes, in the client's order.
    /// A real command carries at most one start, unless a press stamped before
    /// it is still due: that press goes first. Presses stamped after the oldest
    /// retained start wait until it is delivered or refused. Returns a start to
    /// refuse.
    pub(super) fn deliver_retained(
        &mut self,
        resolved_tick: u32,
        live: Option<ActivationToken>,
        real: bool,
        command: &mut SimCommand,
    ) -> Option<ActivationToken> {
        let front = self.activation_edges.oldest_start_tick();
        let mut rejected = None;
        if real {
            let edges = &self.activation_edges;
            self.cadence
                .holster_others(command.firing_slot, self.host_tick, |slot| {
                    edges.has_due_start(slot, resolved_tick)
                });
            command.activation.initiation = None;
            let press_first = front
                .zip(self.oldest_due_press(resolved_tick))
                .is_some_and(|(start, press)| !client_tick_le(start, press));
            if !press_first {
                rejected = self.deliver_due_start(resolved_tick, live, command);
            }
        }
        // A delivered start owns this tick's firing slot, so only presses from
        // its own command or earlier ride with it. A later press, perhaps for
        // the weapon the client switched to, would land on the start's weapon.
        let gate = match command.activation.initiation {
            Some(_) => front.map_or(PressGate::Open, PressGate::Through),
            None => self
                .activation_edges
                .oldest_start_tick()
                .map_or(PressGate::Open, |tick| {
                    PressGate::Through(tick.wrapping_sub(1))
                }),
        };
        self.preserve_due_reload_press(resolved_tick, gate, command);
        self.press_edges.deliver(resolved_tick, gate, command);
        rejected
    }

    /// Oldest reload, use, or drop press whose tick has resolved.
    fn oldest_due_press(&self, resolved_tick: u32) -> Option<u32> {
        self.pending_reload_presses
            .front()
            .copied()
            .filter(|tick| client_tick_le(*tick, resolved_tick))
            .into_iter()
            .chain(self.press_edges.oldest_due(resolved_tick))
            .reduce(|oldest, tick| {
                if client_tick_le(tick, oldest) {
                    tick
                } else {
                    oldest
                }
            })
    }

    /// The oldest retained start, once its command tick has resolved, no
    /// execution is live, and host time allows it. A start whose claim lags
    /// past the catch-up allowance is refused at once rather than delivered
    /// late. Returns a start to refuse: lagging, expired, or settled while
    /// retained.
    fn deliver_due_start(
        &mut self,
        resolved_tick: u32,
        live: Option<ActivationToken>,
        command: &mut SimCommand,
    ) -> Option<ActivationToken> {
        match self
            .activation_edges
            .due_start(resolved_tick, self.host_tick, live.is_some())?
        {
            DueStart::Expired(token) => Some(token),
            DueStart::Start(start) => {
                let newest = self
                    .latest_observed_reload
                    .map_or(resolved_tick, |(tick, _)| tick);
                if self.cadence.lags_allowance(
                    start.token,
                    start.firing_slot,
                    self.host_tick,
                    newest,
                ) {
                    self.activation_edges.refuse_front(start.token);
                    return Some(start.token);
                }
                if !self
                    .cadence
                    .host_time_allows(start.firing_slot, self.host_tick)
                {
                    return None;
                }
                if !self.activation_edges.take_start(start.token) {
                    return Some(start.token);
                }
                self.cadence
                    .begin(start.token, start.firing_slot, self.host_tick);
                command.activation.initiation = Some(start.token);
                command.firing_slot = start.firing_slot;
                None
            }
        }
    }

    /// Deliver retained release/cancel edges. Cadence notes each delivered
    /// release, including one riding with its own start; a charged execution
    /// runs its recovery from it.
    pub(super) fn deliver_edges(
        &mut self,
        activation: &mut ActivationInput,
        live: Option<ActivationToken>,
    ) {
        self.activation_edges
            .deliver(activation, self.host_tick, live);
        if let Some(release) = activation.release {
            self.cadence
                .release(release.token, release.release_tick, self.host_tick);
        }
    }
}

impl HostCommandQueues {
    /// Host ticks a start's claim may trail host now and still be credited;
    /// past it, a start stuck behind newer input is refused.
    pub const CATCH_UP_ALLOWANCE_TICKS: u32 = activation_cadence::CATCH_UP_ALLOWANCE_TICKS;

    /// Host ticks a due start waits in its lane before it expires.
    pub const START_RETENTION_TICKS: u32 = crate::activation_edges::RETENTION_TICKS;

    /// Client half of the cadence rule for a start about to bind `weapon` in
    /// `firing_slot`.
    pub fn activation_cadence(
        &self,
        client_id: u64,
        weapon: EntityId,
        firing_slot: u8,
        start_tick: u32,
    ) -> CadenceVerdict {
        self.clients
            .get(&client_id)
            .map_or(CadenceVerdict::Unrecorded, |state| {
                state
                    .cadence
                    .client_spacing(weapon, firing_slot, start_tick)
            })
    }

    /// Most executions one weapon can admit in any `window` host ticks, the
    /// bound the cadence rule holds whatever client ticks are stamped.
    pub fn cadence_window_bound(window: u32, recovery_ticks: u32) -> u32 {
        activation_cadence::window_bound(window, recovery_ticks)
    }

    /// Firing slot `token`'s start named, once the lane refused it. Its refusal
    /// reports that weapon's recovery, not the delivering command's.
    pub fn refused_start_slot(&self, client_id: u64, token: ActivationToken) -> Option<u8> {
        self.clients
            .get(&client_id)
            .and_then(|state| state.activation_edges.refused_slot(token))
    }

    /// Aim of the command that carried `token`'s start, once playout delivered
    /// it. Host FIRE reconstructs that start's shot along it.
    pub fn start_aim(&self, client_id: u64, token: ActivationToken) -> Option<RemoteStartAim> {
        self.clients
            .get(&client_id)
            .and_then(|state| state.activation_edges.delivered_aim(token))
    }

    /// After the simulation tick, clear each client's cadence record for every
    /// weapon that no longer holds the slot it fired from in that client's
    /// inventory: dropped, handed to another pawn, despawned, or its pawn gone.
    /// Returns each departed weapon with the host ticks its credited recovery
    /// still owes, for the caller to charge to that weapon's own cooldown.
    pub fn release_departed_cadence(
        &mut self,
        registry: &EntityRegistry,
        owners: &MovementOwners,
    ) -> Vec<(EntityId, u32)> {
        let mut departed = Vec::new();
        for (client_id, state) in &mut self.clients {
            if !state.cadence.has_records() {
                continue;
            }
            let inventory = owners
                .iter()
                .find(|(_, owner)| owner == client_id)
                .and_then(|(pawn, _)| registry.get_component::<Inventory>(pawn).ok());
            let held = |slot: usize| {
                inventory.and_then(|inventory| inventory.wieldables.get(slot).copied().flatten())
            };
            departed.extend(state.cadence.release_departed(held, state.host_tick));
        }
        departed
    }

    /// The host machine began `token`'s recovery this host tick. `charged`: the
    /// action charges, so its shots began at the delivered release.
    pub fn activation_recovery_began(
        &mut self,
        client_id: u64,
        token: ActivationToken,
        weapon: EntityId,
        recovery_ticks: u32,
        charged: bool,
    ) {
        if let Some(state) = self.clients.get_mut(&client_id) {
            let host_tick = state.host_tick;
            state
                .cadence
                .recovery_began(token, weapon, recovery_ticks, charged, host_tick);
        }
    }
}
