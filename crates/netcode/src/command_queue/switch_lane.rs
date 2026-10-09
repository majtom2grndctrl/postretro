// Weapon-switch declarations retained beside command playout and delivered in
// the client's tick order with its retained starts and presses.
// See: context/lib/networking.md §Host input command queue · §Combat authority
//
// A switch rides reliable Control while starts and presses ride reliable Input,
// so after a stall a switch applied on arrival overtook them: it cancelled a
// burst or charge the client had already finished and redirected an earlier
// drop or reload. The lane holds each switch until everything the client
// stamped before it has reached the host machine.

use std::collections::VecDeque;

use postretro_foundation::ActivationToken;
use postretro_net::wire::ClientSwitchDeclaration;

use super::press_edges::PressGate;
use super::{CLIENT_TICK_HALF_RANGE, ClientCommandState};
use crate::activation_edges::{MAX_RETAINED_ACTIVATION_EDGES, RETENTION_TICKS};
use crate::prediction::client_tick_le;

/// Per-client bound, the same as the start, edge and press lanes. A client
/// switches at most once per command, so 64 unanswered switches take over a
/// second of switching every tick through a stall.
pub(super) const MAX_RETAINED_SWITCHES: usize = MAX_RETAINED_ACTIVATION_EDGES;

/// Client ticks a switch may be stamped ahead of the newest command received.
/// The client sends the switch just before the command it was made on, on
/// another channel, so a legitimate lead is about one Input resend. A tick
/// further ahead is malformed: it would wait for a command that is not coming.
pub(crate) const SWITCH_LEAD_TICKS: u32 = RETENTION_TICKS;

/// Host ticks a retained switch may wait from intake before it applies
/// whatever is still pending ahead of it, as a switch did before it carried a
/// tick. Covers the largest legitimate lead plus the start lane's two-second
/// retention, so an ordinary stall never reaches it; a stopped stream does.
pub(crate) const SWITCH_WAIT_TICKS: u32 = SWITCH_LEAD_TICKS + RETENTION_TICKS;

/// What a host fixed tick does with a switch the lane released.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SwitchDelivery {
    /// Validate against the live inventory, apply, and reply accepted or refused.
    Apply(ClientSwitchDeclaration),
    /// Its client tick was malformed: reply refused without touching inventory.
    Refuse(ClientSwitchDeclaration),
}

#[derive(Debug, Clone, Copy)]
struct RetainedSwitch {
    declaration: ClientSwitchDeclaration,
    malformed: bool,
    /// Host tick of intake, for `SWITCH_WAIT_TICKS`.
    retained_at: u32,
}

impl RetainedSwitch {
    fn delivery(self) -> SwitchDelivery {
        if self.malformed {
            SwitchDelivery::Refuse(self.declaration)
        } else {
            SwitchDelivery::Apply(self.declaration)
        }
    }
}

/// Retained switches in arrival order, and the deliveries released but not yet
/// applied. Replies leave in arrival order: the client settles its switch chain
/// front-first and ignores an outcome for any other declaration.
#[derive(Debug, Default)]
pub(super) struct SwitchLane {
    retained: VecDeque<RetainedSwitch>,
    released: Vec<SwitchDelivery>,
}

impl SwitchLane {
    pub(super) fn take_released(&mut self) -> Vec<SwitchDelivery> {
        std::mem::take(&mut self.released)
    }
}

impl ClientCommandState {
    /// Retain a switch in arrival order. A tick stamped more than
    /// `SWITCH_LEAD_TICKS` ahead of the newest command received is kept only to
    /// be refused in order. At the bound the oldest switch is released at once,
    /// unordered, so a reply is never lost.
    pub(super) fn retain_switch(&mut self, declaration: ClientSwitchDeclaration) {
        let malformed = self.newest_observed_tick().is_some_and(|newest| {
            let lead = declaration.client_tick.wrapping_sub(newest);
            lead > SWITCH_LEAD_TICKS && lead < CLIENT_TICK_HALF_RANGE
        });
        if self.switches.retained.len() >= MAX_RETAINED_SWITCHES
            && let Some(oldest) = self.switches.retained.pop_front()
        {
            self.switches.released.push(oldest.delivery());
        }
        self.switches.retained.push_back(RetainedSwitch {
            declaration,
            malformed,
            retained_at: self.host_tick,
        });
    }

