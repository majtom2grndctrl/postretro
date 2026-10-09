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
        self.activation_edges
            .observe(input, command.client_tick, self.host_tick);
        if let Some(token) = input.initiation {
            self.activation_edges.observe_start(
                token,
                command.movement.firing_slot,
                RemoteStartAim {
                    pitch: command.movement.aim_pitch,
                    yaw: command.movement.facing_yaw,
                },
            );
        }
    }

    /// Deliver one advancing resolution's retained lanes, in the client's order.
    /// A real command carries at most one start, and none while an older press
    /// is still due: that press goes first. Presses stamped after the oldest
    /// retained start wait until it is delivered or refused, and presses stamped
    /// after the live execution's clock wait for it. Returns a start to refuse.
    pub(super) fn deliver_retained(
        &mut self,
        resolved_tick: u32,
        live: Option<ActivationToken>,
        real: bool,
        command: &mut SimCommand,
    ) -> Option<ActivationToken> {
        let mut rejected = None;
        if real {
            let edges = &self.activation_edges;
            self.cadence
                .holster_others(command.firing_slot, self.host_tick, |slot| {
                    edges.has_due_start(slot, resolved_tick)
                });
            command.activation.initiation = None;
            let press_first = self
                .activation_edges
                .oldest_start_tick()
                .zip(self.oldest_due_press(resolved_tick))
                .is_some_and(|(start, press)| !client_tick_le(start, press));
            if !press_first {
                rejected = self.deliver_due_start(resolved_tick, live, command);
            }
        }
        let gate = self.order_gate(live, command.activation.initiation);
        self.deliver_presses(resolved_tick, gate, command);
        rejected
    }

    /// Client ticks through which presses may reach the weapon this resolution.
    /// `delivered`: a start this resolution carries, which owns its firing slot,
    /// so only presses from its own command or earlier ride with it; a later
    /// press, perhaps for the weapon the client switched to, would land on the
    /// start's weapon. Otherwise presses stamped before the oldest retained
    /// start may go. Either way none passes the live execution's clock.
    pub(super) fn order_gate(
        &self,
        live: Option<ActivationToken>,
        delivered: Option<ActivationToken>,
    ) -> PressGate {
        let starts = match delivered {
            Some(token) => PressGate::Through(token.start_tick),
            None => PressGate::through(
                self.activation_edges
                    .oldest_start_tick()
                    .map(|tick| tick.wrapping_sub(1)),
            ),
        };
        starts.and(PressGate::through(self.live_horizon(live)))
    }

    /// Client tick the live execution's clock has reached, for inputs it holds.
    fn live_horizon(&self, live: Option<ActivationToken>) -> Option<u32> {
        self.cadence.order_horizon(live, self.host_tick, |token| {
            self.activation_edges.undelivered_release(token)
        })
    }

    /// Newest client tick received from this client.
    fn newest_observed_tick(&self) -> Option<u32> {
        self.latest_observed_reload.map(|(tick, _)| tick)
    }

    /// The oldest retained start, once its command tick has resolved, no
    /// execution is live, and host time allows it. A start the client half
    /// refuses, or whose claim lags past the catch-up allowance, is refused at
    /// once rather than held: the first even while an execution is live, since
    /// that execution only moves the record later. Returns a start to refuse:
    /// refused, lagging, expired, or settled while retained.
    fn deliver_due_start(
        &mut self,
        resolved_tick: u32,
        live: Option<ActivationToken>,
        command: &mut SimCommand,
    ) -> Option<ActivationToken> {
        let start = match self
            .activation_edges
            .due_start(resolved_tick, self.host_tick)?
        {
            DueStart::Expired(token) => return Some(token),
            DueStart::Start(start) => start,
        };
        let newest = self.newest_observed_tick().unwrap_or(resolved_tick);
        let refused = self
            .cadence
            .client_half_refuses(start.firing_slot, start.token.start_tick)
            || (live.is_none()
                && self.cadence.lags_allowance(
                    start.token,
                    start.firing_slot,
                    self.host_tick,
                    newest,
                ));
        if refused {
            self.activation_edges.refuse_front(start.token);
            return Some(start.token);
        }
        if live.is_some()
            || !self
                .cadence
                .host_time_allows(start.firing_slot, self.host_tick)
        {
            return None;
        }
        if !self.activation_edges.take_start(start.token) {
            return Some(start.token);
        }
        self.cadence
            .begin(start.token, start.firing_slot, self.host_tick, newest);
        command.activation.initiation = Some(start.token);
        command.firing_slot = start.firing_slot;
        None
    }

    /// Deliver retained release/cancel edges. A cancel stamped after the live
    /// execution's clock waits for it. Cadence notes each delivered release,
    /// including one riding with its own start; a charged execution runs its
    /// recovery from it.
    pub(super) fn deliver_edges(
        &mut self,
        activation: &mut ActivationInput,
        live: Option<ActivationToken>,
    ) {
        let horizon = self.live_horizon(live);
        self.activation_edges
            .deliver(activation, self.host_tick, live, horizon);
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
    ///
    /// Call once per host simulation tick. Clients that left since the last
    /// call report their records here too, so a weapon whose client
    /// disconnected is charged like one that was dropped.
    pub fn release_departed_cadence(
        &mut self,
        registry: &EntityRegistry,
        owners: &MovementOwners,
    ) -> Vec<(EntityId, u32)> {
        let mut departed = self.departed.advance();
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
        for &(weapon, owed_ticks) in &departed {
            self.departed.depart(weapon, owed_ticks);
        }
        departed
    }

    /// The host machine admitted `token`'s execution. `charged`: its shots wait
    /// for the client's release, so presses do not wait on its clock until then.
    pub fn activation_admitted(&mut self, client_id: u64, token: ActivationToken, charged: bool) {
        if let Some(state) = self.clients.get_mut(&client_id) {
            state.cadence.admitted(token, charged);
        }
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
            let cool_ticks = self.departed.cool_ticks(weapon);
            state.cadence.recovery_began(
                token,
                weapon,
                recovery_ticks,
                charged,
                host_tick,
                cool_ticks,
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::super::{HostCommandQueues, MovementOwners, ResolvedCommand, neutral_sim_command};
    use postretro_entities::{EntityId, EntityRegistry};
    use postretro_foundation::{
        ActivationId, ActivationLane, ActivationProgram, ActivationStep, ActivationToken,
    };
    use postretro_net::wire::{InputCommand, WireActivationToken};

    const CLIENT: u64 = 7;
    /// 600 ms: a slow press weapon's recovery.
    const SLOW: u32 = 36;

    fn token(start_tick: u32) -> ActivationToken {
        ActivationToken {
            start_tick,
            lane: ActivationLane::Primary,
        }
    }

    /// A command at `tick` naming `slot`, carrying a start when `starts`.
    fn command(tick: u32, slot: u8, starts: bool) -> InputCommand {
        let mut command = crate::netcode::wire_convert::sim_command_to_input(
            &neutral_sim_command(0.0),
            tick,
            0.0,
        );
        command.movement.firing_slot = slot;
        if starts {
            command.activation.initiation = Some(WireActivationToken {
                start_tick: tick,
                lane: 0,
            });
        }
        command
    }

    /// Ingest `stream(tick)` one command per resolution from tick 2, after a
    /// two-command bootstrap; each resolution resolves the command one tick
    /// behind the newest, the playout margin.
    fn lockstep(
        queues: &mut HostCommandQueues,
        ticks: std::ops::Range<u32>,
        stream: impl Fn(u32) -> InputCommand,
        mut after: impl FnMut(&mut HostCommandQueues, &ResolvedCommand),
    ) -> Vec<ResolvedCommand> {
        if ticks.start == 2 {
            for tick in 0..2 {
                assert!(queues.ingest(CLIENT, &stream(tick)));
            }
        }
        ticks
            .map(|tick| {
                assert!(queues.ingest(CLIENT, &stream(tick)));
                let resolved = queues.resolve_tick(CLIENT).unwrap();
                after(queues, &resolved);
                resolved
            })
            .collect()
    }

    fn delivered(resolved: &[ResolvedCommand], start: u32) -> Option<&ResolvedCommand> {
        resolved
            .iter()
            .find(|resolved| resolved.command.activation.initiation == Some(token(start)))
    }

    /// The host machine began each delivered start's recovery on its own tick.
    fn fire_slow(queues: &mut HostCommandQueues, resolved: &ResolvedCommand) {
        if let Some(start) = resolved.command.activation.initiation {
            let weapon = EntityId::from_raw(9 + u32::from(resolved.command.firing_slot));
            queues.activation_recovery_began(CLIENT, start, weapon, SLOW, false);
        }
    }

    // Regression: a second tap inside the weapon's own recovery, which the
    // client's prediction refuses but still names, waited at the lane front on
    // host time for most of the recovery before its refusal. Every later start,
    // including another weapon's after a switch, and every later press waited
    // behind it.
    #[test]
    fn lane_refuses_a_start_inside_its_weapons_recovery_at_once_and_holds_nothing_behind_it() {
        let stream = |tick: u32| {
            let slot = u8::from(tick >= 7);
            let mut command = command(tick, slot, matches!(tick, 2 | 5 | 8));
            command.reload = tick == 9;
            command
        };
        let mut queues = HostCommandQueues::new();
        let resolved = lockstep(&mut queues, 2..14, stream, fire_slow);
        let refused = resolved
            .iter()
            .find(|resolved| resolved.rejected_activation == Some(token(5)))
            .expect("the tap inside recovery is refused by the lane");
        assert_eq!(refused.client_tick, 5, "refused on its own tick, not held");
        assert!(refused.command.activation.initiation.is_none());
        let switched = delivered(&resolved, 8).expect("the other weapon's start fires");
        assert_eq!(switched.client_tick, 8, "never held behind the refused tap");
        assert_eq!(switched.command.firing_slot, 1);
        let reload = resolved
            .iter()
            .find(|resolved| resolved.command.reload)
            .expect("the reload press is delivered");
        assert_eq!(reload.client_tick, 9);
        assert_eq!(reload.command.firing_slot, 1);
    }

    #[test]
    fn lane_refuses_a_start_inside_its_weapons_recovery_while_an_execution_is_live() {
        let program = ActivationProgram::new(
            vec![
                ActivationStep::Shot,
                ActivationStep::Wait { ticks: 20 },
                ActivationStep::Shot,
            ],
            None,
            SLOW,
        )
        .unwrap();
        let mut queues = HostCommandQueues::new();
        let resolved = lockstep(
            &mut queues,
            2..8,
            |tick| command(tick, 0, matches!(tick, 2 | 4)),
            |queues, resolved| {
                if let Some(start) = resolved.command.activation.initiation {
                    let id = ActivationId {
                        pawn: 1,
                        token: start,
                    };
                    let weapon = EntityId::from_raw(9);
                    assert!(queues.activations.accept(CLIENT, id, weapon, &program, 0));
                    queues.activation_admitted(CLIENT, start, false);
                    queues.activation_recovery_began(CLIENT, start, weapon, SLOW, false);
                }
            },
        );
        assert!(queues.activations.live_binding(CLIENT).is_some());
        let refused = resolved
            .iter()
            .find(|resolved| resolved.rejected_activation == Some(token(4)))
            .expect("refused while the burst is still live");
        assert_eq!(refused.client_tick, 4);
    }

    // Regression: a disconnecting client's cadence records were dropped without
    // charging their owed recovery to the weapons it left behind.
    #[test]
    fn removed_clients_records_charge_their_owed_recovery_on_the_next_tick() {
        let mut queues = HostCommandQueues::new();
        lockstep(
            &mut queues,
            2..6,
            |tick| command(tick, 0, tick == 2),
            fire_slow,
        );
        queues.remove_client(CLIENT);
        let charged =
            queues.release_departed_cadence(&EntityRegistry::new(), &MovementOwners::new());
        let [(weapon, owed)] = charged.as_slice() else {
            panic!("the departed client's record is charged: {charged:?}");
        };
        assert_eq!(*weapon, EntityId::from_raw(9));
        assert!(
            (SLOW - 4..=SLOW).contains(owed),
            "most of the recovery is still owed: {owed}"
        );
        assert!(
            queues
                .release_departed_cadence(&EntityRegistry::new(), &MovementOwners::new())
                .is_empty(),
            "charged once"
        );
        assert_eq!(
            queues.departed.cool_ticks(*weapon),
            0,
            "its chain still owes"
        );
    }
}
