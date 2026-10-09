// Retained activation starts and their client-domain cadence beside host playout.
// See: context/lib/networking.md §Combat authority · §Host input command queue

use super::activation_cadence::CadenceVerdict;
use super::{ClientCommandState, HostCommandQueues};
use crate::activation_edges::DueStart;
use crate::sim::SimCommand;
use postretro_entities::EntityId;
use postretro_foundation::{ActivationInput, ActivationToken};
use postretro_net::wire::InputCommand;

impl ClientCommandState {
    /// Intake retains edges and starts before stale-drop or catch-up trim.
    pub(super) fn observe_activation(&mut self, command: &InputCommand) {
        let input = crate::wire_convert::activation_input_from_wire(command.activation);
        self.activation_edges.observe(input, self.host_tick);
        if let Some(token) = input.initiation {
            self.activation_edges.observe_start(
                token,
                command.client_tick,
                command.movement.firing_slot,
            );
        }
    }

    /// A real resolution carries at most one retained start: the oldest, once its
    /// command tick has resolved, no execution is live, and host time allows it.
    /// The command's own start rides the same lane, so it is cleared here.
    /// Returns a start to refuse: expired, or settled while retained.
    pub(super) fn deliver_due_start(
        &mut self,
        resolved_tick: u32,
        live: Option<ActivationToken>,
        command: &mut SimCommand,
    ) -> Option<ActivationToken> {
        command.activation.initiation = None;
        match self
            .activation_edges
            .due_start(resolved_tick, self.host_tick, live.is_some())?
        {
            DueStart::Expired(token) => Some(token),
            DueStart::Start(start) => {
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