    /// Release this resolution's switch, before any start or press it carries,
    /// so a start or press stamped on the switch's own command lands after it,
    /// as the client's equip pass ran before its fire. At most one switch
    /// applies per resolution; malformed ones are refused as they reach the front.
    ///
    /// Bound: a switch applies once its tick has resolved and nothing older is
    /// pending, and never later than `SWITCH_WAIT_TICKS` host ticks after intake.
    pub(super) fn deliver_switches(&mut self, live: Option<ActivationToken>) {
        let mut applied = false;
        while let Some(front) = self.switches.retained.front().copied() {
            if !front.malformed && (applied || !self.switch_ready(front, live)) {
                break;
            }
            self.switches.retained.pop_front();
            self.switches.released.push(front.delivery());
            applied |= !front.malformed;
        }
    }

    /// Whether `switch` may apply now: everything the client stamped before it
    /// has reached the host machine. That is its own command received and
    /// resolved, no older retained start, no older due press, and the live
    /// execution's clock at its tick, so every shot the client fired before
    /// switching is minted first.
    fn switch_ready(&self, switch: RetainedSwitch, live: Option<ActivationToken>) -> bool {
        if self.host_tick.wrapping_sub(switch.retained_at) >= SWITCH_WAIT_TICKS {
            return true;
        }
        // No command ever received: nothing to order against.
        let Some(newest) = self.newest_observed_tick() else {
            return true;
        };
        let tick = switch.declaration.client_tick;
        let Some(cursor) = self.resolved_cursor else {
            return false;
        };
        if !client_tick_le(tick, cursor) || !client_tick_le(tick, newest) {
            return false;
        }
        let older = |other: u32| !client_tick_le(tick, other);
        !self.activation_edges.oldest_start_tick().is_some_and(older)
            && !self.oldest_due_press(cursor).is_some_and(older)
            && PressGate::through(self.live_horizon(live)).admits(tick)
    }

    /// Client ticks through which starts and presses may go while a switch is
    /// retained: those stamped before it. One stamped on the switch's own
    /// command rides with it once it is released.
    pub(super) fn switch_gate(&self) -> PressGate {
        let oldest = self
            .switches
            .retained
            .iter()
            .filter(|switch| !switch.malformed)
            .map(|switch| switch.declaration.client_tick)
            .reduce(|oldest, tick| {
                if client_tick_le(tick, oldest) {
                    tick
                } else {
                    oldest
                }
            });
        PressGate::through(oldest.map(|tick| tick.wrapping_sub(1)))
    }
}

#[cfg(test)]
mod tests {
    use super::super::*;
    use super::{MAX_RETAINED_SWITCHES, SWITCH_LEAD_TICKS, SWITCH_WAIT_TICKS, SwitchDelivery};
    use postretro_entities::EntityId;
    use postretro_foundation::{
        ActivationId, ActivationLane, ActivationProgram, ActivationStep, ActivationToken,
    };
    use postretro_net::wire::{ClientSwitchDeclaration, WireActivationToken};

    const CLIENT: u64 = 7;

    fn command(client_tick: u32) -> InputCommand {
        let mut command = crate::netcode::wire_convert::sim_command_to_input(
            &neutral_sim_command(0.0),
            client_tick,
            0.0,
        );
        command.movement.wish_dir = [1.0, 0.0];
        command
    }

    fn start(command: &mut InputCommand) {
        command.activation.initiation = Some(WireActivationToken {
            start_tick: command.client_tick,
            lane: 0,
        });
    }

    fn switch(declaration_id: u32, slot: u8, client_tick: u32) -> ClientSwitchDeclaration {
        ClientSwitchDeclaration {
            declaration_id,
            slot,
            client_tick,
        }
    }

    /// One host fixed tick: resolve, then take what the lane released.
    fn tick(queues: &mut HostCommandQueues) -> (ResolvedCommand, Vec<SwitchDelivery>) {
        let resolved = queues.resolve_tick(CLIENT).expect("the stream resolves");
        (resolved, queues.take_switch_deliveries(CLIENT))
    }

    /// A host stall: `backlog` lands at once, then one new command arrives per
    /// fixed tick, as a live client keeps sending.
    fn stall(
        queues: &mut HostCommandQueues,
        backlog: impl IntoIterator<Item = InputCommand>,
        resolutions: u32,
    ) -> Vec<(ResolvedCommand, Vec<SwitchDelivery>)> {
        let mut next = 0;
        for command in backlog {
            next = command.client_tick + 1;
            assert!(queues.ingest(CLIENT, &command));
        }
        (0..resolutions)
            .map(|offset| {
                assert!(queues.ingest(CLIENT, &command(next + offset)));
                tick(queues)
            })
            .collect()
    }

