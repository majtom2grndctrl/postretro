// Retained activation starts and their client-domain cadence beside host playout.
// See: context/lib/networking.md §Combat authority · §Host input command queue

use super::activation_cadence::{self, CadenceVerdict};
use super::press_edges::PressGate;
use super::{ClientCommandState, HostCommandQueues, MovementOwners};
use crate::activation_edges::{DueStart, LaneRefusal};
use crate::prediction::client_tick_le;
use crate::sim::{RemoteStartAim, SimCommand};
use postretro_entities::components::inventory::Inventory;
use postretro_entities::{EntityId, EntityRegistry};
use postretro_foundation::{ActivationInput, ActivationToken};
use postretro_net::wire::InputCommand;
use postretro_sim::weapon::pawn_gate::VaultSparedCharges;

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
                self.host_tick,
            );
        }
    }

    /// Deliver one advancing resolution's retained lanes, in the client's order.
    /// A real command carries at most one start, and none while an older press
    /// is still due: that press goes first. Presses stamped after the oldest
    /// retained start wait until it is delivered or refused, and presses stamped
    /// after the live execution's clock wait for it. Starts and presses stamped
    /// on or after a retained switch's command wait for that switch. Returns a
    /// start to refuse.
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
    /// start may go. Either way none passes the live execution's clock, nor a
    /// retained switch stamped at or before it.
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
        starts
            .and(PressGate::through(self.live_horizon(live)))
            .and(self.switch_gate())
    }

    /// Client tick the live execution's clock has reached, for inputs it holds.
    pub(super) fn live_horizon(&self, live: Option<ActivationToken>) -> Option<u32> {
        self.cadence.order_horizon(live, self.host_tick, |token| {
            self.activation_edges.undelivered_release(token)
        })
    }

    /// Newest client tick received from this client.
    pub(super) fn newest_observed_tick(&self) -> Option<u32> {
        self.latest_observed_reload.map(|(tick, _)| tick)
    }

    /// The oldest retained start, once its command tick has resolved, no
    /// execution is live, and host time allows it. A start the client half
    /// refuses, or whose claim lags past the catch-up allowance, is refused at
    /// once rather than held: the first even while an execution is live, since
    /// that execution only moves the record later. A start stamped on or after
    /// a retained switch's command waits for it untouched, its expiry not yet
    /// running.
    /// Returns a start to refuse: refused, lagging, expired, or settled while
    /// retained.
    fn deliver_due_start(
        &mut self,
        resolved_tick: u32,
        live: Option<ActivationToken>,
        command: &mut SimCommand,
    ) -> Option<ActivationToken> {
        if self
            .activation_edges
            .oldest_start_tick()
            .is_some_and(|tick| !self.switch_gate().admits(tick))
        {
            return None;
        }
        let start = match self
            .activation_edges
            .due_start(resolved_tick, self.host_tick)?
        {
            DueStart::Expired(token) => return Some(token),
            DueStart::Start(start) => start,
        };
        let newest = self.newest_observed_tick().unwrap_or(resolved_tick);
        let refused = if self
            .cadence
            .client_half_refuses(start.firing_slot, start.token.start_tick)
        {
            Some(LaneRefusal::ClientHalf)
        } else if live.is_none()
            && self
                .cadence
                .lags_allowance(start.token, start.firing_slot, self.host_tick, newest)
        {
            Some(LaneRefusal::Lagged)
        } else {
            None
        };
        if let Some(reason) = refused {
            self.activation_edges.refuse_front(start.token, reason);
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
    /// execution's clock waits for it. A release of a charge a vault spared
    /// waits for the resolution of its own client tick, read from the cursor
    /// this resolution already set; every other release goes at once.
    /// Cadence notes each delivered release, including one riding with its own
    /// start; a charged execution runs its recovery from it.
    pub(super) fn deliver_edges(
        &mut self,
        activation: &mut ActivationInput,
        live: Option<ActivationToken>,
    ) {
        let horizon = self.live_horizon(live);
        let spent_release = self.cadence.spent_release();
        self.activation_edges.deliver(
            activation,
            self.host_tick,
            live,
            horizon,
            spent_release,
            self.resolved_cursor,
        );
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

    /// Why the lane refused `token`'s start, once it has. Only a lagging
    /// refusal is one the catch-up allowance mandates.
    pub fn lane_refusal(&self, client_id: u64, token: ActivationToken) -> Option<LaneRefusal> {
        self.clients
            .get(&client_id)
            .and_then(|state| state.activation_edges.refusal(token))
    }

    /// Record the shooter's fire gate at the start of `resolved_tick`, and stamp
    /// `client_id`'s starts due by it with the gate of each one's own tick
    /// (`ActivationEdges::stamp_fire_gate`). Returns the gate recorded for
    /// `resolved_tick`: the first resolution's, when a frontier hold resolves
    /// it again. The caller judges the tick by it.
    ///
    /// `spared`: the shooter's firing-slot actions whose charge a vault does
    /// not interrupt, read from its inventory now. A closed gate passes over a
    /// start or execution of one of them still charging, as the client's did.
    pub fn stamp_start_fire_gate(
        &mut self,
        client_id: u64,
        resolved_tick: u32,
        blocked: bool,
        spared: VaultSparedCharges,
    ) -> bool {
        match self.clients.get_mut(&client_id) {
            Some(state) => {
                state.activation_edges.set_vault_spared_charges(spared);
                state
                    .activation_edges
                    .stamp_fire_gate(resolved_tick, blocked)
            }
            None => blocked,
        }
    }

    /// `client_id`'s pawn starts client tick `tick` with its fire gate closed,
    /// such as a vault with fire blocked. Its client cancelled whatever it was
    /// executing on that tick, except a charge whose action a vault does not
    /// interrupt and that it had not released before `tick`, which it kept
    /// charging through the vault and whose release it held past it. It sends
    /// no cancel for what it stopped, so every other execution it began before
    /// `tick` stops there as if it had: the live one, a start delivered this
    /// resolution, and each retained start stamped earlier carry a cancel
    /// stamped `tick` (`ActivationEdges::cancel_before`). The lane delivers
    /// each once its execution's clock reaches `tick`, so a late execution
    /// mints every shot the client fired before the vault and none after,
    /// wherever the vault stands when it is delivered. The spared charge
    /// takes no cancel, and its release waits in the lane for its own tick's
    /// resolution (`ActivationEdges::deliver`).
    ///
    /// Returns whether the live execution's clock has already reached `tick`.
    /// This resolution's edges are already delivered, so the caller cancels
    /// that execution itself, before this tick's simulation. False for a
    /// spared live charge.
    pub fn close_start_fire_gate(&mut self, client_id: u64, tick: u32) -> bool {
        let live = self
            .activations
            .live_binding(client_id)
            .map(|(id, _)| id.token);
        let Some(state) = self.clients.get_mut(&client_id) else {
            return false;
        };
        state
            .activation_edges
            .cancel_before(tick, live, state.host_tick);
        if live.is_some_and(|live| state.activation_edges.vault_spares_live(live, tick)) {
            return false;
        }
        // No horizon: nothing holds a cancel, as an unreleased charge holds
        // none. A charge released before `tick` holds it at its release tick,
        // so its shot fires before the cancel lands.
        live.is_some()
            && state
                .live_horizon(live)
                .is_none_or(|horizon| client_tick_le(tick, horizon))
    }

    /// The fire gate stamped on `token`'s delivered start, if it was stamped.
    pub fn start_fire_blocked(&self, client_id: u64, token: ActivationToken) -> Option<bool> {
        self.clients
            .get(&client_id)
            .and_then(|state| state.activation_edges.delivered_fire_blocked(token))
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
    use postretro_net::wire::{InputCommand, WireActivationRelease, WireActivationToken};
    use postretro_sim::weapon::pawn_gate::VaultSparedCharges;

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
        let mut command =
            crate::netcode::wire_convert::sim_command_to_input(&neutral_sim_command(0.0), tick);
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

    /// The host machine admitted `start`: it binds the ledger and is live.
    fn accept_live(queues: &mut HostCommandQueues, start: ActivationToken, charged: bool) {
        let program = ActivationProgram::new(vec![ActivationStep::Shot], None, SLOW).unwrap();
        let id = ActivationId {
            pawn: 1,
            token: start,
        };
        assert!(
            queues
                .activations
                .accept(CLIENT, id, EntityId::from_raw(9), &program, 0)
        );
        queues.activation_admitted(CLIENT, start, charged);
    }

    // Regression: a release delivered on the resolution whose tick first
    // started vaulting left the unfired charge with no horizon, so the host
    // cancelled it before this tick's simulation fired the charged shot the
    // client fired before its vault.
    #[test]
    fn a_vault_after_a_charges_release_lets_the_charged_shot_fire_first() {
        let start = token(2);
        let vault = 7;
        let mut closed_at_once = None;
        let resolved = lockstep(
            &mut HostCommandQueues::new(),
            2..12,
            |tick| {
                let mut command = command(tick, 0, tick == start.start_tick);
                if tick == 5 {
                    command.activation.release = Some(WireActivationRelease {
                        token: wire_token(start),
                        release_tick: 5,
                    });
                }
                command
            },
            |queues, resolved| {
                if resolved.command.activation.initiation == Some(start) {
                    accept_live(queues, start, true);
                }
                if resolved.command.activation.release.is_some() {
                    closed_at_once = Some(queues.close_start_fire_gate(CLIENT, vault));
                    // The machine fires the charge at its release this tick.
                    queues.activation_recovery_began(
                        CLIENT,
                        start,
                        EntityId::from_raw(9),
                        SLOW,
                        true,
                    );
                }
            },
        );
        assert_eq!(
            closed_at_once,
            Some(false),
            "the charge's clock sits at its release, before the vault"
        );
        let released = resolved
            .iter()
            .position(|resolved| resolved.command.activation.release.is_some())
            .expect("the release is delivered");
        let cancelled = resolved
            .iter()
            .position(|resolved| resolved.command.activation.cancel == Some(start))
            .expect("the vault cancels the charge's execution");
        assert_eq!(
            cancelled - released,
            2,
            "cancelled once the clock runs from the release (5) to the vault (7)"
        );
    }

    // Regression: a start whose command playout yielded past reached the lane
    // after a vault began, and carried no cancel for it until the next closed
    // resolution, so the host minted rounds the client never fired.
    #[test]
    fn a_start_arriving_after_a_vault_began_is_cancelled_where_it_began() {
        let start = token(2);
        let mut queues = HostCommandQueues::new();
        let resolve = |queues: &mut HostCommandQueues, blocked: bool| {
            let resolved = queues.resolve_tick(CLIENT).unwrap();
            if queues.stamp_start_fire_gate(
                CLIENT,
                resolved.client_tick,
                blocked,
                VaultSparedCharges::default(),
            ) {
                queues.close_start_fire_gate(CLIENT, resolved.client_tick);
            }
            if resolved.command.activation.initiation == Some(start) {
                accept_live(queues, start, false);
            }
            resolved
        };
        for tick in 0..2 {
            assert!(queues.ingest(CLIENT, &command(tick, 0, false)));
        }
        resolve(&mut queues, false);
        resolve(&mut queues, false);
        // Tick 2 is lost; tick 3's arrival makes playout yield past it.
        assert!(queues.ingest(CLIENT, &command(3, 0, false)));
        assert_eq!(resolve(&mut queues, false).client_tick, 2);
        assert!(queues.ingest(CLIENT, &command(4, 0, false)));
        assert_eq!(
            resolve(&mut queues, true).client_tick,
            3,
            "the pawn starts tick 3 vaulting"
        );
        // A later copy of tick 2 is stale, but its start is retained.
        assert!(!queues.ingest(CLIENT, &command(2, 0, true)));
        assert!(queues.ingest(CLIENT, &command(5, 0, false)));
        let delivered = resolve(&mut queues, true);
        assert_eq!(delivered.command.activation.initiation, Some(start));
        assert_eq!(queues.start_fire_blocked(CLIENT, start), Some(false));
        assert!(queues.ingest(CLIENT, &command(6, 0, false)));
        assert_eq!(
            resolve(&mut queues, true).command.activation.cancel,
            Some(start),
            "cancelled as its clock reaches 3, where the vault began"
        );
    }

    fn wire_token(start: ActivationToken) -> WireActivationToken {
        WireActivationToken {
            start_tick: start.start_tick,
            lane: 0,
        }
    }

    // Regression: a charged start reaching the lane after its tick resolved
    // took its missed vault cancel only when stamped, after the resolution
    // that admitted it had already sent a release the client stamped on the
    // vault's first tick. The client's gate cancelled the charge before that
    // release, so the host fired a charged shot the client never fired.
    #[test]
    fn a_late_charged_start_never_fires_a_release_its_clients_vault_cancelled() {
        let start = token(2);
        let mut queues = HostCommandQueues::new();
        let resolve = |queues: &mut HostCommandQueues, blocked: bool| {
            let resolved = queues.resolve_tick(CLIENT).unwrap();
            if queues.stamp_start_fire_gate(
                CLIENT,
                resolved.client_tick,
                blocked,
                VaultSparedCharges::default(),
            ) {
                queues.close_start_fire_gate(CLIENT, resolved.client_tick);
            }
            if resolved.command.activation.initiation == Some(start) {
                accept_live(queues, start, true);
            }
            resolved
        };
        let mut resolutions = Vec::new();
        for tick in 0..2 {
            assert!(queues.ingest(CLIENT, &command(tick, 0, false)));
        }
        resolutions.push(resolve(&mut queues, false));
        resolutions.push(resolve(&mut queues, false));
        // Tick 2 is lost; tick 3's arrival makes playout yield past it.
        assert!(queues.ingest(CLIENT, &command(3, 0, false)));
        resolutions.push(resolve(&mut queues, false));
        // The client releases on tick 4, the vault's first tick; its gate
        // cancels the charge before the release.
        let mut released = command(4, 0, false);
        released.activation.release = Some(WireActivationRelease {
            token: wire_token(start),
            release_tick: 4,
        });
        assert!(queues.ingest(CLIENT, &released));
        resolutions.push(resolve(&mut queues, false));
        assert!(queues.ingest(CLIENT, &command(5, 0, false)));
        let closed = resolve(&mut queues, true);
        assert_eq!(closed.client_tick, 4, "the pawn starts tick 4 vaulting");
        resolutions.push(closed);
        // A later copy of tick 2 is stale, but its start is retained.
        assert!(!queues.ingest(CLIENT, &command(2, 0, true)));
        for tick in 6..12 {
            assert!(queues.ingest(CLIENT, &command(tick, 0, false)));
            resolutions.push(resolve(&mut queues, true));
        }
        assert!(
            resolutions
                .iter()
                .any(|resolved| resolved.command.activation.initiation == Some(start)),
            "the late start is delivered"
        );
        assert!(
            resolutions
                .iter()
                .all(|resolved| resolved.command.activation.release.is_none()),
            "the release the vault cancelled never reaches the machine"
        );
        assert!(
            resolutions
                .iter()
                .any(|resolved| resolved.command.activation.cancel == Some(start)),
            "the charge is cancelled"
        );
    }

    // Regression: a cancel for an execution that does not charge waited behind
    // a release the client sent before it, which the machine ignores, so the
    // burst minted a shot after the client's cancel.
    #[test]
    fn a_cancel_never_waits_behind_a_release_an_uncharged_execution_ignores() {
        let start = token(2);
        let mut queues = HostCommandQueues::new();
        for tick in 0..3 {
            assert!(queues.ingest(CLIENT, &command(tick, 0, tick == start.start_tick)));
        }
        queues.resolve_tick(CLIENT).unwrap();
        queues.resolve_tick(CLIENT).unwrap();
        let delivered = queues.resolve_tick(CLIENT).unwrap();
        assert_eq!(delivered.command.activation.initiation, Some(start));
        accept_live(&mut queues, start, false);
        // Input stalls at the frontier while the burst's clock runs on to 5.
        for _ in 0..2 {
            let frozen = queues.resolve_tick(CLIENT).unwrap();
            assert_eq!(frozen.source, super::super::ResolutionSource::Held);
        }
        let mut released = command(3, 0, false);
        released.activation.release = Some(WireActivationRelease {
            token: wire_token(start),
            release_tick: 3,
        });
        let mut cancelled = command(4, 0, false);
        cancelled.activation.cancel = Some(wire_token(start));
        assert!(queues.ingest(CLIENT, &released));
        assert!(queues.ingest(CLIENT, &cancelled));
        assert_eq!(
            queues
                .resolve_tick(CLIENT)
                .unwrap()
                .command
                .activation
                .cancel,
            Some(start),
            "the cancel stamped 4 goes as soon as the clock passes it"
        );
    }

    // A live charge whose action a vault does not interrupt keeps charging
    // through its client's vault: no closed tick cancels it, at once or
    // later, and its release after the vault reaches the machine on the
    // resolution of its own tick. Without the opt-out the vault cancels it
    // and the release never goes.
    #[test]
    fn a_charge_the_vault_spares_reaches_its_release_after_the_vault() {
        let start = token(2);
        let vault = 5..9;
        for spared in [true, false] {
            let charges = if spared {
                VaultSparedCharges::default().with(0, ActivationLane::Primary)
            } else {
                VaultSparedCharges::default()
            };
            let mut closed_at_once = Vec::new();
            let resolved = lockstep(
                &mut HostCommandQueues::new(),
                2..16,
                |tick| {
                    let mut command = command(tick, 0, tick == start.start_tick);
                    if tick == 10 {
                        command.activation.release = Some(WireActivationRelease {
                            token: wire_token(start),
                            release_tick: 10,
                        });
                    }
                    command
                },
                |queues, resolved| {
                    let blocked = vault.contains(&resolved.client_tick);
                    if queues.stamp_start_fire_gate(CLIENT, resolved.client_tick, blocked, charges)
                    {
                        closed_at_once
                            .push(queues.close_start_fire_gate(CLIENT, resolved.client_tick));
                    }
                    if resolved.command.activation.initiation == Some(start) {
                        accept_live(queues, start, true);
                    }
                },
            );
            let released_on =
                resolved
                    .iter()
                    .find(|resolved| {
                        resolved.command.activation.release.is_some_and(|release| {
                            release.token == start && release.release_tick == 10
                        })
                    })
                    .map(|resolved| resolved.client_tick);
            let released = released_on.is_some();
            if spared {
                // Received two ticks ahead of the cursor, it waits for its
                // own tick, so it fires on the client's tick and aim.
                assert_eq!(released_on, Some(10), "rides its own tick's resolution");
            }
            let cancelled = resolved
                .iter()
                .any(|resolved| resolved.command.activation.cancel == Some(start));
            assert_eq!(released, spared, "spared: {spared}");
            assert_eq!(cancelled, !spared, "spared: {spared}");
            assert_eq!(
                closed_at_once.iter().any(|&at_once| at_once),
                !spared,
                "spared: {spared}"
            );
        }
    }
}