    /// Prime a stream through `through`, disarming the buildup latch.
    fn primed(through: u32) -> HostCommandQueues {
        let mut queues = HostCommandQueues::new();
        for tick in 0..=through {
            assert!(queues.ingest(CLIENT, &command(tick)));
        }
        while queues.resolved_cursor(CLIENT) != Some(through - 1) {
            queues.resolve_tick(CLIENT);
        }
        queues
    }

    #[test]
    fn a_switch_with_nothing_older_pending_applies_on_its_own_tick() {
        let mut queues = primed(4);
        let declared = switch(1, 2, 6);
        queues.retain_switch(CLIENT, declared);
        assert!(queues.ingest(CLIENT, &command(5)));
        assert!(queues.ingest(CLIENT, &command(6)));
        let mut released = Vec::new();
        loop {
            let (resolved, deliveries) = tick(&mut queues);
            if !deliveries.is_empty() {
                released.push((resolved.client_tick, deliveries));
            }
            if resolved.client_tick == 6 {
                break;
            }
        }
        assert_eq!(
            released,
            vec![(6, vec![SwitchDelivery::Apply(declared)])],
            "released on the resolution of the command it was made on"
        );
    }

    #[test]
    fn a_switch_waits_for_an_older_retained_start_and_a_later_start_waits_for_it() {
        let mut queues = primed(1);
        let declared = switch(1, 1, 6);
        // A host stall: the whole backlog lands at once, deep enough to trim.
        queues.retain_switch(CLIENT, declared);
        let backlog = (2..14).map(|tick| {
            let mut command = command(tick);
            if tick == 3 || tick == 9 {
                start(&mut command);
            }
            command
        });
        let mut order = Vec::new();
        for (resolved, deliveries) in stall(&mut queues, backlog, 20) {
            if !deliveries.is_empty() {
                order.push("switch");
            }
            match resolved.command.activation.initiation {
                Some(token) if token.start_tick == 3 => order.push("start 3"),
                Some(token) if token.start_tick == 9 => order.push("start 9"),
                _ => {}
            }
        }
        assert_eq!(order, vec!["start 3", "switch", "start 9"]);
    }

    #[test]
    fn a_switch_waits_for_an_older_drop_press_which_keeps_its_own_slot() {
        let mut queues = primed(1);
        let declared = switch(1, 0, 6);
        queues.retain_switch(CLIENT, declared);
        let backlog = (2..14).map(|tick| {
            let mut command = command(tick);
            command.movement.firing_slot = u8::from(tick < 6);
            command.movement.drop_pressed = tick == 4;
            command.reload = tick == 3;
            if tick == 8 {
                start(&mut command);
            }
            command
        });
        let mut order = Vec::new();
        for (resolved, deliveries) in stall(&mut queues, backlog, 20) {
            if resolved.command.reload {
                order.push(("reload", resolved.command.firing_slot));
            }
            if resolved.command.drop_pressed {
                order.push(("drop", resolved.command.firing_slot));
            }
            if !deliveries.is_empty() {
                order.push(("switch", 0));
            }
            if resolved.command.activation.initiation.is_some() {
                order.push(("start", resolved.command.firing_slot));
            }
        }
        assert_eq!(
            order,
            vec![("reload", 1), ("drop", 1), ("switch", 0), ("start", 0)],
            "slot 1's reload and drop reach the host before the switch to slot 0"
        );
    }

    #[test]
    fn a_press_stamped_on_the_switch_command_rides_with_it_and_a_later_one_waits() {
        let mut queues = primed(1);
        let first = switch(1, 1, 5);
        let second = switch(2, 0, 7);
        queues.retain_switch(CLIENT, first);
        queues.retain_switch(CLIENT, second);
        // A start at 3 holds the first switch back past both presses' ticks.
        let backlog = (2..14).map(|tick| {
            let mut command = command(tick);
            command.movement.use_pressed = tick == 5 || tick == 8;
            if tick == 3 {
                start(&mut command);
            }
            command
        });
        let mut order = Vec::new();
        for (resolved, deliveries) in stall(&mut queues, backlog, 20) {
            for delivery in deliveries {
                let SwitchDelivery::Apply(declaration) = delivery else {
                    panic!("well-formed switches apply");
                };
                order.push(format!("switch {}", declaration.declaration_id));
            }
            if resolved.command.use_pressed {
                order.push("use".to_owned());
            }
        }
        assert_eq!(order, ["switch 1", "use", "switch 2", "use"]);
    }

    #[test]
    fn a_blocked_switch_applies_within_its_wait_bound() {
        let mut queues = primed(4);
        // Stamped on a command that never arrives: the client went silent.
        let declared = switch(1, 1, 10);
        queues.retain_switch(CLIENT, declared);
        let mut waited = 0;
        loop {
            queues.resolve_tick(CLIENT);
            waited += 1;
            if !queues.take_switch_deliveries(CLIENT).is_empty() {
                break;
            }
            assert!(
                waited <= SWITCH_WAIT_TICKS,
                "the switch waits past its bound"
            );
        }
        assert_eq!(waited, SWITCH_WAIT_TICKS);
    }

    #[test]
    fn a_switch_from_a_client_with_no_stream_applies_on_the_next_tick() {
        let mut queues = HostCommandQueues::new();
        let declared = switch(1, 1, 500);
        queues.retain_switch(CLIENT, declared);
        assert!(queues.resolve_tick(CLIENT).is_none(), "nothing to resolve");
        assert_eq!(
            queues.take_switch_deliveries(CLIENT),
            vec![SwitchDelivery::Apply(declared)]
        );
    }

    #[test]
    fn a_tick_stamped_past_the_lead_is_refused_in_arrival_order() {
        let mut queues = primed(4);
        let late = switch(1, 1, 6);
        let malformed = switch(2, 2, 4 + SWITCH_LEAD_TICKS + 1);
        let at_lead = switch(3, 0, 4 + SWITCH_LEAD_TICKS);
        // Wrap-aware: a tick behind the stream by half the range is late, not ahead.
        let behind = switch(4, 1, 4u32.wrapping_sub(1 << 30));
        for declared in [late, malformed, at_lead, behind] {
            queues.retain_switch(CLIENT, declared);
        }
        let mut deliveries = Vec::new();
        for tick in 5..7 {
            assert!(queues.ingest(CLIENT, &command(tick)));
        }
        for _ in 0..4 {
            deliveries.extend(tick(&mut queues).1);
        }
        assert_eq!(
            deliveries,
            vec![
                SwitchDelivery::Apply(late),
                SwitchDelivery::Refuse(malformed)
            ],
            "the malformed switch is refused right behind its predecessor"
        );
        assert!(
            !deliveries.contains(&SwitchDelivery::Apply(at_lead)),
            "a tick at the lead bound is well-formed and waits for its command"
        );
    }

    #[test]
    fn the_switch_lane_is_bounded_and_releases_its_oldest_at_overflow() {
        let mut queues = primed(4);
        for id in 0..=MAX_RETAINED_SWITCHES as u32 {
            queues.retain_switch(CLIENT, switch(id, 1, 100 + id));
        }
        assert_eq!(
            queues.take_switch_deliveries(CLIENT),
            vec![SwitchDelivery::Apply(switch(0, 1, 100))],
            "the 65th switch releases the oldest, unordered"
        );
    }

    #[test]
    fn a_live_execution_holds_a_switch_until_its_clock_reaches_the_switch() {
        let mut queues = primed(1);
        let token = ActivationToken {
            start_tick: 3,
            lane: ActivationLane::Primary,
        };
        let declared = switch(1, 1, 6);
        queues.retain_switch(CLIENT, declared);
        for tick in 2..14 {
            let mut command = command(tick);
            if tick == 3 {
                start(&mut command);
            }
            assert!(queues.ingest(CLIENT, &command));
        }
        let mut delivered_at = None;
        let mut switched_at = None;
        for host in 0..20 {
            assert!(queues.ingest(CLIENT, &command(14 + host)));
            let (resolved, deliveries) = tick(&mut queues);
            if resolved.command.activation.initiation == Some(token) {
                delivered_at = Some(host);
                let program = ActivationProgram::new(
                    vec![
                        ActivationStep::Shot,
                        ActivationStep::Wait { ticks: 20 },
                        ActivationStep::Shot,
                    ],
                    None,
                    36,
                )
                .unwrap();
                let id = ActivationId { pawn: 1, token };
                let weapon = EntityId::from_raw(9);
                assert!(queues.activations.accept(CLIENT, id, weapon, &program, 0));
                queues.activation_admitted(CLIENT, token, false);
                queues.activation_recovery_began(CLIENT, token, weapon, 36, false);
            }
            if !deliveries.is_empty() {
                switched_at = Some(host);
            }
        }
        let (delivered_at, switched_at) = (delivered_at.unwrap(), switched_at.unwrap());
        assert_eq!(
            switched_at - delivered_at,
            6 - 3,
            "the switch waits until the execution's clock reaches its tick"
        );
    }
}
